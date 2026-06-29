# `paavo-cli board add`: accept probe-rs selector strings directly

**Date:** 2026-06-29
**Status:** Approved (design)
**Crates:** `paavo-proto` (parser + type), `paavo-cli` (wiring), `paavod` (validation), `paavo-probe` (attach plumbing)

## Problem

Registering a board today means hand-massaging the output of `probe-rs list`
into `paavo-cli board add --probe VID:PID:serial`, and the CLI parses that with a
naive split (`crates/paavo-cli/src/cmd_boards.rs:35`):

```rust
let mut parts = probe.split(':');
let vid = parts.next()...; let pid = parts.next()...; let serial = parts.next()...;
```

That is wrong in three ways against what `probe-rs list` actually prints:

```
[0]: MCU-LINK on-board (r2E4) CMSIS-DAP V3.172 -- 1fc9:0143-0:EDFHUAFM4J5ZJ (CMSIS-DAP)
```

1. **The `-0` is the USB *interface* index**, not part of the PID. probe-rs's
   selector grammar is `VID:PID[-INTERFACE][:SERIAL]` (see
   `probe-rs/src/probe/selector.rs::from_str`). The naive split stores
   `pid = "0143-0"`, which then fails `u16::from_str_radix(..,16)`.
2. **Serials can contain colons.** probe-rs uses `splitn(3, ':')` precisely
   because some serials are colon-bearing (ESP JTAG uses a MAC, e.g.
   `303a:1001:DC:DA:0C:D3:FE:D8`). `split(':')` truncates them.
3. **Failure is discovered far too late.** VID/PID are stored as raw strings and
   only parsed as hex at `probe_attach` (`crates/paavo-probe/src/session.rs:140`,
   `:146`). A bad selector therefore survives registration, queues a job, builds
   the crate, claims a board, and only *then* fails with `infra_err` — an entire
   build+flash cycle wasted on a typo.

Operators will copy-paste `probe-rs list` output. The tool should understand it.

## Goals

- `paavo-cli board add --probe ...` accepts **either** a bare selector token
  (`1fc9:0143-0:EDFHUAFM4J5ZJ`) **or** a full pasted `probe-rs list` line, and
  normalizes it.
- Match probe-rs's own grammar: `splitn(3, ':')`, hex VID/PID, optional
  `-INTERFACE`, colon-bearing serials.
- **Preserve the USB interface** (the `-N`) so multi-interface probes (FTDI
  dual-channel `0403:6010-1`) remain addressable.
- **Fail fast.** A malformed selector is rejected at `board add` (locally, before
  the network round-trip) *and* at `POST /boards` (so any client, including
  `curl`, gets a `400` instead of a row that fails at attach).
- Canonicalize stored VID/PID to lowercase 4-hex (`0X1FC9` → `1fc9`).

## Non-goals (YAGNI)

- **No daemon-side probe discovery.** A `paavo-cli board discover` that runs
  `probe-rs list` on the lab box and offers a numbered pick is a worthwhile
  *future* follow-up (no pasting at all), but it needs a new daemon endpoint that
  pokes hardware plus its own auth story. Out of scope here; noted in
  [Future work](#future-work-out-of-scope-here).
- **Serial stays a `String` (empty = "no filter").** probe-rs distinguishes "no
  serial" (`None`) from "match probes with empty serial" (`Some("")`); paavo
  collapses both to `""` (`crates/paavo-probe/src/session.rs:153`). Preserving
  that distinction is extra wire surface for a case the lab's serial-bearing
  CMSIS-DAP probes never hit. Left as-is.
- **No `boards.toml` grammar change.** `boards.toml` is structured TOML
  (`vid = "1fc9"`, and now optionally `interface = 0`), not a probe-rs token. The
  new parser is a `board add` convenience only; TOML authors get the optional
  field for free via serde.
- **No consolidation of `paavo-probe::parse_hex_u16`** into the new proto helper.
  Both will tolerate `0x`/whitespace and parse base-16 identically; deduping them
  is a tidy-up that can come later.

## Decisions

| Question | Decision |
|----------|----------|
| Accepted input forms | Bare selector token **and** full `probe-rs list` line. |
| Where the parser lives | `paavo-proto` (`ProbeSelector::parse`) — it owns the type; both CLI and daemon depend on proto. |
| The `-INTERFACE` index | **Preserved** as a new optional `ProbeSelector::interface: Option<u8>`, threaded to probe-rs at attach. |
| Validation scope | **CLI + daemon.** CLI parses/normalizes for instant local feedback; `POST /boards` re-validates (hex VID/PID) → `400`. |
| VID/PID normalization | Lowercase 4-hex via `format!("{:04x}", u16)`; tolerate `0x`/whitespace. |
| Serial | `String`, may be empty, may contain `:` (`splitn(3, ':')`). |
| Wire compatibility | `ProbeSelector` has **no** `#[serde(deny_unknown_fields)]`, so the new field is additive both ways: old JSON → `None` (serde default); new JSON → old readers ignore it. No DB migration (JSON in a `TEXT` column). |
| Error type | `ProbeSelectorParseError` (thiserror) in proto; CLI wraps with `anyhow` context; daemon maps to `400`. |

## Design

### 1. Wire type — `crates/paavo-proto/src/board.rs`

Add one optional field, mirroring the existing `BoardSelector` optional-field
style (`board.rs:55-59`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSelector {
    /// USB vendor id, normalized lowercase 4-hex, e.g. `"1fc9"`.
    pub vid: String,
    /// USB product id, normalized lowercase 4-hex, e.g. `"0143"`.
    pub pid: String,
    /// Probe serial number as reported by USB. May be empty ("no filter")
    /// and may contain `:` (e.g. ESP JTAG MAC serials).
    pub serial: String,
    /// USB interface index (the `-N` in a probe-rs selector). `None` matches
    /// any interface; set only for multi-interface probes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<u8>,
}
```

`skip_serializing_if` keeps existing rows and the byte-level wire-compat fixtures
identical when `interface` is `None`.

### 2. Parser — `crates/paavo-proto/src/board.rs`

```rust
impl ProbeSelector {
    /// Parse a probe-rs selector token OR a full `probe-rs list` line.
    pub fn parse(input: &str) -> Result<Self, ProbeSelectorParseError>;

    /// Validate that vid/pid are hex u16 (used by paavod at registration).
    pub fn validate(&self) -> Result<(), ProbeSelectorParseError>;
}
```

`parse` algorithm:

1. **Extract the token.** Trim. If the input contains ` -- ` (a `probe-rs list`
   line: `[N]: <identifier> -- <selector> (<TYPE>)`), take the substring after
   the **last** ` -- `, then strip a trailing ` (...)` parenthetical, then trim.
   Otherwise use the trimmed input as the token.
2. **Split the token** with `splitn(3, ':')` → `vid`, `pid_iface`, `serial?`
   (3-way split so colons inside the serial survive).
3. **Interface.** `pid_iface.split_once('-')`: if present, `pid` is the left
   side and the right side is the interface — empty (the `VID:PID-` case) →
   `None`, else `u8::from_str` → `Some`. If no `-`, `pid = pid_iface`,
   `interface = None`.
4. **Hex VID/PID.** Parse each as `u16` base-16 (tolerating a `0x`/`0X` prefix
   and surrounding whitespace, matching `paavo-probe`'s `parse_hex_u16`).
   Re-emit normalized as `format!("{:04x}", n)`.
5. **Serial.** The third `splitn` part verbatim if present, else `""`.
6. Return `ProbeSelector { vid, pid, serial, interface }`.

`validate` re-runs step 4's hex check on the struct's existing `vid`/`pid`
strings (the daemon receives an already-structured selector over the wire).

```rust
#[derive(thiserror::Error, Debug)]
pub enum ProbeSelectorParseError {
    #[error("selector is empty or missing the PID (expected `VID:PID[-IFACE][:SERIAL]`, \
             e.g. `1fc9:0143:ABCD1234`)")]
    Format,
    #[error("bad VID {value:?}: {source}")]
    BadVid { value: String, source: std::num::ParseIntError },
    #[error("bad PID {value:?}: {source}")]
    BadPid { value: String, source: std::num::ParseIntError },
    #[error("bad USB interface {value:?}: {source}")]
    BadInterface { value: String, source: std::num::ParseIntError },
}
```

### 3. CLI wiring — `crates/paavo-cli/src/cmd_boards.rs`

Replace the `probe.split(':')` block (`cmd_boards.rs:35-47`) with the proto
parser, failing locally before any network call:

```rust
let probe_selector = ProbeSelector::parse(&probe).map_err(|e| {
    anyhow!(
        "invalid --probe {probe:?}: {e}\n\n\
         Paste a probe-rs selector (`1fc9:0143:SERIAL`) or a full `probe-rs list` line."
    )
})?;
let spec = BoardSpec { id: instance, kind, probe_selector, chip_name: chip,
                       target_name: target, wiring_profile: Some(wiring_profile),
                       health: BoardHealth::Healthy };
```

Update the `--probe` help (`crates/paavo-cli/src/cli.rs:145`) from `VID:PID:serial`
to note both accepted forms.

### 4. Daemon validation — `crates/paavod/src/routes/boards.rs::add_board`

After the existing `health == Healthy` check and before insert
(`routes/boards.rs:56-60`):

```rust
spec.probe_selector
    .validate()
    .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid probe_selector: {e}")))?;
```

So a raw `curl` POST with a non-hex VID/PID is rejected at registration, not at
attach.

### 5. Attach plumbing — `crates/paavo-probe/src/session.rs`

Where the probe-rs `DebugProbeSelector` is constructed (`session.rs:161`), pass
the interface through instead of the implicit/`None` it uses today:

```rust
let selector = DebugProbeSelector {
    vendor_id: vid,
    product_id: pid,
    serial_number: serial_filter,
    interface: opts.probe_selector.interface,
};
```

VID/PID/serial plumbing is unchanged; `parse_hex_u16` still runs (and now always
succeeds, because registration validated).

### 6. Existing data / migration

- No schema migration: `ProbeSelector` is JSON in the `board.probe_selector`
  `TEXT` column (`crates/paavo-db/migrations/V1__initial.sql:11`). Old rows
  deserialize with `interface = None` via serde default.
- The previously hand-fixed `mcxa266-01` row (`pid = "0143"`, no interface)
  remains valid; no backfill needed.

## Testing / verification

### `paavo-proto` unit tests (`#[cfg(test)]` in `board.rs`)

`parse` cases:

1. `bare_token` — `1fc9:0143:EDFHUAFM4J5ZJ` → vid/pid/serial, `interface == None`.
2. `with_interface` — `1fc9:0143-0:EDFHUAFM4J5ZJ` → `interface == Some(0)`.
3. `empty_interface` — `1fc9:0143-:S` → `interface == None`.
4. `colon_serial` — `303a:1001:DC:DA:0C:D3:FE:D8` → serial kept whole.
5. `no_serial` — `1fc9:0143` → serial `""`, `interface == None`.
6. `full_list_line` — `[0]: MCU-LINK on-board (r2E4) CMSIS-DAP V3.172 -- 1fc9:0143-0:EDFHUAFM4J5ZJ (CMSIS-DAP)`
   → `{ vid:"1fc9", pid:"0143", serial:"EDFHUAFM4J5ZJ", interface:Some(0) }`.
7. `normalization` — `0X1FC9:143` and ` 1fc9 : 0143 ` → `vid:"1fc9", pid:"0143"`.
8. errors — bad hex VID (`zz:0143:S` → `BadVid`), bad PID (`1fc9:gg:S` → `BadPid`),
   missing PID (`1fc9` → `Format`), bad interface (`1fc9:0143-x:S` → `BadInterface`).

Wire-compat:

9. `roundtrip_none` — `ProbeSelector` with `interface: None` serializes to the
   pre-existing JSON byte-for-byte (extend/keep the existing fixture).
10. `roundtrip_some` — new fixture: `interface: Some(0)` adds exactly the
    `"interface":0` key and round-trips.

### `paavod` (`crates/paavod/...` route test)

- `add_board_rejects_bad_selector` — `POST /boards` with `vid:"zz"` ⇒ `400`.

### `paavo-cli` (`crates/paavo-cli/tests/`, `assert_cmd` + `predicates`)

- `board_add_accepts_full_list_line` — `board add --probe '[0]: ... -- 1fc9:0143-0:SER (CMSIS-DAP)' ...`
  against the existing fake-daemon harness registers a board whose selector is
  `{ vid:"1fc9", pid:"0143", serial:"SER", interface:Some(0) }`.
- `board_add_rejects_garbage_probe` — a non-hex `--probe` exits non-zero with the
  helpful message, **without** contacting the daemon.

### Full gate (CI parity, from `AGENTS.md`)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All green (`RUSTFLAGS="-Dwarnings"` — no warnings allowed).

## Affected files

- `crates/paavo-proto/src/board.rs` — `interface` field; `ProbeSelector::parse`
  + `validate`; `ProbeSelectorParseError`; unit tests + wire-compat fixtures.
- `crates/paavo-cli/src/cmd_boards.rs` — use `ProbeSelector::parse`.
- `crates/paavo-cli/src/cli.rs` — reword `--probe` help.
- `crates/paavod/src/routes/boards.rs` — `validate()` → `400` in `add_board`.
- `crates/paavo-probe/src/session.rs` — pass `interface` into `DebugProbeSelector`.
- `crates/paavo-cli/tests/…` — CLI accept/reject tests.

## Future work (out of scope here)

1. **`paavo-cli board discover`** — paavod runs `probe-rs list` on the lab box and
   returns a numbered inventory; the operator picks an index, no pasting. Needs a
   new daemon endpoint that touches hardware plus an auth story for it.
2. **Consolidate hex parsing** — fold `paavo-probe::parse_hex_u16` into the
   `paavo-proto` helper so there is a single base-16 VID/PID parser.

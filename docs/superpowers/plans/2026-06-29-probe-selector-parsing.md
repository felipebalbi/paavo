# probe-rs-native selector parsing for `board add` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `paavo-cli board add --probe` accept probe-rs selector strings (bare token *or* a full `probe-rs list` line) directly, parse/validate them at registration, and preserve the USB interface index.

**Architecture:** A canonical parser lives in `paavo-proto` on `ProbeSelector` (the type's owner). `paavo-cli` calls it to normalize before POST; `paavod` re-validates at `POST /boards`. `ProbeSelector` gains an optional `interface: Option<u8>` field (additive, no `deny_unknown_fields`, no DB migration) that `paavo-probe` threads into probe-rs's `DebugProbeSelector` at attach.

**Tech Stack:** Rust 1.95, serde/serde_json, thiserror, axum (paavod), assert_cmd + predicates (CLI tests). Spec: `docs/superpowers/specs/2026-06-29-probe-selector-parsing-design.md`.

---

## File structure

- `crates/paavo-proto/src/board.rs` — add `interface` field; `ProbeSelectorParseError`; `ProbeSelector::parse` + `validate`; `parse_hex_u16` helper + `extract_selector_token` helper; `#[cfg(test)] mod tests`.
- `crates/paavo-proto/tests/serde_roundtrip.rs` — wire-compat tests for the new field.
- `crates/paavo-cli/src/cmd_boards.rs` — replace the naive `split(':')` with `ProbeSelector::parse`.
- `crates/paavo-cli/src/cli.rs` — reword `--probe` help.
- `crates/paavo-cli/tests/board_add_selector.rs` — new CLI fail-fast test.
- `crates/paavod/src/routes/boards.rs` — `validate()` → `400` in `add_board`.
- `crates/paavod/tests/api_boards.rs` — daemon rejects bad selector.
- `crates/paavo-probe/src/session.rs` — pass `interface` into `DebugProbeSelector`.
- Many test/src files — add `interface: None,` to existing `ProbeSelector { … }` literals (Task 1, compiler-driven).
- `AGENTS.md`, `docs/deployment.md` — doc fixes (Task 7).

---

### Task 1: Add the `interface` field and repair every `ProbeSelector` literal

Adding a field breaks all struct-literal construction sites. The new field is optional on the wire (serde default + skip-when-none), so behavior is unchanged; this task is purely the type change + mechanical literal repair, gated by a wire-compat test.

**Files:**
- Modify: `crates/paavo-proto/src/board.rs:6-14`
- Test: `crates/paavo-proto/tests/serde_roundtrip.rs` (append)
- Modify (add `interface: None,`): every in-workspace `ProbeSelector { … }` literal (list below)

- [ ] **Step 1: Write the failing wire-compat tests**

Append to `crates/paavo-proto/tests/serde_roundtrip.rs`:

```rust
#[test]
fn probe_selector_omits_interface_when_none() {
    let sel = paavo_proto::ProbeSelector {
        vid: "1fc9".into(),
        pid: "0143".into(),
        serial: "ABCD".into(),
        interface: None,
    };
    let j = serde_json::to_string(&sel).unwrap();
    assert!(!j.contains("interface"), "None interface must be omitted: {j}");
    assert_eq!(sel, serde_json::from_str(&j).unwrap());
}

#[test]
fn probe_selector_serializes_interface_when_some() {
    let sel = paavo_proto::ProbeSelector {
        vid: "0403".into(),
        pid: "6010".into(),
        serial: "X".into(),
        interface: Some(1),
    };
    let j = serde_json::to_string(&sel).unwrap();
    assert!(j.contains("\"interface\":1"), "Some interface must serialize: {j}");
    assert_eq!(sel, serde_json::from_str(&j).unwrap());
}
```

- [ ] **Step 2: Run — expect a COMPILE failure (field doesn't exist yet)**

Run: `cargo test -p paavo-proto --test serde_roundtrip`
Expected: FAIL to compile — `struct ProbeSelector has no field named interface`.

- [ ] **Step 3: Add the field**

In `crates/paavo-proto/src/board.rs`, replace the struct body (lines 6-14):

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
    /// any interface; set only for multi-interface probes (e.g. FTDI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<u8>,
}
```

- [ ] **Step 4: Add `interface: None,` to every existing literal (compiler-driven)**

Run `cargo build --workspace --all-targets` and add `interface: None,` to each `ProbeSelector { … }` the compiler flags (E0063). The full in-workspace set:

```
crates/paavo-cli/tests/cli_run_board_resolution.rs:30
crates/paavo-cli/src/cmd_run.rs:386
crates/paavo-cli/src/cmd_boards.rs:51        (superseded in Task 4, but must compile now)
crates/paavo-core/tests/common/mod.rs:44
crates/paavo-db/src/job.rs:927
crates/paavo-db/tests/board_ops.rs:18
crates/paavo-db/tests/job_ops.rs:24
crates/paavo-proto/src/stats.rs:214
crates/paavo-proto/tests/board_view.rs:9
crates/paavo-proto/tests/board_view.rs:42
crates/paavo-proto/tests/serde_roundtrip.rs:169
crates/paavo-probe/tests/real_session_ram_boot.rs:186
crates/paavo-probe/tests/real_session_connect.rs:71
crates/paavo-probe/tests/real_session_drive.rs:65
crates/paavod/tests/shutdown_flow.rs:25
crates/paavod/tests/real_runner_end_to_end.rs:124
crates/paavod/tests/api_jobs.rs:24
crates/paavod/tests/api_boards.rs:234
crates/paavod/tests/api_boards.rs:369
crates/paavod/tests/dispatch_loop.rs:60
crates/paavod/tests/api_admin.rs:67
crates/paavod/tests/cron_enqueue.rs:44
crates/paavo-web/tests/api_dashboard.rs:42
```

Example edit (each site looks like this):

```rust
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "ABC".into(),
            interface: None,   // <-- add this line
        },
```

> Note: `dev/seed-demo/src/main.rs:375` also constructs `ProbeSelector` but is **workspace-excluded** (not built by `cargo build --workspace`); add `interface: None,` there too only if you build that dev tool, but it is out of CI scope.

- [ ] **Step 5: Run — workspace compiles, wire-compat tests pass**

Run: `cargo test --workspace`
Expected: PASS (the two new tests included; `interface` omitted when `None`, present when `Some`).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(proto): add optional interface to ProbeSelector (additive wire field)"
```

---

### Task 2: Selector parser + validator in `paavo-proto`

**Files:**
- Modify: `crates/paavo-proto/src/board.rs` (add error enum, `impl ProbeSelector`, two free helpers, tests module)

- [ ] **Step 1: Write the failing unit tests**

Append to `crates/paavo-proto/src/board.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_token() {
        let s = ProbeSelector::parse("1fc9:0143:EDFHUAFM4J5ZJ").unwrap();
        assert_eq!(
            s,
            ProbeSelector {
                vid: "1fc9".into(),
                pid: "0143".into(),
                serial: "EDFHUAFM4J5ZJ".into(),
                interface: None,
            }
        );
    }

    #[test]
    fn parse_with_interface() {
        let s = ProbeSelector::parse("1fc9:0143-0:EDFHUAFM4J5ZJ").unwrap();
        assert_eq!(s.pid, "0143");
        assert_eq!(s.interface, Some(0));
    }

    #[test]
    fn parse_empty_interface_is_none() {
        let s = ProbeSelector::parse("1fc9:0143-:S").unwrap();
        assert_eq!(s.interface, None);
        assert_eq!(s.serial, "S");
    }

    #[test]
    fn parse_colon_serial_preserved() {
        let s = ProbeSelector::parse("303a:1001:DC:DA:0C:D3:FE:D8").unwrap();
        assert_eq!(s.vid, "303a");
        assert_eq!(s.pid, "1001");
        assert_eq!(s.serial, "DC:DA:0C:D3:FE:D8");
    }

    #[test]
    fn parse_no_serial_is_empty() {
        let s = ProbeSelector::parse("1fc9:0143").unwrap();
        assert_eq!(s.serial, "");
        assert_eq!(s.interface, None);
    }

    #[test]
    fn parse_normalizes_hex() {
        let s = ProbeSelector::parse("0X1FC9:143:S").unwrap();
        assert_eq!(s.vid, "1fc9");
        assert_eq!(s.pid, "0143");
    }

    #[test]
    fn parse_rejects_bad_vid() {
        assert!(matches!(
            ProbeSelector::parse("zz:0143:S"),
            Err(ProbeSelectorParseError::BadVid { .. })
        ));
    }

    #[test]
    fn parse_rejects_bad_pid() {
        assert!(matches!(
            ProbeSelector::parse("1fc9:gg:S"),
            Err(ProbeSelectorParseError::BadPid { .. })
        ));
    }

    #[test]
    fn parse_rejects_missing_pid() {
        assert!(matches!(
            ProbeSelector::parse("1fc9"),
            Err(ProbeSelectorParseError::Format)
        ));
    }

    #[test]
    fn parse_rejects_bad_interface() {
        assert!(matches!(
            ProbeSelector::parse("1fc9:0143-x:S"),
            Err(ProbeSelectorParseError::BadInterface { .. })
        ));
    }

    #[test]
    fn validate_accepts_normalized() {
        let s = ProbeSelector {
            vid: "1fc9".into(),
            pid: "0143".into(),
            serial: "S".into(),
            interface: None,
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_hex() {
        let s = ProbeSelector {
            vid: "zz".into(),
            pid: "0143".into(),
            serial: "S".into(),
            interface: None,
        };
        assert!(s.validate().is_err());
    }
}
```

- [ ] **Step 2: Run — expect compile failure (`parse`/`validate`/error type missing)**

Run: `cargo test -p paavo-proto board::tests`
Expected: FAIL to compile — no `parse`, `validate`, or `ProbeSelectorParseError`.

- [ ] **Step 3: Implement the error type, parser, validator, and helpers**

Add to `crates/paavo-proto/src/board.rs` (after the `ProbeSelector` struct). Note the existing import is `use serde::{Deserialize, Serialize};` — add `use thiserror::Error;` at the top of the file if not present (thiserror is a workspace dep used across the codebase).

```rust
/// Error parsing a probe-rs selector string into a [`ProbeSelector`].
#[derive(Debug, Error)]
pub enum ProbeSelectorParseError {
    /// Selector was empty or had no PID part.
    #[error("selector is empty or missing the PID (expected `VID:PID[-IFACE][:SERIAL]`, \
             e.g. `1fc9:0143:ABCD1234`)")]
    Format,
    /// VID was not a hex u16.
    #[error("bad VID {value:?}: {source}")]
    BadVid {
        /// The offending VID text.
        value: String,
        /// The underlying parse error.
        source: std::num::ParseIntError,
    },
    /// PID was not a hex u16.
    #[error("bad PID {value:?}: {source}")]
    BadPid {
        /// The offending PID text.
        value: String,
        /// The underlying parse error.
        source: std::num::ParseIntError,
    },
    /// Interface suffix was not a u8.
    #[error("bad USB interface {value:?}: {source}")]
    BadInterface {
        /// The offending interface text.
        value: String,
        /// The underlying parse error.
        source: std::num::ParseIntError,
    },
}

impl ProbeSelector {
    /// Parse a probe-rs selector token **or** a full `probe-rs list` line.
    ///
    /// Grammar matches probe-rs's own `DebugProbeSelector`:
    /// `VID:PID[-IFACE][:SERIAL]`, split via `splitn(3, ':')` so colon-bearing
    /// serials (e.g. ESP JTAG MACs) survive. VID/PID are hex and are
    /// normalized to lowercase 4-hex.
    pub fn parse(input: &str) -> Result<Self, ProbeSelectorParseError> {
        let token = extract_selector_token(input);

        let mut parts = token.splitn(3, ':');
        let vid_raw = parts.next().unwrap_or("").trim();
        let pid_field = parts
            .next()
            .ok_or(ProbeSelectorParseError::Format)?
            .trim();
        let serial = parts.next().map(|s| s.to_string()).unwrap_or_default();

        // Peel the optional `-IFACE` off the PID field.
        let (pid_raw, interface) = match pid_field.split_once('-') {
            Some((pid, iface)) => {
                let iface = iface.trim();
                let interface = if iface.is_empty() {
                    None
                } else {
                    Some(iface.parse::<u8>().map_err(|source| {
                        ProbeSelectorParseError::BadInterface {
                            value: iface.to_string(),
                            source,
                        }
                    })?)
                };
                (pid.trim(), interface)
            }
            None => (pid_field, None),
        };

        if vid_raw.is_empty() || pid_raw.is_empty() {
            return Err(ProbeSelectorParseError::Format);
        }

        let vid = parse_hex_u16(vid_raw).map_err(|source| ProbeSelectorParseError::BadVid {
            value: vid_raw.to_string(),
            source,
        })?;
        let pid = parse_hex_u16(pid_raw).map_err(|source| ProbeSelectorParseError::BadPid {
            value: pid_raw.to_string(),
            source,
        })?;

        Ok(ProbeSelector {
            vid: format!("{vid:04x}"),
            pid: format!("{pid:04x}"),
            serial,
            interface,
        })
    }

    /// Validate that `vid`/`pid` are hex `u16`. Used by paavod at registration
    /// (the wire already carries a structured selector, so there's nothing to
    /// re-split — just confirm the fields are well-formed).
    pub fn validate(&self) -> Result<(), ProbeSelectorParseError> {
        parse_hex_u16(&self.vid).map_err(|source| ProbeSelectorParseError::BadVid {
            value: self.vid.clone(),
            source,
        })?;
        parse_hex_u16(&self.pid).map_err(|source| ProbeSelectorParseError::BadPid {
            value: self.pid.clone(),
            source,
        })?;
        Ok(())
    }
}

/// Extract the `VID:PID…` token from either a bare token or a full
/// `probe-rs list` line (`[N]: <identifier> -- <token> (<TYPE>)`).
fn extract_selector_token(input: &str) -> &str {
    let s = input.trim();
    match s.rfind(" -- ") {
        Some(idx) => {
            let after = s[idx + 4..].trim();
            // Strip a trailing ` (TYPE)` parenthetical only on the list-line path.
            match after.rfind(" (") {
                Some(p) => after[..p].trim(),
                None => after,
            }
        }
        None => s,
    }
}

/// Parse a hex string into `u16`, tolerating a `0x`/`0X` prefix and whitespace.
/// Kept in sync with `paavo_probe::session::parse_hex_u16` (see spec future-work).
fn parse_hex_u16(s: &str) -> Result<u16, std::num::ParseIntError> {
    let s = s.trim();
    let stripped = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(stripped, 16)
}
```

- [ ] **Step 4: Run — token tests pass**

Run: `cargo test -p paavo-proto board::tests`
Expected: PASS (all 12 tests). The full-`probe-rs list`-line case is added in Task 3.

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-proto/src/board.rs
git commit -m "feat(proto): ProbeSelector::parse + validate (probe-rs token grammar)"
```

---

### Task 3: Accept a full `probe-rs list` line

`extract_selector_token` already handles ` -- ` and the trailing parenthetical; this task proves it end-to-end and locks it with a test.

**Files:**
- Modify: `crates/paavo-proto/src/board.rs` (`mod tests`)

- [ ] **Step 1: Write the failing test**

Add inside `mod tests` in `crates/paavo-proto/src/board.rs`:

```rust
    #[test]
    fn parse_full_probe_rs_list_line() {
        let line = "[0]: MCU-LINK on-board (r2E4) CMSIS-DAP V3.172 \
                    -- 1fc9:0143-0:EDFHUAFM4J5ZJ (CMSIS-DAP)";
        let s = ProbeSelector::parse(line).unwrap();
        assert_eq!(
            s,
            ProbeSelector {
                vid: "1fc9".into(),
                pid: "0143".into(),
                serial: "EDFHUAFM4J5ZJ".into(),
                interface: Some(0),
            }
        );
    }
```

- [ ] **Step 2: Run — expect PASS (it should already pass)**

Run: `cargo test -p paavo-proto board::tests::parse_full_probe_rs_list_line`
Expected: PASS. `extract_selector_token` finds the last ` -- `, strips ` (CMSIS-DAP)`, leaving `1fc9:0143-0:EDFHUAFM4J5ZJ`.

> If it FAILS, the bug is in `extract_selector_token` from Task 2 — fix it there (it is the only code that handles the line form) before continuing.

- [ ] **Step 3: Commit**

```bash
git add crates/paavo-proto/src/board.rs
git commit -m "test(proto): ProbeSelector::parse accepts full probe-rs list line"
```

---

### Task 4: Wire `paavo-cli board add` to the parser

**Files:**
- Modify: `crates/paavo-cli/src/cmd_boards.rs:34-56`
- Modify: `crates/paavo-cli/src/cli.rs:145`
- Test (create): `crates/paavo-cli/tests/board_add_selector.rs`

- [ ] **Step 1: Write the failing CLI test (fail-fast, no daemon)**

Create `crates/paavo-cli/tests/board_add_selector.rs`:

```rust
//! `board add` must reject a malformed --probe locally, before any network
//! call. We point PAAVO_HOST at an unreachable address; if the command fails
//! with the parse message (not a connection error) we know it never POSTed.

use assert_cmd::Command as AssertCommand;

#[test]
fn board_add_rejects_invalid_probe_before_network() {
    let mut cmd = AssertCommand::cargo_bin("paavo-cli").unwrap();
    cmd.env("PAAVO_HOST", "http://127.0.0.1:1")
        .args([
            "board", "add",
            "--kind", "mcxa266",
            "--instance", "mcxa266-99",
            "--probe", "zz:gg:NOSER",
            "--chip", "MCXA266VFL",
            "--target", "frdm-mcx-a266",
        ]);
    cmd.assert()
        .failure()
        .stderr(predicates::str::contains("bad VID"));
}
```

- [ ] **Step 2: Run — expect FAIL (today it splits naively and tries to POST)**

Run: `cargo test -p paavo-cli --test board_add_selector`
Expected: FAIL — current code accepts `zz`/`gg` and attempts a POST, so stderr shows a connection error, not `"bad VID"`.

- [ ] **Step 3: Replace the naive split with the parser**

In `crates/paavo-cli/src/cmd_boards.rs`, replace the `BoardOp::Add` body (lines 34-56) — the `let mut parts = probe.split(':'); …` block through the `BoardSpec` literal — with:

```rust
            let probe_selector = ProbeSelector::parse(&probe).map_err(|e| {
                anyhow!(
                    "invalid --probe {probe:?}: {e}\n\n\
                     Paste a probe-rs selector (`1fc9:0143:SERIAL`) or a full \
                     `probe-rs list` line."
                )
            })?;
            let spec = BoardSpec {
                id: instance,
                kind,
                probe_selector,
                chip_name: chip,
                target_name: target,
                wiring_profile: Some(wiring_profile),
                health: BoardHealth::Healthy,
            };
            client.add_board(&spec).await?;
            println!("added: {}", spec.id);
            Ok(())
```

The existing `use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};` already imports `ProbeSelector`; `anyhow!` is already imported.

- [ ] **Step 4: Reword the `--probe` help**

In `crates/paavo-cli/src/cli.rs`, replace the `probe` doc comment (line 145, `/// VID:PID:serial.`) with:

```rust
        /// Probe selector. Accepts a probe-rs selector token
        /// (`1fc9:0143:SERIAL`, optional `-IFACE`) or a full `probe-rs list`
        /// line pasted verbatim.
        #[arg(long)]
        probe: String,
```

- [ ] **Step 5: Run — CLI test passes**

Run: `cargo test -p paavo-cli --test board_add_selector`
Expected: PASS — `parse("zz:gg:NOSER")` returns `BadVid`, the command errors locally with `"bad VID"`, never contacting `127.0.0.1:1`.

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-cli/src/cmd_boards.rs crates/paavo-cli/src/cli.rs crates/paavo-cli/tests/board_add_selector.rs
git commit -m "feat(paavo-cli): board add accepts probe-rs selector strings"
```

---

### Task 5: Validate the selector at `POST /boards`

**Files:**
- Modify: `crates/paavod/src/routes/boards.rs:56-60`
- Test: `crates/paavod/tests/api_boards.rs` (append)

- [ ] **Step 1: Write the failing daemon test**

Append to `crates/paavod/tests/api_boards.rs` (reuses the file's existing `state()`, `sample_board_json()`, `post_json()` helpers):

```rust
#[tokio::test]
async fn add_board_rejects_non_hex_vid() {
    let app = build_router(state());
    let mut body = sample_board_json();
    body["probe_selector"]["vid"] = serde_json::json!("zz");
    let resp = post_json(app, "/boards", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 2: Run — expect FAIL (no validation yet → 201)**

Run: `cargo test -p paavod --test api_boards add_board_rejects_non_hex_vid`
Expected: FAIL — currently returns `201 Created` (the bad selector is stored).

- [ ] **Step 3: Add validation in `add_board`**

In `crates/paavod/src/routes/boards.rs::add_board`, after the `health == Healthy` check (immediately before `let now_ms = …` at line 56), insert:

```rust
    spec.probe_selector
        .validate()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid probe_selector: {e}")))?;
```

(`StatusCode` is already imported in this file.)

- [ ] **Step 4: Run — daemon test passes**

Run: `cargo test -p paavod --test api_boards`
Expected: PASS — bad VID now yields `400`; existing `/boards` tests still green.

- [ ] **Step 5: Commit**

```bash
git add crates/paavod/src/routes/boards.rs crates/paavod/tests/api_boards.rs
git commit -m "feat(paavod): validate probe_selector at POST /boards (400 on bad hex)"
```

---

### Task 6: Thread the interface into probe-rs at attach

Hardware-only code path (`RealSession::connect` runs only with a real probe), so this is verified by compilation + clippy; the `PAAVO_HW=1` tests exercise it on real hardware.

**Files:**
- Modify: `crates/paavo-probe/src/session.rs:164`

- [ ] **Step 1: Pass the interface through**

In `crates/paavo-probe/src/session.rs`, change the `DebugProbeSelector` literal (line 164) from `interface: None,` to:

```rust
            interface: opts.probe_selector.interface,
```

So the block reads:

```rust
        let selector = DebugProbeSelector {
            vendor_id: vid,
            product_id: pid,
            interface: opts.probe_selector.interface,
            serial_number: serial_filter.clone(),
        };
```

- [ ] **Step 2: Build + lint**

Run: `cargo clippy -p paavo-probe --all-targets -- -D warnings`
Expected: PASS (no warnings).

- [ ] **Step 3: Commit**

```bash
git add crates/paavo-probe/src/session.rs
git commit -m "feat(paavo-probe): honor ProbeSelector.interface at probe attach"
```

---

### Task 7: Docs

**Files:**
- Modify: `docs/deployment.md` (the stale `board add`/`boards.toml` claim)
- Modify: `AGENTS.md` (landmines note)

- [ ] **Step 1: Correct `docs/deployment.md`**

Find the `boards.toml` bullet (under "State directory layout"):

```
- `boards.toml` — `paavo-cli board add` writes this; restart paavod to pick up changes.
```

Replace with:

```
- `boards.toml` — declarative seed read at startup (inserts boards whose id is
  not already in the DB). `paavo-cli board add` writes the **DB** via `POST
  /boards`, not this file; edit `boards.toml` by hand to seed a fresh deploy.
```

- [ ] **Step 2: Add an `AGENTS.md` landmine note**

Under the "Landmines & gotchas" section in `AGENTS.md`, add a bullet:

```
- **`board add` accepts probe-rs `list` output directly.** `paavo-cli board add
  --probe` takes a probe-rs selector token (`1fc9:0143-0:SERIAL`, where `-0` is
  the USB interface) or a full pasted `probe-rs list` line. VID/PID are
  validated as hex at registration (CLI **and** `POST /boards`), not deferred to
  `probe_attach`. The canonical parser is `ProbeSelector::parse` in
  `paavo-proto`.
```

- [ ] **Step 3: Commit**

```bash
git add docs/deployment.md AGENTS.md
git commit -m "docs: board add parses probe-rs selectors; fix boards.toml description"
```

---

### Final verification (CI parity)

- [ ] **Run the full gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: all green (`RUSTFLAGS="-Dwarnings"` in CI — no warnings allowed).

- [ ] **If `cargo fmt --all -- --check` reports diffs**, run `cargo fmt --all`, review, and commit:

```bash
git add -A
git commit -m "style: cargo fmt"
```

---

## Self-review

**Spec coverage:** Goals → Task 4 (both input forms), Task 2/3 (grammar incl. colon-serials), Task 1+6 (preserve interface), Task 4+5 (fail-fast CLI + daemon), Task 2 (4-hex normalization). Non-goals respected (no discovery endpoint; serial stays `String`; `boards.toml` structured; `parse_hex_u16` duplicated, noted). Wire-compat (Task 1 tests), migration (none; covered in Task 1 note). Every spec "Affected files" entry has a task.

**Placeholder scan:** none — all steps carry real code/commands and expected output.

**Type consistency:** `ProbeSelector { vid, pid, serial, interface }` used identically across Tasks 1-6; `ProbeSelectorParseError::{Format,BadVid,BadPid,BadInterface}` defined in Task 2 and matched in Task 2's tests and surfaced in Task 4's CLI message; `validate()` defined in Task 2, called in Task 5; `parse()` defined in Task 2, called in Task 4; `extract_selector_token`/`parse_hex_u16` defined in Task 2, exercised in Task 3.

# `paavo-cli run`: derive board kind from the daemon inventory (demote `--board-kind`)

**Date:** 2026-06-17
**Status:** Approved (design)
**Crates:** `paavo-cli` (only)

## Problem

`paavo-cli run` forces the operator to type `--board-kind` on every
invocation. `cmd_run::run` hard-rejects its absence:

```rust
// crates/paavo-cli/src/cmd_run.rs:29
let kind = board_kind.ok_or_else(|| anyhow::anyhow!("--board-kind is required for `run`"))?;
```

That is redundant friction. A test crate already declares what it runs on via
`paavo_meta::target!(b"frdm-mcx-a266")`, and when an operator wants a *specific*
board they think in terms of an **instance** (`mcxa266-02`), not a kind. The
board *kind* should not be something you retype; it should be inferred.

The fully-principled fix — have **paavod** read `.paavo.target` out of the built
ELF and place the job by `BoardSpec::target_name`, with no client-supplied kind
at all — is a larger, cross-crate change (it moves placement after the build,
relaxes submit-time validation, and finally wires up the currently-vestigial
`paavo_probe::sections::parse_meta_sections`). That work is **deferred to a
separate project** (see [Future work](#future-work-out-of-scope-here)).

This spec does the small, immediately useful slice: keep the daemon exactly as
it is (kind still required on the wire and validated at submit — the "instant
`SelectorNeverMatches` check" stays), and make **the CLI** fill in the kind for
you using the inventory it can already fetch from `GET /boards`.

## Goals

- Make `--board-kind` optional on `paavo-cli run`.
- When `--instance <id>` is given, derive the kind from that board (via `GET
  /boards`) so `--board-kind` is unnecessary — matching "a specific board is an
  *instance*, not a *kind*".
- When neither flag is given, default to the lab's sole kind if the inventory
  has exactly one; otherwise fail with an actionable message.
- Keep the resolved `board_selector` fully populated on the wire, so **paavod,
  the HTTP contract, and the submit-time validation are untouched**.
- Factor the decision into a pure, fully unit-tested function (no network in the
  logic under test).

## Non-goals (YAGNI)

- **No `paavo-meta` / `.paavo.target` consumption.** The ELF section parser
  (`crates/paavo-probe/src/sections.rs`) stays unused by runtime code; we do not
  read the target from the crate or the ELF in this change.
- **No daemon, DB, route, or wire changes.** `BoardSelector` keeps a required
  `kind`; `GET /boards` and `POST /jobs` are unchanged. This is additive client
  behavior only.
- **No `--wiring-profile` flag** (`run` does not expose one today; the selector
  field stays `None`).
- **No corpus / schedule work.** That is the separately-tracked "Change B".
- No caching of the inventory, retries, or offline mode — one `GET /boards` when
  needed, surfaced errors otherwise.

## Decisions

| Question | Decision |
|----------|----------|
| `--board-kind` requiredness | **Optional.** Already `Option<String>` at the clap layer (`cli.rs:27`); we remove the `ok_or_else` rejection and resolve instead. |
| Primary resolution mechanism | `--instance <id>` → fetch `GET /boards`, find that board, use its `kind`. |
| Neither flag given | Fetch `GET /boards`; **exactly one** distinct kind ⇒ use it; **multiple** ⇒ error listing them; **zero boards** ⇒ error asking for `--board-kind`. |
| `--board-kind` given, **no** `--instance` | Use it verbatim — **no** `GET /boards` round-trip. The daemon validates the kind at submit as it does today. |
| Both `--instance` and `--board-kind` given | Must **agree**; if the instance's kind differs from `--board-kind`, hard error. |
| `--instance` not in inventory | Hard error (we cannot derive a kind; do **not** submit a guess and let the upload+400 happen). |
| Which `kind` wins when derived | The instance's actual kind. `--board-kind`, if also present, is only a cross-check. |
| Inventory includes quarantined boards? | **Yes.** Kind is intrinsic; a quarantined board's kind is still a valid target (the job simply waits). Health is not consulted for resolution. |
| Transparency | On success, print `resolved board: kind=<k> instance=<id|any>` to **stderr** (keeps stdout = the job id, matching the existing `eprintln!` follow-hint convention). |
| Error type | `anyhow` with `.context()` (CLI convention); propagates to `main` → non-zero exit. |

## Design

All changes live in `crates/paavo-cli/`. `main.rs` is **unchanged**:
`cmd_run::run` keeps its signature (`board_kind: Option<&str>`, `instance:
Option<&str>`), so the existing dispatch at `main.rs:36-48` still compiles.

### 1. Typed inventory fetch — `crates/paavo-cli/src/client.rs`

`GET /boards` returns a bare JSON array of board objects in `BoardView` shape
(flattened `BoardSpec` + operational fields). `cmd_boards::list` currently
decodes it untyped as `Vec<Value>`. Add a typed accessor reused by `run`:

```rust
use paavo_proto::BoardView;

/// List the daemon's board inventory (`GET /boards`).
pub async fn list_boards(&self) -> Result<Vec<BoardView>> {
    self.get_json::<Vec<BoardView>>("/boards").await
}
```

`BoardView` flattens `BoardSpec`, so `view.spec.id` / `view.spec.kind` are the
fields we need. (Leaving `cmd_boards::list`'s untyped rendering as-is keeps this
change minimal; it may adopt `list_boards` opportunistically but that is not
required.)

### 2. Pure resolver — `crates/paavo-cli/src/cmd_run.rs`

The decision is a pure function over the fetched inventory, so every branch is
unit-testable without HTTP:

```rust
use paavo_proto::{BoardSelector, BoardView};
use std::collections::BTreeSet;

/// Resolve the wire `BoardSelector` from the two optional CLI flags and the
/// daemon inventory. Pure: all I/O (the `GET /boards`) happens in the caller.
///
/// Rules (see spec Decisions table):
///   (instance=Some, _)      → derive kind from that board; cross-check
///                             `board_kind` if also given.
///   (instance=None, Some k) → use `k` verbatim (inventory ignored).
///   (instance=None, None)   → sole inventory kind, or an actionable error.
fn resolve_board_selector(
    board_kind: Option<&str>,
    instance: Option<&str>,
    boards: &[BoardView],
) -> Result<BoardSelector> {
    match (instance, board_kind) {
        (Some(id), kind_hint) => {
            let board = boards
                .iter()
                .find(|b| b.spec.id == id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no board with id '{id}' in inventory (known: {})",
                        join_ids(boards)
                    )
                })?;
            let kind = board.spec.kind.as_str();
            if let Some(hint) = kind_hint {
                if hint != kind {
                    anyhow::bail!(
                        "board '{id}' is kind '{kind}', which conflicts with --board-kind '{hint}'"
                    );
                }
            }
            Ok(BoardSelector {
                kind: kind.to_string(),
                instance: Some(id.to_string()),
                wiring_profile: None,
            })
        }
        (None, Some(kind)) => Ok(BoardSelector {
            kind: kind.to_string(),
            instance: None,
            wiring_profile: None,
        }),
        (None, None) => {
            let kinds: BTreeSet<&str> = boards.iter().map(|b| b.spec.kind.as_str()).collect();
            match kinds.len() {
                1 => Ok(BoardSelector {
                    kind: kinds.into_iter().next().unwrap().to_string(),
                    instance: None,
                    wiring_profile: None,
                }),
                0 => anyhow::bail!(
                    "no boards registered with paavod; pass --board-kind <kind> \
                     (or register a board first)"
                ),
                _ => anyhow::bail!(
                    "multiple board kinds available ({}); \
                     pass --instance <id> or --board-kind <kind>",
                    kinds.into_iter().collect::<Vec<_>>().join(", ")
                ),
            }
        }
    }
}
```

`join_ids` is a one-line helper rendering the known board ids for the
not-found message.

### 3. Caller wiring — `crates/paavo-cli/src/cmd_run.rs::run`

Replace the `ok_or_else` line (`cmd_run.rs:29`) with: fetch the inventory **only
when needed** (i.e. unless an explicit `--board-kind` with no `--instance` lets
us skip the round-trip), resolve, then announce.

```rust
// was: let kind = board_kind.ok_or_else(...)?;
let selector = {
    // The (None, Some) branch is the only one that needs no inventory.
    let need_inventory = instance.is_some() || board_kind.is_none();
    let boards = if need_inventory {
        client.list_boards().await.context("fetching board inventory")?
    } else {
        Vec::new()
    };
    resolve_board_selector(board_kind, instance, &boards)?
};
eprintln!(
    "resolved board: kind={} instance={}",
    selector.kind,
    selector.instance.as_deref().unwrap_or("(any)")
);
```

and feed `selector` straight into the existing `JobSpec` literal
(`cmd_run.rs:42-46`), replacing the inline `BoardSelector { kind: kind.into(),
instance: instance.map(String::from), wiring_profile: None }`:

```rust
let spec = JobSpec {
    priority: /* unchanged */,
    submitter: /* unchanged */,
    board_selector: selector,
    inactivity_timeout_ms: /* unchanged */,
    hard_max_ms: /* unchanged */,
    skip_cache,
};
```

### 4. Help text — `crates/paavo-cli/src/cli.rs`

The `Run.board_kind` doc comment (`cli.rs:25`) currently reads "Required board
kind (e.g. mcxa266)." Reword to reflect the new behavior:

```rust
/// Board kind (e.g. mcxa266). Optional: inferred from --instance, or
/// defaulted when the lab has a single kind. Required only to
/// disambiguate when multiple kinds exist and no --instance is given.
#[arg(long)]
board_kind: Option<String>,
```

No structural clap change — the field is already `Option<String>`.

## Testing / verification

### Unit tests — `crates/paavo-cli/src/cmd_run.rs` (`#[cfg(test)]`)

Drive `resolve_board_selector` directly with hand-built `BoardView`s (a small
`fn bv(id: &str, kind: &str) -> BoardView` helper fills the operational fields
with defaults). This is the definition-of-done for correctness; all seven
branches:

1. `instance_derives_kind` — `instance=Some("mcxa266-02")`, no kind, inventory
   has that board ⇒ selector `{ kind: "mcxa266", instance: Some("mcxa266-02") }`.
2. `instance_not_found_errors` — `instance` not in inventory ⇒ error mentions the
   id and the known ids.
3. `instance_and_matching_kind_ok` — both given and agree ⇒ Ok, kind from board.
4. `instance_and_conflicting_kind_errors` — both given, disagree ⇒ error mentions
   both kinds.
5. `neither_single_kind_defaults` — inventory all one kind ⇒ selector `{ kind,
   instance: None }`.
6. `neither_multiple_kinds_errors` — inventory has ≥2 kinds ⇒ error lists them
   (sorted, deterministic via `BTreeSet`).
7. `neither_zero_boards_errors` — empty inventory ⇒ "no boards registered" error.

Plus `explicit_kind_no_instance_skips_inventory` — `board_kind=Some, instance=None`
returns the verbatim kind even when passed an **empty** slice (proving the
caller is right to skip the fetch).

### CLI surface — `crates/paavo-cli/tests/` (`assert_cmd` + `predicates`)

- `run_help_board_kind_optional`: `run --help` no longer says "Required"; mentions
  inference. (If an existing help test asserts the old wording, update it.)
- Optional (only if the existing CLI harness already spins a `PAAVO_FAKE_RUNNER`
  daemon — do not add new daemon plumbing for it): `run <crate>` with a
  single-board inventory succeeds without `--board-kind`; with a multi-kind
  inventory and neither flag, exits non-zero with the "multiple board kinds"
  message. The pure-function unit tests are the primary guarantee; this is
  belt-and-suspenders.

### Full gate (CI parity, from `AGENTS.md`)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All green. (`RUSTFLAGS="-Dwarnings"` in CI — no warnings allowed.)

## Affected files

- `crates/paavo-cli/src/client.rs` — add typed `list_boards() -> Vec<BoardView>`.
- `crates/paavo-cli/src/cmd_run.rs` — `resolve_board_selector` (+ `join_ids`
  helper), replace the `--board-kind` rejection with fetch-resolve-announce,
  feed the resolved selector into `JobSpec`; unit tests.
- `crates/paavo-cli/src/cli.rs` — reword the `Run.board_kind` doc comment.
- `crates/paavo-cli/tests/…` — help-text assertion (and the optional
  daemon-backed test if the harness already exists).

`crates/paavo-cli/src/main.rs` is unchanged.

## Future work (out of scope here)

Tracked for later, in order:

1. **"Tests declare their own target" (the real `paavo-meta` wiring).** Have
   paavod read `.paavo.target` from the built ELF, place jobs by
   `BoardSpec::target_name`, demote/relax client-supplied kind end-to-end, and
   move placement validation after the build. This is the cross-crate change
   this spec deliberately avoids; revisiting `paavo-meta` is a prerequisite.
2. **DB-backed corpus + `paavo-cli corpus` + no-cache nightly ("Change B").**
   Uploaded-snapshot corpus entries keyed by `(schedule_id, crate-name)`,
   scheduled jobs forced to `skip_cache = true` so dependencies float while the
   test source is frozen. Cleanest once (1) lands, because a corpus entry then
   stores no board kind at all.

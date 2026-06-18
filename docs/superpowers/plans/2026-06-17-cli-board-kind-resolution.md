# `paavo-cli` board-kind resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `--board-kind` optional on `paavo-cli run` by deriving the board kind from the daemon's `GET /boards` inventory (via `--instance` or a sole-kind default), with no daemon/wire/`paavo-meta` changes.

**Architecture:** All changes live in the `paavo-cli` binary crate. A pure function `resolve_board_selector(board_kind, instance, &[BoardView]) -> Result<BoardSelector>` makes the decision and is unit-tested without HTTP; `cmd_run::run` fetches the inventory only when needed, resolves, announces the choice on stderr, and feeds the resulting `BoardSelector` into the existing `JobSpec`. The daemon still receives and validates a fully-populated selector exactly as today.

**Tech Stack:** Rust 1.95.0, `clap`, `anyhow`, `reqwest`, `paavo-proto` (`BoardView`/`BoardSelector`); tests use `assert_cmd` + `predicates` and the existing `paavod`-in-process harness.

**Spec:** `docs/superpowers/specs/2026-06-17-cli-board-kind-resolution-design.md`

---

## File Structure

- `crates/paavo-cli/src/client.rs` — add a typed `list_boards() -> Vec<BoardView>` accessor over `GET /boards`.
- `crates/paavo-cli/src/cmd_run.rs` — add `resolve_board_selector` + `join_ids`, rewire `run()` to fetch/resolve/announce, and host the unit tests in the existing `#[cfg(test)] mod tests`.
- `crates/paavo-cli/src/cli.rs` — reword the `Run.board_kind` doc comment (no structural change).
- `crates/paavo-cli/tests/cli_help.rs` — add a help-text assertion.
- `crates/paavo-cli/tests/cli_run_board_resolution.rs` — new daemon-backed integration test (ambiguous-kind fail-fast).

No `Cargo.toml` changes: every dependency used by the new integration test (`paavod`, `paavo-db`, `tempfile`, `tokio`, `parking_lot`, `assert_cmd`, `predicates`) is already a `paavo-cli` dev-dependency (see `tests/cli_jobs_against_paavod.rs`).

---

## Task 1: Board-kind resolution (resolver + client method + `run()` wiring + help)

Everything that introduces the resolver and the `list_boards` method also *uses*
them in the same commit — required, because unused `pub`/private items fail
`dead_code` under `-D warnings` in a binary crate. TDD ordering is preserved
inside the task: tests first (red), implement, wire, green.

**Files:**
- Modify: `crates/paavo-cli/src/cmd_run.rs` (imports, resolver, `run()` body, tests)
- Modify: `crates/paavo-cli/src/client.rs` (new `list_boards`)
- Modify: `crates/paavo-cli/src/cli.rs:25-27` (doc comment)

- [ ] **Step 1: Write the failing unit tests**

In `crates/paavo-cli/src/cmd_run.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block (it currently starts with `use super::*;` near line 280), add the inventory builder and the eight resolver tests. Add these imports at the top of the `mod tests` block (right after the existing `use std::io::Read as _;`):

```rust
    // NOTE: do NOT import `BoardView` here — it is added at the file's top
    // level in Step 3 and reaches this module via the existing `use super::*`.
    // Importing it again would be a redundant import (caught by `-D warnings`).
    use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};

    /// Build a minimal `BoardView` for resolver tests. Only `id` and
    /// `kind` matter to `resolve_board_selector`; the rest are filler.
    fn bv(id: &str, kind: &str) -> BoardView {
        BoardView {
            spec: BoardSpec {
                id: id.into(),
                kind: kind.into(),
                probe_selector: ProbeSelector {
                    vid: "1366".into(),
                    pid: "1015".into(),
                    serial: format!("S-{id}"),
                },
                chip_name: "MCXA266".into(),
                target_name: format!("target-{kind}"),
                wiring_profile: None,
                health: BoardHealth::Healthy,
            },
            quarantine_reason: None,
            consecutive_infra_failures: 0,
            last_used_at: None,
            created_at: 0,
        }
    }

    #[test]
    fn instance_derives_kind() {
        let inv = vec![bv("mcxa266-01", "mcxa266"), bv("mcxa266-02", "mcxa266")];
        let sel = resolve_board_selector(None, Some("mcxa266-02"), &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance.as_deref(), Some("mcxa266-02"));
        assert_eq!(sel.wiring_profile, None);
    }

    #[test]
    fn instance_not_found_errors() {
        let inv = vec![bv("mcxa266-01", "mcxa266")];
        let err = resolve_board_selector(None, Some("nope"), &inv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope"), "got: {msg}");
        assert!(msg.contains("mcxa266-01"), "should list known ids; got: {msg}");
    }

    #[test]
    fn instance_and_matching_kind_ok() {
        let inv = vec![bv("mcxa266-01", "mcxa266")];
        let sel = resolve_board_selector(Some("mcxa266"), Some("mcxa266-01"), &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance.as_deref(), Some("mcxa266-01"));
    }

    #[test]
    fn instance_and_conflicting_kind_errors() {
        let inv = vec![bv("mcxa266-01", "mcxa266")];
        let err = resolve_board_selector(Some("rt685"), Some("mcxa266-01"), &inv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mcxa266"), "got: {msg}");
        assert!(msg.contains("rt685"), "got: {msg}");
    }

    #[test]
    fn neither_single_kind_defaults() {
        let inv = vec![bv("mcxa266-01", "mcxa266"), bv("mcxa266-02", "mcxa266")];
        let sel = resolve_board_selector(None, None, &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance, None);
    }

    #[test]
    fn neither_multiple_kinds_errors() {
        let inv = vec![bv("mcxa266-01", "mcxa266"), bv("rt685-01", "rt685")];
        let err = resolve_board_selector(None, None, &inv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple board kinds"), "got: {msg}");
        assert!(msg.contains("mcxa266") && msg.contains("rt685"), "got: {msg}");
    }

    #[test]
    fn neither_zero_boards_errors() {
        let inv: Vec<BoardView> = vec![];
        let err = resolve_board_selector(None, None, &inv).unwrap_err();
        assert!(
            err.to_string().contains("no boards registered"),
            "got: {err}"
        );
    }

    #[test]
    fn explicit_kind_no_instance_skips_inventory() {
        // Empty inventory on purpose: the (None, Some) branch must not
        // depend on it (the caller skips the GET /boards round-trip).
        let inv: Vec<BoardView> = vec![];
        let sel = resolve_board_selector(Some("mcxa266"), None, &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance, None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p paavo-cli --bins 2>&1 | tail -20`
(`paavo-cli` is a binary-only crate, so use `--bins`, not `--lib`. The unit tests live in `cmd_run.rs`.)
Expected: FAIL to compile — `cannot find function 'resolve_board_selector' in this scope` (and `cannot find type 'BoardView'` until Step 3 adds the top-level import). This is the red state.

- [ ] **Step 3: Implement the resolver**

In `crates/paavo-cli/src/cmd_run.rs`, change the imports at the top of the file. Replace:

```rust
use paavo_proto::{BoardSelector, JobSpec, Priority};
use std::path::Path;
```

with:

```rust
use paavo_proto::{BoardSelector, BoardView, JobSpec, Priority};
use std::collections::BTreeSet;
use std::path::Path;
```

Then add these two functions to the file (place them just above the `#[cfg(test)]` module, after `parse_duration_ms`):

```rust
/// Render the known board ids for an error message.
fn join_ids(boards: &[BoardView]) -> String {
    boards
        .iter()
        .map(|b| b.spec.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the wire `BoardSelector` from the two optional CLI flags plus the
/// daemon inventory. Pure: the caller performs the `GET /boards`.
///
/// Rules (spec 2026-06-17-cli-board-kind-resolution):
///   (instance=Some, _)      derive kind from that board; cross-check an
///                           explicit `board_kind` if also given.
///   (instance=None, Some k) use `k` verbatim (inventory ignored).
///   (instance=None, None)   the sole inventory kind, or an actionable error.
fn resolve_board_selector(
    board_kind: Option<&str>,
    instance: Option<&str>,
    boards: &[BoardView],
) -> Result<BoardSelector> {
    match (instance, board_kind) {
        (Some(id), kind_hint) => {
            let board = boards.iter().find(|b| b.spec.id == id).ok_or_else(|| {
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

- [ ] **Step 4: Add the typed client accessor**

In `crates/paavo-cli/src/client.rs`, change the import line:

```rust
use paavo_proto::{BoardSpec, JobSpec};
```

to:

```rust
use paavo_proto::{BoardSpec, BoardView, JobSpec};
```

Then add this method inside `impl Client` (e.g. just after `add_board`):

```rust
    /// List the daemon's board inventory (`GET /boards`). Returns the
    /// same `BoardView` shape paavo-web consumes; `run` uses it to
    /// resolve the board kind client-side.
    pub async fn list_boards(&self) -> Result<Vec<BoardView>> {
        self.get_json::<Vec<BoardView>>("/boards").await
    }
```

- [ ] **Step 5: Wire `run()` to resolve, and update the help text**

In `crates/paavo-cli/src/cmd_run.rs::run`, replace this block (currently lines 29-50):

```rust
    let kind = board_kind.ok_or_else(|| anyhow::anyhow!("--board-kind is required for `run`"))?;
    let crate_dir = resolve_crate_dir(path)?;
    let tar_bytes = make_tar(&crate_dir).context("tarring crate dir")?;

    // JobSpec is the wire shape paavod's PostJobMetadata deserializes.
    // No `source` (server forces Cli per spec §9.1), no `tar_blake3`
    // (paavod computes it during streaming).
    let spec = JobSpec {
        priority: match priority {
            PriorityArg::Interactive => Priority::Interactive,
            PriorityArg::Scheduled => Priority::Scheduled,
        },
        submitter: whoami().unwrap_or_else(|| "anon".into()),
        board_selector: BoardSelector {
            kind: kind.into(),
            instance: instance.map(String::from),
            wiring_profile: None,
        },
        inactivity_timeout_ms: inactivity.map(parse_duration_ms).transpose()?,
        hard_max_ms: timeout.map(parse_duration_ms).transpose()?,
        skip_cache,
    };
```

with:

```rust
    // Resolve the board selector BEFORE touching the crate, so a bad
    // --instance or an ambiguous kind fails fast without tarring or
    // uploading anything. Only the explicit-kind-without-instance case
    // can skip the GET /boards round-trip.
    let selector = {
        let need_inventory = instance.is_some() || board_kind.is_none();
        let boards = if need_inventory {
            client
                .list_boards()
                .await
                .context("fetching board inventory from paavod")?
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

    let crate_dir = resolve_crate_dir(path)?;
    let tar_bytes = make_tar(&crate_dir).context("tarring crate dir")?;

    // JobSpec is the wire shape paavod's PostJobMetadata deserializes.
    // No `source` (server forces Cli per spec §9.1), no `tar_blake3`
    // (paavod computes it during streaming).
    let spec = JobSpec {
        priority: match priority {
            PriorityArg::Interactive => Priority::Interactive,
            PriorityArg::Scheduled => Priority::Scheduled,
        },
        submitter: whoami().unwrap_or_else(|| "anon".into()),
        board_selector: selector,
        inactivity_timeout_ms: inactivity.map(parse_duration_ms).transpose()?,
        hard_max_ms: timeout.map(parse_duration_ms).transpose()?,
        skip_cache,
    };
```

Then in `crates/paavo-cli/src/cli.rs`, replace the `Run.board_kind` doc comment (lines 25-27):

```rust
        /// Required board kind (e.g. mcxa266).
        #[arg(long)]
        board_kind: Option<String>,
```

with:

```rust
        /// Board kind (e.g. mcxa266). Optional: inferred from --instance,
        /// or defaulted when the lab has a single kind. Needed only to
        /// disambiguate when multiple kinds exist and no --instance is
        /// given.
        #[arg(long)]
        board_kind: Option<String>,
```

- [ ] **Step 6: Run the unit tests and a scoped clippy to verify green + no dead code**

Run: `cargo test -p paavo-cli --bins 2>&1 | tail -20`
Expected: PASS — all eight resolver tests `ok` (among the `cmd_run.rs` bin unit tests).

Run: `cargo clippy -p paavo-cli --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings (resolver + `list_boards` are now used by `run()`/tests; no `dead_code`).

- [ ] **Step 7: Commit**

```bash
git add crates/paavo-cli/src/cmd_run.rs crates/paavo-cli/src/client.rs crates/paavo-cli/src/cli.rs
git commit -m "feat(paavo-cli): derive board kind from inventory; --board-kind now optional"
```

---

## Task 2: CLI help + daemon-backed integration tests

**Files:**
- Modify: `crates/paavo-cli/tests/cli_help.rs`
- Create: `crates/paavo-cli/tests/cli_run_board_resolution.rs`

- [ ] **Step 1: Add the help-text assertion (failing first)**

Append to `crates/paavo-cli/tests/cli_help.rs`:

```rust
#[test]
fn run_help_board_kind_is_optional() {
    // `run --help` must telegraph that --board-kind is no longer
    // mandatory — it's inferred from --instance or a single-kind lab.
    let out = Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["run", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("inferred"),
        "`run --help` should explain board-kind inference; got:\n{text}"
    );
}
```

- [ ] **Step 2: Run it to verify it passes**

Run: `cargo test -p paavo-cli --test cli_help run_help_board_kind_is_optional 2>&1 | tail -20`
Expected: PASS (the reworded doc comment from Task 1 Step 5 contains "inferred").

> If this FAILS, the help reword in Task 1 Step 5 was not applied — fix the
> `cli.rs` doc comment, do not weaken the assertion.

- [ ] **Step 3: Write the integration test**

Create `crates/paavo-cli/tests/cli_run_board_resolution.rs` with the full contents below. It mirrors the established harness in `tests/cli_jobs_against_paavod.rs` (ephemeral-port `paavod`), seeds two board *kinds*, and asserts `run` with no flags fails fast with the ambiguous-kind error — before any crate work, which is why a non-existent path is safe to pass.

```rust
//! Spins up paavod with two board kinds and asserts `paavo-cli run`
//! (no --board-kind / --instance) fails fast with the ambiguous-kind
//! error, BEFORE it tars or uploads anything. The bogus crate path is
//! intentional: selector resolution runs before crate handling, so the
//! command must error at resolution and never touch the path.

use assert_cmd::Command as AssertCommand;
use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use parking_lot::Mutex;
use std::sync::Arc;
use tempfile::tempdir;

fn spec(id: &str, kind: &str) -> BoardSpec {
    BoardSpec {
        id: id.into(),
        kind: kind.into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: format!("S-{id}"),
        },
        chip_name: "MCXA266".into(),
        target_name: format!("target-{kind}"),
        wiring_profile: None,
        health: BoardHealth::Healthy,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_without_flags_fails_on_ambiguous_kind() {
    let tmp = tempdir().unwrap();
    let sd = paavod::state_dir::StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
    paavo_db::BoardRow::insert(db.raw_conn(), &spec("mcxa266-01", "mcxa266"), 0).unwrap();
    paavo_db::BoardRow::insert(db.raw_conn(), &spec("rt685-01", "rt685"), 0).unwrap();

    let cfg = Arc::new(Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir: tmp.path().to_path_buf(),
            max_upload_bytes: 256 * 1024 * 1024,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
            max_concurrent_builds: 5,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    });
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: cfg,
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        build_cancel: paavod::cancellation::BuildCancelRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    AssertCommand::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", format!("http://{addr}"))
        .args(["run", "/nonexistent-crate-dir"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("multiple board kinds"));

    server.abort();
}
```

- [ ] **Step 4: Run the integration test**

Run: `cargo test -p paavo-cli --test cli_run_board_resolution 2>&1 | tail -30`
Expected: PASS — `run_without_flags_fails_on_ambiguous_kind` ok.

> If it fails to compile on an `AppState`/`Config` field mismatch, the daemon's
> struct shape drifted since this plan was written. Open
> `crates/paavo-cli/tests/cli_jobs_against_paavod.rs` and copy its current
> `Config { .. }` / `AppState { .. }` literals verbatim — they are the
> source of truth for these fields.

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-cli/tests/cli_help.rs crates/paavo-cli/tests/cli_run_board_resolution.rs
git commit -m "test(paavo-cli): cover board-kind resolution (help + ambiguous-kind fail-fast)"
```

---

## Task 3: Full workspace gate

**Files:** none (verification only; commit any formatting fixes).

- [ ] **Step 1: Format**

Run: `cargo fmt --all`
Then: `cargo fmt --all -- --check`
Expected: the `--check` run exits 0 (no diff).

- [ ] **Step 2: Clippy (CI parity)**

Run: `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20`
Expected: finishes with no warnings/errors.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: all suites `ok`, 0 failed (2 hardware tests show as `ignored`).

- [ ] **Step 4: Commit any formatting changes (only if Step 1 modified files)**

```bash
git add -A
git commit -m "style(paavo-cli): cargo fmt" || echo "nothing to format-commit"
```

---

## Notes

- **No `AGENTS.md` update needed.** This adds no crate boundary, build/test
  command, convention, or landmine — it's an additive CLI ergonomics change. The
  golden rules in `AGENTS.md` still hold verbatim.
- **Daemon untouched.** `BoardSelector.kind` stays required on the wire and
  validated at submit; this plan only changes how `paavo-cli` populates it.
- **Deferred (separate projects, in order):** (1) real `paavo-meta` /
  `.paavo.target` consumption so the daemon resolves the target from the built
  ELF; (2) Change B — DB-backed corpus + `paavo-cli corpus` + no-cache nightly.

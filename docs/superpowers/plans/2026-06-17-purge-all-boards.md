# `admin purge --boards` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in `--boards` flag to `paavo-cli admin purge` that — behind a `[y/N]` confirmation (bypassable with `--yes`) — additionally wipes the entire board inventory, by extending `POST /admin/purge` with a `?boards=true` query parameter.

**Architecture:** The daemon gains a `PurgeQuery { boards: bool }` extractor on the existing purge handler; when `boards` is set it runs `DELETE FROM board` inside the same DB-lock scope as the existing job truncation (FK-safe — all `job` rows, the only board references, are deleted first), then refreshes the in-memory inventory cache after dropping the lock. The CLI flips `AdminOp::Purge` to a struct variant carrying `boards`/`yes`, prompts before destructive board deletion, and appends `?boards=true` to the request path.

**Tech Stack:** Rust 1.95.0, axum (daemon), clap (CLI), `serde`, `rusqlite`, `tokio`; tests via `assert_cmd` + `predicates` (CLI) and `tower::ServiceExt::oneshot` (daemon).

Spec: `docs/superpowers/specs/2026-06-17-purge-all-boards-design.md`.

---

## File Structure

- `crates/paavod/src/routes/admin.rs` — **modify**. Add `PurgeQuery` extractor, a `Query<PurgeQuery>` handler arg, a conditional `DELETE FROM board`, and a post-wipe inventory refresh. Single responsibility unchanged: the `/admin/*` reset endpoint.
- `crates/paavod/tests/api_admin.rs` — **modify**. Add three integration tests for the board-purge path. Reuses the existing `state_with_dir` / `seed_board` / `seed_in_flight_job` / `post_empty` helpers as-is.
- `crates/paavo-cli/src/cli.rs` — **modify**. `AdminOp::Purge` unit variant → struct variant `{ boards: bool, yes: bool }`.
- `crates/paavo-cli/src/cmd_admin.rs` — **modify**. Confirmation helper + query-param dispatch.
- `crates/paavo-cli/tests/cli_help.rs` — **modify**. Add a help test asserting `--boards`/`--yes` are listed.
- `crates/paavo-cli/tests/cli_admin_purge.rs` — **create**. Confirmation-gate behavior tests (declined/empty abort; `--yes` bypass reaches network).
- `crates/paavo-cli/src/main.rs` — **no change** (the `Cmd::Admin { op }` arm forwards `op` whole to `cmd_admin::op`).

---

## Task 1: Daemon — `?boards=true` wipes the board inventory

**Files:**
- Modify: `crates/paavod/src/routes/admin.rs`
- Test: `crates/paavod/tests/api_admin.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/paavod/tests/api_admin.rs` (the `seed_board` helper already
exists; `s.inventory` is an `Arc<Mutex<Vec<BoardSpec>>>`):

```rust
#[tokio::test]
async fn purge_with_boards_true_deletes_boards_and_clears_inventory() {
    let (_sd, s) = state_with_dir();
    let spec = seed_board(&s);
    // Pre-populate the in-memory inventory cache so we can prove the
    // handler clears it (not just the DB table).
    *s.inventory.lock() = vec![spec.clone()];
    let _job = seed_terminal_job(&s);

    let app = build_router(s.clone());
    let resp = post_empty(app, "/admin/purge?boards=true").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let db = s.db.lock();
    let n_board: i64 = db
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM board", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n_board, 0, "boards should be wiped with ?boards=true");
    let n_job: i64 = db
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM job", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n_job, 0);
    drop(db);
    assert!(
        s.inventory.lock().is_empty(),
        "in-memory inventory cache should be empty after board purge"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavod purge_with_boards_true_deletes_boards_and_clears_inventory`
Expected: FAIL. The current handler ignores the query string, so it preserves
the board — `assert_eq!(n_board, 0, ...)` fails with `left: 1, right: 0` (and/or
the inventory assertion fails). It must compile, because the only new surface is
the request URI string.

- [ ] **Step 3: Add the `Query` import**

In `crates/paavod/src/routes/admin.rs`, change the extract import line:

```rust
use axum::extract::{Query, State};
```

(Currently it is `use axum::extract::State;`.)

- [ ] **Step 4: Add the `PurgeQuery` type**

In `crates/paavod/src/routes/admin.rs`, immediately above the
`/// POST /admin/purge` doc comment (just before `pub async fn purge`), insert:

```rust
/// Query string for `POST /admin/purge`.
///
/// `boards=true` additionally wipes the entire `board` inventory (in
/// addition to the job/log/cache/artifact reset). Omitted or `false`
/// preserves boards — the original, backward-compatible behavior, so
/// existing callers that POST with no query string are unaffected.
#[derive(Debug, Default, serde::Deserialize)]
pub struct PurgeQuery {
    /// Also delete every board row.
    #[serde(default)]
    pub boards: bool,
}
```

- [ ] **Step 5: Add the `Query` extractor to the handler signature**

Change the `purge` signature from:

```rust
pub async fn purge(State(s): State<AppState>) -> HandlerResult<StatusCode> {
```

to:

```rust
pub async fn purge(
    State(s): State<AppState>,
    Query(q): Query<PurgeQuery>,
) -> HandlerResult<StatusCode> {
```

- [ ] **Step 6: Conditionally delete boards inside the DB scope**

In `crates/paavod/src/routes/admin.rs`, find the end of the truncation loop —
the line `info!("purge: db rows cleared (job + log_frame + build_cache)");` —
and insert the board delete immediately **after** it, still inside the same
`{ let db = s.db.lock(); ... }` scope (before the scope's closing `}`):

```rust
        if q.boards {
            conn.execute("DELETE FROM board", []).map_err(|e| {
                error!(error = %e, "purge: DELETE FROM board failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("DELETE FROM board: {e}"),
                )
            })?;
            info!("purge: board inventory cleared");
        }
```

This is FK-safe: `job.board_id` is the only reference to `board`, and all `job`
rows were just deleted; `schedule` does not reference `board`.

- [ ] **Step 7: Refresh the inventory cache after the lock is dropped**

In `crates/paavod/src/routes/admin.rs`, find the final
`info!("purge: complete");` line and insert this block immediately **before** it
(this is after the `db` guard has been dropped and after the filesystem wipe):

```rust
    if q.boards {
        // Empty the in-memory inventory cache to match the now-empty
        // `board` table. Lossy: the DB is authoritative and paavod
        // re-hydrates the cache from it on restart, so a failed refresh
        // only leaves a briefly-stale cache, never a wrong DB. Called
        // after dropping our db guard — refresh_inventory takes the db
        // then inventory locks itself, preserving lock ordering.
        if let Err(e) = crate::routes::boards::refresh_inventory(&s) {
            warn!(error = %e, "purge: inventory cache refresh failed after board wipe");
        }
    }
```

- [ ] **Step 8: Run the test to verify it passes**

Run: `cargo test -p paavod purge_with_boards_true_deletes_boards_and_clears_inventory`
Expected: PASS.

- [ ] **Step 9: Run the full admin test module to confirm no regression**

Run: `cargo test -p paavod --test api_admin`
Expected: PASS — all pre-existing tests (`purge_preserves_boards_and_schedules`,
`purge_refuses_with_409_when_job_is_running`, etc.) still green, because a
missing/`false` `boards` defaults to no board deletion.

- [ ] **Step 10: Commit**

```bash
git add crates/paavod/src/routes/admin.rs crates/paavod/tests/api_admin.rs
git commit -m "feat(paavod): POST /admin/purge?boards=true wipes the board inventory"
```

---

## Task 2: Daemon — lock down default-preserve and the 409 gate with `boards=true`

**Files:**
- Test: `crates/paavod/tests/api_admin.rs`

- [ ] **Step 1: Write the two guard tests**

Append to `crates/paavod/tests/api_admin.rs`:

```rust
#[tokio::test]
async fn purge_with_boards_false_preserves_boards() {
    let (_sd, s) = state_with_dir();
    let spec = seed_board(&s);
    let app = build_router(s.clone());
    let resp = post_empty(app, "/admin/purge?boards=false").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let db = s.db.lock();
    let board_row = paavo_db::BoardRow::get(db.raw_conn(), &spec.id).unwrap();
    assert_eq!(board_row.spec.id, spec.id, "boards must survive ?boards=false");
}

#[tokio::test]
async fn purge_with_boards_still_refuses_409_when_job_is_running() {
    let (_sd, s) = state_with_dir();
    let spec = seed_board(&s);
    let _id = seed_in_flight_job(&s);
    let app = build_router(s.clone());
    let resp = post_empty(app, "/admin/purge?boards=true").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    // The in-flight gate runs before any deletion: the board survives.
    let db = s.db.lock();
    let board_row = paavo_db::BoardRow::get(db.raw_conn(), &spec.id).unwrap();
    assert_eq!(board_row.spec.id, spec.id, "board wiped despite 409");
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p paavod --test api_admin purge_with_boards`
Expected: PASS for both `purge_with_boards_false_preserves_boards` and
`purge_with_boards_still_refuses_409_when_job_is_running` (the behavior already
exists from Task 1; these tests pin it against regressions).

- [ ] **Step 3: Commit**

```bash
git add crates/paavod/tests/api_admin.rs
git commit -m "test(paavod): pin boards=false preserve + 409 gate for board purge"
```

---

## Task 3: CLI — `--boards`/`--yes` flags, confirmation, and query dispatch

**Files:**
- Modify: `crates/paavo-cli/src/cli.rs`
- Modify: `crates/paavo-cli/src/cmd_admin.rs`
- Modify: `crates/paavo-cli/tests/cli_help.rs`

- [ ] **Step 1: Write the failing help test**

Append to `crates/paavo-cli/tests/cli_help.rs`:

```rust
#[test]
fn admin_purge_help_lists_boards_and_yes() {
    // Operators reading `admin purge --help` must discover the new
    // opt-in board wipe and its confirmation bypass.
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["admin", "purge", "--help"])
        .assert()
        .success()
        .stdout(contains("--boards"))
        .stdout(contains("--yes"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavo-cli --test cli_help admin_purge_help_lists_boards_and_yes`
Expected: FAIL — `admin purge` has no flags today, so `--boards`/`--yes` are
absent from the help output.

- [ ] **Step 3: Change `AdminOp::Purge` to a struct variant**

In `crates/paavo-cli/src/cli.rs`, replace the `AdminOp` enum (the
`/// `admin` ops.` block at the end of the file) with:

```rust
/// `admin` ops.
#[derive(Subcommand, Debug)]
pub enum AdminOp {
    /// Dev-loop reset: wipe job artifacts on disk (sandboxes, uploads,
    /// cargo-target, cached ELFs) and truncate `job` / `log_frame` /
    /// `build_cache` in the DB. Preserves boards and schedules unless
    /// `--boards` is given. Refused if any job is currently building,
    /// awaiting a board, or running. See spec §9.5 / §10.3.
    Purge {
        /// Also permanently delete every board from the inventory, in
        /// addition to the job/artifact wipe. Prompts for confirmation
        /// unless `--yes` is given.
        #[arg(long)]
        boards: bool,
        /// Skip the `--boards` confirmation prompt (for scripts/CI).
        #[arg(long, short = 'y')]
        yes: bool,
    },
}
```

The doc comment keeps "wipe"/"truncate"/"delete" so the existing
`admin_purge_help_mentions_wipe` test stays green.

- [ ] **Step 4: Rewrite the dispatch + confirmation in `cmd_admin.rs`**

Replace the entire contents of `crates/paavo-cli/src/cmd_admin.rs` with:

```rust
//! `paavo-cli admin ...`.

use crate::cli::AdminOp;
use crate::client::Client;
use anyhow::Result;
use std::io::{self, Write};

/// Dispatch `paavo-cli admin <op>`.
pub async fn op(client: &Client, op: AdminOp) -> Result<()> {
    match op {
        AdminOp::Purge { boards, yes } => {
            if boards && !yes && !confirm_board_purge()? {
                println!("purge aborted");
                return Ok(());
            }
            // The board wipe is opt-in via a query param so the default
            // path stays byte-for-byte the original request.
            let path = if boards {
                "/admin/purge?boards=true"
            } else {
                "/admin/purge"
            };
            client.post_json::<()>(path, None).await?;
            println!("{}", if boards { "purged (including boards)" } else { "purged" });
            Ok(())
        }
    }
}

/// Prompt the operator before the destructive board wipe. Reads one
/// line from stdin and returns `true` only for `y`/`yes`
/// (case-insensitive, trimmed). Empty input, EOF, or anything else
/// returns `false` — the prompt defaults to "no".
fn confirm_board_purge() -> Result<bool> {
    println!(
        "WARNING: --boards permanently deletes ALL boards from the inventory, \
         on top of wiping all jobs, logs, build cache, and on-disk artifacts."
    );
    print!("Purge ALL boards too? [y/N]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let ans = line.trim().to_ascii_lowercase();
    Ok(ans == "y" || ans == "yes")
}
```

- [ ] **Step 5: Run the help test + build to verify it passes and compiles**

Run: `cargo test -p paavo-cli --test cli_help`
Expected: PASS — `admin_purge_help_lists_boards_and_yes` passes and
`admin_purge_help_mentions_wipe` / `admin_subcommand_lists_purge` stay green.
(`main.rs` needs no change: its `Cmd::Admin { op } => cmd_admin::op(&client, op)`
arm forwards the whole `op`.)

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-cli/src/cli.rs crates/paavo-cli/src/cmd_admin.rs crates/paavo-cli/tests/cli_help.rs
git commit -m "feat(paavo-cli): admin purge --boards with [y/N] confirm and --yes bypass"
```

---

## Task 4: CLI — confirmation-gate behavior tests

**Files:**
- Create: `crates/paavo-cli/tests/cli_admin_purge.rs`

- [ ] **Step 1: Write the behavior tests**

Create `crates/paavo-cli/tests/cli_admin_purge.rs`:

```rust
//! Confirmation-gate behavior for `paavo-cli admin purge --boards`.
//!
//! Strategy: point the CLI at an unreachable host. Any code path that
//! issues an HTTP request fails to connect and exits non-zero. So a
//! command that *exits 0* must have short-circuited before the request
//! — which is exactly what a declined confirmation should do.

use assert_cmd::Command;
use predicates::str::contains;

/// Nothing listens on TCP port 1; connection is refused immediately.
const DEAD_HOST: &str = "http://127.0.0.1:1";

#[test]
fn purge_boards_declined_aborts_without_calling_server() {
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", DEAD_HOST)
        .args(["admin", "purge", "--boards"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(contains("aborted"));
}

#[test]
fn purge_boards_empty_answer_aborts() {
    // EOF / empty line must be treated as "no".
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", DEAD_HOST)
        .args(["admin", "purge", "--boards"])
        .write_stdin("")
        .assert()
        .success()
        .stdout(contains("aborted"));
}

#[test]
fn purge_boards_yes_flag_bypasses_prompt_and_reaches_network() {
    // --yes skips the prompt, so the command proceeds to the HTTP call
    // and fails to connect to the dead host → non-zero exit. That
    // failure is the signal the prompt was bypassed.
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", DEAD_HOST)
        .args(["admin", "purge", "--boards", "--yes"])
        .assert()
        .failure();
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p paavo-cli --test cli_admin_purge`
Expected: PASS for all three tests.

- [ ] **Step 3: Commit**

```bash
git add crates/paavo-cli/tests/cli_admin_purge.rs
git commit -m "test(paavo-cli): admin purge --boards confirmation gate (abort vs --yes)"
```

---

## Task 5: Full workspace gate

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt --all`
Then verify clean: `cargo fmt --all -- --check`
Expected: no diff.

- [ ] **Step 2: Clippy (CI parity, warnings are errors)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings, no errors.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: PASS (no hardware needed; the new tests are host-only).

- [ ] **Step 4: Commit any formatting fixups**

Only if Step 1 changed files that weren't already committed:

```bash
git add -A
git commit -m "style: cargo fmt for admin purge --boards"
```

---

## Self-Review notes

- **Spec coverage:** flag `--boards` (Task 3) ✓; `[y/N]` default-N confirmation +
  empty/EOF abort (Tasks 3–4) ✓; `--yes` bypass (Tasks 3–4) ✓; additive purge via
  `?boards=true` server-side (Task 1) ✓; FK-safe `DELETE FROM board` after job
  truncation (Task 1, Step 6) ✓; inventory-cache refresh (Task 1, Step 7) ✓;
  backward-compat default preserve + 409 gate (Tasks 1–2) ✓; tests for daemon and
  CLI (all tasks) ✓; full fmt/clippy/test gate (Task 5) ✓.
- **Type consistency:** `PurgeQuery { boards: bool }` defined in Task 1 Step 4 and
  consumed in Step 5; `AdminOp::Purge { boards, yes }` defined in Task 3 Step 3 and
  matched in Step 4; `refresh_inventory(&s)` is the existing `pub(crate)` fn in
  `routes/boards.rs`; `post_json::<()>(path, None)` matches the existing client
  signature. Stdout markers (`aborted`, `purged`, `purged (including boards)`) are
  consistent between `cmd_admin.rs` and the CLI tests.
- **No placeholders:** every code step shows complete code; every run step shows the
  exact command and expected result.

# `admin purge`: optional board-inventory wipe behind a confirmation gate

**Date:** 2026-06-17
**Status:** Approved (design)
**Crates:** `paavo-cli` (flags + confirmation), `paavod` (query param + handler)

## Problem

`paavo-cli admin purge` is the dev-loop reset: it `POST`s `/admin/purge`, and
that handler (`crates/paavod/src/routes/admin.rs`) wipes job artifacts on disk
plus `job` / `log_frame` / `build_cache` in the DB while **deliberately
preserving `board` and `schedule` rows** — operators do not want to
re-register probes after every reset.

Sometimes, though, an operator wants a genuinely clean slate that also drops
the board inventory (e.g. tearing down a lab bench, or recovering from a
corrupt/mis-registered fleet). Today that requires manual `DELETE /boards/:id`
calls, which are gated on each board being quarantined first — clumsy for a
"wipe everything" intent. We want a single, opt-in, **confirmed** way to also
purge all boards.

## Goals

- Add a `--boards` flag to `paavo-cli admin purge` that, **in addition to** the
  normal purge, wipes the entire board inventory.
- When `--boards` is set, require an interactive `[y/N]` confirmation that
  **defaults to N** (no answer / empty / EOF ⇒ abort).
- Add a `--yes` / `-y` flag that skips the confirmation (for scripts/CI and to
  let the destructive path be tested non-interactively).
- Extend the daemon so `POST /admin/purge?boards=true` also deletes all `board`
  rows and refreshes the in-memory inventory cache.
- Preserve **all** existing behavior when the flag is absent (wire-compatible,
  existing tests untouched).

## Non-goals (YAGNI)

- No purging of `schedule` rows (boards only, as requested).
- No selective / per-kind board purge — it is all-or-nothing.
- No auth, audit log, or soft-delete (v1 `/admin/*` has none; unchanged).
- No confirmation prompt for the default (boards-preserved) purge — its
  behavior and UX are unchanged.

## Decisions

| Question | Decision |
|----------|----------|
| Purge scope of the flag | **Additive.** Run the normal purge (jobs/logs/build-cache/artifacts) **and** wipe all boards. |
| Flag name | `--boards` (matches the existing `board` subcommand vocabulary). |
| Confirmation | Interactive `Purge ALL boards too? [y/N]`. Accept only `y`/`yes` (trimmed, case-insensitive). Anything else — including empty input / EOF / non-interactive stdin — **aborts**. |
| Confirmation bypass | `--yes` / `-y` skips the prompt. Has no effect without `--boards`. |
| Abort = error? | **No.** A declined confirmation is a successful no-op: print "purge aborted", exit 0, make **no** HTTP call. |
| Wire shape | Query parameter `POST /admin/purge?boards=true` (default `false`). Chosen over a JSON body so the existing no-body `post_json::<()>(path, None)` client path is reused unchanged. |
| FK safety | `DELETE FROM board` runs **after** the existing `DELETE FROM job`, so the only board references (`job.board_id`) are already gone; `schedule` does not reference `board`. Safe. |
| In-flight gate | The handler's existing `409` gate (building / awaiting_board / running) already runs first, so a board can never be mid-use when it is deleted. No new gate needed. |

## Design

### CLI surface — `crates/paavo-cli/src/cli.rs`

`AdminOp::Purge` changes from a unit variant to a struct variant:

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
        /// Also permanently delete every board from the inventory
        /// (in addition to the job/artifact wipe). Prompts for
        /// confirmation unless `--yes` is given.
        #[arg(long)]
        boards: bool,
        /// Skip the confirmation prompt for `--boards`.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}
```

The doc comment retains destructive wording ("wipe"/"truncate"/"delete") so the
`admin_purge_help_mentions_wipe` test still passes.

### Confirmation + dispatch — `crates/paavo-cli/src/cmd_admin.rs`

```rust
pub async fn op(client: &Client, op: AdminOp) -> Result<()> {
    match op {
        AdminOp::Purge { boards, yes } => {
            if boards && !yes && !confirm_board_purge()? {
                println!("purge aborted");
                return Ok(());
            }
            let path = if boards {
                "/admin/purge?boards=true"
            } else {
                "/admin/purge"
            };
            client.post_json::<()>(path, None).await?;
            println!(if boards { "purged (including boards)" } else { "purged" });
            Ok(())
        }
    }
}
```

`confirm_board_purge` prints a one-line warning naming what will be destroyed,
prints the `[y/N]` prompt, reads **one line** from `std::io::stdin`, trims it,
and returns `true` only for `y` / `yes` (case-insensitive). A read error or EOF
(empty read) returns `false` (abort). The `post_json::<()>` turbofish names the
**request** body type (`()` ⇒ no body sent); `post_json` ignores the response
body and only checks status, so the `204 No Content` reply needs no change.

> Note: the query is appended to the path string passed to `post_json`, which
> already does `format!("{}{}", self.base, path)`. No client signature change.

### Server handler — `crates/paavod/src/routes/admin.rs`

Add a typed query extractor and a conditional `DELETE FROM board`:

```rust
#[derive(serde::Deserialize, Default)]
pub struct PurgeQuery {
    #[serde(default)]
    pub boards: bool,
}

pub async fn purge(
    State(s): State<AppState>,
    Query(q): Query<PurgeQuery>,
) -> HandlerResult<StatusCode> {
    {
        let db = s.db.lock();
        let conn = db.raw_conn();
        // ... existing in-flight 409 gate (unchanged) ...
        // ... existing DELETE FROM log_frame / build_cache / job (unchanged) ...
        if q.boards {
            conn.execute("DELETE FROM board", [])
                .map_err(/* 500, same pattern as the loop above */)?;
            info!("purge: board inventory cleared");
        }
    } // db guard dropped here — before any await / inventory lock

    // ... existing best-effort filesystem wipe (unchanged) ...

    if q.boards {
        // Empty the in-memory inventory cache to match the now-empty
        // board table. Lossy: the DB is authoritative and a paavod
        // restart re-hydrates from it, so a failed refresh only leaves
        // a briefly-stale cache, never a wrong DB.
        if let Err(e) = crate::routes::boards::refresh_inventory(&s) {
            warn!(error = %e, "purge: inventory cache refresh failed after board wipe");
        }
    }

    info!("purge: complete");
    Ok(StatusCode::NO_CONTENT)
}
```

`refresh_inventory` is already `pub(crate)` in `routes/boards.rs` and reads the
`board` table into `s.inventory`; after `DELETE FROM board` it yields an empty
inventory. It takes `db` then `inventory` locks internally — we call it only
**after** dropping our own `db` guard, preserving lock ordering and the
`await_holding_lock` deny.

The `DELETE FROM board` lives inside the **same** `db.lock()` scope as the job
truncation so the whole DB-side wipe is one atomic unit relative to other
writers, consistent with the existing handler.

## Testing / verification

### `paavod` integration — `crates/paavod/tests/api_admin.rs`

- `purge_with_boards_true_deletes_boards`: seed boards + jobs, `POST
  /admin/purge?boards=true` ⇒ `204`, `board` table empty, jobs/log_frame/
  build_cache empty.
- `purge_default_preserves_boards` (covered today by
  `purge_preserves_boards_and_schedules`; add/keep an explicit
  `?boards=false` assertion): boards survive.
- `purge_with_boards_still_refuses_409_when_job_running`: the in-flight gate
  fires **before** any board deletion when `?boards=true`.
- (Helper note: the existing `post_empty` test helper must accept a path with a
  query string; it already takes the full path, so `?boards=true` just rides
  along.)

### CLI — `crates/paavo-cli/tests/cli_help.rs` + new test

- `admin_purge_help_lists_boards_and_yes`: `admin purge --help` contains
  `--boards` and `--yes`.
- Confirmation behavior (new test, `assert_cmd` with `.write_stdin`):
  - `admin purge --boards` with stdin `"n\n"` (and with empty stdin) ⇒ exits 0,
    stdout contains "aborted", and (pointing at an unreachable host) makes **no**
    network call — i.e. it does not error on connection, proving the abort
    short-circuits before the HTTP request.
  - `admin purge --boards --yes` ⇒ skips the prompt (reaches the HTTP call;
    against a non-running host it fails at the request, which is the signal the
    prompt was bypassed). Where a live `paavod` fixture is available
    (`cli_jobs_against_paavod.rs` pattern), assert the boards are gone after.

### Full gate

`cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` all green.

## Affected files

- `crates/paavo-cli/src/cli.rs` — `AdminOp::Purge` becomes a struct variant
  with `boards` + `yes`.
- `crates/paavo-cli/src/cmd_admin.rs` — confirmation helper + query-param
  dispatch.
- `crates/paavod/src/routes/admin.rs` — `PurgeQuery` extractor, conditional
  `DELETE FROM board`, post-wipe inventory refresh.
- `crates/paavod/tests/api_admin.rs` — board-purge integration tests.
- `crates/paavo-cli/tests/cli_help.rs` (+ new CLI test file) — help +
  confirmation tests.

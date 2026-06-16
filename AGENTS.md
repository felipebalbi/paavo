# AGENTS.md

Guidance for AI coding agents working **on the paavo source tree**. (If you
are instead helping someone *author a test crate* to submit to paavo, see
`templates/` and the README quick start — this file is about hacking on paavo
itself.)

**paavo** is a self-hosted Linux **hardware-in-the-loop (HIL) test runner**
for the `embassy-mcxa` HAL (and any future embassy chip wired into the lab).
It is a Rust workspace of 10 crates producing 3 binaries:

- **`paavod`** — the daemon. Owns the job queue, the board fleet, the SQLite
  database, the build sandbox, and the HTTP API.
- **`paavo-cli`** — the developer's command-line client. Scaffolds test
  crates, submits jobs, follows logs.
- **`paavo-web`** — a read-only web viewer for jobs, boards, and schedules.

> **The code is the source of truth.** The design specs under
> `docs/superpowers/` are valuable background but are partly stale (see
> [Landmines](#landmines--gotchas)). When docs and code disagree, trust the
> code — and fix the doc if you can.

---

## Golden rules (definition of done)

Before you consider any change complete, satisfy **all** of these. CI enforces
the first three and they are non-negotiable.

1. **Format, lint, and test must pass** — CI runs exactly these three commands,
   and so should you:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
   CI sets `RUSTFLAGS="-Dwarnings"`, so **any** warning fails the build. Run
   `cargo fmt --all` (without `--check`) to fix formatting.

2. **Use the pinned toolchain.** `rust-toolchain.toml` pins **Rust 1.95.0**
   (with `rustfmt` + `clippy`). Don't reach for newer-than-1.95 features.

3. **Keep the crate invariants:**
   - Library crates declare `#![forbid(unsafe_code)]` and
     `#![warn(missing_docs)]`. Don't introduce `unsafe`; document new public
     items.
   - **Never hold a lock across `.await`.** `paavod` declares
     `#![deny(clippy::await_holding_lock)]` (`crates/paavod/src/app_state.rs`).
     DB access goes through a `parking_lot::Mutex` that must be dropped before
     any await point.
   - **Respect the dependency DAG** (see [Crate map](#crate-map)). Dependencies
     flow strictly upward from `paavo-proto`, which depends on no internal
     crate. Do not add a back-edge (e.g. nothing makes `paavo-proto` depend on
     anything).

4. **Commit style: Conventional Commits with a crate scope.** Match the
   existing history:
   ```
   feat(paavod): two-stage dispatch — parallel build pool + board-decoupled run
   feat(db): V4 migration adds awaiting_board state (FK-safe rebuild)
   docs(spec): ...
   ```
   Feature work lands as a series of small, scoped commits.

5. **Keep this file current.** If you change the architecture, crate
   boundaries, build/test commands, conventions, or any landmine described
   here, **update `AGENTS.md` in the same change** so the next agent inherits
   accurate guidance. A stale AGENTS.md is worse than none.

---

## Commands

```bash
# Build / check everything
cargo build --workspace
cargo check --workspace

# Test
cargo test --workspace                 # full suite (deterministic, no hardware)
cargo test -p paavo-core               # one crate
cargo test -p paavod dispatch          # filter by name

# Format / lint (what CI runs)
cargo fmt --all                        # fix
cargo fmt --all -- --check             # check (CI)
cargo clippy --workspace --all-targets -- -D warnings

# Run the daemon locally with NO hardware (fake runner: every job Passes)
PAAVO_FAKE_RUNNER=1 cargo run -p paavod -- --config sample-paavo.toml
# sample-paavo.toml binds 127.0.0.1:8090, state_dir = /tmp/paavo

# Drive it with the CLI from another shell
PAAVO_HOST=http://127.0.0.1:8090 cargo run -p paavo-cli -- boards
PAAVO_HOST=http://127.0.0.1:8090 cargo run -p paavo-cli -- jobs

# Serve the read-only web UI
cargo run -p paavo-web -- --config sample-paavo.toml
```

**System dependency:** `probe-rs` needs `libudev-dev` and `pkg-config` on Linux
**even for host-only tests/clippy**. CI installs them; on a fresh box:
`sudo apt-get install -y libudev-dev pkg-config`.

**Hardware-gated tests** are marked `#[ignore]` *and* early-return unless
`PAAVO_HW=1` is set. They need a real MCX-A266 EVK plugged in. To run them:
```bash
PAAVO_HW=1 cargo test -p paavo-probe -- --ignored
```
Leave them alone unless you have the hardware; the default `cargo test
--workspace` skips them.

**No-hardware end-to-end smoke:** `manual-smoke.nu` (nushell) drives `paavo-cli`
against a local `paavod` running with `PAAVO_FAKE_RUNNER=1`, using the fixture
at `tests/fixtures/smoke-crate`.

**Useful env vars:** `PAAVO_FAKE_RUNNER=1` (daemon uses `FakeRunner`),
`PAAVO_HOST` (CLI target URL), `PAAVO_HW=1` (enable hardware tests),
`RUST_LOG` / tracing `EnvFilter` (e.g. `RUST_LOG=paavo_probe=trace`).

---

## Architecture & data flow

Three binaries, one workspace, split along runtime boundaries. **SQLite (WAL
mode) is the only IPC**: `paavod` is the single writer, `paavo-web` is a
read-only reader. They never call each other directly.

### Job state machine

Defined in **`crates/paavo-proto/src/job.rs`** (`JobState`). This is the spine
of the whole system:

```
Submitted ──(build slot free)──▶ Building ──build err──────▶ Failed(BuildErr)
    │                                │ build OK / cache hit
    │ cancel                         ▼
    ▼                          AwaitingBoard ──(board free)──▶ Running ──▶ Passed
 Aborted(User)                       │                            │         Failed(TestErr|InfraErr)
                                cancel│                            │         TimedOut(Inactivity|HardMax)
                                      ▼                            ▼         Aborted(User|DaemonShutdown|Interrupted)
                                Aborted(User)               (probe + watchdog)
```

- **Terminal states:** `Passed`, `Failed` (BuildErr / TestErr / InfraErr),
  `TimedOut` (Inactivity / HardMax), `Aborted` (User / DaemonShutdown /
  Interrupted). `is_terminal()` covers the last four.
- **`AwaitingBoard`** is the seam introduced by the parallel build pool: the
  build phase holds **no** hardware; only the run phase claims a board. (DB
  migration `V4`.)
- Only `Failed(InfraErr)` (and `TimedOut(Inactivity)` when the probe didn't
  release cleanly) counts toward **board auto-quarantine**.

### Lifecycle (submit → build → run → results)

1. **Submit** — `paavo-cli run` tars the crate and `POST /jobs` (multipart:
   `metadata` JSON + `crate` tar). `paavod` streams the tar to disk while
   blake3-hashing it, validates the board selector + ceiling, and inserts a
   `Submitted` row. Acceptance is unbounded; the concurrency cap only gates
   *execution*.
2. **Dispatch** — `paavod`'s dispatch loop (`crates/paavod/src/dispatch.rs`)
   ticks periodically: **run stage first** (`pick_runnable` → claim a free LRU
   board → `AwaitingBoard → Running`), then **build stage** (`pick_buildable`,
   single-flight by tar blake3 → acquire an in-memory build slot →
   `Submitted → Building`). Both run on `tokio::task::spawn_blocking`.
3. **Build** — `paavo-build` unpacks the tar and runs `cargo build --release`
   in a per-slot target dir; on a build-cache hit it skips straight to
   `AwaitingBoard`. The ELF is copied to a content-addressed cache
   (`cache/elf/<blake>.elf`). Cargo output streams live to clients.
4. **Run** — `paavo-runner` + `paavo-probe` flash the ELF, boot it, attach RTT,
   and decode defmt frames. A watchdog enforces inactivity / hard-max / cancel.
   **Pass = an info frame whose message is exactly `Test OK`, immediately
   followed by a `Bkpt`** (`crates/paavo-runner/src/worker.rs`).
5. **Finalize** — the terminal state + outcome detail are written; the
   quarantine counter updates; a `Terminal` event is broadcast.
6. **Results** — `GET /jobs/:id/stream` replays persisted `log_frame` rows then
   live-tails new frames as NDJSON, ending in a `terminal` line. `paavo-cli
   logs --follow` and `paavo-web`'s SSE proxy both consume this.

**Resilience:** on startup, orphaned in-flight rows (`building` / `running`)
are swept to `Aborted(Interrupted)`. On `SIGTERM`, the daemon stops taking new
picks, returns 503 on `POST /jobs`, drains within a grace period, then signals
`DaemonShutdown` to anything still running.

---

## Crate map

Dependencies flow **upward** — a crate may only depend on crates above it.
`paavo-proto` is the root and depends on no internal crate.

| Crate | Role | Internal deps | Key files |
|-------|------|---------------|-----------|
| **paavo-proto** | Wire/protocol types + the job state machine. Pure data; everything may depend on it. `deny_unknown_fields`, additive-only wire compat. | — | `src/job.rs` (`JobState`), `src/stream.rs` (NDJSON `WireMessage`), `src/board.rs`, `src/ids.rs` (ULID `JobId`) |
| **paavo-meta** | `no_std` `macro_rules!` macros (`target!`, `timeout!`, `inactivity_timeout!`) that embed metadata into `.paavo.*` ELF sections. Used **only** by scaffolded test crates, not by any paavo binary. | — | `src/lib.rs`, `build.rs`, `paavo.x` |
| **paavo-build** | tar unpack + `cargo build --release` + ELF discovery. Streams/cancels the cargo child. DB-free by design. | — | `src/build.rs`, `src/elf.rs`, `src/tar.rs` |
| **paavo-db** | SQLite persistence + schema. Single writer (paavod), single RO reader (web). refinery migrations. | proto | `src/db.rs`, `src/job.rs`, `src/board.rs`, `migrations/V*.sql` |
| **paavo-probe** | Low-level `probe-rs` 0.31 + defmt driver: connect, flash, RAM-boot, RTT attach, decode. | proto | `src/session.rs`, `src/sections.rs`, `src/event.rs` |
| **paavo-runner** | Per-job board worker + watchdog; drives a probe session to a `JobOutcome`. | proto, probe | `src/worker.rs`, `src/watchdog.rs`, `src/job.rs` |
| **paavo-core** | Scheduler + policy glue (enqueue, quarantine, cancel, build-cache bridge). **No HTTP, no async runtime.** | proto, db, build, runner | `src/scheduler.rs`, `src/enqueue.rs`, `src/quarantine.rs`, `src/build_cache.rs` |
| **paavod** | The daemon. The **only** crate with axum. Two-stage dispatch, routes, config, cron, drain, frame sink. Largest crate. | proto, db, core, build, runner, probe | `src/main.rs`, `src/dispatch.rs`, `src/app.rs`, `src/routes/`, `src/config.rs` |
| **paavo-cli** | Developer HTTP client (clap). The only user-facing TUI. | proto *(dev-only: paavod, paavo-db for integration tests)* | `src/cli.rs`, `src/cmd_run.rs`, `src/cmd_new.rs`, `src/client.rs` |
| **paavo-web** | Read-only web backend for the WASM SPA. Reads SQLite RO; serves a JSON/SSE API and embeds the `paavo-web-ui` (Leptos CSR) bundle; proxies the daemon's NDJSON stream to browser SSE. | proto, db | `src/app.rs`, `src/api/`, `src/proxy.rs`, `src/index.rs`, `src/embed.rs` |

---

## Conventions

- **Errors:** library crates use **`thiserror`** with structured, typed errors
  (`DbError`, `CoreError`, `BuildError`, `ProbeError`) and `#[from]`
  conversions; variants map cleanly to HTTP status (`NotFound`→404,
  `AlreadyExists`/`Conflict`→409, validation→400). Binaries (`paavod`,
  `paavo-cli`, `paavo-web` mains/handlers) use **`anyhow`** with `.context()`.
  HTTP handlers return `Result<T, (StatusCode, String)>`.
- **Logging:** **`tracing`** everywhere with structured fields; each binary
  installs `tracing-subscriber` with an `EnvFilter` (`RUST_LOG`). No `println!`
  for diagnostics.
- **Testing:** the house style is **hand-written test doubles** behind traits
  (`ProbeSession`, `Runner`, `Builder`) — e.g. `FakeRunner`, `FakeSession`,
  `FakeBuilder`, `CountingRunner`, `PanickyRunner`. CLI tests use **`assert_cmd`
  + `predicates`** (`Command::cargo_bin("paavo-cli")`). Filesystem/DB tests use
  **`tempfile`**. Wire types have serde round-trip + byte-level wire-compat
  tests.
- **Migrations:** **`refinery`** with `embed_migrations!("./migrations")` and
  versioned `V{n}__{name}.sql` files. SQLite can't `ALTER` a `CHECK`
  constraint, so changing the set of valid states means an **FK-safe table
  rebuild** (see `V4__awaiting_board.sql`).
- **Data types:** IDs are **ULIDs** (lexicographically time-sortable).
  Timestamps are **epoch-milliseconds `i64`**. Wire/view types deliberately
  exclude server-local filesystem paths.
- **Doc style:** the codebase favors heavy explanatory doc comments — rationale,
  rejected alternatives, spec cross-references, single-writer assumptions.
  Match that density when you touch non-obvious code.

---

## Landmines & gotchas

- **Workspace-excluded crates won't build with the host toolchain.** These are
  intentionally outside `[workspace] members` (see `Cargo.toml` `exclude`):
  `tests/fixtures/smoke-crate`, `soak-tests/`, `dev/probe-rs-spike`,
  `dev/spike-fixture-mcxa266`. The fixtures cross-compile to
  `thumbv8m.main-none-eabihf`. `cargo test --workspace` does **not** touch
  them; don't "fix" them into the workspace.
- **`.cargo/config.toml` inside test crates is load-bearing.** `paavo-cli run`
  strips `target/`, `.git/`, and `Cargo.lock` from the tar but **keeps
  `.cargo/config.toml`** — it sets the defmt log level so the info-level
  `Test OK` frame survives a `--release` build. Drop it and tests look "stuck"
  because the pass marker never reaches the binary.
- **The pass contract is exact:** `info!("Test OK")` (trimmed, exactly that
  string) followed by `cortex_m::asm::bkpt()`. A bkpt without a preceding
  `Test OK` is classified as a test error.
- **Known doc/code drift — trust the code:**
  - `paavo-web` is a **JSON/SSE API backend that embeds the Leptos CSR
    WASM SPA** (`paavo-web-ui`) via `rust-embed` over
    `../paavo-web-ui/dist`, **not** server-rendered HTML. The `dist/`
    bundle is git-ignored and built out of band (`trunk build` /
    `just build-ui`); rust-embed does **not** generate its accessors when
    `dist/` is absent, so `paavo-web` won't compile until the UI bundle
    exists. CI/fresh checkouts must build the UI before
    `cargo build/test --workspace`.
  - `paavo-meta` is **self-contained `macro_rules!` macros**, not a
    re-export of any upstream `*-meta` crate.
  - `insta`, `proptest`, and `mockall` are pinned in
    `[workspace.dependencies]` but are **not actually used** by any crate.
    Don't cite them as conventions or assume snapshot/property tests exist.

---

## Where to look next

- `README.md` — quick starts (developer workstation + lab machine).
- `docs/deployment.md` — Linux deploy, state-dir layout, log retention.
- `docs/hw-smoke-checklist.md` — manual hardware checklist for releases.
- `contrib/` — systemd units, udev rules, annotated `paavo.toml.example`.
- `sample-paavo.toml` — annotated local-dev config (every knob explained).
- `docs/superpowers/specs/` & `plans/` — design docs. The **2026-06-09** master
  spec is the big-picture intent but is **partly stale**; the **2026-06-16**
  specs (parallel build pool, log-frame persistence, live dashboard, startup
  reconciliation) reflect the current implementation and are the better source
  of truth for those subsystems.

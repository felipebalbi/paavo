# paavo — design

**Date:** 2026-06-09
**Status:** Draft for review
**Author:** Felipe Balbi
**Repo:** `paavo` (new, to be created at `github.com/felipebalbi/paavo`)

> Named after Paavo Nurmi, the *Flying Finn* — Olympic distance runner. Fitting
> for a test runner whose nightly job is long-distance: hours-long stability
> soaks against embedded targets.

---

## 1. Purpose & scope

`paavo` is a self-hosted, Linux-based hardware-in-the-loop (HIL) test
orchestrator for the `embassy-mcxa` HAL (and any future embassy chip wired up
to the lab). It runs on one dedicated lab machine that owns a fleet of NXP
eval boards connected via probe-rs probes.

paavo serves two distinct workflows on the same physical fleet:

1. **Nightly automated test runs** against the latest `embassy-rs/embassy`
   `main`, plus paavo's own long-running stability ("soak") tests that are not
   appropriate for the upstream repository.
2. **Ad-hoc developer requests** during the day, via `paavo-cli`, where a
   developer uploads a single test crate, has it built on the lab machine, run
   on the requested board kind, and the result streamed back.

The boards (mcxa266 fleet, rt685-evk fleet) are wired with a semi-standard,
documented harness so any test that targets a given board kind can run on any
healthy instance of that kind without rewiring.

### Non-goals (explicitly out of scope)

- **Not a CI replacement.** paavo does not gate PRs on GitHub, does not post
  status checks, does not run on PR events. It is an unattended lab service
  driven by a clock and by developer CLI calls.
- **Not multi-tenant.** No authentication. paavo binds to a private network
  only; security is delegated to the network perimeter.
- **Not cross-platform** for the daemon. The daemon runs on Linux only.
  `paavo-cli` is a thin HTTP client and works from Linux, macOS, and Windows
  developer workstations.
- **Not a teleprobe replacement on the user-facing side.** We keep using
  `teleprobe-meta`'s `target!()` / `timeout!()` macros inside test source code
  so test authors are unaffected. But paavo does **not** link the `teleprobe`
  binary's internals as a library (teleprobe is currently a binary-only
  crate; refactoring it just for paavo is unjustified). paavo-runner talks to
  `probe-rs` directly and decodes defmt via `defmt-decoder`. See §4.3 and §16.

---

## 2. Background and prior art

- **`embassy-rs/teleprobe`** — already runs the per-binary flash-from-RAM
  + defmt-monitor loop the embassy project relies on. It is, however, a
  binary-only crate (`main.rs`, no `lib.rs`). Refactoring it into a library
  purely so paavo can link it would add maintenance burden on a fork the
  embassy team doesn't promise stability for. Instead, paavo talks to
  `probe-rs` directly and reuses the same on-chip protocol via the public
  `teleprobe-meta` crate (which **is** a library and is what test authors
  interact with). Where teleprobe has solved a non-obvious problem — e.g.
  the NXP RT685S "skip post-load reset" quirk — paavo reimplements the
  equivalent logic on the probe-rs API.
- **`embassy-rs/embassy` tests** (e.g. `tests/mcxa2xx/src/bin/*.rs`) — already
  follow a clean per-binary test convention: success = `defmt::info!("Test
  OK")` followed by `cortex_m::asm::bkpt()`; failure = panic, assert, or
  timeout. We adopt this convention unchanged.
- **`teleprobe-meta`** — provides the `target!()` and `timeout!()` macros that
  embed metadata into the ELF's `.paavo.*` link sections (a paavo-native format that mirrors the embassy teleprobe layout). We extend with
  one new sibling macro (see §6.4) and otherwise consume it as-is.
- **Prior work (`usain`)** — the user's earlier Python test runner whose
  spiritual successor this is. Named for Usain Bolt. paavo is the Rust
  rewrite-from-scratch, with a different name (Paavo Nurmi) to signal that it
  is not a port.

---

## 3. Architecture overview

paavo is **three binaries in one cargo workspace**, separated along genuine
runtime boundaries:

- **`paavod`** — the daemon. Headless, foreground process, supervised by
  systemd. Owns the job queue, the board fleet, the SQLite database, the
  build sandbox, and the HTTP API. Drives flashing and monitoring by talking
  to `probe-rs` directly via the `paavo-probe` crate.
- **`paavo-web`** — the read-only viewer. Leptos SSR web UI, bound by default
  to `127.0.0.1:8081`. Opens the SQLite file in read-only WAL mode. Separate
  systemd unit; can be restarted or upgraded independently of the daemon.
- **`paavo-cli`** — the developer client. HTTP client to `paavod`. Subcommands
  include `run`, `new`, `cancel`, `logs`, `boards`, `jobs`, `board add`,
  `board quarantine`, `board unquarantine`, `board remove`.

### 3.1 Top-level data flow

```
dev workstation                       lab machine (Linux)
┌──────────────┐                      ┌──────────────────────────────────────┐
│ paavo-cli    │     HTTP/JSON        │            paavod                    │
│              │ ───────────────────► │ ┌──────────┐   ┌─────────────────┐   │
│ + crate.tar  │      crate.tar       │ │ HTTP API │──►│ Job queue (mem) │   │
│              │                      │ └──────────┘   └────────┬────────┘   │
│              │ ◄────────────────    │      ▲                  │            │
│              │   NDJSON log frames  │ ┌────┴─────┐    ┌───────▼──────┐     │
└──────────────┘                      │ │ Sched/   │◄──►│ BoardWorker  │     │
                                      │ │ timer    │    │ (per board)  │     │
                                      │ └──────────┘    └──────┬───────┘     │
                                      │      │                 │ paavo-probe │
                                      │      ▼                 ▼  → probe-rs │
                                      │ ┌────────────────────────────────┐   │
                                      │ │     SQLite (WAL mode)          │◄──┐
                                      │ └────────────────────────────────┘   │
                                      │  systemd: paavod.service             │
                                      └──────────────────────────────────────┘
                                                                             │
                                      ┌──────────────────────────────────────┘
                                      ▼
                                      ┌──────────────────────────────────────┐
                                      │            paavo-web                 │
                                      │  Leptos SSR, reads SQLite RO         │
                                      │  systemd: paavo-web.service          │
                                      └──────────────────────────────────────┘
```

**Runner wiring** (M2 stub → M7 real): the `BoardWorker` box above is
`paavo-runner::run_job` and has been real since M2.2 against a mock
`ProbeSession`. The probe-rs adapter behind it — `paavo-probe::RealSession`
— was stubbed at M2.1 (and the `RealRunner` in `paavod::main` was a unit
struct returning `InfraErr` until M7). M7 replaces both: `RealSession`
gets a real probe-rs `Session` + RTT + defmt-decoder; `RealRunner`
becomes a struct that owns the DB handle, the job-logs broker, the
cancellation registry, and the config, so it can fish the ELF path off
the job row and stream `LogFrame`s into the live broker without the
dispatch layer needing new params.

### 3.2 Why two-binary daemon/UI split (not one binary)

- The daemon must stay up to honor the nightly schedule; the UI is
  nice-to-have. Different uptime SLOs.
- The UI bringing in the Leptos toolchain (WASM, trunk, etc) should not force
  the headless daemon build to drag those deps.
- A UI crash must not poison in-flight jobs.

SQLite (WAL mode + busy-timeout) is the only IPC between them: the daemon
writes, the UI reads. The web UI does not need to show "live tailing" of the
currently-running job's log because that is the dev workflow's job (the
streaming NDJSON response from `POST /jobs`); the web UI's job is the
historical view.

---

## 4. Workspace layout

```
paavo/
├── Cargo.toml                      # workspace
├── rust-toolchain.toml             # pin rustc for the daemon
├── README.md
├── LICENSE-APACHE, LICENSE-MIT
│
├── crates/
│   ├── paavo-proto/                # wire types: Job, JobSpec, BoardSpec,
│   │                               #   JobStatus, LogFrame, BoardHealth, etc.
│   │                               # serde (de)serialization. No I/O.
│   │
│   ├── paavo-meta/                  # no_std helper crate for test crates.
│   │                               # Re-exports teleprobe-meta's existing
│   │                               #   target!() and timeout!() macros,
│   │                               #   adds inactivity_timeout!(). Lives in
│   │                               #   paavo until upstreamed (see §16.2).
│   │
│   ├── paavo-db/                   # SQLite schema, migrations, typed queries
│   │                               # Owns the schema. RW and RO handles.
│   │
│   ├── paavo-build/                # Crate-tar unpack, sandbox dir mgmt,
│   │                               #   cargo build invocation, ELF discovery,
│   │                               #   build-cache reuse policy.
│   │
│   ├── paavo-probe/               # Low-level probe driver. Wraps probe-rs
│   │                               #   directly:
│   │                               #   - connect via probe selector
│   │                               #   - parse .paavo.* ELF sections
│   │                               #     (via `object`)
│   │                               #   - load ELF (RAM-from-flash or flash,
│   │                               #     per analysis; RT685S quirk handled)
│   │                               #   - start RTT, decode defmt frames
│   │                               #     (via `defmt-decoder`)
│   │                               #   - emit Event stream
│   │                               #     (Frame | Bkpt | Panic | Disconnect)
│   │                               # No watchdog, no job concept.
│   │
│   ├── paavo-runner/                # Owns a probe for the duration of one job
│   │                               #   via paavo-probe. Runs the inactivity
│   │                               #   + hard-max watchdog. Streams defmt
│   │                               #   frames out as LogFrame.
│   │
│   ├── paavo-core/                 # Scheduler, priority queue, board fleet,
│   │                               #   BoardWorker, quarantine policy.
│   │                               # Glues db + build + runner. No HTTP.
│   │
│   ├── paavod/                     # Binary. HTTP server (axum), config,
│   │                               #   nightly cron, systemd integration,
│   │                               #   signal handling.
│   │
│   ├── paavo-cli/                  # Binary. clap CLI. HTTP client to paavod.
│   │
│   └── paavo-web/                  # Binary. Leptos SSR. Reads paavo-db RO.
│
├── templates/                      # cargo-generate templates (one per board)
│   ├── mcxa266/
│   │   ├── cargo-generate.toml
│   │   ├── Cargo.toml.liquid
│   │   ├── memory.x
│   │   ├── build.rs
│   │   ├── .cargo/config.toml
│   │   └── src/main.rs
│   └── rt685-evk/
│       └── (same structure)
│
├── soak-tests/                     # paavo's own long-running stability tests
│   ├── mcxa266/
│   │   ├── dma-stress-overnight/   # each subdir = one full test crate
│   │   └── ...
│   └── rt685-evk/
│       └── ...
│
├── contrib/
│   ├── paavod.service              # systemd unit
│   ├── paavo-web.service
│   └── paavo.toml.example          # annotated config
│
└── docs/superpowers/specs/         # design + plan docs
```

### 4.1 Crate boundary rules (enforced from day one)

- `paavo-proto` depends on no other workspace crate.
- `paavo-meta` is `no_std`; depends on `teleprobe-meta`. Consumed only by
  scaffolded test crates (not by any paavo binary).
- `paavo-db` depends only on `paavo-proto`.
- `paavo-build` depends only on `paavo-proto`.
- `paavo-probe` depends on `paavo-proto`. Pulls in `probe-rs`,
  `defmt-decoder`, `object`. No other workspace crate.
- `paavo-runner` depends on `paavo-proto` and `paavo-probe`.
- `paavo-core` depends on `paavo-proto`, `paavo-db`, `paavo-build`,
  `paavo-runner`. **No HTTP.**
- `paavod` is the only place `axum` lives.
- `paavo-web` is the only place `leptos` lives.
- `paavo-cli` is the only place a user-facing TUI lives (clap, indicatif).

### 4.2 Why this split

- Integration tests for `paavo-core` can use an in-memory `paavo-db` and a
  fake `paavo-runner` (no probes, no cargo) so nightly scheduling logic is
  unit-testable deterministically.
- `paavo-web` can evolve its Leptos version independently of the daemon.
- A future "remote builder" feature is a `paavo-build` trait swap, not a
  daemon rewrite.

### 4.3 BoardWorker concurrency

- One `BoardWorker` **OS thread** per board (not a tokio task), because
  `probe-rs` is blocking and an OS thread is easier to abandon cleanly when
  a probe call hangs.
- A paired watchdog OS thread per BoardWorker (see §6).
- Communication between the axum task layer and the BoardWorker threads is
  via `crossbeam_channel` or `flume` (mpsc, blocking on the worker side,
  awaitable on the axum side).

---

## 5. Job lifecycle and state machine

### 5.1 States

```
                            ┌──────────┐
                            │ Submitted│  POST /jobs accepted; tar persisted;
                            │ (queued) │  row written to DB
                            └────┬─────┘
                                 │ scheduler picks job + board
                                 ▼
                            ┌──────────┐
                            │ Building │  paavo-build: untar, cargo build,
                            │          │  locate ELF
                            └────┬─────┘
                ┌────────────────┼────────────────┐
       build error                build OK
                ▼                                  ▼
        ┌─────────────┐                     ┌──────────┐
        │ Failed      │                     │ Running  │  paavo-runner attached
        │ (BuildErr)  │                     │          │  to probe, streaming
        └─────────────┘                     └────┬─────┘
                                                 │
            ┌───────────────────┬────────────────┼─────────────────┐
       "Test OK" + bkpt    panic / assert  watchdog tripped   cancelled
            │                   │                │                  │
            ▼                   ▼                ▼                  ▼
       ┌────────┐         ┌──────────┐    ┌──────────┐       ┌──────────┐
       │ Passed │         │ Failed   │    │ TimedOut │       │ Aborted  │
       └────────┘         │ (TestErr)│    └──────────┘       └──────────┘
                          └──────────┘
```

Plus two terminal states reachable from anywhere via infra failure:

- `Failed { InfraErr }` — probe attach failed, mass-erase failed, RTT init
  failed.
- `Aborted { DaemonShutdown }` — daemon got SIGTERM mid-job.

### 5.2 Terminal outcomes (six)

| Outcome              | Counts toward board infra-failure?                |
| -------------------- | ------------------------------------------------- |
| `Passed`             | no                                                |
| `Failed{TestErr}`    | no                                                |
| `Failed{BuildErr}`   | no                                                |
| `Failed{InfraErr}`   | **yes**                                           |
| `TimedOut{Inactivity}` | yes **only** if BoardWorker could not release the probe |
| `TimedOut{HardMax}`  | no                                                |
| `Aborted{User}`      | no                                                |
| `Aborted{DaemonShutdown}` | no                                           |

This split matters: a buggy soak test that hangs the chip would otherwise
quarantine a perfectly good board.

### 5.3 Priority queue rules

Two priorities for v1:

- `Interactive` — submitted via `paavo-cli run`.
- `Scheduled` — submitted by the nightly cron job.

Scheduler algorithm:

1. Pop the highest-priority Submitted job.
2. If multiple healthy boards match its `board_selector`, pick the
   **least-recently-used** healthy board (rotates load + exposes flaky
   boards faster).
3. If no eligible healthy board is free, leave the job in the queue and try
   the next priority cohort.

**Starvation protection**: a `Scheduled` job queued longer than
`starvation_threshold` (default 6 h) is promoted to `Interactive` priority.
Tunable in config.

### 5.4 Cancellation

`paavo-cli cancel <job_id>` effect by current state:

- `Submitted` → row marked `Aborted{User}`, removed from queue.
- `Building` → SIGINT to the child `cargo` process, wait for exit, mark
  `Aborted{User}`.
- `Running` → signal watchdog with `force_cancel`; watchdog sends Cancel to
  BoardWorker.

The HTTP path: `POST /jobs/:id/cancel` calls
`paavo_core::cancel_if_submitted` inline for `Submitted` (204), and
falls back to `AppState::cancellation::signal(id, RunCommand::Cancel)`
for `Building`/`Running`. The registry holds a
`crossbeam_channel::Sender<RunCommand>` per active worker; the watchdog
inside `paavo-runner` maps `Cancel` → `Aborted{User}` and
`DaemonShutdown` → `Aborted{DaemonShutdown}` per the variants in
`paavo_proto::AbortReason`. If the registry has no live sender (worker
already exited, dispatch loop not running yet, terminal row), the
handler returns 409 — "not cancellable in state X".

### 5.5 Board selector

A `JobSpec` includes a `board_selector` that the scheduler matches against
board inventory:

- `{ kind: "mcxa266" }` — any healthy board of this kind. Dev default.
- `{ kind: "mcxa266", instance: "mcxa266-02" }` — specific board (debugging a
  flaky instance).
- `{ kind: "mcxa266", wiring_profile: "alt-spi" }` — boards tagged with named
  wiring profiles; selector requires the profile.

Selectors matching no possible board (e.g. typo `mcxap266`) are **rejected at
enqueue time**, not silently queued forever.

---

## 6. Timeouts, watchdog, drain

### 6.1 Watchdog responsibilities

Inside `paavo-runner`, two threads cooperate per running job:

```
BoardWorker thread (owns probe)
  ├─ runs paavo-probe's Session::run in a loop, pushing every event into an mpsc
  └─ on each event push, updates a shared AtomicInstant "last_activity"

Watchdog thread (paired with the worker)
  ├─ sleeps in 5 s ticks
  ├─ on each tick:
  │    if now() - last_activity > inactivity_timeout
  │      OR now() - start > hard_max:
  │        send Cancel to BoardWorker
  │        if BoardWorker doesn't drop the probe within 10 s grace:
  │           mark job as TimedOut + probe_unresponsive
  │           signal BoardWorker to abandon the probe (worker thread exits)
  │           bump infra_failure counter on the board
  └─ exits when BoardWorker exits
```

### 6.2 Defaults (config-tunable)

- **Inactivity timeout (no defmt frame received)**: **120 s**, overridable per
  test via a new `paavo_meta::inactivity_timeout!()` macro (see §6.4).
- **Ad-hoc hard wall-clock max**: **15 minutes**, overridable via
  `paavo-cli run --timeout 4h`.
- **Scheduled soak hard max**: **4 hours**, overridable via the soak test's
  `paavo_meta::timeout!()`.
- **Daemon-wide ceiling**: **8 hours**. Any test requesting more is refused at
  enqueue time.

### 6.3 SIGTERM drain semantics

- On `SIGTERM`, paavod stops accepting new jobs (HTTP returns 503).
- All active workers' watchdogs are signalled to use
  `min(remaining, grace_period)` as the hard max, with `grace_period`
  defaulting to 60 s.
- Any job still running after grace is marked `Aborted{DaemonShutdown}` with
  partial defmt log persisted.
- v1: an in-flight nightly soak job is *not* saved by extending its timeout.
  v2 may add `--drain-wait=8h`.

### 6.4 New `inactivity_timeout!()` macro

We add a sibling to `paavo_meta::target!()` and `paavo_meta::timeout!()`
(which re-export `teleprobe_meta::{target, timeout}`),
shipped in the `paavo-meta` workspace crate (§4):

```rust
// In paavo-meta/src/lib.rs
//
// Re-export the existing teleprobe-meta macros so test crates only need
// one dependency:
pub use teleprobe_meta::{target, timeout};

/// Set the per-job no-frame inactivity timeout, in seconds.
#[macro_export]
macro_rules! inactivity_timeout {
    ($val:literal) => {
        #[link_section = ".paavo.inactivity_timeout"]
        #[used]
        #[no_mangle]
        static _TELEPROBE_INACTIVITY_TIMEOUT: u32 = $val;
    };
}
```

`paavo-probe` reads this section from the ELF (via `object`); if absent, falls
back to the job's `inactivity_timeout_ms`, which itself falls back to the
daemon default.

The section name uses a `.paavo.*` prefix; the schema mirrors embassy teleprobe's layout but uses a distinct namespace so paavo's tooling owns the wire format end-to-end.

---

## 7. Storage model (SQLite)

WAL mode, single writer (paavod), one reader (paavo-web), no other processes
should open the DB. Five tables:

### 7.1 `board`

| column                         | type | notes                                         |
| ------------------------------ | ---- | --------------------------------------------- |
| `id`                           | TEXT PK | e.g. `mcxa266-01`                          |
| `kind`                         | TEXT | e.g. `mcxa266`, `rt685-evk`                   |
| `probe_selector`               | JSON | VID:PID:serial                                |
| `chip_name`                    | TEXT | for probe-rs                                  |
| `target_name`                  | TEXT | must match `paavo_meta::target!()` in ELF |
| `wiring_profile`               | TEXT | nullable                                      |
| `health`                       | TEXT | enum: `healthy` / `quarantined`               |
| `quarantine_reason`            | TEXT | nullable                                      |
| `consecutive_infra_failures`   | INT  |                                               |
| `last_used_at`                 | INT  | epoch ms                                      |
| `created_at`                   | INT  |                                               |

### 7.2 `job`

| column                | type     | notes                                            |
| --------------------- | -------- | ------------------------------------------------ |
| `id`                  | TEXT PK  | ULID                                             |
| `priority`            | INT      | smaller = higher                                 |
| `submitter`           | TEXT     | identifier from CLI; no auth                     |
| `source`              | TEXT     | enum: `cli` / `scheduler`                        |
| `board_selector`      | JSON     |                                                  |
| `inactivity_timeout_ms` | INT    |                                                  |
| `hard_max_ms`         | INT      |                                                  |
| `state`               | TEXT     | enum: `submitted` / `building` / `running` / `passed` / `failed` / `timedout` / `aborted` |
| `outcome_detail`      | JSON     | nullable, e.g. `{"kind":"Failed","reason":"TestErr"}` |
| `board_id`            | TEXT     | nullable; set when scheduled                     |
| `submitted_at`        | INT      |                                                  |
| `started_at`          | INT      | nullable                                         |
| `finished_at`         | INT      | nullable                                         |
| `tar_blake3`          | TEXT     | hash of the uploaded tar (build cache key)       |
| `tar_path`            | TEXT     | where on disk the tar lives                     |
| `elf_path`            | TEXT     | nullable; set after build                       |
| `cargo_update_packages` | JSON   | array of package names; `paavo_build::BuildPlan` runs `cargo update -p <pkg>` for each before `cargo build`. HTTP-submitted jobs always pass `[]`; the nightly cron threads it through from `[[corpus]].cargo_update`. |

### 7.3 `log_frame`

| column      | type | notes                                |
| ----------- | ---- | ------------------------------------ |
| `job_id`    | TEXT FK |                                   |
| `seq`       | INT  | monotonic per-job                    |
| `ts_us`     | INT  | microseconds since job start         |
| `level`     | TEXT | `trace`/`debug`/`info`/`warn`/`error` |
| `target`    | TEXT |                                      |
| `message`   | TEXT |                                      |

PRIMARY KEY `(job_id, seq)`. Big table; retention policy in §7.6.

### 7.4 `build_cache`

| column         | type    | notes                              |
| -------------- | ------- | ---------------------------------- |
| `tar_blake3`   | TEXT PK | cache key                          |
| `elf_path`     | TEXT    |                                    |
| `built_at`     | INT     |                                    |
| `last_used_at` | INT     |                                    |
| `size_bytes`   | INT     |                                    |

LRU eviction when total `size_bytes` exceeds `build_cache.max_bytes` config
value (default 5 GiB).

### 7.5 `schedule`

| column                | type    | notes                              |
| --------------------- | ------- | ---------------------------------- |
| `id`                  | TEXT PK | e.g. `nightly`                     |
| `cron`                | TEXT    |                                    |
| `enabled`             | INT     | bool                               |
| `last_triggered_at`   | INT     | nullable; updated on every cron fire |
| `last_completed_at`   | INT     | nullable; updated only when the corpus pass produced at least one successful enqueue |

The **corpus** (which test crates to run) is config-file only, not DB —
version-controlled in `paavo.toml`.

**Cron firing contract.** On each fire, paavod walks every `[[corpus]]`
entry. For each entry it lists the first-level subdirectories under
`entry.path` that contain a `Cargo.toml`, tars each one (streamed to
`${state_dir}/uploads/.tmp-<jobid>.tar`, blake3'd in flight, atomically
renamed to `<blake>.tar`), and enqueues one `Scheduled` job per crate
with `board_selector.kind = entry.kind` (no `instance`, no
`wiring_profile` — corpus jobs target the kind-level pool) and
`cargo_update_packages = entry.cargo_update` (threaded into the
build sandbox per §8.1 step 4).

`schedule.last_triggered_at` is bumped at the *start* of every fire
(before the walk). `schedule.last_completed_at` is bumped at the *end*
only if at least one job was successfully enqueued — a misconfigured
corpus that enqueues zero jobs leaves `last_completed_at` unchanged so
operators can detect silent failure on the web UI.

**Cron-during-drain.** When `state.drain.is_draining()` the cron driver
short-circuits the entire fire — neither `last_triggered_at` nor
`last_completed_at` is updated, no enqueues happen, and a single INFO
log line records the skip. M4.3.d's SIGTERM handler also calls
`JobScheduler::shutdown` so subsequent fires don't happen.

### 7.6 Log retention (v1)

- Keep full log for any job whose terminal state was not `Passed` —
  indefinitely.
- Keep full log for `Passed` jobs for **30 days**, then truncate to summary
  lines only (`level >= warn`).
- Vacuum runs nightly after the scheduled run, off-peak.

Tunable in config. `retention.passed_full_log_days = -1` disables truncation.

### 7.7 Why JSON for `outcome_detail`

Variants carry different fields (`BuildErr` → compile diagnostics; `TestErr`
→ panic message + frame number; `TimedOut` → duration + reason). JSON is
flexible; the alternative of separate columns leaves most rows NULL. Both
paavod and paavo-web parse the JSON via the same `paavo-proto` types.

---

## 8. Build environment

paavo does **not** pin embassy. The test crate's own `Cargo.toml` is the
source of truth for which embassy revision is built against:

- A dev iterates against their own `felipebalbi/embassy` branch by pointing
  their test crate at it (`embassy-mcxa = { git = "...", branch = "..." }`).
- Nightly soak test crates in `paavo/soak-tests/mcxa266/*/Cargo.toml` git-dep
  `embassy-rs/embassy` `main`.
- "Periodically pull from embassy-rs/embassy" is therefore implemented by
  `cargo update -p embassy-mcxa` (and friends) inside each soak test crate's
  build sandbox, before `cargo build`. paavo never does git operations on
  embassy itself.

### 8.1 Build sandbox

For each job, paavo-build:

1. Looks up `tar_blake3` in the `build_cache` table. If hit, jump to §8.2.
2. Untars the crate into `${paavo_state}/sandboxes/${job_id}/`.
3. Sets `CARGO_TARGET_DIR=${paavo_state}/cargo-target/` (shared across jobs
   for incremental reuse; cargo's own locking handles concurrent reads).
4. Runs `cargo update -p <pkg>` for every package listed in
   `job.cargo_update_packages`. HTTP-submitted (`paavo-cli run`) jobs
   always pass `[]` so the dep graph is locked at submit time; the
   nightly cron threads each `[[corpus]].cargo_update` entry through
   to here so soak runs pull fresh embassy revisions.
5. Runs `cargo build --release` (build profile from job spec; default
   release).
6. Discovers the ELF via `[package.metadata.embassy].build.artifact-dir` or,
   if absent, by scanning `target/<triple>/release/`.
7. Records the ELF path under `build_cache` and returns it.

### 8.2 Build cache reuse

- Content-addressed by tar `blake3`. If the dev re-runs the exact same tar
  (e.g. testing flakiness), build is instant.
- A 1-byte edit to `src/main.rs` produces a fresh tar hash; cache miss; full
  rebuild (incremental cargo target dir saves most work in practice).
- LRU eviction when total cache size exceeds `build_cache.max_bytes` (default
  5 GiB).

---

## 9. HTTP API (paavod ↔ paavo-cli)

Bound by default to `127.0.0.1:8080`, overridable via config (`server.bind`).
JSON request/response except where noted.

### 9.1 Job submission

`POST /jobs`

- Multipart body with exactly two parts:
  - `metadata` (`Content-Type: application/json`) — `{ "priority":
    "interactive"|"scheduled", "submitter": "<free text>",
    "board_selector": { "kind": "...", "instance": "...", "wiring_profile":
    "..." }, "inactivity_timeout_ms": <u64 optional>, "hard_max_ms":
    <u64 optional> }`. Fields not listed are rejected
    (`#[serde(deny_unknown_fields)]`). `inactivity_timeout_ms` defaults to
    `timeouts.default_inactivity_s * 1000`; `hard_max_ms` defaults to
    `timeouts.default_ad_hoc_hard_max_s * 1000`. **`source` is NOT a wire
    field** — every HTTP submit is recorded as `JobSource::Cli`. The
    scheduler reaches `paavo_core::enqueue_job` directly, bypassing HTTP.
  - `crate` (`Content-Type: application/octet-stream`, `filename=...`) —
    the tarred test crate. Streamed to disk; not buffered in memory.
- Responses:
  - `202 Accepted` with body `{ "job_id": "01H..." }` on enqueue.
  - `400 Bad Request` for: missing/duplicate metadata or crate part;
    `metadata` not valid JSON; selector matches no inventory board;
    `hard_max_ms` exceeds `timeouts.daemon_ceiling_s * 1000`; required
    metadata field missing.
  - `413 Payload Too Large` if the multipart body exceeds
    `server.max_upload_bytes` (default 256 MiB; raise for fleets with
    large vendored deps).
  - `503 Service Unavailable` while paavod is draining (§6.3).
  - `500 Internal Server Error` for unexpected `paavo-db` / `paavo-build`
    / `paavo-core` / I/O failures; the daemon logs the full error.
- Persistence model: the `crate` field is streamed to
  `${state_dir}/uploads/.tmp-<jobid>.tar`, hashed in flight with blake3,
  then atomically renamed to `${state_dir}/uploads/<blake3>.tar`. If the
  final path already exists (a previously-submitted identical tar), the
  temp file is removed without overwriting — the build cache keeps the
  warm copy. Validation (selector + ceiling) happens BEFORE the rename
  so a 400 leaves no orphan tar on disk.

### 9.2 Job log streaming

`GET /jobs/:id/stream`

- Long-lived NDJSON response (`Content-Type: application/x-ndjson`).
  Each line is one JSON object, parseable independently. Four line
  types:
  - `{"type":"frame","frame":{<LogFrame>}}` — one log frame. `frame.seq`
    is monotonic per job; clients can dedup if the historical/live
    boundary races.
  - `{"type":"terminal","outcome":{<JobOutcome>}}` — exactly once on
    the happy path, immediately before the stream closes.
  - `{"type":"lagged","missed":<u64>}` — informational: the live
    broadcast channel dropped `missed` frames because the client
    couldn't keep up. Client should re-fetch from the historical
    endpoint to recover. (`broadcast::RecvError::Lagged` surfaces here.)
  - `{"type":"truncated","reason":"..."}` — degraded close marker.
    Emitted when the stream ends without a `terminal` line (worker
    died without finalizing, lagged eviction ate the Terminal event,
    or DB error while paging historical frames). The client should
    treat this as an unknown-outcome close and re-query via
    `GET /jobs/:id` for the authoritative state.
- The "full historical log" is delivered in 1000-frame pages — there
  is no v1 cap. A DB error while paging surfaces as a `truncated`
  line, not a silent empty body.
- If the job is already terminal when the call arrives the response is
  the historical pages followed by the terminal line, then closes.
- 400 if `:id` is not a valid ULID. 404 if no such job. 500 if the
  DB has terminal state with NULL outcome (corrupted row). Otherwise
  200.
- Serves during drain (operators monitoring shutdown need access to
  the live log of an in-flight job).
- Lifecycle: the handler reads the row FIRST and only subscribes to
  the live broker if the job exists AND is non-terminal. This
  prevents an unauthenticated DoS where `GET /jobs/<random-ulid>/stream`
  in a loop would otherwise materialize one broker entry per request
  and never reclaim it.

### 9.3 Job query

- `GET /jobs/:id` — returns a single `JobView` (current state + outcome
  when terminal). 404 on unknown id. Invalid id (not a valid ULID)
  returns 400.
- `GET /jobs?state=&limit=` — returns `Vec<JobView>` ordered by
  `submitted_at` descending (newest first). Query parameters:
  - `state` (optional) — filter by one of `submitted`, `building`,
    `running`, `passed`, `failed`, `timedout`, `aborted`. Omitted ⇒
    defaults to `submitted` (the operator's queue view). Unknown
    value ⇒ 400.
  - `limit` (optional) — bound on the number of rows. Default 50,
    must be `1..=500`. Unparseable or out-of-range ⇒ 400.
  - **v1 has no pagination cursor**: with > `limit` matching rows the
    response is truncated, not paginated. Cursor-based pagination
    (`?cursor=<opaque>&limit=N` with a `next_cursor` envelope) is a
    planned follow-up; until then the bare `Vec<JobView>` shape is
    the wire contract.
- `POST /jobs/:id/cancel` — see §5.4. v1 carve-out: only `Submitted`
  jobs are cancellable inline (returns 204 + state ⇒ `Aborted`).
  `Building` and `Running` jobs return `409 Conflict` until M4.3
  wires the worker-signal path. Unknown id ⇒ 404. Invalid id ⇒ 400.
- `JobView` wire shape (defined in `paavo-proto`): id, priority,
  submitter, source, board_selector, inactivity_timeout_ms,
  hard_max_ms, state, outcome (Option), board_id (Option),
  submitted_at, started_at (Option), finished_at (Option),
  tar_blake3, cargo_update_packages. **Deliberately excludes** the
  daemon-local filesystem paths (`tar_path`, `elf_path`); those are
  server-internal.
- All three endpoints serve during drain (operators monitoring
  shutdown need read access; cancelling an in-flight job during
  drain is exactly when you'd need it). Same carve-out as §9.4.

### 9.4 Board management

- `GET /boards` — return the current fleet as a JSON array of `BoardView`
  objects. Each entry exposes the static board spec (id, kind, probe
  selector, chip, target, wiring profile, health) plus the operational
  fields: `quarantine_reason` (Option), `consecutive_infra_failures`
  (the auto-quarantine counter), `last_used_at` (epoch ms of most
  recent dispatch, Option), `created_at` (epoch ms of registration).
  Rows are ordered by `id` ascending so the CLI and web UI render a
  stable fleet listing.
- `POST /boards` — add a board (used by `paavo-cli board add`). Body is
  a `BoardSpec`; the daemon rejects any `health != "healthy"` with
  `400 Bad Request` because the quarantine flow requires a `reason`,
  which `BoardSpec` does not carry. Duplicate `id` returns `409
  Conflict`. (v1 does NOT probe-validate the selector against a
  physically connected probe — deferred; flagged as a follow-up so
  ops can register boards out-of-band of probe presence.)
- `POST /boards/:id/quarantine` — manual quarantine. JSON body
  `{"reason": "..."}` is required and rejected with `400` if missing
  or whitespace-only. Unknown id returns `404`.
- `POST /boards/:id/unquarantine` — clear quarantine and reset the
  consecutive-infra-failure counter to 0. Unknown id returns `404`.
- `DELETE /boards/:id` — permanently remove a board from the
  inventory. Guard: the row must currently be quarantined
  (`health = 'quarantined'`) so the operator has already documented
  *why* it is being retired and any in-flight jobs have had a chance
  to drain. Returns `400 Bad Request` if the row is `healthy`,
  `404 Not Found` if the row does not exist, and `409 Conflict` if
  any `job` row references this `board_id` (preserves audit history
  per §11). The FK is enforced by SQLite — `PRAGMA foreign_keys =
  ON` is set at `Db::open`. On success the inventory cache is
  refreshed and `204 No Content` is returned. The CLI exposes this
  as `paavo-cli board remove <id>` (§10.2). Rationale for the
  quarantine-first guard: symmetric with the "quarantine then
  delete" pattern §6.3 already uses for graceful shutdown, and
  blocks the foot-gun where an operator rm's a healthy board the
  dispatcher just claimed.
- All five endpoints continue to serve while paavod is draining
  (§6.3 drain semantics gate `POST /jobs` only — operators must be
  able to quarantine a misbehaving board during shutdown).
- Error envelope is `text/plain` in v1 — a JSON envelope (`{"error":
  "...", ...}`) is a planned follow-up; the wire shape on success is
  already JSON.

### 9.5 Admin

- `POST /admin/purge` — operator-driven dev-loop reset. Wipes all
  job artifacts on disk and in the DB, preserving board and
  schedule rows. On disk: deletes everything under
  `${state_dir}/sandboxes/`, `${state_dir}/uploads/`, and
  `${state_dir}/cargo-target/`. In the DB: truncates `job`,
  `log_frame`, and `build_cache`. Boards and schedules are
  preserved so the operator does not lose registered probes or
  cron rows.
- Returns `204 No Content` on success.
- Returns `409 Conflict` if any `job` row is in state `building`
  or `running` (mirrors the drain gate from §6.3; preserves the
  invariant that the dispatcher never has its sandbox yanked
  mid-flash). The operator must `paavo-cli cancel <id>` (or
  wait) for in-flight jobs to terminate before purge.
- Continues to serve while paavod is draining (no point gating
  this — drain is exactly when an operator would want to reset).
- CLI: `paavo-cli admin purge` (§10.3).
- v1 has no auth, no audit log, no soft-delete recovery. This is
  a dev-loop convenience verb — production operators with valuable
  job history should rely on retention sweeps (§8) instead.

### 9.5 Health

- `GET /health` — liveness probe; returns `200` with a small JSON blob even
  while draining.
- `GET /ready` — readiness; returns `503` during shutdown drain.

---

## 10. CLI surface (`paavo-cli`)

The CLI is the only user-facing terminal experience. All other terminal
behavior (the daemon's logs, the web UI) is operator-facing, not dev-facing.

### 10.1 Developer workflow subcommands

- `paavo-cli run <path> [--board-kind mcxa266] [--instance mcxa266-02] [--timeout 1h] [--inactivity 60s] [--priority interactive]`
  - `<path>` may be a `.rs` file, a crate directory, or a pre-built ELF.
  - If `.rs`: detect parent test-crate (walk up looking for `Cargo.toml`); if
    none, refuse with a hint to run `paavo-cli new` first.
  - If directory: tar the crate.
  - If `.elf`: skip build (paavod accepts a pre-built ELF as a degenerate
    crate tar with a single ELF + marker file).
  - Streams the NDJSON log to the terminal until terminal outcome; exit code
    reflects outcome (0 = Passed, non-zero per outcome class).
- `paavo-cli new <crate-name> --board-kind mcxa266 [--kind quick|soak]`
  - Thin wrapper around `cargo generate` against the paavo repo's
    templates. Requires `cargo-generate` on the user's `PATH`; if
    missing, fails with a clear message ("install with `cargo install
    cargo-generate`") rather than trying to download it. See §10.5.
- `paavo-cli cancel <job_id>`
- `paavo-cli logs <job_id> [--follow]`
- `paavo-cli jobs [--state running] [--limit 20]`

### 10.2 Operator subcommands

- `paavo-cli boards`
- `paavo-cli board add --kind mcxa266 --instance mcxa266-02 --probe 1366:1015:000123456789 --chip MCXA266VFL --target frdm-mcx-a266 [--wiring-profile default]`
- `paavo-cli board quarantine <id> --reason "broken JTAG header"`
- `paavo-cli board unquarantine <id>`
- `paavo-cli board remove <id>` — permanently delete the row. Refused
  unless the board is currently quarantined (the quarantine reason
  doubles as the deletion justification) and unless no `job` row still
  references it. Operators who want to delete a board with referenced
  jobs must wait for `retention` to age them out (per §11).

### 10.3 Admin subcommands

- `paavo-cli admin purge` — dev-loop reset. Calls `POST /admin/purge`
  (§9.5). Wipes all job artifacts on disk (`sandboxes/`, `uploads/`,
  `cargo-target/`) and in the DB (`job`, `log_frame`, `build_cache`)
  while preserving board and schedule rows. Refused with `409
  Conflict` if any job is currently `building` or `running`.
  Intended for the `manual-smoke` loop and for operators recovering
  from a botched state dir; not a substitute for proper retention.

### 10.4 Server discovery

- `PAAVO_HOST` env var (e.g. `http://lab.local:8080`).
- `--host` flag overrides.
- Per-user `~/.config/paavo/cli.toml` for persistent defaults.

### 10.5 `paavo-cli new` template scaffolding (M7)

`paavo-cli new` shells out to `cargo generate` to materialise a test
crate from one of the templates shipped under `templates/` in the paavo
repo. Behaviour contract:

- Required flags: `<crate-name>`, `--board-kind {mcxa266|rt685-evk}`.
  Optional: `--kind {quick|soak}` (defaults to `quick`), `--into <dir>`
  (defaults to `./<crate-name>`), `--templates-path <path>` (defaults
  to the paavo repo root if `paavo-cli` is invoked from inside the
  repo, otherwise to a checkout under `$XDG_CACHE_HOME/paavo/templates`).
- Pre-flight: probe `cargo-generate --version`. If the binary is
  missing or returns non-zero, exit with status 2 and the message
  `cargo-generate not found on PATH. Install with: cargo install
  cargo-generate`. Do NOT auto-install. Operators install once;
  paavo's CLI stays simple.
- Pre-flight: probe that the requested template subdir
  (`<templates>/<board-kind>`) exists. If absent, list available
  board-kinds and exit non-zero.
- Invocation: `cargo generate --path <templates>/<board-kind> --name
  <crate-name> --destination <into> --define test-kind=<kind>
  --define embassy-rev=<pinned-rev>` with output streamed to the
  user's terminal. `embassy-rev` is pinned in the template's
  `cargo-generate.toml` defaults; paavo-cli only overrides it when
  the user passes `--embassy-rev <sha>`. The pinned rev MUST resolve
  to a commit that has the published `embassy-mcxa 0.1.0`.
- Post-success: print one line summarising the next step
  (`cd <name> && cargo build --release && paavo-cli run -p .`).

Hardware-only chip names go in the scaffolded crate's docs, NOT in
the CLI surface — operators copy them into `boards.toml` once per
lab. See §13 for `boards.toml` shape.

---

## 11. Web UI (`paavo-web`)

- Leptos SSR. Bound to `127.0.0.1:8081` by default.
- Opens `${paavo_state}/paavo.sqlite` in read-only WAL mode with a
  conservative busy timeout.
- Pages (v1):
  - **`/`** — dashboard: board fleet health, currently-running jobs,
    last 24h pass/fail summary.
  - **`/jobs`** — filterable list (date range, board, state, source).
  - **`/jobs/:id`** — single job: metadata + full log frames (paginated;
    "tail" mode for in-progress jobs polls every 2 s).
  - **`/boards`** — board details, recent jobs per board, quarantine
    history.
  - **`/schedule`** — nightly run history, next scheduled trigger time.
- No write actions in the UI for v1 (no "cancel job" button, no "quarantine
  board" button). All mutations go through `paavo-cli`. Reasons: keeps the
  daemon/UI contract one-way; avoids needing auth/CSRF in the UI.

---

## 12. cargo-generate templates

In `paavo/templates/<board-kind>/`.

### 12.1 Template inputs (cargo-generate placeholders)

- `project-name` (required)
- `board-kind` (single-select; locked when generated via `paavo-cli new
  --board-kind`)
- `test-kind` (single-select: `quick` / `soak`; default `quick`)
- `embassy-rev` (default `main`; can be a branch, tag, or SHA)

### 12.2 What the template produces

```
<project-name>/
├── Cargo.toml             # depends on embassy-{mcxa,...}, defmt, defmt-rtt,
│                          #   panic-probe, paavo-meta, cortex-m,
│                          #   cortex-m-rt, plus paavo-test-prelude.
├── Cargo.lock             # committed for reproducibility
├── build.rs               # wires link_ram.x (vendored under templates/shared/)
├── memory.x               # board-specific
├── .cargo/config.toml     # target triple; no runner = (run via paavo-cli)
└── src/main.rs            # skeleton with paavo_meta::target!() set,
                           #   embassy main, "TODO: write your test here"
```

### 12.3 The crate shape is the wire format

The scaffolded crate shape is identical to:

- what `paavo-cli run <crate-dir>` tars and uploads, and
- what lives in `paavo/soak-tests/<board-kind>/<name>/`.

This means a test that started life as `paavo-cli new my-test`, was iterated
on by a dev with `paavo-cli run`, and is then promoted into the nightly
corpus, can be moved into `soak-tests/` with no changes.

### 12.4 Shared linker scripts

`paavo/templates/shared/link_ram_cortex_m.x` is a verbatim copy of the same
file from `embassy-rs/teleprobe` (MIT/Apache-2.0 dual-licensed, attribution
in a header comment). Templates' `build.rs` writes it into `OUT_DIR` and
passes `-Tlink_ram.x` to the linker so test binaries are linked for
flash-from-RAM execution, matching the upstream embassy convention.

The `paavo.x` linker fragment (which preserves the `.paavo.*`
sections containing target / timeout / inactivity_timeout) ships with
`paavo-meta`'s own `build.rs`; downstream test crates pick it up via
`-Tpaavo.x` in their RUSTFLAGS.

### 12.5 `paavo-test-prelude` (deferred)

A small `no_std` lib that re-exports the common imports (`defmt`,
`embassy_executor`, `embassy_time`, `panic_probe as _`, `defmt_rtt as _`,
`paavo_meta`). Lives in the paavo repo as a published-or-path-dep crate.
**Deferred to a later milestone**; v1 templates spell out all imports
explicitly, and use `paavo-meta` directly for `target!()` / `timeout!()` /
`inactivity_timeout!()`.

---

## 13. Configuration (`paavo.toml`)

A single TOML file on the lab machine, default path
`/etc/paavo/paavo.toml`, overridable via `--config` and `PAAVO_CONFIG`.

```toml
[server]
bind = "127.0.0.1:8080"
state_dir = "/var/lib/paavo"
# Per-request body cap for POST /jobs multipart uploads.
# Default = 256 MiB. Raise for fleets with large vendored deps.
max_upload_bytes = 268_435_456

[web]
bind = "127.0.0.1:8081"

[timeouts]
default_inactivity_s = 120
default_ad_hoc_hard_max_s = 900       # 15 min
default_scheduled_hard_max_s = 14400  # 4 h
daemon_ceiling_s = 28800              # 8 h
shutdown_grace_s = 60

[scheduler]
starvation_threshold_s = 21600        # 6 h
# Cron expression: 6-field `sec min hour dom mon dow` (the `cron` crate's
# native form, also what `tokio-cron-scheduler` parses). Time zone is
# the daemon process's local TZ. Example below = "every day at 19:00:00".
nightly_cron = "0 0 19 * * *"

[build_cache]
max_bytes = 5_368_709_120             # 5 GiB

[retention]
passed_full_log_days = 30             # -1 = never truncate

[quarantine]
consecutive_infra_failures = 3

[[corpus]]
name = "embassy-mcxa-regression"
# Board kind the corpus targets. Must match a `board.kind` registered
# via `paavo-cli board add`. The cron driver uses this directly when
# building the selector for every Scheduled job it enqueues; the
# corpus PATH basename is not parsed.
kind = "mcxa266"
path = "/var/lib/paavo/checkouts/embassy/tests/mcxa266"
# Packages to `cargo update -p ...` before building each crate. The
# nightly run threads these through to `paavo_build::BuildPlan::cargo_update_packages`
# so each rebuild pulls fresh revisions of the listed deps (the
# regression-detection purpose of the nightly).
cargo_update = ["embassy-mcxa", "embassy-executor"]

[[corpus]]
name = "paavo-soak-mcxa266"
kind = "mcxa266"
path = "/var/lib/paavo/checkouts/paavo/soak-tests/mcxa266"
cargo_update = []
```

Plus boards, which live in a separate file managed by `paavo-cli board add`
(so an admin command does not have to know the format of the main config):

```toml
# /var/lib/paavo/boards.toml
[[board]]
id = "mcxa266-01"
kind = "mcxa266"
probe_selector = { vid = "1366", pid = "1015", serial = "000123456789" }
chip_name = "MCXA266VFL"
target_name = "frdm-mcx-a266"
wiring_profile = "default"
```

---

## 14. Deployment

Linux only. Installed via `cargo install --path crates/paavod` (and `paavod`
+ `paavo-web` separately). systemd units shipped under `contrib/`.

### 14.1 systemd

`contrib/paavod.service` — runs as a dedicated `paavo` user, restart on
failure, `ProtectSystem=strict`, `ReadWritePaths=/var/lib/paavo`,
`StateDirectory=paavo`, no privileges except USB device access (via udev
rules for the probes).

`contrib/paavo-web.service` — same `paavo` user, only needs read access to
`/var/lib/paavo/paavo.sqlite*`.

### 14.2 udev

Document the probe-rs udev rules in `contrib/99-probes.rules`. paavo does
*not* install them automatically; the operator runs `cp contrib/99-probes.rules /etc/udev/rules.d/`.

### 14.3 Security posture (v1)

- No auth. Daemon binds `127.0.0.1` by default. Operator who wants LAN
  access reconfigures bind + protects with firewall / WireGuard / Tailscale.
- Web UI same posture.
- No TLS in v1. If a future v2 wants LAN exposure, add reverse-proxy notes
  (caddy / nginx terminating TLS).

---

## 15. Testing strategy

- **`paavo-proto`** — pure types; doctests for serde round-trip.
- **`paavo-db`** — integration tests against in-memory SQLite; schema
  migration tests forward and backward (where possible).
- **`paavo-build`** — feed it a tiny fixture crate (a no-op embedded crate
  pinned in `tests/fixtures/`); assert it produces an ELF. Build cache
  reuse test.
- **`paavo-probe`** — unit-test the .paavo.* section parser against
  fixture ELFs in `tests/fixtures/`. The probe-rs adapter is harder to unit
  test without hardware; cover it with a trait + fake-probe mock for the
  parts paavo-runner cares about (event emission, disconnect simulation,
  RAM-vs-flash decision).
- **`paavo-runner`** — fake `paavo-probe::Session` (trait + mock impl) covers
  all event shapes; watchdog test that fires Cancel after configured silence,
  and a separate test for the hard-max path.
- **`paavo-core`** — integration tests with in-memory DB and fake runner;
  cover: priority ordering, starvation promotion, board LRU pick,
  quarantine on N consecutive infra failures, board un-quarantine resets
  counter, board selector rejection.
- **`paavod`** — end-to-end with `axum`'s `TestServer`; cover full job
  lifecycle with fake runner.
- **`paavo-cli`** — assert_cmd-style tests against `paavod` test server.
- **Hardware-in-the-loop smoke test** — manual, run by hand on the dev
  workstation any time the RealRunner / RealSession code changes. Spin
  up paavod against a real mcxa266 (and rt685-evk once supported),
  submit one passing test, confirm `Passed` outcome and decoded defmt
  output in the NDJSON tail. Gated in CI under `PAAVO_HW=1` with
  `#[ignore]` so the default `cargo test --workspace` stays
  hardware-free.

---

## 16. Prerequisite work (outside the paavo repo)

### 16.1 Upstreaming `teleprobe-meta::inactivity_timeout!()`

The macro lives in the paavo workspace as part of `paavo-meta` (§4, §6.4).
Once it's exercised in production, propose it upstream to
`embassy-rs/teleprobe`'s `teleprobe-meta` crate. The section name
(`.paavo.inactivity_timeout`) so per-test overrides ride alongside the
existing target/timeout sections under the same paavo namespace.

Until accepted upstream, scaffolded test crates depend on `paavo-meta`. After
acceptance, `paavo-meta` becomes a thin re-export shim and eventually goes
away.

### 16.2 (Not required) teleprobe library refactor

Earlier drafts proposed refactoring `felipebalbi/teleprobe` into a library so
`paavo-runner` could link it. This was rejected: paavo uses `probe-rs` and
`defmt-decoder` directly via the `paavo-probe` crate, and reuses
`teleprobe-meta` (which is already a library) for the on-ELF metadata
convention. The teleprobe binary is not modified.

---

## 17. Items deferred to v2 (explicit)

- Per-job env vars / build features overrides (today the test crate's
  Cargo.toml is the source of truth).
- Job dependencies / fan-out (every job is independent).
- Result-derived alerts (notify on first failure of a previously-passing
  test).
- Auto power-cycle on board failure (requires controllable USB hub / smart
  plug hardware).
- Auth (any kind).
- TLS / LAN exposure of the web UI.
- Multi-machine paavo (one daemon, many lab machines as probe-rs sources).
- `paavo-test-prelude` shared crate (§12.5).
- Web UI write actions.
- Per-board subprocess workers (Approach C from brainstorming).

### Deferred from M7 (RealRunner happy-path is in scope; these are not)

M7 ships the **minimum viable** real-hardware loop: one happy-path
"Test OK" against a real MCX-A266 EVK with defmt decoding. Everything
below is deliberately out of M7's scope and rolled to M8:

- **Cancellation mid-flash / mid-run**: `paavo-cli cancel <job_id>`
  during `download_file` or during RTT polling. M7's RealRunner reads
  the cancellation receiver but does not act on it; the watchdog
  still enforces hard-max and inactivity timeouts so the job will
  always terminate, just not on user demand.
- **Drain interrupting a running session**: SIGTERM during a flash
  today waits for the worker thread to finish naturally (which it
  always will inside the hard-max budget). Explicit mid-flash abort
  is M8.
- **Hard-max watchdog killing a stuck probe**: today, if probe-rs
  itself wedges (deadlock inside `download_file`, lost USB), the
  watchdog cannot interrupt it. The watchdog thread will mark the
  job `TimedOut{HardMax}` in the DB, but the actual probe-owning
  thread may continue to occupy the probe for as long as probe-rs
  takes to error out. Spec §6.1 envisions a `probe_unresponsive`
  flag for this case; M8 wires it.
- **Multi-probe selection by selector** when more than one matching
  probe is plugged in (M7 errors with "ambiguous selector"; M8 picks
  by `BoardSpec.instance_id`).
- **rt685-evk RealSession parity** — same code path, different chip
  name + RAM-only memory map. Adds a second template + a second
  ProbeSelector path. M8.
- **`paavod` running on Linux + udev rules** — M7's smoke is Windows-host
  by user choice. The contrib/ systemd + udev assets shipped in M6 are
  unchanged; validating them against a real Linux lab box is M8.

---

## 18. Open questions for spec review

These were recorded with the recommended answer; flag any that should
change before we move to the implementation plan.

1. ~~teleprobe lib refactor as a prerequisite work item~~ — **resolved
   during review**: dropped entirely. paavo talks to `probe-rs` directly via
   `paavo-probe`. See §16.2.
2. **`soak-tests/` lives in the paavo repo for v1** — recommended **yes**.
   Alternative: separate `paavo-soak-tests` repo. Easy to split later if it
   becomes annoying.
3. **`outcome_detail` as JSON column vs. separate columns** — recommended
   **JSON**. Tradeoffs in §7.7.

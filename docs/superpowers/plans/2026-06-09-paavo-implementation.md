# paavo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build paavo — a self-hosted Linux HIL test runner for embassy-mcxa with three binaries (`paavod` daemon, `paavo-web` viewer, `paavo-cli` client) following TDD throughout.

**Architecture:** Cargo workspace with 10 crates separated along genuine runtime boundaries (proto/meta/db/build/probe/runner/core + 3 binaries). SQLite WAL for persistence and daemon→UI IPC. probe-rs + defmt-decoder driven directly via `paavo-probe`. One OS thread per board for blocking probe-rs work, paired watchdog thread.

**Tech Stack:** Rust 1.95, axum, clap, rusqlite + refinery, probe-rs 0.31, defmt-decoder 1.1.0, blake3, ulid, tokio-cron-scheduler, cargo-generate (templates). `paavo-meta` (own metadata macro crate, no upstream dep) emits the `.paavo.*` ELF sections that `paavo-probe` parses. Web UI is server-side HTML via axum (no client framework); UnoCSS (CDN runtime) provides utility-class styling.

**Spec reference:** `docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md`

---

## Milestone overview

- **M0** — Scaffold workspace, 10 empty crate skeletons with boundary-correct deps, CI
- **M1** — Foundation crates: `paavo-proto`, `paavo-meta`, `paavo-db`
- **M2** — Probe layer: `paavo-probe`, `paavo-runner` (with fake-probe for tests)
- **M3** — Build + core: `paavo-build`, `paavo-core` (scheduler, board fleet)
- **M4** — Daemon + CLI: `paavod`, `paavo-cli`
- **M5** — Web UI: `paavo-web` (server-side HTML via axum + UnoCSS CDN runtime, read-only)
- **M6** — Templates, soak tests, ops (systemd, udev, README)

Every task within a milestone is one of:
- Write failing test → run → confirm FAIL
- Implement minimal code → run → confirm PASS
- Commit

Each step is 2-5 minutes. Bite-sized. Commits are frequent and atomic.

---

## Milestone 0 — Scaffold

Goal: empty but correctly-wired workspace. Every crate compiles. CI runs `fmt`, `clippy`, `test` on push.

### Task 0.1: Workspace root files

**Files:**
- Create: `D:\workspace\paavo\Cargo.toml`
- Create: `D:\workspace\paavo\rust-toolchain.toml`
- Create: `D:\workspace\paavo\.gitignore`
- Create: `D:\workspace\paavo\LICENSE-APACHE`
- Create: `D:\workspace\paavo\LICENSE-MIT`
- Create: `D:\workspace\paavo\README.md`

- [ ] **Step 1: Create rust-toolchain.toml**

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.95.0"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

- [ ] **Step 2: Create .gitignore**

`.gitignore`:
```
/target
**/*.rs.bk
Cargo.lock.bak
.idea/
.vscode/
*.swp
*.swo
.DS_Store

# paavod runtime state (when run locally for dev)
/var-paavo/
*.sqlite
*.sqlite-wal
*.sqlite-shm
```

- [ ] **Step 3: Create workspace Cargo.toml**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = [
    "crates/paavo-proto",
    "crates/paavo-meta",
    "crates/paavo-db",
    "crates/paavo-build",
    "crates/paavo-probe",
    "crates/paavo-runner",
    "crates/paavo-core",
    "crates/paavod",
    "crates/paavo-cli",
    "crates/paavo-web",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://github.com/felipebalbi/paavo"
authors = ["Felipe Balbi <febalbi@microsoft.com>"]
rust-version = "1.95"

[workspace.dependencies]
# Internal
paavo-proto  = { path = "crates/paavo-proto" }
paavo-meta   = { path = "crates/paavo-meta" }
paavo-db     = { path = "crates/paavo-db" }
paavo-build  = { path = "crates/paavo-build" }
paavo-probe  = { path = "crates/paavo-probe" }
paavo-runner = { path = "crates/paavo-runner" }
paavo-core   = { path = "crates/paavo-core" }

# Serialization / data
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
toml        = "0.8"
ulid        = { version = "1.2.1", features = ["serde"] }
blake3      = "1.8.5"
hex         = "0.4"

# Async / HTTP / runtime
tokio       = { version = "1", features = ["full"] }
axum        = { version = "0.7", features = ["multipart", "macros"] }
tower       = "0.5"
tower-http  = { version = "0.6", features = ["trace", "limit"] }
reqwest     = { version = "0.12", features = ["json", "multipart", "stream"] }
futures     = "0.3"
bytes       = "1"

# Errors / logging
anyhow      = "1"
thiserror   = "2"
tracing     = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Storage
rusqlite    = { version = "0.32", features = ["bundled"] }
refinery    = { version = "0.8", features = ["rusqlite"] }

# Scheduling
tokio-cron-scheduler = "0.15.1"
cron        = "0.12"

# Probe / embedded decode
probe-rs       = "0.31"
defmt-decoder  = "1.1.0"
object         = "0.36"

# CLI / TUI
clap        = { version = "4", features = ["derive", "env"] }
indicatif   = "0.17"

# Test deps
tempfile    = "3"
proptest    = "1"
assert_cmd  = "2"
predicates  = "3"
mockall     = "0.13"
insta       = { version = "1", features = ["json"] }

# Concurrency
parking_lot     = "0.12"
crossbeam-channel = "0.5"

# Misc
chrono      = { version = "0.4", features = ["serde"] }
once_cell   = "1"
tar         = "0.4"
walkdir     = "2"

[profile.release]
lto = "thin"
codegen-units = 1
debug = 1

[profile.dev]
opt-level = 1
```

> Note: `paavo-meta` owns its metadata macros (`target!`, `timeout!`, `inactivity_timeout!`) directly — no external git dep, no upstream coordination cost. The macros emit `.paavo.*` ELF sections that `paavo-probe` reads; the namespace is owned end-to-end by this workspace.

- [ ] **Step 4: Create LICENSE-APACHE**

Run:
```pwsh
Invoke-WebRequest -Uri "https://www.apache.org/licenses/LICENSE-2.0.txt" -OutFile "D:\workspace\paavo\LICENSE-APACHE"
```

If offline, copy the standard Apache 2.0 license text manually.

- [ ] **Step 5: Create LICENSE-MIT**

`LICENSE-MIT`:
```
MIT License

Copyright (c) 2026 Felipe Balbi

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 6: Create README.md skeleton**

`README.md`:
```markdown
# paavo

Self-hosted Linux hardware-in-the-loop test runner for the `embassy-mcxa` HAL.

Named after Paavo Nurmi — Olympic distance runner — fitting for a test runner
whose nightly job is long-distance soaks against embedded targets.

## Status

Pre-1.0. Under active development. See
`docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md` for the
design.

## Quick start

(Quickstart instructions land in Milestone 6.)

## License

Dual-licensed under MIT or Apache-2.0 at your option.
```

- [ ] **Step 7: Verify the empty workspace parses**

Run: `cargo metadata --format-version 1 --no-deps`
Expected: fails because no member crates exist yet. That's OK — we'll fix in Task 0.2. Skip this step or expect error `failed to load manifest for workspace member`.

- [ ] **Step 8: Commit**

```pwsh
git -C D:\workspace\paavo add Cargo.toml rust-toolchain.toml .gitignore LICENSE-APACHE LICENSE-MIT README.md
git -C D:\workspace\paavo commit -m "feat(scaffold): workspace root, toolchain pin, licenses"
```

---

### Task 0.2: 10 crate skeletons, boundary-correct deps

Each crate gets a `Cargo.toml` (with only allowed workspace deps) and a `src/lib.rs` (or `src/main.rs` for binaries) containing a single trivial doctest so `cargo test --workspace` has something to run from day one.

**Files (create all):**
- `crates/paavo-proto/Cargo.toml`, `crates/paavo-proto/src/lib.rs`
- `crates/paavo-meta/Cargo.toml`, `crates/paavo-meta/src/lib.rs`
- `crates/paavo-db/Cargo.toml`, `crates/paavo-db/src/lib.rs`
- `crates/paavo-build/Cargo.toml`, `crates/paavo-build/src/lib.rs`
- `crates/paavo-probe/Cargo.toml`, `crates/paavo-probe/src/lib.rs`
- `crates/paavo-runner/Cargo.toml`, `crates/paavo-runner/src/lib.rs`
- `crates/paavo-core/Cargo.toml`, `crates/paavo-core/src/lib.rs`
- `crates/paavod/Cargo.toml`, `crates/paavod/src/main.rs`
- `crates/paavo-cli/Cargo.toml`, `crates/paavo-cli/src/main.rs`
- `crates/paavo-web/Cargo.toml`, `crates/paavo-web/src/main.rs`

- [ ] **Step 1: Create paavo-proto crate**

`crates/paavo-proto/Cargo.toml`:
```toml
[package]
name = "paavo-proto"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "Wire types and protocol definitions shared across the paavo workspace."

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
ulid = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }
```

`crates/paavo-proto/src/lib.rs`:
```rust
//! Wire types and protocol definitions for paavo.
//!
//! This crate has no workspace dependencies. It is pure data: every other
//! paavo crate is permitted to depend on `paavo-proto`, and `paavo-proto`
//! depends on none of them.
//!
//! ```
//! assert_eq!(paavo_proto::CRATE_NAME, "paavo-proto");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-proto";
```

- [ ] **Step 2: Create paavo-meta crate (no_std)**

`crates/paavo-meta/Cargo.toml`:
```toml
[package]
name = "paavo-meta"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
description = "no_std metadata macros for paavo test crates: target!, timeout!, inactivity_timeout!. Self-contained, no upstream dep."
build = "build.rs"

[dependencies]
```

> Empty `[dependencies]` is intentional — `paavo-meta` owns its macros end-to-end. The companion `build.rs` (added in Task 1.2) writes the linker fragment that preserves `.paavo.*` sections. For Task 0.2's skeleton we leave `build.rs` for Task 1.2 to add; the empty `lib.rs` below will compile without it.

`crates/paavo-meta/src/lib.rs`:
```rust
//! no_std metadata helpers for paavo test crates.
//!
//! Task 1.2 fills in the `target!`, `timeout!`, and `inactivity_timeout!`
//! macros along with the `build.rs` that ships the linker fragment.
//! For Task 0.2 (scaffolding) this crate is intentionally empty.
#![no_std]
#![forbid(unsafe_code)]
```

- [ ] **Step 3: Create paavo-db crate**

`crates/paavo-db/Cargo.toml`:
```toml
[package]
name = "paavo-db"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "SQLite schema, migrations, and typed queries for paavo."

[dependencies]
paavo-proto = { workspace = true }
rusqlite = { workspace = true }
refinery = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
ulid = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/paavo-db/src/lib.rs`:
```rust
//! SQLite-backed persistence for paavo.
//!
//! ```
//! assert_eq!(paavo_db::CRATE_NAME, "paavo-db");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-db";
```

- [ ] **Step 4: Create paavo-build crate**

`crates/paavo-build/Cargo.toml`:
```toml
[package]
name = "paavo-build"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
description = "Untar, cargo build, and ELF discovery for paavo test crates."

[dependencies]
tar = { workspace = true }
blake3 = { workspace = true }
walkdir = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

> Note: dep list is intentionally lean. `paavo-build` does not depend on any other workspace crate (spec §4.1) and has no DB plumbing — the cache layer that pairs ELFs with `paavo_db::BuildCacheEntry` lives in `paavo-core::build_cache` (Task 3.2.e). `tempfile` is `[dev-dependencies]` only — production sandboxes are owned by paavod (`StateDir::sandboxes_dir`), not allocated by `paavo-build`.

`crates/paavo-build/src/lib.rs`:
```rust
//! Sandbox tar unpack, `cargo build`, and ELF discovery for paavo.
//!
//! ```
//! assert_eq!(paavo_build::CRATE_NAME, "paavo-build");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-build";
```

- [ ] **Step 5: Create paavo-probe crate**

`crates/paavo-probe/Cargo.toml`:
```toml
[package]
name = "paavo-probe"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "Low-level probe-rs + defmt-decoder driver for paavo."

[dependencies]
paavo-proto    = { workspace = true }
probe-rs       = { workspace = true }
defmt-decoder  = { workspace = true }
object         = { workspace = true }
thiserror      = { workspace = true }
tracing        = { workspace = true }
parking_lot    = { workspace = true }
crossbeam-channel = { workspace = true }
chrono         = { workspace = true }
```

`crates/paavo-probe/src/lib.rs`:
```rust
//! Low-level probe driver. Wraps `probe-rs` and `defmt-decoder` and parses
//! `.paavo.*` ELF sections.
//!
//! ```
//! assert_eq!(paavo_probe::CRATE_NAME, "paavo-probe");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-probe";
```

- [ ] **Step 6: Create paavo-runner crate**

`crates/paavo-runner/Cargo.toml`:
```toml
[package]
name = "paavo-runner"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
description = "Watchdog + paavo-probe driver for one in-flight paavo job."

[dependencies]
paavo-proto = { workspace = true }
paavo-probe = { workspace = true }
parking_lot = { workspace = true }
crossbeam-channel = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/paavo-runner/src/lib.rs`:
```rust
//! Per-job runner: owns one probe via paavo-probe, runs the inactivity
//! and hard-max watchdog, emits LogFrame events.
//!
//! ```
//! assert_eq!(paavo_runner::CRATE_NAME, "paavo-runner");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-runner";
```

- [ ] **Step 7: Create paavo-core crate**

`crates/paavo-core/Cargo.toml`:
```toml
[package]
name = "paavo-core"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
description = "Scheduler, board fleet, quarantine policy, build-cache glue for paavo. No HTTP, no async runtime."

[dependencies]
paavo-proto  = { workspace = true }
paavo-db     = { workspace = true }
paavo-build  = { workspace = true }
paavo-runner = { workspace = true }
rusqlite     = { workspace = true }
thiserror    = { workspace = true }
chrono       = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/paavo-core/src/lib.rs`:
```rust
//! Scheduler, board fleet, and quarantine policy for paavo. No HTTP lives
//! in this crate; see `paavod` for the axum surface.
//!
//! ```
//! assert_eq!(paavo_core::CRATE_NAME, "paavo-core");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-core";
```

- [ ] **Step 8: Create paavod binary**

`crates/paavod/Cargo.toml`:
```toml
[package]
name = "paavod"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "paavo daemon: HTTP server, scheduler, board fleet owner."

[dependencies]
paavo-proto = { workspace = true }
paavo-db    = { workspace = true }
paavo-core  = { workspace = true }
axum        = { workspace = true }
tokio       = { workspace = true }
tower       = { workspace = true }
tower-http  = { workspace = true }
tokio-cron-scheduler = { workspace = true }
serde       = { workspace = true }
serde_json  = { workspace = true }
toml        = { workspace = true }
clap        = { workspace = true }
anyhow      = { workspace = true }
thiserror   = { workspace = true }
tracing     = { workspace = true }
tracing-subscriber = { workspace = true }
chrono      = { workspace = true }
futures     = { workspace = true }
bytes       = { workspace = true }

[dev-dependencies]
reqwest    = { workspace = true }
tempfile   = { workspace = true }
```

`crates/paavod/src/main.rs`:
```rust
//! paavo daemon entry point.
fn main() {
    println!("paavod placeholder; see plan Milestone 4");
}
```

- [ ] **Step 9: Create paavo-cli binary**

`crates/paavo-cli/Cargo.toml`:
```toml
[package]
name = "paavo-cli"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "paavo command-line client."

[dependencies]
paavo-proto = { workspace = true }
clap        = { workspace = true }
reqwest     = { workspace = true }
tokio       = { workspace = true }
serde       = { workspace = true }
serde_json  = { workspace = true }
toml        = { workspace = true }
anyhow      = { workspace = true }
thiserror   = { workspace = true }
tracing     = { workspace = true }
tracing-subscriber = { workspace = true }
indicatif   = { workspace = true }
chrono      = { workspace = true }
futures     = { workspace = true }
bytes       = { workspace = true }
tar         = { workspace = true }
walkdir     = { workspace = true }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile   = { workspace = true }
```

`crates/paavo-cli/src/main.rs`:
```rust
//! paavo-cli entry point.
fn main() {
    println!("paavo-cli placeholder; see plan Milestone 4");
}
```

- [ ] **Step 10: Create paavo-web binary**

`crates/paavo-web/Cargo.toml`:
```toml
[package]
name = "paavo-web"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "paavo read-only web viewer (server-side HTML + UnoCSS)."

[dependencies]
paavo-proto = { workspace = true }
paavo-db    = { workspace = true }
axum        = { workspace = true }
tokio       = { workspace = true }
tower       = { workspace = true }
tower-http  = { workspace = true }
serde       = { workspace = true }
serde_json  = { workspace = true }
toml        = { workspace = true }
clap        = { workspace = true }
anyhow      = { workspace = true }
tracing     = { workspace = true }
tracing-subscriber = { workspace = true }
chrono      = { workspace = true }
parking_lot = { workspace = true }
rusqlite    = { workspace = true }
```

`crates/paavo-web/src/main.rs`:
```rust
//! paavo-web entry point.
fn main() {
    println!("paavo-web placeholder; see plan Milestone 5");
}
```

- [ ] **Step 11: Verify the whole workspace compiles**

Run: `cargo build --workspace`
Expected: completes successfully (slow first time; pulls down probe-rs and the axum/tokio stack). Warnings about unused deps are fine — clippy in Task 0.3 will handle them later.

- [ ] **Step 12: Verify the smoke doctests run**

Run: `cargo test --workspace --doc`
Expected: each of `paavo-proto`, `paavo-db`, `paavo-build`, `paavo-probe`, `paavo-runner`, `paavo-core` reports 1 doctest passed. `paavo-meta` reports 0 (it's `no_std` and we didn't add a host doctest).

- [ ] **Step 13: Commit**

```pwsh
git -C D:\workspace\paavo add crates
git -C D:\workspace\paavo commit -m "feat(scaffold): 10 crate skeletons with boundary-correct deps"
```

---

### Task 0.3: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `.github/workflows/README.md`

- [ ] **Step 1: Create CI workflow**

`.github/workflows/ci.yml`:
```yaml
name: ci

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-Dwarnings"

jobs:
  fmt:
    name: rustfmt
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.95.0
        with:
          components: rustfmt
      - run: cargo fmt --all -- --check

  clippy:
    name: clippy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.95.0
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps for probe-rs
        run: sudo apt-get update && sudo apt-get install -y libudev-dev pkg-config
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    name: cargo test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.95.0
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps for probe-rs
        run: sudo apt-get update && sudo apt-get install -y libudev-dev pkg-config
      - run: cargo test --workspace --all-features
```

- [ ] **Step 2: Brief note for the workflows dir**

`.github/workflows/README.md`:
```markdown
CI for paavo. `ci.yml` runs `fmt`, `clippy`, and `cargo test --workspace`
against Rust 1.95 on Ubuntu. probe-rs needs `libudev-dev` on Linux even for
host-only tests because of its dev-dependency graph.
```

- [ ] **Step 3: Run fmt locally to make sure it passes**

Run: `cargo fmt --all -- --check`
Expected: no output (success).

- [ ] **Step 4: Run clippy locally**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: passes. If a workspace-dep is unused in one crate, drop it from that crate's Cargo.toml.

- [ ] **Step 5: Run the full test suite locally**

Run: `cargo test --workspace`
Expected: all crates compile, doctests pass, 0 unit tests fail (because we have 0).

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add .github
git -C D:\workspace\paavo commit -m "ci: workspace fmt + clippy + test on push and PR"
```

---

### Milestone 0 exit criteria

- [ ] `cargo build --workspace` is green
- [ ] `cargo test --workspace` is green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is green
- [ ] `cargo fmt --all -- --check` is green
- [ ] 10 crates exist with boundary-correct deps; no cycles; `paavo-core` does not depend on axum; `paavo-proto` depends on no other workspace crate

---

## Milestone 1 — Foundation crates

Goal: pure-data types in `paavo-proto`, the metadata-macro crate `paavo-meta`, and a working SQLite schema with migrations in `paavo-db`. All TDD.

### Task 1.1: paavo-proto — core types

Spec coverage: §5 (JobState, JobOutcome, Priority), §5.5 (BoardSelector), §7 (rows that serialize from these), §9 (HTTP wire format).

**Files:**
- Create: `crates/paavo-proto/src/lib.rs` (replace skeleton)
- Create: `crates/paavo-proto/src/job.rs`
- Create: `crates/paavo-proto/src/board.rs`
- Create: `crates/paavo-proto/src/log.rs`
- Create: `crates/paavo-proto/src/ids.rs`
- Test: `crates/paavo-proto/tests/serde_roundtrip.rs`

- [ ] **Step 1: Write the failing JobId / serde round-trip test**

`crates/paavo-proto/tests/serde_roundtrip.rs`:
```rust
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSource,
    JobSpec, JobState, LogFrame, LogLevel, Priority, TerminalOutcome,
    TimeoutReason,
};

#[test]
fn job_id_roundtrip() {
    let id = JobId::new();
    let s = serde_json::to_string(&id).unwrap();
    let parsed: JobId = serde_json::from_str(&s).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn priority_roundtrip() {
    for p in [Priority::Interactive, Priority::Scheduled] {
        let s = serde_json::to_string(&p).unwrap();
        let parsed: Priority = serde_json::from_str(&s).unwrap();
        assert_eq!(p, parsed);
    }
}

#[test]
fn board_selector_roundtrip() {
    let s = BoardSelector {
        kind: "mcxa266".into(),
        instance: Some("mcxa266-02".into()),
        wiring_profile: Some("alt-spi".into()),
    };
    let json = serde_json::to_string(&s).unwrap();
    let parsed: BoardSelector = serde_json::from_str(&json).unwrap();
    assert_eq!(s, parsed);
}

#[test]
fn job_state_roundtrip() {
    let states = [
        JobState::Submitted,
        JobState::Building,
        JobState::Running,
        JobState::Passed,
        JobState::Failed,
        JobState::TimedOut,
        JobState::Aborted,
    ];
    for st in states {
        let s = serde_json::to_string(&st).unwrap();
        let parsed: JobState = serde_json::from_str(&s).unwrap();
        assert_eq!(st, parsed);
    }
}

#[test]
fn job_outcome_roundtrip_all_variants() {
    let outcomes = [
        JobOutcome::Passed,
        JobOutcome::Failed(TerminalOutcome::TestErr {
            message: "assertion failed".into(),
        }),
        JobOutcome::Failed(TerminalOutcome::BuildErr {
            stderr: "E0432".into(),
        }),
        JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: "probe_attach".into(),
            message: "no probe".into(),
        }),
        JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms: 120_000,
        },
        JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms: 900_000,
        },
        JobOutcome::Aborted {
            by: paavo_proto::AbortReason::User,
        },
        JobOutcome::Aborted {
            by: paavo_proto::AbortReason::DaemonShutdown,
        },
    ];
    for o in outcomes {
        let s = serde_json::to_string(&o).unwrap();
        let parsed: JobOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(o, parsed);
    }
}

#[test]
fn log_frame_roundtrip() {
    let f = LogFrame {
        seq: 42,
        ts_us: 1_234_567,
        level: LogLevel::Info,
        target: Some("app::dma".into()),
        message: "Test OK".into(),
    };
    let s = serde_json::to_string(&f).unwrap();
    let parsed: LogFrame = serde_json::from_str(&s).unwrap();
    assert_eq!(f, parsed);
}

#[test]
fn job_spec_roundtrip() {
    let spec = JobSpec {
        priority: Priority::Interactive,
        submitter: "felipe".into(),
        source: JobSource::Cli,
        board_selector: BoardSelector {
            kind: "mcxa266".into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: Some(120_000),
        hard_max_ms: Some(900_000),
        tar_blake3: "deadbeef".into(),
    };
    let s = serde_json::to_string(&spec).unwrap();
    let parsed: JobSpec = serde_json::from_str(&s).unwrap();
    assert_eq!(spec, parsed);
}

#[test]
fn board_spec_roundtrip() {
    let b = BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: paavo_proto::ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "000123456789".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    };
    let s = serde_json::to_string(&b).unwrap();
    let parsed: BoardSpec = serde_json::from_str(&s).unwrap();
    assert_eq!(b, parsed);
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cargo test -p paavo-proto`
Expected: FAILS to compile — none of the types exist. Error mentions unresolved imports for `JobId`, `Priority`, `BoardSelector`, etc.

- [ ] **Step 3: Implement `paavo-proto::ids`**

`crates/paavo-proto/src/ids.rs`:
```rust
//! Stable identifier types.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A job identifier. ULID under the hood: lexicographically sortable by
/// creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(ulid::Ulid);

impl JobId {
    /// Generate a new job id from the current system time.
    pub fn new() -> Self {
        Self(ulid::Ulid::new())
    }

    /// Return the underlying ULID.
    pub fn as_ulid(&self) -> ulid::Ulid {
        self.0
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for JobId {
    type Err = ulid::DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(ulid::Ulid::from_str(s)?))
    }
}
```

- [ ] **Step 4: Implement `paavo-proto::board`**

`crates/paavo-proto/src/board.rs`:
```rust
//! Board inventory and selector types.

use serde::{Deserialize, Serialize};

/// VID/PID/serial selector for a probe, matching the probe-rs naming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSelector {
    /// USB vendor id, hex string e.g. `"1366"`.
    pub vid: String,
    /// USB product id, hex string e.g. `"1015"`.
    pub pid: String,
    /// Probe serial number as reported by USB.
    pub serial: String,
}

/// Whether a board is currently eligible to receive jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardHealth {
    /// Eligible for job dispatch.
    Healthy,
    /// Excluded from dispatch (manual or auto quarantine).
    Quarantined,
}

/// A registered board in the lab inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSpec {
    /// Lab-unique identifier, e.g. `mcxa266-01`.
    pub id: String,
    /// Board kind, e.g. `mcxa266`. Must match what `paavo_meta::target!()`
    /// emits in scaffolded crates of this kind.
    pub kind: String,
    /// Physical probe used to flash + debug this board.
    pub probe_selector: ProbeSelector,
    /// probe-rs chip name (passed to `Session::new`).
    pub chip_name: String,
    /// `paavo_meta::target!()` value scaffolded test crates write for this
    /// kind. Used to verify ELFs land on the correct fleet.
    pub target_name: String,
    /// Optional named wiring profile (e.g. `alt-spi`). Selectors that ask
    /// for a profile only match boards tagged with that profile.
    pub wiring_profile: Option<String>,
    /// Current health.
    pub health: BoardHealth,
}

/// Job-side selector for matching against the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSelector {
    /// Required board kind.
    pub kind: String,
    /// Optional specific instance (`mcxa266-02`). When set, only that board
    /// is eligible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Optional required wiring profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wiring_profile: Option<String>,
}

impl BoardSelector {
    /// True if `board` satisfies this selector. Health is **not** checked
    /// here — that is the scheduler's job.
    pub fn matches(&self, board: &BoardSpec) -> bool {
        if self.kind != board.kind {
            return false;
        }
        if let Some(inst) = &self.instance {
            if inst != &board.id {
                return false;
            }
        }
        if let Some(profile) = &self.wiring_profile {
            if board.wiring_profile.as_deref() != Some(profile.as_str()) {
                return false;
            }
        }
        true
    }
}
```

- [ ] **Step 5: Implement `paavo-proto::log`**

`crates/paavo-proto/src/log.rs`:
```rust
//! Log frame types as streamed from paavo-runner to paavo-core to paavo-cli.

use serde::{Deserialize, Serialize};

/// defmt log severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// `defmt::trace!`
    Trace,
    /// `defmt::debug!`
    Debug,
    /// `defmt::info!`
    Info,
    /// `defmt::warn!`
    Warn,
    /// `defmt::error!`
    Error,
}

/// One decoded defmt frame emitted by a running test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFrame {
    /// Monotonic sequence number per job, starting at 0.
    pub seq: u64,
    /// Microseconds since job start.
    pub ts_us: u64,
    /// Log severity.
    pub level: LogLevel,
    /// defmt `target` (Rust module path), if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Decoded message body.
    pub message: String,
}
```

- [ ] **Step 6: Implement `paavo-proto::job`**

`crates/paavo-proto/src/job.rs`:
```rust
//! Job state machine, priority, source, and outcome types.

use crate::board::BoardSelector;
use serde::{Deserialize, Serialize};

/// Scheduler priority. Lower variant value = higher priority. Serializes as
/// snake_case strings on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Ad-hoc developer requests (`paavo-cli run`).
    Interactive,
    /// Nightly cron jobs.
    Scheduled,
}

impl Priority {
    /// Numeric weight used by the scheduler's `BinaryHeap`. Smaller = sooner.
    pub fn weight(self) -> u8 {
        match self {
            Priority::Interactive => 0,
            Priority::Scheduled => 1,
        }
    }
}

/// Where a job came from. Distinct from `Priority` because a starvation-
/// promoted Scheduled job retains `JobSource::Scheduler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobSource {
    /// Submitted via `paavo-cli`.
    Cli,
    /// Submitted by the nightly scheduler.
    Scheduler,
}

/// One of the seven persistent states in the job state machine. See
/// `JobOutcome` for the finer-grained terminal-state information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    /// Accepted by the daemon, not yet dispatched.
    #[serde(rename = "submitted")]
    Submitted,
    /// `paavo-build` is compiling.
    #[serde(rename = "building")]
    Building,
    /// `paavo-runner` is attached to a probe.
    #[serde(rename = "running")]
    Running,
    /// Terminal: test reported `Test OK` + bkpt.
    #[serde(rename = "passed")]
    Passed,
    /// Terminal: test failed (build error, test error, or infra error).
    #[serde(rename = "failed")]
    Failed,
    /// Terminal: inactivity or hard-max watchdog tripped. Wire form is
    /// `"timedout"` (one word), matching the SQL CHECK constraint.
    #[serde(rename = "timedout")]
    TimedOut,
    /// Terminal: user cancel or daemon shutdown.
    #[serde(rename = "aborted")]
    Aborted,
}

impl JobState {
    /// True for `Passed`/`Failed`/`TimedOut`/`Aborted`.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Passed | JobState::Failed | JobState::TimedOut | JobState::Aborted
        )
    }
}

/// Specific reason for a `Failed` terminal state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalOutcome {
    /// `cargo build` failed.
    BuildErr {
        /// Captured stderr from cargo (truncated by daemon if huge).
        stderr: String,
    },
    /// Test ran but failed: panic, assert, or defmt-encoded error frame.
    TestErr {
        /// Human-readable summary.
        message: String,
    },
    /// Infrastructure failure: probe attach, mass erase, RTT init, etc.
    /// Contributes to consecutive-infra-failure quarantine count.
    InfraErr {
        /// Pipeline stage that failed (`probe_attach`, `flash`, `rtt_init`,
        /// `defmt_decode`, ...).
        stage: String,
        /// Underlying error.
        message: String,
    },
}

/// Why a job timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutReason {
    /// No defmt frame for `inactivity_timeout` seconds.
    Inactivity,
    /// Total wall clock exceeded `hard_max`.
    HardMax,
}

/// Who initiated an abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortReason {
    /// `paavo-cli cancel`.
    User,
    /// SIGTERM drain ran out of grace.
    DaemonShutdown,
}

/// Fully-tagged terminal outcome stored in the `job.outcome_detail` JSON
/// column and returned on the wire.
///
/// Wire format (externally-tagged; default serde):
/// - `Passed` → `"passed"`
/// - `Failed(TerminalOutcome::TestErr { message })` → `{"failed":{"kind":"test_err","message":"..."}}`
/// - `TimedOut { reason, elapsed_ms }` → `{"timed_out":{"reason":"inactivity","elapsed_ms":120000}}`
/// - `Aborted { by }` → `{"aborted":{"by":"user"}}`
///
/// Externally-tagged is used (instead of `#[serde(tag = "outcome")]`)
/// because internal tagging does not support tuple variants like
/// `Failed(TerminalOutcome)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    /// Reached `Test OK` + bkpt.
    Passed,
    /// Failure with detail.
    Failed(TerminalOutcome),
    /// Watchdog fired.
    TimedOut {
        /// Cause.
        reason: TimeoutReason,
        /// Elapsed time at the moment the watchdog fired.
        elapsed_ms: u64,
    },
    /// Aborted with detail.
    Aborted {
        /// Who.
        by: AbortReason,
    },
}

impl JobOutcome {
    /// True if the outcome should bump the board's consecutive_infra_failures
    /// counter. Per spec §5.2:
    /// - `Failed(InfraErr)` → yes
    /// - other outcomes → no (caller may additionally count
    ///   `TimedOut(Inactivity)` only when the BoardWorker could not release
    ///   the probe; that knowledge does not live in `JobOutcome` itself).
    pub fn counts_toward_infra_failure(&self) -> bool {
        matches!(self, JobOutcome::Failed(TerminalOutcome::InfraErr { .. }))
    }
}

/// The request side of a job, as serialised in the `POST /jobs` multipart
/// JSON part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    /// Scheduler priority.
    pub priority: Priority,
    /// Free-form submitter id (no auth).
    pub submitter: String,
    /// Where the request came from.
    pub source: JobSource,
    /// Board match rules.
    pub board_selector: BoardSelector,
    /// Per-job inactivity override. `None` means use the ELF's
    /// `inactivity_timeout!()`, falling back to the daemon default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactivity_timeout_ms: Option<u64>,
    /// Per-job hard-max override. `None` means use the daemon default for
    /// the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_max_ms: Option<u64>,
    /// blake3 of the uploaded crate tar, used as build-cache key.
    pub tar_blake3: String,
}
```

- [ ] **Step 7: Wire it all together in `paavo-proto::lib`**

`crates/paavo-proto/src/lib.rs`:
```rust
//! Wire types and protocol definitions for paavo.
//!
//! This crate has no workspace dependencies. It is pure data: every other
//! paavo crate is permitted to depend on `paavo-proto`, and `paavo-proto`
//! depends on none of them.
//!
//! ```
//! assert_eq!(paavo_proto::CRATE_NAME, "paavo-proto");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-proto";

mod board;
mod ids;
mod job;
mod log;

pub use board::{BoardHealth, BoardSelector, BoardSpec, ProbeSelector};
pub use ids::JobId;
pub use job::{
    AbortReason, JobOutcome, JobSource, JobSpec, JobState, Priority, TerminalOutcome,
    TimeoutReason,
};
pub use log::{LogFrame, LogLevel};
```

- [ ] **Step 8: Run the test to confirm it passes**

Run: `cargo test -p paavo-proto`
Expected: `serde_roundtrip` integration test reports 8 passed (one per `#[test]` above), doctest reports 1 passed.

- [ ] **Step 9: Run clippy on paavo-proto**

Run: `cargo clippy -p paavo-proto --all-targets -- -D warnings`
Expected: pass.

- [ ] **Step 10: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-proto
git -C D:\workspace\paavo commit -m "feat(proto): core types JobId/JobSpec/BoardSpec/JobOutcome/LogFrame"
```

---

### Task 1.2: paavo-meta — owns target!, timeout!, inactivity_timeout!

Spec coverage: §6.4 (macro definition). `paavo-meta` is self-contained — it owns three macros and the linker fragment that preserves their sections in cross-compiled embedded builds. The section name prefix is `.paavo.*`, owned end-to-end by this workspace; no external tool reads these sections today.

**Files:**
- Modify: `crates/paavo-meta/src/lib.rs`
- Create: `crates/paavo-meta/build.rs`
- Create: `crates/paavo-meta/paavo.x`
- Create: `crates/paavo-meta/tests/macro_expansion.rs`

- [ ] **Step 1: Add the linker fragment**

`crates/paavo-meta/paavo.x` — preserves the `.paavo.*` sections so they survive `cargo build --release` for embedded targets (cortex-m linker drops unreferenced sections by default).

```
/* paavo-meta linker fragment. Preserves the .paavo.* ELF sections
 * emitted by target!(), timeout!(), and inactivity_timeout!() so that
 * paavo-probe can read them out of the linked binary. */
SECTIONS
{
    .paavo (INFO) :
    {
        KEEP(*(.paavo.target))
        KEEP(*(.paavo.timeout))
        KEEP(*(.paavo.inactivity_timeout))
    }
}
INSERT AFTER .text;
```

- [ ] **Step 2: Add the build script**

`crates/paavo-meta/build.rs`:
```rust
//! Copy `paavo.x` into `OUT_DIR` and tell rustc to add `OUT_DIR` to the
//! linker search path. Downstream test crates can then put `-Tpaavo.x`
//! in their RUSTFLAGS (the cargo-generate templates do this in Milestone 6).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let frag = include_str!("paavo.x");
    fs::write(out.join("paavo.x"), frag).expect("writing paavo.x to OUT_DIR");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=paavo.x");
    println!("cargo:rerun-if-changed=build.rs");
}
```

- [ ] **Step 3: Write the failing macro-expansion test**

`crates/paavo-meta/tests/macro_expansion.rs`:
```rust
//! Compile-and-link tests for the macro surface. We can't easily assert
//! that the macros land in `.paavo.*` sections on a host build —
//! that's the embedded linker's job, and the host build steers them into
//! `.rodata.*` instead. But we *can* prove the macros expand and link on
//! the host: that catches typos in the macro bodies.
//!
//! Real ELF-section assertions live in `paavo-probe::tests::sections`
//! (Milestone 2), which builds against synthetic ELF fixtures.

paavo_meta::target!(b"frdm-mcx-a266");
paavo_meta::timeout!(60);
paavo_meta::inactivity_timeout!(30);

#[test]
fn macros_expand_and_link() {
    // The macros emit `pub static` items into this very module; read them
    // directly. `from_le_bytes` documents the on-ELF wire format the test
    // is asserting against (see paavo-probe's section parser).
    assert_eq!(u32::from_le_bytes(_PAAVO_META_TIMEOUT), 60);
    assert_eq!(u32::from_le_bytes(_PAAVO_META_INACTIVITY_TIMEOUT), 30);
}
```

- [ ] **Step 4: Run the test to confirm it fails**

Run: `cargo test -p paavo-meta`
Expected: FAILS to compile — `paavo_meta::target!`, `paavo_meta::timeout!`, `paavo_meta::inactivity_timeout!` don't exist yet.

- [ ] **Step 5: Implement the three macros**

Replace `crates/paavo-meta/src/lib.rs` with:
```rust
//! no_std metadata helpers for paavo test crates.
//!
//! Provides three macros that embed per-test metadata into ELF sections
//! that `paavo-probe` reads at job-dispatch time:
//!
//! - [`target!`] — board kind this test targets (e.g. `b"frdm-mcx-a266"`).
//! - [`timeout!`] — hard-max wall clock for this test, in seconds.
//! - [`inactivity_timeout!`] — per-test override for the inactivity
//!   watchdog, in seconds.
//!
//! The companion `build.rs` ships a linker fragment (`paavo.x`) that
//! preserves the `.paavo.*` sections through the embedded linker.
//! The section name prefix is `.paavo.*` and is owned end-to-end by this
//! workspace; no external tool reads these sections today.
#![no_std]
#![forbid(unsafe_code)]

/// Embed a target identifier as a NUL-terminated byte string in
/// `.paavo.target`. Match against `BoardSpec::target_name` server-side.
///
/// **Call at most once per binary**: the macro emits a `#[no_mangle]`
/// static; a second invocation in the same crate is a hard linker error.
///
/// Pass the literal **without** a trailing NUL; the macro appends one.
///
/// ```ignore
/// paavo_meta::target!(b"frdm-mcx-a266");
/// ```
#[macro_export]
macro_rules! target {
    ($val:literal) => {
        #[cfg_attr(target_os = "none", link_section = ".paavo.target")]
        #[cfg_attr(not(target_os = "none"), link_section = ".rodata.paavo_meta_target")]
        #[used]
        #[no_mangle]
        pub static _PAAVO_META_TARGET: [u8; { $val.len() + 1 }] = {
            let mut buf = [0u8; { $val.len() + 1 }];
            let src: &[u8] = $val;
            let mut i = 0;
            while i < src.len() {
                buf[i] = src[i];
                i += 1;
            }
            buf
        };
    };
}

/// Embed the per-test hard-max wall clock (seconds) in `.paavo.timeout`.
///
/// **Call at most once per binary**: the macro emits a `#[no_mangle]`
/// static; a second invocation in the same crate is a hard linker error.
///
/// On-ELF wire format: 4 little-endian bytes (u32 LE). `paavo-probe` reads
/// the section with `u32::from_le_bytes`. The macro stores the bytes
/// explicitly so the contract holds on any target endianness.
#[macro_export]
macro_rules! timeout {
    ($val:literal) => {
        #[cfg_attr(target_os = "none", link_section = ".paavo.timeout")]
        #[cfg_attr(
            not(target_os = "none"),
            link_section = ".rodata.paavo_meta_timeout"
        )]
        #[used]
        #[no_mangle]
        pub static _PAAVO_META_TIMEOUT: [u8; 4] = ($val as u32).to_le_bytes();
    };
}

/// Embed the per-test inactivity-timeout override (seconds) in
/// `.paavo.inactivity_timeout`. `paavo-probe` reads this section; if
/// absent, falls back to the job's `inactivity_timeout_ms`, which itself
/// falls back to the daemon's configured default.
///
/// **Call at most once per binary**: the macro emits a `#[no_mangle]`
/// static; a second invocation in the same crate is a hard linker error.
///
/// On-ELF wire format: 4 little-endian bytes (u32 LE).
#[macro_export]
macro_rules! inactivity_timeout {
    ($val:literal) => {
        #[cfg_attr(
            target_os = "none",
            link_section = ".paavo.inactivity_timeout"
        )]
        #[cfg_attr(
            not(target_os = "none"),
            link_section = ".rodata.paavo_meta_inactivity_timeout"
        )]
        #[used]
        #[no_mangle]
        pub static _PAAVO_META_INACTIVITY_TIMEOUT: [u8; 4] = ($val as u32).to_le_bytes();
    };
}
```

> The `cfg_attr` split is because `link_section = ".paavo.*"` is rejected by some host linkers (ld/lld on Linux/macOS will accept it; some Windows linkers won't). The host path steers them into `.rodata.*` so the workspace test compiles everywhere. The macro's real consumer is cross-compiled embedded builds, where the first arm fires and `paavo.x` preserves the section through the linker.

- [ ] **Step 6: Run the test to confirm it passes**

Run: `cargo test -p paavo-meta`
Expected: 1 passed (`macros_expand_and_link`).

- [ ] **Step 7: Confirm the workspace still builds**

Run: `cargo build --workspace`
Expected: green. `paavo-meta`'s new `build.rs` runs once, writes `paavo.x` into the crate's `OUT_DIR`.

- [ ] **Step 8: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-meta Cargo.toml Cargo.lock
git -C D:\workspace\paavo commit -m "feat(meta): own target!/timeout!/inactivity_timeout! macros + paavo.x linker fragment"
```

---

### Task 1.3: paavo-db — schema, migrations, typed queries

Spec coverage: §7.1–§7.6 (5 tables, retention, JSON columns), §5.1 (state strings), §11 (RO open).

**Files:**
- Create: `crates/paavo-db/src/lib.rs` (replace skeleton)
- Create: `crates/paavo-db/src/error.rs`
- Create: `crates/paavo-db/src/db.rs`
- Create: `crates/paavo-db/src/board.rs`
- Create: `crates/paavo-db/src/job.rs`
- Create: `crates/paavo-db/src/log.rs`
- Create: `crates/paavo-db/src/build_cache.rs`
- Create: `crates/paavo-db/src/schedule.rs`
- Create: `crates/paavo-db/migrations/V1__initial.sql`
- Create: `crates/paavo-db/build.rs`
- Test: `crates/paavo-db/tests/migrations.rs`
- Test: `crates/paavo-db/tests/board_ops.rs`
- Test: `crates/paavo-db/tests/job_ops.rs`
- Test: `crates/paavo-db/tests/log_ops.rs`
- Test: `crates/paavo-db/tests/build_cache_ops.rs`

#### 1.3.a: Migrations + Db::open

- [ ] **Step 1: Write the failing migrations test**

`crates/paavo-db/tests/migrations.rs`:
```rust
use paavo_db::Db;
use tempfile::tempdir;

#[test]
fn open_runs_migrations_and_creates_tables() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();

    let conn = db.raw_conn();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    // refinery_schema_history is the migrator's bookkeeping table — fine to
    // see, just filter it out.
    let user: Vec<&str> = tables
        .iter()
        .map(|s| s.as_str())
        .filter(|n| *n != "refinery_schema_history")
        .collect();

    assert_eq!(
        user,
        vec!["board", "build_cache", "job", "log_frame", "schedule"]
    );
}

#[test]
fn open_is_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    {
        let _db = Db::open(&path).unwrap();
    }
    // Second open against same file must succeed (re-running migrations is a
    // no-op when they're already applied).
    let _db = Db::open(&path).unwrap();
}

#[test]
fn open_readonly_works_against_existing_db() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    {
        let _rw = Db::open(&path).unwrap();
    }
    let ro = Db::open_readonly(&path).unwrap();
    let count: i64 = ro
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM board", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-db --test migrations`
Expected: FAILS — `Db` not found.

- [ ] **Step 3: Create the migration SQL**

`crates/paavo-db/migrations/V1__initial.sql`:
```sql
-- See spec §7 for column rationale.

PRAGMA foreign_keys = ON;

CREATE TABLE board (
    id                          TEXT PRIMARY KEY,
    kind                        TEXT NOT NULL,
    probe_selector              TEXT NOT NULL,           -- JSON
    chip_name                   TEXT NOT NULL,
    target_name                 TEXT NOT NULL,
    wiring_profile              TEXT,
    health                      TEXT NOT NULL CHECK (health IN ('healthy','quarantined')),
    quarantine_reason           TEXT,
    consecutive_infra_failures  INTEGER NOT NULL DEFAULT 0,
    last_used_at                INTEGER,                 -- epoch ms, nullable
    created_at                  INTEGER NOT NULL
);

CREATE INDEX idx_board_kind_health ON board(kind, health);

CREATE TABLE job (
    id                       TEXT PRIMARY KEY,           -- ULID
    priority                 INTEGER NOT NULL,           -- smaller = higher
    submitter                TEXT NOT NULL,
    source                   TEXT NOT NULL CHECK (source IN ('cli','scheduler')),
    board_selector           TEXT NOT NULL,              -- JSON
    inactivity_timeout_ms    INTEGER NOT NULL,
    hard_max_ms              INTEGER NOT NULL,
    state                    TEXT NOT NULL CHECK (state IN
        ('submitted','building','running','passed','failed','timedout','aborted')),
    outcome_detail           TEXT,                       -- JSON, nullable
    board_id                 TEXT REFERENCES board(id),
    submitted_at             INTEGER NOT NULL,
    started_at               INTEGER,
    finished_at              INTEGER,
    tar_blake3               TEXT NOT NULL,
    tar_path                 TEXT NOT NULL,
    elf_path                 TEXT
);

CREATE INDEX idx_job_state           ON job(state);
CREATE INDEX idx_job_submitted_at    ON job(submitted_at);
CREATE INDEX idx_job_priority_subat  ON job(priority, submitted_at) WHERE state = 'submitted';

CREATE TABLE log_frame (
    job_id   TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    seq      INTEGER NOT NULL,
    ts_us    INTEGER NOT NULL,
    level    TEXT NOT NULL CHECK (level IN ('trace','debug','info','warn','error')),
    target   TEXT,
    message  TEXT NOT NULL,
    PRIMARY KEY (job_id, seq)
);

CREATE INDEX idx_log_frame_job_level ON log_frame(job_id, level);

CREATE TABLE build_cache (
    tar_blake3    TEXT PRIMARY KEY,
    elf_path      TEXT NOT NULL,
    built_at      INTEGER NOT NULL,
    last_used_at  INTEGER NOT NULL,
    size_bytes    INTEGER NOT NULL
);

CREATE INDEX idx_build_cache_lru ON build_cache(last_used_at);

CREATE TABLE schedule (
    id                  TEXT PRIMARY KEY,
    cron                TEXT NOT NULL,
    enabled             INTEGER NOT NULL CHECK (enabled IN (0,1)),
    last_triggered_at   INTEGER,
    last_completed_at   INTEGER
);
```

- [ ] **Step 4: Wire refinery via build.rs**

`crates/paavo-db/build.rs`:
```rust
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
```

- [ ] **Step 5: Implement `paavo-db::error`**

`crates/paavo-db/src/error.rs`:
```rust
//! Error type for paavo-db.

use thiserror::Error;

/// Errors returned by paavo-db operations.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Migration application failed.
    #[error("migration: {0}")]
    Migration(#[from] refinery::Error),
    /// JSON column failed to (de)serialize.
    #[error("json column: {0}")]
    Json(#[from] serde_json::Error),
    /// Row found but a CHECK-constrained string value was unrecognized.
    #[error("unknown enum variant for column {column}: {value}")]
    UnknownEnum {
        /// SQL column name.
        column: &'static str,
        /// Value pulled from the row.
        value: String,
    },
}

/// `Result` alias used throughout paavo-db.
pub type Result<T, E = DbError> = std::result::Result<T, E>;
```

- [ ] **Step 6: Implement `paavo-db::db`**

`crates/paavo-db/src/db.rs`:
```rust
//! `Db` — the owned SQLite handle. Single writer (paavod), single reader
//! (paavo-web). WAL mode + busy timeout.

use crate::error::{DbError, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

mod embedded {
    refinery::embed_migrations!("./migrations");
}

/// Owned SQLite handle, plus migration bookkeeping.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (or create) a read-write SQLite database at `path` and run any
    /// pending migrations. WAL journal mode, 5 s busy timeout, foreign keys on.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut conn = Connection::open(path.as_ref())?;
        configure(&mut conn, /* readonly = */ false)?;
        embedded::migrations::runner().run(&mut conn).map_err(DbError::from)?;
        Ok(Self { conn })
    }

    /// Open `path` read-only. Caller (paavo-web) must wait for paavod to have
    /// created the file first.
    pub fn open_readonly<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut conn = Connection::open_with_flags(
            path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        configure(&mut conn, /* readonly = */ true)?;
        Ok(Self { conn })
    }

    /// Raw connection accessor — bypasses the typed query helpers in
    /// `board.rs`, `job.rs`, etc. Use only for tests or for queries the
    /// typed surface does not yet cover.
    pub fn raw_conn(&self) -> &Connection {
        &self.conn
    }

    /// Mutable raw connection accessor (for `transaction()`). Same caveat
    /// as `raw_conn` — prefer typed helpers when one exists.
    pub fn raw_conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

fn configure(conn: &mut Connection, readonly: bool) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    if !readonly {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
    }
    Ok(())
}
```

- [ ] **Step 7: Add stub modules + re-exports in `paavo-db::lib`**

Replace `crates/paavo-db/src/lib.rs` with:
```rust
//! SQLite-backed persistence for paavo. Owns the schema; exposes typed
//! query helpers per table. Single writer (paavod), single reader
//! (paavo-web).
//!
//! ```
//! assert_eq!(paavo_db::CRATE_NAME, "paavo-db");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-db";

mod board;
mod build_cache;
mod db;
mod error;
mod job;
mod log;
mod schedule;

pub use board::BoardRow;
pub use build_cache::{BuildCacheEntry, BuildCacheStats};
pub use db::Db;
pub use error::{DbError, Result};
pub use job::{JobRow, NewJob, OutcomeRecord};
pub use log::LogFrameRow;
pub use schedule::{ScheduleRow, ScheduleUpdate};
```

- [ ] **Step 8: Add temporary empty stub modules so it compiles**

`crates/paavo-db/src/board.rs`:
```rust
//! Board table helpers (filled in by Task 1.3.b).

/// Row representation of the `board` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRow;
```

`crates/paavo-db/src/job.rs`:
```rust
//! Job table helpers (filled in by Task 1.3.c).

/// Row representation of the `job` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow;

/// Insert-time job representation (no state, no outcome).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob;

/// Captured terminal outcome to record when transitioning to a terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeRecord;
```

`crates/paavo-db/src/log.rs`:
```rust
//! log_frame table helpers (filled in by Task 1.3.d).

/// Row representation of the `log_frame` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFrameRow;
```

`crates/paavo-db/src/build_cache.rs`:
```rust
//! build_cache table helpers (filled in by Task 1.3.e).

/// Row representation of the `build_cache` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheEntry;

/// Aggregate stats for the build-cache LRU policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildCacheStats;
```

`crates/paavo-db/src/schedule.rs`:
```rust
//! schedule table helpers (filled in by paavod cron wiring).

/// Row representation of the `schedule` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRow;

/// Partial-update payload used when the cron driver fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleUpdate;
```

- [ ] **Step 9: Run the migrations test to confirm it passes**

Run: `cargo test -p paavo-db --test migrations`
Expected: 3 passed.

- [ ] **Step 10: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-db
git -C D:\workspace\paavo commit -m "feat(db): schema + migrations + Db::open / Db::open_readonly"
```

---

#### 1.3.b: Board table typed helpers

- [ ] **Step 1: Write the failing board ops test**

`crates/paavo-db/tests/board_ops.rs`:
```rust
use chrono::Utc;
use paavo_db::{BoardRow, Db};
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir); // tempdir lives for the test process
    db
}

fn sample_board() -> BoardSpec {
    BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "ABC".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    }
}

#[test]
fn insert_then_get_round_trips() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();

    let got = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(got.spec, sample_board());
    assert_eq!(got.consecutive_infra_failures, 0);
    assert_eq!(got.created_at, now);
}

#[test]
fn list_all_returns_inserted_boards_sorted_by_id() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    let mut a = sample_board();
    a.id = "mcxa266-02".into();
    let mut b = sample_board();
    b.id = "mcxa266-01".into();
    BoardRow::insert(db.raw_conn(), &a, now).unwrap();
    BoardRow::insert(db.raw_conn(), &b, now).unwrap();

    let rows = BoardRow::list_all(db.raw_conn()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].spec.id, "mcxa266-01");
    assert_eq!(rows[1].spec.id, "mcxa266-02");
}

#[test]
fn find_healthy_for_selector_filters_by_kind_and_excludes_quarantined() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    let mut healthy_mcx = sample_board();
    healthy_mcx.id = "mcxa266-01".into();
    BoardRow::insert(db.raw_conn(), &healthy_mcx, now).unwrap();

    let mut quarantined_mcx = sample_board();
    quarantined_mcx.id = "mcxa266-02".into();
    quarantined_mcx.health = BoardHealth::Quarantined;
    BoardRow::insert(db.raw_conn(), &quarantined_mcx, now).unwrap();

    let mut healthy_rt = sample_board();
    healthy_rt.id = "rt685-01".into();
    healthy_rt.kind = "rt685-evk".into();
    BoardRow::insert(db.raw_conn(), &healthy_rt, now).unwrap();

    let sel = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: None,
    };
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].spec.id, "mcxa266-01");
}

#[test]
fn touch_last_used_updates_only_that_column() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    BoardRow::touch_last_used(db.raw_conn(), "mcxa266-01", now + 1000).unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.last_used_at, Some(now + 1000));
    assert_eq!(row.created_at, now);
}

#[test]
fn quarantine_and_unquarantine_flip_health_and_reset_counter() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    BoardRow::bump_infra_failure(db.raw_conn(), "mcxa266-01").unwrap();
    BoardRow::bump_infra_failure(db.raw_conn(), "mcxa266-01").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 2);

    BoardRow::quarantine(db.raw_conn(), "mcxa266-01", "broken header").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Quarantined);
    assert_eq!(row.quarantine_reason.as_deref(), Some("broken header"));

    BoardRow::unquarantine(db.raw_conn(), "mcxa266-01").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
    assert_eq!(row.consecutive_infra_failures, 0);
    assert!(row.quarantine_reason.is_none());
}

#[test]
fn reset_infra_failures_clears_counter_on_pass() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    BoardRow::bump_infra_failure(db.raw_conn(), "mcxa266-01").unwrap();
    BoardRow::reset_infra_failures(db.raw_conn(), "mcxa266-01").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 0);
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-db --test board_ops`
Expected: FAILS — `BoardRow::insert`, `get`, etc. don't exist.

- [ ] **Step 3: Implement board helpers**

Replace `crates/paavo-db/src/board.rs`:
```rust
//! Board table typed helpers.

use crate::error::{DbError, Result};
use paavo_proto::{BoardHealth, BoardSelector, BoardSpec, ProbeSelector};
use rusqlite::{params, Connection, OptionalExtension, Row};

/// One row from the `board` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRow {
    /// The publicly-shaped board spec.
    pub spec: BoardSpec,
    /// Free-form reason, set when `spec.health == Quarantined`.
    pub quarantine_reason: Option<String>,
    /// Counts toward auto-quarantine threshold (config:
    /// `quarantine.consecutive_infra_failures`).
    pub consecutive_infra_failures: u32,
    /// Last successful dispatch in epoch ms.
    pub last_used_at: Option<i64>,
    /// First-seen epoch ms.
    pub created_at: i64,
}

impl BoardRow {
    /// Insert a new board. Initial counters/values: 0 infra failures, no
    /// last_used_at, no quarantine reason.
    pub fn insert(conn: &Connection, spec: &BoardSpec, now_ms: i64) -> Result<()> {
        let probe_json = serde_json::to_string(&spec.probe_selector)?;
        conn.execute(
            "INSERT INTO board (
                id, kind, probe_selector, chip_name, target_name,
                wiring_profile, health, quarantine_reason,
                consecutive_infra_failures, last_used_at, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, NULL, ?8)",
            params![
                spec.id,
                spec.kind,
                probe_json,
                spec.chip_name,
                spec.target_name,
                spec.wiring_profile,
                health_to_str(spec.health),
                now_ms,
            ],
        )?;
        Ok(())
    }

    /// Fetch a single board by id. Errors if missing.
    pub fn get(conn: &Connection, id: &str) -> Result<Self> {
        conn.query_row("SELECT * FROM board WHERE id = ?1", params![id], from_row)?
    }

    /// List all boards, ordered by id ascending.
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare("SELECT * FROM board ORDER BY id ASC")?;
        let rows = stmt
            .query_map([], from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find healthy boards matching the selector. Result is unordered; the
    /// scheduler decides LRU.
    pub fn find_healthy_for_selector(
        conn: &Connection,
        sel: &BoardSelector,
    ) -> Result<Vec<Self>> {
        let mut sql =
            String::from("SELECT * FROM board WHERE kind = ?1 AND health = 'healthy'");
        let mut next_param = 2;
        if sel.instance.is_some() {
            sql.push_str(&format!(" AND id = ?{next_param}"));
            next_param += 1;
        }
        if sel.wiring_profile.is_some() {
            sql.push_str(&format!(" AND wiring_profile = ?{next_param}"));
        }

        let mut stmt = conn.prepare(&sql)?;
        let mut bound: Vec<&dyn rusqlite::ToSql> = vec![&sel.kind];
        if let Some(inst) = &sel.instance {
            bound.push(inst);
        }
        if let Some(wp) = &sel.wiring_profile {
            bound.push(wp);
        }
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bound), from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find a board by id, returning `Ok(None)` if missing.
    pub fn find(conn: &Connection, id: &str) -> Result<Option<Self>> {
        let row = conn
            .query_row("SELECT * FROM board WHERE id = ?1", params![id], from_row)
            .optional()?;
        match row {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Update `last_used_at` to `now_ms`.
    pub fn touch_last_used(conn: &Connection, id: &str, now_ms: i64) -> Result<()> {
        conn.execute(
            "UPDATE board SET last_used_at = ?1 WHERE id = ?2",
            params![now_ms, id],
        )?;
        Ok(())
    }

    /// Increment `consecutive_infra_failures` by 1.
    pub fn bump_infra_failure(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET consecutive_infra_failures =
             consecutive_infra_failures + 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Reset `consecutive_infra_failures` to 0. Called after a job whose
    /// outcome does not count toward infra failure (per
    /// `JobOutcome::counts_toward_infra_failure`).
    pub fn reset_infra_failures(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET consecutive_infra_failures = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Flip board to `quarantined` and record a reason.
    pub fn quarantine(conn: &Connection, id: &str, reason: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET health = 'quarantined', quarantine_reason = ?1
             WHERE id = ?2",
            params![reason, id],
        )?;
        Ok(())
    }

    /// Flip board back to `healthy`, clear quarantine reason and reset the
    /// infra failure counter.
    pub fn unquarantine(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET health = 'healthy', quarantine_reason = NULL,
             consecutive_infra_failures = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

fn health_to_str(h: BoardHealth) -> &'static str {
    match h {
        BoardHealth::Healthy => "healthy",
        BoardHealth::Quarantined => "quarantined",
    }
}

fn health_from_str(s: &str) -> Result<BoardHealth> {
    match s {
        "healthy" => Ok(BoardHealth::Healthy),
        "quarantined" => Ok(BoardHealth::Quarantined),
        other => Err(DbError::UnknownEnum {
            column: "board.health",
            value: other.to_string(),
        }),
    }
}

/// Map a row to a Result, with JSON/enum decoding errors surfacing as
/// `DbError`.
fn from_row(r: &Row<'_>) -> rusqlite::Result<Result<BoardRow>> {
    let probe_json: String = r.get("probe_selector")?;
    let health_str: String = r.get("health")?;
    let id: String = r.get("id")?;
    let kind: String = r.get("kind")?;
    let chip_name: String = r.get("chip_name")?;
    let target_name: String = r.get("target_name")?;
    let wiring_profile: Option<String> = r.get("wiring_profile")?;
    let quarantine_reason: Option<String> = r.get("quarantine_reason")?;
    let raw_counter: i64 = r.get("consecutive_infra_failures")?;
    let last_used_at: Option<i64> = r.get("last_used_at")?;
    let created_at: i64 = r.get("created_at")?;

    Ok((|| -> Result<BoardRow> {
        let probe_selector: ProbeSelector = serde_json::from_str(&probe_json)?;
        let health = health_from_str(&health_str)?;
        let consecutive_infra_failures: u32 =
            raw_counter
                .try_into()
                .map_err(|_| DbError::UnknownEnum {
                    column: "board.consecutive_infra_failures",
                    value: "negative or > u32::MAX".into(),
                })?;
        Ok(BoardRow {
            spec: BoardSpec {
                id,
                kind,
                probe_selector,
                chip_name,
                target_name,
                wiring_profile,
                health,
            },
            quarantine_reason,
            consecutive_infra_failures,
            last_used_at,
            created_at,
        })
    })())
}
```

- [ ] **Step 4: Run the board ops test**

Run: `cargo test -p paavo-db --test board_ops`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-db/src/board.rs crates/paavo-db/tests/board_ops.rs
git -C D:\workspace\paavo commit -m "feat(db): board table typed helpers (insert/get/list/quarantine/etc.)"
```

---

#### 1.3.c: Job table typed helpers

- [ ] **Step 1: Write the failing job ops test**

`crates/paavo-db/tests/job_ops.rs`:
```rust
use chrono::Utc;
use paavo_db::{Db, JobRow, NewJob, OutcomeRecord};
use paavo_proto::{
    BoardSelector, JobId, JobOutcome, JobSource, JobState, Priority, TerminalOutcome,
};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

fn sample_new_job(id: JobId) -> NewJob {
    NewJob {
        id,
        priority: Priority::Interactive,
        submitter: "felipe".into(),
        source: JobSource::Cli,
        board_selector: BoardSelector {
            kind: "mcxa266".into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: 120_000,
        hard_max_ms: 900_000,
        tar_blake3: "deadbeef".into(),
        tar_path: "/var/lib/paavo/uploads/deadbeef.tar".into(),
    }
}

#[test]
fn insert_then_get_round_trips() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.id, id);
    assert_eq!(row.state, JobState::Submitted);
    assert_eq!(row.priority, Priority::Interactive);
    assert_eq!(row.submitted_at, now);
    assert!(row.outcome.is_none());
    assert!(row.board_id.is_none());
    assert!(row.elf_path.is_none());
}

#[test]
fn next_submitted_returns_highest_priority_oldest_first() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    let scheduled = JobId::new();
    let mut sched = sample_new_job(scheduled);
    sched.priority = Priority::Scheduled;
    sched.source = JobSource::Scheduler;
    JobRow::insert(db.raw_conn(), &sched, now).unwrap();

    // Interactive comes 1 ms later than scheduled but should sort first by
    // priority.
    let interactive = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(interactive), now + 1).unwrap();

    let picks = JobRow::list_submitted(db.raw_conn(), 10).unwrap();
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0].id, interactive);
    assert_eq!(picks[1].id, scheduled);
}

#[test]
fn transition_to_building_sets_state_and_board_id() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", now + 10).unwrap();

    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Building);
    assert_eq!(row.board_id.as_deref(), Some("mcxa266-01"));
    assert_eq!(row.started_at, Some(now + 10));
}

#[test]
fn transition_to_running_records_elf_path() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", now + 10).unwrap();
    JobRow::transition_to_running(db.raw_conn(), &id, "/cache/abc/foo.elf").unwrap();

    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Running);
    assert_eq!(row.elf_path.as_deref(), Some("/cache/abc/foo.elf"));
}

#[test]
fn finalize_to_passed_stores_outcome_json() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", now + 10).unwrap();
    JobRow::transition_to_running(db.raw_conn(), &id, "/cache/foo.elf").unwrap();

    let rec = OutcomeRecord {
        state: JobState::Passed,
        outcome: JobOutcome::Passed,
        finished_at_ms: now + 5_000,
    };
    JobRow::finalize(db.raw_conn(), &id, &rec).unwrap();
    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Passed);
    assert_eq!(row.outcome, Some(JobOutcome::Passed));
    assert_eq!(row.finished_at, Some(now + 5_000));
}

#[test]
fn finalize_to_failed_with_test_err_round_trips_outcome_detail() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();

    let outcome = JobOutcome::Failed(TerminalOutcome::TestErr {
        message: "panicked at 'assertion failed'".into(),
    });
    let rec = OutcomeRecord {
        state: JobState::Failed,
        outcome: outcome.clone(),
        finished_at_ms: now + 2_000,
    };
    JobRow::finalize(db.raw_conn(), &id, &rec).unwrap();

    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.outcome, Some(outcome));
}

#[test]
fn list_by_state_filters_correctly() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    let a = JobId::new();
    let b = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(a), now).unwrap();
    JobRow::insert(db.raw_conn(), &sample_new_job(b), now + 1).unwrap();

    JobRow::transition_to_building(db.raw_conn(), &a, "mcxa266-01", now + 5).unwrap();

    let submitted = JobRow::list_by_state(db.raw_conn(), JobState::Submitted, 50).unwrap();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].id, b);

    let building = JobRow::list_by_state(db.raw_conn(), JobState::Building, 50).unwrap();
    assert_eq!(building.len(), 1);
    assert_eq!(building[0].id, a);
}

#[test]
fn delete_cascades_to_log_frames() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    db.raw_conn()
        .execute(
            "INSERT INTO log_frame (job_id, seq, ts_us, level, target, message)
             VALUES (?1, 0, 0, 'info', NULL, 'hi')",
            rusqlite::params![id.to_string()],
        )
        .unwrap();

    JobRow::delete(db.raw_conn(), &id).unwrap();
    let count: i64 = db
        .raw_conn()
        .query_row(
            "SELECT COUNT(*) FROM log_frame WHERE job_id = ?1",
            rusqlite::params![id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-db --test job_ops`
Expected: FAILS — `JobRow::insert` and friends don't exist.

- [ ] **Step 3: Implement job helpers**

Replace `crates/paavo-db/src/job.rs`:
```rust
//! Job table typed helpers.

use crate::error::{DbError, Result};
use paavo_proto::{
    BoardSelector, JobId, JobOutcome, JobSource, JobState, Priority,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::str::FromStr;

/// Insert payload — everything the caller has at enqueue time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    /// ULID, pre-generated by the caller.
    pub id: JobId,
    /// Scheduler priority.
    pub priority: Priority,
    /// Submitter free text.
    pub submitter: String,
    /// Origin (cli vs scheduler).
    pub source: JobSource,
    /// Selector the scheduler will match against the board fleet.
    pub board_selector: BoardSelector,
    /// Effective inactivity timeout (already resolved against the daemon
    /// default; ELF override is applied later when paavo-probe parses the
    /// ELF section).
    pub inactivity_timeout_ms: u64,
    /// Effective hard-max wall clock for this job.
    pub hard_max_ms: u64,
    /// blake3 of the uploaded tar.
    pub tar_blake3: String,
    /// On-disk location of the persisted tar.
    pub tar_path: String,
}

/// Captured terminal-state transition payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeRecord {
    /// Target terminal state (one of `Passed`/`Failed`/`TimedOut`/`Aborted`).
    pub state: JobState,
    /// Fully-tagged outcome.
    pub outcome: JobOutcome,
    /// Wall-clock finish time in epoch ms.
    pub finished_at_ms: i64,
}

/// One row from the `job` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    /// Job id.
    pub id: JobId,
    /// Priority.
    pub priority: Priority,
    /// Submitter text.
    pub submitter: String,
    /// Origin.
    pub source: JobSource,
    /// Selector.
    pub board_selector: BoardSelector,
    /// Effective inactivity timeout (ms).
    pub inactivity_timeout_ms: u64,
    /// Hard-max wall clock (ms).
    pub hard_max_ms: u64,
    /// Current state.
    pub state: JobState,
    /// Decoded `JobOutcome`, when in a terminal state.
    pub outcome: Option<JobOutcome>,
    /// Board id this job was dispatched to.
    pub board_id: Option<String>,
    /// Enqueue time, epoch ms.
    pub submitted_at: i64,
    /// Time the scheduler picked this job, epoch ms.
    pub started_at: Option<i64>,
    /// Time the worker reached terminal state, epoch ms.
    pub finished_at: Option<i64>,
    /// blake3 of the uploaded tar.
    pub tar_blake3: String,
    /// On-disk tar path.
    pub tar_path: String,
    /// ELF path once `paavo-build` finishes, otherwise `None`.
    pub elf_path: Option<String>,
}

impl JobRow {
    /// Insert a new job in `Submitted` state.
    pub fn insert(conn: &Connection, j: &NewJob, now_ms: i64) -> Result<()> {
        let sel_json = serde_json::to_string(&j.board_selector)?;
        conn.execute(
            "INSERT INTO job (
                id, priority, submitter, source, board_selector,
                inactivity_timeout_ms, hard_max_ms, state, outcome_detail,
                board_id, submitted_at, started_at, finished_at,
                tar_blake3, tar_path, elf_path
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'submitted', NULL, NULL,
                      ?8, NULL, NULL, ?9, ?10, NULL)",
            params![
                j.id.to_string(),
                j.priority.weight() as i64,
                j.submitter,
                source_to_str(j.source),
                sel_json,
                j.inactivity_timeout_ms as i64,
                j.hard_max_ms as i64,
                now_ms,
                j.tar_blake3,
                j.tar_path,
            ],
        )?;
        Ok(())
    }

    /// Fetch a job by id. Errors if missing.
    pub fn get(conn: &Connection, id: &JobId) -> Result<Self> {
        conn.query_row(
            "SELECT * FROM job WHERE id = ?1",
            params![id.to_string()],
            from_row,
        )?
    }

    /// Fetch a job by id, returning `Ok(None)` if missing.
    pub fn find(conn: &Connection, id: &JobId) -> Result<Option<Self>> {
        let row = conn
            .query_row(
                "SELECT * FROM job WHERE id = ?1",
                params![id.to_string()],
                from_row,
            )
            .optional()?;
        match row {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// List up to `limit` jobs in `Submitted` state, ordered by
    /// (priority asc, submitted_at asc).
    pub fn list_submitted(conn: &Connection, limit: u32) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM job WHERE state = 'submitted'
             ORDER BY priority ASC, submitted_at ASC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// List up to `limit` jobs in the given state, ordered by `submitted_at`
    /// descending (newest first).
    pub fn list_by_state(
        conn: &Connection,
        state: JobState,
        limit: u32,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM job WHERE state = ?1
             ORDER BY submitted_at DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(
                params![state_to_str(state), limit as i64],
                from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// `Submitted → Building`, recording the chosen board and start time.
    pub fn transition_to_building(
        conn: &Connection,
        id: &JobId,
        board_id: &str,
        now_ms: i64,
    ) -> Result<()> {
        let n = conn.execute(
            "UPDATE job SET state = 'building', board_id = ?1, started_at = ?2
             WHERE id = ?3 AND state = 'submitted'",
            params![board_id, now_ms, id.to_string()],
        )?;
        if n == 0 {
            return Err(DbError::UnknownEnum {
                column: "job.state",
                value: "expected 'submitted' for transition_to_building".into(),
            });
        }
        Ok(())
    }

    /// `Building → Running`, recording the built ELF path.
    pub fn transition_to_running(
        conn: &Connection,
        id: &JobId,
        elf_path: &str,
    ) -> Result<()> {
        let n = conn.execute(
            "UPDATE job SET state = 'running', elf_path = ?1
             WHERE id = ?2 AND state = 'building'",
            params![elf_path, id.to_string()],
        )?;
        if n == 0 {
            return Err(DbError::UnknownEnum {
                column: "job.state",
                value: "expected 'building' for transition_to_running".into(),
            });
        }
        Ok(())
    }

    /// Apply a terminal-state transition with outcome.
    pub fn finalize(
        conn: &Connection,
        id: &JobId,
        rec: &OutcomeRecord,
    ) -> Result<()> {
        assert!(rec.state.is_terminal(), "finalize requires a terminal state");
        let detail_json = serde_json::to_string(&rec.outcome)?;
        conn.execute(
            "UPDATE job SET state = ?1, outcome_detail = ?2, finished_at = ?3
             WHERE id = ?4",
            params![
                state_to_str(rec.state),
                detail_json,
                rec.finished_at_ms,
                id.to_string()
            ],
        )?;
        Ok(())
    }

    /// Delete a job; `ON DELETE CASCADE` clears its log_frames.
    pub fn delete(conn: &Connection, id: &JobId) -> Result<()> {
        conn.execute("DELETE FROM job WHERE id = ?1", params![id.to_string()])?;
        Ok(())
    }
}

fn priority_from_i64(n: i64) -> Result<Priority> {
    match n {
        0 => Ok(Priority::Interactive),
        1 => Ok(Priority::Scheduled),
        other => Err(DbError::UnknownEnum {
            column: "job.priority",
            value: other.to_string(),
        }),
    }
}

fn source_to_str(s: JobSource) -> &'static str {
    match s {
        JobSource::Cli => "cli",
        JobSource::Scheduler => "scheduler",
    }
}

fn source_from_str(s: &str) -> Result<JobSource> {
    match s {
        "cli" => Ok(JobSource::Cli),
        "scheduler" => Ok(JobSource::Scheduler),
        other => Err(DbError::UnknownEnum {
            column: "job.source",
            value: other.to_string(),
        }),
    }
}

fn state_to_str(s: JobState) -> &'static str {
    match s {
        JobState::Submitted => "submitted",
        JobState::Building => "building",
        JobState::Running => "running",
        JobState::Passed => "passed",
        JobState::Failed => "failed",
        JobState::TimedOut => "timedout",
        JobState::Aborted => "aborted",
    }
}

fn state_from_str(s: &str) -> Result<JobState> {
    Ok(match s {
        "submitted" => JobState::Submitted,
        "building" => JobState::Building,
        "running" => JobState::Running,
        "passed" => JobState::Passed,
        "failed" => JobState::Failed,
        "timedout" => JobState::TimedOut,
        "aborted" => JobState::Aborted,
        other => {
            return Err(DbError::UnknownEnum {
                column: "job.state",
                value: other.to_string(),
            })
        }
    })
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<Result<JobRow>> {
    let id_str: String = r.get("id")?;
    let priority_i64: i64 = r.get("priority")?;
    let submitter: String = r.get("submitter")?;
    let source_str: String = r.get("source")?;
    let sel_json: String = r.get("board_selector")?;
    let inactivity: i64 = r.get("inactivity_timeout_ms")?;
    let hardmax: i64 = r.get("hard_max_ms")?;
    let state_str: String = r.get("state")?;
    let outcome_json: Option<String> = r.get("outcome_detail")?;
    let board_id: Option<String> = r.get("board_id")?;
    let submitted_at: i64 = r.get("submitted_at")?;
    let started_at: Option<i64> = r.get("started_at")?;
    let finished_at: Option<i64> = r.get("finished_at")?;
    let tar_blake3: String = r.get("tar_blake3")?;
    let tar_path: String = r.get("tar_path")?;
    let elf_path: Option<String> = r.get("elf_path")?;

    Ok((|| -> Result<JobRow> {
        let id = JobId::from_str(&id_str).map_err(|_| DbError::UnknownEnum {
            column: "job.id",
            value: id_str.clone(),
        })?;
        let priority = priority_from_i64(priority_i64)?;
        let source = source_from_str(&source_str)?;
        let board_selector: BoardSelector = serde_json::from_str(&sel_json)?;
        let state = state_from_str(&state_str)?;
        let outcome = match outcome_json {
            Some(j) => Some(serde_json::from_str::<JobOutcome>(&j)?),
            None => None,
        };
        Ok(JobRow {
            id,
            priority,
            submitter,
            source,
            board_selector,
            inactivity_timeout_ms: inactivity as u64,
            hard_max_ms: hardmax as u64,
            state,
            outcome,
            board_id,
            submitted_at,
            started_at,
            finished_at,
            tar_blake3,
            tar_path,
            elf_path,
        })
    })())
}
```

- [ ] **Step 4: Run the job ops test**

Run: `cargo test -p paavo-db --test job_ops`
Expected: 8 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-db/src/job.rs crates/paavo-db/tests/job_ops.rs
git -C D:\workspace\paavo commit -m "feat(db): job table typed helpers (insert/transitions/finalize/list)"
```

---

#### 1.3.d: Log frame table typed helpers + retention

- [ ] **Step 1: Write the failing log ops test**

`crates/paavo-db/tests/log_ops.rs`:
```rust
use chrono::Utc;
use paavo_db::{Db, JobRow, LogFrameRow, NewJob, OutcomeRecord};
use paavo_proto::{
    BoardSelector, JobId, JobOutcome, JobSource, JobState, LogFrame, LogLevel,
    Priority, TerminalOutcome,
};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

fn enqueue_job(db: &Db) -> JobId {
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(
        db.raw_conn(),
        &NewJob {
            id,
            priority: Priority::Interactive,
            submitter: "test".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
        },
        now,
    )
    .unwrap();
    id
}

fn finalize_passed(db: &Db, id: &JobId) {
    let now = Utc::now().timestamp_millis();
    JobRow::finalize(
        db.raw_conn(),
        id,
        &OutcomeRecord {
            state: JobState::Passed,
            outcome: JobOutcome::Passed,
            finished_at_ms: now,
        },
    )
    .unwrap();
}

#[test]
fn append_then_list_round_trips() {
    let db = fresh_db();
    let id = enqueue_job(&db);

    let frames = vec![
        LogFrame { seq: 0, ts_us: 100, level: LogLevel::Info, target: None, message: "a".into() },
        LogFrame { seq: 1, ts_us: 200, level: LogLevel::Warn, target: Some("foo".into()), message: "b".into() },
        LogFrame { seq: 2, ts_us: 300, level: LogLevel::Error, target: None, message: "c".into() },
    ];
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();

    let got = LogFrameRow::list(db.raw_conn(), &id, 0, 10).unwrap();
    assert_eq!(got, frames);
}

#[test]
fn list_paginates() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames: Vec<_> = (0..50)
        .map(|i| LogFrame {
            seq: i,
            ts_us: i * 100,
            level: LogLevel::Info,
            target: None,
            message: format!("msg-{i}"),
        })
        .collect();
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();

    let page = LogFrameRow::list(db.raw_conn(), &id, 20, 10).unwrap();
    assert_eq!(page.len(), 10);
    assert_eq!(page[0].seq, 20);
    assert_eq!(page[9].seq, 29);
}

#[test]
fn count_for_job_returns_total() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames: Vec<_> = (0..7)
        .map(|i| LogFrame {
            seq: i,
            ts_us: i,
            level: LogLevel::Info,
            target: None,
            message: "x".into(),
        })
        .collect();
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();
    assert_eq!(LogFrameRow::count_for_job(db.raw_conn(), &id).unwrap(), 7);
}

#[test]
fn duplicate_seq_is_rejected() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let f = LogFrame { seq: 0, ts_us: 0, level: LogLevel::Info, target: None, message: "x".into() };
    LogFrameRow::append_batch(db.raw_conn(), &id, &[f.clone()]).unwrap();
    let err = LogFrameRow::append_batch(db.raw_conn(), &id, &[f]).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("UNIQUE") || msg.contains("PRIMARY KEY"), "{msg}");
}

#[test]
fn truncate_passed_keeps_warn_and_error_only() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames = vec![
        LogFrame { seq: 0, ts_us: 1, level: LogLevel::Trace, target: None, message: "t".into() },
        LogFrame { seq: 1, ts_us: 2, level: LogLevel::Info,  target: None, message: "i".into() },
        LogFrame { seq: 2, ts_us: 3, level: LogLevel::Warn,  target: None, message: "w".into() },
        LogFrame { seq: 3, ts_us: 4, level: LogLevel::Error, target: None, message: "e".into() },
    ];
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();
    finalize_passed(&db, &id);

    // Pretend "now" is way past the retention horizon.
    let now_ms = Utc::now().timestamp_millis() + 60 * 86_400_000;
    let cut = LogFrameRow::truncate_old_passed(db.raw_conn(), 30, now_ms).unwrap();
    assert_eq!(cut, 2);
    let remaining = LogFrameRow::list(db.raw_conn(), &id, 0, 10).unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(remaining.iter().all(|f| matches!(f.level, LogLevel::Warn | LogLevel::Error)));
}

#[test]
fn truncate_disabled_when_days_is_negative() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames = vec![
        LogFrame { seq: 0, ts_us: 1, level: LogLevel::Trace, target: None, message: "t".into() },
    ];
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();
    finalize_passed(&db, &id);

    let now_ms = Utc::now().timestamp_millis() + 1_000 * 86_400_000;
    let cut = LogFrameRow::truncate_old_passed(db.raw_conn(), -1, now_ms).unwrap();
    assert_eq!(cut, 0);
    let remaining = LogFrameRow::list(db.raw_conn(), &id, 0, 10).unwrap();
    assert_eq!(remaining.len(), 1);
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-db --test log_ops`
Expected: FAILS — `LogFrameRow::*` not implemented.

- [ ] **Step 3: Implement log frame helpers**

Replace `crates/paavo-db/src/log.rs`:
```rust
//! Log frame table typed helpers, plus the truncate-on-pass retention sweep.

use crate::error::{DbError, Result};
use paavo_proto::{JobId, LogFrame, LogLevel};
use rusqlite::{params, Connection, Row};

/// One row from the `log_frame` table. Same shape as `paavo_proto::LogFrame`,
/// but lives here so the typed query helpers have a clear owner.
pub type LogFrameRow = LogFrame;

/// Marker trait-less inherent impl block for the table.
pub struct LogFrameOps;

impl LogFrameOps {}

// Helpers are exposed as inherent methods on `LogFrame` via free functions
// in this module. We use an associated impl block under a private extension
// trait pattern to avoid orphan-impl issues.
#[allow(clippy::module_name_repetitions)]
pub trait LogFrameDb: Sized {
    /// Append a batch of frames for a job in one transaction.
    fn append_batch(conn: &Connection, job_id: &JobId, frames: &[Self]) -> Result<()>;
    /// Return frames `[offset, offset+limit)` ordered by `seq` ascending.
    fn list(
        conn: &Connection,
        job_id: &JobId,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Self>>;
    /// Total frame count for a job.
    fn count_for_job(conn: &Connection, job_id: &JobId) -> Result<u64>;
    /// Retention sweep. Delete frames with `level IN (trace, debug, info)`
    /// for any `Passed` job whose `finished_at` is older than
    /// `passed_full_log_days` ago.
    ///
    /// `passed_full_log_days < 0` disables truncation entirely. Returns the
    /// number of frames deleted.
    fn truncate_old_passed(
        conn: &Connection,
        passed_full_log_days: i32,
        now_ms: i64,
    ) -> Result<u64>;
}

impl LogFrameDb for LogFrame {
    fn append_batch(conn: &Connection, job_id: &JobId, frames: &[Self]) -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO log_frame (job_id, seq, ts_us, level, target, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for f in frames {
                stmt.execute(params![
                    job_id.to_string(),
                    f.seq as i64,
                    f.ts_us as i64,
                    level_to_str(f.level),
                    f.target,
                    f.message,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn list(
        conn: &Connection,
        job_id: &JobId,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT seq, ts_us, level, target, message FROM log_frame
             WHERE job_id = ?1 ORDER BY seq ASC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(
                params![job_id.to_string(), limit as i64, offset as i64],
                row_to_frame,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn count_for_job(conn: &Connection, job_id: &JobId) -> Result<u64> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM log_frame WHERE job_id = ?1",
            params![job_id.to_string()],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    fn truncate_old_passed(
        conn: &Connection,
        passed_full_log_days: i32,
        now_ms: i64,
    ) -> Result<u64> {
        if passed_full_log_days < 0 {
            return Ok(0);
        }
        let cutoff = now_ms - (passed_full_log_days as i64) * 86_400_000;
        let n = conn.execute(
            "DELETE FROM log_frame
             WHERE level IN ('trace','debug','info')
               AND job_id IN (
                   SELECT id FROM job
                   WHERE state = 'passed'
                     AND finished_at IS NOT NULL
                     AND finished_at < ?1
               )",
            params![cutoff],
        )?;
        Ok(n as u64)
    }
}

fn level_to_str(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

fn level_from_str(s: &str) -> Result<LogLevel> {
    Ok(match s {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        other => {
            return Err(DbError::UnknownEnum {
                column: "log_frame.level",
                value: other.to_string(),
            })
        }
    })
}

fn row_to_frame(r: &Row<'_>) -> rusqlite::Result<Result<LogFrame>> {
    let seq: i64 = r.get(0)?;
    let ts_us: i64 = r.get(1)?;
    let level_str: String = r.get(2)?;
    let target: Option<String> = r.get(3)?;
    let message: String = r.get(4)?;
    Ok(level_from_str(&level_str).map(|level| LogFrame {
        seq: seq as u64,
        ts_us: ts_us as u64,
        level,
        target,
        message,
    }))
}
```

Update `crates/paavo-db/src/lib.rs` re-exports to also export the trait:
```rust
pub use log::{LogFrameDb, LogFrameRow};
```

Replace the existing line `pub use log::LogFrameRow;` with the line above.

- [ ] **Step 4: Update the failing test to bring the trait into scope**

`crates/paavo-db/tests/log_ops.rs` add `use paavo_db::LogFrameDb;` near the top with the other paavo_db imports.

- [ ] **Step 5: Run the log ops test**

Run: `cargo test -p paavo-db --test log_ops`
Expected: 6 passed.

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-db/src/log.rs crates/paavo-db/src/lib.rs crates/paavo-db/tests/log_ops.rs
git -C D:\workspace\paavo commit -m "feat(db): log_frame helpers + truncate_old_passed retention sweep"
```

---

#### 1.3.e: Build cache table typed helpers + LRU eviction

- [ ] **Step 1: Write the failing build_cache test**

`crates/paavo-db/tests/build_cache_ops.rs`:
```rust
use chrono::Utc;
use paavo_db::{BuildCacheEntry, BuildCacheStats, Db};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

#[test]
fn upsert_then_get_round_trips() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    let e = BuildCacheEntry {
        tar_blake3: "aaa".into(),
        elf_path: "/cache/aaa/foo.elf".into(),
        built_at: now,
        last_used_at: now,
        size_bytes: 1_000,
    };
    BuildCacheEntry::upsert(db.raw_conn(), &e).unwrap();
    let got = BuildCacheEntry::get(db.raw_conn(), "aaa").unwrap();
    assert_eq!(got, e);
}

#[test]
fn touch_last_used_advances_recency() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BuildCacheEntry::upsert(
        db.raw_conn(),
        &BuildCacheEntry {
            tar_blake3: "aaa".into(),
            elf_path: "/c/foo.elf".into(),
            built_at: now,
            last_used_at: now,
            size_bytes: 100,
        },
    )
    .unwrap();
    BuildCacheEntry::touch_last_used(db.raw_conn(), "aaa", now + 1_000).unwrap();
    let e = BuildCacheEntry::get(db.raw_conn(), "aaa").unwrap();
    assert_eq!(e.last_used_at, now + 1_000);
    assert_eq!(e.built_at, now);
}

#[test]
fn stats_reports_total_size_and_count() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    for (k, s) in [("a", 100), ("b", 250), ("c", 75)] {
        BuildCacheEntry::upsert(
            db.raw_conn(),
            &BuildCacheEntry {
                tar_blake3: k.into(),
                elf_path: format!("/c/{k}.elf"),
                built_at: now,
                last_used_at: now,
                size_bytes: s,
            },
        )
        .unwrap();
    }
    let st = BuildCacheEntry::stats(db.raw_conn()).unwrap();
    assert_eq!(st, BuildCacheStats { total_bytes: 425, count: 3 });
}

#[test]
fn evict_until_under_drops_least_recently_used_first() {
    let db = fresh_db();
    let t = Utc::now().timestamp_millis();
    // Insert three entries with increasing recency.
    BuildCacheEntry::upsert(db.raw_conn(), &BuildCacheEntry {
        tar_blake3: "oldest".into(), elf_path: "/c/o.elf".into(),
        built_at: t, last_used_at: t,         size_bytes: 100,
    }).unwrap();
    BuildCacheEntry::upsert(db.raw_conn(), &BuildCacheEntry {
        tar_blake3: "middle".into(), elf_path: "/c/m.elf".into(),
        built_at: t, last_used_at: t + 100,   size_bytes: 100,
    }).unwrap();
    BuildCacheEntry::upsert(db.raw_conn(), &BuildCacheEntry {
        tar_blake3: "newest".into(), elf_path: "/c/n.elf".into(),
        built_at: t, last_used_at: t + 200,   size_bytes: 100,
    }).unwrap();

    // Total = 300; cap to 150. Expect 'oldest' and 'middle' dropped.
    let evicted = BuildCacheEntry::evict_until_under(db.raw_conn(), 150).unwrap();
    assert_eq!(
        evicted.iter().map(|e| e.tar_blake3.as_str()).collect::<Vec<_>>(),
        vec!["oldest", "middle"]
    );
    let st = BuildCacheEntry::stats(db.raw_conn()).unwrap();
    assert_eq!(st.total_bytes, 100);
    assert_eq!(st.count, 1);
}

#[test]
fn evict_until_under_noop_when_already_under_cap() {
    let db = fresh_db();
    let t = Utc::now().timestamp_millis();
    BuildCacheEntry::upsert(db.raw_conn(), &BuildCacheEntry {
        tar_blake3: "only".into(), elf_path: "/c/o.elf".into(),
        built_at: t, last_used_at: t, size_bytes: 50,
    }).unwrap();
    let evicted = BuildCacheEntry::evict_until_under(db.raw_conn(), 100).unwrap();
    assert!(evicted.is_empty());
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-db --test build_cache_ops`
Expected: FAILS — types and methods don't exist yet.

- [ ] **Step 3: Implement build_cache helpers**

Replace `crates/paavo-db/src/build_cache.rs`:
```rust
//! build_cache table typed helpers and LRU eviction policy.

use crate::error::Result;
use rusqlite::{params, Connection, Row};

/// One row from the `build_cache` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheEntry {
    /// blake3 of the input tar.
    pub tar_blake3: String,
    /// On-disk ELF location.
    pub elf_path: String,
    /// First-built time, epoch ms.
    pub built_at: i64,
    /// Last-accessed time, epoch ms (drives LRU).
    pub last_used_at: i64,
    /// Disk footprint of the cached ELF, in bytes.
    pub size_bytes: u64,
}

/// Aggregate stats for the build-cache LRU policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildCacheStats {
    /// Sum of `size_bytes` across all entries.
    pub total_bytes: u64,
    /// Number of entries.
    pub count: u64,
}

impl BuildCacheEntry {
    /// Insert or replace an entry by `tar_blake3`.
    pub fn upsert(conn: &Connection, e: &BuildCacheEntry) -> Result<()> {
        conn.execute(
            "INSERT INTO build_cache
                (tar_blake3, elf_path, built_at, last_used_at, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(tar_blake3) DO UPDATE SET
                elf_path = excluded.elf_path,
                last_used_at = excluded.last_used_at,
                size_bytes = excluded.size_bytes",
            params![
                e.tar_blake3,
                e.elf_path,
                e.built_at,
                e.last_used_at,
                e.size_bytes as i64
            ],
        )?;
        Ok(())
    }

    /// Fetch an entry; errors if missing.
    pub fn get(conn: &Connection, tar_blake3: &str) -> Result<BuildCacheEntry> {
        let e = conn.query_row(
            "SELECT tar_blake3, elf_path, built_at, last_used_at, size_bytes
             FROM build_cache WHERE tar_blake3 = ?1",
            params![tar_blake3],
            row_to_entry,
        )?;
        Ok(e)
    }

    /// Find an entry, returning `Ok(None)` if missing.
    pub fn find(conn: &Connection, tar_blake3: &str) -> Result<Option<BuildCacheEntry>> {
        let opt = conn
            .query_row(
                "SELECT tar_blake3, elf_path, built_at, last_used_at, size_bytes
                 FROM build_cache WHERE tar_blake3 = ?1",
                params![tar_blake3],
                row_to_entry,
            )
            .ok();
        Ok(opt)
    }

    /// Update `last_used_at`.
    pub fn touch_last_used(
        conn: &Connection,
        tar_blake3: &str,
        now_ms: i64,
    ) -> Result<()> {
        conn.execute(
            "UPDATE build_cache SET last_used_at = ?1 WHERE tar_blake3 = ?2",
            params![now_ms, tar_blake3],
        )?;
        Ok(())
    }

    /// Aggregate stats across all entries.
    pub fn stats(conn: &Connection) -> Result<BuildCacheStats> {
        let (total, count): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes),0), COUNT(*) FROM build_cache",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(BuildCacheStats {
            total_bytes: total as u64,
            count: count as u64,
        })
    }

    /// Drop entries in `last_used_at` ascending order until total bytes
    /// fits under `max_bytes`. Returns the entries evicted (for the caller
    /// to remove from disk).
    pub fn evict_until_under(
        conn: &Connection,
        max_bytes: u64,
    ) -> Result<Vec<BuildCacheEntry>> {
        let mut evicted = Vec::new();
        loop {
            let st = Self::stats(conn)?;
            if st.total_bytes <= max_bytes {
                return Ok(evicted);
            }
            let oldest: Option<BuildCacheEntry> = conn
                .query_row(
                    "SELECT tar_blake3, elf_path, built_at, last_used_at, size_bytes
                     FROM build_cache ORDER BY last_used_at ASC LIMIT 1",
                    [],
                    row_to_entry,
                )
                .ok();
            let Some(victim) = oldest else { return Ok(evicted) };
            conn.execute(
                "DELETE FROM build_cache WHERE tar_blake3 = ?1",
                params![victim.tar_blake3],
            )?;
            evicted.push(victim);
        }
    }
}

fn row_to_entry(r: &Row<'_>) -> rusqlite::Result<BuildCacheEntry> {
    Ok(BuildCacheEntry {
        tar_blake3: r.get(0)?,
        elf_path: r.get(1)?,
        built_at: r.get(2)?,
        last_used_at: r.get(3)?,
        size_bytes: r.get::<_, i64>(4)? as u64,
    })
}
```

- [ ] **Step 4: Run the build_cache test**

Run: `cargo test -p paavo-db --test build_cache_ops`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-db/src/build_cache.rs crates/paavo-db/tests/build_cache_ops.rs
git -C D:\workspace\paavo commit -m "feat(db): build_cache helpers + LRU evict_until_under"
```

---

#### 1.3.f: Schedule table typed helpers (minimal)

Goal: enough so the cron driver in M4 can land. Full driver lives in paavod.

- [ ] **Step 1: Write the failing test**

`crates/paavo-db/tests/schedule_ops.rs`:
```rust
use paavo_db::{Db, ScheduleRow, ScheduleUpdate};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

#[test]
fn upsert_then_get() {
    let db = fresh_db();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 19 * * *".into(),
            enabled: true,
            last_triggered_at: None,
            last_completed_at: None,
        },
    )
    .unwrap();
    let r = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(r.cron, "0 19 * * *");
    assert!(r.enabled);
    assert!(r.last_triggered_at.is_none());
}

#[test]
fn apply_update_sets_triggered_then_completed() {
    let db = fresh_db();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 19 * * *".into(),
            enabled: true,
            last_triggered_at: None,
            last_completed_at: None,
        },
    )
    .unwrap();
    ScheduleRow::apply_update(
        db.raw_conn(),
        "nightly",
        &ScheduleUpdate {
            last_triggered_at: Some(100),
            last_completed_at: None,
        },
    )
    .unwrap();
    let r = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(r.last_triggered_at, Some(100));

    ScheduleRow::apply_update(
        db.raw_conn(),
        "nightly",
        &ScheduleUpdate {
            last_triggered_at: None,
            last_completed_at: Some(200),
        },
    )
    .unwrap();
    let r = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(r.last_completed_at, Some(200));
    assert_eq!(r.last_triggered_at, Some(100));
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-db --test schedule_ops`
Expected: FAILS — types not implemented.

- [ ] **Step 3: Implement schedule helpers**

Replace `crates/paavo-db/src/schedule.rs`:
```rust
//! schedule table typed helpers.

use crate::error::Result;
use rusqlite::{params, Connection};

/// One row from the `schedule` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRow {
    /// Schedule id, e.g. `nightly`.
    pub id: String,
    /// Cron expression.
    pub cron: String,
    /// Whether the schedule is currently active.
    pub enabled: bool,
    /// Last firing time, epoch ms.
    pub last_triggered_at: Option<i64>,
    /// Last completion time, epoch ms.
    pub last_completed_at: Option<i64>,
}

/// Partial update used after a schedule firing or completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleUpdate {
    /// New value for `last_triggered_at`, if any.
    pub last_triggered_at: Option<i64>,
    /// New value for `last_completed_at`, if any.
    pub last_completed_at: Option<i64>,
}

impl ScheduleRow {
    /// Insert or replace a schedule row by id.
    pub fn upsert(conn: &Connection, s: &ScheduleRow) -> Result<()> {
        conn.execute(
            "INSERT INTO schedule
                (id, cron, enabled, last_triggered_at, last_completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                cron = excluded.cron,
                enabled = excluded.enabled",
            params![
                s.id,
                s.cron,
                s.enabled as i64,
                s.last_triggered_at,
                s.last_completed_at
            ],
        )?;
        Ok(())
    }

    /// Fetch one schedule by id.
    pub fn get(conn: &Connection, id: &str) -> Result<ScheduleRow> {
        conn.query_row(
            "SELECT id, cron, enabled, last_triggered_at, last_completed_at
             FROM schedule WHERE id = ?1",
            params![id],
            |r| {
                Ok(ScheduleRow {
                    id: r.get(0)?,
                    cron: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? == 1,
                    last_triggered_at: r.get(3)?,
                    last_completed_at: r.get(4)?,
                })
            },
        )
        .map_err(Into::into)
    }

    /// Apply a partial update; fields set to `None` are not touched.
    pub fn apply_update(
        conn: &Connection,
        id: &str,
        u: &ScheduleUpdate,
    ) -> Result<()> {
        if let Some(t) = u.last_triggered_at {
            conn.execute(
                "UPDATE schedule SET last_triggered_at = ?1 WHERE id = ?2",
                params![t, id],
            )?;
        }
        if let Some(t) = u.last_completed_at {
            conn.execute(
                "UPDATE schedule SET last_completed_at = ?1 WHERE id = ?2",
                params![t, id],
            )?;
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run the schedule test**

Run: `cargo test -p paavo-db --test schedule_ops`
Expected: 2 passed.

- [ ] **Step 5: Run the whole crate's test suite + clippy**

Run: `cargo test -p paavo-db && cargo clippy -p paavo-db --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-db/src/schedule.rs crates/paavo-db/tests/schedule_ops.rs
git -C D:\workspace\paavo commit -m "feat(db): schedule helpers (upsert/get/apply_update)"
```

---

### Milestone 1 exit criteria

- [ ] `cargo test --workspace` green
- [ ] `paavo-proto` round-trips every wire type defined in spec §5, §7, §9
- [ ] `paavo-meta::inactivity_timeout!()` compiles and `#[used] static` is reachable
- [ ] `paavo-db::Db::open` creates 5 user tables + indexes + WAL; `open_readonly` works against the same file
- [ ] `paavo-db` typed helpers cover board/job/log_frame/build_cache/schedule with TDD tests for every public method

---

## Milestone 2 — Probe layer

Goal: `paavo-probe` (ELF section parser, probe-rs adapter behind a trait, defmt-decoder wrapper) and `paavo-runner` (one BoardWorker OS thread, paired watchdog, mpsc out). All TDD using a fake probe trait impl.

### Task 2.1: paavo-probe — ELF section parser

Spec coverage: §4 (`paavo-probe` responsibilities), §6.4 (`.paavo.inactivity_timeout`), on-ELF section convention (`.paavo.target`, `.paavo.timeout` emitted by `paavo-meta` macros).

**Files:**
- Create: `crates/paavo-probe/src/lib.rs` (replace skeleton)
- Create: `crates/paavo-probe/src/sections.rs`
- Create: `crates/paavo-probe/src/error.rs`
- Create: `crates/paavo-probe/tests/fixtures/.gitkeep`
- Create: `crates/paavo-probe/build_fixture.sh` (a helper, not strictly required for tests)
- Create: `crates/paavo-probe/tests/sections.rs`
- Create: `crates/paavo-probe/tests/synthetic_elf.rs`

We can't easily build real cross-compiled ELFs in workspace tests; instead, we synthesize tiny ELFs with the named sections via the `object` crate's writer, so the parser is testable on host with no toolchain.

- [ ] **Step 1: Write the failing section-parser test**

`crates/paavo-probe/tests/sections.rs`:
```rust
use paavo_probe::sections::{parse_meta_sections, MetaSections};

fn synth_elf(sections: &[(&str, &[u8])]) -> Vec<u8> {
    use object::write::{Object, StandardSection, Symbol, SymbolSection};
    use object::{Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope};

    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Arm, Endianness::Little);

    // Required for ARM thumb elves; not strictly necessary for parsing but
    // keeps the file shape realistic.
    let text_id = obj.section_id(StandardSection::Text);
    obj.append_section_data(text_id, &[0u8; 4], 4);

    for (name, data) in sections {
        let sect_id = obj.add_section(
            Vec::new(),
            name.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );
        obj.append_section_data(sect_id, data, 4);
        let _ = obj.add_symbol(Symbol {
            name: name.replace('.', "_").into_bytes(),
            value: 0,
            size: data.len() as u64,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(sect_id),
            flags: SymbolFlags::None,
        });
    }
    obj.write().unwrap()
}

#[test]
fn parses_all_three_sections_when_present() {
    let target = b"frdm-mcx-a266\0";
    let timeout = (3600u32).to_le_bytes();
    let inact = (60u32).to_le_bytes();
    let elf = synth_elf(&[
        (".paavo.target", target),
        (".paavo.timeout", &timeout),
        (".paavo.inactivity_timeout", &inact),
    ]);
    let m = parse_meta_sections(&elf).unwrap();
    assert_eq!(m.target.as_deref(), Some("frdm-mcx-a266"));
    assert_eq!(m.timeout_s, Some(3600));
    assert_eq!(m.inactivity_timeout_s, Some(60));
}

#[test]
fn missing_sections_become_none() {
    let elf = synth_elf(&[(".paavo.target", b"foo\0")]);
    let m = parse_meta_sections(&elf).unwrap();
    assert_eq!(m.target.as_deref(), Some("foo"));
    assert_eq!(m.timeout_s, None);
    assert_eq!(m.inactivity_timeout_s, None);
}

#[test]
fn empty_target_section_is_an_error() {
    let elf = synth_elf(&[(".paavo.target", b"")]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("target") && msg.contains("empty"), "{msg}");
}

#[test]
fn wrong_size_timeout_section_is_an_error() {
    let elf = synth_elf(&[(".paavo.timeout", &[1u8, 2, 3])]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("timeout") && msg.contains("4 bytes"), "{msg}");
}

#[test]
fn meta_sections_default_is_all_none() {
    let m = MetaSections::default();
    assert!(m.target.is_none());
    assert!(m.timeout_s.is_none());
    assert!(m.inactivity_timeout_s.is_none());
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-probe --test sections`
Expected: FAILS — `paavo_probe::sections` doesn't exist.

- [ ] **Step 3: Implement section parser**

`crates/paavo-probe/src/error.rs`:
```rust
//! Errors returned by paavo-probe.

use thiserror::Error;

/// Errors during ELF parsing or probe operations.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// `object` crate refused to parse the ELF.
    #[error("elf parse: {0}")]
    Elf(#[from] object::Error),
    /// A `.paavo.target` section was empty (or first byte was NUL).
    #[error("`.paavo.target` section is empty")]
    EmptyTarget,
    /// A `.paavo.target` section had unexpected wire format (NUL-less,
    /// interior NUL with trailing bytes, or invalid UTF-8).
    #[error("`.paavo.target` section is malformed: {reason}")]
    MalformedTarget {
        /// Human-readable diagnostic.
        reason: String,
    },
    /// A `.paavo.timeout` / `.paavo.inactivity_timeout` section was
    /// not exactly 4 bytes (u32 LE).
    #[error("`{section}` section must be 4 bytes (u32 LE), got {got}")]
    BadIntegerSection {
        /// Section name.
        section: &'static str,
        /// Actual length.
        got: usize,
    },
    /// probe-rs connect or operation error (only used when the real adapter
    /// is in play; mocks never produce this).
    #[error("probe-rs: {0}")]
    ProbeRs(String),
    /// defmt-decoder failed to read or decode the symbol table.
    #[error("defmt decode: {0}")]
    Defmt(String),
}

/// Result alias.
pub type Result<T, E = ProbeError> = std::result::Result<T, E>;
```

`crates/paavo-probe/src/sections.rs`:
```rust
//! Parser for the `.paavo.*` ELF sections embedded by the `paavo-meta`
//! macros (and, incidentally, by any existing tooling that emits the same
//! section names — wire-format compatible).

use crate::error::{ProbeError, Result};
use object::{Object, ObjectSection};

/// Parsed contents of all three optional metadata sections.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MetaSections {
    /// `.paavo.target` — must match a `BoardSpec::target_name` in the
    /// inventory.
    pub target: Option<String>,
    /// `.paavo.timeout` — per-test hard-max override, in seconds.
    pub timeout_s: Option<u32>,
    /// `.paavo.inactivity_timeout` — per-test inactivity override, in
    /// seconds.
    pub inactivity_timeout_s: Option<u32>,
}

/// Parse the three `.paavo.*` sections out of an ELF byte buffer.
///
/// - Missing sections yield `None` on the corresponding field — they are
///   not errors.
/// - Sections present but malformed (empty target, wrong-size integer) are
///   errors so the caller doesn't fall back to a default that masks a real
///   bug in the test crate's build wiring.
pub fn parse_meta_sections(elf: &[u8]) -> Result<MetaSections> {
    let file = object::File::parse(elf)?;
    let mut out = MetaSections::default();

    if let Some(s) = section_data(&file, ".paavo.target")? {
        out.target = Some(parse_cstring(s)?);
    }
    if let Some(s) = section_data(&file, ".paavo.timeout")? {
        out.timeout_s = Some(parse_u32_le(".paavo.timeout", s)?);
    }
    if let Some(s) = section_data(&file, ".paavo.inactivity_timeout")? {
        out.inactivity_timeout_s =
            Some(parse_u32_le(".paavo.inactivity_timeout", s)?);
    }
    Ok(out)
}

fn section_data<'a>(
    file: &'a object::File<'a>,
    name: &str,
) -> Result<Option<&'a [u8]>> {
    let Some(section) = file.section_by_name(name) else {
        return Ok(None);
    };
    let data = section.data()?;
    Ok(Some(data))
}

/// Parse `.paavo.target` wire format: exactly N non-NUL bytes
/// followed by a single trailing NUL.
///
/// Anything else (empty, no NUL, interior NUL with trailing bytes,
/// invalid UTF-8) is a malformed-producer error — this parser refuses
/// to paper over a build-wiring bug for *any* downstream producer (not
/// just paavo-meta, which is the only one today).
fn parse_cstring(bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() {
        return Err(ProbeError::EmptyTarget);
    }
    let Some(nul_pos) = bytes.iter().position(|&b| b == 0) else {
        return Err(ProbeError::MalformedTarget {
            reason: "missing trailing NUL".into(),
        });
    };
    if nul_pos == 0 {
        return Err(ProbeError::EmptyTarget);
    }
    if nul_pos != bytes.len() - 1 {
        return Err(ProbeError::MalformedTarget {
            reason: format!(
                "interior NUL at byte {nul_pos} with {trailing} trailing bytes after",
                trailing = bytes.len() - nul_pos - 1
            ),
        });
    }
    std::str::from_utf8(&bytes[..nul_pos])
        .map(str::to_owned)
        .map_err(|e| ProbeError::MalformedTarget {
            reason: format!("invalid UTF-8 at byte {}", e.valid_up_to()),
        })
}

fn parse_u32_le(name: &'static str, bytes: &[u8]) -> Result<u32> {
    if bytes.len() != 4 {
        return Err(ProbeError::BadIntegerSection {
            section: name,
            got: bytes.len(),
        });
    }
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
```

`crates/paavo-probe/src/lib.rs`:
```rust
//! Low-level probe driver. Wraps `probe-rs` and `defmt-decoder` and parses
//! the `.paavo.*` ELF sections that scaffolded test crates emit via the
//! `paavo-meta` macros.
//!
//! Layered for testability:
//! - `sections` — pure ELF byte parser (no probe-rs).
//! - `event` — `Event` enum streamed back to `paavo-runner`.
//! - `Session` (trait) — the probe-rs adapter surface. Real impl wraps
//!   `probe_rs::Session`; a `MockSession` impl lives in `tests/` for
//!   `paavo-runner` to drive deterministically.
//!
//! ```
//! assert_eq!(paavo_probe::CRATE_NAME, "paavo-probe");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-probe";

mod error;
mod event;
mod session;
pub mod sections;

pub use error::{ProbeError, Result};
pub use event::Event;
pub use session::{ProbeSession, RealSession, RealSessionOptions};
```

For now, stub `event.rs` and `session.rs` so the crate compiles:

`crates/paavo-probe/src/event.rs`:
```rust
//! Events emitted by a probe session. Implemented fully in Task 2.2.

use paavo_proto::LogFrame;

/// One observable event from a running test binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Decoded defmt frame.
    LogFrame(LogFrame),
    /// CPU hit a software breakpoint (the embassy `bkpt()` convention).
    /// Combined with a preceding `defmt::info!("Test OK")` this signals pass.
    Bkpt,
    /// A panic was observed (panic-probe encodes via defmt; this event is
    /// emitted when the runner recognises the panic frame pattern).
    Panic {
        /// Captured panic message.
        message: String,
    },
    /// Probe lost the target (USB drop, target reset without our consent).
    Disconnect,
}
```

`crates/paavo-probe/src/session.rs`:
```rust
//! Probe session abstraction. The real impl lives here; tests in
//! paavo-runner stub the trait with a fake.

use crate::error::Result;
use crate::event::Event;

/// Long-lived probe session that flashes and observes a single test.
///
/// Implementors must be `Send` because the BoardWorker thread owns the
/// session for the duration of a job.
pub trait ProbeSession: Send {
    /// Block until the next event is available, or return `Ok(None)` if the
    /// target has reached a clean stop. Implementations may return events as
    /// they become available with no inter-event delay.
    fn next_event(&mut self, timeout_ms: u32) -> Result<Option<Event>>;
}

/// Connection options for the real probe-rs adapter.
#[derive(Debug, Clone)]
pub struct RealSessionOptions {
    /// USB selector for probe-rs.
    pub probe_selector: paavo_proto::ProbeSelector,
    /// probe-rs chip name.
    pub chip_name: String,
    /// Path to the ELF to flash and run.
    pub elf_path: std::path::PathBuf,
    /// If true, skip the post-load reset (NXP RT685S quirk; see spec §2).
    pub skip_post_load_reset: bool,
}

/// Real `probe-rs` + `defmt-decoder` backed session. Fully wired in
/// Milestone 6 (hardware smoke); the in-tree tests use a mock session.
pub struct RealSession {
    _opts: RealSessionOptions,
}

impl RealSession {
    /// Connect to a probe, flash the ELF, and start RTT.
    ///
    /// **Hardware-only** — this constructor reaches out to probe-rs and
    /// requires a physical probe + board. Workspace tests must use a mock
    /// impl of `ProbeSession`.
    pub fn connect(opts: RealSessionOptions) -> Result<Self> {
        // The probe-rs API surface is implemented in Task 6.4 (hardware
        // smoke). For now we provide the constructor signature so callers
        // (paavo-runner) can compile, but it returns an error if invoked.
        Err(crate::ProbeError::ProbeRs(
            "RealSession::connect is wired in Milestone 6.4; \
             use a mock ProbeSession for in-workspace tests"
                .into(),
        ))?;
        Ok(Self { _opts: opts })
    }
}

impl ProbeSession for RealSession {
    fn next_event(&mut self, _timeout_ms: u32) -> Result<Option<Event>> {
        Err(crate::ProbeError::ProbeRs(
            "RealSession::next_event is wired in Milestone 6.4".into(),
        ))
    }
}
```

- [ ] **Step 4: Run the sections test**

Run: `cargo test -p paavo-probe --test sections`
Expected: 5 passed.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p paavo-probe --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-probe
git -C D:\workspace\paavo commit -m "feat(probe): .paavo.* ELF section parser + ProbeSession trait surface"
```

---

### Task 2.2: paavo-runner — BoardWorker + Watchdog

Spec coverage: §4.3 (one OS thread per board), §6.1 (watchdog responsibilities), §6.2 (defaults), §6.4 (inactivity_timeout precedence).

**Files:**
- Create: `crates/paavo-runner/src/lib.rs` (replace skeleton)
- Create: `crates/paavo-runner/src/worker.rs`
- Create: `crates/paavo-runner/src/watchdog.rs`
- Create: `crates/paavo-runner/src/job.rs`
- Test: `crates/paavo-runner/tests/inactivity_watchdog.rs`
- Test: `crates/paavo-runner/tests/hard_max_watchdog.rs`
- Test: `crates/paavo-runner/tests/pass_path.rs`
- Test: `crates/paavo-runner/tests/cancel_path.rs`
- Test: `crates/paavo-runner/tests/common/mod.rs`

- [ ] **Step 1: Sketch the runner module structure**

`crates/paavo-runner/src/lib.rs`:
```rust
//! Per-job runner: owns one probe via `paavo-probe`, runs the inactivity
//! and hard-max watchdog, streams `LogFrame` events out, and returns a
//! terminal `JobOutcome`.
//!
//! ```
//! assert_eq!(paavo_runner::CRATE_NAME, "paavo-runner");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-runner";

mod job;
mod watchdog;
mod worker;

pub use job::{JobInputs, JobOutputs, RunCommand};
pub use worker::{run_job, BoardWorkerHandle};
```

`crates/paavo-runner/src/job.rs`:
```rust
//! Per-job input / output shapes for `run_job`.

use crossbeam_channel::{Receiver, Sender};
use paavo_proto::{JobId, JobOutcome, LogFrame};

/// Inputs the caller provides to a BoardWorker.
pub struct JobInputs {
    /// Job being executed.
    pub job_id: JobId,
    /// Effective inactivity timeout for this job, in **milliseconds**.
    pub inactivity_timeout_ms: u64,
    /// Effective hard-max wall clock for this job, in **milliseconds**.
    pub hard_max_ms: u64,
    /// How long to wait, after we ask the worker to stop, before declaring
    /// the probe unresponsive and counting an infra failure.
    pub probe_release_grace_ms: u64,
    /// Cancel signal channel — receive end is checked by the watchdog.
    pub cancel_rx: Receiver<RunCommand>,
}

/// Outputs produced by a BoardWorker.
pub struct JobOutputs {
    /// LogFrame stream — closed when the worker exits.
    pub log_tx: Sender<LogFrame>,
}

/// External commands to a running job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCommand {
    /// User-requested cancel. Watchdog signals worker; on timely release,
    /// outcome is `Aborted{User}`.
    Cancel,
    /// Daemon shutdown drain. Same as `Cancel` but produces
    /// `Aborted{DaemonShutdown}`.
    DaemonShutdown,
}
```

`crates/paavo-runner/src/watchdog.rs`:
```rust
//! Watchdog thread: tick every 100 ms, fire Cancel if either inactivity or
//! hard-max exceeded, or if the cancel channel produces a command.

use crate::job::RunCommand;
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Reason the watchdog signalled a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `now - last_activity > inactivity`.
    Inactivity,
    /// `now - start > hard_max`.
    HardMax,
    /// External `RunCommand::Cancel`.
    UserCancel,
    /// External `RunCommand::DaemonShutdown`.
    DaemonShutdown,
}

/// Shared mutable state between the BoardWorker and its Watchdog.
pub struct WatchdogState {
    /// Wall-clock instant of the most recent LogFrame; updated on every
    /// frame the worker sees.
    pub last_activity: Mutex<Instant>,
    /// Wall-clock instant the job started.
    pub started_at: Instant,
    /// Set by the watchdog when it has fired; the worker checks this each
    /// time it considers another `next_event` call.
    pub stop_reason: Mutex<Option<StopReason>>,
}

impl WatchdogState {
    /// Construct fresh state.
    pub fn new(now: Instant) -> Arc<Self> {
        Arc::new(Self {
            last_activity: Mutex::new(now),
            started_at: now,
            stop_reason: Mutex::new(None),
        })
    }

    /// Worker bumps this on every event observed.
    pub fn touch(&self, now: Instant) {
        *self.last_activity.lock() = now;
    }

    /// Worker polls this to decide whether to break its loop.
    pub fn stop_reason(&self) -> Option<StopReason> {
        *self.stop_reason.lock()
    }
}

/// Tick loop. Returns when a stop has been signalled.
pub fn run_watchdog(
    state: Arc<WatchdogState>,
    inactivity: Duration,
    hard_max: Duration,
    cancel_rx: Receiver<RunCommand>,
    notify_tx: Sender<StopReason>,
    tick: Duration,
) {
    loop {
        // External signal?
        if let Ok(cmd) = cancel_rx.try_recv() {
            let reason = match cmd {
                RunCommand::Cancel => StopReason::UserCancel,
                RunCommand::DaemonShutdown => StopReason::DaemonShutdown,
            };
            set_and_notify(&state, reason, &notify_tx);
            return;
        }
        let now = Instant::now();
        let elapsed_total = now.duration_since(state.started_at);
        let elapsed_silent = now.duration_since(*state.last_activity.lock());
        if elapsed_silent > inactivity {
            set_and_notify(&state, StopReason::Inactivity, &notify_tx);
            return;
        }
        if elapsed_total > hard_max {
            set_and_notify(&state, StopReason::HardMax, &notify_tx);
            return;
        }
        std::thread::sleep(tick);
    }
}

fn set_and_notify(
    state: &WatchdogState,
    reason: StopReason,
    notify_tx: &Sender<StopReason>,
) {
    *state.stop_reason.lock() = Some(reason);
    let _ = notify_tx.send(reason);
}
```

`crates/paavo-runner/src/worker.rs`:
```rust
//! BoardWorker entry point. `run_job` takes a probe session (real or mock),
//! drives it until either the test reports done (`Test OK` + bkpt → pass,
//! panic frame → fail) or the watchdog fires.

use crate::job::{JobInputs, JobOutputs};
use crate::watchdog::{run_watchdog, StopReason, WatchdogState};
use crossbeam_channel::{bounded, Sender};
use paavo_probe::{Event, ProbeSession};
use paavo_proto::{
    AbortReason, JobOutcome, LogFrame, LogLevel, TerminalOutcome, TimeoutReason,
};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Handle to a spawned BoardWorker thread. Drop blocks the caller until the
/// thread exits.
pub struct BoardWorkerHandle {
    join: Option<thread::JoinHandle<JobOutcome>>,
}

impl BoardWorkerHandle {
    /// Wait for the worker to finish and return the terminal outcome.
    pub fn join(mut self) -> JobOutcome {
        self.join
            .take()
            .expect("join already called")
            .join()
            .expect("BoardWorker panicked")
    }
}

impl Drop for BoardWorkerHandle {
    fn drop(&mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Run a job to terminal outcome.
///
/// `make_session` is called on the worker thread (so probe-rs runs off the
/// caller's thread). The mock-session test path supplies a closure that
/// returns a `Box<dyn ProbeSession>` wrapping a deterministic event source.
pub fn run_job<F>(
    inputs: JobInputs,
    outputs: JobOutputs,
    make_session: F,
) -> BoardWorkerHandle
where
    F: FnOnce() -> paavo_probe::Result<Box<dyn ProbeSession>> + Send + 'static,
{
    let JobInputs {
        job_id: _,
        inactivity_timeout_ms,
        hard_max_ms,
        probe_release_grace_ms,
        cancel_rx,
    } = inputs;
    let JobOutputs { log_tx } = outputs;

    let join = thread::Builder::new()
        .name("paavo-board-worker".into())
        .spawn(move || -> JobOutcome {
            let state = WatchdogState::new(Instant::now());

            let (notify_tx, notify_rx) = bounded::<StopReason>(1);
            let watchdog_state = state.clone();
            let watchdog_inactivity = Duration::from_millis(inactivity_timeout_ms);
            let watchdog_hardmax = Duration::from_millis(hard_max_ms);
            let watchdog_join = thread::Builder::new()
                .name("paavo-watchdog".into())
                .spawn(move || {
                    run_watchdog(
                        watchdog_state,
                        watchdog_inactivity,
                        watchdog_hardmax,
                        cancel_rx,
                        notify_tx,
                        Duration::from_millis(100),
                    )
                })
                .expect("spawn watchdog");

            let session = match make_session() {
                Ok(s) => s,
                Err(e) => {
                    let _ = watchdog_join.join();
                    return JobOutcome::Failed(TerminalOutcome::InfraErr {
                        stage: "probe_attach".into(),
                        message: format!("{e}"),
                    });
                }
            };
            let outcome = drive_session(
                session,
                state.clone(),
                &log_tx,
                notify_rx,
                Duration::from_millis(probe_release_grace_ms),
            );
            let _ = watchdog_join.join();
            outcome
        })
        .expect("spawn worker");

    BoardWorkerHandle { join: Some(join) }
}

fn drive_session(
    mut session: Box<dyn ProbeSession>,
    state: Arc<WatchdogState>,
    log_tx: &Sender<LogFrame>,
    notify_rx: crossbeam_channel::Receiver<StopReason>,
    release_grace: Duration,
) -> JobOutcome {
    let mut seen_test_ok = false;
    loop {
        // Watchdog-fired stop takes priority over everything.
        if let Some(reason) = state.stop_reason() {
            return finalise_for_stop(reason, state.started_at, release_grace, &mut session);
        }

        match session.next_event(/* timeout_ms = */ 500) {
            Ok(Some(Event::LogFrame(frame))) => {
                state.touch(Instant::now());
                // Pass detection: an info-level `Test OK` followed by `Bkpt`.
                if frame.level == LogLevel::Info && frame.message.contains("Test OK") {
                    seen_test_ok = true;
                }
                let _ = log_tx.send(frame);
            }
            Ok(Some(Event::Bkpt)) => {
                if seen_test_ok {
                    return JobOutcome::Passed;
                }
                // bkpt without Test OK marker → treat as test error.
                return JobOutcome::Failed(TerminalOutcome::TestErr {
                    message: "bkpt without preceding Test OK".into(),
                });
            }
            Ok(Some(Event::Panic { message })) => {
                return JobOutcome::Failed(TerminalOutcome::TestErr { message });
            }
            Ok(Some(Event::Disconnect)) => {
                return JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "probe_disconnect".into(),
                    message: "probe disconnected mid-run".into(),
                });
            }
            Ok(None) => {
                // No event this tick; loop back to watchdog check.
                continue;
            }
            Err(e) => {
                return JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "probe_io".into(),
                    message: format!("{e}"),
                });
            }
        }

        // Drain any watchdog notification that came in during the call.
        if let Ok(_reason) = notify_rx.try_recv() {
            // The next loop iteration will pick it up via `stop_reason()`.
        }
    }
}

fn finalise_for_stop(
    reason: StopReason,
    started_at: Instant,
    _release_grace: Duration,
    _session: &mut Box<dyn ProbeSession>,
) -> JobOutcome {
    let elapsed_ms = Instant::now().duration_since(started_at).as_millis() as u64;
    match reason {
        StopReason::Inactivity => JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms,
        },
        StopReason::HardMax => JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms,
        },
        StopReason::UserCancel => JobOutcome::Aborted {
            by: AbortReason::User,
        },
        StopReason::DaemonShutdown => JobOutcome::Aborted {
            by: AbortReason::DaemonShutdown,
        },
    }
}
```

- [ ] **Step 2: Confirm the crate still compiles**

Run: `cargo build -p paavo-runner`
Expected: builds. No tests run yet.

- [ ] **Step 3: Write the test scaffolding (FakeSession)**

`crates/paavo-runner/tests/common/mod.rs`:
```rust
//! Test scaffolding shared across paavo-runner integration tests.

use crossbeam_channel::{bounded, Sender};
use paavo_probe::{Event, ProbeError, ProbeSession, Result as ProbeResult};
use paavo_proto::{LogFrame, LogLevel};
use std::time::Duration;

/// Programmable fake probe session.
///
/// Caller pushes scripted events with `script_event`; the session returns
/// them in order via `next_event`. When the script is exhausted, the session
/// returns `Ok(None)` (no event) on each call, simulating an idle probe.
///
/// `next_event` blocks up to `timeout_ms`. Test cases that want to provoke
/// the inactivity watchdog use a small inactivity timeout (e.g. 200 ms) and
/// then leave the script empty.
pub struct FakeSession {
    rx: crossbeam_channel::Receiver<Event>,
}

/// Caller-side handle for scripting events onto a FakeSession.
pub struct FakeScript {
    tx: Sender<Event>,
}

impl FakeScript {
    /// Push a single log frame.
    pub fn log(&self, level: LogLevel, msg: &str) {
        self.tx
            .send(Event::LogFrame(LogFrame {
                seq: 0, // worker doesn't assign seq today; not under test
                ts_us: 0,
                level,
                target: None,
                message: msg.into(),
            }))
            .unwrap();
    }

    /// Push a Bkpt event.
    pub fn bkpt(&self) {
        self.tx.send(Event::Bkpt).unwrap();
    }

    /// Push a Panic event.
    pub fn panic(&self, msg: &str) {
        self.tx
            .send(Event::Panic {
                message: msg.into(),
            })
            .unwrap();
    }

    /// Push a Disconnect event.
    pub fn disconnect(&self) {
        self.tx.send(Event::Disconnect).unwrap();
    }
}

/// Build a FakeSession + its scripting handle.
pub fn fake_session() -> (FakeSession, FakeScript) {
    let (tx, rx) = bounded(64);
    (FakeSession { rx }, FakeScript { tx })
}

impl ProbeSession for FakeSession {
    fn next_event(&mut self, timeout_ms: u32) -> ProbeResult<Option<Event>> {
        match self
            .rx
            .recv_timeout(Duration::from_millis(timeout_ms as u64))
        {
            Ok(ev) => Ok(Some(ev)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(None),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                // Script handle dropped without disconnect event — treat as
                // idle (no event) so the watchdog can fire if relevant.
                Ok(None)
            }
        }
    }
}

/// Convenience: wrap a constructed FakeSession in a `Box<dyn ProbeSession>`
/// for `run_job`. Use it inside the `make_session` closure.
pub fn into_box(s: FakeSession) -> ProbeResult<Box<dyn ProbeSession>> {
    Ok(Box::new(s) as Box<dyn ProbeSession>)
}

/// Used by one test that wants `make_session` to fail.
pub fn fail_to_connect() -> ProbeResult<Box<dyn ProbeSession>> {
    Err(ProbeError::ProbeRs("fake: connect failed".into()))
}
```

- [ ] **Step 4: Write the pass-path test**

`crates/paavo-runner/tests/pass_path.rs`:
```rust
mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{JobId, JobOutcome, LogLevel};
use paavo_runner::{run_job, JobInputs, JobOutputs};

#[test]
fn test_ok_then_bkpt_produces_passed() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    script.log(LogLevel::Info, "boot complete");
    script.log(LogLevel::Info, "Test OK");
    script.bkpt();

    let outcome = handle.join();
    assert_eq!(outcome, JobOutcome::Passed);
    drop(cancel_tx);

    // Two frames should have been forwarded; the bkpt is consumed silently.
    let frames: Vec<_> = log_rx.try_iter().collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1].message, "Test OK");
}

#[test]
fn panic_event_produces_failed_testerr() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    script.panic("assertion failed: x == 0");
    let outcome = handle.join();
    match outcome {
        JobOutcome::Failed(paavo_proto::TerminalOutcome::TestErr { message }) => {
            assert!(message.contains("assertion failed"), "{message}");
        }
        other => panic!("expected Failed(TestErr), got {other:?}"),
    }
    drop(cancel_tx);
}

#[test]
fn disconnect_event_produces_failed_infraerr() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    script.disconnect();
    let outcome = handle.join();
    match outcome {
        JobOutcome::Failed(paavo_proto::TerminalOutcome::InfraErr { stage, .. }) => {
            assert_eq!(stage, "probe_disconnect");
        }
        other => panic!("expected Failed(InfraErr), got {other:?}"),
    }
    drop(cancel_tx);
}

#[test]
fn connect_failure_produces_infra_err() {
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        common::fail_to_connect,
    );

    let outcome = handle.join();
    match outcome {
        JobOutcome::Failed(paavo_proto::TerminalOutcome::InfraErr { stage, .. }) => {
            assert_eq!(stage, "probe_attach");
        }
        other => panic!("expected Failed(InfraErr probe_attach), got {other:?}"),
    }
    drop(cancel_tx);
}
```

- [ ] **Step 5: Write the inactivity watchdog test**

`crates/paavo-runner/tests/inactivity_watchdog.rs`:
```rust
mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{JobId, JobOutcome, TimeoutReason};
use paavo_runner::{run_job, JobInputs, JobOutputs};

#[test]
fn inactivity_timeout_fires_when_no_events_arrive() {
    let (sess, _script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 200,
            hard_max_ms: 30_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    let outcome = handle.join();
    assert_eq!(
        outcome,
        JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms: match outcome.clone() {
                JobOutcome::TimedOut { elapsed_ms, .. } => elapsed_ms,
                _ => unreachable!(),
            },
        },
        "expected Inactivity timeout, got {outcome:?}",
    );
    if let JobOutcome::TimedOut { elapsed_ms, .. } = outcome {
        assert!(elapsed_ms >= 200, "elapsed_ms {elapsed_ms} should be >= 200");
        assert!(elapsed_ms < 2_000, "elapsed_ms {elapsed_ms} should be < 2000");
    }
    drop(cancel_tx);
}
```

- [ ] **Step 6: Write the hard-max watchdog test**

`crates/paavo-runner/tests/hard_max_watchdog.rs`:
```rust
mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{JobId, JobOutcome, LogLevel, TimeoutReason};
use paavo_runner::{run_job, JobInputs, JobOutputs};
use std::thread;
use std::time::Duration;

#[test]
fn hard_max_fires_even_when_frames_keep_arriving() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 30_000, // never trips
            hard_max_ms: 400,              // ~half a second
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    // Push a frame every 50 ms for up to 2 s so inactivity can't fire.
    let _producer = thread::spawn(move || {
        for i in 0..40 {
            script.log(LogLevel::Info, &format!("tick {i}"));
            thread::sleep(Duration::from_millis(50));
        }
    });

    let outcome = handle.join();
    drop(cancel_tx);

    match outcome {
        JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms,
        } => {
            assert!(elapsed_ms >= 400, "elapsed_ms {elapsed_ms} >= 400");
            assert!(elapsed_ms < 3_000, "elapsed_ms {elapsed_ms} < 3000");
        }
        other => panic!("expected TimedOut(HardMax), got {other:?}"),
    }
}
```

- [ ] **Step 7: Write the cancel test**

`crates/paavo-runner/tests/cancel_path.rs`:
```rust
mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{AbortReason, JobId, JobOutcome};
use paavo_runner::{run_job, JobInputs, JobOutputs, RunCommand};
use std::thread;
use std::time::Duration;

#[test]
fn user_cancel_produces_aborted_user() {
    let (sess, _script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 30_000,
            hard_max_ms: 30_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    thread::sleep(Duration::from_millis(150));
    cancel_tx.send(RunCommand::Cancel).unwrap();

    let outcome = handle.join();
    assert_eq!(
        outcome,
        JobOutcome::Aborted {
            by: AbortReason::User
        }
    );
}

#[test]
fn daemon_shutdown_produces_aborted_daemon_shutdown() {
    let (sess, _script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 30_000,
            hard_max_ms: 30_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    thread::sleep(Duration::from_millis(150));
    cancel_tx.send(RunCommand::DaemonShutdown).unwrap();

    let outcome = handle.join();
    assert_eq!(
        outcome,
        JobOutcome::Aborted {
            by: AbortReason::DaemonShutdown
        }
    );
}
```

- [ ] **Step 8: Run all paavo-runner tests**

Run: `cargo test -p paavo-runner`
Expected: all 9 tests across 4 test files passing. The inactivity_watchdog test takes ~200-500 ms, hard_max ~400-600 ms, cancel ~150 ms each.

If a flaky timing assertion fails, widen the bound (the `< 2_000`, `< 3_000`, etc.) — these are coarse so they should not fail on a loaded CI runner, but tune up if needed.

- [ ] **Step 9: Clippy**

Run: `cargo clippy -p paavo-runner --all-targets -- -D warnings`
Expected: pass.

- [ ] **Step 10: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-runner
git -C D:\workspace\paavo commit -m "feat(runner): BoardWorker + Watchdog + 4 deterministic outcome paths"
```

---

### Milestone 2 exit criteria

- [ ] `paavo-probe::sections::parse_meta_sections` correctly handles present/absent/malformed for the three sections
- [ ] `paavo-runner::run_job` produces every variant of `JobOutcome` in deterministic tests
- [ ] No probe-rs dynamic linkage required to run the workspace tests
- [ ] `cargo test --workspace` still green

---

## Milestone 3 — Build + core

Goal: `paavo-build` (sandbox + `cargo build` + ELF discovery + blake3-keyed cache reuse) and `paavo-core` (scheduler, board fleet, BoardWorker pool, quarantine). `paavo-core` exercises a fake `Runner` trait so its tests don't touch real probes.

### Task 3.1: paavo-build — tar unpack + ELF discovery + cache reuse

Spec coverage: §8.1, §8.2.

**Files:**
- Create: `crates/paavo-build/src/lib.rs` (replace skeleton)
- Create: `crates/paavo-build/src/error.rs`
- Create: `crates/paavo-build/src/tar.rs`
- Create: `crates/paavo-build/src/build.rs`
- Create: `crates/paavo-build/src/elf.rs`
- Test: `crates/paavo-build/tests/tar_unpack.rs`
- Test: `crates/paavo-build/tests/elf_discovery.rs`

> Note: build-cache helpers live in **`paavo-core`** (Task 3.2.e), not in `paavo-build`. That keeps `paavo-build` honest to spec §4.1 ("paavo-build depends only on paavo-proto") — only the glue layer that composes builds with persistence needs `paavo-db`.

#### 3.1.a: Tar unpack + blake3

- [ ] **Step 1: Write the failing tar test**

`crates/paavo-build/tests/tar_unpack.rs`:
```rust
use paavo_build::tar::{blake3_hex, unpack_into};
use paavo_build::BuildError;
use tempfile::tempdir;

fn build_sample_tar() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        let body = b"fn main() { println!(\"hi\"); }\n";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("hello/src/main.rs").unwrap();
        hdr.set_size(body.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder.append(&hdr, &body[..]).unwrap();

        let manifest = b"[package]\nname = \"hello\"\nversion = \"0.1.0\"\n";
        let mut hdr2 = tar::Header::new_gnu();
        hdr2.set_path("hello/Cargo.toml").unwrap();
        hdr2.set_size(manifest.len() as u64);
        hdr2.set_mode(0o644);
        hdr2.set_cksum();
        builder.append(&hdr2, &manifest[..]).unwrap();

        builder.finish().unwrap();
    }
    buf
}

#[test]
fn unpack_extracts_all_entries() {
    let dir = tempdir().unwrap();
    let tar = build_sample_tar();
    let dst = unpack_into(&tar, dir.path()).unwrap();
    assert!(dst.join("hello/src/main.rs").is_file());
    assert!(dst.join("hello/Cargo.toml").is_file());
}

#[test]
fn unpack_rejects_path_escape() {
    let dir = tempdir().unwrap();
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        let mut hdr = tar::Header::new_gnu();
        // tar 0.4's `Header::set_path` validates and refuses any `..`
        // component, so we cannot construct a malicious header through it.
        // Write the raw bytes into the GNU name field directly — this is
        // exactly the threat model that `validate_path` defends against
        // (a hostile archive crafted outside the standard Builder API).
        let name = b"../escape.rs";
        hdr.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name);
        hdr.set_size(0);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder.append(&hdr, &[][..]).unwrap();
        builder.finish().unwrap();
    }
    let err = unpack_into(&buf, dir.path()).unwrap_err();
    assert!(
        matches!(err, BuildError::PathEscape { reason: "parent-dir", .. }),
        "expected PathEscape{{reason: 'parent-dir'}}, got: {err:?}"
    );
}

#[test]
fn blake3_hex_is_deterministic() {
    let a = build_sample_tar();
    let b = build_sample_tar();
    let ha = blake3_hex(&a);
    let hb = blake3_hex(&b);
    assert_eq!(ha, hb);
    assert_eq!(ha.len(), 64); // hex blake3
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-build --test tar_unpack`
Expected: FAIL.

- [ ] **Step 3: Implement error + tar modules**

`crates/paavo-build/src/error.rs`:
```rust
//! Errors returned by paavo-build operations.

use thiserror::Error;

/// Errors from tar unpack, cargo invocation, and ELF discovery.
#[derive(Debug, Error)]
pub enum BuildError {
    /// I/O failure. The `tar` crate surfaces archive-corruption errors as
    /// `std::io::Error`, so tar-stream errors flow through here too.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// An entry inside the archive had a path that would escape the
    /// destination directory (absolute path or contained `..`).
    #[error("path-escape: entry {path:?} would escape sandbox ({reason})")]
    PathEscape {
        /// The offending entry path as read from the archive.
        path: std::path::PathBuf,
        /// What we caught: "absolute" or "parent-dir".
        reason: &'static str,
    },
    /// The artifact-dir hint in `[package.metadata.embassy].build.artifact-dir`
    /// pointed to a path that doesn't exist under the crate dir. Always a
    /// manifest authoring error; distinct from "I scanned and found no
    /// ELF" (`NoElf`).
    #[error("hint-dir does not exist: {dir}")]
    HintDirMissing {
        /// Fully-joined `crate_dir + artifact_dir` that we expected.
        dir: String,
    },
    /// Manifest parse error.
    #[error("manifest: {0}")]
    Manifest(String),
    /// `cargo build` failed; stderr captured.
    #[error("cargo build failed (exit {exit:?}); stderr:\n{stderr}")]
    Cargo {
        /// Exit code from `std::process::ExitStatus::code()`. `None` means
        /// the process was terminated by a signal (Unix) and has no exit
        /// code.
        exit: Option<i32>,
        /// Captured stderr (tail).
        stderr: String,
    },
    /// `cargo build` succeeded but no ELF could be located.
    #[error("no ELF artifact found in {dir}")]
    NoElf {
        /// Directory that was scanned.
        dir: String,
    },
}

/// Result alias.
pub type Result<T, E = BuildError> = std::result::Result<T, E>;
```

`crates/paavo-build/src/tar.rs`:
```rust
//! Tar unpacking with path-escape rejection, plus blake3 hashing of the
//! raw tar bytes (used as the build-cache key).

use crate::error::{BuildError, Result};
use std::path::{Component, Path, PathBuf};

/// Stable hex digest of `bytes` (typically the raw tar archive). Caller
/// uses this as the `paavo_db::build_cache` key.
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Unpack `bytes` into `dst`, returning the directory we unpacked into.
///
/// Rejects entries whose path escapes `dst` via `..` or absolute paths.
///
/// **Non-atomic.** On error, `dst` may have been created and may contain
/// partially-extracted entries from before the rejection. Callers that
/// need cleanup should pass a `tempfile::TempDir` (which deletes on drop)
/// or remove `dst` after a failed call.
pub fn unpack_into(bytes: &[u8], dst: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dst)?;
    let mut archive = tar::Archive::new(bytes);
    for entry in archive.entries()? {
        let mut e = entry?;
        let path = e.path()?.into_owned();
        validate_path(&path)?;
        e.unpack_in(dst)?;
    }
    Ok(dst.to_path_buf())
}

fn validate_path(p: &Path) -> Result<()> {
    if p.is_absolute() {
        return Err(BuildError::PathEscape {
            path: p.to_path_buf(),
            reason: "absolute",
        });
    }
    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(BuildError::PathEscape {
                path: p.to_path_buf(),
                reason: "parent-dir",
            });
        }
    }
    Ok(())
}
```

`crates/paavo-build/src/lib.rs` (replace skeleton):
```rust
//! Tar unpack, `cargo build`, and ELF discovery for paavo. Build-cache
//! plumbing (paired with `paavo-db::BuildCacheEntry`) lives in
//! `paavo-core::build_cache` — this crate stays free of any DB dep so spec
//! §4.1's boundary ("paavo-build depends only on paavo-proto") holds.
//!
//! ```
//! assert_eq!(paavo_build::CRATE_NAME, "paavo-build");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-build";

mod build;
mod elf;
mod error;
pub mod tar;

pub use build::{build_release, BuildPlan, BuildResult};
pub use elf::{discover_elf, ManifestArtifactHint};
pub use error::{BuildError, Result};
```

`crates/paavo-build/src/build.rs` (stub for now — full impl in 3.1.c):
```rust
//! `cargo build --release` invocation.

use crate::error::Result;
use std::path::PathBuf;

/// Build plan derived from a `JobSpec` and a sandbox directory.
#[derive(Debug, Clone)]
pub struct BuildPlan {
    /// Sandbox dir containing the unpacked crate.
    pub crate_dir: PathBuf,
    /// `CARGO_TARGET_DIR` to share across jobs for incremental reuse.
    pub target_dir: PathBuf,
    /// Optional `cargo update -p ...` packages to refresh before building
    /// (used by soak-test corpora that track `embassy-rs/embassy` main).
    pub cargo_update_packages: Vec<String>,
}

/// What `build_release` returns.
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Path to the discovered ELF.
    pub elf_path: PathBuf,
    /// Size of the ELF on disk, bytes.
    pub elf_size_bytes: u64,
    /// Captured stdout/stderr (useful for surfacing build warnings).
    pub stderr_tail: String,
}

/// Invoke `cargo build --release` in `plan.crate_dir`, then discover the ELF.
///
/// Implemented fully in Task 3.1.c so the test in 3.1.a doesn't depend on
/// `cargo` being on PATH.
pub fn build_release(_plan: &BuildPlan) -> Result<BuildResult> {
    Err(crate::error::BuildError::Cargo {
        exit: None,
        stderr: "build_release is wired in Task 3.1.c".into(),
    })
}
```

`crates/paavo-build/src/elf.rs` (stub for now — filled in 3.1.b):
```rust
//! ELF discovery from `[package.metadata.embassy]` or directory scan.

use crate::error::{BuildError, Result};
use std::path::{Path, PathBuf};

/// Optional manifest hint: `[package.metadata.embassy].build.artifact-dir`.
#[derive(Debug, Default, Clone)]
pub struct ManifestArtifactHint {
    /// Sub-path under the crate dir that is known to contain the ELF.
    pub artifact_dir: Option<PathBuf>,
}

/// Locate the ELF for a built crate. See Task 3.1.b for the implementation.
pub fn discover_elf(
    _crate_dir: &Path,
    _target_dir: &Path,
    _hint: &ManifestArtifactHint,
) -> Result<PathBuf> {
    Err(BuildError::NoElf {
        dir: "discover_elf is wired in Task 3.1.b".into(),
    })
}
```

- [ ] **Step 4: Run the tar test**

Run: `cargo test -p paavo-build --test tar_unpack`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-build
git -C D:\workspace\paavo commit -m "feat(build): tar unpack + blake3 keying + crate module skeleton"
```

---

#### 3.1.b: ELF discovery

- [ ] **Step 1: Write the failing ELF discovery test**

`crates/paavo-build/tests/elf_discovery.rs`:
```rust
use paavo_build::{discover_elf, ManifestArtifactHint};
use std::fs;
use tempfile::tempdir;

fn make_elf(path: &std::path::Path) {
    // Minimal valid-ish ELF prefix (magic + class + endian). The discovery
    // logic only verifies the magic.
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 2, 1, 1];
    bytes.extend(std::iter::repeat_n(0u8, 57)); // pad to typical Ehdr size
    fs::write(path, &bytes).unwrap();
}

#[test]
fn picks_up_hinted_artifact_dir() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let artifact_dir = crate_dir.join("artifacts");
    fs::create_dir_all(&artifact_dir).unwrap();
    make_elf(&artifact_dir.join("hello.elf"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint {
            artifact_dir: Some("artifacts".into()),
        },
    )
    .unwrap();
    assert_eq!(elf, artifact_dir.join("hello.elf"));
}

#[test]
fn scans_target_release_when_no_hint() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    fs::create_dir_all(&release).unwrap();
    make_elf(&release.join("hello"));
    make_elf(&release.join("hello.elf"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap();
    // Prefer the explicit .elf extension when both are present.
    assert_eq!(elf, release.join("hello.elf"));
}

#[test]
fn scans_host_target_release_dir() {
    // Host builds (no cross-compile) write directly to target/release/,
    // not target/<triple>/release/. Task 3.1.c's `build_release` test
    // depends on this path being recognized.
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let release = crate_dir.join("target").join("release");
    fs::create_dir_all(&release).unwrap();
    make_elf(&release.join("hello"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap();
    assert_eq!(elf, release.join("hello"));
}

#[test]
fn errors_when_no_elf_present() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    fs::create_dir_all(&release).unwrap();

    let err = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("no ELF"), "{msg}");
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-build --test elf_discovery`
Expected: FAIL — current `discover_elf` is a stub.

- [ ] **Step 3: Implement discovery**

Replace `crates/paavo-build/src/elf.rs`:
```rust
//! ELF discovery from `[package.metadata.embassy].build.artifact-dir` or
//! a fallback `target/<triple>/release/` scan.

use crate::error::{BuildError, Result};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Optional manifest hint: `[package.metadata.embassy].build.artifact-dir`.
#[derive(Debug, Default, Clone)]
pub struct ManifestArtifactHint {
    /// Sub-path relative to the crate dir that is known to contain the ELF.
    pub artifact_dir: Option<PathBuf>,
}

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// Locate the ELF for a built crate.
///
/// Strategy:
/// 1. If `hint.artifact_dir` is set: scan that directory (recursively) for
///    an ELF magic file. Prefer files ending in `.elf`.
/// 2. Otherwise scan `target_dir/<triple>/release/` (one level of `<triple>`
///    expansion). Prefer files ending in `.elf`.
pub fn discover_elf(
    crate_dir: &Path,
    target_dir: &Path,
    hint: &ManifestArtifactHint,
) -> Result<PathBuf> {
    let scan_root = if let Some(artifact) = &hint.artifact_dir {
        let joined = crate_dir.join(artifact);
        if !joined.is_dir() {
            return Err(BuildError::HintDirMissing {
                dir: joined.display().to_string(),
            });
        }
        joined
    } else {
        let release_glob = scan_release_dirs(target_dir);
        match release_glob {
            Some(p) => p,
            None => {
                return Err(BuildError::NoElf {
                    dir: target_dir.display().to_string(),
                })
            }
        }
    };
    pick_elf(&scan_root)
}

fn scan_release_dirs(target_dir: &Path) -> Option<PathBuf> {
    // Host builds: target/release/. Cross builds: target/<triple>/release/.
    // Prefer the bare host path when both exist.
    let direct = target_dir.join("release");
    if direct.is_dir() {
        return Some(direct);
    }
    // Collect + sort by file_name so the chosen triple-dir is
    // deterministic across machines/filesystems. `read_dir` order is
    // OS- and filesystem-dependent, which would otherwise make CI fragile.
    let mut entries: Vec<_> = std::fs::read_dir(target_dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for ent in entries {
        let release = ent.path().join("release");
        if release.is_dir() {
            return Some(release);
        }
    }
    None
}

fn pick_elf(root: &Path) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(root)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .flatten()
    {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if is_elf(p) {
            candidates.push(p.to_path_buf());
        }
    }
    // First pass: stable lexicographic order so within-class ordering is
    // reproducible across machines/filesystems (WalkDir traversal order is
    // OS-dependent).
    candidates.sort();
    // Second pass: stable sort by `.elf`-extension; .elf files sort last
    // so `pop()` returns them first.
    candidates.sort_by(|a, b| {
        let ax = a.extension().and_then(|s| s.to_str()) == Some("elf");
        let bx = b.extension().and_then(|s| s.to_str()) == Some("elf");
        ax.cmp(&bx)
    });
    candidates.pop().ok_or_else(|| BuildError::NoElf {
        dir: root.display().to_string(),
    })
}

fn is_elf(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    use std::io::Read;
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    magic == ELF_MAGIC
}
```

- [ ] **Step 4: Run the discovery test**

Run: `cargo test -p paavo-build --test elf_discovery`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-build/src/elf.rs crates/paavo-build/tests/elf_discovery.rs
git -C D:\workspace\paavo commit -m "feat(build): ELF discovery via manifest hint or target/<triple>/release scan"
```

---

#### 3.1.c: `cargo build` invocation

- [ ] **Step 1: Write the failing build test**

`crates/paavo-build/tests/build_invocation.rs`:
```rust
//! Drives `cargo build --release` against a tiny host crate fixture. The
//! point of this test is to exercise the invocation path; it does *not*
//! cross-compile to an embedded target (CI does not have `thumbv*` linkers
//! installed). The fixture is a `cdylib`-less binary so the discovery path
//! picks up the host triple's release dir.

use paavo_build::{build_release, BuildPlan};
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

fn write_fixture(root: &std::path::Path) {
    let crate_dir = root.join("hello");
    fs::create_dir_all(crate_dir.join("src")).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        r#"[package]
name = "hello"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "hello"
path = "src/main.rs"
"#,
    )
    .unwrap();
    fs::write(
        crate_dir.join("src").join("main.rs"),
        r#"fn main() { println!("hi"); }"#,
    )
    .unwrap();
}

#[test]
fn build_release_produces_elf_for_host_target() {
    if std::env::var_os("CARGO").is_none() {
        eprintln!("skipping: CARGO not set in env");
        return;
    }
    let dir = tempdir().unwrap();
    write_fixture(dir.path());
    let plan = BuildPlan {
        crate_dir: dir.path().join("hello"),
        target_dir: dir.path().join("cargo-target"),
        cargo_update_packages: vec![],
    };
    let res = build_release(&plan).unwrap();
    let elf: PathBuf = res.elf_path;
    assert!(elf.is_file(), "expected ELF at {elf:?}");
    assert!(res.elf_size_bytes > 0);
}

#[test]
fn build_release_captures_stderr_on_failure() {
    if std::env::var_os("CARGO").is_none() {
        eprintln!("skipping: CARGO not set in env");
        return;
    }
    let dir = tempdir().unwrap();
    let crate_dir = dir.path().join("broken");
    fs::create_dir_all(crate_dir.join("src")).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        r#"[package]
name = "broken"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        crate_dir.join("src").join("main.rs"),
        r#"fn main() { compile_error!("kaboom"); }"#,
    )
    .unwrap();
    let plan = BuildPlan {
        crate_dir,
        target_dir: dir.path().join("cargo-target"),
        cargo_update_packages: vec![],
    };
    let err = build_release(&plan).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("kaboom") || msg.contains("compile_error"), "{msg}");
}
```

- [ ] **Step 2: Implement build_release**

Replace `crates/paavo-build/src/build.rs`:
```rust
//! `cargo build --release` invocation, with stderr capture and ELF discovery
//! handoff.

use crate::elf::{discover_elf, ManifestArtifactHint};
use crate::error::{BuildError, Result};
use std::path::PathBuf;
use std::process::Command;

/// Build plan derived from a `JobSpec` and a sandbox directory.
#[derive(Debug, Clone)]
pub struct BuildPlan {
    /// Sandbox dir containing the unpacked crate.
    pub crate_dir: PathBuf,
    /// `CARGO_TARGET_DIR` to share across jobs for incremental reuse.
    pub target_dir: PathBuf,
    /// Optional `cargo update -p ...` packages to refresh before building.
    pub cargo_update_packages: Vec<String>,
}

/// What `build_release` returns.
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Path to the discovered ELF.
    pub elf_path: PathBuf,
    /// Size of the ELF on disk, bytes.
    pub elf_size_bytes: u64,
    /// Captured stderr tail (last 8 KB).
    pub stderr_tail: String,
}

/// Invoke `cargo build --release` in `plan.crate_dir`, then discover the ELF.
pub fn build_release(plan: &BuildPlan) -> Result<BuildResult> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    for pkg in &plan.cargo_update_packages {
        // Mirror the build path so soak operators see the real cargo
        // diagnostic on `cargo update` failure, not a hard-coded sentinel.
        run_cargo(&cargo, &["update", "-p", pkg], plan)?;
    }

    let stderr_tail = run_cargo(&cargo, &["build", "--release"], plan)?;

    let hint = ManifestArtifactHint::default();
    let elf_path = discover_elf(&plan.crate_dir, &plan.target_dir, &hint)?;
    let elf_size_bytes = std::fs::metadata(&elf_path)?.len();
    Ok(BuildResult {
        elf_path,
        elf_size_bytes,
        stderr_tail,
    })
}

/// Run a cargo subcommand with stderr capture. Returns the (success-path)
/// stderr tail (≤ 8 KiB) or a `BuildError::Cargo` carrying the failure exit
/// code and stderr tail. All cargo invocations should go through this so
/// the `cargo update` and `cargo build` paths stay symmetric in error
/// surfacing.
fn run_cargo(
    cargo: &std::ffi::OsStr,
    args: &[&str],
    plan: &BuildPlan,
) -> Result<String> {
    let output = Command::new(cargo)
        .args(args)
        .current_dir(&plan.crate_dir)
        .env("CARGO_TARGET_DIR", &plan.target_dir)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr_tail = tail(&stderr, 8 * 1024);
    if !output.status.success() {
        return Err(BuildError::Cargo {
            exit: output.status.code(),
            stderr: stderr_tail,
        });
    }
    Ok(stderr_tail)
}

fn tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let start = s.len() - max_bytes;
    let mut idx = start;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    s[idx..].to_string()
}
```

> Note: `scan_release_dirs` in `elf.rs` already handles both `target/release/` (host builds) and `target/<triple>/release/` (cross builds) — see 3.1.b's implementation. No further changes to `elf.rs` are needed in this sub-task.

- [ ] **Step 3: Run the invocation test**

Run: `cargo test -p paavo-build --test build_invocation`
Expected: 2 passed (the success case takes several seconds; cargo downloads no deps for this trivial fixture).

- [ ] **Step 4: Re-run the discovery test to make sure the elf.rs edit didn't break anything**

Run: `cargo test -p paavo-build --test elf_discovery`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-build
git -C D:\workspace\paavo commit -m "feat(build): cargo build --release invocation with stderr capture"
```

---

#### 3.1.d: Build-cache helpers — moved to paavo-core

The cache lookup/store helpers (originally planned here) live in `paavo-core::build_cache` instead — see **Task 3.2.e**. That keeps `paavo-build`'s dep list free of `paavo-db` and `rusqlite`, honoring the spec §4.1 boundary "paavo-build depends only on paavo-proto."

No work in this sub-task; proceed to Task 3.2.

---

### Task 3.2: paavo-core — scheduler, board fleet, quarantine

Spec coverage: §5.3 (priority + LRU), §5.4 (cancel by state), §5.5 (selector rejection at enqueue), §6 (timeouts wired through), §13 (`quarantine.consecutive_infra_failures`).

`paavo-core` does **not** spin up real threads or talk to probe-rs in unit tests — it goes through a `Runner` trait whose test impl is deterministic.

**Files:**
- Create: `crates/paavo-core/src/lib.rs` (replace skeleton)
- Create: `crates/paavo-core/src/error.rs`
- Create: `crates/paavo-core/src/runner.rs`
- Create: `crates/paavo-core/src/selector.rs`
- Create: `crates/paavo-core/src/quarantine.rs`
- Create: `crates/paavo-core/src/scheduler.rs`
- Create: `crates/paavo-core/src/enqueue.rs`
- Test: `crates/paavo-core/tests/enqueue.rs`
- Test: `crates/paavo-core/tests/scheduler_priority.rs`
- Test: `crates/paavo-core/tests/scheduler_lru.rs`
- Test: `crates/paavo-core/tests/scheduler_starvation.rs`
- Test: `crates/paavo-core/tests/quarantine.rs`
- Test: `crates/paavo-core/tests/cancel.rs`
- Test: `crates/paavo-core/tests/common/mod.rs`

#### 3.2.a: lib skeleton + `Runner` trait + `Core` handle

- [ ] **Step 1: Define the public surface in lib.rs**

`crates/paavo-core/src/lib.rs`:
```rust
//! Scheduler, board fleet, and quarantine policy for paavo. No HTTP — the
//! `paavod` crate owns axum and wraps a `Core` handle.
//!
//! ```
//! assert_eq!(paavo_core::CRATE_NAME, "paavo-core");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-core";

mod build_cache;
mod enqueue;
mod error;
mod quarantine;
mod runner;
mod scheduler;
mod selector;

pub use build_cache::{cache_lookup, cache_store, CacheLookup};
pub use enqueue::{enqueue_job, EnqueueRequest};
pub use error::{CoreError, Result};
pub use quarantine::{apply_outcome_to_board, QuarantinePolicy};
pub use runner::{RunOutcome, Runner};
pub use scheduler::{pick_next, SchedulerConfig, ScheduledJob};
pub use selector::selector_matches_any;
```

`crates/paavo-core/src/error.rs`:
```rust
//! Errors returned by paavo-core operations.

use thiserror::Error;

/// All errors surfaced by paavo-core public API.
#[derive(Debug, Error)]
pub enum CoreError {
    /// paavo-db error.
    #[error("db: {0}")]
    Db(#[from] paavo_db::DbError),
    /// paavo-build error.
    #[error("build: {0}")]
    Build(#[from] paavo_build::BuildError),
    /// Selector matched no possible board in the inventory (per spec §5.5,
    /// rejected at enqueue time, not silently queued).
    #[error("selector matches no board in inventory: {0:?}")]
    SelectorNeverMatches(paavo_proto::BoardSelector),
    /// Requested hard-max exceeds the daemon ceiling.
    #[error("requested hard_max_ms {requested} exceeds daemon ceiling {ceiling}")]
    OverCeiling {
        /// What was asked.
        requested: u64,
        /// Daemon-configured ceiling.
        ceiling: u64,
    },
    /// Cancel was issued in a state where it doesn't apply.
    #[error("cannot cancel job in state {state:?}")]
    NotCancellable {
        /// State the job was in.
        state: paavo_proto::JobState,
    },
}

/// Result alias.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;
```

`crates/paavo-core/src/runner.rs`:
```rust
//! Abstraction over `paavo-runner::run_job`. Production code wires this to
//! the real BoardWorker. Tests substitute a deterministic in-process impl.

use paavo_proto::JobOutcome;

/// What a runner reports back when a job finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// Terminal job outcome.
    pub outcome: JobOutcome,
    /// True if the BoardWorker successfully released the probe before the
    /// release-grace expired. Per spec §5.2, when this is `false` and the
    /// outcome is `TimedOut{Inactivity}`, the board's infra-failure counter
    /// must be bumped.
    pub probe_released_cleanly: bool,
}

/// Production code passes `Box<dyn Runner>`; tests pass `FakeRunner`.
pub trait Runner: Send + Sync {
    /// Run a job on `board_id` and block until terminal. The job has
    /// already had its row transitioned to `Building` and its tar/ELF
    /// resolved by the caller.
    fn run(&self, job_id: paavo_proto::JobId, board_id: &str) -> RunOutcome;
}
```

`crates/paavo-core/src/selector.rs`:
```rust
//! Selector-matches-any helper used at enqueue time.

use paavo_proto::{BoardSelector, BoardSpec};

/// Returns `true` if at least one board in `inventory` satisfies `sel`.
/// Ignores health (per spec §5.5: rejection is for impossible selectors,
/// not for transient un-availability).
pub fn selector_matches_any(sel: &BoardSelector, inventory: &[BoardSpec]) -> bool {
    inventory.iter().any(|b| sel.matches(b))
}
```

`crates/paavo-core/src/quarantine.rs`:
```rust
//! Quarantine policy. Reacts to terminal outcomes.

use paavo_proto::{JobOutcome, TerminalOutcome};
use rusqlite::Connection;

/// Policy parameters (from `paavo.toml::[quarantine]`).
#[derive(Debug, Clone, Copy)]
pub struct QuarantinePolicy {
    /// Threshold: when `consecutive_infra_failures` reaches this, the
    /// board is auto-quarantined with reason
    /// `"auto: N consecutive infra failures"`.
    pub consecutive_infra_failures: u32,
}

impl Default for QuarantinePolicy {
    fn default() -> Self {
        Self {
            consecutive_infra_failures: 3,
        }
    }
}

/// Apply an outcome to a board's quarantine state. Caller must have already
/// called `JobRow::finalize`. Returns `Ok(true)` if the board was just
/// auto-quarantined.
pub fn apply_outcome_to_board(
    conn: &Connection,
    board_id: &str,
    outcome: &JobOutcome,
    probe_released_cleanly: bool,
    policy: QuarantinePolicy,
) -> paavo_db::Result<bool> {
    let counts_toward_infra = match outcome {
        JobOutcome::Failed(TerminalOutcome::InfraErr { .. }) => true,
        JobOutcome::TimedOut {
            reason: paavo_proto::TimeoutReason::Inactivity,
            ..
        } => !probe_released_cleanly,
        _ => false,
    };
    if !counts_toward_infra {
        paavo_db::BoardRow::reset_infra_failures(conn, board_id)?;
        return Ok(false);
    }
    paavo_db::BoardRow::bump_infra_failure(conn, board_id)?;
    let row = paavo_db::BoardRow::get(conn, board_id)?;
    if row.consecutive_infra_failures >= policy.consecutive_infra_failures {
        paavo_db::BoardRow::quarantine(
            conn,
            board_id,
            &format!(
                "auto: {n} consecutive infra failures",
                n = row.consecutive_infra_failures
            ),
        )?;
        return Ok(true);
    }
    Ok(false)
}
```

`crates/paavo-core/src/scheduler.rs`:
```rust
//! Scheduler: pick the highest-priority eligible job + LRU healthy board.

use chrono::Utc;
use paavo_proto::Priority;
use rusqlite::Connection;

/// Scheduler configuration (subset of `paavo.toml`).
#[derive(Debug, Clone, Copy)]
pub struct SchedulerConfig {
    /// Scheduled jobs older than this get promoted to Interactive priority
    /// (spec §5.3 starvation rule).
    pub starvation_threshold_ms: i64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            starvation_threshold_ms: 6 * 60 * 60 * 1_000, // 6 h
        }
    }
}

/// A successful pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledJob {
    /// Job to dispatch.
    pub job: paavo_db::JobRow,
    /// Board to dispatch onto.
    pub board: paavo_db::BoardRow,
}

/// Look at all `Submitted` jobs in priority/starvation order; for each, look
/// at all eligible healthy boards in LRU order (`last_used_at ASC NULLS
/// FIRST`); return the first matching pair, or `Ok(None)` if nothing can
/// dispatch right now.
///
/// Pure read — caller is responsible for the subsequent
/// `JobRow::transition_to_building` + `BoardRow::touch_last_used`.
pub fn pick_next(
    conn: &Connection,
    config: SchedulerConfig,
) -> paavo_db::Result<Option<ScheduledJob>> {
    let now_ms = Utc::now().timestamp_millis();
    let jobs = paavo_db::JobRow::list_submitted(conn, 200)?;
    // Promote scheduled jobs that have starved.
    let mut promoted: Vec<paavo_db::JobRow> = jobs
        .into_iter()
        .map(|mut j| {
            if j.priority == Priority::Scheduled
                && now_ms - j.submitted_at >= config.starvation_threshold_ms
            {
                j.priority = Priority::Interactive;
            }
            j
        })
        .collect();
    promoted.sort_by_key(|j| (j.priority.weight(), j.submitted_at));

    for job in promoted {
        let boards =
            paavo_db::BoardRow::find_healthy_for_selector(conn, &job.board_selector)?;
        if boards.is_empty() {
            continue;
        }
        let pick = lru_pick(boards);
        return Ok(Some(ScheduledJob { job, board: pick }));
    }
    Ok(None)
}

fn lru_pick(mut boards: Vec<paavo_db::BoardRow>) -> paavo_db::BoardRow {
    // Sort: never-used (None) first, then ascending last_used_at; then id
    // ascending for determinism.
    boards.sort_by(|a, b| {
        match (a.last_used_at, b.last_used_at) {
            (None, None) => a.spec.id.cmp(&b.spec.id),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => x.cmp(&y).then(a.spec.id.cmp(&b.spec.id)),
        }
    });
    boards.into_iter().next().expect("nonempty by caller")
}
```

`crates/paavo-core/src/enqueue.rs`:
```rust
//! Enqueue path: validate selector & ceiling, persist tar metadata, insert
//! job row in `Submitted` state.

use crate::error::{CoreError, Result};
use crate::selector::selector_matches_any;
use chrono::Utc;
use paavo_proto::{BoardSelector, BoardSpec, JobId, JobSource, Priority};
use rusqlite::Connection;

/// One enqueue request.
#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    /// Pre-allocated job id (caller may want to log it before insert).
    pub job_id: JobId,
    /// Scheduler priority.
    pub priority: Priority,
    /// Submitter free text.
    pub submitter: String,
    /// Where the request came from.
    pub source: JobSource,
    /// Selector.
    pub board_selector: BoardSelector,
    /// Effective inactivity ms (already resolved against the daemon default
    /// by the HTTP layer; ELF override is applied later).
    pub inactivity_timeout_ms: u64,
    /// Effective hard-max ms.
    pub hard_max_ms: u64,
    /// blake3 of the uploaded tar.
    pub tar_blake3: String,
    /// On-disk persisted tar path.
    pub tar_path: String,
    /// Daemon ceiling for hard-max; requests above this are rejected.
    pub daemon_ceiling_ms: u64,
}

/// Validate + persist a new job.
pub fn enqueue_job(
    conn: &Connection,
    inventory: &[BoardSpec],
    req: EnqueueRequest,
) -> Result<JobId> {
    if req.hard_max_ms > req.daemon_ceiling_ms {
        return Err(CoreError::OverCeiling {
            requested: req.hard_max_ms,
            ceiling: req.daemon_ceiling_ms,
        });
    }
    if !selector_matches_any(&req.board_selector, inventory) {
        return Err(CoreError::SelectorNeverMatches(req.board_selector));
    }
    let new = paavo_db::NewJob {
        id: req.job_id,
        priority: req.priority,
        submitter: req.submitter,
        source: req.source,
        board_selector: req.board_selector,
        inactivity_timeout_ms: req.inactivity_timeout_ms,
        hard_max_ms: req.hard_max_ms,
        tar_blake3: req.tar_blake3,
        tar_path: req.tar_path,
    };
    paavo_db::JobRow::insert(conn, &new, Utc::now().timestamp_millis())?;
    Ok(req.job_id)
}
```

- [ ] **Step 2: Confirm the crate compiles**

Run: `cargo build -p paavo-core`
Expected: builds.

- [ ] **Step 3: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-core
git -C D:\workspace\paavo commit -m "feat(core): module skeleton + selector/quarantine/scheduler/enqueue stubs"
```

---

#### 3.2.b: Enqueue + selector rejection tests

- [ ] **Step 1: Common test harness**

`crates/paavo-core/tests/common/mod.rs`:
```rust
//! Shared scaffolding for paavo-core integration tests.

use chrono::Utc;
use paavo_db::{BoardRow, Db};
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
use tempfile::tempdir;

pub fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

pub fn insert_board(db: &Db, id: &str, kind: &str, health: BoardHealth) -> BoardSpec {
    let spec = BoardSpec {
        id: id.into(),
        kind: kind.into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: id.into(),
        },
        chip_name: "X".into(),
        target_name: format!("target-{kind}"),
        wiring_profile: Some("default".into()),
        health,
    };
    BoardRow::insert(db.raw_conn(), &spec, Utc::now().timestamp_millis()).unwrap();
    if health == BoardHealth::Quarantined {
        BoardRow::quarantine(db.raw_conn(), id, "test setup").unwrap();
    }
    spec
}

pub fn list_inventory_specs(db: &Db) -> Vec<BoardSpec> {
    BoardRow::list_all(db.raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect()
}
```

- [ ] **Step 2: Write the enqueue test**

`crates/paavo-core/tests/enqueue.rs`:
```rust
mod common;

use common::{fresh_db, insert_board, list_inventory_specs};
use paavo_core::{enqueue_job, CoreError, EnqueueRequest};
use paavo_proto::{
    BoardHealth, BoardSelector, JobId, JobSource, Priority,
};

#[test]
fn enqueue_inserts_a_submitted_job() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let id = JobId::new();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let returned = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        EnqueueRequest {
            job_id: id,
            priority: Priority::Interactive,
            submitter: "felipe".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: Some("default".into()),
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            daemon_ceiling_ms: 8 * 60 * 60 * 1_000,
        },
        now_ms,
    )
    .unwrap();
    assert_eq!(returned, id);

    let row = paavo_db::JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, paavo_proto::JobState::Submitted);
}

#[test]
fn rejects_selector_with_no_matching_board() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let err = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        EnqueueRequest {
            job_id: JobId::new(),
            priority: Priority::Interactive,
            submitter: "x".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxap266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            daemon_ceiling_ms: 8 * 60 * 60 * 1_000,
        },
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap_err();
    assert!(matches!(err, CoreError::SelectorNeverMatches(_)), "{err}");
}

#[test]
fn accepts_when_only_match_is_quarantined() {
    // Per spec §5.5 the selector must be *possible*, not currently available.
    // A quarantined board is still possible — bring it back online with
    // unquarantine and it can run. So this case should be accepted.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Quarantined);
    let id = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        EnqueueRequest {
            job_id: JobId::new(),
            priority: Priority::Interactive,
            submitter: "x".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            daemon_ceiling_ms: 8 * 60 * 60 * 1_000,
        },
        chrono::Utc::now().timestamp_millis(),
    );
    assert!(id.is_ok(), "quarantined boards should not block enqueue");
}

#[test]
fn rejects_hard_max_above_daemon_ceiling() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let err = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        EnqueueRequest {
            job_id: JobId::new(),
            priority: Priority::Scheduled,
            submitter: "scheduler".into(),
            source: JobSource::Scheduler,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 9 * 60 * 60 * 1_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            daemon_ceiling_ms: 8 * 60 * 60 * 1_000,
        },
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap_err();
    assert!(
        matches!(err, CoreError::OverCeiling { requested, ceiling }
            if requested == 9*60*60*1_000 && ceiling == 8*60*60*1_000),
        "{err}",
    );
}
```

- [ ] **Step 2: Run to confirm pass**

Run: `cargo test -p paavo-core --test enqueue`
Expected: 4 passed.

- [ ] **Step 3: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-core/tests/common crates/paavo-core/tests/enqueue.rs
git -C D:\workspace\paavo commit -m "test(core): enqueue path — selector + ceiling validation"
```

---

#### 3.2.c: Scheduler priority + LRU + starvation tests

> **Note on the harness:** the test snippets below show per-file `enqueue_at` / `enqueue` helpers for clarity. The actual committed code (and the recommended pattern for 3.2.d/e) uses a single shared helper `common::enqueue_with(db, submitted_at_ms, |req| { ... })` that takes a closure to override fields on `default_enqueue_request("mcxa266")`. See `crates/paavo-core/tests/common/mod.rs` for the signature. The plan code below works either way; prefer the shared helper for new tests.

- [ ] **Step 1: Priority test**

`crates/paavo-core/tests/scheduler_priority.rs`:
```rust
mod common;

use common::{default_enqueue_request, fresh_db, insert_board};
use paavo_core::{enqueue_job, pick_next, SchedulerConfig};
use paavo_proto::{BoardHealth, JobId, JobSource, Priority};

/// Inject a deterministic `submitted_at` so priority/order assertions are
/// race-free. Uses the M3.2.b harness helper to keep the EnqueueRequest
/// boilerplate out of the test bodies.
fn enqueue_at(
    db: &paavo_db::Db,
    priority: Priority,
    source: JobSource,
    submitted_at_ms: i64,
) -> JobId {
    let mut req = default_enqueue_request("mcxa266");
    req.priority = priority;
    req.source = source;
    let id = req.job_id;
    enqueue_job(
        db.raw_conn(),
        &common::list_inventory_specs(db),
        req,
        submitted_at_ms,
    )
    .unwrap();
    id
}

const T0: i64 = 1_700_000_000_000;
const NOW: i64 = T0 + 60_000;

#[test]
fn picks_interactive_over_scheduled_even_if_older() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    // Scheduled inserted "first" (submitted_at lower).
    let scheduled = enqueue_at(&db, Priority::Scheduled, JobSource::Scheduler, T0);
    let interactive = enqueue_at(&db, Priority::Interactive, JobSource::Cli, T0 + 2_000);

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.job.id, interactive);
    assert_ne!(pick.job.id, scheduled);
}

#[test]
fn returns_none_when_no_healthy_board_matches() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Quarantined);
    let _ = enqueue_at(&db, Priority::Interactive, JobSource::Cli, T0);

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW).unwrap();
    assert!(pick.is_none());
}

#[test]
fn returns_none_when_no_submitted_jobs() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW).unwrap();
    assert!(pick.is_none());
}

#[test]
fn within_a_priority_class_oldest_submitted_wins() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let a = enqueue_at(&db, Priority::Interactive, JobSource::Cli, T0);
    let _b = enqueue_at(&db, Priority::Interactive, JobSource::Cli, T0 + 2_000);

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.job.id, a);
}
```

- [ ] **Step 2: LRU test**

`crates/paavo-core/tests/scheduler_lru.rs`:
```rust
mod common;

use common::{default_enqueue_request, fresh_db, insert_board, list_inventory_specs};
use paavo_core::{enqueue_job, pick_next, SchedulerConfig};
use paavo_proto::BoardHealth;

const NOW: i64 = 1_700_000_060_000;

fn enqueue(db: &paavo_db::Db) {
    let req = default_enqueue_request("mcxa266");
    enqueue_job(db.raw_conn(), &list_inventory_specs(db), req, NOW - 60_000).unwrap();
}

#[test]
fn never_used_board_wins_over_recently_used() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    insert_board(&db, "mcxa266-02", "mcxa266", BoardHealth::Healthy);

    // Mark -01 as recently used.
    paavo_db::BoardRow::touch_last_used(db.raw_conn(), "mcxa266-01", NOW - 1_000).unwrap();
    enqueue(&db);

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.board.spec.id, "mcxa266-02");
}

#[test]
fn older_last_used_wins_when_both_have_used() {
    let db = fresh_db();
    insert_board(&db, "a", "mcxa266", BoardHealth::Healthy);
    insert_board(&db, "b", "mcxa266", BoardHealth::Healthy);
    paavo_db::BoardRow::touch_last_used(db.raw_conn(), "a", 500).unwrap();
    paavo_db::BoardRow::touch_last_used(db.raw_conn(), "b", 100).unwrap();
    enqueue(&db);

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.board.spec.id, "b");
}
```

- [ ] **Step 3: Starvation promotion test**

`crates/paavo-core/tests/scheduler_starvation.rs`:
```rust
mod common;

use common::{default_enqueue_request, fresh_db, insert_board, list_inventory_specs};
use paavo_core::{enqueue_job, pick_next, SchedulerConfig};
use paavo_proto::{BoardHealth, JobId, JobSource, Priority};

#[test]
fn scheduled_job_older_than_threshold_outranks_a_fresh_interactive() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    // Use injected timestamps so the test is race-free.
    const T_SCHEDULED: i64 = 1_700_000_000_000;
    const T_INTERACTIVE: i64 = T_SCHEDULED + 80; // ms
    const NOW: i64 = T_INTERACTIVE + 1;
    let cfg = SchedulerConfig {
        starvation_threshold_ms: 50,
    };

    let scheduled = JobId::new();
    let mut sreq = default_enqueue_request("mcxa266");
    sreq.job_id = scheduled;
    sreq.priority = Priority::Scheduled;
    sreq.source = JobSource::Scheduler;
    sreq.submitter = "scheduler".into();
    sreq.hard_max_ms = 14_400_000;
    enqueue_job(db.raw_conn(), &list_inventory_specs(&db), sreq, T_SCHEDULED).unwrap();

    let interactive = JobId::new();
    let mut ireq = default_enqueue_request("mcxa266");
    ireq.job_id = interactive;
    ireq.priority = Priority::Interactive;
    ireq.source = JobSource::Cli;
    ireq.submitter = "cli".into();
    enqueue_job(db.raw_conn(), &list_inventory_specs(&db), ireq, T_INTERACTIVE).unwrap();

    let pick = pick_next(db.raw_conn(), cfg, NOW).unwrap().unwrap();
    assert_eq!(
        pick.job.id, scheduled,
        "starved Scheduled job should outrank fresh Interactive"
    );
}

#[test]
fn scheduled_job_within_threshold_does_not_promote() {
    // Mirror image of the above: when now-submitted_at < threshold, the
    // Scheduled job is NOT promoted and the fresh Interactive wins.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    const T_SCHEDULED: i64 = 1_700_000_000_000;
    const T_INTERACTIVE: i64 = T_SCHEDULED + 30; // ms
    const NOW: i64 = T_INTERACTIVE + 1;
    let cfg = SchedulerConfig {
        starvation_threshold_ms: 50, // 50ms threshold, scheduled is only 31ms old at NOW
    };

    let scheduled = JobId::new();
    let mut sreq = default_enqueue_request("mcxa266");
    sreq.job_id = scheduled;
    sreq.priority = Priority::Scheduled;
    sreq.source = JobSource::Scheduler;
    enqueue_job(db.raw_conn(), &list_inventory_specs(&db), sreq, T_SCHEDULED).unwrap();

    let interactive = JobId::new();
    let mut ireq = default_enqueue_request("mcxa266");
    ireq.job_id = interactive;
    ireq.priority = Priority::Interactive;
    ireq.source = JobSource::Cli;
    enqueue_job(db.raw_conn(), &list_inventory_specs(&db), ireq, T_INTERACTIVE).unwrap();

    let pick = pick_next(db.raw_conn(), cfg, NOW).unwrap().unwrap();
    assert_eq!(
        pick.job.id, interactive,
        "non-starved Scheduled job should not outrank a fresh Interactive"
    );
}
```

- [ ] **Step 4: Run all three scheduler tests**

Run: `cargo test -p paavo-core --tests`
Expected: 4 priority + 2 lru + 2 starvation + 6 enqueue = **14 passed**.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-core/tests/scheduler_priority.rs crates/paavo-core/tests/scheduler_lru.rs crates/paavo-core/tests/scheduler_starvation.rs
git -C D:\workspace\paavo commit -m "test(core): scheduler — priority + LRU + starvation promotion"
```

---

#### 3.2.d: Quarantine + cancel tests

- [ ] **Step 1: Quarantine test**

`crates/paavo-core/tests/quarantine.rs`:
```rust
mod common;

use common::{fresh_db, insert_board};
use paavo_core::{apply_outcome_to_board, QuarantinePolicy};
use paavo_proto::{
    BoardHealth, JobOutcome, TerminalOutcome, TimeoutReason,
};

#[test]
fn three_infra_errs_auto_quarantine_the_board() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 3,
    };

    for i in 0..2 {
        let just_quarantined = apply_outcome_to_board(
            db.raw_conn(),
            "mcxa266-01",
            &JobOutcome::Failed(TerminalOutcome::InfraErr {
                stage: "probe_attach".into(),
                message: "boom".into(),
            }),
            true,
            policy,
        )
        .unwrap();
        assert!(!just_quarantined, "iter {i}");
    }
    let just_quarantined = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: "probe_attach".into(),
            message: "boom".into(),
        }),
        true,
        policy,
    )
    .unwrap();
    assert!(just_quarantined);
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Quarantined);
    assert!(row
        .quarantine_reason
        .unwrap_or_default()
        .starts_with("auto: 3"));
}

#[test]
fn a_passing_run_resets_the_counter() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 3,
    };

    apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: "x".into(),
            message: "x".into(),
        }),
        true,
        policy,
    )
    .unwrap();
    apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Passed,
        true,
        policy,
    )
    .unwrap();
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 0);
}

#[test]
fn inactivity_timeout_with_unreleased_probe_counts_toward_quarantine() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1,
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms: 120_000,
        },
        /* probe_released_cleanly = */ false,
        policy,
    )
    .unwrap();
    assert!(just_q);
}

#[test]
fn inactivity_timeout_with_clean_probe_release_does_not_count() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1,
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms: 120_000,
        },
        /* probe_released_cleanly = */ true,
        policy,
    )
    .unwrap();
    assert!(!just_q);
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
}

#[test]
fn hard_max_does_not_count_toward_quarantine() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1,
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms: 900_000,
        },
        false, // even with bad release
        policy,
    )
    .unwrap();
    assert!(!just_q);
}
```

- [ ] **Step 2: Cancel-by-state test**

The cancel path is a thin wrapper over `JobRow::finalize` for Submitted + a signal for Building/Running. We test the Submitted shortcut here; the Building/Running paths land integrated in M4 with the daemon's worker pool.

`crates/paavo-core/tests/cancel.rs`:
```rust
mod common;

use common::{enqueue_with, fresh_db, insert_board};
use paavo_core::CoreError;
use paavo_proto::{AbortReason, BoardHealth, JobOutcome, JobState};

const NOW: i64 = 1_700_000_000_000;

#[test]
fn cancel_submitted_job_finalizes_with_aborted_user() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let id = enqueue_with(&db, NOW, |_| {});

    let outcome = paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 1).unwrap();
    assert_eq!(
        outcome,
        Some(JobOutcome::Aborted {
            by: AbortReason::User
        })
    );
    let row = paavo_db::JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Aborted);
    assert_eq!(
        row.outcome,
        Some(JobOutcome::Aborted {
            by: AbortReason::User
        })
    );
}

#[test]
fn cancel_running_job_returns_not_cancellable_inline() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let id = enqueue_with(&db, NOW, |_| {});

    // Force into Running state.
    paavo_db::JobRow::transition_to_building(
        db.raw_conn(),
        &id,
        "mcxa266-01",
        NOW + 1,
    )
    .unwrap();
    paavo_db::JobRow::transition_to_running(db.raw_conn(), &id, "/cache/foo.elf").unwrap();

    let res = paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 2);
    let err = res.unwrap_err();
    assert!(matches!(err, CoreError::NotCancellable {
        state: JobState::Running
    }), "{err}");
}

#[test]
fn cancel_already_finalized_returns_not_cancellable() {
    // Aborted/Passed/Failed/TimedOut are all terminal; cancel must reject.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let id = enqueue_with(&db, NOW, |_| {});

    // First cancel succeeds.
    paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 1).unwrap();

    // Second cancel must reject; state is now Aborted.
    let err = paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 2).unwrap_err();
    assert!(matches!(err, CoreError::NotCancellable {
        state: JobState::Aborted
    }), "{err}");
}
```

- [ ] **Step 3: Add `cancel_if_submitted` to paavo-core**

Append to `crates/paavo-core/src/lib.rs`:
```rust
mod cancel;
pub use cancel::cancel_if_submitted;
```

`crates/paavo-core/src/cancel.rs`:
```rust
//! Cancellation path that lives entirely in paavo-core: short-circuit for
//! the `Submitted` state. The `Building`/`Running` paths go through paavod
//! because they need to signal a running BoardWorker — that wiring lives in
//! M4.

use crate::error::{CoreError, Result};
use paavo_proto::{AbortReason, JobId, JobOutcome, JobState};
use rusqlite::Connection;

/// If the job is in `Submitted` state, mark it `Aborted{User}` and return
/// the outcome. Otherwise, return `NotCancellable`.
///
/// `now_ms` is the wall-clock instant to record as `finished_at_ms`.
/// Production passes `Utc::now().timestamp_millis()`; tests inject
/// deterministic values.
pub fn cancel_if_submitted(
    conn: &Connection,
    id: &JobId,
    now_ms: i64,
) -> Result<Option<JobOutcome>> {
    let row = paavo_db::JobRow::get(conn, id)?;
    if row.state != JobState::Submitted {
        return Err(CoreError::NotCancellable { state: row.state });
    }
    let outcome = JobOutcome::Aborted {
        by: AbortReason::User,
    };
    paavo_db::JobRow::finalize(
        conn,
        id,
        &paavo_db::OutcomeRecord {
            state: JobState::Aborted,
            outcome: outcome.clone(),
            finished_at_ms: now_ms,
        },
    )?;
    Ok(Some(outcome))
}
```

- [ ] **Step 4: Run all paavo-core tests**

Run: `cargo test -p paavo-core`
Expected: enqueue 6 + scheduler priority 4 + scheduler lru 3 + scheduler starvation 3 + quarantine 5 + cancel 3 = **24 integration tests**. Plus 1 doctest from lib.rs = **25 total**.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p paavo-core --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-core
git -C D:\workspace\paavo commit -m "feat(core): quarantine policy + Submitted-state cancel shortcut, full TDD coverage"
```

---

#### 3.2.e: build_cache helpers (paavo-core)

The cache lookup/store helpers compose `paavo-build::build_release` with `paavo-db::BuildCacheEntry`. They live in `paavo-core` because they cross both modules — exactly the glue role `paavo-core` exists for. The helpers are stateless: callers (paavod) pass a `Connection`; on Hit, the function prunes stale rows when the ELF file has been deleted (self-healing). `evict_lru` is also exposed so the nightly cron can prune the cache.

**Files:**
- Create: `crates/paavo-core/src/build_cache.rs`
- Test: `crates/paavo-core/tests/build_cache.rs`

**Note:** `cache_lookup` takes `now_ms: i64` per the M3.2.a testability discipline (used to bump `last_used_at`). Production passes `Utc::now().timestamp_millis()`; tests inject deterministic values. `CoreError` gets an `Io(#[from] std::io::Error)` variant to type the cache_store filesystem-stat failure cleanly (previously the plan wrapped io::Error in rusqlite::Error::ToSqlConversionFailure which lied about the error class).

- [ ] **Step 1: Add `Io` variant to `CoreError`**

In `crates/paavo-core/src/error.rs`, add a new variant after `Build`:

```rust
    /// I/O failure outside paavo-build (e.g. stat'ing an ELF file in the
    /// build-cache helpers).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
```

- [ ] **Step 2: Write the failing test**

`crates/paavo-core/tests/build_cache.rs`:
```rust
mod common;

use common::fresh_db;
use paavo_core::{cache_lookup, cache_store, evict_lru, CacheLookup};
use paavo_db::BuildCacheEntry;
use std::fs;
use tempfile::tempdir;

const NOW: i64 = 1_700_000_000_000;

#[test]
fn lookup_returns_miss_when_no_entry() {
    let db = fresh_db();
    assert_eq!(
        cache_lookup(db.raw_conn(), "deadbeef", NOW).unwrap(),
        CacheLookup::Miss
    );
}

#[test]
fn lookup_returns_hit_after_store_and_bumps_last_used() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let elf = tmp.path().join("foo.elf");
    fs::write(&elf, b"\x7fELF").unwrap();

    let blake = "aabbccdd";
    cache_store(db.raw_conn(), blake, &elf, NOW).unwrap();

    match cache_lookup(db.raw_conn(), blake, NOW + 1).unwrap() {
        CacheLookup::Hit { elf_path } => assert_eq!(elf_path, elf),
        CacheLookup::Miss => panic!("expected Hit"),
    }
    let row = BuildCacheEntry::get(db.raw_conn(), blake).unwrap();
    assert_eq!(row.last_used_at, NOW + 1, "lookup must bump last_used_at");
}

#[test]
fn lookup_returns_miss_if_elf_file_disappeared() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let elf = tmp.path().join("foo.elf");
    fs::write(&elf, b"\x7fELF").unwrap();
    let blake = "ff00ff00";
    cache_store(db.raw_conn(), blake, &elf, NOW).unwrap();
    fs::remove_file(&elf).unwrap();

    assert_eq!(
        cache_lookup(db.raw_conn(), blake, NOW + 1).unwrap(),
        CacheLookup::Miss
    );
    // The stale row should have been pruned.
    assert!(BuildCacheEntry::find(db.raw_conn(), blake).unwrap().is_none());
}

#[test]
fn store_records_size_from_filesystem() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let elf = tmp.path().join("foo.elf");
    let payload = b"\x7fELFsomemorebytes";
    fs::write(&elf, payload).unwrap();
    cache_store(db.raw_conn(), "sizetest", &elf, NOW).unwrap();
    let row = BuildCacheEntry::get(db.raw_conn(), "sizetest").unwrap();
    assert_eq!(row.size_bytes, payload.len() as u64);
}

#[test]
fn evict_lru_drops_least_recently_used_entries_and_unlinks_files() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();

    // Three entries with distinct last_used_at; size 100 each = 300 total.
    let mut elfs: Vec<std::path::PathBuf> = Vec::new();
    for (i, blake) in ["aa", "bb", "cc"].iter().enumerate() {
        let path = tmp.path().join(format!("{blake}.elf"));
        fs::write(&path, vec![0u8; 100]).unwrap();
        cache_store(db.raw_conn(), blake, &path, NOW + i as i64).unwrap();
        elfs.push(path);
    }

    // Cap at 200 bytes -> evict the oldest (aa).
    let evicted = evict_lru(db.raw_conn(), 200).unwrap();
    assert_eq!(evicted.len(), 1, "expected exactly one eviction");
    assert_eq!(evicted[0].tar_blake3, "aa");
    assert!(!elfs[0].exists(), "evicted ELF file must be unlinked");
    assert!(elfs[1].exists(), "non-evicted ELF must remain");
    assert!(elfs[2].exists());
}
```

- [ ] **Step 3: Run to confirm fail**

Run: `cargo test -p paavo-core --test build_cache`
Expected: FAIL — `cache_lookup` / `cache_store` / `evict_lru` / `CacheLookup` don't exist in `paavo-core` yet.

- [ ] **Step 4: Implement the helpers**

`crates/paavo-core/src/build_cache.rs`:
```rust
//! Build-cache helpers: pair `paavo-build` (which produces ELFs) with
//! `paavo-db::BuildCacheEntry` (which persists where the ELF landed).
//!
//! Lives in `paavo-core` because it bridges the two; `paavo-build` itself
//! stays DB-free per spec §4.1.

use crate::error::{CoreError, Result};
use paavo_db::BuildCacheEntry;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Outcome of a cache lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLookup {
    /// Cache hit. Caller can skip `paavo_build::build_release`.
    Hit {
        /// Cached ELF.
        elf_path: PathBuf,
    },
    /// Cache miss.
    Miss,
}

/// Look up a tar's cached ELF by blake3. Returns `Miss` if there's no row,
/// or if the row's ELF file has gone missing on disk (in which case the
/// stale row is also pruned so this function is self-healing).
///
/// `now_ms` is the wall-clock instant to record as `last_used_at` on a
/// hit. Production passes `Utc::now().timestamp_millis()`; tests inject
/// deterministic values.
pub fn cache_lookup(
    conn: &Connection,
    tar_blake3: &str,
    now_ms: i64,
) -> Result<CacheLookup> {
    let Some(entry) = BuildCacheEntry::find(conn, tar_blake3)? else {
        return Ok(CacheLookup::Miss);
    };
    let elf_path = PathBuf::from(&entry.elf_path);
    if !elf_path.is_file() {
        // Self-heal: drop the stale row. Errors here are best-effort —
        // the user-visible answer is still Miss.
        let _ = conn.execute(
            "DELETE FROM build_cache WHERE tar_blake3 = ?1",
            rusqlite::params![tar_blake3],
        );
        return Ok(CacheLookup::Miss);
    }
    BuildCacheEntry::touch_last_used(conn, tar_blake3, now_ms)?;
    Ok(CacheLookup::Hit { elf_path })
}

/// Insert (or refresh) a cache entry mapping `tar_blake3 -> elf_path`.
/// Stats the ELF file to record its size; failure to stat returns
/// `CoreError::Io`.
pub fn cache_store(
    conn: &Connection,
    tar_blake3: &str,
    elf_path: &Path,
    now_ms: i64,
) -> Result<()> {
    let size = std::fs::metadata(elf_path)?.len();
    BuildCacheEntry::upsert(
        conn,
        &BuildCacheEntry {
            tar_blake3: tar_blake3.to_string(),
            elf_path: elf_path.display().to_string(),
            built_at: now_ms,
            last_used_at: now_ms,
            size_bytes: size,
        },
    )?;
    Ok(())
}

/// Evict cache entries until total size <= `max_bytes`. Removes the
/// underlying ELF files on disk for each evicted row. Returns the list
/// of evicted entries (in eviction order — least-recently-used first).
///
/// Best-effort on file deletion: if an ELF file is already gone, the
/// eviction still counts as successful (the DB row is removed regardless).
pub fn evict_lru(conn: &Connection, max_bytes: u64) -> Result<Vec<BuildCacheEntry>> {
    let evicted = BuildCacheEntry::evict_until_under(conn, max_bytes)?;
    for entry in &evicted {
        let _ = std::fs::remove_file(&entry.elf_path);
    }
    Ok(evicted)
}
```

- [ ] **Step 5: Re-export from `lib.rs`**

In `crates/paavo-core/src/lib.rs`, add:
```rust
mod build_cache;
```
(alphabetically, before `mod cancel;`)

and:
```rust
pub use build_cache::{cache_lookup, cache_store, evict_lru, CacheLookup};
```
(alphabetically, before `pub use cancel::...`)

- [ ] **Step 6: Run the test**

Run: `cargo test -p paavo-core --test build_cache`
Expected: 5 passed.

- [ ] **Step 7: Full paavo-core suite**

Run: `cargo test -p paavo-core`
Expected: 6 enqueue + 4 priority + 3 lru + 3 starvation + 8 quarantine + 4 cancel + 5 build_cache = **33 integration tests**. Plus 1 doctest = **34 total**.

- [ ] **Step 8: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-core
git -C D:\workspace\paavo commit -m "feat(core): build_cache lookup/store/evict glue between paavo-build and paavo-db"
```

---

### Milestone 3 exit criteria

- [ ] `paavo-build` can untar, build, and discover an ELF for a host-cargo fixture
- [ ] `paavo-core::cache_lookup` is self-healing (drops stale rows when ELF is gone)
- [ ] `paavo-core::enqueue_job` rejects impossible selectors and over-ceiling timeouts
- [ ] `paavo-core::pick_next` honours priority, starvation promotion, and LRU board selection
- [ ] `paavo-core::apply_outcome_to_board` implements §5.2 table exactly (including the inactivity+release-failed subtlety)
- [ ] `cargo test --workspace` green

---

## Milestone 4 — Daemon + CLI

Goal: `paavod` HTTP server with config loading, multipart upload, NDJSON streaming, cron, SIGTERM drain; `paavo-cli` with all subcommands.

### Task 4.1: paavod — config + state dir

Spec coverage: §13 (config schema), §14.1 (state dir under `/var/lib/paavo`).

**Files:**
- Create: `crates/paavod/src/main.rs` (replace skeleton)
- Create: `crates/paavod/src/config.rs`
- Create: `crates/paavod/src/state_dir.rs`
- Test: `crates/paavod/tests/config_loading.rs`

- [ ] **Step 1: Write the failing config test**

`crates/paavod/tests/config_loading.rs`:
```rust
use paavod::config::Config;
use std::fs;
use tempfile::tempdir;

// Cron is 6-field (`sec min hour dom mon dow`) — the `cron` crate's
// native form, also what `tokio-cron-scheduler` parses. Time zone is
// the daemon's local TZ. `"0 0 19 * * *"` = "every day at 19:00:00".
const SAMPLE: &str = r#"
[server]
bind = "127.0.0.1:8080"
state_dir = "/var/lib/paavo"

[web]
bind = "127.0.0.1:8081"

[timeouts]
default_inactivity_s = 120
default_ad_hoc_hard_max_s = 900
default_scheduled_hard_max_s = 14400
daemon_ceiling_s = 28800
shutdown_grace_s = 60

[scheduler]
starvation_threshold_s = 21600
nightly_cron = "0 0 19 * * *"

[build_cache]
max_bytes = 5368709120

[retention]
passed_full_log_days = 30

[quarantine]
consecutive_infra_failures = 3

[[corpus]]
name = "embassy-mcxa-regression"
path = "/var/lib/paavo/checkouts/embassy/tests/mcxa2xx"
cargo_update = ["embassy-mcxa", "embassy-executor"]

[[corpus]]
name = "paavo-soak-mcxa266"
path = "/var/lib/paavo/checkouts/paavo/soak-tests/mcxa266"
cargo_update = []
"#;

#[test]
fn parses_sample_config() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(&p, SAMPLE).unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.server.bind, "127.0.0.1:8080");
    assert_eq!(cfg.web.bind, "127.0.0.1:8081");
    assert_eq!(cfg.timeouts.default_inactivity_s, 120);
    assert_eq!(cfg.timeouts.daemon_ceiling_s, 28800);
    assert_eq!(cfg.scheduler.nightly_cron, "0 0 19 * * *");
    assert_eq!(cfg.build_cache.max_bytes, 5_368_709_120);
    assert_eq!(cfg.retention.passed_full_log_days, 30);
    assert_eq!(cfg.quarantine.consecutive_infra_failures, 3);
    assert_eq!(cfg.corpus.len(), 2);
    assert_eq!(cfg.corpus[0].name, "embassy-mcxa-regression");
    assert_eq!(
        cfg.corpus[0].cargo_update,
        vec!["embassy-mcxa".to_string(), "embassy-executor".into()]
    );
}

#[test]
fn rejects_invalid_cron() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(
        &p,
        SAMPLE.replace("0 0 19 * * *", "not a valid cron expression"),
    )
    .unwrap();
    let err = Config::load(&p).unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("cron"));
}

#[test]
fn rejects_five_field_cron() {
    // 5-field POSIX cron is a common ops-user mistake. Reject it
    // explicitly with a message that mentions both "cron" and the
    // 6-field expectation, so the operator can fix it without
    // having to read the `cron` crate's source.
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(&p, SAMPLE.replace("0 0 19 * * *", "0 19 * * *")).unwrap();
    let err = Config::load(&p).unwrap_err();
    let msg = format!("{err}").to_lowercase();
    assert!(msg.contains("cron"), "error should mention cron: {err}");
}

#[test]
fn defaults_used_when_optional_blocks_omitted() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(
        &p,
        r#"
[server]
bind = "127.0.0.1:8080"
state_dir = "/var/lib/paavo"
[web]
bind = "127.0.0.1:8081"
[scheduler]
nightly_cron = "0 0 19 * * *"
"#,
    )
    .unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.timeouts.default_inactivity_s, 120);
    assert_eq!(cfg.retention.passed_full_log_days, 30);
    assert_eq!(cfg.quarantine.consecutive_infra_failures, 3);
    assert!(cfg.corpus.is_empty());
}

#[test]
fn missing_file_error_mentions_path() {
    // Pins the `reading <path>` context wrapped around the io::Error.
    let dir = tempdir().unwrap();
    let p = dir.path().join("does-not-exist.toml");
    let err = Config::load(&p).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("does-not-exist.toml"),
        "error should mention the missing path: {msg}"
    );
}

#[test]
fn malformed_toml_error_mentions_paavo_toml() {
    // Pins the `parsing paavo.toml` context wrapped around the toml::Error.
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(&p, "[server\nbind = oops").unwrap();
    let err = Config::load(&p).unwrap_err();
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("paavo.toml") || msg.contains("parsing"),
        "error should mention the file or `parsing`: {msg}"
    );
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavod --test config_loading`
Expected: FAIL — `paavod::config::Config` doesn't exist.

- [ ] **Step 3: Implement config**

`crates/paavod/src/config.rs`:
```rust
//! paavo.toml schema + loader. See spec §13.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::str::FromStr;

/// Top-level config.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Daemon HTTP server.
    pub server: ServerConfig,
    /// Read-only web UI.
    pub web: WebConfig,
    /// Timeouts (defaults applied if section omitted).
    #[serde(default)]
    pub timeouts: TimeoutsConfig,
    /// Scheduler (required for `nightly_cron`).
    pub scheduler: SchedulerConfig,
    /// Build cache (defaults applied if section omitted).
    #[serde(default)]
    pub build_cache: BuildCacheConfig,
    /// Retention (defaults applied if section omitted).
    #[serde(default)]
    pub retention: RetentionConfig,
    /// Quarantine (defaults applied if section omitted).
    #[serde(default)]
    pub quarantine: QuarantineConfig,
    /// Corpus entries for the nightly run (may be empty).
    #[serde(default)]
    pub corpus: Vec<CorpusEntry>,
}

/// `[server]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// `host:port`. Required — declare explicitly in `paavo.toml`. The
    /// spec sample uses `127.0.0.1:8080`.
    pub bind: String,
    /// Daemon state dir (sandboxes, sqlite, build cache, etc.).
    pub state_dir: std::path::PathBuf,
}

/// `[web]`.
#[derive(Debug, Clone, Deserialize)]
pub struct WebConfig {
    /// `host:port` for the read-only web UI (axum + vanilla JS + UnoCSS CDN).
    pub bind: String,
}

/// `[timeouts]`.
#[derive(Debug, Clone, Deserialize)]
pub struct TimeoutsConfig {
    /// Inactivity timeout when ELF doesn't override and CLI doesn't override.
    #[serde(default = "default_inactivity_s")]
    pub default_inactivity_s: u64,
    /// Hard-max wall clock for ad-hoc `paavo-cli run` jobs.
    #[serde(default = "default_ad_hoc_hard_max_s")]
    pub default_ad_hoc_hard_max_s: u64,
    /// Hard-max wall clock for scheduled nightly jobs.
    #[serde(default = "default_scheduled_hard_max_s")]
    pub default_scheduled_hard_max_s: u64,
    /// Daemon ceiling — refuse `hard_max_ms > daemon_ceiling_s * 1000` at enqueue.
    #[serde(default = "default_daemon_ceiling_s")]
    pub daemon_ceiling_s: u64,
    /// SIGTERM drain grace.
    #[serde(default = "default_shutdown_grace_s")]
    pub shutdown_grace_s: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            default_inactivity_s: default_inactivity_s(),
            default_ad_hoc_hard_max_s: default_ad_hoc_hard_max_s(),
            default_scheduled_hard_max_s: default_scheduled_hard_max_s(),
            daemon_ceiling_s: default_daemon_ceiling_s(),
            shutdown_grace_s: default_shutdown_grace_s(),
        }
    }
}

fn default_inactivity_s() -> u64 { 120 }
fn default_ad_hoc_hard_max_s() -> u64 { 900 }
fn default_scheduled_hard_max_s() -> u64 { 14_400 }
fn default_daemon_ceiling_s() -> u64 { 28_800 }
fn default_shutdown_grace_s() -> u64 { 60 }

/// `[scheduler]`.
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Cron expression. **6-field** `sec min hour dom mon dow` (the
    /// `cron` crate's native form, also what `tokio-cron-scheduler`
    /// parses). Time zone is the daemon process's local TZ. Example:
    /// `"0 0 19 * * *"` = "every day at 19:00:00".
    pub nightly_cron: String,
    /// Promote Scheduled→Interactive after this many seconds queued.
    #[serde(default = "default_starvation_threshold_s")]
    pub starvation_threshold_s: i64,
}

impl SchedulerConfig {
    /// Parse `nightly_cron` into a `cron::Schedule`. The downstream
    /// nightly cron driver in M4.3.c uses this so the same library
    /// that validates the expression at startup is the one that
    /// actually fires it.
    pub fn schedule(&self) -> Result<cron::Schedule, cron::error::Error> {
        cron::Schedule::from_str(&self.nightly_cron)
    }
}

fn default_starvation_threshold_s() -> i64 { 21_600 }

/// `[build_cache]`.
#[derive(Debug, Clone, Deserialize)]
pub struct BuildCacheConfig {
    /// LRU cap in bytes.
    #[serde(default = "default_build_cache_max_bytes")]
    pub max_bytes: u64,
}

impl Default for BuildCacheConfig {
    fn default() -> Self {
        Self { max_bytes: default_build_cache_max_bytes() }
    }
}

fn default_build_cache_max_bytes() -> u64 { 5 * 1024 * 1024 * 1024 }

/// `[retention]`.
#[derive(Debug, Clone, Deserialize)]
pub struct RetentionConfig {
    /// After this many days, drop trace/debug/info frames from `Passed`
    /// jobs. Negative disables truncation.
    #[serde(default = "default_passed_full_log_days")]
    pub passed_full_log_days: i32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            passed_full_log_days: default_passed_full_log_days(),
        }
    }
}

fn default_passed_full_log_days() -> i32 { 30 }

/// `[quarantine]`.
#[derive(Debug, Clone, Deserialize)]
pub struct QuarantineConfig {
    /// Auto-quarantine threshold.
    #[serde(default = "default_consecutive_infra_failures")]
    pub consecutive_infra_failures: u32,
}

impl Default for QuarantineConfig {
    fn default() -> Self {
        Self {
            consecutive_infra_failures: default_consecutive_infra_failures(),
        }
    }
}

fn default_consecutive_infra_failures() -> u32 { 3 }

/// One `[[corpus]]` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct CorpusEntry {
    /// Human-readable name (e.g. `embassy-mcxa-regression`).
    pub name: String,
    /// Filesystem path holding one or more test crates (each subdir = one
    /// test crate per the spec).
    pub path: std::path::PathBuf,
    /// Packages to `cargo update -p ...` before building each crate
    /// (e.g. `["embassy-mcxa", "embassy-executor"]`).
    #[serde(default)]
    pub cargo_update: Vec<String>,
}

impl Config {
    /// Load from a path; validates the nightly cron expression.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        let cfg: Config = toml::from_str(&raw).context("parsing paavo.toml")?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate a programmatically-built `Config`. Public so callers
    /// who build a `Config` via struct literal (tests, future
    /// `paavo-cli config validate`) can re-check the same invariants
    /// `Config::load` enforces.
    pub fn validate(&self) -> Result<()> {
        self.scheduler.schedule().map_err(|e| {
            anyhow!(
                "scheduler.nightly_cron is not a valid cron expression — \
                 must be 6-field `sec min hour dom mon dow` ({e})"
            )
        })?;
        Ok(())
    }
}
```

`crates/paavod/src/state_dir.rs`:
```rust
//! Layout under `server.state_dir`.

use std::path::{Path, PathBuf};

/// Resolved sub-paths inside the daemon state directory.
#[derive(Debug, Clone)]
pub struct StateDir {
    /// Root.
    pub root: PathBuf,
    /// SQLite database file.
    pub sqlite_path: PathBuf,
    /// Tar uploads keyed by blake3.
    pub uploads_dir: PathBuf,
    /// Per-job sandbox dirs.
    pub sandboxes_dir: PathBuf,
    /// Shared `CARGO_TARGET_DIR`.
    pub cargo_target_dir: PathBuf,
    /// Cached ELFs keyed by blake3.
    pub cache_elfs_dir: PathBuf,
    /// boards.toml — managed by `paavo-cli board add`.
    pub boards_toml: PathBuf,
}

impl StateDir {
    /// Compute paths under `root`; does not create them.
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            root: root.to_path_buf(),
            sqlite_path: root.join("paavo.sqlite"),
            uploads_dir: root.join("uploads"),
            sandboxes_dir: root.join("sandboxes"),
            cargo_target_dir: root.join("cargo-target"),
            cache_elfs_dir: root.join("cache").join("elf"),
            boards_toml: root.join("boards.toml"),
        }
    }

    /// Create `root` and every subdirectory under it. Idempotent. Does
    /// NOT touch `sqlite_path` (created by paavo-db) or `boards_toml`
    /// (created by paavo-cli). TODO(M4.4): also chmod the root to 0700
    /// on Unix once paavod's main wires this up at startup.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.uploads_dir)?;
        std::fs::create_dir_all(&self.sandboxes_dir)?;
        std::fs::create_dir_all(&self.cargo_target_dir)?;
        std::fs::create_dir_all(&self.cache_elfs_dir)?;
        Ok(())
    }
}
```

`crates/paavod/src/main.rs` (temporary lib stub so tests can `use paavod::*`):
```rust
//! paavod binary entry point. The real `tokio::main` body lands in 4.4.

fn main() {
    println!("paavod: see plan Task 4.4 for the runtime entry point");
}
```

We need `paavod` to also expose its modules to tests, so add a `lib.rs`:

Create `crates/paavod/src/lib.rs`:
```rust
//! paavod library — pulled out so integration tests can `use paavod::*`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name.
pub const CRATE_NAME: &str = "paavod";

pub mod config;
pub mod state_dir;
```

And tell Cargo about it — replace `crates/paavod/Cargo.toml`'s `[dependencies]` / structure with:
```toml
[lib]
name = "paavod"
path = "src/lib.rs"

[[bin]]
name = "paavod"
path = "src/main.rs"
```
(Keep the existing dependency block underneath, and add `cron = { workspace = true }` to it — required by `Config::validate`.)

- [ ] **Step 4: Run the config test**

Run: `cargo test -p paavod --test config_loading`
Expected: 6 passed (parses_sample_config, rejects_invalid_cron, rejects_five_field_cron, defaults_used_when_optional_blocks_omitted, missing_file_error_mentions_path, malformed_toml_error_mentions_paavo_toml).

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): paavo.toml schema + loader + state_dir layout"
```

---

### Task 4.2: paavod — HTTP API skeleton (axum)

Spec coverage: §9.1–§9.5.

**Files:**
- Create: `crates/paavod/src/app.rs`
- Create: `crates/paavod/src/routes/mod.rs`
- Create: `crates/paavod/src/routes/jobs.rs`
- Create: `crates/paavod/src/routes/boards.rs`
- Create: `crates/paavod/src/routes/health.rs`
- Create: `crates/paavod/src/app_state.rs`
- Test: `crates/paavod/tests/api_health.rs`
- Test: `crates/paavod/tests/api_jobs.rs`
- Test: `crates/paavod/tests/api_boards.rs`

#### 4.2.a: AppState + router shell + health endpoints

- [ ] **Step 1: Add AppState**

`crates/paavod/src/app_state.rs`:
```rust
//! Shared axum state: db handle, config, fleet inventory cache, and the
//! SIGTERM drain flag.
//!
//! Concurrency contract:
//! - `db` uses `parking_lot::Mutex` because every SQLite call is sub-ms
//!   and the daemon is single-host. Lock duration is bounded; never hold
//!   the guard across an `.await`. Handlers that need to do async work
//!   after a read should copy the rows out, drop the guard, then await.
//!   `await_holding_lock` would warn about this if we ever drift.
//! - `inventory` is a write-through cache of the `boards` table. It MUST
//!   be hydrated once at startup by `paavod::main` (see Task 4.4) before
//!   the HTTP server starts accepting requests — otherwise the daemon
//!   will reject every selector after a restart until the operator does
//!   a redundant `POST /boards`. Handlers that mutate boards refresh
//!   the cache under the same lock.
//! - `drain` is a one-shot flag (false → true). M4.3.d wires the SIGTERM
//!   handler that calls `set_draining`.

#![deny(clippy::await_holding_lock)]

use paavo_proto::BoardSpec;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Drain mode for SIGTERM handling. One-way flag (false → true).
#[derive(Debug, Default, Clone)]
pub struct DrainState {
    inner: Arc<AtomicBool>,
}

impl DrainState {
    /// Returns true while the daemon is draining for shutdown.
    pub fn is_draining(&self) -> bool {
        // Acquire pairs with Release in `set_draining`; both writers
        // and readers see a consistent transition.
        self.inner.load(Ordering::Acquire)
    }
    /// Mark drain mode. Idempotent.
    pub fn set_draining(&self) {
        self.inner.store(true, Ordering::Release);
    }
}

/// Shared axum state.
#[derive(Clone)]
pub struct AppState {
    /// Daemon SQLite handle. Locked per-handler via `lock()`; serialised
    /// access — see the concurrency contract in the module docstring.
    pub db: Arc<Mutex<paavo_db::Db>>,
    /// Loaded config (immutable post-load).
    pub config: Arc<crate::config::Config>,
    /// In-memory inventory snapshot. Hydrated by `paavod::main` at
    /// startup; refreshed by every successful `boards` write.
    pub inventory: Arc<Mutex<Vec<BoardSpec>>>,
    /// One-shot SIGTERM drain flag.
    pub drain: DrainState,
}

impl AppState {
    /// Take a copy of the current inventory for selector validation.
    pub fn inventory_snapshot(&self) -> Vec<BoardSpec> {
        self.inventory.lock().clone()
    }
}
```

- [ ] **Step 2: Add health endpoints**

`crates/paavod/src/routes/health.rs`:
```rust
//! GET /health and GET /ready.
//!
//! Per spec §9.5: `/health` is liveness — always 200 with a small JSON
//! body, even while draining. `/ready` is readiness — 503 while draining,
//! 200 otherwise. Liveness probes (systemd Watchdog, k8s) must not kill
//! the daemon mid-drain.

use crate::app_state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

/// Body for both endpoints.
#[derive(Serialize)]
pub struct HealthBody {
    /// Always `"paavod"`.
    pub service: &'static str,
    /// True while not draining. Used by both `/health` and `/ready`,
    /// but only `/ready` flips its HTTP status based on it.
    pub ready: bool,
    /// Crate version.
    pub version: &'static str,
}

fn body(ready: bool) -> HealthBody {
    HealthBody {
        service: "paavod",
        ready,
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// Liveness — always 200, even while draining. Body reports the drain
/// state so monitoring can observe it without flipping the probe.
pub async fn health(State(s): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, Json(body(!s.drain.is_draining())))
}

/// Readiness — 503 while draining, 200 otherwise.
pub async fn ready(State(s): State<AppState>) -> impl IntoResponse {
    let draining = s.drain.is_draining();
    let status = if draining {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (status, Json(body(!draining)))
}
```

- [ ] **Step 3: Router shell + stubs for jobs/boards**

`crates/paavod/src/routes/mod.rs`:
```rust
//! HTTP routes mounted on the axum Router.

pub mod boards;
pub mod health;
pub mod jobs;
```

`crates/paavod/src/routes/jobs.rs` (stubs):
```rust
//! /jobs/* handlers. Filled in by 4.2.b/c.

use crate::app_state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

/// Stub that respects spec §6.3 ("drain returns 503 for new jobs") even
/// though the real handler isn't here yet. Locks in the invariant.
fn drain_then(state: &AppState, what: &'static str) -> (StatusCode, &'static str) {
    if state.drain.is_draining() {
        (StatusCode::SERVICE_UNAVAILABLE, "paavod is draining")
    } else {
        (StatusCode::NOT_IMPLEMENTED, what)
    }
}

/// POST /jobs — placeholder. Returns 503 while draining (spec §6.3).
pub async fn post_jobs(State(s): State<AppState>) -> impl IntoResponse {
    drain_then(&s, "POST /jobs not yet wired")
}

/// GET /jobs — placeholder.
pub async fn list_jobs(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "GET /jobs not yet wired")
}

/// GET /jobs/:id — placeholder.
pub async fn get_job(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "GET /jobs/:id not yet wired")
}

/// POST /jobs/:id/cancel — placeholder.
pub async fn cancel_job(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /jobs/:id/cancel not yet wired",
    )
}

/// GET /jobs/:id/stream — placeholder.
pub async fn stream_job(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "GET /jobs/:id/stream not yet wired",
    )
}
```

`crates/paavod/src/routes/boards.rs` (stubs):
```rust
//! /boards/* handlers. Filled in by 4.2.b.

use crate::app_state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

/// GET /boards — placeholder.
pub async fn list_boards(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "GET /boards not yet wired")
}

/// POST /boards — placeholder.
pub async fn add_board(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "POST /boards not yet wired")
}

/// POST /boards/:id/quarantine — placeholder.
pub async fn quarantine_board(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /boards/:id/quarantine not yet wired",
    )
}

/// POST /boards/:id/unquarantine — placeholder.
pub async fn unquarantine_board(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /boards/:id/unquarantine not yet wired",
    )
}
```

`crates/paavod/src/app.rs`:
```rust
//! axum app constructor.

use crate::app_state::AppState;
use crate::routes;
use axum::routing::{get, post};
use axum::Router;

/// Build the axum Router with all routes mounted.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/ready", get(routes::health::ready))
        .route(
            "/jobs",
            post(routes::jobs::post_jobs).get(routes::jobs::list_jobs),
        )
        .route("/jobs/:id", get(routes::jobs::get_job))
        .route("/jobs/:id/cancel", post(routes::jobs::cancel_job))
        .route("/jobs/:id/stream", get(routes::jobs::stream_job))
        .route(
            "/boards",
            get(routes::boards::list_boards).post(routes::boards::add_board),
        )
        .route(
            "/boards/:id/quarantine",
            post(routes::boards::quarantine_board),
        )
        .route(
            "/boards/:id/unquarantine",
            post(routes::boards::unquarantine_board),
        )
        .with_state(state)
}
```

Expose modules in lib:
```rust
// crates/paavod/src/lib.rs — add:
pub mod app;
pub mod app_state;
pub mod routes;
```

Add the new runtime dep to `crates/paavod/Cargo.toml` `[dependencies]`:
```toml
parking_lot = { workspace = true }
```
(`AppState` uses `parking_lot::Mutex`; `tower` is already a dep so `ServiceExt::oneshot` works in the integration test.)

- [ ] **Step 4: Add the health integration test**

`crates/paavod/tests/api_health.rs`:
```rust
use axum::body::to_bytes;
use axum::http::Request;
use paavo_db::Db;
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

fn make_state() -> AppState {
    // Cross-platform: derive every path from the same tempdir so the
    // test never embeds a Unix-only `/tmp/...` literal. The dir is
    // leaked per workspace convention — see
    // `crates/paavo-core/tests/common/mod.rs::fresh_db`.
    let dir = tempdir().unwrap();
    let state_dir = dir.path().to_path_buf();
    let db = Db::open(state_dir.join("paavo.sqlite")).unwrap();
    std::mem::forget(dir);
    let cfg = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    };
    AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(cfg),
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
    }
}

async fn get(state: AppState, uri: &str) -> (axum::http::StatusCode, Value) {
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 2048).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

#[tokio::test]
async fn health_is_200_with_body() {
    let (status, body) = get(make_state(), "/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["service"], "paavod");
    assert_eq!(body["ready"], true);
}

#[tokio::test]
async fn health_stays_200_while_draining() {
    // Spec §9.5: `/health` is liveness — must return 200 even while
    // draining, otherwise systemd / k8s probes kill us mid-drain. The
    // body MUST report `ready: false` so monitoring can still observe
    // the drain.
    let state = make_state();
    state.drain.set_draining();
    let (status, body) = get(state, "/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["service"], "paavod");
    assert_eq!(body["ready"], false);
}

#[tokio::test]
async fn ready_is_200_when_not_draining() {
    let (status, body) = get(make_state(), "/ready").await;
    assert_eq!(status, 200);
    assert_eq!(body["ready"], true);
}

#[tokio::test]
async fn ready_flips_to_503_when_draining() {
    let state = make_state();
    state.drain.set_draining();
    let (status, body) = get(state, "/ready").await;
    assert_eq!(status, 503);
    assert_eq!(body["service"], "paavod");
    assert_eq!(body["ready"], false);
}

#[tokio::test]
async fn post_jobs_returns_503_while_draining() {
    // Spec §6.3: drain returns 503 for new jobs. The stub locks this
    // in so the invariant survives until M4.2.b fills the real handler.
    let state = make_state();
    state.drain.set_draining();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/jobs")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn post_jobs_returns_501_when_not_draining() {
    // While not draining the stub returns 501 — locks in that the
    // drain check doesn't accidentally short-circuit the no-drain path.
    let app = build_router(make_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/jobs")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
}
```

- [ ] **Step 5: Run the health test**

Run: `cargo test -p paavod --test api_health`
Expected: 6 passed (`health_is_200_with_body`, `health_stays_200_while_draining`, `ready_is_200_when_not_draining`, `ready_flips_to_503_when_draining`, `post_jobs_returns_503_while_draining`, `post_jobs_returns_501_when_not_draining`).

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): axum router shell + AppState + health/ready endpoints"
```

---

#### 4.2.b: Board management routes

Spec coverage: §9.4. After the first round of review this sub-task grew
to also include the supporting work in `paavo-db` (typed `NotFound` /
`AlreadyExists` variants) and `paavo-proto` (a `BoardView` JSON shape
that exposes the operational fields §9.4 promises — last-used,
quarantine reason, etc.). Reasoning: the HTTP handlers can only return
correct status codes if the DB surfaces typed errors, and the operator
UI can only render meaningful state if the JSON wire shape includes
the fields. Both are prerequisites; bundling them keeps the milestone
atomic.

**Files (this sub-task touches three crates):**
- Modify: `crates/paavo-db/src/error.rs` (add `NotFound`, `AlreadyExists`)
- Modify: `crates/paavo-db/src/board.rs` (return typed variants)
- Test: extend `crates/paavo-db/tests/board_ops.rs`
- Create: `crates/paavo-proto/src/board.rs` (add `BoardView`)
- Test: extend `crates/paavo-proto/tests/` (or rely on use sites)
- Modify: `crates/paavod/src/routes/boards.rs` (real handlers + typed mapping + validation)
- Modify: `crates/paavod/Cargo.toml` (add `tracing` if not already — it is)
- Create: `crates/paavod/tests/api_boards.rs`

- [ ] **Step 1: Add typed `NotFound` and `AlreadyExists` variants to `paavo-db`**

Replace `crates/paavo-db/src/error.rs`:
```rust
//! Error type for paavo-db.

use thiserror::Error;

/// Errors returned by paavo-db operations.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SQLite error (catch-all for low-level rusqlite failures
    /// we don't yet pattern-match into a typed variant).
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Migration application failed.
    #[error("migration: {0}")]
    Migration(#[from] refinery::Error),
    /// JSON column failed to (de)serialize.
    #[error("json column: {0}")]
    Json(#[from] serde_json::Error),
    /// Row found but a CHECK-constrained string value was unrecognized.
    #[error("unknown enum variant for column {column}: {value}")]
    UnknownEnum {
        /// SQL column name.
        column: &'static str,
        /// Value pulled from the row.
        value: String,
    },
    /// A typed entity was looked up or mutated by id but did not exist.
    /// Surfaces to HTTP as `404 Not Found`.
    #[error("{entity} not found: {id}")]
    NotFound {
        /// Logical entity name (e.g. `"board"`, `"job"`).
        entity: &'static str,
        /// Id we looked for.
        id: String,
    },
    /// A typed entity was inserted but its primary key already exists.
    /// Surfaces to HTTP as `409 Conflict`.
    #[error("{entity} already exists: {id}")]
    AlreadyExists {
        /// Logical entity name.
        entity: &'static str,
        /// The duplicate id.
        id: String,
    },
}

/// `Result` alias used throughout paavo-db.
pub type Result<T, E = DbError> = std::result::Result<T, E>;
```

In `crates/paavo-db/src/board.rs`:

- `BoardRow::insert` must detect `SqliteFailure(code, _)` where
  `code.code == ErrorCode::ConstraintViolation` and convert to
  `DbError::AlreadyExists { entity: "board", id: spec.id.clone() }`. Keep
  every other rusqlite error wrapped in `DbError::Sqlite` (free via `?`).
- `BoardRow::quarantine` and `BoardRow::unquarantine` must inspect the
  rows-affected count from `conn.execute(...)`. If zero, return
  `Err(DbError::NotFound { entity: "board", id: id.to_string() })`.
- The same treatment for `touch_last_used`, `bump_infra_failure`,
  `reset_infra_failures` — any single-row mutation that silently no-ops
  on a missing id is a footgun. Add the check + return `NotFound`.

Concrete pattern (use for all four mutators):
```rust
let n = conn.execute("UPDATE board SET … WHERE id = ?1", params![…, id])?;
if n == 0 {
    return Err(DbError::NotFound {
        entity: "board",
        id: id.to_string(),
    });
}
Ok(())
```

Concrete pattern for `insert`:
```rust
use rusqlite::{Error as RusqliteError, ErrorCode};
match conn.execute("INSERT INTO board (…) VALUES (…)", params![…]) {
    Ok(_) => Ok(()),
    Err(RusqliteError::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => {
        Err(DbError::AlreadyExists {
            entity: "board",
            id: spec.id.clone(),
        })
    }
    Err(other) => Err(other.into()),
}
```

Extend `crates/paavo-db/tests/board_ops.rs` with:

```rust
#[test]
fn insert_duplicate_id_returns_already_exists() {
    let db = fresh_db();
    let now = chrono::Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    let err = BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap_err();
    match err {
        paavo_db::DbError::AlreadyExists { entity, id } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "mcxa266-01");
        }
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
}

#[test]
fn quarantine_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::quarantine(db.raw_conn(), "ghost", "reason").unwrap_err();
    match err {
        paavo_db::DbError::NotFound { entity, id } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "ghost");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn unquarantine_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::unquarantine(db.raw_conn(), "ghost").unwrap_err();
    assert!(matches!(err, paavo_db::DbError::NotFound { .. }));
}

#[test]
fn touch_last_used_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::touch_last_used(db.raw_conn(), "ghost", 1).unwrap_err();
    assert!(matches!(err, paavo_db::DbError::NotFound { .. }));
}
```

Run: `cargo test -p paavo-db`. Expected: existing 39 tests still pass plus 4 new ones.

- [ ] **Step 2: Add `BoardView` JSON shape to `paavo-proto`**

Spec §9.4 calls for `GET /boards` to expose "current fleet, health,
last-used". `BoardSpec` covers the first two but omits operational
fields (`last_used_at`, `quarantine_reason`, `consecutive_infra_failures`,
`created_at`). Add a richer view type in `paavo-proto` so paavod and
paavo-cli can deserialize the same shape.

In `crates/paavo-proto/src/board.rs`, append:
```rust
/// JSON shape returned by `GET /boards` and `GET /boards/:id`. Wraps a
/// `BoardSpec` with the operational fields the spec §9.4 promises:
/// last-used timestamp, quarantine reason, the infra-failure counter
/// that drives auto-quarantine, and the registration timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardView {
    /// Inlined spec fields (`#[serde(flatten)]` so the wire shape is
    /// flat: `{ "id": ..., "kind": ..., ..., "last_used_at": ..., ... }`).
    #[serde(flatten)]
    pub spec: BoardSpec,
    /// Free-form reason recorded when `spec.health == Quarantined`.
    /// `None` when the board is healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine_reason: Option<String>,
    /// Counts toward the auto-quarantine threshold
    /// (`quarantine.consecutive_infra_failures`).
    pub consecutive_infra_failures: u32,
    /// Epoch ms of the most recent successful dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
    /// Epoch ms when this board was first registered.
    pub created_at: i64,
}
```

(Re-export at `crates/paavo-proto/src/lib.rs`: add `BoardView` to the
existing `pub use board::{…};` line.)

Round-trip the new shape with a quick test in
`crates/paavo-proto/tests/board_view.rs`:
```rust
use paavo_proto::{BoardHealth, BoardSpec, BoardView, ProbeSelector};

#[test]
fn board_view_round_trips_through_json() {
    let view = BoardView {
        spec: BoardSpec {
            id: "x".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "1366".into(),
                pid: "1015".into(),
                serial: "ABC".into(),
            },
            chip_name: "X".into(),
            target_name: "T".into(),
            wiring_profile: Some("default".into()),
            health: BoardHealth::Quarantined,
        },
        quarantine_reason: Some("flaky".into()),
        consecutive_infra_failures: 3,
        last_used_at: Some(42),
        created_at: 7,
    };
    let j = serde_json::to_value(&view).unwrap();
    // Flatten: `id` is at the top level alongside `quarantine_reason`.
    assert_eq!(j["id"], "x");
    assert_eq!(j["quarantine_reason"], "flaky");
    assert_eq!(j["consecutive_infra_failures"], 3);
    assert_eq!(j["last_used_at"], 42);
    assert_eq!(j["created_at"], 7);

    let back: BoardView = serde_json::from_value(j).unwrap();
    assert_eq!(back, view);
}

#[test]
fn board_view_omits_none_quarantine_reason() {
    let view = BoardView {
        spec: BoardSpec {
            id: "x".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "1".into(),
                pid: "2".into(),
                serial: "S".into(),
            },
            chip_name: "X".into(),
            target_name: "T".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        },
        quarantine_reason: None,
        consecutive_infra_failures: 0,
        last_used_at: None,
        created_at: 0,
    };
    let j = serde_json::to_value(&view).unwrap();
    // BoardView's own Option fields use skip_serializing_if.
    assert!(j.get("quarantine_reason").is_none());
    assert!(j.get("last_used_at").is_none());
    // BoardSpec::wiring_profile does NOT use skip_serializing_if (the
    // field's serde attrs live on `BoardSelector::wiring_profile`,
    // not on `BoardSpec`), so it serializes as `null` here. Pin the
    // explicit null so a future skip_serializing_if change is caught.
    assert!(j["wiring_profile"].is_null());
}
```

Run: `cargo test -p paavo-proto`. Expected: existing tests pass + 2 new.

- [ ] **Step 3: Write the failing paavod boards test**

`crates/paavod/tests/api_boards.rs`:
```rust
use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

fn state() -> AppState {
    // Cross-platform: derive every path from the same tempdir; leak the
    // dir per the workspace convention in
    // `crates/paavo-core/tests/common/mod.rs::fresh_db`.
    let dir = tempdir().unwrap();
    let state_dir = dir.path().to_path_buf();
    let db = Db::open(state_dir.join("paavo.sqlite")).unwrap();
    std::mem::forget(dir);
    let cfg = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    };
    AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(cfg),
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
    }
}

fn sample_board_json() -> Value {
    json!({
        "id": "mcxa266-01",
        "kind": "mcxa266",
        "probe_selector": { "vid": "1366", "pid": "1015", "serial": "ABC" },
        "chip_name": "MCXA266VFL",
        "target_name": "frdm-mcx-a266",
        "wiring_profile": "default",
        "health": "healthy"
    })
}

async fn post_json(app: axum::Router, uri: &str, body: Value) -> axum::http::Response<axum::body::Body> {
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(bytes))
        .unwrap();
    app.oneshot(req).await.unwrap()
}

async fn post_empty(app: axum::Router, uri: &str) -> axum::http::Response<axum::body::Body> {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap()
}

async fn read_json(resp: axum::http::Response<axum::body::Body>) -> Value {
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn post_boards_then_get_boards_returns_full_view() {
    let s = state();
    let app = build_router(s.clone());

    let resp = post_json(app.clone(), "/boards", sample_board_json()).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
        .uri("/boards")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = read_json(resp).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    // GET /boards returns BoardView, not BoardSpec. The view exposes
    // the operational fields §9.4 promises.
    assert_eq!(v[0]["id"], "mcxa266-01");
    assert_eq!(v[0]["consecutive_infra_failures"], 0);
    assert!(v[0]["created_at"].as_i64().unwrap() > 0);
    // No quarantine, so no reason field.
    assert!(v[0].get("quarantine_reason").is_none());
    // last_used_at is None on a freshly added board.
    assert!(v[0].get("last_used_at").is_none());

    let inv = s.inventory_snapshot();
    assert_eq!(inv.len(), 1);
    assert_eq!(inv[0].id, "mcxa266-01");
}

#[tokio::test]
async fn get_boards_orders_by_id_ascending() {
    // Locks in the contract that paavo-db's `list_all` ORDER BY id ASC
    // is preserved through the HTTP layer. paavo-cli renders fleets
    // in this order.
    let s = state();
    let app = build_router(s.clone());

    let mut a = sample_board_json();
    a["id"] = json!("mcxa266-02");
    a["probe_selector"]["serial"] = json!("BBB");
    let mut b = sample_board_json();
    b["id"] = json!("mcxa266-01");
    b["probe_selector"]["serial"] = json!("AAA");
    assert_eq!(post_json(app.clone(), "/boards", a).await.status(), StatusCode::CREATED);
    assert_eq!(post_json(app.clone(), "/boards", b).await.status(), StatusCode::CREATED);

    let req = Request::builder().uri("/boards").body(axum::body::Body::empty()).unwrap();
    let v = read_json(app.oneshot(req).await.unwrap()).await;
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], "mcxa266-01");
    assert_eq!(arr[1]["id"], "mcxa266-02");
}

#[tokio::test]
async fn post_boards_rejects_duplicate_id_with_409() {
    let s = state();
    let app = build_router(s.clone());

    let resp = post_json(app.clone(), "/boards", sample_board_json()).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = post_json(app, "/boards", sample_board_json()).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn post_boards_rejects_non_healthy_with_400() {
    // §9.4: initial registration must be `Healthy`; the quarantine flow
    // requires a `reason`. Accepting `health: "quarantined"` here would
    // let a client create a quarantined board with `quarantine_reason
    // = NULL`, violating the data invariant.
    let s = state();
    let app = build_router(s.clone());

    let mut body = sample_board_json();
    body["health"] = json!("quarantined");
    let resp = post_json(app, "/boards", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn quarantine_unknown_board_returns_404() {
    let s = state();
    let app = build_router(s.clone());
    let resp = post_json(app, "/boards/ghost/quarantine", json!({"reason": "x"})).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unquarantine_unknown_board_returns_404() {
    let s = state();
    let app = build_router(s.clone());
    let resp = post_empty(app, "/boards/ghost/unquarantine").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn quarantine_rejects_empty_reason_with_400() {
    let s = state();
    let app = build_router(s.clone());
    assert_eq!(
        post_json(app.clone(), "/boards", sample_board_json()).await.status(),
        StatusCode::CREATED,
    );
    let resp = post_json(app, "/boards/mcxa266-01/quarantine", json!({"reason": "   "})).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn quarantine_and_unquarantine_flip_health_and_cache() {
    let s = state();
    // Seed directly via db so the test pins the cache-refresh contract
    // independently of `add_board`.
    paavo_db::BoardRow::insert(
        s.db.lock().raw_conn(),
        &BoardSpec {
            id: "b".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "x".into(),
                pid: "x".into(),
                serial: "x".into(),
            },
            chip_name: "x".into(),
            target_name: "x".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        },
        0,
    )
    .unwrap();
    *s.inventory.lock() = paavo_db::BoardRow::list_all(s.db.lock().raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect();

    let app = build_router(s.clone());

    let resp = post_json(
        app.clone(),
        "/boards/b/quarantine",
        json!({"reason": "broken header"}),
    )
    .await;
    assert_eq!(resp.status(), 204);
    let row = paavo_db::BoardRow::get(s.db.lock().raw_conn(), "b").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Quarantined);
    assert_eq!(row.quarantine_reason.as_deref(), Some("broken header"));
    assert_eq!(s.inventory_snapshot()[0].health, BoardHealth::Quarantined);

    let resp = post_empty(app, "/boards/b/unquarantine").await;
    assert_eq!(resp.status(), 204);
    let row = paavo_db::BoardRow::get(s.db.lock().raw_conn(), "b").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
    assert!(row.quarantine_reason.is_none());
    assert_eq!(s.inventory_snapshot()[0].health, BoardHealth::Healthy);
}
```

- [ ] **Step 4: Implement board routes with typed mapping + validation + atomic refresh**

Replace `crates/paavod/src/routes/boards.rs`:
```rust
//! /boards/* handlers.
//!
//! Lock ordering: every mutating handler takes `s.db.lock()` for the
//! mutation, drops the guard, then calls `refresh_inventory(&s)` which
//! locks `s.db` and `s.inventory` together (db → inventory) to
//! atomically read the table and replace the cache. Holding both
//! together inside `refresh_inventory` means two concurrent writers
//! cannot interleave their reads/writes and leave a stale snapshot.
//!
//! If `refresh_inventory` fails after a successful mutation, the cache
//! is briefly stale but the DB is the source of truth and the next
//! successful write (or paavod's startup hydration) will reconverge.
//! We therefore log + warn on refresh failure but still report the
//! mutation as successful — never return 500 to a caller whose write
//! actually committed.

use crate::app_state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use paavo_db::DbError;
use paavo_proto::{BoardHealth, BoardSpec, BoardView};
use serde::Deserialize;
use tracing::{error, warn};

/// Shorthand for handler results. Errors carry an HTTP status + a
/// stable text/plain message. A richer JSON envelope is a future
/// upgrade (tracked separately).
type HandlerResult<T> = Result<T, (StatusCode, String)>;

/// GET /boards. Returns `Vec<BoardView>` so callers see the same
/// operational fields the spec promises (last-used, quarantine reason,
/// infra-failure counter, created-at).
pub async fn list_boards(State(s): State<AppState>) -> HandlerResult<Json<Vec<BoardView>>> {
    let rows = paavo_db::BoardRow::list_all(s.db.lock().raw_conn()).map_err(db_to_http)?;
    let views: Vec<BoardView> = rows.into_iter().map(row_to_view).collect();
    Ok(Json(views))
}

/// POST /boards. Body is a `BoardSpec`; `health` must be `Healthy` —
/// quarantine flows through the dedicated endpoint so `quarantine_reason`
/// can never be `NULL` for a quarantined row.
pub async fn add_board(
    State(s): State<AppState>,
    Json(spec): Json<BoardSpec>,
) -> HandlerResult<StatusCode> {
    if spec.health != BoardHealth::Healthy {
        return Err((
            StatusCode::BAD_REQUEST,
            "board must be registered as `healthy`; use POST \
             /boards/:id/quarantine to quarantine after creation"
                .into(),
        ));
    }
    let now_ms = Utc::now().timestamp_millis();
    {
        let db = s.db.lock();
        paavo_db::BoardRow::insert(db.raw_conn(), &spec, now_ms).map_err(db_to_http)?;
    }
    refresh_inventory_lossy(&s);
    Ok(StatusCode::CREATED)
}

/// Body for `POST /boards/:id/quarantine`.
#[derive(Deserialize)]
pub struct QuarantineBody {
    /// Human-readable reason. Whitespace-only is rejected.
    pub reason: String,
}

/// POST /boards/:id/quarantine.
pub async fn quarantine_board(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<QuarantineBody>,
) -> HandlerResult<StatusCode> {
    let reason = body.reason.trim();
    if reason.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "`reason` is required and must not be whitespace-only".into(),
        ));
    }
    {
        let db = s.db.lock();
        paavo_db::BoardRow::quarantine(db.raw_conn(), &id, reason).map_err(db_to_http)?;
    }
    refresh_inventory_lossy(&s);
    Ok(StatusCode::NO_CONTENT)
}

/// POST /boards/:id/unquarantine.
pub async fn unquarantine_board(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<StatusCode> {
    {
        let db = s.db.lock();
        paavo_db::BoardRow::unquarantine(db.raw_conn(), &id).map_err(db_to_http)?;
    }
    refresh_inventory_lossy(&s);
    Ok(StatusCode::NO_CONTENT)
}

/// Re-read the `boards` table and replace the cached inventory. Takes
/// both locks (db then inventory) so the read+write is atomic with
/// respect to other writers. Called by `paavod::main` at startup to
/// hydrate the initial snapshot — that's why this is `pub(crate)`.
pub(crate) fn refresh_inventory(s: &AppState) -> paavo_db::Result<()> {
    let db = s.db.lock();
    let rows = paavo_db::BoardRow::list_all(db.raw_conn())?;
    let mut inv = s.inventory.lock();
    *inv = rows.into_iter().map(|r| r.spec).collect();
    Ok(())
}

/// Refresh the inventory but never fail the caller's request. If the
/// refresh fails the DB still holds the truth; the cache will
/// reconverge on the next successful mutation or on paavod restart.
fn refresh_inventory_lossy(s: &AppState) {
    if let Err(e) = refresh_inventory(s) {
        warn!(error = %e, "inventory cache refresh failed; DB is still authoritative");
    }
}

fn row_to_view(r: paavo_db::BoardRow) -> BoardView {
    BoardView {
        spec: r.spec,
        quarantine_reason: r.quarantine_reason,
        consecutive_infra_failures: r.consecutive_infra_failures,
        last_used_at: r.last_used_at,
        created_at: r.created_at,
    }
}

/// Map a `DbError` to an HTTP status + message. Typed variants
/// (`NotFound`, `AlreadyExists`) get 404/409 with their own messages;
/// everything else becomes 500 with the `Display` text (info-leak
/// risk on `Sqlite(...)` is acceptable for an internal lab tool but
/// we log the full error so it's not silently lost).
fn db_to_http(err: DbError) -> (StatusCode, String) {
    match err {
        DbError::NotFound { entity, id } => (
            StatusCode::NOT_FOUND,
            format!("{entity} not found: {id}"),
        ),
        DbError::AlreadyExists { entity, id } => (
            StatusCode::CONFLICT,
            format!("{entity} already exists: {id}"),
        ),
        other => {
            error!(error = ?other, "unexpected db error");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{other}"))
        }
    }
}
```

- [ ] **Step 5: Run the boards test**

Run: `cargo test -p paavod --test api_boards`
Expected: 8 passed (`post_boards_then_get_boards_returns_full_view`,
`get_boards_orders_by_id_ascending`, `post_boards_rejects_duplicate_id_with_409`,
`post_boards_rejects_non_healthy_with_400`, `quarantine_unknown_board_returns_404`,
`unquarantine_unknown_board_returns_404`, `quarantine_rejects_empty_reason_with_400`,
`quarantine_and_unquarantine_flip_health_and_cache`).

Also: `cargo test --workspace` to make sure nothing else broke.

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-db crates/paavo-proto crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): board management routes with typed errors and full view"
```

---

#### 4.2.c: Jobs routes — multipart submit, list, get, cancel, stream

This is large; we split into the three sub-steps that drive different test files.

##### 4.2.c.i: Multipart submit + tar persistence

This sub-task grew on review to include the supporting changes that
make a production-grade ingest possible:
- `paavod::config::ServerConfig::max_upload_bytes` — operator-tunable
  per-request body cap (default 256 MiB), wired into the `/jobs` route
  via `axum::extract::DefaultBodyLimit::max`.
- Streaming the `crate` part to a temp file in `${state_dir}/uploads/`
  with blake3 hashing in flight, then atomic rename — never buffers the
  full tar in RAM.
- TOCTOU-safe persistence: `OpenOptions::create_new(true)` so two
  concurrent identical submits cannot truncate each other; on
  `AlreadyExists` we drop our temp file (dedup hit).
- Validation BEFORE persistence rename — selector + ceiling are checked
  against an inventory snapshot first; on failure the temp file is
  unlinked so rejected submits leave no orphan tars.
- `metadata.source` removed from the wire — every HTTP submit is
  recorded as `JobSource::Cli`. The scheduler reaches `enqueue_job`
  directly (the lib call), bypassing HTTP entirely.
- Inventory snapshot moved INSIDE the `s.db.lock()` scope at the
  authoritative enqueue, eliminating the TOCTOU window between
  early-validate and enqueue.
- `tar_path` stored as a UTF-8 string only after explicit conversion;
  non-UTF-8 paths fail with a clear 500 instead of silent corruption.

**Setup before TDD:** add the runtime deps that the new handler pulls
in. In `crates/paavod/Cargo.toml` `[dependencies]`:
```toml
paavo-build = { workspace = true }
blake3      = { workspace = true }
```
(`paavo-core`, `chrono`, `tracing`, `tokio` are already deps.)

- [ ] **Step 1: Add `max_upload_bytes` to `paavod::config::ServerConfig`**

In `crates/paavod/src/config.rs`, extend `ServerConfig`:
```rust
/// `[server]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// `host:port`. Required.
    pub bind: String,
    /// Daemon state dir.
    pub state_dir: std::path::PathBuf,
    /// Per-request multipart body cap for `POST /jobs` (bytes).
    /// Default 256 MiB. Raise for fleets with large vendored deps.
    #[serde(default = "default_max_upload_bytes")]
    pub max_upload_bytes: usize,
}

fn default_max_upload_bytes() -> usize {
    256 * 1024 * 1024
}
```

Extend `crates/paavod/tests/config_loading.rs` with:
```rust
#[test]
fn server_max_upload_bytes_defaults_to_256_mib() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    // Use SAMPLE which omits `max_upload_bytes`.
    fs::write(&p, SAMPLE).unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.server.max_upload_bytes, 256 * 1024 * 1024);
}

#[test]
fn server_max_upload_bytes_can_be_overridden() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    let toml = SAMPLE.replace(
        "[web]",
        "max_upload_bytes = 1048576\n\n[web]",
    );
    fs::write(&p, toml).unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.server.max_upload_bytes, 1_048_576);
}
```

Run: `cargo test -p paavod --test config_loading`. Expect 8 passed
(6 existing + 2 new).

- [ ] **Step 2: Add a `validate_enqueue` helper to `paavo-core`**

The HTTP handler needs to reject `SelectorNeverMatches` and
`OverCeiling` BEFORE persisting the tar; today both checks live inside
`enqueue_job`. Add a separate validator that does just the cheap
pre-persist checks so the handler can call it twice — once for early
fail-fast, then implicitly again inside `enqueue_job` for the
authoritative under-lock check.

In `crates/paavo-core/src/enqueue.rs`, add:
```rust
/// Pre-validate the parts of an enqueue request that do NOT require
/// touching the DB. Used by the HTTP layer to fail fast BEFORE
/// persisting the uploaded tar so rejected submits leave no orphan
/// files on disk. `enqueue_job` re-runs the same checks under the DB
/// lock for the authoritative decision; this helper is purely an
/// optimization for the rejection path.
pub fn validate_enqueue(
    req: &EnqueueRequest,
    inventory: &[BoardSpec],
) -> Result<()> {
    if req.hard_max_ms > req.daemon_ceiling_ms {
        return Err(CoreError::OverCeiling {
            requested: req.hard_max_ms,
            ceiling: req.daemon_ceiling_ms,
        });
    }
    if !selector_matches_any(&req.board_selector, inventory) {
        return Err(CoreError::SelectorNeverMatches(req.board_selector.clone()));
    }
    Ok(())
}
```

Re-export in `crates/paavo-core/src/lib.rs` next to `enqueue_job`:
```rust
pub use enqueue::{enqueue_job, validate_enqueue, EnqueueRequest};
```

Refactor `enqueue_job` to delegate to `validate_enqueue` so we have
exactly one definition of the rules:
```rust
pub fn enqueue_job(
    conn: &Connection,
    inventory: &[BoardSpec],
    req: EnqueueRequest,
    now_ms: i64,
) -> Result<JobId> {
    validate_enqueue(&req, inventory)?;
    let new = paavo_db::NewJob {
        id: req.job_id,
        priority: req.priority,
        submitter: req.submitter,
        source: req.source,
        board_selector: req.board_selector,
        inactivity_timeout_ms: req.inactivity_timeout_ms,
        hard_max_ms: req.hard_max_ms,
        tar_blake3: req.tar_blake3,
        tar_path: req.tar_path,
    };
    paavo_db::JobRow::insert(conn, &new, now_ms)?;
    Ok(req.job_id)
}
```

Add a unit test in `crates/paavo-core/tests/enqueue.rs` (or alongside
the existing enqueue tests) for `validate_enqueue`:
```rust
#[test]
fn validate_enqueue_rejects_over_ceiling_without_db() {
    let req = default_enqueue_request(JobSource::Cli);
    // Bump request above ceiling.
    let mut req = req;
    req.hard_max_ms = req.daemon_ceiling_ms + 1;
    let inventory = vec![sample_board_spec()];
    let err = paavo_core::validate_enqueue(&req, &inventory).unwrap_err();
    assert!(matches!(err, paavo_core::CoreError::OverCeiling { .. }));
}

#[test]
fn validate_enqueue_rejects_unmatched_selector_without_db() {
    let req = default_enqueue_request(JobSource::Cli);
    let inventory: Vec<paavo_proto::BoardSpec> = vec![];
    let err = paavo_core::validate_enqueue(&req, &inventory).unwrap_err();
    assert!(matches!(err, paavo_core::CoreError::SelectorNeverMatches(_)));
}
```

Run: `cargo test -p paavo-core`. Expect existing tests still pass + 2
new.

- [ ] **Step 3: Failing test for submit**

Replace `crates/paavod/tests/api_jobs.rs`:
```rust
use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, JobSource, JobState, ProbeSelector};
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

const BOUNDARY: &str = "----paavotest9999";

fn state_with_upload_cap(tmp_root: &std::path::Path, max_upload_bytes: usize) -> AppState {
    let db = Db::open(tmp_root.join("paavo.sqlite")).unwrap();
    let inv = vec![BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "x".into(),
            pid: "x".into(),
            serial: "x".into(),
        },
        chip_name: "x".into(),
        target_name: "x".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    }];
    paavo_db::BoardRow::insert(db.raw_conn(), &inv[0], 0).unwrap();

    let cfg = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir: tmp_root.to_path_buf(),
            max_upload_bytes,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    };

    let sd = paavod::state_dir::StateDir::from_root(tmp_root);
    sd.ensure_dirs().unwrap();

    AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(cfg),
        inventory: Arc::new(Mutex::new(inv)),
        drain: DrainState::default(),
    }
}

fn state(tmp_root: &std::path::Path) -> AppState {
    state_with_upload_cap(tmp_root, 256 * 1024 * 1024)
}

fn make_multipart_body(tar_bytes: &[u8], meta_json: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"metadata\"\r\n");
    body.extend(b"Content-Type: application/json\r\n\r\n");
    body.extend(meta_json.as_bytes());
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"crate\"; filename=\"crate.tar\"\r\n");
    body.extend(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend(tar_bytes);
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn submit_request(body: Vec<u8>) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/jobs")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(axum::body::Body::from(body))
        .unwrap()
}

fn default_meta() -> Value {
    json!({
        "priority": "interactive",
        "submitter": "felipe",
        "board_selector": { "kind": "mcxa266" },
        "inactivity_timeout_ms": 120000,
        "hard_max_ms": 900000
    })
}

#[tokio::test]
async fn post_jobs_accepts_multipart_and_persists_tar() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let body = make_multipart_body(b"hello tar bytes", &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024).await.unwrap()).unwrap();
    let job_id = v["job_id"].as_str().unwrap();

    // Job row was inserted as Cli source (server-side forced).
    let id: paavo_proto::JobId = job_id.parse().unwrap();
    let row = paavo_db::JobRow::get(s.db.lock().raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Submitted);
    assert_eq!(row.source, JobSource::Cli);
    let upload_path = std::path::Path::new(&row.tar_path);
    assert!(upload_path.is_file(), "expected tar at {upload_path:?}");
}

#[tokio::test]
async fn post_jobs_forces_source_to_cli_even_if_client_sends_scheduler() {
    // Defect 5 from review: client can't claim Scheduler source over HTTP
    // (which would unlock the 4h default hard_max). Even if the wire
    // body includes `"source": "scheduler"`, the server records it as
    // Cli. The wire schema has `#[serde(deny_unknown_fields)]` so any
    // `source` field actually 400s before we get this far, but pin the
    // server-side override semantics with a separate assertion.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let body = make_multipart_body(b"hi", &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024).await.unwrap()).unwrap();
    let id: paavo_proto::JobId = v["job_id"].as_str().unwrap().parse().unwrap();
    let row = paavo_db::JobRow::get(s.db.lock().raw_conn(), &id).unwrap();
    assert_eq!(row.source, JobSource::Cli);
}

#[tokio::test]
async fn post_jobs_rejects_unknown_metadata_field_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let mut meta = default_meta();
    meta["source"] = json!("scheduler"); // not a known field
    let body = make_multipart_body(b"x", &meta.to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_jobs_dedups_identical_tar_on_second_submit() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let body = make_multipart_body(b"dedup payload", &default_meta().to_string());

    let r1 = app.clone().oneshot(submit_request(body.clone())).await.unwrap();
    assert_eq!(r1.status(), StatusCode::ACCEPTED);
    let v1: Value = serde_json::from_slice(&to_bytes(r1.into_body(), 1024).await.unwrap()).unwrap();
    let job1: paavo_proto::JobId = v1["job_id"].as_str().unwrap().parse().unwrap();
    let row1 = paavo_db::JobRow::get(s.db.lock().raw_conn(), &job1).unwrap();
    let mtime1 = std::fs::metadata(&row1.tar_path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let r2 = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(r2.status(), StatusCode::ACCEPTED);
    let v2: Value = serde_json::from_slice(&to_bytes(r2.into_body(), 1024).await.unwrap()).unwrap();
    let job2: paavo_proto::JobId = v2["job_id"].as_str().unwrap().parse().unwrap();
    let row2 = paavo_db::JobRow::get(s.db.lock().raw_conn(), &job2).unwrap();
    assert_eq!(row1.tar_path, row2.tar_path, "same blake3 → same path");
    let mtime2 = std::fs::metadata(&row2.tar_path).unwrap().modified().unwrap();
    assert_eq!(mtime1, mtime2, "second submit must NOT rewrite the file");
}

#[tokio::test]
async fn post_jobs_rejects_impossible_selector_with_400_and_leaves_no_orphan_tar() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let mut meta = default_meta();
    meta["board_selector"]["kind"] = json!("no-such-board");
    let body = make_multipart_body(b"x", &meta.to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // The upload directory must be empty (or contain only the temp file
    // already cleaned up). A rejected submit MUST NOT leak an orphan tar.
    let uploads = tmp.path().join("uploads");
    let entries: Vec<_> = std::fs::read_dir(&uploads)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tar")) // ignore any non-tar artifacts
        .collect();
    assert!(
        entries.is_empty(),
        "expected no orphan .tar in {uploads:?}, found: {entries:?}"
    );
}

#[tokio::test]
async fn post_jobs_rejects_over_ceiling_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    // Daemon ceiling = 8h = 28_800_000 ms; ask for 9h.
    let mut meta = default_meta();
    meta["hard_max_ms"] = json!(32_400_000u64);
    let body = make_multipart_body(b"x", &meta.to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_jobs_rejects_missing_metadata_part_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let mut body = Vec::new();
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"crate\"; filename=\"crate.tar\"\r\n");
    body.extend(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend(b"hi");
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}--\r\n").as_bytes());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_jobs_rejects_while_draining_with_503() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    s.drain.set_draining();
    let app = build_router(s);
    let body = make_multipart_body(b"x", &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn post_jobs_accepts_large_tar_above_default_2mib() {
    // Defect 1 from review: axum's default body limit is 2 MiB. We must
    // override it via the `[server] max_upload_bytes` knob. Submit 5 MiB
    // to prove the override.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path()); // default 256 MiB cap
    let app = build_router(s.clone());
    let big: Vec<u8> = vec![b'x'; 5 * 1024 * 1024];
    let body = make_multipart_body(&big, &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn post_jobs_rejects_oversized_tar_with_413() {
    let tmp = tempdir().unwrap();
    // Set a tight 64 KiB cap and submit 256 KiB.
    let s = state_with_upload_cap(tmp.path(), 64 * 1024);
    let app = build_router(s);
    let big: Vec<u8> = vec![b'x'; 256 * 1024];
    let body = make_multipart_body(&big, &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
```

- [ ] **Step 4: Implement `POST /jobs` — streaming, validated, source-locked**

Replace `crates/paavod/src/routes/jobs.rs`:
```rust
//! /jobs/* handlers.

use crate::app_state::AppState;
use crate::state_dir::StateDir;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use paavo_core::{enqueue_job, validate_enqueue, EnqueueRequest};
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::{error, warn};

/// JSON metadata part on `POST /jobs`. `source` is NOT here — every
/// HTTP submit is recorded as `JobSource::Cli`; the scheduler reaches
/// `enqueue_job` directly. Unknown fields are rejected with 400 so
/// the wire schema is unambiguous.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostJobMetadata {
    /// Scheduler priority.
    pub priority: Priority,
    /// Free text id; no auth.
    pub submitter: String,
    /// Selector.
    pub board_selector: BoardSelector,
    /// Optional inactivity override (ms). Defaults to
    /// `timeouts.default_inactivity_s * 1000`.
    #[serde(default)]
    pub inactivity_timeout_ms: Option<u64>,
    /// Optional hard-max override (ms). Defaults to
    /// `timeouts.default_ad_hoc_hard_max_s * 1000`.
    #[serde(default)]
    pub hard_max_ms: Option<u64>,
}

/// 202 response body.
#[derive(Debug, Serialize)]
pub struct AcceptedBody {
    /// Newly assigned job id.
    pub job_id: String,
}

type HandlerResult<T> = Result<T, (StatusCode, String)>;

/// POST /jobs.
pub async fn post_jobs(
    State(s): State<AppState>,
    mut multipart: Multipart,
) -> HandlerResult<(StatusCode, Json<AcceptedBody>)> {
    if s.drain.is_draining() {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "paavod is draining".into()));
    }

    let job_id = JobId::new();
    let sd = StateDir::from_root(&s.config.server.state_dir);
    sd.ensure_dirs()
        .map_err(|e| internal("ensure_dirs", e.to_string()))?;

    // Reserve a temp file path under uploads/; the JobId disambiguates
    // concurrent uploaders. We stream the `crate` part directly into this
    // file and hash with blake3 in flight, then atomically rename to
    // `<blake>.tar` after validation succeeds.
    let tmp_path = sd.uploads_dir.join(format!(".tmp-{}.tar", job_id));
    // Guard so any early return (validation failure, multipart error)
    // unlinks the temp file even though the handler is `async`.
    let mut cleanup = TempCleanup::new(tmp_path.clone());

    let mut metadata: Option<PostJobMetadata> = None;
    let mut crate_seen = false;
    let mut hasher = blake3::Hasher::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(bad_request("multipart"))?
    {
        match field.name() {
            Some("metadata") => {
                let bytes = field.bytes().await.map_err(bad_request("metadata"))?;
                let parsed: PostJobMetadata =
                    serde_json::from_slice(&bytes).map_err(bad_request("metadata"))?;
                metadata = Some(parsed);
            }
            Some("crate") => {
                if crate_seen {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "duplicate `crate` part".into(),
                    ));
                }
                crate_seen = true;
                let mut file = tokio::fs::File::create(&tmp_path)
                    .await
                    .map_err(|e| internal("create tmp", e.to_string()))?;
                while let Some(chunk) = field.chunk().await.map_err(bad_request("crate"))? {
                    hasher.update(&chunk);
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| internal("write tmp", e.to_string()))?;
                }
                file.flush()
                    .await
                    .map_err(|e| internal("flush tmp", e.to_string()))?;
            }
            _ => {} // ignore unknown fields silently
        }
    }

    let metadata = metadata.ok_or((StatusCode::BAD_REQUEST, "missing metadata part".into()))?;
    if !crate_seen {
        return Err((StatusCode::BAD_REQUEST, "missing crate part".into()));
    }
    let blake = hasher.finalize().to_hex().to_string();

    // Resolve defaults from config.
    let tcfg = &s.config.timeouts;
    let inactivity_timeout_ms = metadata
        .inactivity_timeout_ms
        .unwrap_or(tcfg.default_inactivity_s * 1_000);
    let hard_max_ms = metadata
        .hard_max_ms
        .unwrap_or(tcfg.default_ad_hoc_hard_max_s * 1_000);
    let daemon_ceiling_ms = tcfg.daemon_ceiling_s * 1_000;

    // Validate selector + ceiling BEFORE rename so a 400 leaves no
    // orphan tar on disk. Inventory snapshot here is informational —
    // the authoritative check runs inside enqueue_job under the db
    // lock. The race (board appears/disappears between this check and
    // the enqueue) is acceptable: at worst a valid submit gets a
    // false 400, or a fail-fast 400 slips into an enqueue that the
    // authoritative check rejects with the same error class.
    let pre_req = EnqueueRequest {
        job_id,
        priority: metadata.priority,
        submitter: metadata.submitter.clone(),
        // Server forces source = Cli. The wire schema rejects the field
        // entirely (deny_unknown_fields), but we override here too as
        // defense in depth.
        source: JobSource::Cli,
        board_selector: metadata.board_selector.clone(),
        inactivity_timeout_ms,
        hard_max_ms,
        tar_blake3: String::new(),
        tar_path: String::new(),
        daemon_ceiling_ms,
    };
    {
        let inventory = s.inventory_snapshot();
        validate_enqueue(&pre_req, &inventory).map_err(core_to_http)?;
    }

    // Atomically rename .tmp-<jobid>.tar → <blake>.tar. If the dest
    // already exists (dedup hit) we unlink our temp file — the existing
    // copy is content-identical and keeps the build cache warm.
    let final_path = sd.uploads_dir.join(format!("{blake}.tar"));
    let final_path_str = path_to_utf8(&final_path)?;
    if final_path.is_file() {
        // Dedup hit. Drop our temp via the cleanup guard.
        cleanup.disarm_into(None);
    } else {
        match tokio::fs::rename(&tmp_path, &final_path).await {
            Ok(()) => cleanup.disarm_into(None),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Won by a concurrent submitter; drop our temp.
                cleanup.disarm_into(None);
            }
            Err(e) => return Err(internal("rename tmp", e.to_string())),
        }
    }

    // Enqueue under the db lock with an inventory snapshot taken in the
    // same critical section.
    let now_ms = Utc::now().timestamp_millis();
    let req = EnqueueRequest {
        job_id,
        priority: pre_req.priority,
        submitter: pre_req.submitter,
        source: JobSource::Cli,
        board_selector: pre_req.board_selector,
        inactivity_timeout_ms,
        hard_max_ms,
        tar_blake3: blake,
        tar_path: final_path_str,
        daemon_ceiling_ms,
    };
    let inserted = {
        let db = s.db.lock();
        let inventory = s.inventory.lock().clone();
        enqueue_job(db.raw_conn(), &inventory, req, now_ms).map_err(core_to_http)?
    };
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedBody {
            job_id: inserted.to_string(),
        }),
    ))
}

/// RAII guard that unlinks `path` on drop unless `disarm_into` was
/// called. Used to clean up the streaming temp file on any early-return
/// error path (validation failure, multipart error, mid-stream I/O fault).
struct TempCleanup {
    path: Option<std::path::PathBuf>,
}

impl TempCleanup {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path: Some(path) }
    }
    /// Mark the temp file as handled (renamed away, or intentionally
    /// dropped on a dedup hit). The Drop impl becomes a no-op.
    fn disarm_into(&mut self, _: Option<()>) {
        self.path = None;
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            if let Err(e) = std::fs::remove_file(&p) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(path = %p.display(), error = %e, "failed to clean up temp upload");
                }
            }
        }
    }
}

fn bad_request<E: std::fmt::Display>(
    stage: &'static str,
) -> impl FnOnce(E) -> (StatusCode, String) {
    move |e| (StatusCode::BAD_REQUEST, format!("{stage}: {e}"))
}

fn internal(stage: &'static str, msg: String) -> (StatusCode, String) {
    error!(stage, msg = %msg, "post_jobs internal error");
    (StatusCode::INTERNAL_SERVER_ERROR, msg)
}

fn path_to_utf8(p: &std::path::Path) -> HandlerResult<String> {
    p.to_str().map(|s| s.to_string()).ok_or_else(|| {
        internal(
            "path_to_utf8",
            format!("non-UTF-8 upload path: {}", p.display()),
        )
    })
}

fn core_to_http(e: paavo_core::CoreError) -> (StatusCode, String) {
    use paavo_core::CoreError::*;
    match e {
        SelectorNeverMatches(_) | OverCeiling { .. } | NotCancellable { .. } => {
            (StatusCode::BAD_REQUEST, format!("{e}"))
        }
        Db(_) | Build(_) | Io(_) => {
            error!(error = %e, "post_jobs internal error");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
        }
    }
}

/// GET /jobs?state=...&limit=... — implemented in 4.2.c.ii.
pub async fn list_jobs(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "GET /jobs is wired in 4.2.c.ii")
}

/// GET /jobs/:id — implemented in 4.2.c.ii.
pub async fn get_job(_state: State<AppState>, _id: Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "GET /jobs/:id is wired in 4.2.c.ii",
    )
}

/// POST /jobs/:id/cancel — implemented in 4.2.c.ii.
pub async fn cancel_job(_state: State<AppState>, _id: Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /jobs/:id/cancel is wired in 4.2.c.ii",
    )
}

/// GET /jobs/:id/stream — implemented in 4.2.c.iii.
pub async fn stream_job(_state: State<AppState>, _id: Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "GET /jobs/:id/stream is wired in 4.2.c.iii",
    )
}
```

- [ ] **Step 5: Wire the per-route body limit in `build_router`**

In `crates/paavod/src/app.rs`, apply
`axum::extract::DefaultBodyLimit::max(s.config.server.max_upload_bytes)`
to the `/jobs` POST route via a layer. The simplest shape that works
in axum 0.7:

```rust
//! axum app constructor.

use crate::app_state::AppState;
use crate::routes;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

/// Build the axum Router with all routes mounted.
pub fn build_router(state: AppState) -> Router {
    let max_upload_bytes = state.config.server.max_upload_bytes;
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/ready", get(routes::health::ready))
        .route(
            "/jobs",
            post(routes::jobs::post_jobs)
                .layer(DefaultBodyLimit::max(max_upload_bytes))
                .get(routes::jobs::list_jobs),
        )
        .route("/jobs/:id", get(routes::jobs::get_job))
        .route("/jobs/:id/cancel", post(routes::jobs::cancel_job))
        .route("/jobs/:id/stream", get(routes::jobs::stream_job))
        .route(
            "/boards",
            get(routes::boards::list_boards).post(routes::boards::add_board),
        )
        .route(
            "/boards/:id/quarantine",
            post(routes::boards::quarantine_board),
        )
        .route(
            "/boards/:id/unquarantine",
            post(routes::boards::unquarantine_board),
        )
        .with_state(state)
}
```

- [ ] **Step 6: Update `api_health.rs`**

Remove `post_jobs_returns_501_when_not_draining` AND
`post_jobs_returns_503_while_draining` — the new handler responds to
empty bodies with 400 (multipart parser failure), not 503. The drain
semantics are pinned by `api_jobs.rs::post_jobs_rejects_while_draining_with_503`
which sends a valid multipart body.

- [ ] **Step 7: Run all the tests**

```pwsh
cargo test -p paavo-core
cargo test -p paavod
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected paavod tests:
- `api_jobs`: 9 passed (the 7 from the test file above + the 2 new
  large-tar tests).
- `api_health`: 4 passed (the two removed above are gone).
- `api_boards`: 8 passed (unchanged).
- `config_loading`: 8 passed (6 existing + 2 new for max_upload_bytes).

- [ ] **Step 8: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-core crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): POST /jobs multipart submit — streaming, validated, source-locked"
```

---

##### 4.2.c.ii: List / get / cancel

- [ ] **Step 1: Add the tests** (append to `crates/paavod/tests/api_jobs.rs`):
```rust
#[tokio::test]
async fn get_job_returns_row() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    // Insert directly.
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        &s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
        },
        0,
    )
    .unwrap();
    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["id"], id.to_string());
    assert_eq!(v["state"], "submitted");
}

#[tokio::test]
async fn cancel_submitted_job_returns_204() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        &s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
        },
        0,
    )
    .unwrap();
    let app = build_router(s.clone());
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{id}/cancel"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    let row = paavo_db::JobRow::get(&s.db.lock().raw_conn(), &id).unwrap();
    assert_eq!(row.state, paavo_proto::JobState::Aborted);
}

#[tokio::test]
async fn list_jobs_filters_by_state() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    for _ in 0..3 {
        let id = paavo_proto::JobId::new();
        paavo_db::JobRow::insert(
            &s.db.lock().raw_conn(),
            &paavo_db::NewJob {
                id,
                priority: paavo_proto::Priority::Interactive,
                submitter: "x".into(),
                source: paavo_proto::JobSource::Cli,
                board_selector: paavo_proto::BoardSelector { kind: "mcxa266".into(), instance: None, wiring_profile: None },
                inactivity_timeout_ms: 120_000,
                hard_max_ms: 900_000,
                tar_blake3: "x".into(),
                tar_path: "/tmp/x.tar".into(),
            },
            0,
        )
        .unwrap();
    }
    let app = build_router(s);
    let req = Request::builder()
        .uri("/jobs?state=submitted&limit=2")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
}
```

- [ ] **Step 2: Implement the three handlers**

Replace the three `NOT_IMPLEMENTED` stubs in `crates/paavod/src/routes/jobs.rs` with:
```rust
/// GET /jobs?state=&limit=.
pub async fn list_jobs(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<paavo_db::JobRow>>, (StatusCode, String)> {
    let limit: u32 = q
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
        .min(500);
    let db = s.db.lock();
    let rows = if let Some(state_str) = q.get("state") {
        let state = parse_state(state_str)?;
        paavo_db::JobRow::list_by_state(db.raw_conn(), state, limit)
    } else {
        paavo_db::JobRow::list_by_state(db.raw_conn(), paavo_proto::JobState::Submitted, limit)
    }
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(rows))
}

fn parse_state(s: &str) -> Result<paavo_proto::JobState, (StatusCode, String)> {
    use paavo_proto::JobState::*;
    Ok(match s {
        "submitted" => Submitted,
        "building" => Building,
        "running" => Running,
        "passed" => Passed,
        "failed" => Failed,
        "timedout" => TimedOut,
        "aborted" => Aborted,
        _ => return Err((StatusCode::BAD_REQUEST, format!("unknown state: {s}"))),
    })
}

/// GET /jobs/:id.
pub async fn get_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<paavo_db::JobRow>, (StatusCode, String)> {
    let id: JobId = id
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid job id".into()))?;
    let db = s.db.lock();
    match paavo_db::JobRow::find(db.raw_conn(), &id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        Some(row) => Ok(Json(row)),
        None => Err((StatusCode::NOT_FOUND, "no such job".into())),
    }
}

/// POST /jobs/:id/cancel.
pub async fn cancel_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id: JobId = match id.parse() {
        Ok(j) => j,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid id").into_response(),
    };
    let res = {
        let db = s.db.lock();
        paavo_core::cancel_if_submitted(db.raw_conn(), &id)
    };
    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(paavo_core::CoreError::NotCancellable { state }) => {
            // Building/Running: in M4.3 we'll send a signal to the worker.
            // For now, return 409 so the API is honest.
            (
                StatusCode::CONFLICT,
                format!("not cancellable in state {state:?}"),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
```

- [ ] **Step 3: Make `paavo_db::JobRow` serialize**

Add `#[derive(serde::Serialize)]` to `JobRow` in `crates/paavo-db/src/job.rs`. That requires adding `serde` to its `dependencies` (already there per Task 0.2). All fields are already serde-compatible types.

- [ ] **Step 4: Run tests**

Run: `cargo test -p paavod --test api_jobs`
Expected: 5 passed (the original 2 plus the 3 new ones).

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod crates/paavo-db
git -C D:\workspace\paavo commit -m "feat(paavod): GET /jobs[?state=], GET /jobs/:id, POST /jobs/:id/cancel"
```

---

##### 4.2.c.iii: Job log stream (NDJSON long-poll)

This handler needs a live source of log frames keyed by job id. We add an `inbox` to `AppState` plus a "completed" terminal frame marker.

- [ ] **Step 1: Add a JobLogs broker to AppState**

`crates/paavod/src/job_logs.rs`:
```rust
//! Per-job broadcast of log frames + terminal marker. In-memory only;
//! historical logs are read from sqlite.

use paavo_proto::{JobId, JobOutcome, LogFrame};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// One streamable event on the live channel.
#[derive(Debug, Clone)]
pub enum LiveEvent {
    /// One log frame.
    Frame(LogFrame),
    /// Terminal outcome — closes the stream.
    Terminal(JobOutcome),
}

/// Per-job broadcaster.
#[derive(Clone)]
pub struct JobLogsBroker {
    inner: Arc<Mutex<HashMap<JobId, broadcast::Sender<LiveEvent>>>>,
}

impl JobLogsBroker {
    /// Construct an empty broker.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Subscribe to (or create) the channel for `id`. Capacity 256.
    pub fn subscribe(&self, id: JobId) -> broadcast::Receiver<LiveEvent> {
        let mut map = self.inner.lock();
        let sender = map
            .entry(id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone();
        sender.subscribe()
    }

    /// Publish a frame.
    pub fn publish(&self, id: JobId, event: LiveEvent) {
        let map = self.inner.lock();
        if let Some(s) = map.get(&id) {
            let _ = s.send(event);
        }
    }

    /// Drop the channel after a terminal event has been published, so memory
    /// doesn't grow unbounded.
    pub fn finalize(&self, id: JobId) {
        self.inner.lock().remove(&id);
    }
}

impl Default for JobLogsBroker {
    fn default() -> Self {
        Self::new()
    }
}
```

Add `pub mod job_logs;` to `crates/paavod/src/lib.rs`. Add `pub job_logs: crate::job_logs::JobLogsBroker,` to `AppState` and initialize in `make_state`/test helpers with `JobLogsBroker::new()`.

- [ ] **Step 2: Stream handler**

Replace `stream_job` in `crates/paavod/src/routes/jobs.rs`:
```rust
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{self, Stream, StreamExt};
use std::convert::Infallible;

/// GET /jobs/:id/stream — NDJSON-style SSE stream. Each event payload is one
/// JSON line: either `{"type":"frame","frame":...}` or
/// `{"type":"terminal","outcome":...}`. Stream ends after the terminal event.
pub async fn stream_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id: JobId = match id.parse() {
        Ok(i) => i,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid id").into_response(),
    };

    // 1. Historical frames from sqlite.
    let historical: Vec<paavo_proto::LogFrame> = {
        use paavo_db::LogFrameDb;
        let db = s.db.lock();
        paavo_proto::LogFrame::list(db.raw_conn(), &id, 0, 10_000).unwrap_or_default()
    };

    // 2. Check terminal state. If already terminal, emit historical + terminal
    // outcome and close.
    let terminal_already: Option<paavo_proto::JobOutcome> = {
        let db = s.db.lock();
        paavo_db::JobRow::find(db.raw_conn(), &id)
            .ok()
            .flatten()
            .and_then(|r| r.outcome.filter(|_| r.state.is_terminal()))
    };

    if let Some(outcome) = terminal_already {
        let frames = historical.clone();
        let s = stream::iter(
            frames
                .into_iter()
                .map(|f| serde_json::json!({"type":"frame","frame": f}))
                .chain(std::iter::once(
                    serde_json::json!({"type":"terminal","outcome": outcome}),
                ))
                .map(|v| SseEvent::default().data(v.to_string()))
                .map(Ok::<_, Infallible>),
        );
        return Sse::new(s).keep_alive(KeepAlive::default()).into_response();
    }

    // 3. Live subscriber.
    let mut rx = s.job_logs.subscribe(id);
    let live = async_stream::stream! {
        for h in historical {
            yield Ok::<_, Infallible>(SseEvent::default().data(
                serde_json::json!({"type":"frame","frame": h}).to_string(),
            ));
        }
        loop {
            match rx.recv().await {
                Ok(crate::job_logs::LiveEvent::Frame(f)) => {
                    yield Ok(SseEvent::default().data(
                        serde_json::json!({"type":"frame","frame": f}).to_string(),
                    ));
                }
                Ok(crate::job_logs::LiveEvent::Terminal(o)) => {
                    yield Ok(SseEvent::default().data(
                        serde_json::json!({"type":"terminal","outcome": o}).to_string(),
                    ));
                    break;
                }
                Err(_) => break,
            }
        }
    };
    Sse::new(live).keep_alive(KeepAlive::default()).into_response()
}
```

Add deps to `crates/paavod/Cargo.toml` under `[dependencies]`:
```toml
async-stream = "0.3"
```

Add to workspace deps and reference here.

- [ ] **Step 3: Test the stream (terminal case)**

Append to `crates/paavod/tests/api_jobs.rs`:
```rust
#[tokio::test]
async fn stream_terminal_returns_historical_plus_outcome() {
    use paavo_db::LogFrameDb;
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());

    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        &s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector { kind: "mcxa266".into(), instance: None, wiring_profile: None },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
        },
        0,
    )
    .unwrap();
    paavo_proto::LogFrame::append_batch(
        &s.db.lock().raw_conn(),
        &id,
        &[paavo_proto::LogFrame {
            seq: 0, ts_us: 0,
            level: paavo_proto::LogLevel::Info,
            target: None, message: "hi".into(),
        }],
    )
    .unwrap();
    paavo_db::JobRow::finalize(
        &s.db.lock().raw_conn(),
        &id,
        &paavo_db::OutcomeRecord {
            state: paavo_proto::JobState::Passed,
            outcome: paavo_proto::JobOutcome::Passed,
            finished_at_ms: 1,
        },
    )
    .unwrap();

    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}/stream"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("\"frame\""), "{text}");
    assert!(text.contains("\"terminal\""), "{text}");
    // JobOutcome::Passed serializes as the bare string "passed" (externally
    // tagged enum unit variant).
    assert!(text.contains("\"outcome\":\"passed\""), "{text}");
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p paavod --test api_jobs`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod Cargo.toml
git -C D:\workspace\paavo commit -m "feat(paavod): /jobs/:id/stream SSE handler (historical + terminal)"
```

---

### Task 4.3: paavod — worker pool + dispatch loop + nightly cron + SIGTERM drain

Spec coverage: §4.3 (one BoardWorker per board, OS thread), §5.3–§5.4 (dispatch + cancel paths), §6.3 (SIGTERM drain), §13 (nightly cron).

**Files:**
- Create: `crates/paavod/src/dispatch.rs`
- Create: `crates/paavod/src/cancellation.rs`
- Create: `crates/paavod/src/cron.rs`
- Create: `crates/paavod/src/shutdown.rs`
- Test: `crates/paavod/tests/dispatch_loop.rs`

#### 4.3.a: Cancellation registry

- [ ] **Step 1: Implement registry**

`crates/paavod/src/cancellation.rs`:
```rust
//! Per-job cancel-signal registry. Each running job has an entry; cancel
//! handler signals through it. Used by the dispatch loop to satisfy
//! `POST /jobs/:id/cancel` while the job is Building or Running.

use crossbeam_channel::Sender;
use paavo_proto::JobId;
use paavo_runner::RunCommand;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Cancel signal handle keyed by job id.
#[derive(Clone, Default)]
pub struct CancellationRegistry {
    inner: Arc<Mutex<HashMap<JobId, Sender<RunCommand>>>>,
}

impl CancellationRegistry {
    /// Register a fresh sender for a job that's about to run.
    pub fn register(&self, id: JobId, tx: Sender<RunCommand>) {
        self.inner.lock().insert(id, tx);
    }

    /// Drop the sender after the job has finalized.
    pub fn unregister(&self, id: &JobId) {
        self.inner.lock().remove(id);
    }

    /// Try to send a Cancel/DaemonShutdown to a running job.
    pub fn signal(&self, id: &JobId, cmd: RunCommand) -> bool {
        if let Some(tx) = self.inner.lock().get(id) {
            tx.send(cmd).is_ok()
        } else {
            false
        }
    }

    /// Signal every registered job (used during shutdown).
    pub fn signal_all(&self, cmd: RunCommand) {
        for (_id, tx) in self.inner.lock().iter() {
            let _ = tx.send(cmd);
        }
    }
}
```

Add to `lib.rs`: `pub mod cancellation;`. Add `pub cancellation: cancellation::CancellationRegistry,` to `AppState` and default-construct it in tests.

- [ ] **Step 2: Wire cancel handler to use the registry**

In `crates/paavod/src/routes/jobs.rs::cancel_job`, after the `NotCancellable` case, signal the running worker:
```rust
Err(paavo_core::CoreError::NotCancellable { state }) => {
    let signalled = s.cancellation.signal(&id, paavo_runner::RunCommand::Cancel);
    if signalled {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::CONFLICT,
            format!("not cancellable in state {state:?}"),
        )
            .into_response()
    }
}
```

- [ ] **Step 3: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): per-job cancellation registry wired into cancel route"
```

---

#### 4.3.b: Dispatch loop

The dispatch loop is the heart of paavod. We exercise it through a `FakeRunner` so the integration test runs without probes.

- [ ] **Step 1: Implement dispatch**

`crates/paavod/src/dispatch.rs`:
```rust
//! Dispatch loop. Polls `pick_next` and runs jobs end-to-end.
//!
//! For each picked job:
//! 1. Transition row to `Building`, touch board's `last_used_at`.
//! 2. Run `paavo-build::build_release` (or hit cache).
//! 3. Transition to `Running`, register cancellation sender.
//! 4. Call `Runner::run`.
//! 5. Persist outcome, apply quarantine policy, publish terminal event.

use crate::app_state::AppState;
use crate::job_logs::LiveEvent;
use chrono::Utc;
use paavo_build::{tar::unpack_into, BuildPlan};
use paavo_core::{
    apply_outcome_to_board, cache_lookup, cache_store, pick_next, CacheLookup,
    QuarantinePolicy, RunOutcome, Runner, SchedulerConfig,
};
use paavo_db::OutcomeRecord;
use paavo_proto::{JobOutcome, JobState, TerminalOutcome};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

/// Spawn the dispatch loop. Returns immediately. The loop exits when
/// `drain` flips to true *and* there are no running jobs.
pub fn spawn(
    state: AppState,
    runner: Arc<dyn Runner>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // On drain, exit once nothing is in-flight. (In-flight implies the
            // cancellation registry is non-empty.)
            if state.drain.is_draining()
                && state.cancellation.inner_is_empty_for_drain()
            {
                return;
            }

            let pick = {
                let db = state.db.lock();
                pick_next(db.raw_conn(), SchedulerConfig {
                    starvation_threshold_ms: state.config.scheduler.starvation_threshold_s * 1_000,
                })
            };
            let Ok(Some(scheduled)) = pick else {
                sleep(Duration::from_millis(250)).await;
                continue;
            };

            let state_clone = state.clone();
            let runner_clone = runner.clone();
            // Spawn the job onto a tokio blocking task so probe-rs and cargo
            // can use their threads.
            tokio::task::spawn_blocking(move || {
                run_one(state_clone, runner_clone, scheduled.job, scheduled.board);
            });
        }
    })
}

fn run_one(
    state: AppState,
    runner: Arc<dyn Runner>,
    job: paavo_db::JobRow,
    board: paavo_db::BoardRow,
) {
    let job_id = job.id;
    let board_id = board.spec.id.clone();
    let now_ms = Utc::now().timestamp_millis();

    // 1. Transition to Building + touch board.
    {
        let db = state.db.lock();
        if paavo_db::JobRow::transition_to_building(db.raw_conn(), &job_id, &board_id, now_ms).is_err() {
            return; // someone else got it
        }
        let _ = paavo_db::BoardRow::touch_last_used(db.raw_conn(), &board_id, now_ms);
    }

    // 2. Cache lookup or build.
    let elf_path = {
        let lookup = {
            let db = state.db.lock();
            cache_lookup(db.raw_conn(), &job.tar_blake3).ok()
        };
        match lookup {
            Some(CacheLookup::Hit { elf_path }) => elf_path,
            _ => match build_job(&state, &job) {
                Ok(p) => p,
                Err(e) => {
                    finalize_with_outcome(
                        &state,
                        &job_id,
                        &board_id,
                        JobOutcome::Failed(TerminalOutcome::BuildErr { stderr: e }),
                        true,
                    );
                    return;
                }
            },
        }
    };

    // 3. Transition to Running + register cancel.
    {
        let db = state.db.lock();
        let _ = paavo_db::JobRow::transition_to_running(
            db.raw_conn(),
            &job_id,
            &elf_path.display().to_string(),
        );
    }
    let (cancel_tx, _cancel_rx) =
        crossbeam_channel::unbounded::<paavo_runner::RunCommand>();
    state.cancellation.register(job_id, cancel_tx);

    // 4. Run (this hides the real probe + watchdog inside `runner`).
    let RunOutcome {
        outcome,
        probe_released_cleanly,
    } = runner.run(job_id, &board_id);

    state.cancellation.unregister(&job_id);
    finalize_with_outcome(&state, &job_id, &board_id, outcome, probe_released_cleanly);
}

fn build_job(state: &AppState, job: &paavo_db::JobRow) -> Result<std::path::PathBuf, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(&job.tar_path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    let sd = crate::state_dir::StateDir::from_root(&state.config.server.state_dir);
    let crate_dir = sd.sandboxes_dir.join(job.id.to_string());
    unpack_into(&bytes, &crate_dir).map_err(|e| e.to_string())?;

    // Find the unique sub-dir under crate_dir that contains Cargo.toml.
    let crate_root = walkdir::WalkDir::new(&crate_dir)
        .min_depth(1)
        .max_depth(2)
        .into_iter()
        .flatten()
        .find(|e| e.file_name() == "Cargo.toml")
        .map(|e| e.path().parent().unwrap().to_path_buf())
        .ok_or_else(|| "no Cargo.toml in uploaded tar".to_string())?;

    let plan = BuildPlan {
        crate_dir: crate_root,
        target_dir: sd.cargo_target_dir.clone(),
        cargo_update_packages: vec![],
    };
    let res = paavo_build::build_release(&plan).map_err(|e| e.to_string())?;
    {
        let db = state.db.lock();
        let _ = cache_store(
            db.raw_conn(),
            &job.tar_blake3,
            &res.elf_path,
            chrono::Utc::now().timestamp_millis(),
        );
    }
    Ok(res.elf_path)
}

fn finalize_with_outcome(
    state: &AppState,
    job_id: &paavo_proto::JobId,
    board_id: &str,
    outcome: JobOutcome,
    probe_released_cleanly: bool,
) {
    let terminal_state = match &outcome {
        JobOutcome::Passed => JobState::Passed,
        JobOutcome::Failed(_) => JobState::Failed,
        JobOutcome::TimedOut { .. } => JobState::TimedOut,
        JobOutcome::Aborted { .. } => JobState::Aborted,
    };
    let now_ms = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        let _ = paavo_db::JobRow::finalize(
            db.raw_conn(),
            job_id,
            &OutcomeRecord {
                state: terminal_state,
                outcome: outcome.clone(),
                finished_at_ms: now_ms,
            },
        );
        let _ = apply_outcome_to_board(
            db.raw_conn(),
            board_id,
            &outcome,
            probe_released_cleanly,
            QuarantinePolicy {
                consecutive_infra_failures: state.config.quarantine.consecutive_infra_failures,
            },
        );
    }
    state
        .job_logs
        .publish(*job_id, LiveEvent::Terminal(outcome));
    state.job_logs.finalize(*job_id);
}
```

Helper on the cancellation registry (so we don't expose the inner map):

Add to `crates/paavod/src/cancellation.rs`:
```rust
impl CancellationRegistry {
    /// True when nothing is in-flight (used by dispatch loop's drain check).
    pub fn inner_is_empty_for_drain(&self) -> bool {
        self.inner.lock().is_empty()
    }
}
```

Add to `lib.rs`: `pub mod dispatch;`.

- [ ] **Step 2: Add a fake `Runner` and dispatch integration test**

`crates/paavod/tests/dispatch_loop.rs`:
```rust
use parking_lot::Mutex;
use paavo_core::{RunOutcome, Runner};
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSource, JobState, Priority,
    ProbeSelector,
};
use std::sync::Arc;

struct FakeRunner {
    out: Mutex<JobOutcome>,
}

impl Runner for FakeRunner {
    fn run(&self, _id: JobId, _board_id: &str) -> RunOutcome {
        RunOutcome {
            outcome: self.out.lock().clone(),
            probe_released_cleanly: true,
        }
    }
}

#[tokio::test]
async fn dispatch_runs_a_submitted_job_to_completion() {
    use paavod::app_state::{AppState, DrainState};
    use paavod::cancellation::CancellationRegistry;
    use paavod::config::*;
    use paavod::job_logs::JobLogsBroker;
    let tmp = tempfile::tempdir().unwrap();
    let sd = paavod::state_dir::StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    let db = paavo_db::Db::open(&sd.sqlite_path).unwrap();

    // Seed a board.
    paavo_db::BoardRow::insert(
        db.raw_conn(),
        &BoardSpec {
            id: "b".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector { vid: "x".into(), pid: "x".into(), serial: "x".into() },
            chip_name: "x".into(),
            target_name: "x".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        },
        0,
    )
    .unwrap();

    // Write a tar containing a host cargo project (we'll skip the build by
    // pre-populating the cache).
    let tar_path = sd.uploads_dir.join("aaa.tar");
    std::fs::write(&tar_path, b"dummy").unwrap();
    let elf_path = sd.cache_elfs_dir.join("aaa.elf");
    std::fs::write(&elf_path, b"\x7fELF").unwrap();
    paavo_db::BuildCacheEntry::upsert(
        db.raw_conn(),
        &paavo_db::BuildCacheEntry {
            tar_blake3: "aaa".into(),
            elf_path: elf_path.display().to_string(),
            built_at: 0,
            last_used_at: 0,
            size_bytes: 4,
        },
    )
    .unwrap();

    // Seed a job.
    let job_id = JobId::new();
    paavo_db::JobRow::insert(
        db.raw_conn(),
        &paavo_db::NewJob {
            id: job_id,
            priority: Priority::Interactive,
            submitter: "x".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector { kind: "mcxa266".into(), instance: None, wiring_profile: None },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "aaa".into(),
            tar_path: tar_path.display().to_string(),
        },
        0,
    )
    .unwrap();

    let cfg = Arc::new(Config {
        server: ServerConfig { bind: "x".into(), state_dir: tmp.path().to_path_buf() },
        web: WebConfig { bind: "x".into() },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig { nightly_cron: "0 0 19 * * *".into(), starvation_threshold_s: 21_600 },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    });
    let inventory = paavo_db::BoardRow::list_all(db.raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect::<Vec<_>>();

    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: cfg,
        inventory: Arc::new(Mutex::new(inventory)),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };

    let runner: Arc<dyn Runner> = Arc::new(FakeRunner { out: Mutex::new(JobOutcome::Passed) });
    let handle = paavod::dispatch::spawn(state.clone(), runner);

    // Wait until the job reaches terminal state.
    let mut tries = 0;
    let outcome = loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let db = state.db.lock();
        if let Ok(row) = paavo_db::JobRow::get(db.raw_conn(), &job_id) {
            if row.state == JobState::Passed {
                break row.outcome;
            }
        }
        tries += 1;
        assert!(tries < 50, "job never finished");
    };
    assert_eq!(outcome, Some(JobOutcome::Passed));

    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p paavod --test dispatch_loop`
Expected: 1 passed.

- [ ] **Step 4: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): dispatch loop (pick_next → build → run → finalize)"
```

---

#### 4.3.c: Nightly cron driver

- [ ] **Step 1: Implement cron**

`crates/paavod/src/cron.rs`:
```rust
//! Nightly cron driver: when `scheduler.nightly_cron` fires, walk every
//! `[[corpus]]` entry, tar each subdir, and submit it as a Scheduled job.

use crate::app_state::AppState;
use crate::config::CorpusEntry;
use crate::state_dir::StateDir;
use chrono::Utc;
use paavo_core::{enqueue_job, EnqueueRequest};
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use std::path::Path;
use tokio_cron_scheduler::{Job, JobScheduler};

/// Wire up and start the cron job. Returns the scheduler so the caller can
/// shut it down on SIGTERM.
pub async fn start(state: AppState) -> anyhow::Result<JobScheduler> {
    let sched = JobScheduler::new().await?;
    let cron_expr = state.config.scheduler.nightly_cron.clone();
    let state_for_job = state.clone();
    let job = Job::new_async(cron_expr.as_str(), move |_uuid, _l| {
        let state = state_for_job.clone();
        Box::pin(async move {
            if let Err(e) = run_nightly_corpus(&state).await {
                tracing::error!(error=?e, "nightly cron run failed");
            }
        })
    })?;
    sched.add(job).await?;
    sched.start().await?;
    Ok(sched)
}

async fn run_nightly_corpus(state: &AppState) -> anyhow::Result<()> {
    let now_ms = Utc::now().timestamp_millis();
    paavo_db::ScheduleRow::upsert(
        &state.db.lock().raw_conn(),
        &paavo_db::ScheduleRow {
            id: "nightly".into(),
            cron: state.config.scheduler.nightly_cron.clone(),
            enabled: true,
            last_triggered_at: Some(now_ms),
            last_completed_at: None,
        },
    )?;

    let corpus = state.config.corpus.clone();
    for entry in &corpus {
        if let Err(e) = enqueue_corpus_entry(state, entry).await {
            tracing::error!(corpus=%entry.name, error=?e, "corpus enqueue failed");
        }
    }

    paavo_db::ScheduleRow::apply_update(
        &state.db.lock().raw_conn(),
        "nightly",
        &paavo_db::ScheduleUpdate {
            last_triggered_at: None,
            last_completed_at: Some(Utc::now().timestamp_millis()),
        },
    )?;
    Ok(())
}

async fn enqueue_corpus_entry(state: &AppState, entry: &CorpusEntry) -> anyhow::Result<()> {
    for sub in std::fs::read_dir(&entry.path)? {
        let sub = sub?;
        if !sub.file_type()?.is_dir() {
            continue;
        }
        let crate_dir = sub.path();
        if !crate_dir.join("Cargo.toml").is_file() {
            continue;
        }
        // tar the dir.
        let tar_bytes = make_tar(&crate_dir)?;
        let sd = StateDir::from_root(&state.config.server.state_dir);
        sd.ensure_dirs()?;
        let blake = paavo_build::tar::tar_blake3(&tar_bytes);
        let tar_path = sd.uploads_dir.join(format!("{blake}.tar"));
        if !tar_path.is_file() {
            std::fs::write(&tar_path, &tar_bytes)?;
        }
        // Infer board kind from path: convention says `soak-tests/<kind>/...`.
        let kind = crate_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("cannot infer board kind from {crate_dir:?}"))?;
        let req = EnqueueRequest {
            job_id: JobId::new(),
            priority: Priority::Scheduled,
            submitter: format!("nightly:{name}", name = entry.name),
            source: JobSource::Scheduler,
            board_selector: BoardSelector {
                kind: kind.into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: state.config.timeouts.default_inactivity_s * 1_000,
            hard_max_ms: state.config.timeouts.default_scheduled_hard_max_s * 1_000,
            tar_blake3: blake,
            tar_path: tar_path.display().to_string(),
            daemon_ceiling_ms: state.config.timeouts.daemon_ceiling_s * 1_000,
        };
        let inventory = state.inventory_snapshot();
        let db = state.db.lock();
        let _ = enqueue_job(db.raw_conn(), &inventory, req);
    }
    Ok(())
}

fn make_tar(crate_dir: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut tarb = tar::Builder::new(&mut buf);
        tarb.append_dir_all(
            crate_dir.file_name().unwrap_or_default(),
            crate_dir,
        )?;
        tarb.finish()?;
    }
    Ok(buf)
}
```

Add `pub mod cron;` to lib.rs. The cron module is intentionally lightly-tested in the workspace (no good way to fast-forward `tokio-cron-scheduler` without a real wallclock); the integration test in M6.4 (HW smoke) is where we exercise it end-to-end. Workspace test for the *enqueue* part:

`crates/paavod/tests/cron_enqueue.rs`:
```rust
//! Tests that the corpus enqueue helper inserts Scheduled jobs and infers
//! `board_kind` from the parent directory name.

use parking_lot::Mutex;
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::*;
use paavod::job_logs::JobLogsBroker;
use paavo_proto::{BoardHealth, BoardSpec, JobSource, ProbeSelector};
use std::sync::Arc;

#[tokio::test]
async fn corpus_entry_enqueues_one_job_per_crate_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    std::fs::create_dir_all(corpus_root.join("test-a/src")).unwrap();
    std::fs::write(corpus_root.join("test-a/Cargo.toml"), "[package]\nname=\"a\"\nversion=\"0\"\n").unwrap();
    std::fs::write(corpus_root.join("test-a/src/main.rs"), "fn main() {}").unwrap();
    std::fs::create_dir_all(corpus_root.join("test-b/src")).unwrap();
    std::fs::write(corpus_root.join("test-b/Cargo.toml"), "[package]\nname=\"b\"\nversion=\"0\"\n").unwrap();
    std::fs::write(corpus_root.join("test-b/src/main.rs"), "fn main() {}").unwrap();

    let state_root = tmp.path().join("state");
    let sd = paavod::state_dir::StateDir::from_root(&state_root);
    sd.ensure_dirs().unwrap();
    let db = paavo_db::Db::open(&sd.sqlite_path).unwrap();
    paavo_db::BoardRow::insert(
        db.raw_conn(),
        &BoardSpec {
            id: "mcxa266-01".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector { vid: "x".into(), pid: "x".into(), serial: "x".into() },
            chip_name: "x".into(),
            target_name: "x".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        },
        0,
    )
    .unwrap();
    let inventory = vec![paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap().spec];

    let cfg = Arc::new(Config {
        server: ServerConfig { bind: "x".into(), state_dir: state_root.clone() },
        web: WebConfig { bind: "x".into() },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig { nightly_cron: "0 0 19 * * *".into(), starvation_threshold_s: 21_600 },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![CorpusEntry {
            name: "test-corpus".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
    });
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: cfg.clone(),
        inventory: Arc::new(Mutex::new(inventory)),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };

    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        &state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.source == JobSource::Scheduler));
}
```

Add a test hook in `crates/paavod/src/cron.rs`:
```rust
/// Test hook: run the corpus enqueue logic exactly once without scheduling.
#[doc(hidden)]
pub async fn __test_run_once(state: &AppState) -> anyhow::Result<()> {
    run_nightly_corpus(state).await
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p paavod --test cron_enqueue`
Expected: 1 passed.

- [ ] **Step 3: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): nightly cron driver — corpus walker + Scheduled enqueue"
```

---

#### 4.3.d: SIGTERM drain

- [ ] **Step 1: Implement**

`crates/paavod/src/shutdown.rs`:
```rust
//! SIGTERM drain: flip `drain` flag, signal all running jobs with
//! `DaemonShutdown` after the grace period, return when dispatch loop and
//! cron have stopped.

use crate::app_state::AppState;
use paavo_runner::RunCommand;
use std::time::Duration;
use tokio::time::sleep;

/// Run shutdown drain. Caller obtains the future and awaits it during
/// `axum::serve(...).with_graceful_shutdown(future)`.
pub async fn await_signal_then_drain(state: AppState) {
    wait_for_signal().await;
    tracing::info!("drain: SIGTERM/Ctrl-C received");
    state.drain.set_draining();
    let grace = Duration::from_secs(state.config.timeouts.shutdown_grace_s);
    sleep(grace).await;
    tracing::info!("drain: grace expired, signaling DaemonShutdown to all in-flight jobs");
    state.cancellation.signal_all(RunCommand::DaemonShutdown);
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("term");
    let mut int = signal(SignalKind::interrupt()).expect("int");
    tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
```

Add to lib: `pub mod shutdown;`. No standalone test — exercised by 4.4 main.

- [ ] **Step 2: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): SIGTERM drain with grace timeout"
```

---

### Task 4.4: paavod — main()

- [ ] **Step 1: Wire main**

`crates/paavod/src/main.rs`:
```rust
//! paavod entry point.

use anyhow::{Context, Result};
use clap::Parser;
use parking_lot::Mutex;
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::Config;
use paavod::job_logs::JobLogsBroker;
use paavod::state_dir::StateDir;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "paavod", about = "paavo daemon")]
struct Args {
    /// Path to paavo.toml.
    #[arg(long, env = "PAAVO_CONFIG", default_value = "/etc/paavo/paavo.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info".into()))
        .init();

    let args = Args::parse();
    let config = Config::load(&args.config)
        .with_context(|| format!("loading config at {}", args.config.display()))?;

    let sd = StateDir::from_root(&config.server.state_dir);
    sd.ensure_dirs()
        .with_context(|| format!("creating state dirs under {}", sd.root.display()))?;

    let db = paavo_db::Db::open(&sd.sqlite_path)
        .with_context(|| format!("opening sqlite at {}", sd.sqlite_path.display()))?;

    // Load boards.toml if present.
    let inventory = load_inventory(&db, &sd.boards_toml)?;

    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(config.clone()),
        inventory: Arc::new(Mutex::new(inventory)),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };

    // Runner: paavo-runner's real BoardWorker backed by paavo-probe's
    // RealSession. We wrap it in a tiny adapter that implements paavo-core's
    // `Runner` trait.
    let runner: Arc<dyn paavo_core::Runner> = Arc::new(RealRunner {
        state: state.clone(),
    });

    let _dispatch = paavod::dispatch::spawn(state.clone(), runner);
    let _cron = paavod::cron::start(state.clone()).await?;

    let listener = tokio::net::TcpListener::bind(&config.server.bind).await?;
    tracing::info!(bind = %config.server.bind, "paavod listening");

    let state_for_shutdown = state.clone();
    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(paavod::shutdown::await_signal_then_drain(state_for_shutdown))
        .await?;

    Ok(())
}

fn load_inventory(db: &paavo_db::Db, boards_toml: &std::path::Path) -> Result<Vec<paavo_proto::BoardSpec>> {
    // boards.toml is optional; if present, sync rows into the DB first.
    if boards_toml.is_file() {
        #[derive(serde::Deserialize)]
        struct Boards {
            #[serde(default)]
            board: Vec<paavo_proto::BoardSpec>,
        }
        let raw = std::fs::read_to_string(boards_toml)?;
        let b: Boards = toml::from_str(&raw)?;
        for spec in &b.board {
            if paavo_db::BoardRow::find(db.raw_conn(), &spec.id)?.is_none() {
                paavo_db::BoardRow::insert(db.raw_conn(), spec, chrono::Utc::now().timestamp_millis())?;
            }
        }
    }
    Ok(paavo_db::BoardRow::list_all(db.raw_conn())?
        .into_iter()
        .map(|r| r.spec)
        .collect())
}

/// Real-runner adapter. In v1 this spins up a paavo-runner BoardWorker per
/// job with a `paavo-probe::RealSession`. The hardware-only wiring lives in
/// Milestone 6.4; for now this returns an `InfraErr` outcome that surfaces
/// a clear message in the API while keeping the daemon functional for
/// non-hardware integration tests.
struct RealRunner {
    state: AppState,
}

impl paavo_core::Runner for RealRunner {
    fn run(&self, job_id: paavo_proto::JobId, board_id: &str) -> paavo_core::RunOutcome {
        let _ = (&self.state, job_id, board_id);
        paavo_core::RunOutcome {
            outcome: paavo_proto::JobOutcome::Failed(paavo_proto::TerminalOutcome::InfraErr {
                stage: "real_runner".into(),
                message: "RealRunner is wired in Milestone 6.4; \
                          set PAAVO_FAKE_RUNNER=1 in dev to use the fake outcome runner"
                    .into(),
            }),
            probe_released_cleanly: true,
        }
    }
}
```

- [ ] **Step 2: Confirm paavod builds**

Run: `cargo build -p paavod`
Expected: succeeds.

- [ ] **Step 3: Run the full workspace tests**

Run: `cargo test --workspace`
Expected: green (all tests across every crate).

- [ ] **Step 4: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavod
git -C D:\workspace\paavo commit -m "feat(paavod): main() — config load + state dir + boards.toml + dispatch + cron + shutdown"
```

---

### Task 4.5: paavo-cli

Spec coverage: §10.

**Files:**
- Create: `crates/paavo-cli/src/main.rs` (replace skeleton)
- Create: `crates/paavo-cli/src/cli.rs`
- Create: `crates/paavo-cli/src/client.rs`
- Create: `crates/paavo-cli/src/cmd_run.rs`
- Create: `crates/paavo-cli/src/cmd_new.rs`
- Create: `crates/paavo-cli/src/cmd_boards.rs`
- Create: `crates/paavo-cli/src/cmd_jobs.rs`
- Create: `crates/paavo-cli/src/config.rs`
- Test: `crates/paavo-cli/tests/cli_help.rs`
- Test: `crates/paavo-cli/tests/cli_jobs_against_paavod.rs`

#### 4.5.a: CLI surface (clap) + `--help` smoke

- [ ] **Step 1: Write the failing help test**

`crates/paavo-cli/tests/cli_help.rs`:
```rust
use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn top_level_help_lists_all_subcommands() {
    Command::cargo_bin("paavo-cli").unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("run"))
        .stdout(contains("new"))
        .stdout(contains("cancel"))
        .stdout(contains("logs"))
        .stdout(contains("jobs"))
        .stdout(contains("boards"))
        .stdout(contains("board"));
}

#[test]
fn board_subcommand_has_add_quarantine_unquarantine() {
    Command::cargo_bin("paavo-cli").unwrap()
        .args(["board", "--help"])
        .assert()
        .success()
        .stdout(contains("add"))
        .stdout(contains("quarantine"))
        .stdout(contains("unquarantine"));
}
```

- [ ] **Step 2: Run to confirm fail**

Run: `cargo test -p paavo-cli --test cli_help`
Expected: FAILS — paavo-cli still prints "placeholder".

- [ ] **Step 3: Implement the clap CLI surface**

`crates/paavo-cli/src/cli.rs`:
```rust
//! clap command surface for paavo-cli. See spec §10.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Top-level CLI.
#[derive(Parser, Debug)]
#[command(name = "paavo-cli", version, about = "paavo command-line client")]
pub struct Cli {
    /// Daemon URL. Falls back to PAAVO_HOST then ~/.config/paavo/cli.toml.
    #[arg(long, env = "PAAVO_HOST")]
    pub host: Option<String>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

/// One subcommand.
#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Run a test crate (or directory or pre-built ELF) on a board.
    Run {
        /// .rs file, crate dir, or .elf.
        path: PathBuf,
        /// Required board kind (e.g. mcxa266).
        #[arg(long)]
        board_kind: Option<String>,
        /// Specific board instance.
        #[arg(long)]
        instance: Option<String>,
        /// Hard wall-clock max, e.g. "1h", "30m", "120s".
        #[arg(long)]
        timeout: Option<String>,
        /// Inactivity timeout, e.g. "60s".
        #[arg(long)]
        inactivity: Option<String>,
        /// Priority class.
        #[arg(long, default_value = "interactive")]
        priority: PriorityArg,
    },
    /// Scaffold a new test crate via cargo-generate templates.
    New {
        /// Crate name to create.
        name: String,
        /// Required board kind.
        #[arg(long)]
        board_kind: String,
        /// quick / soak.
        #[arg(long, default_value = "quick")]
        kind: TestKindArg,
    },
    /// Cancel a queued or running job.
    Cancel {
        /// Job id (ULID).
        job_id: String,
    },
    /// Stream logs for a job.
    Logs {
        /// Job id.
        job_id: String,
        /// If set, follow until the job terminates.
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// List jobs.
    Jobs {
        /// Filter by state.
        #[arg(long)]
        state: Option<String>,
        /// Max rows.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// List boards.
    Boards,
    /// Board management (operator-side).
    Board {
        #[command(subcommand)]
        op: BoardOp,
    },
}

/// Priority CLI arg.
#[derive(Clone, Debug, clap::ValueEnum)]
pub enum PriorityArg {
    /// Interactive.
    Interactive,
    /// Scheduled.
    Scheduled,
}

/// Test kind for `new`.
#[derive(Clone, Debug, clap::ValueEnum)]
pub enum TestKindArg {
    /// quick.
    Quick,
    /// soak.
    Soak,
}

/// `board` ops.
#[derive(Subcommand, Debug)]
pub enum BoardOp {
    /// Add a board to the inventory.
    Add {
        /// Board kind.
        #[arg(long)]
        kind: String,
        /// Instance id (e.g. mcxa266-02).
        #[arg(long)]
        instance: String,
        /// VID:PID:serial.
        #[arg(long)]
        probe: String,
        /// probe-rs chip name.
        #[arg(long)]
        chip: String,
        /// `paavo_meta::target!()` value.
        #[arg(long)]
        target: String,
        /// Wiring profile, default "default".
        #[arg(long, default_value = "default")]
        wiring_profile: String,
    },
    /// Quarantine a board.
    Quarantine {
        /// Board id.
        id: String,
        /// Reason text.
        #[arg(long)]
        reason: String,
    },
    /// Un-quarantine a board.
    Unquarantine {
        /// Board id.
        id: String,
    },
}
```

`crates/paavo-cli/src/main.rs` (replace skeleton):
```rust
//! paavo-cli entry point.
use anyhow::Result;
use clap::Parser;

mod cli;
mod client;
mod cmd_boards;
mod cmd_jobs;
mod cmd_new;
mod cmd_run;
mod config;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "warn".into()))
        .init();
    let args = cli::Cli::parse();
    let host = config::resolve_host(args.host.as_deref())?;
    let client = client::Client::new(host);
    match args.cmd {
        cli::Cmd::Run { path, board_kind, instance, timeout, inactivity, priority } =>
            cmd_run::run(&client, &path, board_kind.as_deref(), instance.as_deref(),
                         timeout.as_deref(), inactivity.as_deref(), priority).await,
        cli::Cmd::New { name, board_kind, kind } =>
            cmd_new::new(&name, &board_kind, kind),
        cli::Cmd::Cancel { job_id } => cmd_jobs::cancel(&client, &job_id).await,
        cli::Cmd::Logs { job_id, follow } => cmd_jobs::logs(&client, &job_id, follow).await,
        cli::Cmd::Jobs { state, limit } => cmd_jobs::list(&client, state.as_deref(), limit).await,
        cli::Cmd::Boards => cmd_boards::list(&client).await,
        cli::Cmd::Board { op } => cmd_boards::op(&client, op).await,
    }
}
```

Add stub modules so the binary compiles:

`crates/paavo-cli/src/config.rs`:
```rust
//! Host resolution: --host > PAAVO_HOST > ~/.config/paavo/cli.toml > default.

use anyhow::Result;

/// Resolve the daemon host string.
pub fn resolve_host(flag: Option<&str>) -> Result<String> {
    if let Some(h) = flag {
        return Ok(h.to_string());
    }
    if let Ok(h) = std::env::var("PAAVO_HOST") {
        return Ok(h);
    }
    let p = dirs_helper().join("paavo").join("cli.toml");
    if p.is_file() {
        #[derive(serde::Deserialize)]
        struct CliCfg { host: String }
        let raw = std::fs::read_to_string(&p)?;
        let cfg: CliCfg = toml::from_str(&raw)?;
        return Ok(cfg.host);
    }
    Ok("http://127.0.0.1:8080".into())
}

fn dirs_helper() -> std::path::PathBuf {
    if let Ok(c) = std::env::var("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(c);
    }
    if let Ok(h) = std::env::var("HOME") {
        return std::path::PathBuf::from(h).join(".config");
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return std::path::PathBuf::from(h).join(".config");
    }
    std::path::PathBuf::from(".")
}
```

`crates/paavo-cli/src/client.rs`:
```rust
//! Thin HTTP client around the paavod surface.

use anyhow::{Context, Result};
use paavo_proto::{BoardSpec, JobSpec};
use serde::de::DeserializeOwned;

/// HTTP client.
pub struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    /// Construct.
    pub fn new(base: String) -> Self {
        Self {
            base,
            http: reqwest::Client::new(),
        }
    }

    /// Submit a job. Returns the new `job_id` string.
    pub async fn submit_job(
        &self,
        spec: &JobSpec,
        tar_bytes: Vec<u8>,
    ) -> Result<String> {
        let meta = serde_json::to_string(spec)?;
        let form = reqwest::multipart::Form::new()
            .part(
                "metadata",
                reqwest::multipart::Part::bytes(meta.into_bytes())
                    .mime_str("application/json")?,
            )
            .part(
                "crate",
                reqwest::multipart::Part::bytes(tar_bytes)
                    .file_name("crate.tar")
                    .mime_str("application/octet-stream")?,
            );
        let resp = self
            .http
            .post(format!("{}/jobs", self.base))
            .multipart(form)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.text().await.unwrap_or_default());
        }
        #[derive(serde::Deserialize)]
        struct Body { job_id: String }
        let body: Body = resp.json().await?;
        Ok(body.job_id)
    }

    /// GET helper.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self.http.get(format!("{}{}", self.base, path)).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.text().await.unwrap_or_default());
        }
        let val = resp.json().await?;
        Ok(val)
    }

    /// POST with optional JSON body.
    pub async fn post_json<B: serde::Serialize>(
        &self,
        path: &str,
        body: Option<&B>,
    ) -> Result<()> {
        let mut req = self.http.post(format!("{}{}", self.base, path));
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.text().await.unwrap_or_default());
        }
        Ok(())
    }

    /// Stream `GET /jobs/:id/stream` as SSE lines. Returns a stream of
    /// `(event_kind, json)` pairs.
    pub async fn stream(&self, job_id: &str) -> Result<reqwest::Response> {
        let resp = self
            .http
            .get(format!("{}/jobs/{}/stream", self.base, job_id))
            .send()
            .await
            .with_context(|| "GET /jobs/:id/stream")?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.status());
        }
        Ok(resp)
    }

    /// Add a board.
    pub async fn add_board(&self, spec: &BoardSpec) -> Result<()> {
        self.post_json("/boards", Some(spec)).await
    }
}
```

`crates/paavo-cli/src/cmd_run.rs`:
```rust
//! `paavo-cli run`: tar a crate dir / .rs / .elf and submit. Streams output.

use crate::client::Client;
use crate::cli::PriorityArg;
use anyhow::{Context, Result};
use paavo_proto::{BoardSelector, JobSource, JobSpec, Priority};
use std::path::Path;

/// Entry point for `paavo-cli run`.
pub async fn run(
    client: &Client,
    path: &Path,
    board_kind: Option<&str>,
    instance: Option<&str>,
    timeout: Option<&str>,
    inactivity: Option<&str>,
    priority: PriorityArg,
) -> Result<()> {
    let kind = board_kind
        .ok_or_else(|| anyhow::anyhow!("--board-kind is required for `run`"))?;
    let crate_dir = resolve_crate_dir(path)?;
    let tar_bytes = make_tar(&crate_dir).context("tarring crate dir")?;
    let blake = paavo_build::tar::tar_blake3(&tar_bytes);

    let spec = JobSpec {
        priority: match priority {
            PriorityArg::Interactive => Priority::Interactive,
            PriorityArg::Scheduled => Priority::Scheduled,
        },
        submitter: whoami().unwrap_or_else(|| "anon".into()),
        source: JobSource::Cli,
        board_selector: BoardSelector {
            kind: kind.into(),
            instance: instance.map(String::from),
            wiring_profile: None,
        },
        inactivity_timeout_ms: inactivity.map(parse_duration_ms).transpose()?,
        hard_max_ms: timeout.map(parse_duration_ms).transpose()?,
        tar_blake3: blake,
    };

    let job_id = client.submit_job(&spec, tar_bytes).await?;
    println!("submitted: {job_id}");
    stream_logs(client, &job_id).await
}

fn resolve_crate_dir(path: &Path) -> Result<std::path::PathBuf> {
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    if path.is_file() {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        match ext {
            "rs" => {
                // walk up to nearest Cargo.toml.
                let mut cur = path.parent().unwrap_or(path);
                loop {
                    if cur.join("Cargo.toml").is_file() {
                        return Ok(cur.to_path_buf());
                    }
                    match cur.parent() {
                        Some(p) => cur = p,
                        None => anyhow::bail!(
                            ".rs file has no parent Cargo.toml; run `paavo-cli new` first"
                        ),
                    }
                }
            }
            "elf" => {
                anyhow::bail!(
                    "pre-built .elf submission is wired in v1.1; \
                     for now pass a crate dir or .rs file"
                );
            }
            other => anyhow::bail!("unsupported file extension: .{other}"),
        }
    }
    anyhow::bail!("not a file or dir: {path:?}")
}

fn make_tar(dir: &Path) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut t = tar::Builder::new(&mut buf);
        t.append_dir_all(dir.file_name().unwrap_or_default(), dir)?;
        t.finish()?;
    }
    Ok(buf)
}

fn whoami() -> Option<String> {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok()
}

fn parse_duration_ms(s: &str) -> Result<u64> {
    // Supports "120s", "30m", "1h".
    let s = s.trim();
    if let Some(num) = s.strip_suffix('h') {
        return Ok(num.trim().parse::<u64>()? * 3_600_000);
    }
    if let Some(num) = s.strip_suffix('m') {
        return Ok(num.trim().parse::<u64>()? * 60_000);
    }
    if let Some(num) = s.strip_suffix('s') {
        return Ok(num.trim().parse::<u64>()? * 1_000);
    }
    Ok(s.parse::<u64>()?)
}

async fn stream_logs(client: &Client, job_id: &str) -> Result<()> {
    use futures::StreamExt;
    let mut resp = client.stream(job_id).await?;
    let mut buf = String::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find('\n') {
            let line = buf[..idx].trim().to_string();
            buf.drain(..=idx);
            if !line.is_empty() {
                handle_sse_line(&line);
            }
        }
    }
    Ok(())
}

fn handle_sse_line(line: &str) {
    if let Some(data) = line.strip_prefix("data: ") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if v["type"] == "frame" {
                let msg = v["frame"]["message"].as_str().unwrap_or("");
                println!("{msg}");
            } else if v["type"] == "terminal" {
                let outcome = &v["outcome"];
                // outcome is either the string "passed" or a single-key object
                // like {"failed": {...}} / {"timed_out": {...}} / {"aborted": {...}}.
                let tag = outcome
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| {
                        outcome
                            .as_object()
                            .and_then(|m| m.keys().next().cloned())
                    })
                    .unwrap_or_default();
                println!("--- terminal: {outcome}");
                std::process::exit(if tag == "passed" { 0 } else { 1 });
            }
        }
    }
}
```

`crates/paavo-cli/src/cmd_jobs.rs`:
```rust
//! `paavo-cli cancel | logs | jobs`.

use crate::client::Client;
use anyhow::Result;
use serde_json::Value;

/// `paavo-cli cancel <id>`.
pub async fn cancel(client: &Client, job_id: &str) -> Result<()> {
    client.post_json::<()>(&format!("/jobs/{job_id}/cancel"), None).await?;
    println!("cancelled: {job_id}");
    Ok(())
}

/// `paavo-cli logs <id> [--follow]`.
pub async fn logs(client: &Client, job_id: &str, _follow: bool) -> Result<()> {
    use futures::StreamExt;
    let mut resp = client.stream(job_id).await?;
    let mut buf = String::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find('\n') {
            let line = buf[..idx].trim().to_string();
            buf.drain(..=idx);
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if v["type"] == "frame" {
                        let msg = v["frame"]["message"].as_str().unwrap_or("");
                        println!("{msg}");
                    } else if v["type"] == "terminal" {
                        println!("--- terminal: {}", v["outcome"]);
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}

/// `paavo-cli jobs [--state ...] [--limit N]`.
pub async fn list(client: &Client, state: Option<&str>, limit: u32) -> Result<()> {
    let mut path = format!("/jobs?limit={limit}");
    if let Some(s) = state {
        path.push_str(&format!("&state={s}"));
    }
    let rows: Vec<Value> = client.get_json(&path).await?;
    for r in rows {
        println!(
            "{id}  {state:9} {priority:11} {submitter}",
            id = r["id"].as_str().unwrap_or(""),
            state = r["state"].as_str().unwrap_or(""),
            priority = r["priority"].as_str().unwrap_or("?"),
            submitter = r["submitter"].as_str().unwrap_or("")
        );
    }
    Ok(())
}
```

`crates/paavo-cli/src/cmd_boards.rs`:
```rust
//! `paavo-cli boards | board ...`.

use crate::cli::BoardOp;
use crate::client::Client;
use anyhow::{anyhow, Result};
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
use serde_json::Value;

/// `paavo-cli boards`.
pub async fn list(client: &Client) -> Result<()> {
    let rows: Vec<Value> = client.get_json("/boards").await?;
    for r in rows {
        println!(
            "{id:18} {kind:12} {health:13} {target}",
            id = r["id"].as_str().unwrap_or(""),
            kind = r["kind"].as_str().unwrap_or(""),
            health = r["health"].as_str().unwrap_or(""),
            target = r["target_name"].as_str().unwrap_or(""),
        );
    }
    Ok(())
}

/// `paavo-cli board ...`.
pub async fn op(client: &Client, op: BoardOp) -> Result<()> {
    match op {
        BoardOp::Add { kind, instance, probe, chip, target, wiring_profile } => {
            let mut parts = probe.split(':');
            let vid = parts.next().ok_or_else(|| anyhow!("probe missing VID"))?.to_string();
            let pid = parts.next().ok_or_else(|| anyhow!("probe missing PID"))?.to_string();
            let serial = parts.next().ok_or_else(|| anyhow!("probe missing serial"))?.to_string();
            let spec = BoardSpec {
                id: instance,
                kind,
                probe_selector: ProbeSelector { vid, pid, serial },
                chip_name: chip,
                target_name: target,
                wiring_profile: Some(wiring_profile),
                health: BoardHealth::Healthy,
            };
            client.add_board(&spec).await?;
            println!("added: {}", spec.id);
            Ok(())
        }
        BoardOp::Quarantine { id, reason } => {
            #[derive(serde::Serialize)]
            struct Body<'a> { reason: &'a str }
            client.post_json(&format!("/boards/{id}/quarantine"), Some(&Body { reason: &reason })).await?;
            println!("quarantined: {id}");
            Ok(())
        }
        BoardOp::Unquarantine { id } => {
            client.post_json::<()>(&format!("/boards/{id}/unquarantine"), None).await?;
            println!("unquarantined: {id}");
            Ok(())
        }
    }
}
```

`crates/paavo-cli/src/cmd_new.rs`:
```rust
//! `paavo-cli new`: thin wrapper around `cargo generate`.

use crate::cli::TestKindArg;
use anyhow::Result;

/// `paavo-cli new <name> --board-kind ... --kind ...`
pub fn new(name: &str, board_kind: &str, kind: TestKindArg) -> Result<()> {
    let kind_str = match kind {
        TestKindArg::Quick => "quick",
        TestKindArg::Soak => "soak",
    };
    // Templates live in <paavo-repo>/templates/<board-kind>/. The user is
    // expected to have the paavo repo cloned at a known location; we honor
    // PAAVO_TEMPLATES_DIR to point at it.
    let templates_dir = std::env::var("PAAVO_TEMPLATES_DIR").unwrap_or_else(|_| {
        // fallback: try sibling of the binary.
        let mut p = std::env::current_exe().unwrap_or_default();
        p.pop();
        p.push("../share/paavo/templates");
        p.display().to_string()
    });
    let template_path = std::path::PathBuf::from(&templates_dir).join(board_kind);
    if !template_path.is_dir() {
        anyhow::bail!(
            "template not found at {template_path:?}; \
             set PAAVO_TEMPLATES_DIR to point at the paavo repo's `templates/` dir"
        );
    }
    let status = std::process::Command::new("cargo")
        .args(["generate", "--path"])
        .arg(&template_path)
        .args(["--name", name])
        .args(["--define", &format!("test-kind={kind_str}")])
        .status()?;
    if !status.success() {
        anyhow::bail!("cargo generate exited with {status}");
    }
    Ok(())
}
```

- [ ] **Step 4: Run the help test**

Run: `cargo test -p paavo-cli --test cli_help`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-cli
git -C D:\workspace\paavo commit -m "feat(cli): clap surface + reqwest client + 7 subcommands"
```

---

#### 4.5.b: End-to-end CLI ↔ paavod jobs test

The point: prove `paavo-cli jobs` shells out, hits the local paavod TestServer, and prints job rows.

- [ ] **Step 1: Write the failing test**

`crates/paavo-cli/tests/cli_jobs_against_paavod.rs`:
```rust
//! Spawns paavod on an ephemeral port, seeds a job, then runs
//! `paavo-cli jobs` and asserts the output contains the job id.

use assert_cmd::Command as AssertCommand;
use parking_lot::Mutex;
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::*;
use paavod::job_logs::JobLogsBroker;
use paavo_db::Db;
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn paavo_cli_jobs_lists_seeded_job() {
    let tmp = tempdir().unwrap();
    let sd = paavod::state_dir::StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();

    let id = JobId::new();
    paavo_db::JobRow::insert(
        db.raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: Priority::Interactive,
            submitter: "felipe".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector { kind: "mcxa266".into(), instance: None, wiring_profile: None },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
        },
        0,
    )
    .unwrap();

    let cfg = Arc::new(Config {
        server: ServerConfig { bind: "x".into(), state_dir: tmp.path().to_path_buf() },
        web: WebConfig { bind: "x".into() },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig { nightly_cron: "0 0 19 * * *".into(), starvation_threshold_s: 21_600 },
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
        job_logs: JobLogsBroker::new(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    AssertCommand::cargo_bin("paavo-cli").unwrap()
        .env("PAAVO_HOST", format!("http://{addr}"))
        .args(["jobs", "--state", "submitted"])
        .assert()
        .success()
        .stdout(predicates::str::contains(&id.to_string()));

    server.abort();
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p paavo-cli --test cli_jobs_against_paavod`
Expected: 1 passed.

- [ ] **Step 3: Clippy + commit**

Run: `cargo clippy -p paavo-cli --all-targets -- -D warnings`
Expected: green.

```pwsh
git -C D:\workspace\paavo add crates/paavo-cli
git -C D:\workspace\paavo commit -m "test(cli): end-to-end paavo-cli jobs against in-process paavod"
```

---

### Milestone 4 exit criteria

- [ ] `paavod` runs (`cargo run -p paavod -- --config sample.toml`) and serves `/health`/`/ready`/`/boards`/`/jobs`
- [ ] `paavo-cli` builds and `paavo-cli jobs` queries a live paavod
- [ ] Dispatch loop runs a fake-runner job end-to-end including DB transitions and terminal stream event
- [ ] SIGTERM drain wiring exists (compiles + signal handler) — full hardware-path drain validated in M6.4
- [ ] `cargo test --workspace` green

---

## Milestone 5 — Web UI (paavo-web)

Spec coverage: §11 (5 read-only pages). v1 renders plain HTML server-side via axum — no client framework, no client-side interactivity. Styling uses **UnoCSS via the CDN runtime**: one `<script>` tag in `<head>`, then Tailwind-style utility class names on elements. Zero build step.

**Design tokens** (per user request: "clean, simple, pleasant, techy, easy-to-read"):

- **Typography:** monospace everywhere (`font-mono`) for the techy feel.
- **Layout:** generous whitespace (`max-w-5xl mx-auto p-6 leading-relaxed`).
- **Palette:** zinc neutrals (`text-zinc-900`, `bg-zinc-50`, borders `border-zinc-200`); state accents `text-emerald-700` (passed), `text-rose-700` (failed/quarantined), `text-blue-700` (running/building).
- **Tables:** subtle row separators only (`border-b border-zinc-200`), no full grid lines.
- **Nav:** sticky top bar with backdrop blur (`sticky top-0 backdrop-blur bg-zinc-50/80`).

### Task 5.1: paavo-web — RO db handle + axum mount + 5 pages

**Files:**
- Create: `crates/paavo-web/src/lib.rs`
- Create: `crates/paavo-web/src/main.rs` (replace skeleton)
- Create: `crates/paavo-web/src/app.rs`
- Create: `crates/paavo-web/src/db.rs`
- Create: `crates/paavo-web/src/config.rs`
- Create: `crates/paavo-web/src/pages/mod.rs`
- Create: `crates/paavo-web/src/pages/dashboard.rs`
- Create: `crates/paavo-web/src/pages/jobs_list.rs`
- Create: `crates/paavo-web/src/pages/job_detail.rs`
- Create: `crates/paavo-web/src/pages/boards.rs`
- Create: `crates/paavo-web/src/pages/schedule.rs`
- Test: `crates/paavo-web/tests/smoke.rs`

- [ ] **Step 1: Update paavo-web Cargo.toml**

Replace `crates/paavo-web/Cargo.toml` with:
```toml
[package]
name = "paavo-web"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "paavo read-only web viewer."

[lib]
name = "paavo_web"
path = "src/lib.rs"

[[bin]]
name = "paavo-web"
path = "src/main.rs"

[dependencies]
paavo-proto = { workspace = true }
paavo-db    = { workspace = true }
axum        = { workspace = true }
tokio       = { workspace = true }
tower       = { workspace = true }
tower-http  = { workspace = true }
serde       = { workspace = true }
serde_json  = { workspace = true }
toml        = { workspace = true }
clap        = { workspace = true }
anyhow      = { workspace = true }
tracing     = { workspace = true }
tracing-subscriber = { workspace = true }
chrono      = { workspace = true }
parking_lot = { workspace = true }
rusqlite    = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
tower    = { workspace = true }
```

- [ ] **Step 2: Implement config, db, app, and 5 pages**

`crates/paavo-web/src/lib.rs`:
```rust
//! paavo-web library — exposed so integration tests can construct a router.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name.
pub const CRATE_NAME: &str = "paavo-web";

pub mod app;
pub mod config;
pub mod db;
pub mod pages;
```

`crates/paavo-web/src/config.rs`:
```rust
//! paavo-web only reads the bits of paavo.toml it needs (state_dir + bind).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Subset of paavo.toml relevant to the UI.
#[derive(Debug, Clone, Deserialize)]
pub struct RootConfig {
    /// `[server]` (state_dir).
    pub server: ServerSection,
    /// `[web]` (bind).
    pub web: WebSection,
}

/// `[server]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    /// State dir containing paavo.sqlite.
    pub state_dir: PathBuf,
}

/// `[web]`.
#[derive(Debug, Clone, Deserialize)]
pub struct WebSection {
    /// `host:port`.
    pub bind: String,
}

impl RootConfig {
    /// Load from path.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        Ok(toml::from_str(&raw).context("parsing paavo.toml")?)
    }
}
```

`crates/paavo-web/src/db.rs`:
```rust
//! Read-only DB handle + minimal typed queries used by the pages.

use paavo_db::{BoardRow, Db, JobRow};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

/// Read-only DB.
#[derive(Clone)]
pub struct WebDb {
    inner: Arc<Mutex<Db>>,
}

impl WebDb {
    /// Open paavo.sqlite in WAL+RO mode.
    pub fn open(path: &Path) -> paavo_db::Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(Db::open_readonly(path)?)),
        })
    }

    /// All boards.
    pub fn all_boards(&self) -> paavo_db::Result<Vec<BoardRow>> {
        BoardRow::list_all(self.inner.lock().raw_conn())
    }

    /// `limit` most recent jobs across all states.
    pub fn recent_jobs(&self, limit: u32) -> paavo_db::Result<Vec<JobRow>> {
        let db = self.inner.lock();
        let mut stmt = db.raw_conn().prepare(
            "SELECT id, priority, submitter, source, board_selector,
                    inactivity_timeout_ms, hard_max_ms, state, outcome_detail,
                    board_id, submitted_at, started_at, finished_at,
                    tar_blake3, tar_path, elf_path
             FROM job ORDER BY submitted_at DESC LIMIT ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![limit as i64])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(decode_job_row(r)?);
        }
        Ok(out)
    }

    /// One job by id.
    pub fn job(&self, id: &paavo_proto::JobId) -> paavo_db::Result<Option<JobRow>> {
        JobRow::find(self.inner.lock().raw_conn(), id)
    }

    /// Up to `limit` log frames for a job.
    pub fn job_logs(
        &self,
        id: &paavo_proto::JobId,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_proto::LogFrame>> {
        use paavo_db::LogFrameDb;
        paavo_proto::LogFrame::list(self.inner.lock().raw_conn(), id, 0, limit)
    }
}

fn decode_job_row(r: &rusqlite::Row<'_>) -> paavo_db::Result<JobRow> {
    use paavo_proto::*;
    use std::str::FromStr;
    let id_str: String = r.get("id")?;
    let priority_i: i64 = r.get("priority")?;
    let priority = if priority_i == 0 { Priority::Interactive } else { Priority::Scheduled };
    let submitter: String = r.get("submitter")?;
    let source = if r.get::<_, String>("source")? == "scheduler" { JobSource::Scheduler } else { JobSource::Cli };
    let sel_json: String = r.get("board_selector")?;
    let sel: BoardSelector = serde_json::from_str(&sel_json)?;
    let inactivity: i64 = r.get("inactivity_timeout_ms")?;
    let hardmax: i64 = r.get("hard_max_ms")?;
    let state = match r.get::<_, String>("state")?.as_str() {
        "submitted" => JobState::Submitted,
        "building" => JobState::Building,
        "running" => JobState::Running,
        "passed" => JobState::Passed,
        "failed" => JobState::Failed,
        "timedout" => JobState::TimedOut,
        "aborted" => JobState::Aborted,
        _ => JobState::Failed,
    };
    let outcome_json: Option<String> = r.get("outcome_detail")?;
    let outcome = outcome_json
        .map(|j| serde_json::from_str::<JobOutcome>(&j))
        .transpose()?;
    let board_id: Option<String> = r.get("board_id")?;
    let submitted_at: i64 = r.get("submitted_at")?;
    let started_at: Option<i64> = r.get("started_at")?;
    let finished_at: Option<i64> = r.get("finished_at")?;
    let tar_blake3: String = r.get("tar_blake3")?;
    let tar_path: String = r.get("tar_path")?;
    let elf_path: Option<String> = r.get("elf_path")?;
    Ok(JobRow {
        id: JobId::from_str(&id_str).map_err(|_| paavo_db::DbError::UnknownEnum {
            column: "job.id",
            value: id_str,
        })?,
        priority,
        submitter,
        source,
        board_selector: sel,
        inactivity_timeout_ms: inactivity as u64,
        hard_max_ms: hardmax as u64,
        state,
        outcome,
        board_id,
        submitted_at,
        started_at,
        finished_at,
        tar_blake3,
        tar_path,
        elf_path,
    })
}
```

`crates/paavo-web/src/app.rs`:
```rust
//! axum router.

use crate::db::WebDb;
use axum::routing::get;
use axum::Router;

/// Build the router.
pub fn build_router(db: WebDb) -> Router {
    Router::new()
        .route("/", get(crate::pages::dashboard::render))
        .route("/jobs", get(crate::pages::jobs_list::render))
        .route("/jobs/:id", get(crate::pages::job_detail::render))
        .route("/boards", get(crate::pages::boards::render))
        .route("/schedule", get(crate::pages::schedule::render))
        .with_state(db)
}
```

`crates/paavo-web/src/pages/mod.rs`:
```rust
//! HTML pages.

pub mod boards;
pub mod dashboard;
pub mod job_detail;
pub mod jobs_list;
pub mod schedule;

use axum::response::Html;

/// HTML shell wrapping a page body. Pulls in the UnoCSS CDN runtime so
/// utility classes work without a build step. Mono font + zinc palette
/// throughout for the "techy + clean + easy-to-read" feel.
pub fn html_shell(title: &str, body: String) -> Html<String> {
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{title} — paavo</title>
<script src="https://cdn.jsdelivr.net/npm/@unocss/runtime"></script>
<script>
  // Pin UnoCSS preset before the runtime applies. Defaults are fine for v1.
  window.__unocss = {{
    presets: [() => window.__unocss_runtime?.presets.uno()],
  }};
</script>
</head>
<body class="font-mono text-zinc-900 bg-zinc-50 leading-relaxed">
<nav class="sticky top-0 backdrop-blur bg-zinc-50/80 border-b border-zinc-200 px-6 py-3 flex gap-6">
  <a href="/" class="hover:text-blue-700">dashboard</a>
  <a href="/jobs" class="hover:text-blue-700">jobs</a>
  <a href="/boards" class="hover:text-blue-700">boards</a>
  <a href="/schedule" class="hover:text-blue-700">schedule</a>
</nav>
<main class="max-w-5xl mx-auto p-6">
{body}
</main>
</body>
</html>"#
    ))
}

pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Map a `JobState` to its UnoCSS color class. Used by every page that
/// displays a state badge or a state column.
pub(crate) fn state_class(s: paavo_proto::JobState) -> &'static str {
    use paavo_proto::JobState::*;
    match s {
        Passed => "text-emerald-700",
        Failed | TimedOut | Aborted => "text-rose-700",
        Running | Building => "text-blue-700",
        Submitted => "text-zinc-600",
    }
}

/// Map a `BoardHealth` to its UnoCSS color class.
pub(crate) fn health_class(h: paavo_proto::BoardHealth) -> &'static str {
    match h {
        paavo_proto::BoardHealth::Healthy => "text-emerald-700",
        paavo_proto::BoardHealth::Quarantined => "text-rose-700",
    }
}
```

`crates/paavo-web/src/pages/dashboard.rs`:
```rust
//! `/` dashboard.

use crate::db::WebDb;
use axum::extract::State;
use axum::response::Html;

/// Shared utility-class snippets used across this file.
const H1: &str = "text-2xl font-semibold mb-4";
const H2: &str = "text-lg font-semibold mt-8 mb-2 text-zinc-700";
const TABLE: &str = "w-full text-sm";
const TH: &str = "text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300";
const TD: &str = "py-1.5 border-b border-zinc-200";

/// Render.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let boards = db.all_boards().unwrap_or_default();
    let jobs = db.recent_jobs(20).unwrap_or_default();

    let mut body = format!(r#"<h1 class="{H1}">paavo</h1>"#);
    body.push_str(&format!(
        r#"<p class="text-zinc-600"><span class="font-semibold text-zinc-900">{}</span> boards · <span class="font-semibold text-zinc-900">{}</span> recent jobs</p>"#,
        boards.len(),
        jobs.len()
    ));
    body.push_str(&format!(r#"<h2 class="{H2}">Board fleet</h2>"#));
    body.push_str(&format!(
        r#"<table class="{TABLE}"><thead><tr><th class="{TH}">id</th><th class="{TH}">kind</th><th class="{TH}">health</th><th class="{TH}">last used</th></tr></thead><tbody>"#
    ));
    for b in &boards {
        body.push_str(&format!(
            r#"<tr><td class="{TD}">{id}</td><td class="{TD}">{kind}</td><td class="{TD} {hc}">{h:?}</td><td class="{TD}">{lu}</td></tr>"#,
            id = super::html_escape(&b.spec.id),
            kind = super::html_escape(&b.spec.kind),
            hc = super::health_class(b.spec.health),
            h = b.spec.health,
            lu = b.last_used_at.map(|t| t.to_string()).unwrap_or_else(|| "—".into()),
        ));
    }
    body.push_str("</tbody></table>");

    body.push_str(&format!(r#"<h2 class="{H2}">Recent jobs</h2>"#));
    body.push_str(&format!(
        r#"<table class="{TABLE}"><thead><tr><th class="{TH}">id</th><th class="{TH}">state</th><th class="{TH}">priority</th><th class="{TH}">submitter</th></tr></thead><tbody>"#
    ));
    for j in &jobs {
        body.push_str(&format!(
            r#"<tr><td class="{TD}"><a class="text-blue-700 hover:underline" href="/jobs/{id}">{id}</a></td><td class="{TD} {sc}">{s:?}</td><td class="{TD}">{p:?}</td><td class="{TD}">{u}</td></tr>"#,
            id = j.id,
            sc = super::state_class(j.state),
            s = j.state,
            p = j.priority,
            u = super::html_escape(&j.submitter),
        ));
    }
    body.push_str("</tbody></table>");
    super::html_shell("dashboard", body)
}
```

`crates/paavo-web/src/pages/jobs_list.rs`:
```rust
//! `/jobs`.

use crate::db::WebDb;
use axum::extract::{Query, State};
use axum::response::Html;
use std::collections::HashMap;

/// Render.
pub async fn render(
    State(db): State<WebDb>,
    Query(q): Query<HashMap<String, String>>,
) -> Html<String> {
    let limit: u32 = q
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100)
        .min(500);
    let jobs = db.recent_jobs(limit).unwrap_or_default();
    let mut body = format!(
        r#"<h1 class="text-2xl font-semibold mb-4">jobs <span class="text-zinc-500 font-normal text-base">(last {})</span></h1>"#,
        jobs.len()
    );
    body.push_str(
        r#"<table class="w-full text-sm"><thead><tr>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">id</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">state</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">priority</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">submitter</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">submitted</th>
</tr></thead><tbody>"#,
    );
    for j in &jobs {
        body.push_str(&format!(
            r#"<tr>
<td class="py-1.5 border-b border-zinc-200"><a class="text-blue-700 hover:underline" href="/jobs/{id}">{id}</a></td>
<td class="py-1.5 border-b border-zinc-200 {sc}">{s:?}</td>
<td class="py-1.5 border-b border-zinc-200">{p:?}</td>
<td class="py-1.5 border-b border-zinc-200">{u}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{ts}</td>
</tr>"#,
            id = j.id,
            sc = super::state_class(j.state),
            s = j.state,
            p = j.priority,
            u = super::html_escape(&j.submitter),
            ts = j.submitted_at,
        ));
    }
    body.push_str("</tbody></table>");
    super::html_shell("jobs", body)
}
```

`crates/paavo-web/src/pages/job_detail.rs`:
```rust
//! `/jobs/:id`.

use crate::db::WebDb;
use axum::extract::{Path, State};
use axum::response::Html;
use std::str::FromStr;

/// Render.
pub async fn render(State(db): State<WebDb>, Path(id): Path<String>) -> Html<String> {
    let id = match paavo_proto::JobId::from_str(&id) {
        Ok(i) => i,
        Err(_) => {
            return super::html_shell(
                "job",
                r#"<p class="text-rose-700">invalid id</p>"#.into(),
            )
        }
    };
    let job = match db.job(&id).ok().flatten() {
        Some(j) => j,
        None => {
            return super::html_shell(
                "job",
                r#"<p class="text-rose-700">not found</p>"#.into(),
            )
        }
    };
    let logs = db.job_logs(&id, 5000).unwrap_or_default();
    let mut body = format!(
        r#"<h1 class="text-2xl font-semibold mb-2">job <code class="text-base bg-zinc-100 px-2 py-0.5 rounded">{id}</code></h1>
<p class="text-zinc-600 mb-4">state: <span class="font-semibold {sc}">{state:?}</span> · priority: <span class="text-zinc-900 font-semibold">{prio:?}</span> · submitter: <span class="text-zinc-900">{sub}</span></p>"#,
        id = job.id,
        sc = super::state_class(job.state),
        state = job.state,
        prio = job.priority,
        sub = super::html_escape(&job.submitter),
    );
    if let Some(o) = &job.outcome {
        body.push_str(&format!(
            r#"<p class="mb-4">outcome: <code class="bg-zinc-100 px-2 py-0.5 rounded text-sm">{}</code></p>"#,
            super::html_escape(&serde_json::to_string(o).unwrap_or_default())
        ));
    }
    body.push_str(r#"<h2 class="text-lg font-semibold mt-6 mb-2 text-zinc-700">log</h2>"#);
    body.push_str(r#"<pre class="bg-zinc-900 text-zinc-100 text-xs leading-snug p-4 rounded overflow-x-auto">"#);
    for f in logs.iter().take(2000) {
        body.push_str(&format!(
            "{:>10} [{:?}] {}\n",
            f.ts_us,
            f.level,
            super::html_escape(&f.message)
        ));
    }
    body.push_str("</pre>");
    super::html_shell("job", body)
}
```

`crates/paavo-web/src/pages/boards.rs`:
```rust
//! `/boards`.

use crate::db::WebDb;
use axum::extract::State;
use axum::response::Html;

/// Render.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let rows = db.all_boards().unwrap_or_default();
    let mut body = String::from(
        r#"<h1 class="text-2xl font-semibold mb-4">boards</h1>
<table class="w-full text-sm"><thead><tr>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">id</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">kind</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">health</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">infra fails</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">last used</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">reason</th>
</tr></thead><tbody>"#,
    );
    for b in &rows {
        body.push_str(&format!(
            r#"<tr>
<td class="py-1.5 border-b border-zinc-200">{id}</td>
<td class="py-1.5 border-b border-zinc-200">{k}</td>
<td class="py-1.5 border-b border-zinc-200 {hc}">{h:?}</td>
<td class="py-1.5 border-b border-zinc-200">{n}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{lu}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{r}</td>
</tr>"#,
            id = super::html_escape(&b.spec.id),
            k = super::html_escape(&b.spec.kind),
            hc = super::health_class(b.spec.health),
            h = b.spec.health,
            n = b.consecutive_infra_failures,
            lu = b.last_used_at.map(|t| t.to_string()).unwrap_or_else(|| "—".into()),
            r = super::html_escape(&b.quarantine_reason.clone().unwrap_or_default()),
        ));
    }
    body.push_str("</tbody></table>");
    super::html_shell("boards", body)
}
```

`crates/paavo-web/src/pages/schedule.rs`:
```rust
//! `/schedule`.

use crate::db::WebDb;
use axum::extract::State;
use axum::response::Html;

/// Render.
pub async fn render(State(_db): State<WebDb>) -> Html<String> {
    // paavo-db doesn't expose a schedule.list_all helper yet; once paavod's
    // cron has fired at least once, M4.3.c writes a row that we can render
    // here. For now, emit a placeholder row.
    let mut body = String::from(
        r#"<h1 class="text-2xl font-semibold mb-4">schedule</h1>
<table class="w-full text-sm"><thead><tr>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">id</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">cron</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">enabled</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">last triggered</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">last completed</th>
</tr></thead><tbody>"#,
    );
    body.push_str(
        r#"<tr><td colspan="5" class="py-3 text-zinc-500 italic">schedule table contents render via paavod's cron after first firing</td></tr>"#,
    );
    body.push_str("</tbody></table>");
    super::html_shell("schedule", body)
}
```

`crates/paavo-web/src/main.rs` (replace skeleton):
```rust
//! paavo-web entry point.
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "paavo-web", version)]
struct Args {
    /// Path to paavo.toml.
    #[arg(long, env = "PAAVO_CONFIG", default_value = "/etc/paavo/paavo.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let cfg = paavo_web::config::RootConfig::load(&args.config)?;
    let sqlite_path = cfg.server.state_dir.join("paavo.sqlite");
    let db = paavo_web::db::WebDb::open(&sqlite_path)?;
    let listener = tokio::net::TcpListener::bind(&cfg.web.bind).await?;
    tracing::info!(bind=%cfg.web.bind, "paavo-web listening");
    axum::serve(listener, paavo_web::app::build_router(db)).await?;
    Ok(())
}
```

- [ ] **Step 3: Smoke test**

`crates/paavo-web/tests/smoke.rs`:
```rust
use axum::body::to_bytes;
use axum::http::Request;
use paavo_db::Db;
use paavo_web::db::WebDb;
use tempfile::tempdir;
use tower::ServiceExt;

#[tokio::test]
async fn dashboard_renders_on_empty_db() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let _ = Db::open(&path).unwrap();
    let db = WebDb::open(&path).unwrap();
    let app = paavo_web::app::build_router(db);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert!(body.contains("Board fleet"), "{body}");
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p paavo-web`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```pwsh
git -C D:\workspace\paavo add crates/paavo-web
git -C D:\workspace\paavo commit -m "feat(web): RO sqlite viewer — dashboard / jobs / boards / schedule pages"
```

---

### Milestone 5 exit criteria

- [ ] `paavo-web` opens the same sqlite file paavod writes to in WAL+RO mode
- [ ] 5 pages render without panic on an empty DB
- [ ] `cargo test --workspace` green

---

## Milestone 6 — Templates, soak, ops

Goal: scaffolding templates for `cargo-generate`, one initial soak test crate, systemd units, udev rules, README + deployment doc, hardware smoke checklist.

### Task 6.1: Linker fragments + shared template assets

Spec coverage: §12.4 (vendored `link_ram_cortex_m.x` with attribution).

**Files:**
- Create: `templates/shared/link_ram_cortex_m.x`
- Create: `templates/shared/README.md`

- [ ] **Step 1: Vendor link_ram_cortex_m.x**

Run:
```pwsh
git ls-remote https://github.com/embassy-rs/teleprobe.git HEAD
```
Capture the SHA. Then download the file:
```pwsh
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/embassy-rs/teleprobe/<SHA>/link_ram_cortex_m.x" -OutFile "D:\workspace\paavo\templates\shared\link_ram_cortex_m.x"
```

Open the file and insert these lines at the very top (above existing content):
```
/*
 * Vendored from https://github.com/embassy-rs/teleprobe
 * at commit <SHA you fetched>, licensed MIT OR Apache-2.0.
 * Used unchanged. Refresh if the upstream version changes.
 */
```

- [ ] **Step 2: Templates README**

`templates/shared/README.md`:
```markdown
# Shared template assets

- `link_ram_cortex_m.x` — vendored verbatim from `embassy-rs/teleprobe`.
  Each board-kind template wires it into `OUT_DIR` via `build.rs` and
  passes `-Tlink_ram.x` to the linker so test binaries run from RAM
  (matching the upstream embassy convention).
```

- [ ] **Step 3: Commit**

```pwsh
git -C D:\workspace\paavo add templates
git -C D:\workspace\paavo commit -m "templates: vendor link_ram_cortex_m.x from teleprobe (attribution header)"
```

---

### Task 6.2: mcxa266 template

**Files:**
- Create: `templates/mcxa266/cargo-generate.toml`
- Create: `templates/mcxa266/Cargo.toml.liquid`
- Create: `templates/mcxa266/memory.x`
- Create: `templates/mcxa266/build.rs`
- Create: `templates/mcxa266/.cargo/config.toml`
- Create: `templates/mcxa266/src/main.rs`

- [ ] **Step 1: cargo-generate.toml**

`templates/mcxa266/cargo-generate.toml`:
```toml
[template]
description = "paavo test crate for the mcxa266 board kind"
ignore = ["target", "Cargo.lock"]

[placeholders.test-kind]
type = "string"
prompt = "Test kind: quick or soak?"
choices = ["quick", "soak"]
default = "quick"

[placeholders.embassy-rev]
type = "string"
prompt = "embassy-rs/embassy revision (branch, tag, or SHA)"
default = "main"
```

- [ ] **Step 2: Cargo.toml.liquid**

`templates/mcxa266/Cargo.toml.liquid`:
```toml
[package]
name = "{{crate_name}}"
version = "0.1.0"
edition = "2021"

[dependencies]
cortex-m       = { version = "0.7", features = ["critical-section-single-core"] }
cortex-m-rt    = "0.7"
defmt          = "0.3"
defmt-rtt      = "0.4"
panic-probe    = { version = "0.3", features = ["print-defmt"] }
embassy-executor = { git = "https://github.com/embassy-rs/embassy", rev = "{{embassy-rev}}", features = ["arch-cortex-m", "executor-thread", "defmt"] }
embassy-time     = { git = "https://github.com/embassy-rs/embassy", rev = "{{embassy-rev}}", features = ["defmt", "defmt-timestamp-uptime"] }
embassy-mcxa     = { git = "https://github.com/embassy-rs/embassy", rev = "{{embassy-rev}}", features = ["mcxa266vfl", "defmt", "time-driver-any"] }
paavo-meta       = { git = "https://github.com/felipebalbi/paavo" }
```

- [ ] **Step 3: memory.x**

`templates/mcxa266/memory.x`:
```
/* mcxa266 default memory map. Confirm against the NXP datasheet for your
 * exact part number before first run. */
MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 1M
  RAM   : ORIGIN = 0x20000000, LENGTH = 192K
}
```

- [ ] **Step 4: build.rs**

`templates/mcxa266/build.rs`:
```rust
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("link_ram.x"),
        include_str!("../shared/link_ram_cortex_m.x"),
    )
    .unwrap();
    fs::write(out.join("memory.x"), include_str!("memory.x")).unwrap();

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tlink_ram.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../shared/link_ram_cortex_m.x");
}
```

- [ ] **Step 5: .cargo/config.toml**

`templates/mcxa266/.cargo/config.toml`:
```toml
[build]
target = "thumbv8m.main-none-eabihf"

[target.thumbv8m.main-none-eabihf]
rustflags = [
  "-C", "link-arg=-Tdefmt.x",
]
```

- [ ] **Step 6: src/main.rs**

`templates/mcxa266/src/main.rs`:
```rust
//! {{crate_name}} — paavo test crate.

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

paavo_meta::target!(b"frdm-mcx-a266");
paavo_meta::timeout!(60);
paavo_meta::inactivity_timeout!(30);

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_mcxa::init(Default::default());
    info!("hello from {{crate_name}}");
    // TODO: write your test here.
    Timer::after(Duration::from_secs(1)).await;
    info!("Test OK");
    cortex_m::asm::bkpt();
}
```

- [ ] **Step 7: Commit**

```pwsh
git -C D:\workspace\paavo add templates/mcxa266
git -C D:\workspace\paavo commit -m "templates(mcxa266): cargo-generate template (Cargo.toml + build.rs + main.rs)"
```

---

### Task 6.3: rt685-evk template

Mirror Task 6.2 verbatim in `templates/rt685-evk/`, with these differences:

- `Cargo.toml.liquid`: replace `embassy-mcxa = { ..., features = ["mcxa266vfl", ...] }` with `embassy-imxrt = { git = "https://github.com/embassy-rs/embassy", rev = "{{embassy-rev}}", features = ["rt685s", "defmt", "time-driver-any"] }`.
- `memory.x`: rt685s memory map (Flash + SRAM regions per NXP RT685S datasheet — operator confirms before first run).
- `.cargo/config.toml`: `target = "thumbv8m.main-none-eabihf"` is correct for rt685s too; no change.
- `src/main.rs`: `paavo_meta::target!(b"rt685-evk");` and replace `embassy_mcxa::init` with `embassy_imxrt::init`.

- [ ] **Step 1: Create the six files** mirroring Task 6.2 with the diffs above.
- [ ] **Step 2: Commit**

```pwsh
git -C D:\workspace\paavo add templates/rt685-evk
git -C D:\workspace\paavo commit -m "templates(rt685-evk): cargo-generate template"
```

---

### Task 6.4: First soak test crate

**Files:**
- Create: `soak-tests/mcxa266/dma-stress-overnight/Cargo.toml`
- Create: `soak-tests/mcxa266/dma-stress-overnight/build.rs`
- Create: `soak-tests/mcxa266/dma-stress-overnight/memory.x`
- Create: `soak-tests/mcxa266/dma-stress-overnight/src/main.rs`
- Create: `soak-tests/mcxa266/dma-stress-overnight/.cargo/config.toml`

- [ ] **Step 1: Add `soak-tests/` to workspace exclude FIRST**

Edit `D:\workspace\paavo\Cargo.toml`'s `[workspace]` block to add `exclude = ["soak-tests"]` (before `members`).

- [ ] **Step 2: Stand up the test crate** — copy the six files from `templates/mcxa266/` into `soak-tests/mcxa266/dma-stress-overnight/`, and replace `src/main.rs` with:

```rust
//! dma-stress-overnight — exercises the embassy-mcxa DMA driver for hours.
//!
//! Skeleton: replace the body with the real stress loop before promoting to
//! the nightly corpus.

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

paavo_meta::target!(b"frdm-mcx-a266");
paavo_meta::timeout!(14400);            // 4 h
paavo_meta::inactivity_timeout!(120);   // 2 min

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_mcxa::init(Default::default());
    info!("dma-stress-overnight skeleton");
    for i in 0u32..10 {
        Timer::after(Duration::from_secs(1)).await;
        info!("tick {=u32}", i);
    }
    info!("Test OK");
    cortex_m::asm::bkpt();
}
```

Also fix the package name in the soak test's `Cargo.toml`:
```toml
[package]
name = "dma-stress-overnight"
version = "0.1.0"
edition = "2021"
```
(Keep the dependencies block the same as the template.)

- [ ] **Step 3: Verify the workspace still builds**

Run: `cargo build --workspace`
Expected: green (the soak test crate is excluded).

- [ ] **Step 4: Commit**

```pwsh
git -C D:\workspace\paavo add soak-tests Cargo.toml
git -C D:\workspace\paavo commit -m "soak-tests(mcxa266): dma-stress-overnight skeleton"
```

---

### Task 6.5: contrib — systemd + udev + sample paavo.toml

**Files:**
- Create: `contrib/paavod.service`
- Create: `contrib/paavo-web.service`
- Create: `contrib/paavo.toml.example`
- Create: `contrib/99-probes.rules`
- Create: `contrib/README.md`

- [ ] **Step 1: paavod.service**

`contrib/paavod.service`:
```ini
[Unit]
Description=paavo daemon (HIL test runner)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=paavo
Group=paavo
ExecStart=/usr/local/bin/paavod --config /etc/paavo/paavo.toml
Restart=on-failure
RestartSec=5
StateDirectory=paavo
ReadWritePaths=/var/lib/paavo
ProtectSystem=strict
ProtectHome=true
NoNewPrivileges=true
PrivateTmp=true
KillSignal=SIGTERM
TimeoutStopSec=90

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 2: paavo-web.service**

`contrib/paavo-web.service`:
```ini
[Unit]
Description=paavo read-only web viewer
After=paavod.service
PartOf=paavod.service

[Service]
Type=simple
User=paavo
Group=paavo
ExecStart=/usr/local/bin/paavo-web --config /etc/paavo/paavo.toml
Restart=on-failure
RestartSec=5
ProtectSystem=strict
ProtectHome=true
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 3: paavo.toml.example**

`contrib/paavo.toml.example` — copy verbatim from spec §13, with `# inline comments` explaining each block. The block content is identical to the spec.

- [ ] **Step 4: 99-probes.rules**

`contrib/99-probes.rules`:
```
# probe-rs udev rules (excerpt). Adjust for the probes in your lab.
# Source: https://probe.rs/docs/getting-started/probe-setup/

# Daplink (e.g. mcxa eval boards)
SUBSYSTEM=="usb", ATTRS{idVendor}=="0d28", ATTRS{idProduct}=="0204", MODE="0660", GROUP="plugdev"

# Segger J-Link
SUBSYSTEM=="usb", ATTRS{idVendor}=="1366", MODE="0660", GROUP="plugdev"

# ST-Link (in case you mix)
SUBSYSTEM=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3748", MODE="0660", GROUP="plugdev"
```

- [ ] **Step 5: contrib/README.md**

`contrib/README.md`:
````markdown
# contrib

Deployment assets. paavo does **not** install these for you.

## Install

```bash
sudo install -d /etc/paavo /var/lib/paavo
sudo install -m 0644 paavo.toml.example /etc/paavo/paavo.toml   # then edit
sudo useradd --system --home /var/lib/paavo paavo
sudo install -m 0755 ../target/release/paavod    /usr/local/bin/
sudo install -m 0755 ../target/release/paavo-web /usr/local/bin/
sudo install -m 0644 paavod.service    /etc/systemd/system/
sudo install -m 0644 paavo-web.service /etc/systemd/system/
sudo install -m 0644 99-probes.rules   /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
sudo systemctl daemon-reload
sudo systemctl enable --now paavod.service paavo-web.service
```
````

- [ ] **Step 6: Commit**

```pwsh
git -C D:\workspace\paavo add contrib
git -C D:\workspace\paavo commit -m "contrib: systemd units + sample paavo.toml + probe udev rules + install README"
```

---

### Task 6.6: README + deployment doc + HW smoke checklist

**Files:**
- Modify: `README.md`
- Create: `docs/deployment.md`
- Create: `docs/hw-smoke-checklist.md`

- [ ] **Step 1: README**

Replace `README.md`:
````markdown
# paavo

Self-hosted Linux hardware-in-the-loop test runner for the `embassy-mcxa`
HAL (and any future embassy chip wired into the lab).

Named after Paavo Nurmi — Olympic distance runner — a fit for a test runner
whose nightly job is hours-long stability soaks.

## Quick start (dev workstation)

```bash
cargo install --git https://github.com/felipebalbi/paavo paavo-cli
export PAAVO_HOST=http://lab.local:8080
paavo-cli new my-dma-test --board-kind mcxa266
cd my-dma-test
$EDITOR src/main.rs
paavo-cli run . --board-kind mcxa266 --timeout 30m
```

## Quick start (lab machine)

See [`docs/deployment.md`](docs/deployment.md) and
[`contrib/README.md`](contrib/README.md).

## Design

- Full design:
  [`docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md`](docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md)
- Implementation plan:
  [`docs/superpowers/plans/2026-06-09-paavo-implementation.md`](docs/superpowers/plans/2026-06-09-paavo-implementation.md)
- HW smoke checklist for releases:
  [`docs/hw-smoke-checklist.md`](docs/hw-smoke-checklist.md)

## License

Dual-licensed under MIT or Apache-2.0 at your option.
````

- [ ] **Step 2: docs/deployment.md**

`docs/deployment.md`:
````markdown
# Deployment

paavo is supported on Linux only (daemon). `paavo-cli` runs anywhere.

## Required system packages (Ubuntu/Debian)

```
sudo apt-get install -y libudev-dev pkg-config build-essential
```

## Build & install

```bash
git clone https://github.com/felipebalbi/paavo /opt/paavo
cd /opt/paavo
cargo build --release -p paavod -p paavo-web
sudo install -m 0755 target/release/paavod    /usr/local/bin/
sudo install -m 0755 target/release/paavo-web /usr/local/bin/
```

Then follow [`contrib/README.md`](../contrib/README.md) for systemd + udev.

## State directory layout

`/var/lib/paavo/`:

- `paavo.sqlite` (+ WAL files) — single writer (paavod), single reader (paavo-web).
- `uploads/` — incoming crate tars, keyed by blake3.
- `sandboxes/` — per-job build dirs.
- `cargo-target/` — shared `CARGO_TARGET_DIR` for cargo's incremental reuse.
- `cache/elf/` — cached ELFs.
- `boards.toml` — `paavo-cli board add` writes this; restart paavod to pick up changes.

## Updating

```bash
cd /opt/paavo && git pull
cargo build --release -p paavod -p paavo-web
sudo install -m 0755 target/release/paavod    /usr/local/bin/
sudo install -m 0755 target/release/paavo-web /usr/local/bin/
sudo systemctl restart paavod.service paavo-web.service
```
````

- [ ] **Step 3: docs/hw-smoke-checklist.md**

`docs/hw-smoke-checklist.md`:
```markdown
# HW smoke checklist

Run after every release tag, manually, against real hardware. Captures
parts not covered by `cargo test --workspace`.

1. Start paavod against an `mcxa266` + an `rt685-evk` board in `boards.toml`.
2. `paavo-cli boards` lists both as `healthy`.
3. Submit a passing test:
   ```bash
   paavo-cli new smoke-pass --board-kind mcxa266
   cd smoke-pass
   paavo-cli run . --board-kind mcxa266
   ```
   Confirm: terminal line includes `"passed"`, exit 0, log stream printed.
4. Submit a panicking test — replace the body of `src/main.rs` with `panic!("smoke");` and:
   ```bash
   paavo-cli run . --board-kind mcxa266
   ```
   Confirm: terminal line includes `"failed":` (object with details), exit 1, panic message visible in log.
5. Submit a hanging test (loop with no defmt). Confirm inactivity watchdog
   fires within `~2 × default_inactivity_s`.
6. `kill -TERM $(pidof paavod)` while a job is running. Confirm the job
   ends in `aborted{daemon_shutdown}` within `shutdown_grace_s + 5s`.
7. `paavo-web` at `http://127.0.0.1:8081/` shows all of the above.
8. After 3 consecutive `Failed{InfraErr}` outcomes (unplug probe), board
   is auto-quarantined; `paavo-cli boards` shows it as `quarantined`.
9. `paavo-cli board unquarantine <id>` brings it back to `healthy`.
10. Cancel a running job mid-flight: `paavo-cli cancel <id>` returns
    `204`; job ends in `aborted{user}`.
```

- [ ] **Step 4: Commit**

```pwsh
git -C D:\workspace\paavo add README.md docs
git -C D:\workspace\paavo commit -m "docs: README quickstart + deployment guide + HW smoke checklist"
```

---

### Milestone 6 exit criteria

- [ ] `templates/mcxa266` and `templates/rt685-evk` accept `cargo generate` and produce a buildable crate (manual check on the lab machine)
- [ ] `soak-tests/mcxa266/dma-stress-overnight` exists; workspace `exclude` keeps it out of `cargo build --workspace`
- [ ] `contrib/` has systemd units + sample paavo.toml + udev rules + install README
- [ ] `docs/deployment.md` and `docs/hw-smoke-checklist.md` exist
- [ ] HW smoke checklist (Task 6.6 / docs) completes end-to-end on real mcxa266 + rt685-evk (manual)

---

## Whole-workspace exit criteria

- [ ] `cargo build --workspace` green
- [ ] `cargo test --workspace` green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` green
- [ ] `cargo fmt --all -- --check` green
- [ ] CI workflow (`.github/workflows/ci.yml`) green on push
- [ ] Every section §1–§17 of the spec has at least one task implementing it (see self-review section below)
- [ ] HW smoke checklist (`docs/hw-smoke-checklist.md`) passes on real mcxa266 + rt685-evk

---

## Self-review notes

Done after writing the plan, applied inline. Captured here for the executor's reference.

**Spec coverage map:**

| Spec section | Implementing task(s) |
|---|---|
| §1 Purpose & scope | M0 (workspace), M2-M4 (daemon/cli/probe); no out-of-scope work |
| §2 Background / prior art | n/a (history) |
| §3 Architecture overview, §3.1 data flow, §3.2 daemon/UI split | M0 (10 crates), M4 (paavod), M5 (paavo-web) |
| §4 Workspace layout, §4.1 boundary rules, §4.2 split rationale, §4.3 BoardWorker concurrency | M0.2 (boundary-correct Cargo.toml deps), M2.2 (OS thread + watchdog) |
| §5 Job lifecycle, §5.1 states, §5.2 outcome table, §5.3 priority, §5.4 cancel, §5.5 selector | M1.1 (types), M3.2.b/c/d (enqueue, scheduler, cancel), M3.2.d (quarantine policy), M4.3.a (cancel registry) |
| §6 Timeouts / watchdog / drain, §6.1–§6.4 | M2.2 (watchdog + WatchdogState), M4.3.d (SIGTERM drain), M1.2 (inactivity_timeout!() macro) |
| §7 Storage model (5 tables), §7.6 retention | M1.3.a (schema + migrations), M1.3.b/c/d/e/f (typed helpers), M1.3.d (truncate_old_passed) |
| §8 Build env, §8.1 sandbox, §8.2 cache | M3.1.a/b/c/d |
| §9 HTTP API | M4.2.a/b/c (all routes) |
| §10 CLI surface | M4.5.a/b (clap surface + e2e test) |
| §11 Web UI | M5.1 (5 pages) |
| §12 cargo-generate templates, §12.4 shared linker | M6.1 (linker), M6.2 (mcxa266), M6.3 (rt685-evk) |
| §13 Configuration | M4.1 (config schema + loader) |
| §14 Deployment, §14.1 systemd, §14.2 udev, §14.3 security | M6.5 (contrib) |
| §15 Testing strategy | applied throughout — every public function has a TDD test |
| §16 Prerequisite work (inactivity_timeout!() upstream) | M1.2 (lives in paavo-meta until upstreamed; spec §16.1) |
| §17 Deferred-to-v2 | n/a (no tasks) |

**Type consistency notes (resolved during write):**

- `JobOutcome::Failed(TerminalOutcome)` — both `paavo-runner` and `paavo-core` quarantine policy use this tagging consistently.
- `JobOutcome` is **externally-tagged** (default serde), not internally-tagged. Wire forms: `"passed"` / `{"failed":{...}}` / `{"timed_out":{...}}` / `{"aborted":{...}}`. Internal tagging (`#[serde(tag="outcome")]`) was rejected because it doesn't support the `Failed(TerminalOutcome)` tuple variant.
- `JobState::TimedOut` uses an **explicit `#[serde(rename = "timedout")]`** (one word) instead of `rename_all = "snake_case"` (which would emit `"timed_out"`). This matches the SQL CHECK constraint and the manual string mappers. All other `JobState` variants also use explicit `#[serde(rename = "…")]` for symmetry.
- `Priority` weights `0`/`1` for Interactive/Scheduled — used both in `paavo-proto::Priority::weight` and the SQL `ORDER BY priority ASC` in `paavo-db::JobRow::list_submitted`.
- `paavo_core::cache_lookup(conn, blake3) -> CacheLookup` — signature is `(&Connection, &str)` everywhere it's called (M4.3.b). Build-cache lives in `paavo-core` (not `paavo-build`) so `paavo-build` honors spec §4.1 "depends only on paavo-proto."
- `paavo_db::LogFrameDb` trait imported wherever frames are read/written.
- `paavo-cli`'s `handle_sse_line` parses both shapes of `JobOutcome` (bare string for `Passed`, single-key object for the rest) when deciding the exit code.

**Placeholders scan:** none — every code step contains complete code; every command has expected output.

**Open implementation gotchas to expect during execution:**

1. `paavo-probe::RealSession` is stubbed (returns error) so workspace tests pass without hardware. M6.4's HW smoke checklist exercises it end-to-end against real probes; the wiring lives in `paavod::main::RealRunner`.
2. The `cron` driver requires `tokio-cron-scheduler` to align with `tokio` major version. If `tokio-cron-scheduler v0.15.1` drifts off the planned `tokio` minor, executor may need to bump to next patch.
3. Workspace `exclude = ["soak-tests"]` MUST come before `members = [...]` in `Cargo.toml`'s `[workspace]` block in some cargo versions; Task 6.4 spells this out.

---


//! clap command surface for paavo-cli. See spec section 10.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Top-level CLI.
#[derive(Parser, Debug)]
#[command(name = "paavo-cli", version, about = "paavo command-line client")]
pub struct Cli {
    /// Daemon URL. Falls back to PAAVO_HOST then ~/.config/paavo/cli.toml.
    #[arg(long, env = "PAAVO_HOST")]
    pub host: Option<String>,
    /// Subcommand.
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
        /// Board kind (e.g. mcxa266). Optional: inferred from --instance,
        /// or defaulted when the lab has a single kind. Needed only to
        /// disambiguate when multiple kinds exist and no --instance is
        /// given.
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
        /// Stream the NDJSON log until the job terminates, and use the
        /// terminal outcome as the exit code (0 = passed, non-zero
        /// otherwise). Without this, `run` is fire-and-forget: submit
        /// the tar, print the job id, exit 0. Tail later with
        /// `paavo-cli logs <id> --follow`.
        #[arg(long, short = 'f')]
        follow: bool,
        /// Force paavod to rebuild this submission instead of reusing
        /// a cached ELF for the same tar. Use when you suspect the
        /// chip or toolchain drifted between runs and want a clean
        /// "compile + flash from scratch" path. The cache row itself
        /// is left intact, so subsequent normal submits still benefit
        /// from the cache.
        #[arg(long)]
        skip_cache: bool,
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
        /// Destination directory; the scaffolded crate lands at
        /// `<into>/<name>/`. Defaults to the current working directory.
        #[arg(long)]
        into: Option<PathBuf>,
        /// Explicit templates root. Overrides the default
        /// auto-discovery (walking up from CWD for a paavo checkout).
        #[arg(long)]
        templates_path: Option<PathBuf>,
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
        /// Operation.
        #[command(subcommand)]
        op: BoardOp,
    },
    /// Admin / dev-loop operations.
    Admin {
        /// Operation.
        #[command(subcommand)]
        op: AdminOp,
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
    /// Permanently remove a board from the inventory. The board must
    /// be currently quarantined and have no referencing job rows
    /// (wait for retention to age out historical jobs first).
    Remove {
        /// Board id.
        id: String,
    },
}

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

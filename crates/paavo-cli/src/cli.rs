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
        /// Operation.
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
    /// Permanently remove a board from the inventory. The board must
    /// be currently quarantined and have no referencing job rows
    /// (wait for retention to age out historical jobs first).
    Remove {
        /// Board id.
        id: String,
    },
}

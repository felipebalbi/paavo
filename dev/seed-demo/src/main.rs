//! `seed-demo` — flood a dev paavo SQLite database with fake boards and a
//! realistic spread of jobs so the paavo-web SPA can be stress-tested
//! (pagination, fuzzy search, every state badge, live inserts) without any
//! hardware or a running daemon.
//!
//! Standalone dev binary — NOT a workspace member (see the root `Cargo.toml`
//! `exclude` list). It links `paavo-db` directly and writes rows through the
//! same typed helpers paavod uses (`BoardRow::insert`, `JobRow::insert`, the
//! job-state transitions, and `LogFrame::append_batch`), so the seeded data is
//! byte-for-byte what the daemon would have produced. Run by hand:
//!
//! ```text
//! cargo run --manifest-path dev/seed-demo/Cargo.toml -- \
//!     --db /tmp/paavo/paavo.sqlite --boards 6 --jobs 300 --trickle-ms 400
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;
use rand::Rng;

use paavo_db::{BoardRow, Db, DbError, JobRow, LogFrameDb, NewJob, OutcomeRecord};
use paavo_proto::{
    AbortReason, BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSource, JobState,
    LogFrame, LogLevel, Priority, ProbeSelector, TerminalOutcome, TimeoutReason,
};

/// CLI arguments.
#[derive(Parser, Debug)]
#[command(
    name = "seed-demo",
    about = "Seed a dev paavo SQLite DB with fake boards + jobs for UI stress-testing"
)]
struct Args {
    /// SQLite database path. Created (with parent dirs) if missing; opening it
    /// runs migrations, so a fresh path yields a fully-schema'd DB.
    #[arg(long, default_value = "/tmp/paavo/paavo.sqlite")]
    db: PathBuf,

    /// Number of fake boards to register (`mcxa266-01` ..).
    #[arg(long, default_value_t = 6)]
    boards: u32,

    /// Number of jobs to insert.
    #[arg(long, default_value_t = 300)]
    jobs: u32,

    /// If > 0, insert the last ~20 jobs one at a time with this delay (ms)
    /// between them, stamped at "now" — a watching paavo-web then shows the
    /// inserts arrive live and the "N new" pill light up.
    #[arg(long, default_value_t = 0)]
    trickle_ms: u64,
}

/// The job end-state the seeder drives a freshly-inserted row to. Determines
/// the transition path, the log-frame flavor, and (for terminal targets) the
/// finalize outcome.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Target {
    Passed,
    FailedTest,
    FailedInfra,
    TimedOut,
    Aborted,
    Running,
    Building,
    AwaitingBoard,
    Submitted,
}

/// Fixed display order for the summary, also the canonical target list.
const ALL_TARGETS: [Target; 9] = [
    Target::Passed,
    Target::FailedTest,
    Target::FailedInfra,
    Target::TimedOut,
    Target::Aborted,
    Target::Running,
    Target::Building,
    Target::AwaitingBoard,
    Target::Submitted,
];

/// Short label used in the printed summary.
fn label(t: Target) -> &'static str {
    match t {
        Target::Passed => "passed",
        Target::FailedTest => "failed(test)",
        Target::FailedInfra => "failed(infra)",
        Target::TimedOut => "timedout",
        Target::Aborted => "aborted",
        Target::Running => "running",
        Target::Building => "building",
        Target::AwaitingBoard => "awaiting_board",
        Target::Submitted => "submitted",
    }
}

/// Map a job index to a target state via fixed 0..100 buckets. The weights
/// reproduce a realistic fleet history: a strong majority pass, a tail of
/// failures and timeouts, and a handful of jobs frozen in each non-terminal
/// state so the UI shows every badge plus live (running/building/awaiting/
/// submitted) rows.
fn plan_for(idx: u32) -> Target {
    match idx % 100 {
        0..=54 => Target::Passed,         // 55%
        55..=66 => Target::FailedTest,    // 12%
        67..=71 => Target::FailedInfra,   //  5%
        72..=76 => Target::TimedOut,      //  5%
        77..=79 => Target::Aborted,       //  3%
        80..=87 => Target::Running,       //  8%
        88..=91 => Target::Building,      //  4%
        92..=95 => Target::AwaitingBoard, //  4%
        _ => Target::Submitted,           //  4% (96..=99)
    }
}

/// Terminal `(state, outcome)` for the five terminal targets. Panics if called
/// on a non-terminal target — a caller bug, since only the terminal arm of
/// [`drive_job`] reaches it.
fn terminal_record(t: Target) -> (JobState, JobOutcome) {
    match t {
        Target::Passed => (JobState::Passed, JobOutcome::Passed),
        Target::FailedTest => (
            JobState::Failed,
            JobOutcome::Failed(TerminalOutcome::TestErr {
                message: "assertion failed: dma fifo".into(),
            }),
        ),
        Target::FailedInfra => (
            JobState::Failed,
            JobOutcome::Failed(TerminalOutcome::InfraErr {
                stage: "probe_attach".into(),
                message: "probe lost".into(),
            }),
        ),
        Target::TimedOut => (
            JobState::TimedOut,
            JobOutcome::TimedOut {
                reason: TimeoutReason::Inactivity,
                elapsed_ms: 120_000,
            },
        ),
        Target::Aborted => (
            JobState::Aborted,
            JobOutcome::Aborted {
                by: AbortReason::User,
            },
        ),
        _ => unreachable!("terminal_record called on a non-terminal target"),
    }
}

/// Accumulates a small log-frame timeline with monotonic `seq` (from 0) and
/// strictly-increasing `ts_us`.
struct FrameLog {
    frames: Vec<LogFrame>,
}

impl FrameLog {
    fn new() -> Self {
        Self { frames: Vec::new() }
    }

    fn push(&mut self, level: LogLevel, target: Option<&str>, message: &str) {
        let seq = self.frames.len() as u64;
        self.frames.push(LogFrame {
            seq,
            ts_us: 1_000 + seq * 1_204_000,
            level,
            target: target.map(str::to_string),
            message: message.to_string(),
        });
    }
}

/// Build a realistic frame timeline for a job of the given target. Frames stop
/// at the point in the lifecycle the target represents (a `Building` job only
/// has cargo output; an `AwaitingBoard` job has the finished build but no run
/// frames; a `Submitted` job has none). `Passed` ends with the exact `Test OK`
/// info frame paavod's runner treats as the pass marker; failures carry an
/// `error` frame instead.
fn frames_for(target: Target) -> Vec<LogFrame> {
    if target == Target::Submitted {
        return Vec::new();
    }
    let mut log = FrameLog::new();

    // Build phase — cargo output, present once a build has started.
    log.push(
        LogLevel::Info,
        Some("cargo:stderr"),
        "   Compiling embassy-mcxa v0.1.0",
    );
    log.push(
        LogLevel::Info,
        Some("cargo:stderr"),
        "   Compiling app v0.1.0",
    );
    if target == Target::Building {
        return log.frames;
    }
    log.push(
        LogLevel::Info,
        Some("cargo:stderr"),
        "    Finished `release` profile [optimized + debuginfo] target(s)",
    );
    if target == Target::AwaitingBoard {
        return log.frames;
    }

    // Run phase — on a board, defmt frames decoded from the test binary.
    log.push(
        LogLevel::Info,
        Some("app"),
        "boot: clocks configured, RTT up",
    );
    log.push(LogLevel::Info, Some("app::dma"), "dma: channel 0 armed");
    log.push(
        LogLevel::Debug,
        Some("app::dma"),
        "dma: descriptor ring @ 0x2000_0400",
    );
    log.push(
        LogLevel::Warn,
        Some("app::dma"),
        "dma: fifo high-water mark hit",
    );
    match target {
        Target::Passed => {
            log.push(
                LogLevel::Info,
                Some("app::dma"),
                "dma: drained 4096 bytes, crc ok",
            );
            log.push(LogLevel::Info, Some("app"), "Test OK");
        }
        Target::FailedTest => {
            log.push(
                LogLevel::Error,
                Some("app::dma"),
                "assertion failed: dma fifo",
            );
        }
        Target::FailedInfra => {
            log.push(
                LogLevel::Error,
                Some("probe"),
                "rtt poll failed: probe connection lost",
            );
        }
        Target::TimedOut => {
            log.push(
                LogLevel::Info,
                Some("app::dma"),
                "dma: waiting for transfer-complete irq...",
            );
        }
        Target::Aborted => {
            log.push(
                LogLevel::Info,
                Some("app"),
                "scenario 3/8: back-to-back transfers",
            );
        }
        Target::Running => {
            log.push(
                LogLevel::Info,
                Some("app"),
                "scenario 2/8: single-shot transfer",
            );
        }
        // Building / AwaitingBoard returned above; Submitted returned at top.
        Target::Building | Target::AwaitingBoard | Target::Submitted => {}
    }
    log.frames
}

/// `Submitted → Building → AwaitingBoard → Running`, claiming `board_id` on the
/// run transition (the build phase holds no hardware).
fn build_to_running(db: &Db, id: &JobId, board_id: &str, submitted_at: i64) -> Result<()> {
    let conn = db.raw_conn();
    JobRow::transition_submitted_to_building(conn, id, submitted_at + 10)?;
    JobRow::transition_building_to_awaiting_board(conn, id, "/tmp/x.elf")?;
    JobRow::transition_awaiting_to_running(conn, id, board_id)?;
    Ok(())
}

/// Drive a freshly-inserted (`Submitted`) job to its target state, appending a
/// flavor-appropriate log timeline along the way.
fn drive_job(db: &Db, id: &JobId, target: Target, board_id: &str, submitted_at: i64) -> Result<()> {
    let conn = db.raw_conn();
    let frames = frames_for(target);
    match target {
        Target::Submitted => {
            // Leave as inserted: no transitions, no frames.
        }
        Target::Building => {
            JobRow::transition_submitted_to_building(conn, id, submitted_at + 10)?;
            LogFrame::append_batch(conn, id, &frames)?;
        }
        Target::AwaitingBoard => {
            JobRow::transition_submitted_to_building(conn, id, submitted_at + 10)?;
            JobRow::transition_building_to_awaiting_board(conn, id, "/tmp/x.elf")?;
            LogFrame::append_batch(conn, id, &frames)?;
        }
        Target::Running => {
            build_to_running(db, id, board_id, submitted_at)?;
            LogFrame::append_batch(conn, id, &frames)?;
        }
        Target::Passed
        | Target::FailedTest
        | Target::FailedInfra
        | Target::TimedOut
        | Target::Aborted => {
            build_to_running(db, id, board_id, submitted_at)?;
            LogFrame::append_batch(conn, id, &frames)?;
            let (state, outcome) = terminal_record(target);
            JobRow::finalize(
                conn,
                id,
                &OutcomeRecord {
                    state,
                    outcome,
                    finished_at_ms: submitted_at + 30_000,
                },
            )?;
        }
    }
    Ok(())
}

/// Epoch milliseconds, as the rest of paavo stores timestamps.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the unix epoch")
        .as_millis() as i64
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Ensure the DB's parent dir exists, then open RW (this runs migrations,
    // creating the schema for a fresh path).
    if let Some(parent) = args.db.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db parent dir {}", parent.display()))?;
        }
    }
    let db = Db::open(&args.db).with_context(|| format!("open db {}", args.db.display()))?;
    let conn = db.raw_conn();

    let base_now = now_ms();

    // Register the fake board fleet. Idempotent: an already-registered board id
    // (from a previous run against the same DB) is skipped, not an error.
    let mut board_ids = Vec::new();
    for i in 1..=args.boards {
        let id = format!("mcxa266-{i:02}");
        let spec = BoardSpec {
            id: id.clone(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "1366".into(),
                pid: "1015".into(),
                serial: format!("FAKE{i:02}"),
            },
            chip_name: "MCXA266".into(),
            target_name: "thumbv8m.main-none-eabihf".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        };
        match BoardRow::insert(conn, &spec, base_now) {
            Ok(()) | Err(DbError::AlreadyExists { .. }) => {}
            Err(e) => return Err(e).context("register board"),
        }
        board_ids.push(id);
    }
    anyhow::ensure!(
        !board_ids.is_empty(),
        "need at least one board; pass --boards >= 1"
    );

    // Insert + drive the jobs.
    let submitters = ["alice", "bob", "carol", "dave", "cron"];
    let mut rng = rand::thread_rng();
    let mut counts: HashMap<Target, u32> = HashMap::new();

    // The last `trickle_tail` jobs are dripped in (one per `trickle_ms`) and
    // stamped at "now" so they sort newest-first as a live-insert demo.
    let trickle_tail = if args.trickle_ms > 0 {
        args.jobs.min(20)
    } else {
        0
    };
    let trickle_start = args.jobs - trickle_tail;

    for idx in 0..args.jobs {
        let target = plan_for(idx);
        let submitter = submitters[idx as usize % submitters.len()];
        let board_id = board_ids[idx as usize % board_ids.len()].as_str();
        let priority = if idx % 4 == 0 {
            Priority::Scheduled
        } else {
            Priority::Interactive
        };

        let is_trickle = idx >= trickle_start;
        if is_trickle {
            sleep(Duration::from_millis(args.trickle_ms));
        }
        // Distinct, strictly-increasing submitted_at so newest-first ordering
        // and `as_of` pagination are stable. Bulk jobs are spaced 1 s apart in
        // the past; trickle jobs use live "now" so each lands newest.
        let submitted_at = if is_trickle {
            now_ms()
        } else {
            base_now - (args.jobs as i64 - idx as i64) * 1000
        };

        let id = JobId::new();
        let nj = NewJob {
            id,
            priority,
            submitter: submitter.to_string(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: format!("{:016x}", rng.gen::<u64>()),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        };
        JobRow::insert(conn, &nj, submitted_at).context("insert job")?;
        drive_job(&db, &id, target, board_id, submitted_at).context("drive job")?;

        *counts.entry(target).or_default() += 1;
    }

    print_summary(&args, &board_ids, &counts, trickle_tail);
    Ok(())
}

/// Print a per-state job count, the board list, and the viewer URL.
fn print_summary(
    args: &Args,
    board_ids: &[String],
    counts: &HashMap<Target, u32>,
    trickle_tail: u32,
) {
    println!();
    println!("seed-demo: wrote to {}", args.db.display());
    println!("jobs by state ({} total):", args.jobs);
    for t in ALL_TARGETS {
        let n = counts.get(&t).copied().unwrap_or(0);
        println!("  {:<16}{:>6}", format!("{}:", label(t)), n);
    }
    println!("boards ({}):", board_ids.len());
    for id in board_ids {
        println!("  {id}");
    }
    if trickle_tail > 0 {
        println!(
            "trickled the last {} job(s) at {} ms spacing (live-insert demo)",
            trickle_tail, args.trickle_ms
        );
    }
    println!();
    println!("Open http://127.0.0.1:8081");
}

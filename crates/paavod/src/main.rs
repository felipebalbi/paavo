//! paavod entry point.

use anyhow::{Context, Result};
use clap::Parser;
use paavo_proto::JobOutcome;
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::Config;
use paavod::job_logs::JobLogsBroker;
use paavod::state_dir::StateDir;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let config = Config::load(&args.config)
        .with_context(|| format!("loading config at {}", args.config.display()))?;

    let sd = StateDir::from_root(&config.server.state_dir);
    sd.ensure_dirs()
        .with_context(|| format!("creating state dirs under {}", sd.root.display()))?;

    let db = paavo_db::Db::open(&sd.sqlite_path)
        .with_context(|| format!("opening sqlite at {}", sd.sqlite_path.display()))?;

    // Sync boards.toml into the `board` table if present, then load the
    // inventory snapshot AppState caches. `inventory` MUST be populated
    // BEFORE we start serving HTTP — otherwise selector validation in
    // POST /jobs sees an empty fleet and rejects everything.
    let inventory = load_inventory(&db, &sd.boards_toml)?;

    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(config.clone()),
        inventory: Arc::new(Mutex::new(inventory)),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };

    // Runner selection: production uses RealRunner (see
    // `paavod::real_runner`; M7.4-7.6 wire the probe-rs adapter).
    // Dev / CI can set PAAVO_FAKE_RUNNER=1 to use a FakeRunner that
    // always returns Passed — enables the M4.5.b end-to-end CLI test
    // to drive a real paavod without hardware.
    let runner: Arc<dyn paavo_core::Runner> = if std::env::var("PAAVO_FAKE_RUNNER").is_ok() {
        tracing::warn!("PAAVO_FAKE_RUNNER=1: using FakeRunner; every job returns Passed");
        Arc::new(FakeRunner)
    } else {
        Arc::new(paavod::real_runner::RealRunner::from_state(&state))
    };

    let dispatch_handle = paavod::dispatch::spawn(state.clone(), runner);
    let cron = paavod::cron::start(state.clone()).await?;

    let bind = config.server.bind.clone();
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding to {bind}"))?;
    tracing::info!(bind = %bind, "paavod listening");

    // Graceful-shutdown future: when SIGTERM/Ctrl-C arrives, flip the
    // drain flag synchronously (so any HTTP request that races between
    // signal arrival and axum's drain still sees `state.drain.is_draining()`
    // and returns 503). Axum will then stop accepting new connections
    // and finish in-flight requests.
    let state_for_axum = state.clone();
    let shutdown_future = async move {
        paavod::shutdown::wait_for_signal().await;
        state_for_axum.drain.set_draining();
        tracing::info!("drain: flipped state.drain; axum will finish in-flight requests");
    };
    axum::serve(listener, build_router(state.clone()))
        .with_graceful_shutdown(shutdown_future)
        .await
        .context("axum serve loop returned an error")?;

    // Axum has stopped accepting connections and finished in-flight
    // requests. Now drain dispatch workers + stop the cron scheduler.
    let grace = Duration::from_secs(state.config.timeouts.shutdown_grace_s);
    paavod::shutdown::drain_with_grace(state.clone(), cron, grace).await;

    // Wait briefly for the dispatch loop to notice drain && active==0
    // and return. Detach if it doesn't return within 5s — the runtime
    // dropping will reclaim the task on process exit.
    let _ = tokio::time::timeout(Duration::from_secs(5), dispatch_handle).await;
    tracing::info!("paavod: clean shutdown complete");
    Ok(())
}

/// Sync `boards.toml` (if present) into the `board` table and return
/// the resulting inventory snapshot. `boards.toml` is treated as the
/// declarative source of truth — paavo-cli writes it; paavod only
/// reads. New entries get inserted; existing entries are left alone
/// (operator might have quarantined a board via paavo-cli).
fn load_inventory(
    db: &paavo_db::Db,
    boards_toml: &std::path::Path,
) -> Result<Vec<paavo_proto::BoardSpec>> {
    if boards_toml.is_file() {
        #[derive(serde::Deserialize)]
        struct Boards {
            #[serde(default)]
            board: Vec<paavo_proto::BoardSpec>,
        }
        let raw = std::fs::read_to_string(boards_toml)
            .with_context(|| format!("reading {}", boards_toml.display()))?;
        let parsed: Boards =
            toml::from_str(&raw).with_context(|| format!("parsing {}", boards_toml.display()))?;
        for spec in &parsed.board {
            if paavo_db::BoardRow::find(db.raw_conn(), &spec.id)?.is_none() {
                paavo_db::BoardRow::insert(
                    db.raw_conn(),
                    spec,
                    chrono::Utc::now().timestamp_millis(),
                )
                .with_context(|| format!("inserting board {}", spec.id))?;
            }
        }
    }
    Ok(paavo_db::BoardRow::list_all(db.raw_conn())?
        .into_iter()
        .map(|r| r.spec)
        .collect())
}

/// Dev/CI runner that always returns Passed. Selected via
/// PAAVO_FAKE_RUNNER=1. The M4.5.b end-to-end CLI test exercises
/// paavod through this so it can run without hardware probes.
struct FakeRunner;

impl paavo_core::Runner for FakeRunner {
    fn run(&self, _ctx: paavo_core::RunContext<'_>) -> paavo_core::RunOutcome {
        paavo_core::RunOutcome {
            outcome: JobOutcome::Passed,
            probe_released_cleanly: true,
        }
    }
}

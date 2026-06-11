//! Nightly cron driver: when `scheduler.nightly_cron` fires, walk every
//! `[[corpus]]` entry, tar each subdir, and submit it as a Scheduled job.
//!
//! Drain interaction: when `state.drain.is_draining()`, the corpus run
//! short-circuits (logs + returns). The cron timer keeps firing — but
//! once paavod is draining, no new work goes in. M4.3.d's SIGTERM
//! handler also stops the scheduler.

use crate::app_state::AppState;
use crate::config::CorpusEntry;
use crate::state_dir::StateDir;
use chrono::Utc;
use paavo_core::{enqueue_job, EnqueueRequest};
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use std::path::Path;
use tokio_cron_scheduler::{Job, JobScheduler};

/// Wire up and start the cron job. Returns the scheduler so the caller
/// can shut it down on SIGTERM.
pub async fn start(state: AppState) -> anyhow::Result<JobScheduler> {
    let sched = JobScheduler::new().await?;
    let cron_expr = state.config.scheduler.nightly_cron.clone();
    let state_for_job = state.clone();
    let job = Job::new_async(cron_expr.as_str(), move |_uuid, _l| {
        let state = state_for_job.clone();
        Box::pin(async move {
            if let Err(e) = run_nightly_corpus(&state).await {
                tracing::error!(error = ?e, "nightly cron run failed");
            }
        })
    })?;
    sched.add(job).await?;
    sched.start().await?;
    Ok(sched)
}

async fn run_nightly_corpus(state: &AppState) -> anyhow::Result<()> {
    if state.drain.is_draining() {
        tracing::info!("nightly cron: draining — skipping corpus run");
        return Ok(());
    }
    let now_ms = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        paavo_db::ScheduleRow::upsert(
            db.raw_conn(),
            &paavo_db::ScheduleRow {
                id: "nightly".into(),
                cron: state.config.scheduler.nightly_cron.clone(),
                enabled: true,
                last_triggered_at: Some(now_ms),
                last_completed_at: None,
            },
        )?;
    }

    let corpus = state.config.corpus.clone();
    for entry in &corpus {
        if let Err(e) = enqueue_corpus_entry(state, entry).await {
            tracing::error!(corpus = %entry.name, error = ?e, "corpus enqueue failed");
        }
    }

    {
        let db = state.db.lock();
        paavo_db::ScheduleRow::apply_update(
            db.raw_conn(),
            "nightly",
            &paavo_db::ScheduleUpdate {
                last_triggered_at: None,
                last_completed_at: Some(Utc::now().timestamp_millis()),
            },
        )?;
    }
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
        let tar_bytes = make_tar(&crate_dir)?;
        let sd = StateDir::from_root(&state.config.server.state_dir);
        sd.ensure_dirs()?;
        let blake = paavo_build::tar::blake3_hex(&tar_bytes);
        let tar_path = sd.uploads_dir.join(format!("{blake}.tar"));
        if !tar_path.is_file() {
            std::fs::write(&tar_path, &tar_bytes)?;
        }
        // Infer board kind from the corpus path: convention is
        // `<corpus_root>/<kind>/<test-crate>/`. The entry's `path` is
        // `<corpus_root>/<kind>`, so the basename gives us `<kind>`.
        let kind = entry
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("cannot infer board kind from {:?}", entry.path))?;
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
        let now_ms = Utc::now().timestamp_millis();
        let result = {
            let db = state.db.lock();
            enqueue_job(db.raw_conn(), &inventory, req, now_ms)
        };
        if let Err(e) = result {
            tracing::error!(
                corpus = %entry.name,
                crate_dir = %crate_dir.display(),
                error = ?e,
                "cron: enqueue_job rejected corpus job",
            );
        }
    }
    Ok(())
}

fn make_tar(crate_dir: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut tarb = tar::Builder::new(&mut buf);
        tarb.append_dir_all(crate_dir.file_name().unwrap_or_default(), crate_dir)?;
        tarb.finish()?;
    }
    Ok(buf)
}

/// Test hook: run the corpus enqueue logic exactly once without
/// scheduling.
#[doc(hidden)]
pub async fn __test_run_once(state: &AppState) -> anyhow::Result<()> {
    run_nightly_corpus(state).await
}

//! Nightly cron driver: when `scheduler.nightly_cron` fires, walk every
//! `[[corpus]]` entry, tar each subdir, and submit it as a Scheduled job.
//!
//! Drain interaction: when `state.drain.is_draining()`, the corpus run
//! short-circuits BEFORE updating the schedule row, so paavo-web's
//! /schedule page can distinguish "fired but skipped" from "completed".
//! The cron timer keeps firing during drain — `start` returns a
//! `CronHandle` whose `shutdown` method paavod::main calls on SIGTERM
//! (M4.3.d).
//!
//! Persistence model: each corpus crate is streamed into a temp tar
//! under `${state_dir}/uploads/.tmp-<jobid>.tar` (blake3-hashed in
//! flight), then atomically renamed to `<blake>.tar`. This mirrors the
//! HTTP POST /jobs idiom so the on-disk layout is uniform and the cron
//! path can't OOM the daemon on a multi-GB vendored-deps tarball.

use crate::app_state::AppState;
use crate::config::CorpusEntry;
use crate::state_dir::StateDir;
use chrono::Utc;
use paavo_core::{enqueue_job, EnqueueRequest};
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use std::path::Path;
use tokio_cron_scheduler::{Job, JobScheduler};

/// Handle to the running cron scheduler. The wrapper exists so callers
/// (paavod::main, M4.3.d's SIGTERM handler) can't accidentally drop
/// the scheduler — `tokio-cron-scheduler` keeps a background tokio task
/// alive that does NOT stop on drop, so a forgotten handle would leak
/// the timer past drain.
#[must_use = "CronHandle MUST be shut down via .shutdown().await — dropping leaks the scheduler task"]
pub struct CronHandle {
    inner: JobScheduler,
}

impl CronHandle {
    /// Stop the scheduler. Safe to call from any async context.
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.inner.shutdown().await?;
        Ok(())
    }
}

/// Wire up and start the cron job. Returns a `CronHandle` so the caller
/// must explicitly call `.shutdown().await` (otherwise the cron task
/// outlives the daemon's drain).
pub async fn start(state: AppState) -> anyhow::Result<CronHandle> {
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
    Ok(CronHandle { inner: sched })
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
    let mut enqueued_total: usize = 0;
    for entry in &corpus {
        match enqueue_corpus_entry(state, entry).await {
            Ok(n) => enqueued_total += n,
            Err(e) => {
                tracing::error!(corpus = %entry.name, error = ?e, "corpus enqueue failed");
            }
        }
    }

    // Only stamp last_completed_at when at least one job actually went
    // in. A totally-failed nightly leaves the column unchanged so the
    // web UI can show "last completed: <stale>" instead of lying.
    if enqueued_total > 0 {
        let db = state.db.lock();
        paavo_db::ScheduleRow::apply_update(
            db.raw_conn(),
            "nightly",
            &paavo_db::ScheduleUpdate {
                last_triggered_at: None,
                last_completed_at: Some(Utc::now().timestamp_millis()),
            },
        )?;
    } else {
        tracing::warn!("nightly cron: 0 jobs enqueued across all corpus entries");
    }
    Ok(())
}

/// Walk one `[[corpus]]` entry: list its first-level subdirs with a
/// `Cargo.toml`, tar + persist + enqueue each. Per-crate failures are
/// logged + the loop continues so one bad crate doesn't lose the
/// whole entry. Returns the count of successfully-enqueued jobs.
async fn enqueue_corpus_entry(state: &AppState, entry: &CorpusEntry) -> anyhow::Result<usize> {
    let mut enqueued: usize = 0;
    for sub in std::fs::read_dir(&entry.path)? {
        let sub = match sub {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    corpus = %entry.name,
                    error = ?e,
                    "cron: read_dir entry failed; skipping",
                );
                continue;
            }
        };
        let is_dir = match sub.file_type() {
            Ok(t) => t.is_dir(),
            Err(e) => {
                tracing::error!(
                    corpus = %entry.name,
                    error = ?e,
                    "cron: file_type failed; skipping",
                );
                continue;
            }
        };
        if !is_dir {
            continue;
        }
        let crate_dir = sub.path();
        if !crate_dir.join("Cargo.toml").is_file() {
            continue;
        }
        match enqueue_one_crate(state, entry, &crate_dir).await {
            Ok(()) => enqueued += 1,
            Err(e) => {
                tracing::error!(
                    corpus = %entry.name,
                    crate_dir = %crate_dir.display(),
                    error = ?e,
                    "cron: failed to enqueue crate",
                );
            }
        }
    }
    Ok(enqueued)
}

/// Tar + persist + enqueue one crate. The tar build runs inside
/// `spawn_blocking` because `tar::Builder::append_dir_all` walks the
/// whole tree synchronously and can take >100ms on a vendored-deps
/// corpus crate. The persist+enqueue happens on the runtime worker
/// (sub-ms SQLite + atomic rename).
async fn enqueue_one_crate(
    state: &AppState,
    entry: &CorpusEntry,
    crate_dir: &Path,
) -> anyhow::Result<()> {
    let job_id = JobId::new();
    let sd = StateDir::from_root(&state.config.server.state_dir);
    sd.ensure_dirs()?;
    let tmp_path = sd.uploads_dir.join(format!(".tmp-{job_id}.tar"));

    // Stream the tar to a temp file + blake3 in flight, mirroring the
    // HTTP POST /jobs idiom. The walk is `spawn_blocking` so the
    // runtime worker isn't stalled on a multi-GB tar build.
    let crate_dir_owned = crate_dir.to_path_buf();
    let tmp_path_owned = tmp_path.clone();
    let blake = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        stream_tar_to_file(&crate_dir_owned, &tmp_path_owned)
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking joined: {e}"))??;

    let final_path = sd.uploads_dir.join(format!("{blake}.tar"));
    // Atomic rename. On dedup hit (final_path exists) the temp file is
    // unlinked; both POSIX and Windows rename silently clobber so the
    // happy path just renames.
    if final_path.is_file() {
        let _ = tokio::fs::remove_file(&tmp_path).await;
    } else {
        tokio::fs::rename(&tmp_path, &final_path)
            .await
            .map_err(|e| anyhow::anyhow!("rename tmp tar: {e}"))?;
    }

    let crate_name = crate_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<unnamed>")
        .to_string();
    let req = EnqueueRequest {
        job_id,
        priority: Priority::Scheduled,
        submitter: format!("nightly:{}:{}", entry.name, crate_name),
        source: JobSource::Scheduler,
        // kind comes from the explicit [[corpus]].kind field, not from
        // the corpus path basename (spec §13).
        board_selector: BoardSelector {
            kind: entry.kind.clone(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: state.config.timeouts.default_inactivity_s * 1_000,
        hard_max_ms: state.config.timeouts.default_scheduled_hard_max_s * 1_000,
        tar_blake3: blake,
        tar_path: final_path.display().to_string(),
        daemon_ceiling_ms: state.config.timeouts.daemon_ceiling_s * 1_000,
        // Thread the corpus's cargo_update list through to the build
        // sandbox. paavo-build runs `cargo update -p <pkg>` for each
        // before `cargo build`, giving the nightly fresh embassy
        // revisions on every run (spec §8.1 step 4).
        cargo_update_packages: entry.cargo_update.clone(),
        // The nightly cron always benefits from cache hits — that's
        // what makes the soak loop tractable across hundreds of crates
        // and dozens of boards. No `--skip-cache` plumbed here today.
        skip_cache: false,
    };
    let inventory = state.inventory_snapshot();
    let now_ms = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        enqueue_job(db.raw_conn(), &inventory, req, now_ms)?;
    }
    Ok(())
}

/// Stream the contents of `crate_dir` into a tar file at `dst`, hashing
/// every byte with blake3 in flight. Returns the hex-encoded blake3
/// digest. Synchronous; meant to be called inside `spawn_blocking`.
fn stream_tar_to_file(crate_dir: &Path, dst: &Path) -> anyhow::Result<String> {
    use std::io::Write;
    let file = std::fs::File::create(dst)?;
    let mut writer = HashingWriter::new(file);
    {
        let mut tarb = tar::Builder::new(&mut writer);
        tarb.append_dir_all(crate_dir.file_name().unwrap_or_default(), crate_dir)?;
        tarb.finish()?;
    }
    writer.flush()?;
    Ok(writer.finalize_hex())
}

/// Wraps a `Write` and feeds every byte into a `blake3::Hasher`. Used
/// by `stream_tar_to_file` so we get the digest without buffering the
/// whole archive.
struct HashingWriter<W: std::io::Write> {
    inner: W,
    hasher: blake3::Hasher,
}

impl<W: std::io::Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }
    fn finalize_hex(self) -> String {
        self.hasher.finalize().to_hex().to_string()
    }
}

impl<W: std::io::Write> std::io::Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Test hook: run the corpus enqueue logic exactly once without
/// scheduling.
#[doc(hidden)]
pub async fn __test_run_once(state: &AppState) -> anyhow::Result<()> {
    run_nightly_corpus(state).await
}

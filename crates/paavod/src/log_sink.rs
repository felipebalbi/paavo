//! `FrameSink`: the shared persistence core for both log forwarders.
//!
//! The build forwarder (dispatch) and the run forwarder (real_runner) are
//! sequential phases of one job. Each constructs a `FrameSink` from the
//! shared `Arc<AtomicU64>` seq counter + the shared job-start `Instant`,
//! then feeds it frames. The sink assigns the authoritative seq, stamps
//! `ts_us` against the shared clock, publishes to the live broker, and
//! batch-persists to `log_frame`. Because the phases are strictly
//! sequential (the build forwarder is joined before the run forwarder
//! spawns), the shared counter is never accessed concurrently and the
//! `(job_id, seq)` rows are contiguous across the build->run boundary.
//!
//! See
//! `docs/superpowers/specs/2026-06-16-c2-log-frame-persistence-design.md`.

use crate::job_logs::{JobLogsBroker, LiveEvent};
use paavo_db::{Db, LogFrameDb};
use paavo_proto::{JobId, LogFrame, LogLevel};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Flush after this many buffered frames.
const BATCH_MAX: usize = 64;
/// Flush at least this often while frames are arriving.
const FLUSH_INTERVAL: Duration = Duration::from_millis(50);

/// Per-forwarder log sink: assign seq, stamp ts_us, publish,
/// batch-persist.
pub struct FrameSink {
    job_id: JobId,
    broker: JobLogsBroker,
    db: Arc<Mutex<Db>>,
    seq: Arc<AtomicU64>,
    job_start: Instant,
    batch: Vec<LogFrame>,
    last_flush: Instant,
}

impl FrameSink {
    /// Construct a sink. `seq` and `job_start` are shared across the
    /// build and run forwarders so frames stay contiguous + monotonic.
    pub fn new(
        job_id: JobId,
        broker: JobLogsBroker,
        db: Arc<Mutex<Db>>,
        seq: Arc<AtomicU64>,
        job_start: Instant,
    ) -> Self {
        Self {
            job_id,
            broker,
            db,
            seq,
            job_start,
            batch: Vec::with_capacity(BATCH_MAX),
            last_flush: Instant::now(),
        }
    }

    /// Ingest one frame: assign seq + ts_us, publish live, buffer, and
    /// flush if the batch is full or the flush interval elapsed.
    pub fn push(&mut self, level: LogLevel, target: Option<String>, message: String) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let ts_us = u64::try_from(self.job_start.elapsed().as_micros()).unwrap_or(u64::MAX);
        let frame = LogFrame {
            seq,
            ts_us,
            level,
            target,
            message,
        };
        self.broker
            .publish(self.job_id, LiveEvent::Frame(frame.clone()));
        self.batch.push(frame);
        if self.batch.len() >= BATCH_MAX || self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush();
        }
    }

    /// Called by a forwarder on its `recv_timeout` timeout: flush a
    /// non-empty batch if the interval elapsed, so frames don't sit
    /// unpersisted while the source is quiet.
    pub fn tick(&mut self) {
        if !self.batch.is_empty() && self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush();
        }
    }

    /// Final flush; call once when the source channel closes.
    pub fn finish(mut self) {
        if !self.batch.is_empty() {
            self.flush();
        }
    }

    fn flush(&mut self) {
        {
            let conn = self.db.lock();
            if let Err(e) = LogFrame::append_batch(conn.raw_conn(), &self.job_id, &self.batch) {
                // A DB write failure leaves a gap in the historical view
                // but MUST NOT abort the build or run; the live broker
                // already delivered every frame.
                tracing::error!(
                    error = %e,
                    job_id = %self.job_id,
                    "log forwarder: append_batch failed; frames lost from history"
                );
            }
        }
        self.batch.clear();
        self.last_flush = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paavo_proto::{BoardSelector, JobSource, LogFrame as ProtoFrame, Priority};
    use tempfile::TempDir;

    fn temp_db() -> (TempDir, Arc<Mutex<Db>>) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("t.sqlite")).unwrap();
        (dir, Arc::new(Mutex::new(db)))
    }

    /// log_frame.job_id is `REFERENCES job(id)` with foreign_keys = ON,
    /// so a job row must exist before append_batch.
    fn seed_job(db: &Arc<Mutex<Db>>) -> JobId {
        let id = JobId::new();
        let conn = db.lock();
        paavo_db::JobRow::insert(
            conn.raw_conn(),
            &paavo_db::NewJob {
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
                cargo_update_packages: vec![],
                skip_cache: false,
            },
            0,
        )
        .unwrap();
        id
    }

    fn rows(db: &Arc<Mutex<Db>>, id: &JobId) -> Vec<ProtoFrame> {
        let conn = db.lock();
        ProtoFrame::list(conn.raw_conn(), id, 0, 10_000).unwrap()
    }

    #[test]
    fn push_assigns_monotonic_seq_and_persists() {
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let seq = Arc::new(AtomicU64::new(0));
        let mut sink = FrameSink::new(id, JobLogsBroker::new(), db.clone(), seq, Instant::now());
        sink.push(LogLevel::Info, Some("cargo:stdout".into()), "a".into());
        sink.push(LogLevel::Warn, None, "b".into());
        sink.push(LogLevel::Error, None, "c".into());
        sink.finish();

        let got = rows(&db, &id);
        assert_eq!(got.len(), 3, "all three frames persisted");
        assert_eq!(got[0].seq, 0);
        assert_eq!(got[1].seq, 1);
        assert_eq!(got[2].seq, 2);
        assert_eq!(got[0].target.as_deref(), Some("cargo:stdout"));
        assert_eq!(got[0].message, "a");
        assert_eq!(got[1].level, LogLevel::Warn);
        assert_eq!(got[2].message, "c");
    }

    #[test]
    fn build_then_run_share_seq_and_clock() {
        // The monotonic-seq test: two sinks over ONE shared counter +
        // clock (the build phase then the run phase). Seqs must be
        // contiguous 0..M+N-1, the first M carry cargo:* targets, and
        // ts_us must be non-decreasing across the boundary.
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let seq = Arc::new(AtomicU64::new(0));
        let job_start = Instant::now();

        let mut build =
            FrameSink::new(id, JobLogsBroker::new(), db.clone(), seq.clone(), job_start);
        build.push(
            LogLevel::Info,
            Some("cargo:stdout".into()),
            "compiling".into(),
        );
        build.push(
            LogLevel::Info,
            Some("cargo:stderr".into()),
            "warning: x".into(),
        );
        build.finish();

        let mut run = FrameSink::new(id, JobLogsBroker::new(), db.clone(), seq.clone(), job_start);
        run.push(LogLevel::Info, None, "hello".into());
        run.push(LogLevel::Info, None, "Test OK".into());
        run.finish();

        let got = rows(&db, &id);
        let seqs: Vec<u64> = got.iter().map(|f| f.seq).collect();
        assert_eq!(
            seqs,
            vec![0, 1, 2, 3],
            "contiguous seqs across the boundary"
        );
        assert!(got[0].target.as_deref().unwrap().starts_with("cargo:"));
        assert!(got[1].target.as_deref().unwrap().starts_with("cargo:"));
        assert_eq!(got[2].target, None);
        assert_eq!(got[3].target, None);
        for w in got.windows(2) {
            assert!(w[1].ts_us >= w[0].ts_us, "ts_us non-decreasing");
        }
    }

    #[test]
    fn push_publishes_to_broker() {
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let broker = JobLogsBroker::new();
        let mut rx = broker.subscribe(id);
        let mut sink = FrameSink::new(
            id,
            broker,
            db.clone(),
            Arc::new(AtomicU64::new(0)),
            Instant::now(),
        );
        sink.push(LogLevel::Info, None, "live".into());

        match rx.try_recv() {
            Ok(LiveEvent::Frame(f)) => {
                assert_eq!(f.seq, 0);
                assert_eq!(f.message, "live");
            }
            other => panic!("expected a Frame on the broker, got {other:?}"),
        }
        sink.finish();
    }

    #[test]
    fn final_flush_persists_partial_batch() {
        // Fewer than BATCH_MAX frames; only finish() forces them out.
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let mut sink = FrameSink::new(
            id,
            JobLogsBroker::new(),
            db.clone(),
            Arc::new(AtomicU64::new(0)),
            Instant::now(),
        );
        sink.push(LogLevel::Info, None, "1".into());
        sink.push(LogLevel::Info, None, "2".into());
        sink.finish();
        assert_eq!(rows(&db, &id).len(), 2);
    }
}

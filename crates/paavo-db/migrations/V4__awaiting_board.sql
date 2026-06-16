-- Extend job.state CHECK to allow 'awaiting_board' (the new build-pool
-- "built, waiting for a board" state).
--
-- SQLite cannot ALTER a CHECK constraint, so the job table is rebuilt.
-- refinery runs this inside a transaction with foreign_keys=ON (set in
-- Db::open BEFORE migrations), so `PRAGMA foreign_keys=OFF` is a no-op
-- here and a plain `DROP TABLE job` would fire log_frame's ON DELETE
-- CASCADE and erase all logs. Instead we rebuild BOTH tables in
-- dependency order: build the new log_frame pointing at the new job
-- table first, so each DROP has no child referencing it.
--
-- The live job table is V1 + V2 (cargo_update_packages) + V3
-- (skip_cache); job_new mirrors all 18 columns in order so
-- `INSERT ... SELECT *` lines up.

-- 1. New job table with the extended state CHECK.
CREATE TABLE job_new (
    id                       TEXT PRIMARY KEY,
    priority                 INTEGER NOT NULL,
    submitter                TEXT NOT NULL,
    source                   TEXT NOT NULL CHECK (source IN ('cli','scheduler')),
    board_selector           TEXT NOT NULL,
    inactivity_timeout_ms    INTEGER NOT NULL,
    hard_max_ms              INTEGER NOT NULL,
    state                    TEXT NOT NULL CHECK (state IN
        ('submitted','building','awaiting_board','running','passed','failed','timedout','aborted')),
    outcome_detail           TEXT,
    board_id                 TEXT REFERENCES board(id),
    submitted_at             INTEGER NOT NULL,
    started_at               INTEGER,
    finished_at              INTEGER,
    tar_blake3               TEXT NOT NULL,
    tar_path                 TEXT NOT NULL,
    elf_path                 TEXT,
    cargo_update_packages    TEXT NOT NULL DEFAULT '[]',
    skip_cache               INTEGER NOT NULL DEFAULT 0
);
INSERT INTO job_new SELECT * FROM job;

-- 2. New log_frame whose FK references job_new (so the old job has no
--    child at drop time).
CREATE TABLE log_frame_new (
    job_id   TEXT NOT NULL REFERENCES job_new(id) ON DELETE CASCADE,
    seq      INTEGER NOT NULL,
    ts_us    INTEGER NOT NULL,
    level    TEXT NOT NULL CHECK (level IN ('trace','debug','info','warn','error')),
    target   TEXT,
    message  TEXT NOT NULL,
    PRIMARY KEY (job_id, seq)
);
INSERT INTO log_frame_new SELECT * FROM log_frame;

-- 3. Drop old child then old parent. Neither has a child referencing it.
DROP TABLE log_frame;
DROP TABLE job;

-- 4. Rename into place. Renaming job_new -> job rewrites log_frame_new's
--    FK to reference `job` (legacy_alter_table is OFF by default).
ALTER TABLE job_new RENAME TO job;
ALTER TABLE log_frame_new RENAME TO log_frame;

-- 5. Recreate indexes (they did not survive the rebuild).
CREATE INDEX idx_job_state           ON job(state);
CREATE INDEX idx_job_submitted_at    ON job(submitted_at);
CREATE INDEX idx_job_priority_subat  ON job(priority, submitted_at) WHERE state = 'submitted';
CREATE INDEX idx_log_frame_job_level ON log_frame(job_id, level);

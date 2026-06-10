-- See spec §7 for column rationale.

-- Foreign-key enforcement is set on the connection in `Db::open`
-- (db.rs::configure). `PRAGMA foreign_keys` is a documented no-op inside a
-- transaction, and refinery wraps each migration in its own tx — so this
-- file cannot enable FK enforcement for itself even if it tried.

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

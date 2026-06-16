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
    /// Per-request multipart body cap for `POST /jobs` (bytes).
    /// Default 256 MiB. Raise for fleets with large vendored deps.
    #[serde(default = "default_max_upload_bytes")]
    pub max_upload_bytes: usize,
}

fn default_max_upload_bytes() -> usize {
    256 * 1024 * 1024
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

fn default_inactivity_s() -> u64 {
    120
}
fn default_ad_hoc_hard_max_s() -> u64 {
    900
}
fn default_scheduled_hard_max_s() -> u64 {
    14_400
}
fn default_daemon_ceiling_s() -> u64 {
    28_800
}
fn default_shutdown_grace_s() -> u64 {
    60
}

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
    /// Max concurrent `cargo build` processes (each gets its own
    /// CARGO_TARGET_DIR). Jobs beyond this wait in `Submitted`.
    #[serde(default = "default_max_concurrent_builds")]
    pub max_concurrent_builds: usize,
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

fn default_starvation_threshold_s() -> i64 {
    21_600
}

fn default_max_concurrent_builds() -> usize {
    5
}

/// `[build_cache]`.
#[derive(Debug, Clone, Deserialize)]
pub struct BuildCacheConfig {
    /// LRU cap in bytes.
    #[serde(default = "default_build_cache_max_bytes")]
    pub max_bytes: u64,
}

impl Default for BuildCacheConfig {
    fn default() -> Self {
        Self {
            max_bytes: default_build_cache_max_bytes(),
        }
    }
}

fn default_build_cache_max_bytes() -> u64 {
    5 * 1024 * 1024 * 1024
}

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

fn default_passed_full_log_days() -> i32 {
    30
}

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

fn default_consecutive_infra_failures() -> u32 {
    3
}

/// One `[[corpus]]` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct CorpusEntry {
    /// Human-readable name (e.g. `embassy-mcxa-regression`).
    pub name: String,
    /// Board kind every crate under this corpus targets. Must match a
    /// `board.kind` registered via `paavo-cli board add`. The cron
    /// driver uses this directly when building the selector for every
    /// Scheduled job; the corpus PATH basename is not parsed (spec
    /// §13 + §7.5).
    pub kind: String,
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

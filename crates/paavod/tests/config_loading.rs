use paavod::config::Config;
use std::fs;
use tempfile::tempdir;

// Cron is 6-field (`sec min hour dom mon dow`) — the `cron` crate's
// native form, also what `tokio-cron-scheduler` parses. Time zone is
// the daemon's local TZ. `"0 0 19 * * *"` = "every day at 19:00:00".
const SAMPLE: &str = r#"
[server]
bind = "127.0.0.1:8080"
state_dir = "/var/lib/paavo"

[web]
bind = "127.0.0.1:8081"

[timeouts]
default_inactivity_s = 120
default_ad_hoc_hard_max_s = 900
default_scheduled_hard_max_s = 14400
daemon_ceiling_s = 28800
shutdown_grace_s = 60

[scheduler]
starvation_threshold_s = 21600
nightly_cron = "0 0 19 * * *"

[build_cache]
max_bytes = 5368709120

[retention]
passed_full_log_days = 30

[quarantine]
consecutive_infra_failures = 3

[[corpus]]
name = "embassy-mcxa-regression"
kind = "mcxa266"
path = "/var/lib/paavo/checkouts/embassy/tests/mcxa266"
cargo_update = ["embassy-mcxa", "embassy-executor"]

[[corpus]]
name = "paavo-soak-mcxa266"
kind = "mcxa266"
path = "/var/lib/paavo/checkouts/paavo/soak-tests/mcxa266"
cargo_update = []
"#;

#[test]
fn parses_sample_config() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(&p, SAMPLE).unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.server.bind, "127.0.0.1:8080");
    assert_eq!(cfg.web.bind, "127.0.0.1:8081");
    assert_eq!(cfg.timeouts.default_inactivity_s, 120);
    assert_eq!(cfg.timeouts.daemon_ceiling_s, 28800);
    assert_eq!(cfg.scheduler.nightly_cron, "0 0 19 * * *");
    assert_eq!(cfg.build_cache.max_bytes, 5_368_709_120);
    assert_eq!(cfg.retention.passed_full_log_days, 30);
    assert_eq!(cfg.quarantine.consecutive_infra_failures, 3);
    assert_eq!(cfg.corpus.len(), 2);
    assert_eq!(cfg.corpus[0].name, "embassy-mcxa-regression");
    assert_eq!(cfg.corpus[0].kind, "mcxa266");
    assert_eq!(cfg.corpus[1].kind, "mcxa266");
    assert_eq!(
        cfg.corpus[0].cargo_update,
        vec!["embassy-mcxa".to_string(), "embassy-executor".into()]
    );
}

#[test]
fn rejects_invalid_cron() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(
        &p,
        SAMPLE.replace("0 0 19 * * *", "not a valid cron expression"),
    )
    .unwrap();
    let err = Config::load(&p).unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("cron"));
}

#[test]
fn rejects_five_field_cron() {
    // 5-field POSIX cron is a common ops-user mistake. Reject it
    // explicitly with a message that mentions both "cron" and the
    // 6-field expectation, so the operator can fix it without
    // having to read the `cron` crate's source.
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(&p, SAMPLE.replace("0 0 19 * * *", "0 19 * * *")).unwrap();
    let err = Config::load(&p).unwrap_err();
    let msg = format!("{err}").to_lowercase();
    assert!(msg.contains("cron"), "error should mention cron: {err}");
}

#[test]
fn defaults_used_when_optional_blocks_omitted() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(
        &p,
        r#"
[server]
bind = "127.0.0.1:8080"
state_dir = "/var/lib/paavo"
[web]
bind = "127.0.0.1:8081"
[scheduler]
nightly_cron = "0 0 19 * * *"
"#,
    )
    .unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.timeouts.default_inactivity_s, 120);
    assert_eq!(cfg.retention.passed_full_log_days, 30);
    assert_eq!(cfg.quarantine.consecutive_infra_failures, 3);
    assert!(cfg.corpus.is_empty());
}

#[test]
fn missing_file_error_mentions_path() {
    // Pins the `reading <path>` context wrapped around the io::Error.
    let dir = tempdir().unwrap();
    let p = dir.path().join("does-not-exist.toml");
    let err = Config::load(&p).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("does-not-exist.toml"),
        "error should mention the missing path: {msg}"
    );
}

#[test]
fn malformed_toml_error_mentions_paavo_toml() {
    // Pins the `parsing paavo.toml` context wrapped around the toml::Error.
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    fs::write(&p, "[server\nbind = oops").unwrap();
    let err = Config::load(&p).unwrap_err();
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("paavo.toml") || msg.contains("parsing"),
        "error should mention the file or `parsing`: {msg}"
    );
}

#[test]
fn server_max_upload_bytes_defaults_to_256_mib() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    // SAMPLE omits `max_upload_bytes` — default must apply.
    fs::write(&p, SAMPLE).unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.server.max_upload_bytes, 256 * 1024 * 1024);
}

#[test]
fn server_max_upload_bytes_can_be_overridden() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("paavo.toml");
    let toml = SAMPLE.replace("[web]", "max_upload_bytes = 1048576\n\n[web]");
    fs::write(&p, toml).unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.server.max_upload_bytes, 1_048_576);
}

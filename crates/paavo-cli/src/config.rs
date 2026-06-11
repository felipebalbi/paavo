//! Host resolution: --host > PAAVO_HOST > ~/.config/paavo/cli.toml > default.

use anyhow::{Context, Result};

/// Resolve the daemon host string.
pub fn resolve_host(flag: Option<&str>) -> Result<String> {
    if let Some(h) = flag {
        return Ok(h.to_string());
    }
    if let Ok(h) = std::env::var("PAAVO_HOST") {
        return Ok(h);
    }
    let p = dirs_helper().join("paavo").join("cli.toml");
    if p.is_file() {
        #[derive(serde::Deserialize)]
        struct CliCfg {
            host: String,
        }
        let raw =
            std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        let cfg: CliCfg =
            toml::from_str(&raw).with_context(|| format!("parsing {}", p.display()))?;
        return Ok(cfg.host);
    }
    Ok("http://127.0.0.1:8080".into())
}

fn dirs_helper() -> std::path::PathBuf {
    if let Ok(c) = std::env::var("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(c);
    }
    if let Ok(h) = std::env::var("HOME") {
        return std::path::PathBuf::from(h).join(".config");
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return std::path::PathBuf::from(h).join(".config");
    }
    std::path::PathBuf::from(".")
}

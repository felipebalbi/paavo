//! `paavo-cli run`: tar a crate dir / .rs / .elf and submit. Streams output.

use crate::cli::PriorityArg;
use crate::client::Client;
use anyhow::{Context, Result};
use paavo_proto::{BoardSelector, JobSpec, Priority};
use std::path::Path;

/// Entry point for `paavo-cli run`.
pub async fn run(
    client: &Client,
    path: &Path,
    board_kind: Option<&str>,
    instance: Option<&str>,
    timeout: Option<&str>,
    inactivity: Option<&str>,
    priority: PriorityArg,
) -> Result<()> {
    let kind = board_kind.ok_or_else(|| anyhow::anyhow!("--board-kind is required for `run`"))?;
    let crate_dir = resolve_crate_dir(path)?;
    let tar_bytes = make_tar(&crate_dir).context("tarring crate dir")?;

    // JobSpec is the wire shape paavod's PostJobMetadata deserializes.
    // No `source` (server forces Cli per spec §9.1), no `tar_blake3`
    // (paavod computes it during streaming).
    let spec = JobSpec {
        priority: match priority {
            PriorityArg::Interactive => Priority::Interactive,
            PriorityArg::Scheduled => Priority::Scheduled,
        },
        submitter: whoami().unwrap_or_else(|| "anon".into()),
        board_selector: BoardSelector {
            kind: kind.into(),
            instance: instance.map(String::from),
            wiring_profile: None,
        },
        inactivity_timeout_ms: inactivity.map(parse_duration_ms).transpose()?,
        hard_max_ms: timeout.map(parse_duration_ms).transpose()?,
    };

    let job_id = client.submit_job(&spec, tar_bytes).await?;
    println!("submitted: {job_id}");
    stream_logs(client, &job_id).await
}

fn resolve_crate_dir(path: &Path) -> Result<std::path::PathBuf> {
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    if path.is_file() {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        match ext {
            "rs" => {
                let mut cur = path.parent().unwrap_or(path);
                loop {
                    if cur.join("Cargo.toml").is_file() {
                        return Ok(cur.to_path_buf());
                    }
                    match cur.parent() {
                        Some(p) => cur = p,
                        None => anyhow::bail!(
                            ".rs file has no parent Cargo.toml; run `paavo-cli new` first"
                        ),
                    }
                }
            }
            "elf" => {
                anyhow::bail!(
                    "pre-built .elf submission is wired in v1.1; \
                     for now pass a crate dir or .rs file"
                );
            }
            other => anyhow::bail!("unsupported file extension: .{other}"),
        }
    }
    anyhow::bail!("not a file or dir: {path:?}")
}

fn make_tar(dir: &Path) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut t = tar::Builder::new(&mut buf);
        t.append_dir_all(dir.file_name().unwrap_or_default(), dir)?;
        t.finish()?;
    }
    Ok(buf)
}

fn whoami() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
}

fn parse_duration_ms(s: &str) -> Result<u64> {
    // Supports "120s", "30m", "1h", or a bare number (ms).
    let s = s.trim();
    if let Some(num) = s.strip_suffix('h') {
        return Ok(num.trim().parse::<u64>()? * 3_600_000);
    }
    if let Some(num) = s.strip_suffix('m') {
        return Ok(num.trim().parse::<u64>()? * 60_000);
    }
    if let Some(num) = s.strip_suffix('s') {
        return Ok(num.trim().parse::<u64>()? * 1_000);
    }
    Ok(s.parse::<u64>()?)
}

async fn stream_logs(client: &Client, job_id: &str) -> Result<()> {
    let mut resp = client.stream(job_id).await?;
    let mut buf = String::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find('\n') {
            let line = buf[..idx].trim().to_string();
            buf.drain(..=idx);
            if !line.is_empty() {
                handle_ndjson_line(&line);
            }
        }
    }
    Ok(())
}

/// Parse one NDJSON line from `/jobs/:id/stream` (spec §9.2). Frames
/// print as `<message>`; the terminal line prints a summary and exits
/// with 0 for Passed, 1 otherwise. `lagged` and `truncated` markers
/// print to stderr so they don't pollute the test-output capture.
fn handle_ndjson_line(line: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        eprintln!("paavo-cli: skipping malformed stream line: {line}");
        return;
    };
    match v["type"].as_str() {
        Some("frame") => {
            let msg = v["frame"]["message"].as_str().unwrap_or("");
            println!("{msg}");
        }
        Some("terminal") => {
            let outcome = &v["outcome"];
            // `outcome` is either the string "passed" or a single-key
            // object like {"failed": {...}} / {"timed_out": {...}} /
            // {"aborted": {...}}.
            let tag = outcome
                .as_str()
                .map(str::to_string)
                .or_else(|| outcome.as_object().and_then(|m| m.keys().next().cloned()))
                .unwrap_or_default();
            println!("--- terminal: {outcome}");
            std::process::exit(if tag == "passed" { 0 } else { 1 });
        }
        Some("lagged") => {
            eprintln!(
                "paavo-cli: log stream lagged ({} frames missed); refetch /jobs/:id for the full log",
                v["missed"].as_u64().unwrap_or(0),
            );
        }
        Some("truncated") => {
            eprintln!(
                "paavo-cli: log stream truncated: {}",
                v["reason"].as_str().unwrap_or("<no reason>"),
            );
        }
        Some(other) => {
            eprintln!("paavo-cli: unknown stream line type: {other}");
        }
        None => {
            eprintln!("paavo-cli: stream line missing `type`: {line}");
        }
    }
}

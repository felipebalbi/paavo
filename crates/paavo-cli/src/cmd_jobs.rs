//! `paavo-cli cancel | logs | jobs`.

use crate::client::Client;
use anyhow::Result;
use serde_json::Value;

/// `paavo-cli cancel <id>`.
pub async fn cancel(client: &Client, job_id: &str) -> Result<()> {
    client
        .post_json::<()>(&format!("/jobs/{job_id}/cancel"), None)
        .await?;
    println!("cancelled: {job_id}");
    Ok(())
}

/// `paavo-cli logs <id> [--follow]`.
///
/// Parses NDJSON lines per spec §9.2: one JSON object per line, with
/// `type` in {frame, terminal, lagged, truncated}. Frame messages
/// print to stdout; the terminal line prints a summary and returns.
/// Lagged/truncated markers print to stderr so they don't pollute
/// command-output capture.
pub async fn logs(client: &Client, job_id: &str, _follow: bool) -> Result<()> {
    let mut resp = client.stream(job_id).await?;
    let mut buf = String::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find('\n') {
            let line = buf[..idx].trim().to_string();
            buf.drain(..=idx);
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                eprintln!("paavo-cli: skipping malformed stream line: {line}");
                continue;
            };
            match v["type"].as_str() {
                Some("frame") => {
                    let msg = v["frame"]["message"].as_str().unwrap_or("");
                    println!("{msg}");
                }
                Some("terminal") => {
                    println!("--- terminal: {}", v["outcome"]);
                    return Ok(());
                }
                Some("lagged") => {
                    eprintln!(
                        "paavo-cli: log stream lagged ({} frames missed)",
                        v["missed"].as_u64().unwrap_or(0),
                    );
                }
                Some("truncated") => {
                    eprintln!(
                        "paavo-cli: log stream truncated: {}",
                        v["reason"].as_str().unwrap_or("<no reason>"),
                    );
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// `paavo-cli jobs [--state ...] [--limit N]`.
pub async fn list(client: &Client, state: Option<&str>, limit: u32) -> Result<()> {
    let mut path = format!("/jobs?limit={limit}");
    if let Some(s) = state {
        path.push_str(&format!("&state={s}"));
    }
    let rows: Vec<Value> = client.get_json(&path).await?;
    for r in rows {
        println!(
            "{id}  {state:9} {priority:11} {submitter}",
            id = r["id"].as_str().unwrap_or(""),
            state = r["state"].as_str().unwrap_or(""),
            priority = r["priority"].as_str().unwrap_or("?"),
            submitter = r["submitter"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

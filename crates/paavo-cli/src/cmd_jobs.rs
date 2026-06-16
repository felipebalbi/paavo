//! `paavo-cli cancel | logs | jobs`.

use crate::client::Client;
use anyhow::Result;
use paavo_proto::WireMessage;
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
/// Parses NDJSON lines per spec §9.2 / `paavo_proto::WireMessage`:
/// one JSON object per line, with `type` in {frame, terminal,
/// lagged, truncated, phase}. Frame messages print to stdout; the
/// terminal line prints a summary and returns. Lagged/truncated/
/// phase markers print to stderr so they don't pollute command-
/// output capture.
///
/// Forward-compat: a future paavod variant that adds a new `type`
/// fails `serde_json::from_str::<WireMessage>`, surfaces here as a
/// "skipping malformed stream line" stderr note, and the loop
/// continues. Older paavo-cli builds will not panic on a daemon
/// upgrade.
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
            let msg = match serde_json::from_str::<WireMessage>(&line) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("paavo-cli: skipping malformed stream line: {e}: {line}");
                    continue;
                }
            };
            match msg {
                WireMessage::Frame { frame } => println!("{}", frame.message),
                WireMessage::Terminal { outcome } => {
                    // Use serde_json to render the outcome verbatim so
                    // operators see the same JSON the daemon emitted —
                    // no Display reimplementation drift.
                    let outcome_json = serde_json::to_string(&outcome).unwrap_or_default();
                    println!("--- terminal: {outcome_json}");
                    return Ok(());
                }
                WireMessage::Lagged { missed } => {
                    eprintln!(
                        "paavo-cli: log stream lagged ({missed} frames missed)"
                    );
                }
                WireMessage::Truncated { reason } => {
                    eprintln!("paavo-cli: log stream truncated: {reason}");
                }
                WireMessage::Phase { phase } => {
                    eprintln!("paavo-cli: phase = {phase:?}");
                }
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

//! `paavo-cli run`: tar a crate dir / .rs / .elf and submit. Streams output.

use crate::cli::PriorityArg;
use crate::client::Client;
use anyhow::{Context, Result};
use paavo_proto::{BoardSelector, JobSpec, Priority};
use std::path::Path;

/// Entry point for `paavo-cli run`.
///
/// Behaviour: tar the crate, POST to `/jobs`, print the assigned ULID.
/// Default is fire-and-forget — exits 0 once the upload is accepted.
/// With `--follow / -f` (`follow=true`), keep the terminal open and
/// stream the NDJSON log until the terminal frame, exiting with a
/// status code that reflects the outcome (0 = Passed, non-zero
/// otherwise). Spec §10.1.
#[allow(clippy::too_many_arguments)] // mirrors the clap surface 1:1 by intent
pub async fn run(
    client: &Client,
    path: &Path,
    board_kind: Option<&str>,
    instance: Option<&str>,
    timeout: Option<&str>,
    inactivity: Option<&str>,
    priority: PriorityArg,
    follow: bool,
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

    if follow {
        stream_logs(client, &job_id).await
    } else {
        // Fire-and-forget: hint at the follow command so the operator
        // doesn't have to remember the syntax. Hint goes to stderr so
        // scripts piping stdout for the job id stay clean.
        eprintln!("tail with: paavo-cli logs {job_id} --follow");
        Ok(())
    }
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

/// Tar the crate directory, skipping build output and editor scratch
/// that paavod doesn't need (and would push the upload over its body
/// cap — a stale `target/` from a local `cargo build` can easily be
/// 500+ MiB).
///
/// Skipped path components (matched on the entry's name, anywhere in
/// the tree): `target`, `.git`, `.cargo` (because the daemon imposes
/// its own; rely on `paavo.toml` instead), `node_modules`, `.idea`,
/// `.vscode`, plus the local `Cargo.lock` file (paavod resolves deps
/// fresh per spec §8.1; shipping the lock would override that).
///
/// The tar entries are prefixed with the crate's directory name so
/// paavod's `unpack_into` produces `<sandbox>/<crate>/Cargo.toml`,
/// matching what `build_or_cache::walkdir` looks for.
fn make_tar(dir: &Path) -> Result<Vec<u8>> {
    let prefix = dir.file_name().unwrap_or_default();
    let mut buf = Vec::new();
    let mut t = tar::Builder::new(&mut buf);

    for entry in walkdir::WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .filter_entry(should_keep)
    {
        let entry = entry.context("walking crate dir")?;
        let relative = entry.path().strip_prefix(dir).unwrap();
        let in_tar = std::path::Path::new(prefix).join(relative);
        let ft = entry.file_type();
        if ft.is_dir() {
            t.append_dir(&in_tar, entry.path())
                .with_context(|| format!("tar append_dir {}", entry.path().display()))?;
        } else if ft.is_file() {
            t.append_path_with_name(entry.path(), &in_tar)
                .with_context(|| format!("tar append_path {}", entry.path().display()))?;
        }
        // Symlinks and other special entries silently skipped — they'd
        // bloat the tar with redundant content and paavod's
        // unpack_into doesn't promise to honor them.
    }
    t.finish().context("tar finalize")?;
    drop(t);
    Ok(buf)
}

/// Filter for `walkdir::WalkDir::filter_entry`. Returning `false`
/// prunes the entry AND (for directories) its entire subtree.
fn should_keep(e: &walkdir::DirEntry) -> bool {
    let Some(name) = e.file_name().to_str() else {
        return true;
    };
    // Build output + VCS + editor scratch. Listed explicitly so a
    // future contributor can grep the rationale for each.
    const SKIP: &[&str] = &[
        "target",       // cargo build output
        ".git",         // VCS
        ".cargo",       // paavod has its own .cargo/config; don't override
        "node_modules", // unlikely but cheap to exclude
        ".idea",        // JetBrains
        ".vscode",      // VS Code
    ];
    if e.file_type().is_dir() && SKIP.contains(&name) {
        return false;
    }
    // Skip Cargo.lock at the crate root only. paavod resolves deps
    // fresh (spec §8.1); a checked-in lock would override that. We
    // can't easily tell "root" here, but Cargo.lock anywhere in a
    // test-crate tree is unusual enough to safely skip globally.
    if e.file_type().is_file() && name == "Cargo.lock" {
        return false;
    }
    true
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

//! `paavo-cli run`: tar a crate dir / .rs / .elf and submit. Streams output.

use crate::cli::PriorityArg;
use crate::client::Client;
use anyhow::{Context, Result};
use paavo_proto::{BoardSelector, BoardView, JobSpec, Priority};
use std::collections::BTreeSet;
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
    skip_cache: bool,
) -> Result<()> {
    // Resolve the board selector BEFORE touching the crate, so a bad
    // --instance or an ambiguous kind fails fast without tarring or
    // uploading anything. Only the explicit-kind-without-instance case
    // can skip the GET /boards round-trip.
    let selector = {
        let need_inventory = instance.is_some() || board_kind.is_none();
        let boards = if need_inventory {
            client
                .list_boards()
                .await
                .context("fetching board inventory from paavod")?
        } else {
            Vec::new()
        };
        resolve_board_selector(board_kind, instance, &boards)?
    };
    eprintln!(
        "resolved board: kind={} instance={}",
        selector.kind,
        selector.instance.as_deref().unwrap_or("(any)")
    );

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
        board_selector: selector,
        inactivity_timeout_ms: inactivity.map(parse_duration_ms).transpose()?,
        hard_max_ms: timeout.map(parse_duration_ms).transpose()?,
        skip_cache,
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
/// the tree): `target`, `.git`, `node_modules`, `.idea`, `.vscode`,
/// plus the local `Cargo.lock` file (paavod resolves deps fresh per
/// spec §8.1; shipping the lock would override that).
///
/// `.cargo/` is INTENTIONALLY kept. The scaffold's `.cargo/config.toml`
/// carries load-bearing settings: `[build] target =
/// "thumbv8m.main-none-eabihf"` (without it, paavod runs `cargo build
/// --release` from the sandbox with no `--target` flag and cargo
/// defaults to the host triple — which then host-compiles cortex-m
/// and fails because its inline asm references thumb-only registers);
/// `[target.thumbv8m.main-none-eabihf] rustflags = ["-C",
/// "link-arg=-Tdefmt.x", "-C", "link-arg=--nmagic"]`; and
/// `[net] git-fetch-with-cli = true` (libgit2's GitHub clone fails
/// on Windows when a git credential helper is configured). All three
/// surfaced during the M7.7 manual smoke.
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
    //
    // NOTE: `.cargo` is INTENTIONALLY NOT in this list. The scaffold's
    // `.cargo/config.toml` carries load-bearing settings (target
    // triple, rustflags, net.git-fetch-with-cli) that paavod does NOT
    // inject and that the build will fail without. See `make_tar`
    // doc comment for the full rationale.
    const SKIP: &[&str] = &[
        "target",       // cargo build output
        ".git",         // VCS
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

/// Parse one NDJSON line from `/jobs/:id/stream` (spec §9.2 /
/// `paavo_proto::WireMessage`). Frames print as `<message>`; the
/// terminal line prints a summary and exits with 0 for Passed, 1
/// otherwise. `lagged`/`truncated`/`phase` markers print to stderr so
/// they don't pollute the test-output capture.
///
/// Forward-compat: a future paavod variant that adds a new `type`
/// fails `serde_json::from_str::<WireMessage>` and surfaces here as
/// a "skipping malformed stream line" stderr note. Older paavo-cli
/// builds never panic on a daemon upgrade — the daemon's wire
/// shape is additive-only by contract (see `paavo-proto`'s
/// `WireMessage` rustdoc).
fn handle_ndjson_line(line: &str) {
    use paavo_proto::{JobOutcome, WireMessage};
    let msg = match serde_json::from_str::<WireMessage>(line) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("paavo-cli: skipping malformed stream line: {e}: {line}");
            return;
        }
    };
    match msg {
        WireMessage::Frame { frame } => println!("{}", frame.message),
        WireMessage::Terminal { outcome } => {
            // Render the outcome JSON so it matches what the daemon
            // emitted byte-for-byte; avoids hand-written Display drift.
            let outcome_json = serde_json::to_string(&outcome).unwrap_or_default();
            println!("--- terminal: {outcome_json}");
            // Exit 0 only on Passed; everything else is a non-zero
            // exit so CI scripts can chain on success.
            std::process::exit(if matches!(outcome, JobOutcome::Passed) {
                0
            } else {
                1
            });
        }
        WireMessage::Lagged { missed } => {
            eprintln!(
                "paavo-cli: log stream lagged ({missed} frames missed); refetch /jobs/:id for the full log"
            );
        }
        WireMessage::Truncated { reason } => {
            eprintln!("paavo-cli: log stream truncated: {reason}");
        }
        WireMessage::Phase { phase } => {
            // Phase is a UI hint for live viewers (paavo-web's banner).
            // CLI tail surfaces it on stderr so it doesn't show up in
            // captured test output, but the operator can still see
            // "build → run" transitions on a manual `paavo-cli run --follow`.
            eprintln!("paavo-cli: phase = {phase:?}");
        }
    }
}

/// Render the known board ids for an error message.
fn join_ids(boards: &[BoardView]) -> String {
    boards
        .iter()
        .map(|b| b.spec.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the wire `BoardSelector` from the two optional CLI flags plus the
/// daemon inventory. Pure: the caller performs the `GET /boards`.
///
/// Rules (spec 2026-06-17-cli-board-kind-resolution):
///   (instance=Some, _)      derive kind from that board; cross-check an
///                           explicit `board_kind` if also given.
///   (instance=None, Some k) use `k` verbatim (inventory ignored).
///   (instance=None, None)   the sole inventory kind, or an actionable error.
fn resolve_board_selector(
    board_kind: Option<&str>,
    instance: Option<&str>,
    boards: &[BoardView],
) -> Result<BoardSelector> {
    match (instance, board_kind) {
        (Some(id), kind_hint) => {
            let board = boards.iter().find(|b| b.spec.id == id).ok_or_else(|| {
                anyhow::anyhow!(
                    "no board with id '{id}' in inventory (known: {})",
                    join_ids(boards)
                )
            })?;
            let kind = board.spec.kind.as_str();
            if let Some(hint) = kind_hint {
                if hint != kind {
                    anyhow::bail!(
                        "board '{id}' is kind '{kind}', which conflicts with --board-kind '{hint}'"
                    );
                }
            }
            Ok(BoardSelector {
                kind: kind.to_string(),
                instance: Some(id.to_string()),
                wiring_profile: None,
            })
        }
        (None, Some(kind)) => Ok(BoardSelector {
            kind: kind.to_string(),
            instance: None,
            wiring_profile: None,
        }),
        (None, None) => {
            let kinds: BTreeSet<&str> = boards.iter().map(|b| b.spec.kind.as_str()).collect();
            match kinds.len() {
                1 => Ok(BoardSelector {
                    kind: kinds.into_iter().next().unwrap().to_string(),
                    instance: None,
                    wiring_profile: None,
                }),
                0 => anyhow::bail!(
                    "no boards registered with paavod; pass --board-kind <kind> \
                     (or register a board first)"
                ),
                _ => anyhow::bail!(
                    "multiple board kinds available ({}); \
                     pass --instance <id> or --board-kind <kind>",
                    kinds.into_iter().collect::<Vec<_>>().join(", ")
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read as _;

    // NOTE: do NOT import `BoardView` here — it is imported at the file's top
    // level and reaches this module via the existing `use super::*`. Importing
    // it again would be a redundant import (caught by `-D warnings`).
    use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};

    /// Build a minimal `BoardView` for resolver tests. Only `id` and
    /// `kind` matter to `resolve_board_selector`; the rest are filler.
    fn bv(id: &str, kind: &str) -> BoardView {
        BoardView {
            spec: BoardSpec {
                id: id.into(),
                kind: kind.into(),
                probe_selector: ProbeSelector {
                    vid: "1366".into(),
                    pid: "1015".into(),
                    serial: format!("S-{id}"),
                    interface: None,
                },
                chip_name: "MCXA266".into(),
                target_name: format!("target-{kind}"),
                wiring_profile: None,
                health: BoardHealth::Healthy,
            },
            quarantine_reason: None,
            consecutive_infra_failures: 0,
            last_used_at: None,
            created_at: 0,
        }
    }

    #[test]
    fn instance_derives_kind() {
        let inv = vec![bv("mcxa266-01", "mcxa266"), bv("mcxa266-02", "mcxa266")];
        let sel = resolve_board_selector(None, Some("mcxa266-02"), &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance.as_deref(), Some("mcxa266-02"));
        assert_eq!(sel.wiring_profile, None);
    }

    #[test]
    fn instance_not_found_errors() {
        let inv = vec![bv("mcxa266-01", "mcxa266")];
        let err = resolve_board_selector(None, Some("nope"), &inv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope"), "got: {msg}");
        assert!(
            msg.contains("mcxa266-01"),
            "should list known ids; got: {msg}"
        );
    }

    #[test]
    fn instance_and_matching_kind_ok() {
        let inv = vec![bv("mcxa266-01", "mcxa266")];
        let sel = resolve_board_selector(Some("mcxa266"), Some("mcxa266-01"), &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance.as_deref(), Some("mcxa266-01"));
    }

    #[test]
    fn instance_and_conflicting_kind_errors() {
        let inv = vec![bv("mcxa266-01", "mcxa266")];
        let err = resolve_board_selector(Some("rt685"), Some("mcxa266-01"), &inv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mcxa266"), "got: {msg}");
        assert!(msg.contains("rt685"), "got: {msg}");
    }

    #[test]
    fn neither_single_kind_defaults() {
        let inv = vec![bv("mcxa266-01", "mcxa266"), bv("mcxa266-02", "mcxa266")];
        let sel = resolve_board_selector(None, None, &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance, None);
    }

    #[test]
    fn neither_multiple_kinds_errors() {
        let inv = vec![bv("mcxa266-01", "mcxa266"), bv("rt685-01", "rt685")];
        let err = resolve_board_selector(None, None, &inv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple board kinds"), "got: {msg}");
        assert!(
            msg.contains("mcxa266") && msg.contains("rt685"),
            "got: {msg}"
        );
    }

    #[test]
    fn neither_zero_boards_errors() {
        let inv: Vec<BoardView> = vec![];
        let err = resolve_board_selector(None, None, &inv).unwrap_err();
        assert!(
            err.to_string().contains("no boards registered"),
            "got: {err}"
        );
    }

    #[test]
    fn explicit_kind_no_instance_skips_inventory() {
        // Empty inventory on purpose: the (None, Some) branch must not
        // depend on it (the caller skips the GET /boards round-trip).
        let inv: Vec<BoardView> = vec![];
        let sel = resolve_board_selector(Some("mcxa266"), None, &inv).unwrap();
        assert_eq!(sel.kind, "mcxa266");
        assert_eq!(sel.instance, None);
    }

    /// Regression for the M7.7 manual smoke: `.cargo/config.toml`
    /// MUST survive the tar. Stripping it (as an earlier version of
    /// `should_keep` did, on the now-invalidated theory that paavod
    /// injects its own config) makes paavod's `cargo build --release`
    /// fall back to the host triple — which then host-compiles
    /// cortex-m and fails with 6 cortex-m errors (E0425 ×4 for
    /// __basepri_{r,w,max} + __faultmask_r, plus 2 "invalid register"
    /// errors for r0/r1).
    #[test]
    fn make_tar_preserves_dot_cargo_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let crate_dir = tmp.path().join("hello-mcxa266");
        fs::create_dir_all(crate_dir.join(".cargo")).unwrap();
        fs::create_dir_all(crate_dir.join("src")).unwrap();
        fs::write(
            crate_dir.join("Cargo.toml"),
            "[package]\nname = \"hello-mcxa266\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(crate_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            crate_dir.join(".cargo/config.toml"),
            "[build]\ntarget = \"thumbv8m.main-none-eabihf\"\n",
        )
        .unwrap();

        let buf = make_tar(&crate_dir).expect("make_tar");
        let mut archive = tar::Archive::new(buf.as_slice());
        let mut names = Vec::new();
        let mut dot_cargo_config_contents: Option<String> = None;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().to_string();
            // Normalize to forward slashes so the assertion works on
            // Windows where tar may emit backslashes.
            let path = path.replace('\\', "/");
            if path == "hello-mcxa266/.cargo/config.toml" {
                let mut s = String::new();
                entry.read_to_string(&mut s).unwrap();
                dot_cargo_config_contents = Some(s);
            }
            names.push(path);
        }

        assert!(
            names.contains(&"hello-mcxa266/.cargo/config.toml".to_string()),
            ".cargo/config.toml missing from tar; entries: {names:?}"
        );
        assert_eq!(
            dot_cargo_config_contents.as_deref(),
            Some("[build]\ntarget = \"thumbv8m.main-none-eabihf\"\n"),
            "config.toml contents mangled in tar"
        );
    }

    /// Sibling positive assertion: `target/` is still stripped. If
    /// this ever flips, multi-GB tar uploads come back.
    #[test]
    fn make_tar_strips_target_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let crate_dir = tmp.path().join("hello-mcxa266");
        fs::create_dir_all(crate_dir.join("src")).unwrap();
        fs::create_dir_all(crate_dir.join("target/release")).unwrap();
        fs::write(
            crate_dir.join("Cargo.toml"),
            "[package]\nname = \"hello-mcxa266\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(crate_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        // A large-ish file under target/ to make a regression obvious
        // in the tar size, not just the entry list.
        fs::write(crate_dir.join("target/release/some.elf"), vec![0u8; 4096]).unwrap();

        let buf = make_tar(&crate_dir).expect("make_tar");
        let mut archive = tar::Archive::new(buf.as_slice());
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path().unwrap().to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(
            !names.iter().any(|n| n.contains("/target/")),
            "target/ should be stripped from tar; entries: {names:?}"
        );
    }
}

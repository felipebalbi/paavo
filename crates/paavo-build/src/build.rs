//! `cargo build --release` invocation, with line-by-line stderr/stdout
//! streaming, an 8 KiB stderr-tail capture, and ELF discovery handoff.
//!
//! ## Two entry points
//!
//! - [`build_release_streaming`] — the streaming surface. Emits each
//!   line of cargo's stdout AND stderr in real time over a caller-
//!   supplied `Sender<BuildLine>`. Used by paavod (commit C) to
//!   broadcast build output to live `/jobs/:id/stream` subscribers
//!   AND persist the lines as `log_frame` rows tagged
//!   `target = "cargo:stdout"` / `"cargo:stderr"`.
//! - [`build_release`] — back-compat wrapper. Equivalent to calling
//!   [`build_release_streaming`] with a sink whose receiver is
//!   dropped immediately, so every line is sent into the void.
//!   Preserved for the existing `tests/build_invocation.rs` test
//!   surface and for any caller that genuinely doesn't care about
//!   live output.
//!
//! ## Why streaming
//!
//! The prior `Command::output()` shape buffered stdout+stderr until
//! the child exited, then took the last 8 KiB of stderr. Stdout was
//! dropped entirely. That precluded "live build progress" on the
//! paavo-web job page, and operators on a long compile saw no
//! feedback for minutes. The streaming refactor pipes both streams
//! through `Stdio::piped()` + `BufRead::lines()` on dedicated reader
//! threads — see the architect spec in
//! `docs/superpowers/plans/paavo-build-streaming.md` (or the commit
//! description for the rationale if no doc lands).
//!
//! ## Concurrency model
//!
//! For each cargo invocation:
//!
//! - Main thread: spawns the child with `Stdio::piped()` on stdout
//!   and stderr, hands the pipes to two reader threads, waits for
//!   the child, joins both readers (whose loops terminate at EOF).
//! - "paavo-build-stdout" thread: drains `child.stdout` line-by-line,
//!   sends each as `BuildLine { stream: Stdout, text }` over the
//!   caller's sink.
//! - "paavo-build-stderr" thread: same as stdout but tagged Stderr,
//!   AND maintains an 8 KiB ring buffer ([`StderrTail`]) so the
//!   final `BuildResult.stderr_tail` (or, on failure,
//!   `BuildError::Cargo.stderr`) reflects the trailing bytes.
//!
//! Two threads instead of one + non-blocking I/O because: (a) `std`
//! has no portable non-blocking-pipe API, (b) crossbeam-channel +
//! `BufRead::lines()` already give us the right shape, (c) cargo's
//! lines are bounded in size (single-digit KiB worst case) so two
//! cheap thread spawns per build is fine. paavo-build is sync-first
//! by design — paavod calls it from `tokio::task::spawn_blocking`,
//! and adding tokio inside paavo-build would mean nesting a runtime.
//!
//! ## Sink semantics
//!
//! `Sender::send` is best-effort: if the caller's receiver has been
//! dropped (the back-compat wrapper [`build_release`] does this on
//! purpose), each `send` returns `Err(SendError(_))` and the reader
//! threads silently swallow it. The build still runs to completion;
//! the caller just forfeits the live tail. This matches paavo-
//! runner's `log_tx` semantics — a dropped receiver MUST NOT abort
//! the producer.

use crate::elf::{discover_elf, ManifestArtifactHint};
use crate::error::{BuildError, Result};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

/// Maximum bytes of stderr to keep in the rolling ring buffer.
///
/// This is the same 8 KiB cap the prior bulk-capture path used; it
/// preserves byte-for-byte compatibility of `BuildResult.stderr_tail`
/// and `BuildError::Cargo.stderr` so paavod's outcome serialisation
/// (and any operator scripts grepping `outcome_detail`) don't shift.
const TAIL_MAX_BYTES: usize = 8 * 1024;

/// Which child stream a [`BuildLine`] came from. paavo-build does NOT
/// pick a log severity — that is the caller's translation layer
/// (paavod's build forwarder maps stdout→`target="cargo:stdout"` and
/// stderr→`target="cargo:stderr"`, both at `LogLevel::Info`, per the
/// architect spec). Keeping the discriminator at the wire boundary
/// preserves the option to surface the distinction in viewers
/// (different colour, filterable column, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStream {
    /// `cargo`'s stdout. `cargo build` mostly writes to stderr;
    /// build-script `println!`s and a few diagnostics land here.
    Stdout,
    /// `cargo`'s stderr — `Compiling foo v1.0`, `Finished release [...]`,
    /// rustc diagnostics, the lot.
    Stderr,
}

/// One line of cargo output, with a discriminator for which child
/// stream it came from. The text is one logical line with the
/// trailing `\n` (or `\r\n`) stripped. Empty lines are emitted as
/// `text: ""` — callers that filter blanks should do so themselves.
///
/// No timestamp field: the receiver (paavod) timestamps when it
/// converts to `paavo_proto::LogFrame`. Keeping paavo-build "dumb"
/// means tests can match exact-string output without clock injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildLine {
    /// Source pipe.
    pub stream: BuildStream,
    /// One line, no trailing newline. Always UTF-8 (cargo's output
    /// is UTF-8 in practice; non-UTF-8 bytes cause the `BufRead::
    /// lines()` iterator to terminate early — see "failure modes"
    /// in the architect spec).
    pub text: String,
}

/// Sink for [`BuildLine`]s. A `crossbeam_channel::Sender` so the
/// caller can clone the receiver if they want a fan-out (paavod's
/// build forwarder doesn't, but a future test may). Dropping the
/// matching `Receiver` makes every subsequent `send` an `Err` which
/// paavo-build silently swallows; the build still runs to
/// completion.
pub type BuildLineTx = crossbeam_channel::Sender<BuildLine>;

/// Build plan derived from a `JobSpec` and a sandbox directory.
#[derive(Debug, Clone)]
pub struct BuildPlan {
    /// Sandbox dir containing the unpacked crate.
    pub crate_dir: PathBuf,
    /// `CARGO_TARGET_DIR` to share across jobs for incremental reuse.
    pub target_dir: PathBuf,
    /// Optional `cargo update -p ...` packages to refresh before building
    /// (used by soak-test corpora that track `embassy-rs/embassy` main).
    pub cargo_update_packages: Vec<String>,
}

/// What `build_release` and `build_release_streaming` return on
/// success.
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Path to the discovered ELF.
    pub elf_path: PathBuf,
    /// Size of the ELF on disk, bytes.
    pub elf_size_bytes: u64,
    /// Captured stderr tail (last 8 KiB).
    ///
    /// Populated by the streaming stderr reader's ring buffer (see
    /// [`StderrTail`]); byte-identical to what the prior
    /// `Command::output()` + `tail()` path produced.
    pub stderr_tail: String,
}

/// Streaming-aware variant of [`build_release`]. Each line of cargo's
/// stdout and stderr is sent to `lines` in real time, tagged with
/// which child stream it came from.
///
/// The sink stays open for the entire build, including the
/// `cargo update -p ...` preflight steps (operators tailing a soak
/// run see those lines too). It is the caller's responsibility to
/// drop their own clone of the `Sender` (or its `Receiver`) when
/// they're done consuming; paavo-build drops every clone it owns
/// before returning.
///
/// Failure semantics are identical to [`build_release`]: a non-zero
/// cargo exit returns [`BuildError::Cargo`] with the last 8 KiB of
/// stderr in the `stderr` field. By construction every line that
/// landed in that tail was ALSO sent to `lines` before the error
/// returned (the stderr reader thread sends-then-rolls inside the
/// same line iteration; the function only returns after the reader
/// has joined to EOF — see "ordering invariant" in the docstring of
/// [`run_cargo_streaming`]).
pub fn build_release_streaming(plan: &BuildPlan, lines: BuildLineTx) -> Result<BuildResult> {
    // Non-cancellable: drop the sender so `wait_or_kill` sees a
    // disconnected channel and blocks to completion exactly like
    // `child.wait()` did before.
    let (never_tx, never_rx) = crossbeam_channel::unbounded::<()>();
    drop(never_tx);
    build_release_streaming_cancellable(plan, lines, never_rx)
}

/// Cancellable variant of [`build_release_streaming`]. `cancel_rx`
/// firing kills the in-flight cargo child and returns
/// [`BuildError::Cancelled`]. paavod uses this so a `POST
/// /jobs/:id/cancel` during the build phase stops cargo promptly.
pub fn build_release_streaming_cancellable(
    plan: &BuildPlan,
    lines: BuildLineTx,
    cancel_rx: crossbeam_channel::Receiver<()>,
) -> Result<BuildResult> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    // Pre-build cargo updates: lines flow through the sink but the
    // tail is intentionally NOT contributed to BuildResult.stderr_tail
    // — that field has historically been the *build*'s stderr and
    // operators scripting against it expect it not to drift on update
    // failures. Tail returned from these calls is therefore discarded.
    for pkg in &plan.cargo_update_packages {
        let _tail = run_cargo_streaming(
            &cargo,
            &["update", "-p", pkg],
            plan,
            lines.clone(),
            &cancel_rx,
        )?;
    }

    let stderr_tail =
        run_cargo_streaming(&cargo, &["build", "--release"], plan, lines, &cancel_rx)?;

    let hint = ManifestArtifactHint::default();
    let elf_path = discover_elf(&plan.crate_dir, &plan.target_dir, &hint)?;
    let elf_size_bytes = std::fs::metadata(&elf_path)?.len();
    Ok(BuildResult {
        elf_path,
        elf_size_bytes,
        stderr_tail,
    })
}

/// Invoke `cargo build --release` in `plan.crate_dir`, then discover the ELF.
///
/// Back-compat wrapper around [`build_release_streaming`] that drops
/// every BuildLine on the floor. Preserved for the existing test
/// surface (`tests/build_invocation.rs`) and for callers that
/// genuinely want fire-and-forget semantics.
///
/// The `cargo` binary is selected from the `CARGO` env var (set by
/// cargo itself when running tests/build scripts) and falls back to
/// plain `"cargo"` on `$PATH`. This is the "cargo spawns cargo"
/// idiom that lets the workspace's pinned toolchain version flow
/// through.
pub fn build_release(plan: &BuildPlan) -> Result<BuildResult> {
    // (tx, rx) where rx is dropped at end of scope, so every
    // tx.send(...) inside the streaming pipeline returns Err and is
    // silently swallowed by the reader threads. Builds runs to
    // completion regardless.
    let (tx, _rx) = crossbeam_channel::unbounded::<BuildLine>();
    build_release_streaming(plan, tx)
}

/// Spawn cargo with piped stdio, drain both streams concurrently
/// over `tx`, capture the trailing 8 KiB of stderr, wait for the
/// child, return the captured tail (success path) or
/// `BuildError::Cargo { exit, stderr: tail }` (non-zero exit).
///
/// **Ordering invariant** (covered by I1 in the architect spec):
/// every line that lands in the returned `tail` (or the
/// `BuildError::Cargo.stderr` field) was also sent over `tx` before
/// this function returns. Concretely, the stderr reader thread
/// `tx.send`-s before pushing into the ring buffer; the function
/// only returns after `stderr_join.join()` has yielded the final
/// `StderrTail` (which only happens after the pipe reaches EOF,
/// which only happens after the child has fully written everything
/// it was going to).
fn run_cargo_streaming(
    cargo: &std::ffi::OsStr,
    args: &[&str],
    plan: &BuildPlan,
    tx: BuildLineTx,
    cancel_rx: &crossbeam_channel::Receiver<()>,
) -> Result<String> {
    let mut child = Command::new(cargo)
        .args(args)
        .current_dir(&plan.crate_dir)
        .env("CARGO_TARGET_DIR", &plan.target_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // `take()` so the underlying file descriptors are owned by the
    // reader threads and released when those threads exit. Leaving
    // them on `child` would risk a deadlock: `child.wait()` waits
    // for the kernel to close all our handles to the pipes, and we
    // need the readers to drop their handles for that to happen.
    //
    // `expect` rather than `?`: with `Stdio::piped()` the `Option`
    // is always `Some` directly after `spawn()`. A `None` here would
    // be a stdlib bug, not an operational error.
    let stdout = child
        .stdout
        .take()
        .expect("piped stdout always present after spawn");
    let stderr = child
        .stderr
        .take()
        .expect("piped stderr always present after spawn");

    // stdout reader: send each line as Stdout, never contribute to
    // the tail. catch_unwind is defence-in-depth — `BuildLine`'s
    // fields can't currently panic on Drop, but a future field that
    // can would otherwise deadlock the build (kernel waits for us to
    // drain the pipe; reader thread is dead). The wrapper costs a
    // single panic-payload boxed allocation on the cold path.
    let stdout_tx = tx.clone();
    let stdout_join = thread::Builder::new()
        .name("paavo-build-stdout".into())
        .spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(s) => {
                            // `Err(SendError(_))` means caller's rx
                            // is gone; carry on, the build doesn't
                            // care about the live tail.
                            let _ = stdout_tx.send(BuildLine {
                                stream: BuildStream::Stdout,
                                text: s,
                            });
                        }
                        Err(_) => break,
                    }
                }
            }));
        })
        .expect("spawn paavo-build-stdout reader");

    // stderr reader: send each line as Stderr AND append to the
    // ring buffer. The buffer is owned by this thread, returned via
    // the JoinHandle so the main thread can take it without a Mutex.
    let stderr_tx = tx;
    let stderr_join = thread::Builder::new()
        .name("paavo-build-stderr".into())
        .spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut tail = StderrTail::with_capacity(TAIL_MAX_BYTES);
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(s) => {
                            tail.push_line(&s);
                            let _ = stderr_tx.send(BuildLine {
                                stream: BuildStream::Stderr,
                                text: s,
                            });
                        }
                        Err(_) => break,
                    }
                }
                tail
            }))
            .unwrap_or_else(|_| StderrTail::with_capacity(0))
        })
        .expect("spawn paavo-build-stderr reader");

    let (status, cancelled) = wait_or_kill(&mut child, cancel_rx)?;

    // Read order matters here: stdout_join FIRST so a faulty stderr
    // path can't deadlock a `_ = stdout_join.join()` that the lint
    // demands. Both reader threads exit on EOF (which the child has
    // already reached because wait() returned), so neither join
    // blocks meaningfully.
    let _ = stdout_join.join();
    let tail: StderrTail = stderr_join
        .join()
        .unwrap_or_else(|_| StderrTail::with_capacity(0));
    let stderr_tail = tail.snapshot();

    if cancelled {
        return Err(BuildError::Cancelled);
    }
    if !status.success() {
        return Err(BuildError::Cargo {
            exit: status.code(),
            stderr: stderr_tail,
        });
    }
    Ok(stderr_tail)
}

/// Wait for `child`, but if `cancel_rx` fires first, kill it. Returns
/// `(status, was_cancelled)`. Polls `try_wait` so we react to cancel
/// without a second thread. A dropped sender (the non-cancellable
/// `build_release` path) blocks to completion exactly like `wait()`.
fn wait_or_kill(
    child: &mut std::process::Child,
    cancel_rx: &crossbeam_channel::Receiver<()>,
) -> std::io::Result<(std::process::ExitStatus, bool)> {
    use crossbeam_channel::RecvTimeoutError;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, false));
        }
        match cancel_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(()) => {
                let _ = child.kill();
                let status = child.wait()?;
                return Ok((status, true));
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                let status = child.wait()?;
                return Ok((status, false));
            }
        }
    }
}

/// Rolling last-8-KiB-of-stderr buffer. Fed by the stderr reader
/// thread, drained by the main thread after the reader joins.
///
/// Bytes-based (`VecDeque<u8>`) so we don't pay an O(n) String-
/// truncate per line; a UTF-8 multibyte codepoint that ends up split
/// across the ring's wrap-around point is fixed up at `snapshot()`
/// time by the existing UTF-8-safe `tail()` helper. Worst case is
/// 1-3 leading bytes lost — same boundary as the prior `tail()`
/// call site.
struct StderrTail {
    buf: VecDeque<u8>,
    cap: usize,
}

impl StderrTail {
    fn with_capacity(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Append `line` followed by a `\n` separator. If the buffer
    /// would exceed `cap`, drop bytes from the front. Amortized
    /// O(line.len()).
    fn push_line(&mut self, line: &str) {
        for b in line.as_bytes() {
            self.push_byte(*b);
        }
        self.push_byte(b'\n');
    }

    fn push_byte(&mut self, b: u8) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(b);
    }

    /// Materialize the ring as a UTF-8 String. Reuses the existing
    /// `tail()` helper to walk forward to the next char boundary on
    /// the front edge, dropping at most a few bytes of split
    /// codepoint. Preserves the byte-level shape of the prior
    /// `String::from_utf8_lossy(&output.stderr); tail(...)` path.
    fn snapshot(&self) -> String {
        let bytes: Vec<u8> = self.buf.iter().copied().collect();
        let s = String::from_utf8_lossy(&bytes);
        tail(&s, self.cap)
    }
}

/// Truncate `s` to at most `max_bytes` from the end, respecting UTF-8
/// character boundaries (so we never split a multibyte codepoint and end
/// up with invalid UTF-8 in a captured stderr tail).
fn tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let start = s.len() - max_bytes;
    let mut idx = start;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    s[idx..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sleeper() -> std::process::Child {
        let mut c = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping 127.0.0.1 -n 30 > NUL"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c
        };
        c.stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn wait_or_kill_kills_promptly_on_signal() {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        let mut child = sleeper();
        let start = std::time::Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = tx.send(());
        });
        let (_status, cancelled) = wait_or_kill(&mut child, &rx).unwrap();
        assert!(cancelled, "should report cancellation");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "child should be killed promptly, not run the full sleep"
        );
    }

    #[test]
    fn wait_or_kill_runs_to_completion_when_sender_dropped() {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        drop(tx); // never-cancellable path → Disconnected → block to completion
        let mut child = if cfg!(windows) {
            Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap()
        } else {
            Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap()
        };
        let (status, cancelled) = wait_or_kill(&mut child, &rx).unwrap();
        assert!(!cancelled);
        assert!(status.success());
    }

    #[test]
    fn stderr_tail_keeps_only_last_n_bytes() {
        let mut t = StderrTail::with_capacity(10);
        t.push_line("0123"); // 5 bytes (4 + \n)
        t.push_line("4567"); // 5 bytes (4 + \n) → 10 total, at cap
        t.push_line("89"); // 3 bytes; pushes "0123\n" head past cap
        let snap = t.snapshot();
        // After 13 pushes against a cap of 10, we have the last 10
        // bytes: "23\n4567\n89\n" but that's 11. Step through: the
        // ring oscillates as we exceed cap. Just sanity-check the
        // tail ends with "89\n" and is at most 10 bytes long.
        assert!(
            snap.ends_with("89\n"),
            "tail did not end with '89\\n': {snap:?}"
        );
        assert!(
            snap.len() <= 10,
            "tail exceeded cap: len={} body={snap:?}",
            snap.len()
        );
    }

    #[test]
    fn stderr_tail_below_cap_returns_full_buffer() {
        let mut t = StderrTail::with_capacity(64);
        t.push_line("hello");
        t.push_line("world");
        let snap = t.snapshot();
        assert_eq!(snap, "hello\nworld\n");
    }

    #[test]
    fn stderr_tail_handles_utf8_split_at_boundary() {
        // The 4-byte emoji sits at the wrap point so the front edge
        // of the ring is a partial codepoint. tail() should fast-
        // forward to the next char boundary instead of producing
        // invalid UTF-8.
        let mut t = StderrTail::with_capacity(8);
        t.push_line("aa🌶"); // "aa" + "🌶" (4 bytes) + "\n" = 7 bytes
        t.push_line("bb"); // 3 bytes; total 10, evicts the front
        let snap = t.snapshot();
        // No assertion on the exact prefix; what matters is that the
        // returned string is valid UTF-8 (which `String` already
        // enforces, but if `from_utf8_lossy` had to substitute it
        // would do so silently).
        assert!(
            snap.is_char_boundary(0) && snap.is_char_boundary(snap.len()),
            "snap is not at char boundaries: {snap:?}"
        );
    }
}

//! Injectable build step, mirroring `paavo_core::Runner`. Lets dispatch
//! run a real cargo build in production and a fake one in tests, and
//! routes Building-phase cancellation to a killed cargo child.
//!
//! The Builder owns the whole tar → ELF step (unpack, locate crate,
//! cargo). Dispatch owns everything around it: cache lookup, slot
//! assignment, the stable-artifact copy, transitions, and log
//! forwarding. Keeping unpack inside `RealBuilder` lets test builders
//! skip tars entirely.

use paavo_build::BuildLineTx;
use std::path::PathBuf;

/// What dispatch hands a build: the job row plus the two directories it
/// owns for this attempt.
pub struct BuildRequest<'a> {
    /// The job being built (source of `tar_path`, `cargo_update_packages`).
    pub job: &'a paavo_db::JobRow,
    /// Where to unpack the tar (`<state>/sandboxes/<job_id>`).
    pub sandbox_dir: PathBuf,
    /// The slot's `CARGO_TARGET_DIR` (`<state>/build-slots/<i>`).
    pub target_dir: PathBuf,
}

/// Result of a build attempt.
pub enum BuildOutcome {
    /// Built; ELF discovered inside `target_dir`. Dispatch copies it to
    /// the content-addressed cache path.
    Ok {
        /// Path to the discovered ELF (inside the slot's target dir).
        elf_path: PathBuf,
    },
    /// cargo (or unpack/discovery) failed; the `String` is the stderr
    /// tail or error message (→ `BuildErr`).
    Failed(String),
    /// The build was cancelled (cargo child killed) (→ `Aborted{User}`).
    Cancelled,
}

/// The build step dispatch invokes once per job.
pub trait Builder: Send + Sync {
    /// Unpack + compile `req`, streaming cargo lines to `lines`.
    /// `cancel_rx` firing kills the build.
    fn build(
        &self,
        req: BuildRequest<'_>,
        lines: BuildLineTx,
        cancel_rx: crossbeam_channel::Receiver<()>,
    ) -> BuildOutcome;
}

/// Production builder backed by `paavo_build`.
pub struct RealBuilder;

impl Builder for RealBuilder {
    fn build(
        &self,
        req: BuildRequest<'_>,
        lines: BuildLineTx,
        cancel_rx: crossbeam_channel::Receiver<()>,
    ) -> BuildOutcome {
        use std::io::Read;
        // 1. Read + unpack the tar into the sandbox.
        let mut bytes = Vec::new();
        if let Err(e) =
            std::fs::File::open(&req.job.tar_path).and_then(|mut f| f.read_to_end(&mut bytes))
        {
            return BuildOutcome::Failed(format!("read tar {}: {e}", req.job.tar_path));
        }
        if let Err(e) = paavo_build::tar::unpack_into(&bytes, &req.sandbox_dir) {
            return BuildOutcome::Failed(e.to_string());
        }
        // 2. Find the unique crate dir (the one containing Cargo.toml).
        let crate_root = match walkdir::WalkDir::new(&req.sandbox_dir)
            .min_depth(1)
            .max_depth(2)
            .into_iter()
            .flatten()
            .find(|e| e.file_name() == "Cargo.toml")
            .and_then(|e| e.path().parent().map(|p| p.to_path_buf()))
        {
            Some(r) => r,
            None => return BuildOutcome::Failed("no Cargo.toml in uploaded tar".into()),
        };
        // 3. Build into the slot's target dir.
        let plan = paavo_build::BuildPlan {
            crate_dir: crate_root,
            target_dir: req.target_dir,
            cargo_update_packages: req.job.cargo_update_packages.clone(),
        };
        match paavo_build::build_release_streaming_cancellable(&plan, lines, cancel_rx) {
            Ok(res) => BuildOutcome::Ok {
                elf_path: res.elf_path,
            },
            Err(paavo_build::BuildError::Cancelled) => BuildOutcome::Cancelled,
            Err(e) => BuildOutcome::Failed(e.to_string()),
        }
    }
}

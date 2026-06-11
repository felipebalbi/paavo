//! `paavo-cli new`: thin wrapper around `cargo generate`.

use crate::cli::TestKindArg;
use anyhow::Result;

/// `paavo-cli new <name> --board-kind ... --kind ...`
pub fn new(name: &str, board_kind: &str, kind: TestKindArg) -> Result<()> {
    let kind_str = match kind {
        TestKindArg::Quick => "quick",
        TestKindArg::Soak => "soak",
    };
    // Templates live in <paavo-repo>/templates/<board-kind>/. The user
    // is expected to have the paavo repo cloned at a known location;
    // we honor PAAVO_TEMPLATES_DIR to point at it.
    let templates_dir = std::env::var("PAAVO_TEMPLATES_DIR").unwrap_or_else(|_| {
        // fallback: try sibling of the binary.
        let mut p = std::env::current_exe().unwrap_or_default();
        p.pop();
        p.push("../share/paavo/templates");
        p.display().to_string()
    });
    let template_path = std::path::PathBuf::from(&templates_dir).join(board_kind);
    if !template_path.is_dir() {
        anyhow::bail!(
            "template not found at {template_path:?}; \
             set PAAVO_TEMPLATES_DIR to point at the paavo repo's `templates/` dir"
        );
    }
    let status = std::process::Command::new("cargo")
        .args(["generate", "--path"])
        .arg(&template_path)
        .args(["--name", name])
        .args(["--define", &format!("test-kind={kind_str}")])
        .status()?;
    if !status.success() {
        anyhow::bail!("cargo generate exited with {status}");
    }
    Ok(())
}

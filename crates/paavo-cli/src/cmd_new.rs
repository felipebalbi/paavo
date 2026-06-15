//! `paavo-cli new` — scaffold a test crate from one of the templates
//! shipped under `templates/` in the paavo repo. Thin wrapper around
//! `cargo generate`; see spec §10.5 for the behaviour contract.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Exit code 2: cargo-generate not on PATH. Distinct from 1 (generic
/// failure) so wrapper scripts can detect "needs install" vs "real bug".
pub const EXIT_MISSING_CARGO_GENERATE: i32 = 2;

/// Arguments to `paavo-cli new`. Spec §10.5.
pub struct NewArgs {
    /// Crate name to scaffold (becomes both the directory and the
    /// `[package].name` in the generated `Cargo.toml`). Must be valid
    /// kebab-case — see [`validate_kebab_name`] for why.
    pub crate_name: String,
    /// Board kind (e.g. `mcxa266`). Must match a directory under
    /// `<templates>/<board-kind>/` containing a `cargo-generate.toml`.
    pub board_kind: String,
    /// Test kind: `quick` or `soak`. Spec §10.1.
    pub kind: String,
    /// Destination directory; the scaffolded crate lands at
    /// `<into>/<crate-name>/`.
    pub into: PathBuf,
    /// Explicit templates root. Overrides auto-discovery.
    pub templates_path: Option<PathBuf>,
}

/// Validate that a crate name is in kebab-case (lowercase letters,
/// digits, and ASCII hyphens; must start with a letter; must not end
/// with a hyphen; no consecutive hyphens).
///
/// We refuse non-kebab names rather than auto-convert them because
/// cargo-generate's silent kebab-conversion would make our post-success
/// `cd <name>` hint lie (cargo-generate would write `./my-test/`, but
/// we'd print `cd MyTest`). Explicit failure with a clear remediation
/// is friendlier than that.
fn validate_kebab_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("crate name is empty");
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        bail!(
            "crate name must start with a lowercase letter (got {first:?} in {name:?}). \
             Use kebab-case: a-z, 0-9, hyphens only — e.g. `hello-mcxa266`."
        );
    }
    if name.ends_with('-') {
        bail!("crate name must not end with a hyphen (got {name:?})");
    }
    let mut last_was_hyphen = false;
    for c in name.chars() {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
        if !ok {
            bail!(
                "crate name must be kebab-case (a-z, 0-9, hyphens only). \
                 Invalid character {c:?} in {name:?}. \
                 cargo-generate would silently rename this; paavo-cli refuses \
                 to so the `cd <name>` hint in the success message stays accurate."
            );
        }
        if c == '-' && last_was_hyphen {
            bail!("crate name must not contain consecutive hyphens (got {name:?})");
        }
        last_was_hyphen = c == '-';
    }
    Ok(())
}

/// Run the `new` verb. Returns a process exit code (not a Result-as-exit
/// — Ok(non-zero) is a clean reportable failure; Err is an unexpected
/// internal error).
pub fn run(args: NewArgs) -> Result<i32> {
    // 0. Validate the crate name BEFORE we touch the filesystem or
    //    shell out. See validate_kebab_name for the rationale.
    validate_kebab_name(&args.crate_name)
        .with_context(|| format!("validating --name {:?}", args.crate_name))?;

    // 1. Resolve templates root.
    let templates_root = resolve_templates_root(args.templates_path.as_deref())
        .context("resolving templates root")?;

    // 2. Check the requested board kind exists.
    let template_dir = templates_root.join(&args.board_kind);
    if !template_dir.join("cargo-generate.toml").is_file() {
        let kinds = list_available_kinds(&templates_root);
        bail!(
            "unknown board kind: {}. Available: {}",
            args.board_kind,
            kinds.join(", ")
        );
    }

    // 3. Check cargo-generate is on PATH. Use the `cargo-generate`
    //    binary directly (rather than `cargo generate`) to avoid the
    //    cargo-proxy hop and to get a clean exit code on missing
    //    binary instead of cargo's generic "no such subcommand" path.
    let cg_check = Command::new("cargo-generate").arg("--version").output();
    let cg_ok = match &cg_check {
        Ok(o) => o.status.success(),
        Err(_) => false,
    };
    if !cg_ok {
        eprintln!(
            "cargo-generate not found on PATH. \
             Install with: cargo install cargo-generate"
        );
        return Ok(EXIT_MISSING_CARGO_GENERATE);
    }

    // 4. Build the cargo-generate invocation. `--vcs none` keeps the
    //    scaffolded crate from initialising its own git repo inside our
    //    `.git` (or wherever the user is generating from). `--silent`
    //    suppresses the interactive prompts — every placeholder in our
    //    templates either has a default in `cargo-generate.toml` or is
    //    supplied via `--define` below, so we never need a TTY.
    let mut cg = Command::new("cargo-generate");
    cg.arg("generate")
        .arg("--path")
        .arg(&template_dir)
        .arg("--name")
        .arg(&args.crate_name)
        .arg("--destination")
        .arg(&args.into)
        .arg("--define")
        .arg(format!("test-kind={}", args.kind))
        .arg("--vcs")
        .arg("none")
        .arg("--silent");

    let status = cg.status().context("invoking cargo-generate")?;
    if !status.success() {
        bail!("cargo-generate exited non-zero: {status}");
    }

    let scaffolded = args.into.join(&args.crate_name);
    println!(
        "\nScaffolded {} at {}.\nNext: cd {} && cargo build --release && paavo-cli run -p .",
        args.crate_name,
        scaffolded.display(),
        args.crate_name
    );
    Ok(0)
}

/// Find the templates directory.
///
/// If `--templates-path` was passed, use it verbatim (after existence
/// check). Otherwise walk ancestors of the current working directory
/// looking for a paavo-repo signature: a directory that contains
/// `templates/` AND `Cargo.toml` AND `crates/`. That triplet is
/// specific enough that we'll never false-positive on a random parent.
fn resolve_templates_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if !p.is_dir() {
            bail!("--templates-path does not exist: {}", p.display());
        }
        return Ok(p.to_path_buf());
    }
    let cwd = std::env::current_dir()?;
    for ancestor in cwd.ancestors() {
        let templates = ancestor.join("templates");
        if templates.is_dir()
            && ancestor.join("Cargo.toml").is_file()
            && ancestor.join("crates").is_dir()
        {
            return Ok(templates);
        }
    }
    bail!(
        "cannot find a `templates/` directory; pass --templates-path \
         or run from inside a paavo checkout"
    );
}

/// Enumerate the board kinds under `<templates>/` by looking for
/// subdirectories that contain a `cargo-generate.toml`.
fn list_available_kinds(root: &Path) -> Vec<String> {
    let mut kinds = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            if e.path().join("cargo-generate.toml").is_file() {
                if let Some(name) = e.file_name().to_str() {
                    kinds.push(name.to_string());
                }
            }
        }
    }
    kinds.sort();
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_kebab_name_accepts_canonical_examples() {
        // These are the shapes we promise to accept in the README and
        // in the post-success "next step" hint.
        for name in [
            "hello",
            "hello-world",
            "hello-mcxa266",
            "smoke-test",
            "a",                                     // shortest valid name
            "x1",                                    // letter + digit
            "x1-y2",                                 // segments with digits
            "abcdefghijklmnopqrstuvwxyz0123456789-", // would-be valid if not for trailing hyphen
        ]
        .iter()
        .filter(|s| !s.ends_with('-'))
        {
            assert!(validate_kebab_name(name).is_ok(), "should accept {name:?}");
        }
    }

    #[test]
    fn validate_kebab_name_rejects_uppercase() {
        // The headline bug: cargo-generate would kebab `MyTest` to
        // `my-test` silently, then our "cd MyTest" hint would lie.
        let err = validate_kebab_name("MyTest").unwrap_err().to_string();
        assert!(err.contains("lowercase letter"), "msg = {err}");
        assert!(err.contains("MyTest"), "msg = {err}");
    }

    #[test]
    fn validate_kebab_name_rejects_underscores_and_spaces() {
        for bad in ["my_test", "my test", "my.test", "my/test"] {
            assert!(validate_kebab_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn validate_kebab_name_rejects_leading_digit_or_hyphen() {
        for bad in ["1abc", "-abc"] {
            assert!(validate_kebab_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn validate_kebab_name_rejects_trailing_hyphen() {
        assert!(validate_kebab_name("abc-").is_err());
    }

    #[test]
    fn validate_kebab_name_rejects_consecutive_hyphens() {
        assert!(validate_kebab_name("a--b").is_err());
    }

    #[test]
    fn validate_kebab_name_rejects_empty() {
        assert!(validate_kebab_name("").is_err());
    }
}

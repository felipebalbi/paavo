//! `paavo-cli new` — scaffold a test crate from a template tree that is
//! either a local directory or a remote git repository. Thin wrapper
//! around `cargo generate`.
//!
//! See docs/superpowers/specs/2026-06-29-cli-new-remote-templates-design.md
//! for the behaviour contract. The pure core (`classify_source`,
//! `build_cg_args`) is unit-tested with no network and no hardware; the
//! side-effecting `run` keeps validation, the cargo-generate presence
//! check, and the spawn.

use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Exit code 2: cargo-generate not on PATH. Distinct from 1 (generic
/// failure) so wrapper scripts can detect "needs install" vs "real bug".
pub const EXIT_MISSING_CARGO_GENERATE: i32 = 2;

/// Default template source when neither `--templates` nor the
/// `PAAVO_TEMPLATES` env var is given. A git URL, so scaffolding works
/// from anywhere with no local checkout. Referenced from `cli.rs` as the
/// clap `default_value` for `--templates`.
pub const DEFAULT_TEMPLATES_URL: &str = "https://github.com/felipebalbi/paavo";

/// Arguments to `paavo-cli new`.
pub struct NewArgs {
    /// Crate name to scaffold (becomes both the directory and the
    /// `[package].name`). Must be valid kebab-case — see
    /// [`validate_kebab_name`].
    pub crate_name: String,
    /// Board kind (e.g. `mcxa266`). Must match a directory under
    /// `<templates>/<subdir>/<board-kind>/` containing a
    /// `cargo-generate.toml`.
    pub board_kind: String,
    /// Test kind: `quick` or `soak`.
    pub kind: String,
    /// Destination directory; the scaffolded crate lands at
    /// `<into>/<crate-name>/`.
    pub into: PathBuf,
    /// Template tree root: a git URL or a local path (auto-detected).
    pub templates: String,
    /// Subdirectory within the root that holds the board-kind folders.
    pub templates_subdir: String,
    /// Optional git ref (tag or commit SHA) to pin a URL source to.
    pub templates_rev: Option<String>,
}

/// Where the template tree is pulled from.
#[derive(Debug, PartialEq, Eq)]
pub enum TemplateSource {
    /// A local directory tree root.
    Local(PathBuf),
    /// A git repository cloned by cargo-generate.
    Git {
        /// Clone URL.
        url: String,
        /// Optional pinned revision (a tag or commit SHA).
        rev: Option<String>,
    },
}

/// Classify a `--templates` value as a git URL or a local path.
///
/// Treated as a git URL iff (case-insensitive) it starts with
/// `http://`, `https://`, `git://`, `ssh://`, is scp-like
/// (`user@host:path`), or ends with `.git`. Otherwise it is a local
/// path. We deliberately do NOT honor cargo-generate's bare `owner/repo`
/// shorthand — it is ambiguous with a relative path. A `rev` is carried
/// only on the `Git` variant; for a local source it is dropped here and
/// the caller warns.
pub fn classify_source(src: &str, rev: Option<String>) -> TemplateSource {
    if looks_like_git_url(src) {
        TemplateSource::Git {
            url: src.to_string(),
            rev,
        }
    } else {
        TemplateSource::Local(PathBuf::from(src))
    }
}

/// Heuristic for "this is a git URL, not a local path".
fn looks_like_git_url(src: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    const SCHEMES: [&str; 4] = ["http://", "https://", "git://", "ssh://"];
    if SCHEMES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    if lower.ends_with(".git") {
        return true;
    }
    // scp-like `user@host:path` with no scheme. Require an '@' followed
    // later by a ':' so a Windows drive path like `C:\x` (which has no
    // '@') stays Local.
    if let Some(at) = src.find('@') {
        if src[at + 1..].contains(':') {
            return true;
        }
    }
    false
}

/// `<root>/<subdir>` with an empty / "." / slash-only subdir collapsing
/// to just `<root>`. Used by both the local template-dir resolver and
/// the available-kinds lister.
fn subdir_base(root: &Path, subdir: &str) -> PathBuf {
    let trimmed = subdir.trim_matches(|c| c == '/' || c == '\\');
    if trimmed.is_empty() || trimmed == "." {
        root.to_path_buf()
    } else {
        root.join(trimmed)
    }
}

/// The local directory expected to hold `<board_kind>`'s
/// `cargo-generate.toml`: `<root>/<subdir>/<board_kind>`.
fn local_template_dir(root: &Path, subdir: &str, board_kind: &str) -> PathBuf {
    subdir_base(root, subdir).join(board_kind)
}

/// The git SUBFOLDER positional (a path *inside the cloned repo*): always
/// forward-slashed, with an empty / "." subdir collapsing to the leaf.
fn git_subfolder(subdir: &str, board_kind: &str) -> String {
    let trimmed = subdir.trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        board_kind.to_string()
    } else {
        format!("{trimmed}/{board_kind}")
    }
}

/// Build the full cargo-generate argv (starting with `generate`). Pure:
/// no filesystem, no spawn. This is the unit-tested seam.
pub fn build_cg_args(
    source: &TemplateSource,
    subdir: &str,
    board_kind: &str,
    crate_name: &str,
    test_kind: &str,
    into: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["generate".into()];
    match source {
        TemplateSource::Local(root) => {
            args.push("--path".into());
            args.push(local_template_dir(root, subdir, board_kind).into_os_string());
        }
        TemplateSource::Git { url, rev } => {
            args.push("--git".into());
            args.push(OsString::from(url.as_str()));
            args.push(OsString::from(git_subfolder(subdir, board_kind)));
            if let Some(rev) = rev {
                args.push("--revision".into());
                args.push(OsString::from(rev.as_str()));
            }
        }
    }
    args.push("--name".into());
    args.push(OsString::from(crate_name));
    args.push("--destination".into());
    args.push(into.as_os_str().to_os_string());
    args.push("--define".into());
    args.push(OsString::from(format!("test-kind={test_kind}")));
    args.push("--vcs".into());
    args.push("none".into());
    args.push("--silent".into());
    args
}

/// Validate that a crate name is in kebab-case (lowercase letters,
/// digits, and ASCII hyphens; must start with a letter; must not end
/// with a hyphen; no consecutive hyphens).
///
/// We refuse non-kebab names rather than auto-convert them because
/// cargo-generate's silent kebab-conversion would make our post-success
/// `cd <name>` hint lie.
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

/// Run the `new` verb. Returns a process exit code (Ok(non-zero) is a
/// clean reportable failure; Err is an unexpected internal error).
pub fn run(args: NewArgs) -> Result<i32> {
    // 0. Validate the crate name BEFORE touching the filesystem/network.
    validate_kebab_name(&args.crate_name)
        .with_context(|| format!("validating --name {:?}", args.crate_name))?;

    // 1. Classify the source. URL → defer all validation to
    //    cargo-generate; Local → rich pre-flight below.
    let source = classify_source(&args.templates, args.templates_rev.clone());

    // 2. Local-only pre-flight: existence + board-kind list.
    if let TemplateSource::Local(root) = &source {
        if !root.is_dir() {
            bail!("templates source does not exist: {}", root.display());
        }
        let template_dir = local_template_dir(root, &args.templates_subdir, &args.board_kind);
        if !template_dir.join("cargo-generate.toml").is_file() {
            let kinds = list_available_kinds(root, &args.templates_subdir);
            bail!(
                "unknown board kind: {}. Available: {}",
                args.board_kind,
                kinds.join(", ")
            );
        }
    }

    // 3. A git ref is meaningless for a local copy — warn and ignore.
    if matches!(&source, TemplateSource::Local(_)) && args.templates_rev.is_some() {
        tracing::warn!("--templates-rev is ignored for a local templates source");
    }

    // 4. Check cargo-generate is on PATH.
    let cg_check = Command::new("cargo-generate").arg("--version").output();
    let cg_ok = matches!(&cg_check, Ok(o) if o.status.success());
    if !cg_ok {
        eprintln!(
            "cargo-generate not found on PATH. \
             Install with: cargo install cargo-generate"
        );
        return Ok(EXIT_MISSING_CARGO_GENERATE);
    }

    // 5. Build and run the cargo-generate invocation. For a Git source
    //    a bad repo / subfolder / ref surfaces here as a non-zero exit.
    let argv = build_cg_args(
        &source,
        &args.templates_subdir,
        &args.board_kind,
        &args.crate_name,
        &args.kind,
        &args.into,
    );
    let status = Command::new("cargo-generate")
        .args(&argv)
        .status()
        .context("invoking cargo-generate")?;
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

/// Enumerate the board kinds under `<root>/<subdir>/` by looking for
/// subdirectories that contain a `cargo-generate.toml`.
fn list_available_kinds(root: &Path, subdir: &str) -> Vec<String> {
    let base = subdir_base(root, subdir);
    let mut kinds = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&base) {
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

    fn as_strings(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn classify_source_detects_git_urls() {
        for s in [
            "https://github.com/x/y",
            "http://h/r",
            "git://h/r",
            "ssh://h/r",
            "git@github.com:x/y",
            "git@github.com:x/y.git",
            "https://h/r.git",
            "HTTPS://H/R",
        ] {
            assert!(
                matches!(classify_source(s, None), TemplateSource::Git { .. }),
                "should classify as git: {s:?}"
            );
        }
    }

    #[test]
    fn classify_source_detects_local_paths() {
        for s in [".", "/abs/path", "rel/path", "..", r"C:\win\path"] {
            assert!(
                matches!(classify_source(s, None), TemplateSource::Local(_)),
                "should classify as local: {s:?}"
            );
        }
    }

    #[test]
    fn classify_source_keeps_rev_only_for_git() {
        match classify_source("https://h/r", Some("v1".into())) {
            TemplateSource::Git { rev, .. } => assert_eq!(rev.as_deref(), Some("v1")),
            other => panic!("expected git, got {other:?}"),
        }
        // Local drops the rev (caller warns).
        assert_eq!(
            classify_source("/x", Some("v1".into())),
            TemplateSource::Local(PathBuf::from("/x"))
        );
    }

    #[test]
    fn build_cg_args_local_uses_path_and_tail() {
        let src = TemplateSource::Local(PathBuf::from("/root"));
        let s = as_strings(&build_cg_args(
            &src,
            "templates",
            "mcxa266",
            "hello",
            "quick",
            Path::new("/out"),
        ));
        assert_eq!(s[0], "generate");
        assert!(s.contains(&"--path".to_string()));
        assert!(!s.contains(&"--git".to_string()));
        let idx = s.iter().position(|x| x == "--path").unwrap();
        assert!(
            s[idx + 1]
                .replace('\\', "/")
                .ends_with("/root/templates/mcxa266"),
            "path was {:?}",
            s[idx + 1]
        );
        // Invariant tail.
        assert!(s.contains(&"--name".to_string()) && s.contains(&"hello".to_string()));
        assert!(s.contains(&"--destination".to_string()));
        assert!(s.contains(&"--define".to_string()) && s.contains(&"test-kind=quick".to_string()));
        assert!(s.contains(&"--vcs".to_string()) && s.contains(&"none".to_string()));
        assert!(s.contains(&"--silent".to_string()));
    }

    #[test]
    fn build_cg_args_git_uses_git_and_subfolder() {
        let src = TemplateSource::Git {
            url: "https://h/r".into(),
            rev: None,
        };
        let s = as_strings(&build_cg_args(
            &src,
            "templates",
            "mcxa266",
            "hello",
            "soak",
            Path::new("/out"),
        ));
        let gi = s.iter().position(|x| x == "--git").unwrap();
        assert_eq!(s[gi + 1], "https://h/r");
        assert_eq!(s[gi + 2], "templates/mcxa266");
        assert!(!s.contains(&"--revision".to_string()));
        assert!(s.contains(&"test-kind=soak".to_string()));
    }

    #[test]
    fn build_cg_args_git_includes_revision_when_set() {
        let src = TemplateSource::Git {
            url: "https://h/r".into(),
            rev: Some("v1.2.0".into()),
        };
        let s = as_strings(&build_cg_args(
            &src,
            "templates",
            "mcxa266",
            "hello",
            "quick",
            Path::new("/out"),
        ));
        let ri = s.iter().position(|x| x == "--revision").unwrap();
        assert_eq!(s[ri + 1], "v1.2.0");
    }

    #[test]
    fn build_cg_args_collapses_dot_subdir() {
        let g = as_strings(&build_cg_args(
            &TemplateSource::Git {
                url: "u".into(),
                rev: None,
            },
            ".",
            "mcxa266",
            "h",
            "quick",
            Path::new("/o"),
        ));
        let gi = g.iter().position(|x| x == "--git").unwrap();
        assert_eq!(g[gi + 2], "mcxa266");

        let l = as_strings(&build_cg_args(
            &TemplateSource::Local(PathBuf::from("/root")),
            ".",
            "mcxa266",
            "h",
            "quick",
            Path::new("/o"),
        ));
        let li = l.iter().position(|x| x == "--path").unwrap();
        assert!(l[li + 1].replace('\\', "/").ends_with("/root/mcxa266"));
    }

    #[test]
    fn validate_kebab_name_accepts_canonical_examples() {
        for name in [
            "hello",
            "hello-world",
            "hello-mcxa266",
            "smoke-test",
            "a",
            "x1",
            "x1-y2",
        ] {
            assert!(validate_kebab_name(name).is_ok(), "should accept {name:?}");
        }
    }

    #[test]
    fn validate_kebab_name_rejects_uppercase() {
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

# `paavo-cli new` Remote/Local Templates — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `paavo-cli new` scaffold a test crate from a git URL *or* a local path, defaulting to a baked-in canonical URL so no checkout is needed.

**Architecture:** A pure core in `cmd_new.rs` — `classify_source` (URL vs path) and `build_cg_args` (exact cargo-generate argv) — fully unit-tested with no network/hardware, behind a thin side-effecting `run()` that keeps validation, the cargo-generate presence check, and the spawn. The CLI gains `--templates` (with `PAAVO_TEMPLATES` env + `--templates-path` alias), `--templates-subdir`, and `--templates-rev`. The old walk-up auto-discovery is removed.

**Tech Stack:** Rust 1.95.0, clap 4 (derive + env), cargo-generate 0.23 (external binary), `assert_cmd`/`predicates`/`tempfile`/`which` for tests, `tracing` for the ignored-rev warning.

**Reference spec:** `docs/superpowers/specs/2026-06-29-cli-new-remote-templates-design.md`

---

## File structure

| File | Responsibility | Change |
|------|----------------|--------|
| `crates/paavo-cli/src/cmd_new.rs` | Source classification, argv construction, `run()` side effects, unit tests | Rewrite |
| `crates/paavo-cli/src/cli.rs` | `New` subcommand flag surface | Modify the `New { … }` variant |
| `crates/paavo-cli/src/main.rs` | Thread `New` flags into `NewArgs` | Modify the `Cmd::New` arm |
| `crates/paavo-cli/tests/cli_new.rs` | Integration tests for `new` | Modify 2 tests + add a helper |
| `README.md` | User-facing docs for `new` templates | Add a subsection |

> **Why one big code task:** the new `TemplateSource::Git` variant must be *constructed in non-test code* the moment it exists, or `cargo clippy --all-targets -D warnings` fails with `variant never constructed`. So the enum, `run()` rewrite, and CLI flags land together in Task 1. Docs (Task 2) and final verification (Task 3) follow.

---

## Task 1: Templates can be a URL or a local path

**Files:**
- Modify: `crates/paavo-cli/src/cli.rs` (the `New { … }` variant, ~lines 59-77)
- Modify: `crates/paavo-cli/src/main.rs` (the `cli::Cmd::New { … }` arm, ~lines 49-74)
- Rewrite: `crates/paavo-cli/src/cmd_new.rs`
- Modify: `crates/paavo-cli/tests/cli_new.rs`

Intermediate states between steps will not compile (the three source files change in lockstep); that is expected. Verify and commit only at the end.

- [ ] **Step 1: Update the CLI flag surface in `cli.rs`**

Replace the `New { … }` variant (currently lines 59-77) with this. Note the `templates_path` field is gone, replaced by `templates` (which keeps `--templates-path` as a visible alias):

```rust
    /// Scaffold a new test crate via cargo-generate templates.
    New {
        /// Crate name to create.
        name: String,
        /// Required board kind.
        #[arg(long)]
        board_kind: String,
        /// quick / soak.
        #[arg(long, default_value = "quick")]
        kind: TestKindArg,
        /// Destination directory; the scaffolded crate lands at
        /// `<into>/<name>/`. Defaults to the current working directory.
        #[arg(long)]
        into: Option<PathBuf>,
        /// Template tree root: a git URL or a local directory
        /// (auto-detected). Defaults to the canonical paavo repo, so no
        /// checkout is needed. Override with the `PAAVO_TEMPLATES` env
        /// var. The legacy `--templates-path` is an alias.
        #[arg(
            long,
            visible_alias = "templates-path",
            env = "PAAVO_TEMPLATES",
            default_value = crate::cmd_new::DEFAULT_TEMPLATES_URL
        )]
        templates: String,
        /// Subdirectory within the templates root that holds the
        /// board-kind folders. Use "." when the root is itself the
        /// templates directory.
        #[arg(long, default_value = "templates")]
        templates_subdir: String,
        /// Pin a URL templates source to a git ref (a tag or commit
        /// SHA). Ignored, with a warning, for local sources.
        #[arg(long)]
        templates_rev: Option<String>,
    },
```

- [ ] **Step 2: Thread the new flags through `main.rs`**

Replace the `cli::Cmd::New { … } => { … }` arm (currently lines 49-74) with:

```rust
        cli::Cmd::New {
            name,
            board_kind,
            kind,
            into,
            templates,
            templates_subdir,
            templates_rev,
        } => {
            let kind_str = match kind {
                cli::TestKindArg::Quick => "quick",
                cli::TestKindArg::Soak => "soak",
            }
            .to_string();
            let into = match into {
                Some(p) => p,
                None => std::env::current_dir()
                    .context("resolving current directory for default --into")?,
            };
            let code = cmd_new::run(cmd_new::NewArgs {
                crate_name: name,
                board_kind,
                kind: kind_str,
                into,
                templates,
                templates_subdir,
                templates_rev,
            })?;
            std::process::exit(code);
        }
```

- [ ] **Step 3: Rewrite `cmd_new.rs` (core + `run` + unit tests)**

Replace the entire contents of `crates/paavo-cli/src/cmd_new.rs` with the following. Keep the existing `validate_kebab_name` function and its tests verbatim (shown here in full so the file is complete):

```rust
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
            s[idx + 1].replace('\\', "/").ends_with("/root/templates/mcxa266"),
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
```

- [ ] **Step 4: Update the integration tests in `tests/cli_new.rs`**

Add a helper and update the two tests that relied on the old local default. The missing-cargo-generate test stays as-is (the default URL source skips the local pre-flight and still hits the presence check → exit 2 with no network). Insert this helper after the `path_without_cargo_generate` function:

```rust
/// The workspace root, discovered by walking up from the test's CWD
/// until we find `templates/mcxa266/cargo-generate.toml`. Used to point
/// `new` at the in-repo templates as a LOCAL source, so these tests are
/// deterministic and never touch the network.
fn workspace_root() -> std::path::PathBuf {
    std::env::current_dir()
        .unwrap()
        .ancestors()
        .find(|p| p.join("templates/mcxa266/cargo-generate.toml").exists())
        .expect("templates/mcxa266 not found from any ancestor of CWD")
        .to_path_buf()
}
```

Replace `new_with_unknown_board_kind_errors_with_kinds_list` with (adds an explicit local `--templates <root>`, since the default is now a URL that would otherwise be deferred to cargo-generate):

```rust
#[test]
fn new_with_unknown_board_kind_errors_with_kinds_list() {
    // Point at the in-repo templates as a LOCAL source so the rich
    // pre-flight (existence + "Available:" list) runs. With the default
    // URL source this validation is deferred to cargo-generate instead.
    let root = workspace_root();
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["new", "hello", "--board-kind", "bogus-xyz", "--templates"])
        .arg(&root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown board kind: bogus-xyz"))
        .stderr(predicate::str::contains("mcxa266"));
}
```

Replace the body of the PAAVO_HW-gated `new_mcxa266_scaffolds_and_typechecks` so it scaffolds from the local in-repo templates (no network even under `PAAVO_HW=1`):

```rust
#[test]
#[ignore] // gated under PAAVO_HW=1 because it does a real cargo-generate +
          // real cargo check against thumbv8m.main-none-eabihf, which is
          // slow and requires the target to be installed.
fn new_mcxa266_scaffolds_and_typechecks() {
    if std::env::var("PAAVO_HW").is_err() {
        eprintln!("PAAVO_HW not set; skipping");
        return;
    }
    let root = workspace_root();
    let tmp = tempfile::tempdir().expect("tempdir");
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args([
            "new",
            "smoke-test",
            "--board-kind",
            "mcxa266",
            "--into",
            tmp.path().to_str().unwrap(),
            "--templates",
        ])
        .arg(&root)
        .assert()
        .success();

    let scaffolded = tmp.path().join("smoke-test");
    assert!(
        scaffolded.join("Cargo.toml").is_file(),
        "Cargo.toml missing"
    );
    assert!(scaffolded.join("src/main.rs").is_file(), "main.rs missing");
    assert!(scaffolded.join("memory.x").is_file(), "memory.x missing");

    let out = std::process::Command::new("cargo")
        .args(["check", "--target", "thumbv8m.main-none-eabihf"])
        .current_dir(&scaffolded)
        .output()
        .expect("spawn cargo check");
    assert!(
        out.status.success(),
        "cargo check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
```

- [ ] **Step 5: Build and run the paavo-cli tests**

Run: `cargo test -p paavo-cli`
Expected: PASS. The new unit tests (`classify_source_*`, `build_cg_args_*`) and the existing `validate_kebab_name_*` pass; the integration tests `new_without_cargo_generate_errors_clearly`, `new_with_unknown_board_kind_errors_with_kinds_list`, and `new_with_non_kebab_name_errors_before_touching_filesystem` pass. The `#[ignore]`d HW test is skipped.

- [ ] **Step 6: Format and lint**

Run: `cargo fmt --all`
Then: `cargo fmt --all -- --check`
Expected: no diff.

Run: `cargo clippy -p paavo-cli --all-targets -- -D warnings`
Expected: no warnings. (In particular, no `variant never constructed` for `TemplateSource::Git` — `run()` constructs it for the default URL source.)

- [ ] **Step 7: Commit**

```bash
git add crates/paavo-cli/src/cli.rs crates/paavo-cli/src/main.rs crates/paavo-cli/src/cmd_new.rs crates/paavo-cli/tests/cli_new.rs
git commit -m "feat(paavo-cli): new scaffolds from a git URL or local path"
```

---

## Task 2: Document the templates sources in the README

**Files:**
- Modify: `README.md` (insert a subsection after the `cli.toml` block, before `## Scheduled runs`)

- [ ] **Step 1: Add the templates subsection**

In `README.md`, immediately after the closing ```` ``` ```` of the `cli.toml` example (line 61) and before `## Scheduled runs` (line 63), insert:

```markdown

### Templates for `paavo-cli new`

`paavo-cli new` scaffolds a test crate from a template tree. The source can
be a git URL or a local directory and is auto-detected. It resolves in this
order:

1. `--templates <url-or-path>` flag
2. `PAAVO_TEMPLATES` environment variable
3. Default: `https://github.com/felipebalbi/paavo` (the canonical repo)

So the quick-start one-liner works with no checkout — `new` clones the
templates for you. The template for a board kind is read from
`<source>/<subdir>/<board-kind>/`, where `<subdir>` defaults to `templates`.

```bash
# Default: clone the canonical repo (no checkout needed).
paavo-cli new my-dma-test --board-kind mcxa266

# Working inside a paavo checkout, against your local template edits:
paavo-cli new my-dma-test --board-kind mcxa266 --templates .

# A fork, pinned to a release tag (a tag or commit SHA):
paavo-cli new my-dma-test --board-kind mcxa266 \
    --templates https://github.com/acme/paavo-fork --templates-rev v1.2.0
```

The legacy `--templates-path` flag still works as an alias for `--templates`,
but now names the tree *root* (use `--templates-subdir .` if it points
directly at a templates directory).
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs(readme): document paavo-cli new template sources"
```

---

## Task 3: Full-workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Run the exact CI gate across the whole workspace**

Run: `cargo fmt --all -- --check`
Expected: no diff.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

Run: `cargo test --workspace`
Expected: PASS. (The `templates_mcxa266_smoke.rs` tests are unaffected — they read the template files textually and never invoke `new`.)

- [ ] **Step 2: (Optional, manual) sanity-check the CLI surface**

Run: `cargo run -p paavo-cli -- new --help`
Expected: help shows `--templates` (with alias `--templates-path`), `--templates-subdir`, `--templates-rev`, and `[env: PAAVO_TEMPLATES=]` on `--templates`.

Run: `cargo run -p paavo-cli -- new my-test --board-kind bogus --templates .`
Expected (from a repo checkout): `unknown board kind: bogus. Available: mcxa266, rt685-evk` (exact kinds depend on `templates/`).

---

## Self-review notes

- **Spec coverage:** `--templates` default URL + env + alias (Task 1 Step 1, Task 2); `--templates-subdir` (Step 1 + `subdir_base`/`git_subfolder`); `--templates-rev` → `--revision` with local-warn (Step 3 `build_cg_args`/`run`); URL validation deferred to cargo-generate (Step 3 `run`); local "Available kinds" preserved (Step 3 `list_available_kinds`); walk-up removed (no `resolve_templates_root` in the rewrite); pure unit-tested core (Step 3 tests); README docs (Task 2); CI gate (Task 3).
- **Type consistency:** `TemplateSource`, `classify_source(&str, Option<String>)`, `build_cg_args(&TemplateSource, &str, &str, &str, &str, &Path) -> Vec<OsString>`, `subdir_base`, `local_template_dir`, `git_subfolder`, `list_available_kinds(&Path, &str)`, and `NewArgs { crate_name, board_kind, kind, into, templates, templates_subdir, templates_rev }` are used identically in `cli.rs`, `main.rs`, `cmd_new.rs`, and the tests.
- **No placeholders:** every code block is complete and compilable.
```

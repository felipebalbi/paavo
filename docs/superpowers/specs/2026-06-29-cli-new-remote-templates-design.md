# `paavo-cli new`: scaffold from a remote URL or a local path (no checkout required)

**Date:** 2026-06-29
**Status:** Approved (design)
**Crates:** `paavo-cli` (only)

## Problem

`paavo-cli new` can only find its templates inside a **local paavo checkout**.
`resolve_templates_root` (`crates/paavo-cli/src/cmd_new.rs:156`) either takes an
explicit `--templates-path` or walks up the CWD ancestors looking for a paavo
repo signature (`templates/` + `Cargo.toml` + `crates/`):

```rust
// crates/paavo-cli/src/cmd_new.rs (today)
for ancestor in cwd.ancestors() {
    let templates = ancestor.join("templates");
    if templates.is_dir()
        && ancestor.join("Cargo.toml").is_file()
        && ancestor.join("crates").is_dir()
    { return Ok(templates); }
}
bail!("cannot find a `templates/` directory; pass --templates-path or run from inside a paavo checkout");
```

That means a user who installed the CLI with `cargo install --git … paavo-cli`
**still has to clone the whole paavo repo** just to scaffold a test crate. It
also makes the README's headline one-liner a lie: `README.md:14` promises that
right after `cargo install`, `paavo-cli new my-dma-test --board-kind mcxa266`
just works — but today it errors out unless you happen to be standing in a
checkout.

cargo-generate — which `new` already shells out to — natively supports cloning
a template from a git URL (`--git <URL> [SUBFOLDER]`). We are simply not using
that path. This change closes the gap: **templates can be a local path or a git
URL, and the default needs no clone at all.**

## Goals

- Let `paavo-cli new` scaffold from a **git URL** *or* a **local directory**,
  auto-detected from a single `--templates <SRC>` flag.
- **Default to a compiled-in canonical URL** (`https://github.com/felipebalbi/paavo`)
  so the README one-liner works from anywhere with no checkout.
- Let advanced users **pin a git ref** (tag or commit SHA) via `--templates-rev`.
- Let users point at any in-repo layout via `--templates-subdir` (default
  `templates`).
- Keep the rich local-path errors (existence check + "Available kinds:" list).
- Factor source resolution and argv construction into **pure, fully
  unit-tested functions** (no network, no hardware in the logic under test).

## Non-goals (YAGNI)

- **No shallow-clone pre-validation of URLs.** For a URL source we do not probe
  the repo before scaffolding; cargo-generate clones and fails if the board
  kind / subfolder / ref does not resolve, and we surface its error verbatim.
- **No `owner/repo` shorthand.** cargo-generate's bare `owner/repo` form is
  ambiguous with a relative path; we require a real URL or an explicit path.
  (Corollary edge case: a local directory literally named `*.git` would be
  misclassified as a URL. This is accepted as a vanishingly rare case rather
  than special-cased.)
- **No config-file setting** for the default source. A flag plus a
  `PAAVO_TEMPLATES` env var is enough (mirrors how `--host` / `PAAVO_HOST`
  already work).
- **No clone caching, retries, or offline mode.**
- **No daemon, DB, route, or wire changes.** `new` is a purely client-side,
  local command; `paavo-proto` and `paavod` are untouched.

## CLI surface

| Flag | Type | Default | Meaning |
|------|------|---------|---------|
| `--templates <SRC>` | string | `PAAVO_TEMPLATES` env, else the baked-in URL `https://github.com/felipebalbi/paavo` | Template **tree root**: a git URL **or** a local dir. Auto-detected. Carries clap `visible_alias = "templates-path"`. |
| `--templates-subdir <PATH>` | string | `templates` | Prefix inside the root where board-kind folders live. Pass `.` to mean "the root *is* the templates dir". |
| `--templates-rev <REF>` | string | none (→ default-branch HEAD) | Git ref to pin to (tag or commit SHA). Ignored (with a warning) for local sources. |

The resolved template location is always:

```
<SRC>/<subdir>/<board-kind>/        (must contain cargo-generate.toml)
```

Resolution precedence for the source: `--templates` flag → `PAAVO_TEMPLATES`
env → baked-in URL constant. (Implemented with clap's `env = "PAAVO_TEMPLATES"`
plus a `default_value` constant, so clap does the layering.)

Usage examples:

```bash
# Default: clone canonical repo, use templates/mcxa266 — no checkout needed.
paavo-cli new my-dma-test --board-kind mcxa266

# In-repo developer testing local template edits:
paavo-cli new my-dma-test --board-kind mcxa266 --templates .

# A fork, pinned to a release tag:
paavo-cli new my-dma-test --board-kind mcxa266 \
    --templates https://github.com/acme/paavo-fork --templates-rev v1.2.0

# A local templates dir that is itself the root (no `templates/` prefix):
paavo-cli new my-test --board-kind mcxa266 \
    --templates /opt/paavo-templates --templates-subdir .
```

## Design — Approach A: pure plan function + thin side-effecting shell

### Types and pure core

```rust
/// Where `paavo-cli new` pulls its template tree from.
enum TemplateSource {
    /// A local tree root on disk.
    Local(PathBuf),
    /// A git repository cloned by cargo-generate, optionally pinned.
    Git { url: String, rev: Option<String> },
}

/// Classify a `--templates` value as a git URL or a local path.
///
/// Git iff (case-insensitive) it starts with `http://`, `https://`,
/// `git://`, `ssh://`, matches scp-like `user@host:path`, or ends with
/// `.git`. Otherwise Local. We do NOT honor cargo-generate's bare
/// `owner/repo` shorthand — too ambiguous with a relative path.
fn classify_source(src: &str, rev: Option<String>) -> TemplateSource;

/// Build the exact cargo-generate argv. Pure: no FS, no spawn.
fn build_cg_args(
    source: &TemplateSource,
    subdir: &str,
    board_kind: &str,
    crate_name: &str,
    test_kind: &str,
    into: &Path,
) -> Vec<OsString>;
```

`build_cg_args` emits:

- `TemplateSource::Local(root)` →
  `generate --path <root>/<subdir>/<board_kind> <common…>`
- `TemplateSource::Git { url, rev }` →
  `generate --git <url> <subdir>/<board_kind> [--revision <rev>] <common…>`

where `<common…>` is the invariant tail, unchanged from today:
`--name <crate_name> --destination <into> --define test-kind=<kind> --vcs none --silent`.

The subfolder joining uses forward slashes for the git positional
(`<subdir>/<board_kind>`), because it is a path *inside the cloned repo* (git /
cargo-generate convention), independent of the host OS separator. The local
`--path` uses a real `PathBuf` join so it is correct on Windows.

**Subdir normalization.** When `--templates-subdir` is `.` or empty, the prefix
collapses so the leaf is joined directly: the git positional is just
`<board_kind>` (never `./<board_kind>`) and the local path is `<root>/<board_kind>`.
A non-trivial subdir may itself contain slashes (e.g. `crates/templates`); it is
joined verbatim ahead of `<board_kind>`. Trailing slashes are trimmed before
joining.

#### Git ref mapping

cargo-generate 0.23 exposes three ref flags: `--branch`, `--tag`, and
`--revision`; internally `--tag` and `--revision` share one checkout slot
(`tag_or_revision` in `clone_tool.rs`) while `--branch` is separate. We map the
single `--templates-rev <REF>` to **`--revision <REF>`**, which covers the
pinning use-cases the flag exists for (a release **tag** or a **commit SHA**).
Tracking an arbitrary non-default *branch* by name is intentionally out of
scope for this flag; users who need that can pass a tag/SHA or clone manually.
When the source is `Local`, a supplied `rev` is dropped with a `tracing::warn!`
(git refs are meaningless for a path copy) rather than being a hard error.

### Side-effecting shell (`run`)

`run()` keeps every side effect, in this order:

1. **Kebab name validation** — `validate_kebab_name` (unchanged), first, before
   any FS or network access.
2. **Classify** the source via `classify_source(&args.templates, args.rev)`.
3. **Local-only pre-flight** (skipped entirely for `Git`):
   - `<SRC>` must exist and be a directory, else
     `bail!("templates source does not exist: …")`.
   - `<SRC>/<subdir>/<board_kind>/cargo-generate.toml` must be a file, else
     `bail!("unknown board kind: {kind}. Available: {list}")` where the list
     comes from scanning subdirs of `<SRC>/<subdir>` (the existing
     `list_available_kinds`, repointed at the subdir).
4. **cargo-generate presence** — `cargo-generate --version` (unchanged); on
   failure print the install hint and return `EXIT_MISSING_CARGO_GENERATE` (2).
5. **Spawn** `cargo-generate` with `build_cg_args(...)`. For a `Git` source this
   is where a bad repo / subfolder / ref surfaces: we propagate cargo-generate's
   non-zero exit and stderr verbatim (`bail!("cargo-generate exited non-zero: {status}")`).
6. **Success hint** — unchanged `cd <name> && cargo build --release && paavo-cli run …`.

The walk-up auto-discovery (`resolve_templates_root`'s ancestor loop) is
**removed**; the default is always the baked-in URL.

## Validation & error handling summary

| Situation | Behavior |
|-----------|----------|
| Non-kebab `--name` | `bail!` before any FS/network (unchanged) |
| `cargo-generate` not on PATH | install hint, exit code 2 (unchanged) |
| Local `<SRC>` missing | `bail!("templates source does not exist: …")` |
| Local board kind missing | `bail!("unknown board kind: X. Available: …")` |
| Git repo / subfolder / ref bad | cargo-generate clones, fails; we surface its exit + stderr |
| `--templates-rev` on a local source | `tracing::warn!`, ignored, proceed |

## Backward compatibility & migration

- `--templates-path` keeps parsing (clap `visible_alias` on `--templates`) but
  now means **tree root**, not the templates dir itself. A script that passed
  `--templates-path /repo/templates` must repoint to `--templates /repo` **or**
  add `--templates-subdir .`. This is a deliberate, documented behavior change,
  acceptable pre-1.0.
- No wire/protocol/DB impact: `new` is local-only.

## Testing

Pure unit tests in `cmd_new.rs` (deterministic, no network, no hardware):

- `classify_source`:
  - Git: `https://…`, `http://…`, `git://…`, `ssh://…`, `git@github.com:o/r.git`,
    `…/foo.git`, and mixed-case schemes.
  - Local: `.`, `/abs/path`, `rel/path`, `..\\rel`, `C:\\win\\path`.
- `build_cg_args`:
  - Local → contains `--path` with the joined `<root>/<subdir>/<board_kind>`;
    contains no `--git`.
  - Git → contains `--git <url>` and the `<subdir>/<board_kind>` positional;
    contains `--revision <rev>` iff `rev` is `Some`.
  - Subdir `.` / empty → leaf is joined directly (git positional `<board_kind>`,
    local path `<root>/<board_kind>`), with no `./` segment.
  - Both → assert the invariant tail (`--name`, `--destination`,
    `--define test-kind=…`, `--vcs none`, `--silent`).
- Keep all existing kebab-name tests.
- Keep the existing missing-`cargo-generate` integration test
  (`tests/cli_new.rs`), which strips cargo-generate from PATH.
- Keep the existing `#[ignore]` / `PAAVO_HW=1` real-generate test; optionally
  add a sibling that runs the **local** path end-to-end against the repo's own
  `templates/` (no network), gated the same way.

The URL/clone path is **not** covered by an automated network test (no network
in CI); its argv construction is covered by `build_cg_args` unit tests, and the
clone itself is delegated to (and trusted from) cargo-generate.

## Docs to update (in the same change)

- `crates/paavo-cli/src/cli.rs` — `new` flag doc comments / help text.
- `crates/paavo-cli/src/cmd_new.rs` — module header (drop the "ships under
  `templates/`… must run from a checkout" framing).
- `README.md` — note that the line-14 one-liner now genuinely works post-install;
  document `--templates` / `--templates-subdir` / `--templates-rev` and the
  in-repo `--templates .` workflow.
- `AGENTS.md` — the Landmines/notes that describe `new` walking up for a checkout
  are now stale; update to the baked-in-URL default.

## Files touched

- `crates/paavo-cli/src/cli.rs` — replace `templates_path` with `templates`
  (+ `visible_alias`), add `templates_subdir`, `templates_rev`.
- `crates/paavo-cli/src/main.rs` — thread the three new fields into `NewArgs`.
- `crates/paavo-cli/src/cmd_new.rs` — `TemplateSource`, `classify_source`,
  `build_cg_args`, rewritten `run`, removed `resolve_templates_root` walk-up,
  repointed `list_available_kinds`, new unit tests.
- `README.md`, `AGENTS.md` — doc updates.

## Future work (out of scope here)

- Optional URL pre-validation (shallow clone / ref existence) for friendlier
  errors than cargo-generate's.
- A `--templates-branch` if branch-tracking (not just tag/SHA pinning) is ever
  needed.
- Pinning the default ref to the CLI's own release version for reproducibility.

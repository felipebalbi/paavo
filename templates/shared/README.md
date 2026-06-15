# Shared template assets

This directory is the **canonical source of record** for assets that
cargo-generate templates need to ship to scaffolded crates. Because
cargo-generate only copies files inside the chosen template directory,
any asset a per-kind template needs at scaffold-time has to live inside
that template directory too. To avoid drift, we keep canonical copies
here and duplicate them into each `templates/<kind>/` directory.

**Currently used by:** `templates/mcxa266/`, `templates/rt685-evk/`.

## Per-template duplicates (must stay in sync)

The following files are byte-identical between this directory and every
per-kind template. After any edit, copy the new version into every
template, then verify with PowerShell:

```pwsh
Get-FileHash templates/shared/link_ram_cortex_m.x, `
  templates/mcxa266/link_ram_cortex_m.x, `
  templates/rt685-evk/link_ram_cortex_m.x
# all three SHA256 values MUST match
```

| File | Why it's duplicated |
| --- | --- |
| `link_ram_cortex_m.x` | Vendored linker fragment from `embassy-rs/teleprobe` (see the attribution header in the file for commit + license). Wired into `OUT_DIR/link_ram.x` by each template's `build.rs` so the test binary runs from RAM. |
| `build.rs` | Identical script across templates: copies `link_ram_cortex_m.x` and `memory.x` into `OUT_DIR`, emits the right `cargo:rustc-link-arg` and `cargo:rerun-if-changed` directives. If you tweak this in one template (e.g. to add `--nmagic`), mirror to every template. |
| `.cargo/config.toml` | Pins `target = "thumbv8m.main-none-eabihf"` (correct for both mcxa266 and rt685-evk's Cortex-M33 cores) and the defmt rustflags. Both templates share the same Cortex-M33 target; if a future template lands on a different core, give it its own pinned target. |

## Refresh procedure

When `embassy-rs/teleprobe` updates `link_ram_cortex_m.x`:

1. Re-download into `templates/shared/link_ram_cortex_m.x` and update
   the attribution header SHA.
2. Copy the same file into every `templates/<kind>/link_ram_cortex_m.x`
   listed under "Currently used by" above.
3. Run the `Get-FileHash` verification block from the previous section.
4. Each template's `build.rs` then `include_str!`s its local copy and
   writes it to `OUT_DIR/link_ram.x`, paired with `-Tlink_ram.x` so the
   test binary runs from RAM (matching the upstream embassy convention).

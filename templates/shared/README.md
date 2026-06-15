# Shared template assets

- `link_ram_cortex_m.x` — vendored verbatim from `embassy-rs/teleprobe`
  (see the attribution header in the file for commit + license).
  This directory is the **canonical source**: each board-kind template
  ships its own copy under `templates/<kind>/link_ram_cortex_m.x` so
  that `cargo generate` produces a fully self-contained crate (cargo-
  generate only copies files inside the chosen template directory, so
  a sibling `templates/shared/` reference would not survive scaffolding).

  When refreshing from upstream:

  1. Re-download into `templates/shared/link_ram_cortex_m.x` and update
     the attribution header SHA.
  2. Copy the same file into every `templates/<kind>/link_ram_cortex_m.x`.
  3. Each template's `build.rs` then `include_str!`s its local copy and
     writes it to `OUT_DIR/link_ram.x`, paired with `-Tlink_ram.x` so
     the test binary runs from RAM (matching the upstream embassy
     convention).

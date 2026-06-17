CI for paavo. `ci.yml` runs `fmt`, `clippy`, and `cargo test --workspace`
against Rust 1.95 on Ubuntu. The `test` job first builds + lints the WASM UI
(`crates/paavo-web-ui`) with a pinned, prebuilt `trunk`, so the workspace gate
embeds the real Leptos bundle into `paavo-web`. probe-rs needs `libudev-dev` on
Linux even for host-only tests because of its dev-dependency graph.

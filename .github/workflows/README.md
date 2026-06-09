CI for paavo. `ci.yml` runs `fmt`, `clippy`, and `cargo test --workspace`
against Rust 1.95 on Ubuntu. probe-rs needs `libudev-dev` on Linux even for
host-only tests because of its dev-dependency graph.

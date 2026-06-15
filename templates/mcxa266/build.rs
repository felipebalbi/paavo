//! Wires the vendored embassy linker fragment + this crate's `memory.x`
//! into `OUT_DIR`, then tells the linker to pull `link_ram.x` in so the
//! resulting binary runs from RAM. Mirrors what `teleprobe` does
//! upstream, but works at cargo-generate scaffold time because the
//! linker fragment is shipped inside the template (rather than read
//! from a sibling `templates/shared/` that disappears after generation).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("link_ram.x"),
        include_str!("link_ram_cortex_m.x"),
    )
    .unwrap();
    fs::write(out.join("memory.x"), include_str!("memory.x")).unwrap();

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tlink_ram.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=link_ram_cortex_m.x");
}

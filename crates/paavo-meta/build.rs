//! Copy `paavo.x` into `OUT_DIR` and tell rustc to add `OUT_DIR` to the
//! linker search path. Downstream test crates can then put `-Tpaavo.x`
//! in their RUSTFLAGS (the cargo-generate templates do this in Milestone 6).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let frag = include_str!("paavo.x");
    fs::write(out.join("paavo.x"), frag).expect("writing paavo.x to OUT_DIR");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=paavo.x");
    println!("cargo:rerun-if-changed=build.rs");
}

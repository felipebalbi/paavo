//! Wires the vendored embassy linker fragment + this crate's `memory.x`
//! into `OUT_DIR`, then tells the linker to pull `link_ram.x` in so the
//! resulting binary runs from RAM. Mirrors templates/mcxa266/build.rs
//! byte-for-byte (the spike fixture is intentionally a structural twin
//! of what cargo-generate produces for `kind = "mcxa266"`, so the M7.7
//! hardware tests in paavo-probe exercise the exact same boot path the
//! production templates use).
//!
//! The fixture vendors its own copy of `link_ram_cortex_m.x` (a duplicate
//! of `templates/shared/link_ram_cortex_m.x`) instead of referencing the
//! shared file via a relative path. Reasons:
//!   - The fixture is workspace-excluded; cargo would happily compile
//!     against `../../templates/shared/...` but that couples a `dev/`
//!     fixture to a sibling tree it has no reason to know about.
//!   - The production templates ship their own copies for the same
//!     reason (cargo-generate output must be self-contained), so the
//!     fixture mirrors them rather than introducing a third pattern.
//! Refresh by `cp templates/shared/link_ram_cortex_m.x \
//!  dev/spike-fixture-mcxa266/link_ram_cortex_m.x` if the canonical
//! file changes.
//!
//! ### Stub `device.x`
//!
//! `link_ram_cortex_m.x` ends with `INCLUDE device.x`, which is the
//! cortex-m-rt convention for letting a PAC crate inject `PROVIDE()`
//! statements for device-specific interrupt handlers. The production
//! templates get this from `embassy-mcxa` (the MCXA-family PAC). The
//! spike fixture deliberately uses no HAL/PAC — its only job is to
//! prove probe-rs + RTT + defmt round-trip — so we write an empty
//! `device.x` here instead. cortex-m-rt's own build.rs gates that
//! `INCLUDE` behind the `device` cargo feature; our vendored linker
//! script doesn't (it matches the templates byte-for-byte), so we
//! satisfy the include with a no-op file. cortex-m-rt's library code
//! defines a default empty `__INTERRUPTS = []` array when the `device`
//! feature is off, so an empty `device.x` resolves cleanly.

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
    // Stub `device.x` — see crate-level doc comment above.
    fs::write(
        out.join("device.x"),
        b"/* Empty stub: spike fixture uses no PAC. See build.rs doc. */\n" as &[u8],
    )
    .unwrap();

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tlink_ram.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=link_ram_cortex_m.x");
}

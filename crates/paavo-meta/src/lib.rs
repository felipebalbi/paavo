//! no_std metadata helpers for paavo test crates.
//!
//! Provides three macros that embed per-test metadata into ELF sections
//! that `paavo-probe` reads at job-dispatch time:
//!
//! - [`target!`] — board kind this test targets (e.g. `b"frdm-mcx-a266"`).
//! - [`timeout!`] — hard-max wall clock for this test, in seconds.
//! - [`inactivity_timeout!`] — per-test override for the inactivity
//!   watchdog, in seconds.
//!
//! The companion `build.rs` ships a linker fragment (`paavo.x`) that
//! preserves the `.paavo.*` sections through the embedded linker.
//! The section name prefix is `.paavo.*` and is owned end-to-end by this
//! workspace; no external tool reads these sections today.
#![no_std]
#![forbid(unsafe_code)]

/// Embed a target identifier as a NUL-terminated byte string in
/// `.paavo.target`. Match against `BoardSpec::target_name` server-side.
///
/// **Call at most once per binary**: the macro emits a `#[no_mangle]`
/// static; a second invocation in the same crate is a hard linker error.
///
/// Pass the literal **without** a trailing NUL; the macro appends one.
///
/// ```ignore
/// paavo_meta::target!(b"frdm-mcx-a266");
/// ```
#[macro_export]
macro_rules! target {
    ($val:literal) => {
        #[cfg_attr(target_os = "none", link_section = ".paavo.target")]
        #[cfg_attr(not(target_os = "none"), link_section = ".rodata.paavo_meta_target")]
        #[used]
        #[no_mangle]
        pub static _PAAVO_META_TARGET: [u8; { $val.len() + 1 }] = {
            let mut buf = [0u8; { $val.len() + 1 }];
            let src: &[u8] = $val;
            let mut i = 0;
            while i < src.len() {
                buf[i] = src[i];
                i += 1;
            }
            buf
        };
    };
}

/// Embed the per-test hard-max wall clock (seconds) in `.paavo.timeout`.
///
/// **Call at most once per binary**: the macro emits a `#[no_mangle]`
/// static; a second invocation in the same crate is a hard linker error.
///
/// On-ELF wire format: 4 little-endian bytes (u32 LE). `paavo-probe` reads
/// the section with `u32::from_le_bytes`. The macro stores the bytes
/// explicitly so the contract holds on any target endianness.
#[macro_export]
macro_rules! timeout {
    ($val:literal) => {
        #[cfg_attr(target_os = "none", link_section = ".paavo.timeout")]
        #[cfg_attr(not(target_os = "none"), link_section = ".rodata.paavo_meta_timeout")]
        #[used]
        #[no_mangle]
        pub static _PAAVO_META_TIMEOUT: [u8; 4] = ($val as u32).to_le_bytes();
    };
}

/// Embed the per-test inactivity-timeout override (seconds) in
/// `.paavo.inactivity_timeout`. `paavo-probe` reads this section; if
/// absent, falls back to the job's `inactivity_timeout_ms`, which itself
/// falls back to the daemon's configured default.
///
/// **Call at most once per binary**: the macro emits a `#[no_mangle]`
/// static; a second invocation in the same crate is a hard linker error.
///
/// On-ELF wire format: 4 little-endian bytes (u32 LE).
#[macro_export]
macro_rules! inactivity_timeout {
    ($val:literal) => {
        #[cfg_attr(target_os = "none", link_section = ".paavo.inactivity_timeout")]
        #[cfg_attr(
            not(target_os = "none"),
            link_section = ".rodata.paavo_meta_inactivity_timeout"
        )]
        #[used]
        #[no_mangle]
        pub static _PAAVO_META_INACTIVITY_TIMEOUT: [u8; 4] = ($val as u32).to_le_bytes();
    };
}

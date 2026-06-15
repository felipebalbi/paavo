//! paavo-smoke — the minimal cross-compiled crate body the manual smoke
//! script ships to paavod. Prototype of what M6.1's `quick-test`
//! cargo-generate template will eventually emit.
//!
//! Why this exists: paavo-build runs `cargo build --release` in the
//! sandboxed crate dir and discovers an ELF in `target/<triple>/release/`.
//! `target/release/` (host build) would also satisfy elf discovery on
//! Linux, but Windows produces PE/COFF binaries (`.exe`), which
//! paavo-build's `is_elf` magic check correctly rejects. A real
//! cross-compiled artifact targeting the DUT (Cortex-M33 here) is the
//! only path that yields an honest ELF on every host.
//!
//! Behaviour: this binary has no real test body. With
//! `PAAVO_FAKE_RUNNER=1` paavod's FakeRunner returns Passed without
//! ever flashing or running the artifact. Once M6.4 lands the
//! RealRunner, this crate will need an actual `paavo_meta::test!` body
//! that emits the run-control sentinels.
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use cortex_m_rt::entry;

#[entry]
fn main() -> ! {
    // The fake-runner path never flashes the ELF, so the body here is
    // unreachable in practice. We still need a real entry point so
    // cortex-m-rt's link-time checks (reset vector, RESET handler) pass.
    // Plain `loop {}` compiles to `b .` — keeps the dep graph at
    // exactly one crate (no cortex-m, no semihosting).
    #[allow(clippy::empty_loop)]
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    #[allow(clippy::empty_loop)]
    loop {}
}

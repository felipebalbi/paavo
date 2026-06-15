//! Minimal Cortex-M33 fixture for the paavo M7.0 probe-rs spike.
//!
//! Links defmt-rtt so the RTT control block exists in RAM and the spike's
//! Step 6 (`Rtt::attach`) actually has something to find. Prints one
//! `Test OK`-style message and then breakpoints — the same exit pattern
//! paavo's real test crates will use.
//!
//! Build:
//!   cargo build --release    # already target-pinned via .cargo/config.toml
//!
//! Then point the spike at the resulting ELF:
//!   ../probe-rs-spike/target/release/probe-rs-spike --elf \
//!     target/thumbv8m.main-none-eabihf/release/spike-fixture-mcxa266

#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt::*;
use {defmt_rtt as _, panic_probe as _};

#[entry]
fn main() -> ! {
    // Five info-level frames so the spike has clear evidence RTT works
    // and can decode an arbitrary number of frames, not just one.
    info!("hello from spike-fixture-mcxa266");
    info!("paavo M7.0 probe-rs spike");
    info!("defmt is alive on the MCX-A266");
    info!("about to emit the Test OK marker");
    info!("Test OK");

    // The same exit convention the real paavo BoardWorker recognises:
    // info!("Test OK") followed by a bkpt. paavo-runner's worker.rs
    // matches `frame.message.trim() == "Test OK"` then waits for Bkpt.
    cortex_m::asm::bkpt();

    // If something resumes us, just loop. We've already printed the marker.
    #[allow(clippy::empty_loop)]
    loop {
        cortex_m::asm::nop();
    }
}

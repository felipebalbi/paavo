//! dma-stress-overnight — exercises the embassy-mcxa DMA driver for hours.
//!
//! Skeleton: replace the body with the real stress loop before promoting to
//! the nightly corpus.

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

paavo_meta::target!(b"frdm-mcx-a266");
paavo_meta::timeout!(14400);            // 4 h
paavo_meta::inactivity_timeout!(120);   // 2 min

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_mcxa::init(Default::default());
    info!("dma-stress-overnight skeleton");
    for i in 0u32..10 {
        Timer::after(Duration::from_secs(1)).await;
        info!("tick {=u32}", i);
    }
    info!("Test OK");
    cortex_m::asm::bkpt();
}

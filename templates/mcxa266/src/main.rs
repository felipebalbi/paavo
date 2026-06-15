//! {{crate_name}} — paavo test crate.

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

paavo_meta::target!(b"frdm-mcx-a266");
paavo_meta::timeout!(60);
paavo_meta::inactivity_timeout!(30);

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_mcxa::init(Default::default());
    info!("hello from {{crate_name}}");
    // TODO: write your test here.
    Timer::after(Duration::from_secs(1)).await;
    info!("Test OK");
    cortex_m::asm::bkpt();
}

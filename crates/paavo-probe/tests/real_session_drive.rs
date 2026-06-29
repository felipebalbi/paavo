//! Hardware-only test for `RealSession::next_event` — the streaming side
//! of the M7 happy path.
//!
//! Gated two ways (same as `real_session_connect.rs`):
//!   - `#[ignore]` so the default `cargo test --workspace` skips it.
//!   - `PAAVO_HW=1` env var so even when run with `--ignored`, dev boxes
//!     without the EVK plugged in self-skip without surfacing as failure.
//!
//! Depends on the spike fixture ELF; build it first by `cd`-ing INTO the
//! fixture directory (NOT via `--manifest-path` from the workspace root —
//! see `real_session_connect.rs` for the rationale):
//!
//!   cd dev/spike-fixture-mcxa266
//!   cargo build --release
//!
//! Run with:
//!   $env:PAAVO_HW = "1"
//!   cargo test -p paavo-probe --test real_session_drive -- --ignored --nocapture
//!
//! The spike fixture emits five `info!()` frames ending with
//! `info!("Test OK")` followed by a `cortex_m::asm::bkpt()`. This test
//! asserts that within 10 s we observe BOTH a decoded `Test OK` Info
//! frame AND a `Bkpt` event — exactly the pair that paavo-runner's
//! `drive_session` needs to map to `JobOutcome::Passed`.

use paavo_probe::{Event, ProbeSession, RealSession, RealSessionOptions};
use paavo_proto::{LogLevel, ProbeSelector};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn hw_or_skip() -> bool {
    if std::env::var("PAAVO_HW").is_err() {
        eprintln!("PAAVO_HW not set; skipping hardware test");
        return false;
    }
    true
}

fn elf_fixture() -> PathBuf {
    let here = std::env::current_dir().expect("cwd");
    let repo = here
        .ancestors()
        .find(|p| p.join("dev/spike-fixture-mcxa266/Cargo.toml").is_file())
        .expect("can't find repo root from CWD");
    let elf = repo.join(
        "dev/spike-fixture-mcxa266/target/thumbv8m.main-none-eabihf/release/spike-fixture-mcxa266",
    );
    assert!(
        elf.is_file(),
        "spike fixture ELF not built. Build it FROM INSIDE the fixture dir:\n  \
         cd {}/dev/spike-fixture-mcxa266 && cargo build --release",
        repo.display()
    );
    elf
}

#[test]
#[ignore]
fn next_event_streams_decoded_test_ok_then_bkpt() {
    if !hw_or_skip() {
        return;
    }

    let opts = RealSessionOptions {
        probe_selector: ProbeSelector {
            vid: "1fc9".into(),             // NXP
            pid: "0143".into(),             // MCU-Link CMSIS-DAP
            serial: "EDFHUAFM4J5ZJ".into(), // Felipe's specific EVK
            interface: None,
        },
        chip_name: "MCXA276".into(), // NOT MCXA266; spike finding
        elf_path: elf_fixture(),
        skip_post_load_reset: false,
    };
    let mut session =
        RealSession::connect(opts).expect("connect must succeed against the MCX-A266 EVK");

    let mut got_test_ok = false;
    let mut got_bkpt = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !(got_test_ok && got_bkpt) {
        match session.next_event(500).expect("next_event") {
            Some(Event::LogFrame(f)) => {
                eprintln!("frame: {:?} {:?}", f.level, f.message);
                if f.level == LogLevel::Info && f.message.trim() == "Test OK" {
                    got_test_ok = true;
                }
            }
            Some(Event::Bkpt) => {
                eprintln!("event: Bkpt");
                got_bkpt = true;
            }
            Some(Event::Panic { message }) => panic!("unexpected panic frame: {message}"),
            Some(Event::Disconnect) => panic!("unexpected disconnect"),
            None => {
                // No event this tick; keep polling.
            }
        }
    }

    assert!(
        got_test_ok,
        "never decoded a `Test OK` info frame within 10s"
    );
    assert!(got_bkpt, "never observed bkpt within 10s");
}

//! Hardware-only test for `RealSession::connect`.
//!
//! Gated two ways:
//!   - `#[ignore]` so the default `cargo test --workspace` skips it.
//!   - `PAAVO_HW=1` env var so even when run with `--ignored`, dev boxes
//!     without the EVK plugged in self-skip without surfacing as failure.
//!
//! Depends on the spike fixture ELF; build it first by `cd`-ing INTO the
//! fixture directory (NOT via `--manifest-path` from the workspace root —
//! that bypasses the fixture's `.cargo/config.toml` which carries the
//! `-Tdefmt.x` linker flag, silently dropping the `.defmt` section and
//! making `RealSession::connect` fail at the `Table::parse` step):
//!
//!   cd dev/spike-fixture-mcxa266
//!   cargo build --release
//!
//! Run with:
//!   $env:PAAVO_HW = "1"
//!   cargo test -p paavo-probe --test real_session_connect -- --ignored --nocapture

use paavo_probe::{RealSession, RealSessionOptions};
use paavo_proto::ProbeSelector;
use std::path::PathBuf;

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
        "spike fixture ELF not built. Build it FROM INSIDE the fixture dir \
         (the .cargo/config.toml there carries the -Tdefmt.x linker flag; \
         building via --manifest-path from elsewhere drops it and produces \
         an ELF with no .defmt section):\n  \
         cd {}/dev/spike-fixture-mcxa266 && cargo build --release",
        repo.display()
    );
    elf
}

#[test]
#[ignore]
fn connect_flashes_and_returns_live_session() {
    if !hw_or_skip() {
        return;
    }
    // The vid/pid/serial below match the MCU-Link on Felipe's MCX-A266 EVK.
    // A different EVK on a different dev box: override these three fields
    // (or wrap them in env-var reads). See `dev/probe-rs-spike/FINDINGS.md`
    // for how to enumerate the locally-visible probes.
    let opts = RealSessionOptions {
        probe_selector: ProbeSelector {
            vid: "1fc9".into(),             // NXP
            pid: "0143".into(),             // MCU-Link CMSIS-DAP
            serial: "EDFHUAFM4J5ZJ".into(), // Felipe's specific EVK
        },
        chip_name: "MCXA276".into(), // NOT MCXA266; spike finding
        elf_path: elf_fixture(),
        skip_post_load_reset: false,
    };
    let session =
        RealSession::connect(opts).expect("connect must succeed against the MCX-A266 EVK");
    // Session struct is returned; drop releases the probe.
    drop(session);
}

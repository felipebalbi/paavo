//! probe-rs API spike for paavo M7 (sub-task 7.0).
//!
//! This is a one-shot exploration binary. It is NOT a workspace member and
//! is NOT shipped. The purpose is to confirm, against real hardware, the
//! probe-rs 0.31 call shapes we'll use in `paavo-probe::RealSession`:
//!
//! - `Lister::new().list_all()` enumeration
//! - `Lister::open(DebugProbeSelector)` by VID/PID/serial
//! - `Probe::attach(target, Permissions)` against the MCXA256 chip
//! - `flashing::download_file(&mut session, elf_path, Format::Elf)`
//! - `Session::core(0)?.reset_and_halt(timeout)` then `.run()`
//! - `rtt::Rtt::attach(&mut core)` + `UpChannel::read(&mut core, &mut buf)`
//!
//! Run it by hand against a real MCX-A266 EVK with an ELF that does some
//! defmt-RTT output. Findings get folded back into the M7 plan/spec.
//!
//! Usage:
//!   cargo run --release -- --elf <path-to-test-elf>
//!
//! Optional flags select a specific probe (defaults to NXP MCU-Link with
//! the serial that's on Felipe's desk):
//!   --vid 0x1fc9 --pid 0x0143 --serial EDFHUAFM4J5ZJ
//!   --chip MCXA256
//!   --read-secs 5    (how long to drain RTT after reset)

use anyhow::{bail, Context, Result};
use clap::Parser;
use probe_rs::{
    flashing::{download_file, FormatKind},
    probe::{list::Lister, DebugProbeSelector},
    Permissions,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "probe-rs-spike", about = "paavo M7.0 probe-rs API spike")]
struct Args {
    /// Path to an ELF that has been built for thumbv8m.main-none-eabihf
    /// and links cortex-m-rt + defmt-rtt. The spike will flash it and
    /// then drain RTT for `--read-secs` seconds.
    #[arg(long)]
    elf: PathBuf,

    /// probe USB vendor id. Defaults to NXP (`0x1fc9`).
    #[arg(long, value_parser = parse_hex_u16, default_value = "0x1fc9")]
    vid: u16,

    /// probe USB product id. Defaults to MCU-Link CMSIS-DAP (`0x0143`).
    #[arg(long, value_parser = parse_hex_u16, default_value = "0x0143")]
    pid: u16,

    /// probe serial number. Defaults to the MCU-Link on Felipe's MCX-A266 EVK
    /// (`EDFHUAFM4J5ZJ`). Pass `--serial ""` to skip serial matching.
    #[arg(long, default_value = "EDFHUAFM4J5ZJ")]
    serial: String,

    /// probe-rs chip name to attach as. MCX-A266 part = `MCXA266VFL`;
    /// probe-rs internal target name = `MCXA256` (the SoC family).
    #[arg(long, default_value = "MCXA256")]
    chip: String,

    /// How many seconds to drain RTT after reset+run.
    #[arg(long, default_value_t = 5)]
    read_secs: u64,
}

fn parse_hex_u16(s: &str) -> Result<u16, String> {
    let s = s.trim();
    let trimmed = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u16::from_str_radix(trimmed, 16).map_err(|e| format!("bad hex u16 '{s}': {e}"))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "probe_rs_spike=info,probe_rs=warn".into()),
        )
        .init();

    let args = Args::parse();

    if !args.elf.is_file() {
        bail!("--elf path does not exist or is not a file: {}", args.elf.display());
    }

    // ─── 1. Enumerate ─────────────────────────────────────────────────────
    println!("== Step 1: enumerate ==");
    let lister = Lister::new();
    let all = lister.list_all();
    if all.is_empty() {
        bail!("no debug probes found by probe-rs");
    }
    for (i, p) in all.iter().enumerate() {
        println!(
            "  [{i}] {} -- {:04x}:{:04x}{} ({:?})",
            p.identifier,
            p.vendor_id,
            p.product_id,
            p.serial_number
                .as_ref()
                .map(|s| format!(" -- s/n: {s}"))
                .unwrap_or_default(),
            p.probe_type(),
        );
    }

    // ─── 2. Open by selector ──────────────────────────────────────────────
    println!("\n== Step 2: open by VID/PID{} ==",
        if args.serial.is_empty() { "" } else { "/serial" });
    let selector = if args.serial.is_empty() {
        DebugProbeSelector {
            vendor_id: args.vid,
            product_id: args.pid,
            interface: None,
            serial_number: None,
        }
    } else {
        DebugProbeSelector {
            vendor_id: args.vid,
            product_id: args.pid,
            interface: None,
            serial_number: Some(args.serial.clone()),
        }
    };
    println!("  selector: {selector:?}");
    let probe = lister
        .open(selector)
        .context("Lister::open failed; is the MCU-Link plugged in and not held by another process?")?;
    println!("  opened probe: {}", probe.get_name());

    // ─── 3. Attach to target ──────────────────────────────────────────────
    println!("\n== Step 3: Probe::attach(chip={}) ==", args.chip);
    let mut session = probe
        .attach(args.chip.as_str(), Permissions::default())
        .with_context(|| format!("Probe::attach({}) failed", args.chip))?;
    println!("  attached. cores = {:?}", session.list_cores());
    println!("  arch = {:?}", session.architecture());

    // ─── 4. Flash the ELF ─────────────────────────────────────────────────
    println!("\n== Step 4: download_file (flash {}) ==", args.elf.display());
    let t = Instant::now();
    // probe-rs 0.31: `download_file` takes `impl Into<Format>`. `Format::Elf`
    // is a variant constructor taking `ElfOptions`, so a bare `Format::Elf`
    // doesn't work. The cleanest path is `FormatKind::Elf` — `FormatKind`
    // is a unit enum and there's a `From<FormatKind> for Format` impl that
    // wraps with `ElfOptions::default()`.
    download_file(&mut session, &args.elf, FormatKind::Elf)
        .with_context(|| format!("download_file({}) failed", args.elf.display()))?;
    println!("  flashed in {:?}", t.elapsed());

    // ─── 5. Reset + run ───────────────────────────────────────────────────
    println!("\n== Step 5: core(0)?.reset_and_halt + run ==");
    {
        let mut core = session.core(0).context("session.core(0) failed")?;
        // reset_and_halt so we know RTT init hasn't raced past us.
        core.reset_and_halt(Duration::from_secs(2))
            .context("core.reset_and_halt failed")?;
        println!("  halted at reset vector.");
        core.run().context("core.run failed")?;
        println!("  running.");
    } // drop core borrow before RTT-attach (which also wants &mut core)

    // ─── 6. Attach RTT and drain ──────────────────────────────────────────
    println!("\n== Step 6: Rtt::attach + drain up channel(s) for {}s ==", args.read_secs);
    // Give the firmware a moment to initialise RTT before we scan.
    std::thread::sleep(Duration::from_millis(200));
    let mut rtt = {
        let mut core = session.core(0).context("session.core(0) for rtt failed")?;
        probe_rs::rtt::Rtt::attach(&mut core)
            .context("Rtt::attach failed (firmware probably hasn't initialised RTT yet — \
                      did you link defmt-rtt, and did main() touch it?)")?
    };
    println!("  control block @ 0x{:x}", rtt.ptr());
    println!("  up channels:");
    for ch in rtt.up_channels.iter() {
        println!(
            "    #{} name={:?} buffer={}B",
            ch.number(),
            ch.name(),
            ch.buffer_size()
        );
    }
    println!("  down channels:");
    for ch in rtt.down_channels.iter() {
        println!(
            "    #{} name={:?} buffer={}B",
            ch.number(),
            ch.name(),
            ch.buffer_size()
        );
    }

    let mut total: usize = 0;
    let mut buf = [0u8; 1024];
    let deadline = Instant::now() + Duration::from_secs(args.read_secs);
    while Instant::now() < deadline {
        // Take a fresh `Core` view each iteration — `Session::core()` is
        // cheap after the first call (it's just returning a handle to the
        // already-attached core, per the rustdoc).
        let mut core = session.core(0)?;
        let Some(ch) = rtt.up_channels.first_mut() else {
            bail!("target firmware exposes zero RTT up channels — nothing to read");
        };
        let n = ch
            .read(&mut core, &mut buf)
            .context("UpChannel::read failed")?;
        if n > 0 {
            total += n;
            // Show first bytes as hex + ascii preview.
            let preview: String = buf[..n.min(64)]
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("  ch{}: read {n}B (total {total}B); preview: {preview:?}",
                     ch.number());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("\n== Done. Drained {total}B of RTT data total. ==");
    if total == 0 {
        println!(
            "  HINT: target firmware never wrote anything to RTT. Either it's not \
             linking defmt-rtt, or its main() returns/halts before producing output, \
             or the up-channel buffer is the wrong one. Try a known-good ELF."
        );
    }

    // ─── 7. Findings dump ─────────────────────────────────────────────────
    println!("\n== Findings (paste into M7 plan) ==");
    println!("- probe-rs version: 0.31");
    println!("- DebugProbeSelector has 4 public fields: vendor_id, product_id,");
    println!("  interface: Option<u8>, serial_number: Option<String>.");
    println!("  (Don't forget `interface: None` — easy to omit.)");
    println!("- DebugProbeInfo::probe_type() is a METHOD, not a field.");
    println!("- Format is NOT a unit enum. Use `FormatKind::Elf` (unit) and let");
    println!("  the From<FormatKind> for Format impl wrap with ElfOptions::default().");
    println!("- Lister::open(DebugProbeSelector): ok");
    println!("- Probe::attach(chip_name_str, Permissions::default()): ok");
    println!("- download_file(&mut session, elf_path, FormatKind::Elf): ok");
    println!("- Session::core(0)?.reset_and_halt(Duration) + .run(): ok");
    println!("- Rtt::attach(&mut core): ok (auto-scans RAM for control block)");
    println!("- UpChannel::read(&mut core, &mut [u8]) -> Result<usize>: ok");
    println!("- Send/Sync: Session is Send + !Sync, Probe is Send + !Sync,");
    println!("  Lister is !Send. BoardWorker thread owns Session for the job;");
    println!("  matches existing ProbeSession trait bound (Send, not Sync).");
    println!("- NXP MCX-A266 (the EVK part) attaches as probe-rs chip name");
    println!("  `MCXA276` — NOT `MCXA266` or `MCXA256`. Document this trap.");
    println!("- A WARN line `probe_rs::vendor::nxp::sequences::mcx: unknown");
    println!("  variant, using default watchpoint configuration` is emitted");
    println!("  twice during attach+reset. Cosmetic; flashing + RTT both work.");
    println!("- defmt 1.0 RTT framing is very compact (~3-4 bytes per info!).");
    println!("  Decoding via defmt_decoder::Table is MANDATORY to recover the");
    println!("  'Test OK' marker; raw-bytes matching will not work.");

    Ok(())
}

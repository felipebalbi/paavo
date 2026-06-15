//! Probe session abstraction. The real `probe-rs` + `defmt-decoder` adapter
//! lives behind this trait; tests in `paavo-runner` (and elsewhere) stub it
//! with a deterministic mock.
//!
//! `RealSession::connect` is wired in M7.4 (this milestone). The matching
//! `RealSession::next_event` (RTT poll + defmt decode + bkpt detect) lands
//! in M7.5 and currently still errors immediately.

use crate::error::{ProbeError, Result};
use crate::event::Event;
use defmt_decoder::Table;
use probe_rs::{
    flashing::{download_file, FormatKind},
    probe::{list::Lister, DebugProbeSelector},
    rtt::Rtt,
    Permissions, Session,
};
use std::path::PathBuf;
use std::time::Duration;

/// Long-lived probe session that flashes and observes a single test.
///
/// Implementors must be `Send` because the BoardWorker thread owns the
/// session for the duration of a job. `Sync` is deliberately NOT required:
/// `probe_rs::Session` is `Send + !Sync` (verified in the M7.0 spike,
/// see `dev/probe-rs-spike/FINDINGS.md`).
pub trait ProbeSession: Send {
    /// Block until the next event is available (up to `timeout_ms`
    /// milliseconds), or return `Ok(None)` if the target has reached a
    /// clean stop. Implementations may return events back-to-back with no
    /// inter-event delay.
    fn next_event(&mut self, timeout_ms: u32) -> Result<Option<Event>>;
}

/// Connection options for the real probe-rs adapter.
#[derive(Debug, Clone)]
pub struct RealSessionOptions {
    /// USB selector for probe-rs.
    pub probe_selector: paavo_proto::ProbeSelector,
    /// probe-rs chip name.
    pub chip_name: String,
    /// Path to the ELF to flash and run.
    pub elf_path: PathBuf,
    /// If true, skip the post-load reset (NXP RT685S quirk; see spec §2).
    pub skip_post_load_reset: bool,
}

/// Real `probe-rs` + `defmt-decoder` backed session.
///
/// Ownership: `Session` is `Send + !Sync` (verified in the M7.0 spike).
/// We hold it owned, on the BoardWorker thread. The `Rtt` handle borrows
/// `&mut Core` per read, which is why `next_event` (M7.5) re-takes
/// `session.core(0)` on each call.
///
/// **Decoder lifetime note**: `defmt_decoder::Table::new_stream_decoder`
/// returns `Box<dyn StreamDecoder + Send + Sync + '_>` — the decoder
/// **borrows** from the `Table`. A self-referencing struct (table + decoder)
/// would require `ouroboros`/`self_cell`/`Pin` machinery. We instead store
/// the owned `Table` and recreate the decoder inside `next_event` (M7.5).
/// `connect` is called once per job and `next_event` is the hot path; the
/// per-call allocation is negligible compared to the probe USB round-trip.
pub struct RealSession {
    /// probe-rs session. Drop releases the probe.
    #[allow(dead_code)] // consumed by next_event in M7.5
    session: Session,
    /// RTT handle scanned out of target RAM. Survives across `next_event`
    /// calls; the up-channel is read via `&mut session.core(0)`.
    #[allow(dead_code)] // consumed by next_event in M7.5
    rtt: Rtt,
    /// defmt decode table parsed from the ELF's `.defmt` section.
    /// Decoder is recreated per `next_event` call from this table.
    #[allow(dead_code)] // consumed by next_event in M7.5
    table: Table,
    /// Reusable read buffer; sized to the up-channel's buffer.
    #[allow(dead_code)] // consumed by next_event in M7.5
    rtt_buf: Vec<u8>,
    /// True once we've emitted `Bkpt`. Used to debounce repeated halts.
    #[allow(dead_code)] // consumed by next_event in M7.5
    seen_bkpt: bool,
}

impl RealSession {
    /// Connect to a probe, flash the ELF, and start RTT.
    ///
    /// Steps:
    ///   1. `Lister::new().open(DebugProbeSelector)` — by VID/PID/serial.
    ///   2. `Probe::attach(chip, Permissions::default())` — chip name as a `&str`.
    ///   3. `flashing::download_file(&mut session, &elf, FormatKind::Elf)`.
    ///   4. `core(0)?.reset_and_halt(2s) + .run()` (unless `skip_post_load_reset`).
    ///   5. `defmt_decoder::Table::parse(elf_bytes)` — for the decode table.
    ///   6. 200 ms sleep, then `Rtt::attach(&mut core)`.
    ///
    /// **Hardware-only**: requires a physical probe + board. Workspace
    /// tests use a mock `ProbeSession` impl.
    pub fn connect(opts: RealSessionOptions) -> Result<Self> {
        // The wire selector is three String fields (vid/pid as hex strings,
        // serial as plain string). Parse the hex VID/PID into u16 here.
        let vid = parse_hex_u16(&opts.probe_selector.vid).map_err(|e| {
            ProbeError::ProbeRs(format!(
                "probe selector: bad vid {:?}: {e}",
                opts.probe_selector.vid
            ))
        })?;
        let pid = parse_hex_u16(&opts.probe_selector.pid).map_err(|e| {
            ProbeError::ProbeRs(format!(
                "probe selector: bad pid {:?}: {e}",
                opts.probe_selector.pid
            ))
        })?;
        // Empty `serial` means "don't filter by serial"; matches the spike behaviour.
        let serial_filter = if opts.probe_selector.serial.is_empty() {
            None
        } else {
            Some(opts.probe_selector.serial.clone())
        };

        // 1. Open the probe by selector.
        let lister = Lister::new();
        let selector = DebugProbeSelector {
            vendor_id: vid,
            product_id: pid,
            interface: None,
            serial_number: serial_filter.clone(),
        };
        let probe = lister.open(selector).map_err(|e| {
            ProbeError::ProbeRs(format!(
                "open probe vid={vid:04x} pid={pid:04x} serial={serial_filter:?}: {e}"
            ))
        })?;

        // 2. Attach to the chip.
        let mut session = probe
            .attach(opts.chip_name.as_str(), Permissions::default())
            .map_err(|e| ProbeError::ProbeRs(format!("attach chip={}: {e}", opts.chip_name)))?;

        // 3. Flash. `FormatKind::Elf` is a unit variant — `Format::Elf`
        // takes `ElfOptions`, and `From<FormatKind> for Format` wraps with
        // `ElfOptions::default()` (spike finding).
        download_file(&mut session, &opts.elf_path, FormatKind::Elf)
            .map_err(|e| ProbeError::ProbeRs(format!("flash {}: {e}", opts.elf_path.display())))?;

        // 4. Reset + run (unless caller asked to skip — RT685S quirk).
        if !opts.skip_post_load_reset {
            let mut core = session
                .core(0)
                .map_err(|e| ProbeError::ProbeRs(format!("session.core(0): {e}")))?;
            core.reset_and_halt(Duration::from_secs(2))
                .map_err(|e| ProbeError::ProbeRs(format!("reset_and_halt: {e}")))?;
            core.run()
                .map_err(|e| ProbeError::ProbeRs(format!("core.run: {e}")))?;
        }

        // 5. Parse `.defmt` for the decode table BEFORE the RTT attach
        // so a malformed ELF surfaces here, not deep in next_event.
        let elf_bytes = std::fs::read(&opts.elf_path).map_err(|e| {
            ProbeError::ProbeRs(format!(
                "read elf {} for .defmt: {e}",
                opts.elf_path.display()
            ))
        })?;
        let table = Table::parse(&elf_bytes)
            .map_err(|e| ProbeError::ProbeRs(format!(".defmt section parse: {e}")))?
            .ok_or_else(|| {
                ProbeError::ProbeRs("ELF has no .defmt section — test crate must link defmt".into())
            })?;

        // 6. Wait briefly for firmware to initialise RTT, then attach.
        // The 200ms is empirically necessary (spike finding); attaching
        // sooner errors with `ControlBlockNotFound`.
        std::thread::sleep(Duration::from_millis(200));
        let rtt = {
            let mut core = session
                .core(0)
                .map_err(|e| ProbeError::ProbeRs(format!("session.core(0) for rtt: {e}")))?;
            Rtt::attach(&mut core).map_err(|e| {
                ProbeError::ProbeRs(format!(
                    "rtt attach: {e} (firmware probably hasn't initialised RTT yet — \
                     link defmt-rtt and ensure main() touches it before doing anything slow)"
                ))
            })?
        };

        // Size the read buffer to the up-channel's buffer; default to
        // 1024 (what defmt-rtt uses) if there are no up channels (we'll
        // never read in that case anyway).
        let buf_size = rtt
            .up_channels
            .first()
            .map(|c| c.buffer_size().max(256))
            .unwrap_or(1024);

        Ok(Self {
            session,
            rtt,
            table,
            rtt_buf: vec![0u8; buf_size],
            seen_bkpt: false,
        })
    }
}

impl ProbeSession for RealSession {
    fn next_event(&mut self, _timeout_ms: u32) -> Result<Option<Event>> {
        Err(ProbeError::ProbeRs(
            "RealSession::next_event is wired in Milestone 7.5".into(),
        ))
    }
}

/// Parse a hex string into a `u16`, tolerating an optional `0x`/`0X` prefix
/// and surrounding whitespace.
///
/// **Always base-16.** A bare `"10"` parses to `0x10 = 16`, NOT to decimal 10.
/// This matches the contract documented on `paavo_proto::ProbeSelector::vid`
/// and `pid` (4 hex digits, e.g. `"1fc9"` for NXP). Operators writing
/// `boards.toml` MUST use hex; a misread `vid = "8137"` would silently
/// target VID `0x8137` (Soundcraft Mixer), not `0x1FC9` (NXP).
fn parse_hex_u16(s: &str) -> std::result::Result<u16, std::num::ParseIntError> {
    let s = s.trim();
    let stripped = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(stripped, 16)
}

/// Compile-time assertion that `RealSession: Send`. The `ProbeSession`
/// trait requires `Send` (the BoardWorker thread owns the session), so a
/// silent regression here would surface only when wiring 7.6 — at which
/// point the cargo error path is hard to read. Catch it at module-compile
/// time instead. (`!Sync` is intentional and unchecked — Rust has no
/// negative bounds; the prose comment on `RealSession` documents it.)
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<RealSession>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_u16_bare() {
        assert_eq!(parse_hex_u16("1fc9").unwrap(), 0x1fc9);
        assert_eq!(parse_hex_u16("0143").unwrap(), 0x0143);
        assert_eq!(parse_hex_u16("0").unwrap(), 0);
        assert_eq!(parse_hex_u16("ffff").unwrap(), 0xffff);
    }

    #[test]
    fn parse_hex_u16_prefix() {
        assert_eq!(parse_hex_u16("0x1fc9").unwrap(), 0x1fc9);
        assert_eq!(parse_hex_u16("0X1FC9").unwrap(), 0x1fc9);
    }

    #[test]
    fn parse_hex_u16_trim_whitespace() {
        assert_eq!(parse_hex_u16("  1fc9 ").unwrap(), 0x1fc9);
        assert_eq!(parse_hex_u16("\t0x143\n").unwrap(), 0x143);
    }

    #[test]
    fn parse_hex_u16_case_insensitive_digits() {
        // u16::from_str_radix accepts both cases.
        assert_eq!(parse_hex_u16("1FC9").unwrap(), 0x1fc9);
        assert_eq!(parse_hex_u16("AbCd").unwrap(), 0xabcd);
    }

    #[test]
    fn parse_hex_u16_rejects_non_hex() {
        assert!(parse_hex_u16("xyz").is_err());
        assert!(parse_hex_u16("1g").is_err());
        assert!(parse_hex_u16("").is_err());
    }

    #[test]
    fn parse_hex_u16_rejects_overflow() {
        // u16 max is 0xffff; 0x10000 overflows.
        assert!(parse_hex_u16("10000").is_err());
        assert!(parse_hex_u16("ffffff").is_err());
    }

    #[test]
    fn parse_hex_u16_bare_decimal_is_treated_as_hex() {
        // Locks in the contract documented on parse_hex_u16: bare digits
        // are HEX, not decimal. A decimal `8137` in boards.toml would
        // silently parse as hex 0x8137. This test exists so a future
        // contributor "fixing" the function to accept decimal first
        // breaks the test and re-reads the contract.
        assert_eq!(parse_hex_u16("8137").unwrap(), 0x8137);
        assert_eq!(parse_hex_u16("10").unwrap(), 0x10);
    }
}

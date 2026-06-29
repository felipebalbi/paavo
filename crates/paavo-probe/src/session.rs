//! Probe session abstraction. The real `probe-rs` + `defmt-decoder` adapter
//! lives behind this trait; tests in `paavo-runner` (and elsewhere) stub it
//! with a deterministic mock.
//!
//! `RealSession::connect` is wired in M7.4 and `RealSession::next_event`
//! (RTT poll + defmt decode + bkpt detect) is wired in M7.5 (this
//! milestone).

use crate::error::{ProbeError, Result};
use crate::event::Event;
use defmt_decoder::{DecodeError, StreamDecoder, Table};
use defmt_parser::Level as DefmtLevel;
use paavo_proto::{LogFrame, LogLevel};
use probe_rs::{
    flashing::{build_loader, BootInfo, DownloadOptions, Format, FormatKind},
    probe::{list::Lister, DebugProbeSelector},
    rtt::{ChannelMode, Rtt},
    Permissions, Session,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Long-lived probe session that flashes and observes a single test.
///
/// Implementors must be `Send` because the BoardWorker thread owns the
/// session for the duration of a job. `Sync` is deliberately NOT required:
/// `probe_rs::Session` is `Send + !Sync` (verified in the M7.0 spike,
/// see `dev/probe-rs-spike/FINDINGS.md`).
pub trait ProbeSession: Send {
    /// Block until the next event is available, or return `Ok(None)` if
    /// the target has reached a clean stop.
    ///
    /// `timeout_ms` is a **hint**, not a hard upper bound. Implementations
    /// are free to wake earlier (so the calling worker can poll its
    /// watchdog state between calls). The real adapter (`RealSession`)
    /// caps each idle sleep at ~50 ms so a cancelled job notices its stop
    /// reason within that window; drive_session is built around the
    /// assumption that it gets a tick to check `state.stop_reason()`
    /// between `next_event` returns regardless of what timeout was asked.
    ///
    /// Implementations MAY return events back-to-back with no inter-event
    /// delay (e.g. when there's a backlog of decoded frames to drain).
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
/// `&mut Core` per read, which is why `next_event` re-takes
/// `session.core(0)` on each call.
///
/// **Decoder lifetime — leak trade-off**: `Table::new_stream_decoder()`
/// returns `Box<dyn StreamDecoder + Send + Sync + '_>` — the decoder
/// **borrows** from the `Table`. Storing both on the same struct is
/// self-referential and would need `ouroboros`/`self_cell`/`Pin`. We
/// instead `Box::leak` the `Table` to give it `'static` lifetime so we
/// can store the decoder directly. Cost: a few KB leaked per
/// `RealSession` ever created. paavod constructs ONE `RealSession` per
/// job and drops it on job completion, so the leak is bounded by
/// jobs-per-process-lifetime. M8 may switch to `self_cell` if memory
/// matters in practice.
pub struct RealSession {
    /// probe-rs session. Drop releases the probe.
    session: Session,
    /// RTT handle scanned out of target RAM. Survives across `next_event`
    /// calls; the up-channel is read via `&mut session.core(0)`.
    rtt: Rtt,
    /// Index into `rtt.up_channels` of the channel named "defmt". Looked
    /// up by name during `connect()` so we don't accidentally read from
    /// a SEGGER-terminal-style sibling channel that some firmwares add at
    /// a lower index. defmt-rtt's own channel is always named "defmt"
    /// (see `defmt-rtt/src/lib.rs::NAME`).
    defmt_channel_index: usize,
    /// defmt stream decoder. Internally buffers RTT bytes across calls;
    /// `received()` appends, `decode()` drains one frame at a time. The
    /// `'static` lifetime is achieved via the `Box::leak(Table)` trick
    /// (see struct doc).
    decoder: Box<dyn StreamDecoder + Send + Sync + 'static>,
    /// Reusable RTT read buffer; sized to the up-channel's buffer.
    rtt_buf: Vec<u8>,
    /// True once we've emitted `Bkpt`. Used to debounce repeated halts:
    /// post-bkpt the target is permanently halted, but we only want ONE
    /// `Bkpt` event — subsequent calls drain RTT only.
    seen_bkpt: bool,
    /// Monotonic sequence number for `LogFrame::seq`, bumped on each
    /// emitted frame.
    seq: u64,
    /// Job start instant; `LogFrame::ts_us` is microseconds since this.
    started_at: Instant,
}

impl RealSession {
    /// Connect to a probe, flash the ELF, and start RTT.
    ///
    /// Steps:
    ///   1. `Lister::new().open(DebugProbeSelector)` — by VID/PID/serial.
    ///   2. `Probe::attach(chip, Permissions::default())` — chip name as a `&str`.
    ///   3. `flashing::build_loader(...)`, capture `loader.boot_info()`,
    ///      then `loader.commit(&mut session, DownloadOptions)`. Going
    ///      via `build_loader` (instead of the convenience wrapper
    ///      `download_file_with_options`) keeps the loader alive long
    ///      enough to inspect `boot_info()`, which we need in step 4
    ///      to decide how to start the firmware.
    ///   4. Boot the loaded image (unless `skip_post_load_reset`):
    ///         - `BootInfo::FromRam { vector_table_addr, .. }` →
    ///           `session.prepare_running_on_ram(vector_table_addr) +
    ///           core.run()`. `prepare_running_on_ram` sets SP_main, PC,
    ///           and VTOR from the vector table — there is NO hardware
    ///           reset, because the chip's flash reset vector points at
    ///           ROM/leftover firmware, not at our RAM-loaded code.
    ///         - `BootInfo::Other` → error. paavo's templates produce
    ///           RAM-resident ELFs only (`link_ram_cortex_m.x`); a
    ///           flash-resident ELF surfacing here is a misconfiguration
    ///           we surface clearly rather than silently boot wrong.
    ///   5. `defmt_decoder::Table::parse(elf_bytes)` — for the decode table.
    ///      The `Table` is `Box::leak`-ed so its `'static` ref can hand
    ///      out a `Box<dyn StreamDecoder + 'static>` we store on Self
    ///      (see struct doc for the trade-off).
    ///   6. 200 ms sleep, then `Rtt::attach_at(&mut core, _SEGGER_RTT)`.
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
            interface: opts.probe_selector.interface,
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

        // 3. Flash. paavo only supports RAM-resident ELFs (linked with
        //    `link_ram_cortex_m.x`); see `templates/*/memory.x` and
        //    `templates/shared/link_ram_cortex_m.x`. The vector table,
        //    .text, .rodata, and .data all live at ORIGIN(RAM) and there
        //    is no FLASH region. To boot one of these, we let probe-rs
        //    write the ELF into RAM and then we set SP/PC/VTOR from the
        //    vector table — we do NOT issue a hardware reset, because
        //    the chip's reset vector points to flash (boot ROM or
        //    leftover firmware), NOT to our RAM-loaded code.
        //
        //    `download_file_with_options` is a thin wrapper around
        //    `build_loader().commit()` that drops the loader and so
        //    discards `loader.boot_info()`. We need that info — it
        //    carries `vector_table_addr` for `prepare_running_on_ram`
        //    — so we call `build_loader` and `commit` directly.
        //
        //    `Format::from(FormatKind::Elf)` wraps the unit-style
        //    `FormatKind::Elf` with the default `ElfOptions` (spike
        //    finding; same as the old `download_file_with_options`
        //    auto-conversion).
        //
        //    Use explicitly-named DownloadOptions instead of relying on
        //    `DownloadOptions::default()`. The contract is: paavod
        //    ALWAYS fully flashes the ELF on every job, even if the
        //    operator resubmits the identical binary back-to-back. No
        //    content-based skip, no read-back-based verify shortcut, no
        //    leftover-region preservation. Two identical submits → two
        //    identical flashes; if the chip drifted into a bad state
        //    between runs, the second flash heals it.
        //
        //    Concretely (all defaults today, named here so a future
        //    DownloadOptions field default-flip can't silently change
        //    behavior):
        //
        //      keep_unwritten_bytes: false  — don't try to preserve
        //          flash sectors we don't write. We replace the whole
        //          image every time.
        //      dry_run:              false  — actually flash.
        //      do_chip_erase:        false  — sector-erase is fine;
        //          we don't need (or want) to nuke the whole device.
        //      skip_erase:           false  — don't pretend the
        //          chip was pre-erased; erase before write every time.
        //      preverify:            false  — don't read-back BEFORE
        //          flashing to skip "unchanged" regions. The whole
        //          image gets written even if the chip already
        //          contained byte-identical content.
        //      verify:               false  — don't read-back AFTER
        //          flashing. We rely on the chip's own write-verify
        //          algorithm (every NXP flash algo does this) and
        //          our RTT/defmt drive proves the firmware actually
        //          executed correctly. A separate verify pass would
        //          double the flash-stage time for no incremental
        //          guarantee.
        //      disable_double_buffering: false — let the algo
        //          double-buffer where supported (faster).
        //
        //    User-facing rule, locked in here: "even if the user sends
        //    the same thing, paavod always compiles and flashes."
        let loader = build_loader(
            &mut session,
            &opts.elf_path,
            Format::from(FormatKind::Elf),
            None,
        )
        .map_err(|e| {
            ProbeError::ProbeRs(format!("build_loader {}: {e}", opts.elf_path.display()))
        })?;
        let boot_info = loader.boot_info();

        let mut download_opts = DownloadOptions::default();
        download_opts.keep_unwritten_bytes = false;
        download_opts.dry_run = false;
        download_opts.do_chip_erase = false;
        download_opts.skip_erase = false;
        download_opts.preverify = false;
        download_opts.verify = false;
        download_opts.disable_double_buffering = false;
        loader
            .commit(&mut session, download_opts)
            .map_err(|e| ProbeError::ProbeRs(format!("flash {}: {e}", opts.elf_path.display())))?;

        // 4. Boot the loaded RAM image (unless caller asked to skip —
        //    RT685S quirk; see spec §17 "Deferred from M7").
        //
        //    `loader.commit()` already left the core reset_and_halted
        //    when `boot_info` is `FromRam` (FlashLoader::commit does an
        //    internal `reset_and_halt(500ms)` before writing RAM regions
        //    — visible as the FIRST `reset_and_halt{timeout=500ms}`
        //    line in probe-rs INFO output). PC currently points to the
        //    chip's flash reset vector, which is NOT our code: our
        //    `Reset` symbol lives in RAM at `vector_table_addr + 4`.
        //
        //    `Session::prepare_running_on_ram(vector_table_addr)`:
        //      - reads `*vector_table_addr` → writes to SP_main
        //      - reads `*(vector_table_addr + 4)` → writes to PC
        //      - writes `vector_table_addr` to VTOR
        //    so a subsequent `core.run()` executes our `Reset` handler
        //    with the correct stack pointer and vector table base. It
        //    does NOT issue a hardware reset, so flash content is
        //    irrelevant. This mirrors what `probe-rs run` and
        //    `cargo-embed` do for RAM-resident binaries.
        //
        //    If `boot_info` is `Other`, the ELF is flash-resident — the
        //    paavo templates do not produce flash-resident ELFs (they
        //    use `link_ram_cortex_m.x`), so this is a misconfiguration
        //    we surface clearly. Adding a flash-resident path back is
        //    intentionally deferred until a kind needs it; reintroducing
        //    `reset_and_halt + run` here without auto-detect would
        //    silently mask the RAM-resident bug fixed in this commit.
        if !opts.skip_post_load_reset {
            match boot_info {
                BootInfo::FromRam {
                    vector_table_addr, ..
                } => {
                    tracing::info!(
                        vector_table_addr = format!("{vector_table_addr:#010x}"),
                        "RAM-resident ELF; calling prepare_running_on_ram (sets SP/PC/VTOR \
                         from vector table; no hardware reset)"
                    );
                    session
                        .prepare_running_on_ram(vector_table_addr)
                        .map_err(|e| {
                            ProbeError::ProbeRs(format!(
                                "prepare_running_on_ram {vector_table_addr:#010x}: {e}"
                            ))
                        })?;
                    let mut core = session.core(0).map_err(|e| {
                        ProbeError::ProbeRs(format!("session.core(0) for run: {e}"))
                    })?;
                    core.run()
                        .map_err(|e| ProbeError::ProbeRs(format!("core.run: {e}")))?;
                }
                BootInfo::Other => {
                    return Err(ProbeError::ProbeRs(format!(
                        "ELF {} is flash-resident (probe-rs reports BootInfo::Other), \
                         but paavo expects RAM-resident ELFs linked with \
                         `link_ram_cortex_m.x` (vector table at ORIGIN(RAM)). Either \
                         relink the test crate against the RAM linker script, or use \
                         the `cargo generate` template under `templates/` which already \
                         does this. See `templates/shared/link_ram_cortex_m.x`.",
                        opts.elf_path.display()
                    )));
                }
            }
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
        // Box::leak gives the Table a 'static lifetime so we can store
        // its dependent decoder on Self without a self-referencing
        // struct. The leak is bounded by jobs-per-process (paavod
        // constructs one RealSession per job and drops on completion).
        // See struct doc for the M8 follow-up.
        let table_static: &'static Table = Box::leak(Box::new(table));
        let decoder = table_static.new_stream_decoder();

        // 6. Wait briefly for firmware to initialise RTT, then attach.
        //    The 200ms is empirically necessary (spike finding); attaching
        //    sooner errors with `ControlBlockNotFound`.
        //
        //    Use `Rtt::attach_at` with the address looked up from the
        //    ELF's `_SEGGER_RTT` symbol — NOT `Rtt::attach()` which
        //    scans RAM for the SEGGER magic. The blind scan can latch
        //    onto a stale or duplicated `SEGGER RTT\0...` byte
        //    sequence elsewhere in RAM (e.g. left over from a previous
        //    boot, or coincidentally present in a constant), then
        //    interpret garbage as a valid control block. That manifests
        //    as `up_channels` looking normal (a single "defmt" entry
        //    with buf=1024) but `read()` returning a tiny pile of bytes
        //    that decode as 5 rapid-fire "malformed frame skipped"
        //    warnings. probe-rs `run` does the same — it grabs the
        //    address from the ELF (via `find_rtt_control_block_in_raw_file`
        //    upstream, which isn't published in 0.31.0 yet — so we
        //    duplicate the trivial symbol lookup here) and calls
        //    `attach_at`. See the M7.7 manual smoke for the byte-level
        //    evidence.
        let rtt_block_addr = find_segger_rtt(&elf_bytes).map_err(|e| {
            ProbeError::ProbeRs(format!(
                "find _SEGGER_RTT in elf {}: {e}",
                opts.elf_path.display()
            ))
        })?;
        tracing::info!(
            address = format!("{rtt_block_addr:#010x}"),
            "found _SEGGER_RTT control block in elf"
        );
        std::thread::sleep(Duration::from_millis(200));
        let rtt = {
            let mut core = session
                .core(0)
                .map_err(|e| ProbeError::ProbeRs(format!("session.core(0) for rtt: {e}")))?;
            Rtt::attach_at(&mut core, rtt_block_addr).map_err(|e| {
                ProbeError::ProbeRs(format!(
                    "rtt attach_at {rtt_block_addr:#010x}: {e} (firmware probably \
                     hasn't initialised RTT yet — ensure main() touches defmt-rtt \
                     before doing anything slow)"
                ))
            })?
        };

        // Log the RTT channel layout at INFO. The defmt-rtt convention is
        // a single up channel named "defmt"; if firmware has more channels
        // (e.g. SEGGER RTT terminal alongside defmt) or the channel name
        // doesn't match, this surfaces it in the operator's terminal
        // without requiring trace-level logging. The M7.7 manual smoke
        // turned up a case where index-0 read raw rzcobs-looking bytes
        // that didn't decode — being able to inspect channel names + IDs
        // is the first step to diagnosing whether we're reading the
        // wrong channel.
        let up_summary: Vec<String> = rtt
            .up_channels
            .iter()
            .enumerate()
            .map(|(i, c)| format!("[{i}] name={:?} buf={}B", c.name(), c.buffer_size()))
            .collect();
        tracing::info!(
            up_count = rtt.up_channels.len(),
            up_channels = ?up_summary,
            "RTT attached"
        );

        // Find the "defmt" up channel by name. defmt-rtt always names
        // its channel "defmt"; if firmware also registers other up
        // channels (SEGGER terminal etc.), index 0 is not necessarily
        // the defmt one and reading from it produces non-defmt bytes
        // that all decode as Malformed.
        let defmt_channel_index = rtt
            .up_channels
            .iter()
            .position(|c| c.name() == Some("defmt"))
            .ok_or_else(|| {
                ProbeError::ProbeRs(format!(
                    "no RTT up channel named \"defmt\" found; channels: {up_summary:?}. \
                     Ensure the test crate links defmt-rtt (`use defmt_rtt as _;`)."
                ))
            })?;
        tracing::info!(defmt_channel_index, "selected RTT channel for defmt decode");

        // Set the channel to BlockIfFull mode, matching what probe-rs's
        // `probe-rs run` does. defmt-rtt's default mode (set at static
        // init time in defmt-rtt/src/lib.rs:92) is
        // `MODE_NON_BLOCKING_TRIM` — which means when the RTT buffer
        // is full, the firmware silently TRIMS the new write. Frames
        // never survive an unread buffer wrap. probe-rs hardens against
        // this by flipping the flag to BlockIfFull on attach: firmware
        // blocks until the host drains, so data is never silently lost.
        //
        // Without this, the M7.7 manual smoke produced 5 "malformed
        // frame skipped" warnings against bytes that looked like
        // rzcobs but didn't match the symbol table — most likely
        // partial frames from a buffer wrap that we never noticed in
        // non-blocking mode. We mirror probe-rs's choice here.
        {
            let mut core = session
                .core(0)
                .map_err(|e| ProbeError::ProbeRs(format!("session.core(0) for set_mode: {e}")))?;
            if let Some(ch) = rtt.up_channels.get(defmt_channel_index) {
                match ch.set_mode(&mut core, ChannelMode::BlockIfFull) {
                    Ok(()) => {
                        tracing::info!("set defmt RTT channel mode to BlockIfFull");
                    }
                    Err(e) => {
                        // Soft-fail: log but continue. Some firmwares may
                        // have a read-only flags field (unusual but
                        // possible). If the mode was already blocking by
                        // other means, the decode path will still work.
                        tracing::warn!(
                            error = %e,
                            "could not set defmt RTT channel to BlockIfFull; \
                             continuing in firmware-default mode (likely \
                             NonBlockingTrim — may lose frames on buffer wrap)"
                        );
                    }
                }
            }
        }

        // Size the read buffer to the defmt up-channel's buffer; default
        // to 1024 (what defmt-rtt uses) as a floor.
        let buf_size = rtt
            .up_channels
            .get(defmt_channel_index)
            .map(|c| c.buffer_size().max(256))
            .unwrap_or(1024);

        Ok(Self {
            session,
            rtt,
            defmt_channel_index,
            decoder,
            rtt_buf: vec![0u8; buf_size],
            seen_bkpt: false,
            seq: 0,
            started_at: Instant::now(),
        })
    }

    /// Read one chunk of bytes off the first up channel, feed into the
    /// defmt decoder, and emit the first decoded frame (if any) as a
    /// `LogFrame`. Returns `Ok(None)` if the channel had no bytes AND
    /// the decoder's internal buffer holds no complete frame.
    ///
    /// Strategy: drain decoder first (it may still hold complete frames
    /// from a previous `received()`), then read more RTT bytes, then
    /// drain again. `Malformed` is skipped (defmt-1.0 can produce these
    /// at framing-buffer wrap; surfacing as Error would kill the job).
    fn poll_rtt_once(&mut self) -> Result<Option<Event>> {
        // 1. Try to pull a frame already decoded from buffered bytes.
        if let Some(evt) = drain_one_frame(&mut self.decoder, &mut self.seq, self.started_at) {
            return Ok(Some(evt));
        }

        // 2. Read more bytes from the defmt up channel (looked up by
        //    name at connect; see `defmt_channel_index`). Reading from
        //    `up_channels.first_mut()` is NOT safe — firmwares that
        //    register multiple up channels (e.g. a SEGGER terminal at
        //    index 0, defmt-rtt's channel at index 1) would give us
        //    non-defmt bytes that the rzcobs decoder would reject as
        //    Malformed for every frame. Surfaced in the M7.7 manual
        //    smoke as 5 rapid "defmt malformed frame skipped" warnings
        //    against bytes `00 05 7e 00 06 7e 00 04 7e 00 03 7e 00 02
        //    7e 00` (the latter looks like rzcobs but doesn't match
        //    any string-table entry — wrong channel).
        let n = {
            let Some(ch) = self.rtt.up_channels.get_mut(self.defmt_channel_index) else {
                return Ok(None);
            };
            let mut core = self
                .session
                .core(0)
                .map_err(|e| ProbeError::ProbeRs(format!("core(0) for rtt: {e}")))?;
            ch.read(&mut core, &mut self.rtt_buf)
                .map_err(|e| ProbeError::ProbeRs(format!("rtt read: {e}")))?
        };
        if n == 0 {
            return Ok(None);
        }
        // Trace-log the raw RTT chunk as hex. Off by default; enable
        // with RUST_LOG=paavo_probe=trace to debug defmt framing
        // mismatches (e.g. "all frames malformed" → check whether the
        // bytes look like a defmt frame start at all, or whether they
        // are some other channel's traffic).
        if tracing::enabled!(tracing::Level::TRACE) {
            let hex: String = self.rtt_buf[..n]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            tracing::trace!(bytes = n, "rtt chunk: {hex}");
        }
        self.decoder.received(&self.rtt_buf[..n]);

        // 3. Try again now that we have more bytes.
        Ok(drain_one_frame(
            &mut self.decoder,
            &mut self.seq,
            self.started_at,
        ))
    }
}

impl ProbeSession for RealSession {
    fn next_event(&mut self, timeout_ms: u32) -> Result<Option<Event>> {
        // Order matters: drain RTT BEFORE the halt check so an in-flight
        // `Test OK` frame doesn't get hidden by an over-eager `Bkpt`
        // return. paavo-runner's drive_session needs LogFrame(Test OK)
        // → Bkpt in that order to flag a pass.
        if let Some(evt) = self.poll_rtt_once()? {
            return Ok(Some(evt));
        }

        if !self.seen_bkpt {
            let halted = {
                let mut core = self
                    .session
                    .core(0)
                    .map_err(|e| ProbeError::ProbeRs(format!("core(0): {e}")))?;
                core.status()
                    .map_err(|e| ProbeError::ProbeRs(format!("core.status: {e}")))?
                    .is_halted()
            };
            if halted {
                self.seen_bkpt = true;
                return Ok(Some(Event::Bkpt));
            }
        }

        // Idle: sleep up to `timeout_ms` (capped at 50ms slice) so the
        // calling worker stays responsive to its watchdog stop. Returning
        // `Ok(None)` is contract-correct: drive_session treats it as
        // "no event this tick, loop back to watchdog check." The
        // trait-level docstring on `next_event` documents that
        // `timeout_ms` is a hint, not a hard upper bound.
        //
        // TODO(M8) — see spec §17 "Deferred from M7": detect
        // `Event::Panic` by recognising the `panic-probe` defmt frame
        // pattern, and `Event::Disconnect` by turning probe-rs USB-drop
        // errors from `core(0)` / `rtt.read` into that variant instead
        // of a `ProbeError::ProbeRs`.
        let slice = std::cmp::min(50u32, timeout_ms.max(1));
        std::thread::sleep(Duration::from_millis(slice as u64));
        Ok(None)
    }
}

/// Pull one decoded frame off the decoder and convert it to an
/// `Event::LogFrame`. Returns `None` on `UnexpectedEof` (need more
/// bytes), `Malformed` (skipped with a warn; bounded retry — see
/// `MAX_MALFORMED_SKIPS`), or after exhausting the skip budget.
///
/// Locate the address of `defmt-rtt`'s `_SEGGER_RTT` control-block
/// symbol inside a parsed ELF, so that `Rtt::attach_at` can target it
/// directly instead of scanning RAM.
///
/// Returns the symbol's virtual address. The bookkeeping is the same
/// thing probe-rs's published 0.31.0 `rtt::find_rtt_control_block_in_raw_file`
/// would do, but that helper is post-0.31.0 and we don't want to git-pin
/// probe-rs just for this. Pre-validates that the symbol is defined
/// (has a section index), not merely declared, so we don't hand
/// `attach_at` a garbage 0 pointer that would scan from address 0.
fn find_segger_rtt(elf: &[u8]) -> std::result::Result<u64, String> {
    use object::{Object, ObjectSymbol};
    let file = object::File::parse(elf).map_err(|e| format!("parse elf: {e}"))?;
    for sym in file.symbols() {
        let Ok(name) = sym.name() else { continue };
        if name != "_SEGGER_RTT" {
            continue;
        }
        if sym.section_index().is_none() {
            // Symbol is declared but not defined in this ELF — that
            // means defmt-rtt isn't actually linked in (the user
            // probably forgot `use defmt_rtt as _;`). Surface that
            // specifically rather than continuing to scan.
            return Err("ELF declares `_SEGGER_RTT` but does not define it; \
                 the test crate must link `defmt-rtt` (`use defmt_rtt as _;`)"
                .into());
        }
        return Ok(sym.address());
    }
    Err("ELF has no `_SEGGER_RTT` symbol; the test crate must link \
         `defmt-rtt` (`use defmt_rtt as _;`)"
        .into())
}

/// Free function (not a method on `RealSession`) because of a real
/// borrow conflict: `decoder.decode()` returns a `Frame<'_>` that
/// reborrows `&mut self.decoder` for its lifetime. Bumping `self.seq`
/// and reading `self.started_at` inside the match arm would be a
/// second `&mut self` / `&self` borrow that the checker can't prove
/// disjoint through method calls (field-disjoint borrowing works for
/// direct field access but not across `self.method(&frame)`). Taking
/// the three pieces as independent args sidesteps it.
fn drain_one_frame(
    decoder: &mut Box<dyn StreamDecoder + Send + Sync + 'static>,
    seq: &mut u64,
    started_at: Instant,
) -> Option<Event> {
    // Bounded retry on Malformed: defmt-decoder advances its internal
    // cursor past each bad frame, so a later good frame may still be
    // decodable from the same buffered bytes — but if a long run of
    // garbage shows up (USB glitch, framing-buffer wrap pathology),
    // we don't want to hot-spin inside one `next_event` call and
    // starve the watchdog. After this many skips we return `None`;
    // the next `next_event` call retries after a sleep slice.
    const MAX_MALFORMED_SKIPS: usize = 16;
    for _ in 0..MAX_MALFORMED_SKIPS {
        match decoder.decode() {
            Ok(frame) => {
                let level = match frame.level() {
                    Some(DefmtLevel::Trace) => LogLevel::Trace,
                    Some(DefmtLevel::Debug) => LogLevel::Debug,
                    Some(DefmtLevel::Info) => LogLevel::Info,
                    Some(DefmtLevel::Warn) => LogLevel::Warn,
                    Some(DefmtLevel::Error) => LogLevel::Error,
                    None => LogLevel::Info,
                };
                let message = frame.display_message().to_string();
                // Frame is dropped here when we exit the match arm; the
                // mutable borrow on `decoder` ends with it.
                let this_seq = *seq;
                *seq += 1;
                // `Duration::as_micros() -> u128`; the `u64::try_from`
                // can only fail after ~584,000 years of uptime, but
                // saturating is cheaper than panicking and matches
                // the rest of paavo's "no panic in hot paths" rule.
                let ts_us = u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
                // Debug-trace every decoded frame so a failure like
                // "bkpt without preceding Test OK" can be diagnosed
                // from logs alone — we can see the exact level + message
                // bytes the decoder produced, and whether the OK frame
                // ever arrived. Off by default; enable with
                // RUST_LOG=paavo_probe=debug.
                tracing::debug!(
                    seq = this_seq,
                    ts_us,
                    ?level,
                    message_len = message.len(),
                    "defmt frame: {message:?}"
                );
                return Some(Event::LogFrame(LogFrame {
                    seq: this_seq,
                    ts_us,
                    level,
                    target: None,
                    message,
                }));
            }
            Err(DecodeError::UnexpectedEof) => return None,
            Err(DecodeError::Malformed) => {
                tracing::warn!("defmt malformed frame skipped");
                continue;
            }
        }
    }
    tracing::warn!(
        skipped = MAX_MALFORMED_SKIPS,
        "defmt malformed-skip budget exhausted in one next_event call; \
         yielding so the watchdog gets a tick"
    );
    None
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

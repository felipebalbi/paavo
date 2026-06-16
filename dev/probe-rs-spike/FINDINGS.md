# probe-rs API spike findings (paavo M7.0)

**Date**: 2026-06-15  
**Hardware**: NXP MCX-A266 EVK (FRDM-MCXA266), MCU-Link on-board CMSIS-DAP V3.172  
**Host**: Windows 11, paavo dev box  
**probe-rs version**: 0.31  
**Ran**: 1 end-to-end success (after 4 API-correctness fixes during compile-debug)

This document is the **wire** between sub-task 7.0 (this spike) and sub-tasks
7.4-7.5 (`RealSession::connect`, `RealSession::next_event`). The actual
spike code lives at `dev/probe-rs-spike/`; the test fixture at
`dev/spike-fixture-mcxa266/`. Both are workspace-excluded and never ship.

## End-to-end result

```
Step 1 (enumerate)              ✓ 3 probes seen; MCU-Link matched by serial
Step 2 (open by selector)       ✓ CMSIS-DAP V2 with bulk endpoints
Step 3 (Probe::attach)          ✓ chip name MCXA276 (NOT MCXA266 / MCXA256)
Step 4 (download_file)          ✓ 314 KiB ELF flashed in 735 ms
Step 5 (reset_and_halt + run)   ✓ halted at vector, then resumed
Step 6 (Rtt::attach + read)     ✓ control block @ 0x20000008, 16 B drained
```

The 16 B that came back were defmt-1.0-framed bytes from 5 `info!` calls,
so the pipe is *demonstrably* working — what's missing is the *decode*.
That's the locked-in scope for sub-task 7.5.

## API findings (paste-ready for paavo-probe)

These four are corrections to what we assumed before the spike. They go
directly into `paavo-probe::session::RealSession`:

```rust
// 1. DebugProbeSelector has 4 PUBLIC fields. The `interface` field is
//    easy to forget (we did). Always set it to `None` for CMSIS-DAP v2.
use probe_rs::probe::DebugProbeSelector;
let selector = DebugProbeSelector {
    vendor_id: 0x1fc9,                              // NXP
    product_id: 0x0143,                             // MCU-Link CMSIS-DAP
    interface: None,                                // ← required field
    serial_number: Some("EDFHUAFM4J5ZJ".into()),    // optional, but use it
};

// 2. DebugProbeInfo::probe_type() is a method, not a field.
let info = lister.list_all();
println!("{:?}", info[0].probe_type());             // not `info[0].probe_type`

// 3. Format::Elf is NOT a unit variant — it takes ElfOptions. The
//    cleanest path is FormatKind::Elf, which `download_file` auto-wraps
//    via `From<FormatKind> for Format` (defaulting ElfOptions).
use probe_rs::flashing::{download_file, FormatKind};
download_file(&mut session, &elf_path, FormatKind::Elf)?;

// 4. Send/Sync (no source changes needed, but document):
//    Session: Send + !Sync     ← matches existing ProbeSession trait
//    Probe:   Send + !Sync
//    Lister:  !Send + !Sync    ← keep on the calling thread, do not
//                                share between threads
```

## Chip-name trap (must document)

NXP markets the part as **MCX-A266** (or **MCX A266VFL**), but probe-rs's
built-in target list does not contain `MCXA266` or `MCXA256`. The closest
match — and the one that works — is **`MCXA276`**.

```
$ probe-rs chip list | grep MCXA
MCXA142..146, 152..156, 165, 166, 175, 176, 275, MCXA276, MCXA577
```

The MCX-A266 is the 266 MHz dual-core variant of the same MCXA2xx family;
its FLASH/RAM map and Cortex-M33 cores are identical to MCXA276 from
probe-rs's perspective. **Action**: paavo's `boards.toml.example` and the
deployment doc must hard-document `chip_name = "MCXA276"` for the
MCX-A266 EVK.

## Non-fatal warning

probe-rs emits this twice (during attach and during reset):

```
WARN probe_rs::vendor::nxp::sequences::mcx: unknown variant, using default
     watchpoint configuration
```

This is cosmetic. Flashing and RTT both work fine. Filter it out of
RealSession's tracing output OR document it as expected. Don't surface
it to the operator as an error.

## RTT control-block scan

```rust
// Rtt::attach(&mut core) scans target RAM for the `SEGGER RTT` magic
// string and returns the populated Rtt with up/down channels.
//
// Found in our fixture: control block @ 0x20000008
//                       1 up channel, name="defmt", buffer=1024 B
//                       0 down channels
//
// We do NOT need attach_at() or attach_region() for paavo's first cut —
// the auto-scan works. (Future M8: pass a region hint for faster scan
// and less probe traffic.)
let rtt = probe_rs::rtt::Rtt::attach(&mut core)?;
let up = rtt.up_channels.first_mut().ok_or_else(|| anyhow!("no up ch"))?;
let n = up.read(&mut core, &mut buf)?;   // non-blocking; returns Ok(0) when empty
```

**Timing**: the spike sleeps 200 ms after `core.run()` before calling
`Rtt::attach`. This is empirically enough for the defmt-rtt `_SEGGER_RTT`
block to be initialised by `panic_probe`'s pre-main hook. If we attach
sooner we get `Error::ControlBlockNotFound`. Document this in
RealSession::connect as a known-good delay.

## defmt 1.0 framing is COMPACT

5 calls to `info!("...")` (3 short strings + 2 long ones, total ASCII
text ≈ 130 bytes) produced **16 bytes of RTT data**. Raw preview:

```
"..~..~..~..~..~."        # 5 frames; 0x7E is the defmt-1.0 SOF byte
```

You cannot string-match `"Test OK"` in this stream. You MUST run it
through `defmt_decoder::StreamDecoder` against the `Table` parsed from
the ELF's `.defmt` section.

```rust
use defmt_decoder::{Table, StreamDecoder};

let elf_bytes = std::fs::read(&elf_path)?;
let table = Table::parse(&elf_bytes)?
    .ok_or_else(|| anyhow!("ELF has no .defmt section"))?;
let mut decoder = table.new_stream_decoder();

// in the read loop:
decoder.received(&buf[..n]);
loop {
    match decoder.decode() {
        Ok(frame) => {
            // frame.display(false).to_string() → "Test OK"
            // frame.level()  → Some(Level::Info)
            // frame.timestamp() → uptime
        }
        Err(DecodeError::UnexpectedEof) => break,   // need more bytes
        Err(DecodeError::Malformed) => { /* skip / log */ }
    }
}
```

## Cargo.toml dep shape for paavo-probe

Currently `paavo-probe/Cargo.toml` (M2.1) only has `object` for the ELF
section parser. M7.4 will add:

```toml
[dependencies]
probe-rs       = { workspace = true }
defmt-decoder  = { workspace = true }
```

Workspace already pins both at `0.31` and `1.1.0`. No version churn.

## Build flags for the test crate

The spike fixture compiles with:

```toml
[build]
target = "thumbv8m.main-none-eabihf"

[target.thumbv8m.main-none-eabihf]
rustflags = [
  "-C", "link-arg=-Tlink.x",      # cortex-m-rt's linker script
  "-C", "link-arg=-Tdefmt.x",     # defmt's relocation section
]

[env]
DEFMT_LOG = "info"                # filter at compile time
```

```toml
# Cargo.toml
[profile.release]
debug = 2          # full debug info — REQUIRED for defmt-decoder
                   # to find function names + .defmt section
```

Note: `debug = 2` is the spike fixture's release profile. paavo's
existing build sandbox (paavo-build) runs `cargo build --release`
with the test crate's Cargo.toml — so the test crate is responsible
for keeping debug info on. Document this in the template README and
in the deployment guide.

## What sub-tasks 7.4 / 7.5 inherit

| Item                                  | Decision (locked) |
|---------------------------------------|-------------------|
| Probe selector by VID/PID/serial      | yes — match the MCU-Link by serial |
| Chip name for MCX-A266                | `"MCXA276"` (document the trap) |
| Flash format                          | `FormatKind::Elf` |
| Reset sequence                        | branch on `loader.boot_info()` — see footnote ¹ |
| RTT attach mode                       | `Rtt::attach(&mut core)` (auto-scan RAM) |
| Up-channel selection                  | `rtt.up_channels.first_mut()` (one channel) |
| Up-channel buffer size                | 1024 B; spike uses `[u8; 1024]` |
| Polling interval (no data)            | 50 ms sleep + retry |
| Pre-RTT-attach delay                  | 200 ms after `core.run()` |
| defmt decoding                        | mandatory; `defmt_decoder::Table` + `StreamDecoder` |
| Vendor-WARN suppression               | filter or document; do not surface as error |
| Send/Sync bounds on `ProbeSession`    | no change — `Send` only matches Session |

¹ **Reset sequence — corrected after M7.7 RAM-resident bug**:
the spike originally locked in `reset_and_halt(2s) + core.run()` after
`download_file`. That was correct *for the spike fixture*
(`dev/spike-fixture-mcxa266/memory.x` defines both `FLASH 0x00000000 1M`
and `RAM 0x20000000 128K`, and `dev/spike-fixture-mcxa266/.cargo/config.toml`
links with `-Tlink.x` → vector table sits in flash). Cortex-M reset
correctly jumps to that flash vector and runs the user code.

paavo's actual templates (`templates/*/memory.x` +
`templates/shared/link_ram_cortex_m.x`) are **RAM-resident**: they define
only RAM and link the vector table at `ORIGIN(RAM) = 0x20000000`. After
`reset_and_halt`, the chip's PC goes to the *flash* reset vector
(boot ROM / leftover firmware) — NOT to the RAM-loaded `Reset`
function — so `core.run()` runs the wrong code. Symptom: RTT attaches
to the (stale) `_SEGGER_RTT` block in RAM, no defmt frames ever come
out because firmware never executes, inactivity timeout fires after 2
minutes. (See `crates/paavo-probe/src/session.rs::RealSession::connect`
step 4.)

`paavo-probe` therefore branches on `FlashLoader::boot_info()`:

  - `BootInfo::FromRam { vector_table_addr, .. }` →
    `session.prepare_running_on_ram(vector_table_addr)` (sets SP_main,
    PC, and VTOR from the vector table; **no** hardware reset) +
    `core.run()`. Mirrors what `probe-rs run` and `cargo-embed` do.
  - `BootInfo::Other` → return a `ProbeError` pointing the operator at
    `templates/shared/link_ram_cortex_m.x`. paavo no longer supports
    flash-resident ELFs.

Side effect: the existing `cargo test -p paavo-probe --test
real_session_connect -- --ignored --nocapture` hardware test (which
points at the spike fixture, flash-resident) now fails with that
explicit error until the spike fixture is relinked against
`link_ram_cortex_m.x`. That test is `#[ignore]`-d and `PAAVO_HW=1`-gated,
so it does not affect default `cargo test`.

## Reproducing the spike

```pwsh
# in dev/probe-rs-spike
cargo build --release

# in dev/spike-fixture-mcxa266
cargo build --release    # cross-compiles to thumbv8m.main-none-eabihf

# from dev/probe-rs-spike
.\target\release\probe-rs-spike.exe `
  --elf "..\spike-fixture-mcxa266\target\thumbv8m.main-none-eabihf\release\spike-fixture-mcxa266" `
  --chip MCXA276 `
  --read-secs 5
```

Probe defaults (`vid=0x1fc9 pid=0x0143 serial=EDFHUAFM4J5ZJ`) match the
MCU-Link on the MCX-A266 EVK on Felipe's desk. Different EVK → pass
`--serial <yours>` or `--serial ""` to skip serial matching.

//! Hardware-only regression pin for the post-M7.7 RAM-resident boot
//! path.
//!
//! `RealSession::connect` now branches on `FlashLoader::boot_info()`:
//! `BootInfo::FromRam` calls `Session::prepare_running_on_ram` (sets
//! SP/PC/VTOR from the in-RAM vector table; no hardware reset),
//! `BootInfo::Other` returns an error pointing at
//! `link_ram_cortex_m.x`. See `crates/paavo-probe/src/session.rs`
//! step 4 and commit 3bc58f2 for the rationale.
//!
//! This test exists so a future "always reset_and_halt" rewrite breaks
//! a test instead of breaking flashing silently. We capture every
//! `tracing` event emitted while `connect()` runs and assert that the
//! info-level event with target `paavo_probe::session` and message
//! "RAM-resident ELF; calling prepare_running_on_ram (...)" fired,
//! carrying `vector_table_addr = "0x20000000"`. That address is the
//! RAM ORIGIN of the spike fixture's `memory.x` (which mirrors the
//! production `templates/mcxa266/memory.x`); a regression that
//! reverts to the flash-resident reset_and_halt+run path would emit
//! a different log (or no log) and fail the assertions here.
//!
//! Gated identically to `real_session_connect.rs`:
//!   - `#[ignore]` so default `cargo test --workspace` skips it.
//!   - `PAAVO_HW=1` env var so dev boxes without the EVK plugged in
//!     self-skip without surfacing as failure.
//!
//! Depends on the spike fixture ELF; build it first by `cd`-ing INTO
//! the fixture directory (the `.cargo/config.toml` there carries the
//! linker flags needed for a clean RAM-resident, defmt-instrumented
//! ELF). See `real_session_connect.rs` for the full rationale.
//!
//!   cd dev/spike-fixture-mcxa266
//!   cargo build --release
//!
//! Run with:
//!   $env:PAAVO_HW = "1"
//!   cargo test -p paavo-probe --test real_session_ram_boot \
//!       -- --ignored --nocapture

use paavo_probe::{RealSession, RealSessionOptions};
use paavo_proto::ProbeSelector;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::Layer;

/// Single captured tracing event.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    level: tracing::Level,
    /// Field name → string-rendered value. The synthetic `message`
    /// field carries the event's static message text; user-supplied
    /// fields like `vector_table_addr` land here under their declared
    /// names.
    fields: HashMap<String, String>,
}

impl CapturedEvent {
    fn message(&self) -> Option<&str> {
        self.fields.get("message").map(|s| s.as_str())
    }
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(|s| s.as_str())
    }
}

/// `tracing_subscriber::Layer` that pushes each emitted event into a
/// shared `Vec`. Cheap, allocation-only; safe to hold across the
/// hardware-bound `RealSession::connect` call. The `Mutex` is fine —
/// `connect` is single-threaded; the lock is uncontended.
#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldCollector::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(CapturedEvent {
            target: metadata.target().to_string(),
            level: *metadata.level(),
            fields: visitor.0,
        });
    }
}

/// `Visit` impl that stringifies every field. We don't care about
/// preserving original numeric types — the assertions below only
/// compare strings — so a single `record_*` per primitive that
/// formats with `Debug` is fine. `record_str` keeps `&str` values
/// unquoted so `vector_table_addr = format!("{:#010x}", _)` (a
/// `String` recorded via `record_debug` per tracing's value protocol)
/// can be matched by raw substring without worrying about quoting
/// asymmetry.
#[derive(Default)]
struct FieldCollector(HashMap<String, String>);

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // Strip the surrounding quotes that `Debug for String` adds
        // so callers can compare against literal field values without
        // having to know which `record_*` overload tracing picked.
        let raw = format!("{value:?}");
        let unquoted = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .map(|s| s.to_string())
            .unwrap_or(raw);
        self.0.insert(field.name().to_string(), unquoted);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

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
fn connect_takes_ram_boot_path_with_vector_table_at_ram_origin() {
    if !hw_or_skip() {
        return;
    }

    // Install a thread-local capture subscriber for the duration of
    // this test so the assertion sees `RealSession::connect`'s
    // events. `set_default` returns a guard whose Drop restores the
    // previous default, so concurrent tests in this binary aren't
    // affected (paavo-probe's hardware tests are also `--test-threads
    // 1`-friendly because they all want the probe).
    let capture = CaptureLayer::default();
    let events = capture.events.clone();
    let subscriber = tracing_subscriber::registry().with(capture);
    let _guard = tracing::subscriber::set_default(subscriber);

    // The vid/pid/serial below match the MCU-Link on Felipe's MCX-A266
    // EVK. A different EVK on a different dev box: override these
    // three fields (or wrap them in env-var reads). See
    // `dev/probe-rs-spike/FINDINGS.md` for how to enumerate the
    // locally-visible probes.
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
    let session =
        RealSession::connect(opts).expect("connect must succeed against the MCX-A266 EVK");

    // Drop the session BEFORE the assertions so a probe is freed even
    // if the assertions panic — otherwise a failing run leaves the
    // probe bound until the test process is killed.
    drop(session);

    // Drop the subscriber guard now that connect() has returned, so
    // the assertion failures themselves don't get captured into our
    // own buffer (and `events` can be locked freely below).
    drop(_guard);

    let events = events.lock().unwrap();

    // The new RAM-boot log line is emitted at `INFO` from target
    // `paavo_probe::session` with the message:
    //   "RAM-resident ELF; calling prepare_running_on_ram (sets
    //    SP/PC/VTOR from vector table; no hardware reset)"
    // and a single field `vector_table_addr` formatted via
    // `format!("{:#010x}", ...)` → exactly 10 chars, e.g.
    // "0x20000000". The spike fixture's memory.x pins ORIGIN(RAM) =
    // 0x20000000, so the address is fixed.
    let ram_boot_event = events
        .iter()
        .find(|e| {
            e.level == tracing::Level::INFO
                && e.target == "paavo_probe::session"
                && e.message()
                    .map(|m| m.starts_with("RAM-resident ELF; calling prepare_running_on_ram"))
                    .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!(
                "expected an info-level `paavo_probe::session` event with message \
                 'RAM-resident ELF; calling prepare_running_on_ram (...)'. \
                 Captured {} events. If a regression switched RealSession::connect \
                 back to a hardcoded reset_and_halt+run path, this assertion is \
                 doing its job — restore the BootInfo branch in \
                 crates/paavo-probe/src/session.rs step 4. \
                 \nCaptured events:\n{}",
                events.len(),
                summarise_events(&events)
            )
        });

    let addr = ram_boot_event
        .field("vector_table_addr")
        .unwrap_or_else(|| {
            panic!(
                "RAM-resident event present but missing `vector_table_addr` field. \
             Fields on the event: {:?}",
                ram_boot_event.fields.keys().collect::<Vec<_>>()
            )
        });
    assert_eq!(
        addr, "0x20000000",
        "vector_table_addr must equal ORIGIN(RAM) for the spike fixture (see \
         dev/spike-fixture-mcxa266/memory.x: `RAM : ORIGIN = 0x20000000`); \
         got {addr:?}. A different value would mean either the fixture's \
         memory.x changed (intentional → update this test) or the boot path \
         is reading the wrong vector table address (regression)."
    );

    // Negative assertion: the `BootInfo::Other` log line ('flash-
    // resident ELF; reset_and_halt + run' was the wording we
    // explicitly DIDN'T pick — but adjacent variants are still worth
    // asserting absence of so a future contributor who adds back a
    // 'fall back to reset_and_halt' branch and emits both messages
    // breaks the test instead of silently resurrecting the bug).
    let unexpected = events.iter().find(|e| {
        e.target == "paavo_probe::session"
            && e.message()
                .map(|m| m.contains("reset_and_halt") || m.contains("flash-resident"))
                .unwrap_or(false)
    });
    assert!(
        unexpected.is_none(),
        "unexpected reset_and_halt / flash-resident log line during RAM-resident \
         connect — the BootInfo branch may have regressed. Event: {:?}",
        unexpected
    );
}

/// Render captured events into a brief multi-line summary so an
/// assertion failure shows the operator what we did see, in order.
fn summarise_events(events: &[CapturedEvent]) -> String {
    let mut out = String::new();
    for (i, e) in events.iter().enumerate() {
        let msg = e.message().unwrap_or("(no message)");
        out.push_str(&format!(
            "  [{i:>2}] {lvl:5} {tgt}: {msg}\n",
            lvl = e.level,
            tgt = e.target,
        ));
    }
    if out.is_empty() {
        out.push_str("  (no events captured — was a global subscriber already set?)\n");
    }
    out
}

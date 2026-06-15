//! Smoke check for `templates/mcxa266` — textually reads
//! `Cargo.toml.liquid` (no liquid substitution, no scaffold, no
//! cross-compile) and asserts that the M7.1 feature-flag corrections
//! are present and the M6.2 stale flags are gone. The corrections
//! locked in here are:
//!
//! 1. `embassy-mcxa` feature: `mcxa266vfl` (fictional) → `mcxa2xx`
//!    (the real flag per `embassy/embassy-mcxa/Cargo.toml`).
//! 2. `embassy-executor` feature: `arch-cortex-m` → `platform-cortex-m`.
//! 3. `cortex-m-rt` adds `set-sp` + `set-vtor` (required by the
//!    RAM-resident vector-table layout in `memory.x`).
//! 4. `defmt` / `defmt-rtt` / `panic-probe` move from `0.x` to `1`.
//!
//! End-to-end render + cross-compile is deferred to 7.2's `paavo-cli
//! new` integration test (gated under `PAAVO_HW=1`).

#[test]
fn templates_mcxa266_smoke_renders_corrected_feature_flags() {
    let template = std::env::current_dir()
        .unwrap()
        .ancestors()
        .find(|p| p.join("templates/mcxa266/cargo-generate.toml").exists())
        .expect("templates/mcxa266 not found from any ancestor of CWD")
        .join("templates/mcxa266");

    let cargo_toml = std::fs::read_to_string(template.join("Cargo.toml.liquid"))
        .expect("read Cargo.toml.liquid");

    // ─── M7.1 correction #1: embassy-mcxa enables mcxa2xx, not mcxa266vfl
    // The feature list is unordered (cargo treats `[features]` activation
    // as a set), so we look for the substring `"mcxa2xx"` anywhere in the
    // embassy-mcxa dependency line rather than anchoring to position.
    assert!(
        cargo_toml
            .lines()
            .any(|l| l.contains("embassy-mcxa") && l.contains(r#""mcxa2xx""#)),
        "embassy-mcxa must enable the mcxa2xx feature (not mcxa266vfl). \
         Cargo.toml.liquid:\n{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains("mcxa266vfl"),
        "stale mcxa266vfl feature flag must be removed"
    );

    // ─── M7.1 correction #2: embassy-executor uses platform-cortex-m
    assert!(
        cargo_toml.contains(r#""platform-cortex-m""#),
        "embassy-executor must use platform-cortex-m (not arch-cortex-m)"
    );
    assert!(
        !cargo_toml.contains(r#""arch-cortex-m""#),
        "stale arch-cortex-m feature flag must be removed"
    );

    // ─── M7.1 correction #3: cortex-m-rt enables set-sp + set-vtor.
    // Both are load-bearing — without them, the linker layout in
    // memory.x doesn't bring up SP and VTOR before main() and the
    // target hard-faults at the first interrupt.
    assert!(
        cargo_toml.contains(r#""set-sp""#),
        "cortex-m-rt must enable set-sp feature (RAM-resident vector \
         table needs it; see memory.x). Cargo.toml.liquid:\n{cargo_toml}"
    );
    assert!(
        cargo_toml.contains(r#""set-vtor""#),
        "cortex-m-rt must enable set-vtor feature (sibling of set-sp; \
         see memory.x). Cargo.toml.liquid:\n{cargo_toml}"
    );

    // ─── Manual-smoke correction: cortex-m must enable `inline-asm`
    // for thumbv8m.main-none-eabihf. Without it, cortex-m 0.7.7's
    // `__basepri_r/w/max` and `__faultmask_r` functions resolve to a
    // stale code path that doesn't have a thumbv8m flavour, breaking
    // the build with E0425 ("cannot find function `__basepri_r` in
    // module `crate::asm::inline`"). embassy-mcxa requires this same
    // feature on its own cortex-m dep but cargo feature unification
    // doesn't reliably propagate it across the workspace boundary
    // when we add cortex-m as a direct dep at the test crate level.
    // Surfaced during the M7.7 manual smoke; lock it in here so no
    // future template refresh accidentally drops it.
    assert!(
        cargo_toml
            .lines()
            .any(|l| l.contains("cortex-m ") && l.contains(r#""inline-asm""#)),
        "cortex-m must enable the inline-asm feature (required for \
         thumbv8m.main-none-eabihf). Cargo.toml.liquid:\n{cargo_toml}"
    );

    // ─── Manual-smoke correction: edition = "2024" is REQUIRED.
    //
    // Edition 2024 implies `resolver = "3"`, which (like resolver
    // 2 before it) splits feature unification by host vs target.
    // The legacy resolver = "1" (the edition-2021 default) unifies
    // a package's feature set across BOTH the [build-dependencies]
    // (host compile) AND the [dependencies] (target compile) slots.
    // That is fatal for nxp-pac: embassy-mcxa pulls nxp-pac in
    // twice —
    //
    //   [dependencies]       nxp-pac = { ..., features = ["rt"] }
    //   [build-dependencies] nxp-pac = { ..., default-features =
    //                                   false, features = ["metadata"] }
    //
    // With resolver 1, the host-compile of nxp-pac inherits `rt`,
    // which transitively pulls `cortex-m` into the host build. The
    // host-compile of cortex-m then tries to compile `register/
    // basepri.rs` (gated `cfg(all(not(armv6m), not(armv8m_base)))`
    // — true on x86_64) which expands `call_asm!(__basepri_r() ->
    // u8)` to `crate::asm::inline::__basepri_r()`. With
    // `inline-asm` enabled (also unified across host/target under
    // resolver 1), `asm/inline.rs` is included for host and emits
    // `asm!("bkpt #0xab", inout("r0") nr, in("r1") arg, ...)` —
    // which fails on x86 with "invalid register `r0`".
    //
    // The full failure was 6 errors (E0425 ×4 for
    // __basepri_{r,w,max} + __faultmask_r, plus 2 "invalid
    // register" errors for r0/r1).
    //
    // Edition 2024 / resolver 3 fixes this cleanly — the host
    // compile of nxp-pac only gets `metadata` (no cortex-m at
    // all) and the problem disappears. Surfaced during the M7.7
    // manual smoke; lock the edition in here so no future template
    // refresh can accidentally downgrade it back to 2021.
    assert!(
        cargo_toml.contains(r#"edition = "2024""#),
        "Cargo.toml.liquid must set `edition = \"2024\"`. Without it, \
         legacy feature unification (resolver 1) leaks thumb-only \
         cortex-m features into the host compile of nxp-pac \
         (build-dep) and the cortex-m host build fails with E0425. \
         Cargo.toml.liquid:\n{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains(r#"edition = "2021""#),
        "stale `edition = \"2021\"` must be removed (it implies \
         resolver = \"1\" which breaks the host build of nxp-pac)"
    );

    // ─── M7.1 correction #4: defmt family moves to 1.x.
    // The version string is open-ended (`"1"` matches anything ≥ 1.0
    // by cargo's caret semantics), so we negate the stale 0.x pins
    // explicitly rather than trying to write a positive regex for
    // every plausible 1.x shape.
    assert!(
        !cargo_toml.contains(r#"defmt          = "0"#) && !cargo_toml.contains(r#"defmt = "0"#),
        "stale defmt 0.x pin must be removed (defmt-decoder needs 1.x \
         for the spike fixture's framing format). Cargo.toml.liquid:\n{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains(r#"defmt-rtt      = "0"#) && !cargo_toml.contains(r#"defmt-rtt = "0"#),
        "stale defmt-rtt 0.x pin must be removed (must match defmt 1.x). \
         Cargo.toml.liquid:\n{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains(r#"panic-probe    = { version = "0"#)
            && !cargo_toml.contains(r#"panic-probe = { version = "0"#),
        "stale panic-probe 0.x pin must be removed (must match defmt 1.x). \
         Cargo.toml.liquid:\n{cargo_toml}"
    );
    // Positive presence check for defmt 1 (catches a hypothetical revert
    // to `defmt = "2"` if the upstream goes that way before paavo does).
    assert!(
        cargo_toml.contains(r#"defmt          = "1"#) || cargo_toml.contains(r#"defmt = "1"#),
        "defmt must be pinned to 1.x. Cargo.toml.liquid:\n{cargo_toml}"
    );
}

/// `.cargo/config.toml` carries settings that paavod's build path
/// cannot work without. This test guards each one with a focused
/// assertion + a comment naming the exact symptom if it's missing,
/// so the next regression doesn't take another 5-hour debugging
/// session to root-cause.
#[test]
fn templates_mcxa266_smoke_renders_dot_cargo_config() {
    let template = std::env::current_dir()
        .unwrap()
        .ancestors()
        .find(|p| p.join("templates/mcxa266/cargo-generate.toml").exists())
        .expect("templates/mcxa266 not found from any ancestor of CWD")
        .join("templates/mcxa266");

    let cargo_config = std::fs::read_to_string(template.join(".cargo/config.toml"))
        .expect("read .cargo/config.toml — required by paavod's build (see make_tar)");

    // ─── #1: `[build] target = "thumbv8m.main-none-eabihf"`.
    // paavod's build path runs `cargo build --release` with NO
    // `--target` flag (paavo_build::build.rs). cargo picks the
    // host triple unless `.cargo/config.toml` overrides it. Without
    // this line, the host-compile of cortex-m fails with 6 errors
    // (E0425 ×4 for __basepri_{r,w,max} + __faultmask_r, plus 2
    // "invalid register" errors for r0/r1).
    assert!(
        cargo_config.contains(r#"target = "thumbv8m.main-none-eabihf""#),
        ".cargo/config.toml must set `target = \"thumbv8m.main-none-eabihf\"` \
         under `[build]`. Without it, paavod's `cargo build --release` \
         defaults to the host triple and the cortex-m host build fails \
         with E0425. .cargo/config.toml:\n{cargo_config}"
    );

    // ─── #2: `DEFMT_LOG = "trace"` (or at minimum "info").
    // defmt's log-level filter is COMPILE-TIME, controlled by the
    // DEFMT_LOG env var read at build time by defmt's proc-macros.
    // Without it, defmt defaults to filtering everything below
    // ERROR for release builds — which strips `info!("Test OK")`
    // from the ELF entirely. paavo's pass-detection contract
    // requires the info-level `Test OK` frame; if it's missing,
    // paavod's decoder reports "malformed frame skipped" warnings
    // because the only frames in the RTT stream are error-level
    // panics (from panic-probe / embassy-executor) whose symbols
    // don't match the pass contract.
    //
    // Surfaced during the M7.7 manual smoke as 5 "defmt malformed
    // frame skipped" warnings in a row, exactly matching the
    // count of error-level frames that DID compile in.
    assert!(
        cargo_config.contains("DEFMT_LOG"),
        ".cargo/config.toml must set `DEFMT_LOG` under `[env]`. \
         Without it, defmt's compile-time filter strips info-level \
         frames from the ELF and the pass-detection contract's \
         `Test OK` frame never makes it into the binary. \
         .cargo/config.toml:\n{cargo_config}"
    );
    // Permit any level that includes "info" or higher verbosity.
    // The string "info" appears as a prefix of both "info" and
    // (less interestingly) other words, so check the actual setting.
    assert!(
        cargo_config.contains(r#"DEFMT_LOG = "trace""#)
            || cargo_config.contains(r#"DEFMT_LOG = "debug""#)
            || cargo_config.contains(r#"DEFMT_LOG = "info""#),
        "DEFMT_LOG must be set to \"trace\", \"debug\", or \"info\" \
         (anything more restrictive strips the `Test OK` frame from \
         the ELF). .cargo/config.toml:\n{cargo_config}"
    );

    // ─── #3: `-Tdefmt.x` link arg.
    // defmt.x carries defmt-decoder's symbol-relocation section.
    // Without it the ELF has no `.defmt` section at all and the
    // decoder has no symbol table to interpret RTT bytes.
    assert!(
        cargo_config.contains(r#""link-arg=-Tdefmt.x""#),
        ".cargo/config.toml must pass `-Tdefmt.x` to the linker. \
         Without it, the ELF has no `.defmt` section and defmt-decoder \
         can't interpret any RTT frames. .cargo/config.toml:\n{cargo_config}"
    );

    // ─── #4: `--nmagic` link arg.
    // cortex-m-rt's link scripts assume the RAM origin is 0x10000-
    // aligned; without --nmagic the linker emits page-aligned
    // sections that overflow into adjacent regions or leave gaps
    // that confuse cortex-m-rt's startup code.
    assert!(
        cargo_config.contains(r#""link-arg=--nmagic""#),
        ".cargo/config.toml must pass `--nmagic` to the linker \
         (see rust-embedded/cortex-m-quickstart#95). \
         .cargo/config.toml:\n{cargo_config}"
    );

    // ─── #5: `[net] git-fetch-with-cli = true`.
    // libgit2's GitHub clone fails on Windows when a git credential
    // helper is configured. Telling cargo to shell out to `git` for
    // fetches sidesteps the issue and is harmless on Linux.
    assert!(
        cargo_config.contains("git-fetch-with-cli = true"),
        ".cargo/config.toml must set `git-fetch-with-cli = true` \
         under `[net]` (libgit2's GitHub clone fails on Windows when \
         a git credential helper is configured). \
         .cargo/config.toml:\n{cargo_config}"
    );

    // ─── #6: must NOT pass `-Tlink.x`.
    // link.x and link_ram.x both `INCLUDE memory.x` and so both
    // define the same regions — passing both produces "region FLASH
    // already defined" linker errors. build.rs emits -Tlink_ram.x,
    // so .cargo/config.toml must NOT also pass -Tlink.x.
    assert!(
        !cargo_config.contains(r#""link-arg=-Tlink.x""#),
        "stale `-Tlink.x` must not be passed (build.rs already emits \
         `-Tlink_ram.x`; both INCLUDE memory.x and conflict). \
         .cargo/config.toml:\n{cargo_config}"
    );

    // ─── #7: `runner = "probe-rs run --chip MCXA276 ..."`.
    // Without a runner, `cargo run --release` from the scaffold
    // crate fails with "binary file not executable" — cargo doesn't
    // know how to invoke a thumbv8m ELF on the host. Setting probe-rs
    // run as the runner makes the scaffold locally runnable without
    // paavo in the loop ("does the scaffold itself work on the
    // hardware?" is the first question to answer when paavod fails
    // and the user wants to isolate). MCXA276 (NOT MCXA266) is the
    // probe-rs target name for the whole MCX-A2xx family.
    assert!(
        cargo_config.contains("runner = \"probe-rs run"),
        ".cargo/config.toml must set a `runner` invoking probe-rs run \
         so `cargo run --release` works as a local-validation \
         equivalent of `paavo-cli run --follow .`. \
         .cargo/config.toml:\n{cargo_config}"
    );
    assert!(
        cargo_config.contains("--chip MCXA276"),
        ".cargo/config.toml's runner must target chip MCXA276 (NOT \
         MCXA266 / MCXA256 — probe-rs advertises the whole MCX-A2xx \
         family under MCXA276; see dev/probe-rs-spike/FINDINGS.md). \
         .cargo/config.toml:\n{cargo_config}"
    );
}

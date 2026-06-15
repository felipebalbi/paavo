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

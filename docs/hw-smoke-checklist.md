# HW smoke checklist

Run after every release tag, manually, against real hardware. Captures
parts not covered by `cargo test --workspace`.

> Status: this checklist is the contract; the steps have not yet been
> validated end-to-end on real hardware in CI. The first operator to
> run it against an `mcxa266` + an `rt685-evk` should annotate any
> drift between the checklist and observed behavior.

1. Start paavod against an `mcxa266` + an `rt685-evk` board in
   `boards.toml`.
2. `paavo-cli boards` lists both as `healthy`.
3. Submit a passing test:
   ```bash
   paavo-cli new smoke-pass --board-kind mcxa266
   cd smoke-pass
   paavo-cli run . --board-kind mcxa266
   ```
   Confirm: terminal line includes `"passed"`, exit 0, log stream
   printed.
4. Submit a panicking test — replace the body of `src/main.rs` with
   `panic!("smoke");` and:
   ```bash
   paavo-cli run . --board-kind mcxa266
   ```
   Confirm: terminal line includes `"failed":` (object with details),
   exit 1, panic message visible in log.
5. Submit a hanging test (loop with no defmt). Confirm inactivity
   watchdog fires within `~2 × default_inactivity_s`.
6. `kill -TERM $(pidof paavod)` while a job is running. Confirm the
   job ends in `aborted{daemon_shutdown}` within
   `shutdown_grace_s + 5s`.
7. `paavo-web` at `http://127.0.0.1:8081/` shows all of the above.
8. After 3 consecutive `Failed{InfraErr}` outcomes (unplug probe),
   board is auto-quarantined; `paavo-cli boards` shows it as
   `quarantined`.
9. `paavo-cli board unquarantine <id>` brings it back to `healthy`.
10. Cancel a running job mid-flight: `paavo-cli cancel <id>` returns
    `204`; job ends in `aborted{user}`.
11. `paavo-cli admin purge` wipes sandboxes/uploads/cargo-target on
    disk and truncates job/log_frame/build_cache in the DB while
    preserving boards/schedules. Re-running step 3 succeeds.

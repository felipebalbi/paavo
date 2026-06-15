#!/usr/bin/env nu
#
# manual-smoke.nu - end-to-end manual exercise of paavo-cli against a
# locally-running paavod (with PAAVO_FAKE_RUNNER=1).
#
# Prereqs:
#   1. Start paavod in another terminal, with the fake runner:
#        $env.PAAVO_FAKE_RUNNER = "1"
#        cargo run -p paavod -- --config sample-paavo.toml
#   2. Run from D:\workspace\paavo so the cargo workspace resolves.
#
# Usage:
#   nu manual-smoke.nu
#
# The script does, in order:
#   - register 2 boards (mcxa266-01, rt685-01)
#   - submit 2 jobs (one per board kind) using paavo-cli run
#   - poll `jobs` until both reach a terminal state
#   - print the final job table
#   - quarantine + unquarantine one board (exercises that path)
#   - quarantine both boards as "removed" (v1 has no DELETE /boards)

$env.PAAVO_HOST = ($env.PAAVO_HOST? | default "http://127.0.0.1:8090")
print $"using PAAVO_HOST=($env.PAAVO_HOST)"

def --env wait-for-daemon [] {
    mut tries = 0
    while $tries < 30 {
        let up = (try { http get $"($env.PAAVO_HOST)/health"; true } catch { false })
        if $up {
            print "daemon is up"
            return
        }
        sleep 200ms
        $tries = $tries + 1
    }
    error make { msg: $"daemon did not respond at ($env.PAAVO_HOST) after 6s" }
}

wait-for-daemon

# ---------------------------------------------------------------
# 0. Reset to a clean slate.
#    - `admin purge` wipes job artifacts (disk + db) but preserves
#      boards by design (operators normally do not want to re-register
#      probes after a purge — see spec §9.5).
#    - For the smoke loop we *do* want a fresh inventory so the
#      `board add` calls below succeed on every run, so we manually
#      sweep the board table too: quarantine each row (required by
#      the FK guard on DELETE /boards/:id) then remove it. Pull the
#      id list from the JSON wire shape rather than the formatted
#      table that `paavo-cli boards` prints.
#    NOTE: purge refuses (409) if any job is currently building or
#    running. If that happens, cancel them with `paavo-cli cancel <id>`
#    or wait for them to terminate, then re-run.
# ---------------------------------------------------------------
print "\n=== reset state (admin purge + wipe inventory) ==="
cargo run -p paavo-cli -- admin purge
let existing = (try { http get $"($env.PAAVO_HOST)/boards" } catch { [] })
for row in $existing {
    let id = $row.id
    print $"  removing leftover board ($id)"
    cargo run -p paavo-cli -- board quarantine $id --reason "smoke reset"
    cargo run -p paavo-cli -- board remove $id
}

# ---------------------------------------------------------------
# 1. Register 2 boards.
# ---------------------------------------------------------------
print "\n=== register boards ==="
cargo run -p paavo-cli -- board add --kind mcxa266 --instance mcxa266-01 --probe 1366:1015:ABC123 --chip MCXA266VFL --target frdm-mcx-a266
cargo run -p paavo-cli -- board add --kind rt685-evk --instance rt685-01 --probe 1366:1015:DEF456 --chip MIMXRT685S --target frdm-rt685-evk
print "\n--- boards after add:"
cargo run -p paavo-cli -- boards

# ---------------------------------------------------------------
# 2. Submit 2 jobs (one per board kind). Use the dedicated
#    cross-compiled fixture under tests/fixtures/smoke-crate so the
#    upload + tar + cargo-build path exercises a real
#    target-thumbv8m.main-none-eabihf ELF (paavo-build's elf
#    discovery requires real ELF magic, not a host PE/COFF .exe).
#    FakeRunner returns Passed instantly once the build resolves.
# ---------------------------------------------------------------
print "\n=== submit jobs ==="
let crate_dir = "tests/fixtures/smoke-crate"
print $"submitting ($crate_dir) -> mcxa266"
cargo run -p paavo-cli -- run $crate_dir --board-kind mcxa266 --timeout 5m
print $"submitting ($crate_dir) -> rt685-evk"
cargo run -p paavo-cli -- run $crate_dir --board-kind rt685-evk --timeout 5m

# Note: `paavo-cli run` blocks on the log stream until terminal. So by
# the time the two `run` calls return, both jobs are already terminal.
# We still poll below to demonstrate the query shape and to make the
# script robust against future changes.

# ---------------------------------------------------------------
# 3. Poll until both jobs hit a terminal state.
# ---------------------------------------------------------------
print "\n=== poll job state ==="
let terminal_states = [passed failed timedout aborted]

mut tries = 0
loop {
    let rows = (http get $"($env.PAAVO_HOST)/jobs?limit=50")
    let states = ($rows | get state | uniq)
    let any_pending = ($rows | where state not-in $terminal_states | length)
    print $"  attempt ($tries): states=($states), pending=($any_pending)"
    if $any_pending == 0 and ($rows | length) >= 2 {
        break
    }
    if $tries >= 30 {
        print "WARN: not all jobs terminal after 30 polls; continuing anyway"
        break
    }
    sleep 500ms
    $tries = $tries + 1
}

print "\n--- final job table (paavo-cli jobs --state passed):"
cargo run -p paavo-cli -- jobs --state passed --limit 20
print "\n--- final job table (paavo-cli jobs --state failed):"
cargo run -p paavo-cli -- jobs --state failed --limit 20

# ---------------------------------------------------------------
# 4. Exercise quarantine round-trip on one board.
# ---------------------------------------------------------------
print "\n=== quarantine/unquarantine round-trip ==="
cargo run -p paavo-cli -- board quarantine mcxa266-01 --reason "manual smoke test"
print "--- boards after quarantine:"
cargo run -p paavo-cli -- boards
cargo run -p paavo-cli -- board unquarantine mcxa266-01
print "--- boards after unquarantine:"
cargo run -p paavo-cli -- boards

# ---------------------------------------------------------------
# 5. Exercise the DELETE /boards/:id path two ways:
#    a) on a fresh board with no referencing jobs (success, 204)
#    b) on the two boards from step 1 (409 — they have job rows)
#    This demonstrates both halves of the FK guard.
# ---------------------------------------------------------------
print "\n=== board remove: success path ==="
cargo run -p paavo-cli -- board add --kind mcxa266 --instance mcxa266-99 --probe 1366:1015:DEMO99 --chip MCXA266VFL --target frdm-mcx-a266
cargo run -p paavo-cli -- board quarantine mcxa266-99 --reason "removing for smoke test"
cargo run -p paavo-cli -- board remove mcxa266-99
print "--- boards after removing mcxa266-99 (should be gone):"
cargo run -p paavo-cli -- boards

print "\n=== board remove: 409 path (boards with job rows) ==="
cargo run -p paavo-cli -- board quarantine mcxa266-01 --reason "retired"
cargo run -p paavo-cli -- board quarantine rt685-01   --reason "retired"
print "attempting to remove boards with referencing jobs (expect 409):"
do { cargo run -p paavo-cli -- board remove mcxa266-01 } | complete | get stderr | print $in
do { cargo run -p paavo-cli -- board remove rt685-01   } | complete | get stderr | print $in
print "--- final boards listing (the two job-referencing boards remain, quarantined):"
cargo run -p paavo-cli -- boards

print "\nDone. Cleanup options:"
print "  - reset between smoke runs (preserves boards/schedules):"
print "      cargo run -p paavo-cli -- admin purge"
print "  - wipe everything including registered boards:"
print "      1. Stop paavod (Ctrl-C in the daemon terminal)."
print $"      2. rm -rf ($env.LOCALAPPDATA)/paavo"
print "      3. Restart paavod."

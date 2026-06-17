# paavo-web

Web viewer for the paavo lab. `paavo-web` is a **JSON + SSE backend** that
embeds the `paavo-web-ui` Leptos CSR single-page app — built by `trunk` into
`../paavo-web-ui/dist` and baked in at compile time via `rust-embed` — and
serves it as one binary. It opens the paavo SQLite database **read-only**
(`paavod` is the single writer; SQLite in WAL mode is the only IPC) and proxies
the daemon's NDJSON job stream to the browser as Server-Sent Events.

If `dist/` was never built, `rust-embed`'s `#[allow_missing]` lets the crate
still compile and every request serves a "UI not built" placeholder — run
`just build-ui` to embed the real SPA. See the repo `README.md` and `AGENTS.md`
for the big picture and `sample-paavo.toml` for every config knob.

## Demo / manual UI test

End-to-end, no hardware: build the SPA, run `paavod` with the fake runner, serve
the UI, register a fake fleet, then flood it with seeded jobs and follow one
genuinely live job. Each numbered step that starts a process wants its own
shell.

```bash
# 0. One-time: wasm target + trunk
rustup target add wasm32-unknown-unknown
cargo install trunk            # or: cargo binstall trunk

# 1. Build the WASM UI (embedded into paavo-web at compile time)
just build-ui                  # = cd crates/paavo-web-ui && trunk build --release

# 2. Start the daemon with the fake runner (every real job Passes). DB → /tmp/paavo/paavo.sqlite
PAAVO_FAKE_RUNNER=1 cargo run -p paavod -- --config sample-paavo.toml

# 3. In another shell: start the web UI → http://127.0.0.1:8081
cargo run -p paavo-web -- --config sample-paavo.toml

# 4. Register fake boards (CLI → paavod on :8090)
export PAAVO_HOST=http://127.0.0.1:8090
for i in 01 02 03 04 05 06; do
  cargo run -p paavo-cli -- board add \
    --kind mcxa266 --instance mcxa266-$i \
    --probe 1366:1015:FAKE$i --chip MCXA266 \
    --target thumbv8m.main-none-eabihf --wiring-profile default
done

# 5a. Flood the UI: seed 300 varied jobs across the boards, trickling the
#     last batch so you see live inserts + the "N new" pill. (Writes the
#     same /tmp/paavo/paavo.sqlite that paavo-web reads.)
just seed-demo                 # = cargo run --manifest-path dev/seed-demo/Cargo.toml -- --db /tmp/paavo/paavo.sqlite --boards 6 --jobs 300 --trickle-ms 400

# 5b. See a genuinely live job stream: submit the smoke fixture and follow it
cargo run -p paavo-cli -- run tests/fixtures/smoke-crate \
  --board-kind mcxa266 --instance mcxa266-01 --follow

# Open http://127.0.0.1:8081 — page through 300 jobs, fuzzy-search "alice mcx",
# watch state badges advance live, open a job to tail + filter its log, toggle dark/light.
```

> **NOTE:** paavod's startup reconciliation sweeps seeded `building`/`running`
> rows to `Aborted(Interrupted)` if you (re)start paavod against the seeded DB.
> For the frozen-variety browse the seeder + paavo-web alone suffice (paavo-web
> only reads — it never mutates the DB); use **5b** for a truly live in-flight
> job.

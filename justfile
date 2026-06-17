# Build the WASM UI into crates/paavo-web-ui/dist (embedded by paavo-web at compile time).
build-ui:
    cd crates/paavo-web-ui && trunk build --release

# Build the UI, then run paavo-web (serves http://127.0.0.1:8081 per sample-paavo.toml).
web: build-ui
    cargo run -p paavo-web -- --config sample-paavo.toml

# Seed the dev DB with fake boards + jobs to stress-test the UI.
seed-demo jobs="300":
    cargo run --manifest-path dev/seed-demo/Cargo.toml -- \
      --db /tmp/paavo/paavo.sqlite --boards 6 --jobs {{jobs}} --trickle-ms 400

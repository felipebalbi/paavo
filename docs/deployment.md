# Deployment

paavo's daemon (`paavod`) and read-only web viewer (`paavo-web`) are
supported on Linux for production deployment. `paavo-cli` runs anywhere
Rust does (Linux, macOS, Windows).

For a dev loop on Windows without hardware, see `manual-smoke.nu` in
the repo root — runs paavod with `PAAVO_FAKE_RUNNER=1` against a
cross-compiled fixture (`tests/fixtures/smoke-crate/`).

## Required system packages (Ubuntu/Debian)

```bash
sudo apt-get install -y libudev-dev pkg-config build-essential
```

## Build & install

```bash
git clone https://github.com/felipebalbi/paavo /opt/paavo
cd /opt/paavo
cargo build --release -p paavod -p paavo-web
sudo install -m 0755 target/release/paavod    /usr/local/bin/
sudo install -m 0755 target/release/paavo-web /usr/local/bin/
```

Then follow [`contrib/README.md`](../contrib/README.md) for systemd + udev.

## State directory layout

`/var/lib/paavo/`:

- `paavo.sqlite` (+ WAL files) — single writer (paavod), single reader (paavo-web).
- `uploads/` — incoming crate tars, keyed by blake3.
- `sandboxes/` — per-job build dirs.
- `cargo-target/` — shared `CARGO_TARGET_DIR` for cargo's incremental reuse.
- `cache/elf/` — cached ELFs paired with `build_cache` table rows (DB row holds the path; the ELF file lives here). LRU evicted when `build_cache.max_bytes` is hit.
- `boards.toml` — `paavo-cli board add` writes this; restart paavod to pick up changes.

## Updating

```bash
cd /opt/paavo && git pull
cargo build --release -p paavod -p paavo-web
sudo install -m 0755 target/release/paavod    /usr/local/bin/
sudo install -m 0755 target/release/paavo-web /usr/local/bin/
sudo systemctl restart paavod.service paavo-web.service
```

## Dev-loop reset

When testing or after a bad run:

```bash
paavo-cli admin purge   # wipes job artifacts; preserves boards + schedules
```

Full reset (preserves nothing):

```bash
sudo systemctl stop paavod paavo-web
sudo rm -rf /var/lib/paavo
sudo systemctl start paavod paavo-web
```

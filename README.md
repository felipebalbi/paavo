# paavo

Self-hosted Linux hardware-in-the-loop test runner for the `embassy-mcxa`
HAL (and any future embassy chip wired into the lab).

Named after Paavo Nurmi — Olympic distance runner — a fit for a test
runner whose nightly job is hours-long stability soaks.

## Quick start (developer workstation)

```bash
cargo install --git https://github.com/felipebalbi/paavo paavo-cli
export PAAVO_HOST=http://lab.local:8080
paavo-cli new my-dma-test --board-kind mcxa266
cd my-dma-test
$EDITOR src/main.rs
paavo-cli run . --board-kind mcxa266 --timeout 30m
```

(Windows developers: same `cargo install` works. See `manual-smoke.nu`
for a self-contained dev loop driven by `PAAVO_FAKE_RUNNER=1`; no
hardware required.)

## Quick start (lab machine)

See [`docs/deployment.md`](docs/deployment.md) and
[`contrib/README.md`](contrib/README.md).

## Configuration

### Server binaries (`paavod`, `paavo-web`)

Both server binaries share the same config file format and default location:

| Source | Path |
|--------|------|
| Default | `/etc/paavo/paavo.toml` |
| Environment | `PAAVO_CONFIG` |
| CLI flag | `--config <path>` |

For production deployments, place your config at `/etc/paavo/paavo.toml` and
no flags are needed. For local development, pass `--config sample-paavo.toml`
explicitly.

See [`sample-paavo.toml`](sample-paavo.toml) for an annotated example with
all available options.

### CLI (`paavo-cli`)

The CLI resolves the daemon URL in this order:

1. `--host` flag
2. `PAAVO_HOST` environment variable
3. `~/.config/paavo/cli.toml` (XDG-compliant; respects `XDG_CONFIG_HOME`)
4. Default: `http://127.0.0.1:8080`

The `cli.toml` file is minimal:

```toml
host = "http://your-paavod-server:8090"
```

## Scheduled runs

Paavo supports automatic nightly (or any cron schedule) test runs via
configuration — there is no CLI command or API for creating schedules.

Configure scheduled runs in `paavo.toml`:

```toml
[scheduler]
# 6-field cron: sec min hour dom mon dow
nightly_cron = "0 0 19 * * *"  # every day at 19:00:00

[[corpus]]
name = "embassy-mcxa-regression"
kind = "mcxa266"
path = "/srv/paavo/test-crates/embassy-mcxa"
cargo_update = ["embassy-mcxa", "embassy-executor"]
```

When the cron fires, `paavod` walks each `[[corpus]]` directory, tars every
test crate it finds, and submits them as `Scheduled` priority jobs. The
`cargo_update` field (optional) specifies which dependencies to
`cargo update -p <name>` before building, ensuring nightly runs test against
the latest upstream.

Multiple `[[corpus]]` entries are supported for different board kinds or
test suites.

## Design

- Full design:
  [`docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md`](docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md)
- Implementation plan:
  [`docs/superpowers/plans/2026-06-09-paavo-implementation.md`](docs/superpowers/plans/2026-06-09-paavo-implementation.md)
- HW smoke checklist for releases:
  [`docs/hw-smoke-checklist.md`](docs/hw-smoke-checklist.md)

## License

Dual-licensed under MIT or Apache-2.0 at your option.

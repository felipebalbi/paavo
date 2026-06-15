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

## Design

- Full design:
  [`docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md`](docs/superpowers/specs/2026-06-09-paavo-test-runner-design.md)
- Implementation plan:
  [`docs/superpowers/plans/2026-06-09-paavo-implementation.md`](docs/superpowers/plans/2026-06-09-paavo-implementation.md)
- HW smoke checklist for releases:
  [`docs/hw-smoke-checklist.md`](docs/hw-smoke-checklist.md)

## License

Dual-licensed under MIT or Apache-2.0 at your option.

# paavo template: mcxa266

Test-crate scaffold for the NXP FRDM-MCX-A266 EVK. Generated via
`paavo-cli new --board-kind mcxa266 <name>` (or `cargo generate`
directly against this template).

## probe-rs chip name

Operators wiring `boards.toml` for the FRDM-MCX-A266 EVK must use
`chip_name = "MCXA276"`. Despite the part being marketed as MCX-A266
(266 MHz dual-core variant), probe-rs's built-in target registry
contains `MCXA275` and `MCXA276` but NOT `MCXA266` or `MCXA256`. The
MCXA276 target's flash + RAM map matches the A266; attaching as
MCXA276 works for flashing, RTT, defmt, the full pipeline. (Verified
against real hardware in M7.0; see `dev/probe-rs-spike/FINDINGS.md`.)

/* NXP MCX-A266 (MCXA2xx family, Cortex-M33).
 *
 * Reference: embassy-rs/embassy examples/mcxa2xx/memory.x.
 * The same map works for MCX-A256 and MCX-A266 — both have 1 MiB
 * flash at 0x00000000 and 128 KiB SRAM at 0x20000000 (the M266
 * exposes extra RAM banks above this but cortex-m-rt only needs
 * the contiguous primary bank).
 *
 * cortex-m-rt's `set-sp` + `set-vtor` features (enabled in
 * Cargo.toml) are required so this map works without a bootloader
 * pre-initialising SP and VTOR.
 */
MEMORY
{
    FLASH : ORIGIN = 0x00000000, LENGTH = 1M
    RAM   : ORIGIN = 0x20000000, LENGTH = 128K
}

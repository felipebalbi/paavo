/* NXP MCX-A266 (MCXA2xx family, Cortex-M33) — RAM-resident layout.
 *
 * paavo tests run from RAM (teleprobe convention): probe-rs loads the
 * ELF directly into SRAM, sets PC + SP + VTOR, and runs. There is no
 * FLASH stage — `link_ram_cortex_m.x` (wired via build.rs) puts the
 * vector table, .text, .rodata, .data, and .bss all into RAM, so
 * memory.x defines only RAM. Defining FLASH here would collide with
 * link_ram.x which assumes ORIGIN(RAM) is the only region.
 *
 * Real silicon RAM (per `probe-rs chip info MCXA276`):
 *   Primary contiguous bank: 240 KiB at 0x20000000.
 *   (Family also has 8 KiB Generic at 0x03000000 and two 8 KiB RAM
 *   banks at 0x04000000/0x04002000, used for ROM/TrustZone scratch;
 *   cortex-m-rt only needs the contiguous main bank.)
 *
 * cortex-m-rt's `set-sp` + `set-vtor` features (enabled in
 * Cargo.toml) are required so this map works without a bootloader
 * pre-initialising SP and VTOR — `link_ram_cortex_m.x` writes the
 * vector table to ORIGIN(RAM) and cortex-m-rt's startup uses VTOR
 * to point the CPU there at runtime. paavo-probe also writes VTOR
 * via `Session::prepare_running_on_ram` before resuming, so the
 * vector table is correctly addressable from the very first
 * exception.
 *
 * Mirrors templates/mcxa266/memory.x byte-for-byte (the spike fixture
 * is intentionally a structural twin of what cargo-generate produces
 * for `kind = "mcxa266"`, so the M7.7 hardware tests exercise the
 * exact same boot path the production templates use).
 */
MEMORY
{
    RAM : ORIGIN = 0x20000000, LENGTH = 240K
}

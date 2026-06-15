/* memory.x for NXP MCX-A266 / MCXA256 family.
 *
 * Per the embassy-mcxa examples reference:
 *   FLASH : 0x00000000, 1 MiB
 *   RAM   : 0x20000000, 128 KiB
 *
 * MCX-A266 (the part on the EVK on Felipe's desk) is a 266MHz dual-core
 * variant of the same family; the user-accessible FLASH+RAM layout is
 * what cortex-m-rt cares about, and that matches the MCXA256 reference.
 *
 * Note: cortex-m-rt's `set-sp` + `set-vtor` features are required to make
 * this layout work without an external bootloader handing off control.
 */
MEMORY
{
    FLASH : ORIGIN = 0x00000000, LENGTH = 1M
    RAM   : ORIGIN = 0x20000000, LENGTH = 128K
}

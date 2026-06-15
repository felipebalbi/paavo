/* rt685s memory map (Cortex-M33 side). The chip has no internal flash;
 * code runs from xSPI-attached external flash mapped at 0x08000000.
 * RAM is the on-chip SRAM partition allocated to the M33 core (the
 * Hifi4 DSP gets the rest). Confirm against the NXP RT685S datasheet
 * for your exact part number before first run. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 64M
  RAM   : ORIGIN = 0x20080000, LENGTH = 256K
}

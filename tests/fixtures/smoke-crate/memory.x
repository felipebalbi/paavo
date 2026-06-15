/* Minimal memory layout for the smoke crate. cortex-m-rt's link.x reads
 * MEMORY from here at link time. The values mirror the MCXA266 flash/
 * RAM map but they don't have to be correct for the manual smoke flow:
 * the FakeRunner never flashes the binary, and the ELF only needs to
 * exist + have valid magic. M6.1 will template-generate a board-
 * specific memory.x per target. */
MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 1024K
  RAM   : ORIGIN = 0x20000000, LENGTH = 224K
}

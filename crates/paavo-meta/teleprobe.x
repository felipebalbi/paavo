/* paavo-meta linker fragment. Preserves the .teleprobe.* ELF sections
 * emitted by target!(), timeout!(), and inactivity_timeout!() so that
 * paavo-probe can read them out of the linked binary. */
SECTIONS
{
    .teleprobe (INFO) :
    {
        KEEP(*(.teleprobe.target))
        KEEP(*(.teleprobe.timeout))
        KEEP(*(.teleprobe.inactivity_timeout))
    }
}
INSERT AFTER .text;

/* paavo-meta linker fragment. Preserves the .paavo.* ELF sections
 * emitted by target!(), timeout!(), and inactivity_timeout!() so that
 * paavo-probe can read them out of the linked binary. */
SECTIONS
{
    .paavo (INFO) :
    {
        KEEP(*(.paavo.target))
        KEEP(*(.paavo.timeout))
        KEEP(*(.paavo.inactivity_timeout))
    }
}
INSERT AFTER .text;

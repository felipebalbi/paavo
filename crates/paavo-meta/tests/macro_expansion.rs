//! Compile-and-link tests for the macro surface. We can't easily assert
//! that the macros land in `.paavo.*` sections on a host build —
//! that's the embedded linker's job, and the host build steers them into
//! `.rodata.*` instead. But we *can* prove the macros expand and link on
//! the host: that catches typos in the macro bodies.
//!
//! Real ELF-section assertions live in `paavo-probe::tests::sections`
//! (Milestone 2), which builds against synthetic ELF fixtures.

paavo_meta::target!(b"frdm-mcx-a266");
paavo_meta::timeout!(60);
paavo_meta::inactivity_timeout!(30);

#[test]
fn macros_expand_and_link() {
    // The macros emit `pub static [u8; 4]` items into this very module in
    // explicit little-endian wire format. Decode with `u32::from_le_bytes`
    // to document the on-ELF contract `paavo-probe` relies on.
    assert_eq!(u32::from_le_bytes(_PAAVO_META_TIMEOUT), 60);
    assert_eq!(u32::from_le_bytes(_PAAVO_META_INACTIVITY_TIMEOUT), 30);
    // target!() emits a byte string; we don't assert against the bytes here
    // because the storage size depends on the literal length. The compile +
    // link of this file is itself the assertion that the macro expands.
}

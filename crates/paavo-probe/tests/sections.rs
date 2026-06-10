//! Integration tests for the `.paavo.*` ELF section parser.
//!
//! Rather than depend on a cross-compiled fixture ELF (which would require
//! an ARM toolchain in CI), we synthesise minimal ELFs in-process with the
//! `object` crate's writer.

use paavo_probe::sections::{parse_meta_sections, MetaSections};

fn synth_elf(sections: &[(&str, &[u8])]) -> Vec<u8> {
    use object::write::{Object, StandardSection, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    };

    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Arm, Endianness::Little);

    // Required for ARM thumb elves; not strictly necessary for parsing but
    // keeps the file shape realistic.
    let text_id = obj.section_id(StandardSection::Text);
    obj.append_section_data(text_id, &[0u8; 4], 4);

    for (name, data) in sections {
        let sect_id = obj.add_section(
            Vec::new(),
            name.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );
        obj.append_section_data(sect_id, data, 4);
        let _ = obj.add_symbol(Symbol {
            name: name.replace('.', "_").into_bytes(),
            value: 0,
            size: data.len() as u64,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(sect_id),
            flags: SymbolFlags::None,
        });
    }
    obj.write().unwrap()
}

#[test]
fn parses_all_three_sections_when_present() {
    let target = b"frdm-mcx-a266\0";
    let timeout = 3600u32.to_le_bytes();
    let inact = 60u32.to_le_bytes();
    let elf = synth_elf(&[
        (".paavo.target", target),
        (".paavo.timeout", &timeout),
        (".paavo.inactivity_timeout", &inact),
    ]);
    let m = parse_meta_sections(&elf).unwrap();
    assert_eq!(m.target.as_deref(), Some("frdm-mcx-a266"));
    assert_eq!(m.timeout_s, Some(3600));
    assert_eq!(m.inactivity_timeout_s, Some(60));
}

#[test]
fn missing_sections_become_none() {
    let elf = synth_elf(&[(".paavo.target", b"foo\0")]);
    let m = parse_meta_sections(&elf).unwrap();
    assert_eq!(m.target.as_deref(), Some("foo"));
    assert_eq!(m.timeout_s, None);
    assert_eq!(m.inactivity_timeout_s, None);
}

#[test]
fn empty_target_section_is_an_error() {
    let elf = synth_elf(&[(".paavo.target", b"")]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("target") && msg.contains("empty"), "{msg}");
}

#[test]
fn wrong_size_timeout_section_is_an_error() {
    let elf = synth_elf(&[(".paavo.timeout", &[1u8, 2, 3])]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("timeout") && msg.contains("4 bytes"), "{msg}");
}

#[test]
fn oversize_timeout_section_is_an_error() {
    let elf = synth_elf(&[(".paavo.timeout", &[1u8, 2, 3, 4, 5])]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("timeout") && msg.contains("4 bytes"), "{msg}");
}

#[test]
fn target_section_without_trailing_nul_is_an_error() {
    let elf = synth_elf(&[(".paavo.target", b"frdm-mcx-a266")]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("missing trailing NUL"), "{msg}");
}

#[test]
fn target_section_with_interior_nul_and_trailing_bytes_is_an_error() {
    let elf = synth_elf(&[(".paavo.target", b"frdm\0junk")]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("interior NUL"), "{msg}");
}

#[test]
fn target_section_with_invalid_utf8_is_an_error() {
    // 0xff is not valid UTF-8.
    let elf = synth_elf(&[(".paavo.target", b"\xff\xfe\0")]);
    let err = parse_meta_sections(&elf).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("invalid UTF-8"), "{msg}");
}

#[test]
fn meta_sections_default_is_all_none() {
    let m = MetaSections::default();
    assert!(m.target.is_none());
    assert!(m.timeout_s.is_none());
    assert!(m.inactivity_timeout_s.is_none());
}

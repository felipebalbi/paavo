//! Parser for the `.paavo.*` ELF sections embedded by the `paavo-meta`
//! macros. The section namespace is owned end-to-end by this workspace;
//! no external tool produces these sections today.
//!
//! Wire format reminder (matches what `paavo-meta` emits):
//! - `.paavo.target`             — NUL-terminated UTF-8 byte string.
//! - `.paavo.timeout`            — exactly 4 bytes, `u32` little-endian.
//! - `.paavo.inactivity_timeout` — exactly 4 bytes, `u32` little-endian.

use crate::error::{ProbeError, Result};
use object::{Object, ObjectSection};

/// Parsed contents of all three optional metadata sections.
///
/// Each field is `None` when the corresponding section is absent. A section
/// that is present but malformed (empty target, wrong-size integer) is a
/// hard error, not a silent fall-back, so the caller doesn't paper over a
/// real build-wiring bug.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MetaSections {
    /// `.paavo.target` — must match a `BoardSpec::target_name` in the
    /// inventory.
    pub target: Option<String>,
    /// `.paavo.timeout` — per-test hard-max override, in seconds.
    pub timeout_s: Option<u32>,
    /// `.paavo.inactivity_timeout` — per-test inactivity override, in
    /// seconds.
    pub inactivity_timeout_s: Option<u32>,
}

/// Parse the three `.paavo.*` sections out of an ELF byte buffer.
///
/// - Missing sections yield `None` on the corresponding field — they are
///   not errors.
/// - Sections present but malformed (empty target, wrong-size integer) are
///   errors so the caller doesn't fall back to a default that masks a real
///   bug in the test crate's build wiring.
pub fn parse_meta_sections(elf: &[u8]) -> Result<MetaSections> {
    let file = object::File::parse(elf)?;
    let mut out = MetaSections::default();

    if let Some(s) = section_data(&file, ".paavo.target")? {
        out.target = Some(parse_cstring(s)?);
    }
    if let Some(s) = section_data(&file, ".paavo.timeout")? {
        out.timeout_s = Some(parse_u32_le(".paavo.timeout", s)?);
    }
    if let Some(s) = section_data(&file, ".paavo.inactivity_timeout")? {
        out.inactivity_timeout_s = Some(parse_u32_le(".paavo.inactivity_timeout", s)?);
    }
    Ok(out)
}

fn section_data<'a>(file: &'a object::File<'a>, name: &str) -> Result<Option<&'a [u8]>> {
    let Some(section) = file.section_by_name(name) else {
        return Ok(None);
    };
    let data = section.data()?;
    Ok(Some(data))
}

fn parse_cstring(bytes: &[u8]) -> Result<String> {
    // Wire format: exactly N non-NUL bytes followed by a single trailing NUL.
    // Anything else (empty, no NUL, interior NUL with trailing bytes, invalid
    // UTF-8) is a malformed-producer error — the parser refuses to paper
    // over a build-wiring bug.
    if bytes.is_empty() {
        return Err(ProbeError::EmptyTarget);
    }
    let Some(nul_pos) = bytes.iter().position(|&b| b == 0) else {
        return Err(ProbeError::MalformedTarget {
            reason: "missing trailing NUL".into(),
        });
    };
    if nul_pos == 0 {
        return Err(ProbeError::EmptyTarget);
    }
    if nul_pos != bytes.len() - 1 {
        return Err(ProbeError::MalformedTarget {
            reason: format!(
                "interior NUL at byte {nul_pos} with {trailing} trailing bytes after",
                trailing = bytes.len() - nul_pos - 1
            ),
        });
    }
    std::str::from_utf8(&bytes[..nul_pos])
        .map(str::to_owned)
        .map_err(|e| ProbeError::MalformedTarget {
            reason: format!("invalid UTF-8 at byte {}", e.valid_up_to()),
        })
}

fn parse_u32_le(name: &'static str, bytes: &[u8]) -> Result<u32> {
    if bytes.len() != 4 {
        return Err(ProbeError::BadIntegerSection {
            section: name,
            got: bytes.len(),
        });
    }
    // Explicit per-byte indexing — no `try_into().unwrap()` chain, no
    // truncating cast. Length is checked above.
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

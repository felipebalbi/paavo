//! Board inventory and selector types.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// VID/PID/serial selector for a probe, matching the probe-rs naming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSelector {
    /// USB vendor id, hex string e.g. `"1fc9"`. Canonical form is lowercase 4-hex.
    pub vid: String,
    /// USB product id, hex string e.g. `"0143"`. Canonical form is lowercase 4-hex.
    pub pid: String,
    /// Probe serial number as reported by USB. May be empty ("no filter")
    /// and may contain `:` (e.g. ESP JTAG MAC serials).
    pub serial: String,
    /// USB interface index (the `-N` in a probe-rs selector). `None` matches
    /// any interface; set only for multi-interface probes (e.g. FTDI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<u8>,
}

/// Error parsing a probe-rs selector string into a [`ProbeSelector`].
#[derive(Debug, Error)]
pub enum ProbeSelectorParseError {
    /// Selector was empty or had no PID part.
    #[error(
        "selector is empty or missing the PID (expected `VID:PID[-IFACE][:SERIAL]`, \
             e.g. `1fc9:0143:ABCD1234`)"
    )]
    Format,
    /// VID was not a hex u16.
    #[error("bad VID {value:?}: {source}")]
    BadVid {
        /// The offending VID text.
        value: String,
        /// The underlying parse error.
        source: std::num::ParseIntError,
    },
    /// PID was not a hex u16.
    #[error("bad PID {value:?}: {source}")]
    BadPid {
        /// The offending PID text.
        value: String,
        /// The underlying parse error.
        source: std::num::ParseIntError,
    },
    /// Interface suffix was not a u8.
    #[error("bad USB interface {value:?}: {source}")]
    BadInterface {
        /// The offending interface text.
        value: String,
        /// The underlying parse error.
        source: std::num::ParseIntError,
    },
}

impl ProbeSelector {
    /// Parse a probe-rs selector token **or** a full `probe-rs list` line.
    ///
    /// Grammar matches probe-rs's own `DebugProbeSelector`:
    /// `VID:PID[-IFACE][:SERIAL]`, split via `splitn(3, ':')` so colon-bearing
    /// serials (e.g. ESP JTAG MACs) survive. VID/PID are hex and are
    /// normalized to lowercase 4-hex.
    pub fn parse(input: &str) -> Result<Self, ProbeSelectorParseError> {
        let token = extract_selector_token(input);

        let mut parts = token.splitn(3, ':');
        let vid_raw = parts.next().unwrap_or("").trim();
        let pid_field = parts.next().ok_or(ProbeSelectorParseError::Format)?.trim();
        let serial = parts.next().map(|s| s.to_string()).unwrap_or_default();

        // Peel the optional `-IFACE` off the PID field.
        let (pid_raw, interface) = match pid_field.split_once('-') {
            Some((pid, iface)) => {
                let iface = iface.trim();
                let interface = if iface.is_empty() {
                    None
                } else {
                    Some(iface.parse::<u8>().map_err(|source| {
                        ProbeSelectorParseError::BadInterface {
                            value: iface.to_string(),
                            source,
                        }
                    })?)
                };
                (pid.trim(), interface)
            }
            None => (pid_field, None),
        };

        if vid_raw.is_empty() || pid_raw.is_empty() {
            return Err(ProbeSelectorParseError::Format);
        }

        let vid = parse_hex_u16(vid_raw).map_err(|source| ProbeSelectorParseError::BadVid {
            value: vid_raw.to_string(),
            source,
        })?;
        let pid = parse_hex_u16(pid_raw).map_err(|source| ProbeSelectorParseError::BadPid {
            value: pid_raw.to_string(),
            source,
        })?;

        Ok(ProbeSelector {
            vid: format!("{vid:04x}"),
            pid: format!("{pid:04x}"),
            serial,
            interface,
        })
    }

    /// Validate that `vid`/`pid` are hex `u16`. Used by paavod at registration
    /// (the wire already carries a structured selector, so there's nothing to
    /// re-split — just confirm the fields are well-formed).
    pub fn validate(&self) -> Result<(), ProbeSelectorParseError> {
        parse_hex_u16(&self.vid).map_err(|source| ProbeSelectorParseError::BadVid {
            value: self.vid.clone(),
            source,
        })?;
        parse_hex_u16(&self.pid).map_err(|source| ProbeSelectorParseError::BadPid {
            value: self.pid.clone(),
            source,
        })?;
        Ok(())
    }
}

/// Extract the `VID:PID…` token from either a bare token or a full
/// `probe-rs list` line (`[N]: <identifier> -- <token> (<TYPE>)`).
fn extract_selector_token(input: &str) -> &str {
    let s = input.trim();
    match s.rfind(" -- ") {
        Some(idx) => {
            let after = s[idx + 4..].trim();
            // Strip a trailing ` (TYPE)` parenthetical only on the list-line path.
            match after.rfind(" (") {
                Some(p) => after[..p].trim(),
                None => after,
            }
        }
        None => s,
    }
}

/// Parse a hex string into `u16`, tolerating a `0x`/`0X` prefix and whitespace.
/// Kept in sync with `paavo_probe::session::parse_hex_u16` (see spec future-work).
fn parse_hex_u16(s: &str) -> Result<u16, std::num::ParseIntError> {
    let s = s.trim();
    let stripped = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(stripped, 16)
}

/// Whether a board is currently eligible to receive jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardHealth {
    /// Eligible for job dispatch.
    Healthy,
    /// Excluded from dispatch (manual or auto quarantine).
    Quarantined,
}

/// A registered board in the lab inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSpec {
    /// Lab-unique identifier, e.g. `mcxa266-01`.
    pub id: String,
    /// Board kind, e.g. `mcxa266`. Must match what `paavo_meta::target!()`
    /// emits in scaffolded crates of this kind.
    pub kind: String,
    /// Physical probe used to flash + debug this board.
    pub probe_selector: ProbeSelector,
    /// probe-rs chip name (passed to `Session::new`).
    pub chip_name: String,
    /// `paavo_meta::target!()` value scaffolded test crates write for this
    /// kind. Used to verify ELFs land on the correct fleet.
    pub target_name: String,
    /// Optional named wiring profile (e.g. `alt-spi`). Selectors that ask
    /// for a profile only match boards tagged with that profile.
    pub wiring_profile: Option<String>,
    /// Current health.
    pub health: BoardHealth,
}

/// Job-side selector for matching against the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSelector {
    /// Required board kind.
    pub kind: String,
    /// Optional specific instance (`mcxa266-02`). When set, only that board
    /// is eligible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Optional required wiring profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wiring_profile: Option<String>,
}

impl BoardSelector {
    /// True if `board` satisfies this selector. Health is **not** checked
    /// here — that is the scheduler's job.
    pub fn matches(&self, board: &BoardSpec) -> bool {
        if self.kind != board.kind {
            return false;
        }
        if let Some(inst) = &self.instance {
            if inst != &board.id {
                return false;
            }
        }
        if let Some(profile) = &self.wiring_profile {
            if board.wiring_profile.as_deref() != Some(profile.as_str()) {
                return false;
            }
        }
        true
    }
}

/// JSON shape returned by `GET /boards` and `GET /boards/:id`. Wraps a
/// `BoardSpec` with the operational fields the spec §9.4 promises:
/// last-used timestamp, quarantine reason, the infra-failure counter
/// that drives auto-quarantine, and the registration timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardView {
    /// Inlined spec fields (`#[serde(flatten)]` so the wire shape is
    /// flat: `{ "id": ..., "kind": ..., ..., "last_used_at": ..., ... }`).
    #[serde(flatten)]
    pub spec: BoardSpec,
    /// Free-form reason recorded when `spec.health == Quarantined`.
    /// `None` when the board is healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine_reason: Option<String>,
    /// Counts toward the auto-quarantine threshold
    /// (`quarantine.consecutive_infra_failures`).
    pub consecutive_infra_failures: u32,
    /// Epoch ms of the most recent successful dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
    /// Epoch ms when this board was first registered.
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_token() {
        let s = ProbeSelector::parse("1fc9:0143:EDFHUAFM4J5ZJ").unwrap();
        assert_eq!(
            s,
            ProbeSelector {
                vid: "1fc9".into(),
                pid: "0143".into(),
                serial: "EDFHUAFM4J5ZJ".into(),
                interface: None,
            }
        );
    }

    #[test]
    fn parse_with_interface() {
        let s = ProbeSelector::parse("1fc9:0143-0:EDFHUAFM4J5ZJ").unwrap();
        assert_eq!(s.pid, "0143");
        assert_eq!(s.interface, Some(0));
    }

    #[test]
    fn parse_empty_interface_is_none() {
        let s = ProbeSelector::parse("1fc9:0143-:S").unwrap();
        assert_eq!(s.interface, None);
        assert_eq!(s.serial, "S");
    }

    #[test]
    fn parse_colon_serial_preserved() {
        let s = ProbeSelector::parse("303a:1001:DC:DA:0C:D3:FE:D8").unwrap();
        assert_eq!(s.vid, "303a");
        assert_eq!(s.pid, "1001");
        assert_eq!(s.serial, "DC:DA:0C:D3:FE:D8");
    }

    #[test]
    fn parse_no_serial_is_empty() {
        let s = ProbeSelector::parse("1fc9:0143").unwrap();
        assert_eq!(s.serial, "");
        assert_eq!(s.interface, None);
    }

    #[test]
    fn parse_normalizes_hex() {
        let s = ProbeSelector::parse("0X1FC9:143:S").unwrap();
        assert_eq!(s.vid, "1fc9");
        assert_eq!(s.pid, "0143");
    }

    #[test]
    fn parse_rejects_bad_vid() {
        assert!(matches!(
            ProbeSelector::parse("zz:0143:S"),
            Err(ProbeSelectorParseError::BadVid { .. })
        ));
    }

    #[test]
    fn parse_rejects_bad_pid() {
        assert!(matches!(
            ProbeSelector::parse("1fc9:gg:S"),
            Err(ProbeSelectorParseError::BadPid { .. })
        ));
    }

    #[test]
    fn parse_rejects_missing_pid() {
        assert!(matches!(
            ProbeSelector::parse("1fc9"),
            Err(ProbeSelectorParseError::Format)
        ));
    }

    #[test]
    fn parse_rejects_bad_interface() {
        assert!(matches!(
            ProbeSelector::parse("1fc9:0143-x:S"),
            Err(ProbeSelectorParseError::BadInterface { .. })
        ));
    }

    #[test]
    fn validate_accepts_normalized() {
        let s = ProbeSelector {
            vid: "1fc9".into(),
            pid: "0143".into(),
            serial: "S".into(),
            interface: None,
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_hex() {
        let s = ProbeSelector {
            vid: "zz".into(),
            pid: "0143".into(),
            serial: "S".into(),
            interface: None,
        };
        assert!(s.validate().is_err());
    }
}

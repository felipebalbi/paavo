//! Board inventory and selector types.

use serde::{Deserialize, Serialize};

/// VID/PID/serial selector for a probe, matching the probe-rs naming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSelector {
    /// USB vendor id, hex string e.g. `"1366"`.
    pub vid: String,
    /// USB product id, hex string e.g. `"1015"`.
    pub pid: String,
    /// Probe serial number as reported by USB.
    pub serial: String,
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

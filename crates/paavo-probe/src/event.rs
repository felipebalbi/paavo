//! Events emitted by a probe session.
//!
//! Currently a four-variant enum that `paavo-runner` consumes. The
//! `LogFrame` variant carries a fully-decoded defmt frame; the other three
//! variants are observed control-flow events from the target.

use paavo_proto::LogFrame;

/// One observable event from a running test binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Decoded defmt frame.
    LogFrame(LogFrame),
    /// CPU hit a software breakpoint (the embassy `bkpt()` convention).
    /// Combined with a preceding `defmt::info!("Test OK")` this signals pass.
    Bkpt,
    /// A panic was observed (panic-probe encodes via defmt; this event is
    /// emitted when the runner recognises the panic frame pattern).
    Panic {
        /// Captured panic message.
        message: String,
    },
    /// Probe lost the target (USB drop, target reset without our consent).
    Disconnect,
}

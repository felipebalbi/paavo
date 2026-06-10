//! Test scaffolding shared across paavo-runner integration tests.

use crossbeam_channel::{bounded, Sender};
use paavo_probe::{Event, ProbeError, ProbeSession, Result as ProbeResult};
use paavo_proto::{LogFrame, LogLevel};
use std::time::Duration;

/// Programmable fake probe session.
///
/// Caller pushes scripted events with `script_event`; the session returns
/// them in order via `next_event`. When the script is exhausted, the session
/// returns `Ok(None)` (no event) on each call, simulating an idle probe.
///
/// `next_event` blocks up to `timeout_ms`. Test cases that want to provoke
/// the inactivity watchdog use a small inactivity timeout (e.g. 200 ms) and
/// then leave the script empty.
pub struct FakeSession {
    rx: crossbeam_channel::Receiver<Event>,
}

/// Caller-side handle for scripting events onto a FakeSession.
pub struct FakeScript {
    tx: Sender<Event>,
}

#[allow(dead_code)]
impl FakeScript {
    /// Push a single log frame.
    pub fn log(&self, level: LogLevel, msg: &str) {
        self.tx
            .send(Event::LogFrame(LogFrame {
                seq: 0, // worker doesn't assign seq today; not under test
                ts_us: 0,
                level,
                target: None,
                message: msg.into(),
            }))
            .unwrap();
    }

    /// Push a Bkpt event.
    pub fn bkpt(&self) {
        self.tx.send(Event::Bkpt).unwrap();
    }

    /// Push a Panic event.
    pub fn panic(&self, msg: &str) {
        self.tx
            .send(Event::Panic {
                message: msg.into(),
            })
            .unwrap();
    }

    /// Push a Disconnect event.
    pub fn disconnect(&self) {
        self.tx.send(Event::Disconnect).unwrap();
    }
}

/// Build a FakeSession + its scripting handle.
#[allow(dead_code)]
pub fn fake_session() -> (FakeSession, FakeScript) {
    let (tx, rx) = bounded(64);
    (FakeSession { rx }, FakeScript { tx })
}

impl ProbeSession for FakeSession {
    fn next_event(&mut self, timeout_ms: u32) -> ProbeResult<Option<Event>> {
        match self
            .rx
            .recv_timeout(Duration::from_millis(u64::from(timeout_ms)))
        {
            Ok(ev) => Ok(Some(ev)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(None),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                // Script handle dropped without disconnect event — treat as
                // idle (no event) so the watchdog can fire if relevant.
                Ok(None)
            }
        }
    }
}

/// Convenience: wrap a constructed FakeSession in a `Box<dyn ProbeSession>`
/// for `run_job`. Use it inside the `make_session` closure.
#[allow(dead_code)]
pub fn into_box(s: FakeSession) -> ProbeResult<Box<dyn ProbeSession>> {
    Ok(Box::new(s) as Box<dyn ProbeSession>)
}

/// Used by one test that wants `make_session` to fail.
#[allow(dead_code)]
pub fn fail_to_connect() -> ProbeResult<Box<dyn ProbeSession>> {
    Err(ProbeError::ProbeRs("fake: connect failed".into()))
}

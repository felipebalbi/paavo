use crossbeam_channel::unbounded;
use paavo_proto::JobId;
use paavo_runner::RunCommand;
use paavod::cancellation::CancellationRegistry;

#[test]
fn register_signal_unregister_round_trip() {
    let reg = CancellationRegistry::default();
    let id = JobId::new();
    let (tx, rx) = unbounded::<RunCommand>();
    reg.register(id, tx);
    assert_eq!(reg.active(), 1);

    assert!(reg.signal(&id, RunCommand::Cancel));
    assert_eq!(rx.recv().unwrap(), RunCommand::Cancel);

    reg.unregister(&id);
    assert_eq!(reg.active(), 0);
    assert!(!reg.signal(&id, RunCommand::Cancel));
}

#[test]
fn signal_on_unknown_id_returns_false() {
    let reg = CancellationRegistry::default();
    assert!(!reg.signal(&JobId::new(), RunCommand::Cancel));
}

#[test]
fn signal_all_reaches_every_registered_worker() {
    let reg = CancellationRegistry::default();
    let (tx1, rx1) = unbounded::<RunCommand>();
    let (tx2, rx2) = unbounded::<RunCommand>();
    reg.register(JobId::new(), tx1);
    reg.register(JobId::new(), tx2);
    reg.signal_all(RunCommand::DaemonShutdown);
    assert_eq!(rx1.recv().unwrap(), RunCommand::DaemonShutdown);
    assert_eq!(rx2.recv().unwrap(), RunCommand::DaemonShutdown);
}

#[test]
fn register_twice_overwrites_silently_for_v1() {
    // v1 semantics: there is exactly one dispatch loop, so a second
    // register for the same id is treated as the authoritative one.
    // Documented so a future contributor can decide to upgrade this
    // to a panic if the invariant changes.
    let reg = CancellationRegistry::default();
    let id = JobId::new();
    let (tx1, rx1) = unbounded::<RunCommand>();
    let (tx2, rx2) = unbounded::<RunCommand>();
    reg.register(id, tx1);
    reg.register(id, tx2);
    assert_eq!(reg.active(), 1);
    assert!(reg.signal(&id, RunCommand::Cancel));
    // Only the second receiver got the signal.
    assert_eq!(rx2.recv().unwrap(), RunCommand::Cancel);
    assert!(rx1.try_recv().is_err());
}

#[test]
fn signal_after_receiver_dropped_returns_false() {
    // The watchdog drops the Receiver on exit; signaling that stale
    // entry must not panic and must report "nothing happened" so the
    // HTTP layer can fall through to 409.
    let reg = CancellationRegistry::default();
    let id = JobId::new();
    let (tx, rx) = unbounded::<RunCommand>();
    reg.register(id, tx);
    drop(rx);
    assert!(!reg.signal(&id, RunCommand::Cancel));
}

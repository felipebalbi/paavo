use paavo_proto::JobId;
use paavo_runner::RunCommand;
use paavod::cancellation::CancellationRegistry;

#[test]
fn register_take_signal_unregister_round_trip() {
    let reg = CancellationRegistry::default();
    let id = JobId::new();
    reg.register(id);
    assert_eq!(reg.active(), 1);

    // The runner takes the rx half before signaling.
    let rx = reg.take_receiver(&id).expect("rx available after register");

    // signal() still works after take_receiver because the sender
    // stays in the entry.
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
fn take_receiver_on_unknown_id_returns_none() {
    let reg = CancellationRegistry::default();
    assert!(reg.take_receiver(&JobId::new()).is_none());
}

#[test]
fn take_receiver_twice_returns_none_second_time() {
    // The runner is the sole consumer; a double-take would mean two
    // workers were spawned for the same job — not legal under the v1
    // single-dispatch invariant, but exercising it ensures we don't
    // panic and that the second caller sees a clear "no rx" signal.
    let reg = CancellationRegistry::default();
    let id = JobId::new();
    reg.register(id);
    assert!(reg.take_receiver(&id).is_some());
    assert!(reg.take_receiver(&id).is_none());
}

#[test]
fn signal_all_reaches_every_registered_worker() {
    let reg = CancellationRegistry::default();
    let id1 = JobId::new();
    let id2 = JobId::new();
    reg.register(id1);
    reg.register(id2);
    let rx1 = reg.take_receiver(&id1).unwrap();
    let rx2 = reg.take_receiver(&id2).unwrap();
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
    reg.register(id);
    let rx1 = reg.take_receiver(&id).unwrap();
    reg.register(id);
    let rx2 = reg.take_receiver(&id).unwrap();
    assert_eq!(reg.active(), 1);
    assert!(reg.signal(&id, RunCommand::Cancel));
    // Only the second receiver got the signal; the first is now
    // attached to the dropped sender from the overwritten entry.
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
    reg.register(id);
    let rx = reg.take_receiver(&id).unwrap();
    drop(rx);
    assert!(!reg.signal(&id, RunCommand::Cancel));
}

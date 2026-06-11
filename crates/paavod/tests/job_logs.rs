use paavo_proto::{AbortReason, JobId, JobOutcome, LogFrame, LogLevel};
use paavod::job_logs::{JobLogsBroker, LiveEvent};

#[tokio::test]
async fn subscribe_publish_receive_in_order() {
    let broker = JobLogsBroker::new();
    let id = JobId::new();
    let mut rx = broker.subscribe(id);
    let n = broker.publish(
        id,
        LiveEvent::Frame(LogFrame {
            seq: 0,
            ts_us: 0,
            level: LogLevel::Info,
            target: None,
            message: "a".into(),
        }),
    );
    assert_eq!(n, 1, "one subscriber received the frame");
    broker.publish(id, LiveEvent::Terminal(JobOutcome::Passed));
    let first = rx.recv().await.unwrap();
    let second = rx.recv().await.unwrap();
    assert!(matches!(first, LiveEvent::Frame(_)));
    assert!(matches!(second, LiveEvent::Terminal(_)));
}

#[tokio::test]
async fn finalize_drops_channel() {
    let broker = JobLogsBroker::new();
    let id = JobId::new();
    let _rx = broker.subscribe(id);
    assert_eq!(broker.active_channels(), 1);
    broker.finalize(id);
    assert_eq!(broker.active_channels(), 0);
}

#[tokio::test]
async fn publish_with_no_subscribers_is_a_noop() {
    let broker = JobLogsBroker::new();
    let id = JobId::new();
    let n = broker.publish(
        id,
        LiveEvent::Terminal(JobOutcome::Aborted {
            by: AbortReason::User,
        }),
    );
    assert_eq!(n, 0);
}

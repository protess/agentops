use agentops_agent::phase::DeltaSink;
use agentops_core::{AgentStep, Phase, StepKind};
use agentops_server::bus::{BusEvent, DeltaKind, StepBus};
use time::OffsetDateTime;
use uuid::Uuid;

fn step(id: Uuid, seq: i64) -> AgentStep {
    AgentStep {
        investigation_id: id,
        seq,
        phase: Phase::Triage,
        kind: StepKind::Text {
            text: format!("t{seq}"),
        },
        created_at: OffsetDateTime::now_utc(),
    }
}

/// A subscriber receives only its own investigation's events. Without a channel per
/// investigation, one busy investigation hits another's slow subscriber with `Lagged`.
#[tokio::test]
async fn subscribers_only_see_their_own_investigation() {
    let bus = StepBus::new();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut ra = bus.subscribe(a);

    bus.publish_step(step(b, 0));
    bus.publish_step(step(a, 0));

    match ra.recv().await.unwrap() {
        BusEvent::Step(s) => assert_eq!(
            s.investigation_id, a,
            "received another investigation's event"
        ),
        other => panic!("expected Step, got {other:?}"),
    }
}

/// Publishing does not panic with no subscribers. `broadcast::Sender::send` returns an
/// `Err` with zero receivers, and unwrapping that would kill the runner on an
/// investigation nobody is watching — which is a normal state.
#[tokio::test]
async fn publishing_with_no_subscribers_is_not_an_error() {
    let bus = StepBus::new();
    bus.publish_step(step(Uuid::new_v4(), 0)); // fails if it panics
}

/// A delta arriving through `DeltaSink` becomes a `Delta` event. **It must not become a
/// `Step`** — a delta is never written to the database and so has no `seq`; emitted as a
/// `step` event it would become subject to replay and break the deduplication logic (spec Section 6.1.3).
#[tokio::test]
async fn text_deltas_become_delta_events_not_steps() {
    let bus = StepBus::new();
    let id = Uuid::new_v4();
    let mut rx = bus.subscribe(id);

    bus.text(id, Phase::Rca, "hello");

    match rx.recv().await.unwrap() {
        BusEvent::Delta {
            kind, text, phase, ..
        } => {
            assert_eq!(kind, DeltaKind::Text);
            assert_eq!(text, "hello");
            assert_eq!(phase, Phase::Rca);
        }
        other => panic!("a delta went out as a Step: {other:?}"),
    }
}

#[tokio::test]
async fn thinking_deltas_are_distinguishable_from_text() {
    let bus = StepBus::new();
    let id = Uuid::new_v4();
    let mut rx = bus.subscribe(id);
    bus.thinking(id, Phase::Triage, "pondering");
    match rx.recv().await.unwrap() {
        BusEvent::Delta { kind, .. } => assert_eq!(kind, DeltaKind::Thinking),
        other => panic!("expected Delta, got {other:?}"),
    }
}

/// The terminal event is delivered. The SSE handler receives it, sends `terminal`, and
/// closes the stream (spec Section 10.2.1).
#[tokio::test]
async fn terminal_is_broadcast() {
    let bus = StepBus::new();
    let id = Uuid::new_v4();
    let mut rx = bus.subscribe(id);
    bus.publish_terminal(id);
    assert!(matches!(
        rx.recv().await.unwrap(),
        BusEvent::Terminal { .. }
    ));
}

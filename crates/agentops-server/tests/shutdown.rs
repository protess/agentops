//! `JobManager::shutdown` — verifies the six-stage order of spec Section 6.1 and the termination race.

use agentops_core::{
    BoxStream, Investigation, InvestigationStatus, LlmError, LlmEvent, LlmProvider, LlmRequest,
    NewArtifact, StepKind, StopReason, Store, TerminalReason, TriggeredBy, Usage,
};
use agentops_server::bus::StepBus;
use agentops_server::jobs::{JobDeps, JobManager};
use agentops_store::PgStore;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

async fn queued(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: "t".into(),
        prompt: "p".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    };
    store.create_investigation(&inv).await.unwrap();
    inv.id
}

async fn running(store: &PgStore) -> Uuid {
    let id = queued(store).await;
    store.mark_running(id).await.unwrap();
    id
}

fn job_deps(provider: Arc<dyn LlmProvider>) -> JobDeps {
    JobDeps {
        provider,
        connections: Vec::new(),
        limits: agentops_agent::limits::Limits::default(),
    }
}

fn manager(store: Arc<PgStore>, provider: Arc<dyn LlmProvider>) -> JobManager<PgStore> {
    JobManager::new(store, StepBus::new(), job_deps(provider))
}

/// Terminates cleanly at once with a single text block.
struct OkProvider;

#[async_trait]
impl LlmProvider for OkProvider {
    fn model_id(&self) -> &str {
        "ok"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(LlmEvent::TextBlock {
                text: "finding".into(),
            }),
            Ok(LlmEvent::Stopped {
                reason: StopReason::EndTurn,
                usage: Usage::default(),
                refusal_category: None,
            }),
        ])))
    }
}

fn ok_provider() -> Arc<dyn LlmProvider> {
    Arc::new(OkProvider)
}

/// Returns a stream that never ends — without shutdown's cancellation this investigation
/// stays `running` forever. The `select!` in `jobs.rs::spawn` takes the cancellation
/// branch the moment it arrives, whatever `run_investigation` is doing with this stream,
/// so this provider emitting nothing at all is sufficient.
struct NeverEndingProvider;

#[async_trait]
impl LlmProvider for NeverEndingProvider {
    fn model_id(&self) -> &str {
        "never-ending"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}

fn never_ending_provider() -> Arc<dyn LlmProvider> {
    Arc::new(NeverEndingProvider)
}

/// Polls until the investigation becomes `running`. A timeout keeps a failure from
/// locking up quietly.
async fn wait_until_running(store: &PgStore, id: Uuid) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if store.get_investigation(id).await.unwrap().status == InvestigationStatus::Running {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("investigation never reached running");
}

fn new_artifact() -> NewArtifact {
    NewArtifact {
        title: "c".into(),
        body: "b".into(),
    }
}

/// TEST-13 — the shutdown race. When shutdown writes `Failed` **while** the task tries to
/// write `Completed`, the conditional transition prevents the overwrite.
///
/// **Verified with real concurrency, not sequential calls** (spec Section 12.1, TEST-13):
/// both termination attempts are launched with `tokio::join!` and exactly one must succeed.
#[sqlx::test(migrations = "../../migrations")]
async fn test_13_concurrent_terminal_transitions_leave_exactly_one_winner(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;

    let a = {
        let store = store.clone();
        async move {
            store
                .complete_investigation(id, &new_artifact())
                .await
                .map(|_| "completed")
        }
    };
    let b = {
        let store = store.clone();
        async move {
            store
                .fail_investigation(id, &TerminalReason::ShutdownRequested)
                .await
                .map(|_| "failed")
        }
    };

    let (ra, rb) = tokio::join!(a, b);
    let winners = [ra.is_ok(), rb.is_ok()].iter().filter(|x| **x).count();
    assert_eq!(
        winners, 1,
        "exactly one must succeed (ra={ra:?}, rb={rb:?})"
    );

    // There must be exactly one terminal step too (the same property as TEST-19).
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminal = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated { .. } | StepKind::ArtifactWritten { .. }
            )
        })
        .count();
    assert_eq!(
        terminal, 1,
        "there are {terminal} terminal steps — there must be exactly one"
    );
}

/// Shutdown marks a live investigation `failed` and leaves `ShutdownRequested` behind.
#[sqlx::test(migrations = "../../migrations")]
async fn shutdown_fails_still_running_investigations_with_a_reason(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    let jm = manager(store.clone(), never_ending_provider());
    jm.spawn(id).unwrap();
    // Wait until the investigation becomes running.
    wait_until_running(&store, id).await;

    jm.shutdown(Duration::from_millis(200)).await;

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Failed);
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::ShutdownRequested,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        terminated, 1,
        "the shutdown reason was not left as exactly one step: {terminated}"
    );
}

/// Shutdown closes the intake **first**. Firing cancellation before closing it would make
/// an investigation arriving in between start with a cancelled token and die at once.
///
/// `close_gate()` and `cancel.cancel()` are both synchronous calls with no `.await`, so
/// the gap between them is nanoseconds — observing from outside with a `sleep` cannot
/// distinguish the order, because in either order both have finished by the time the sleep
/// ends (which is what actually happened in the first implementation — this test did not
/// react to a mutation swapping the order). Only by attempting a `spawn` inside that
/// window through `ShutdownHook` can the order really be checked.
///
/// The correct order: when the hook runs the gate is already closed and cancellation has
/// not yet fired — `spawn` must be refused with `GateClosed`.
/// With the order inverted: when the hook runs cancellation has already fired while the
/// gate is still open — `spawn` **succeeds** and a task holding an already-cancelled token
/// enters the `JoinSet`. That task takes the already-ready `cancel.cancelled()` branch
/// before `mark_running` — `run_investigation`'s first action, a real database round trip
/// that is necessarily pending on first poll — even starts, firing only
/// `bus.publish_terminal` and returning. Because `mark_running` was never called the
/// investigation stays at `queued` without moving a step, and shutdown stage 6 sweeps
/// only `status = 'running'`, so it never notices this investigation at all.
#[sqlx::test(migrations = "../../migrations")]
async fn shutdown_closes_the_gate_before_cancelling(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let jm = manager(store.clone(), ok_provider());
    let id = queued(&store).await;

    let jm_hook = jm.clone();
    let hook: agentops_server::jobs::ShutdownHook = Box::new(move || {
        Box::pin(async move {
            assert!(
                jm_hook.spawn(id).is_err(),
                "the gate closed after cancellation — an investigation was accepted in between"
            );
        })
    });

    jm.shutdown_with_hook(Duration::from_secs(1), Some(hook))
        .await;

    // Being refused, this investigation must stay at `queued` without ever reaching
    // `running`. If the order were inverted and the spawn inside the hook succeeded (the
    // hook's assertion above already catches that), the accepted task would be cancelled
    // before even calling `mark_running` and leave the same `queued` state — exactly the
    // brief's claim that it "dies with no record".
    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(
        inv.status,
        InvestigationStatus::Queued,
        "the investigation transitioned to {:?} — that state cannot arise without passing through running",
        inv.status
    );
}

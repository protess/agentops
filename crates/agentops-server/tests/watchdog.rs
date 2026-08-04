//! The two-layer zombie investigation defense — the watchdog sweep and boot cleanup (spec Sections 6.1 INV-4 and 12.1 TEST-8).

use agentops_core::{
    Investigation, InvestigationStatus, LlmError, LlmEvent, LlmProvider, LlmRequest, Phase,
    StepKind, StopReason, Store, TerminalReason, TriggeredBy, Usage,
};
use agentops_server::bus::StepBus;
use agentops_server::jobs::{JobDeps, JobManager};
use agentops_server::watchdog::{recover_on_boot, sweep_once};
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
        title: "high latency".into(),
        prompt: "why is p99 up".into(),
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

fn manager(store: Arc<PgStore>, provider: Arc<dyn LlmProvider>) -> JobManager<PgStore> {
    JobManager::new(
        store,
        StepBus::new(),
        JobDeps {
            provider,
            connections: Vec::new(),
            limits: agentops_agent::limits::Limits::default(),
        },
    )
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
    ) -> Result<agentops_core::BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
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

/// A helper that briefly disables the trigger and pushes `updated_at` into the past.
/// Without disabling it, the backdate undoes itself (plan 2's `agentops-store/tests`
/// really was caught by this trap).
///
/// The `interval` is passed as a bind parameter rather than string interpolation — sqlx
/// rejects a dynamically assembled SQL string at compile time (`SqlSafeStr`). Seconds go
/// to `make_interval` the same way as in `stale_running_ids`.
async fn backdate(store: &PgStore, id: Uuid, ago: Duration) {
    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query("ALTER TABLE investigations DISABLE TRIGGER t_investigations_touch")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE investigations SET updated_at = now() - make_interval(secs => $1) WHERE id = $2",
    )
    .bind(ago.as_secs_f64())
    .bind(id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE investigations ENABLE TRIGGER t_investigations_touch")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// TEST-8 — a `running` investigation whose `updated_at` was backdated is cleaned up as `failed`.
#[sqlx::test(migrations = "../../migrations")]
async fn test_8_a_stalled_running_investigation_is_reclaimed(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    backdate(&store, id, Duration::from_secs(20 * 60)).await;

    let n = sweep_once(&store, &StepBus::new(), Duration::from_secs(15 * 60))
        .await
        .unwrap();

    assert_eq!(n, 1);
    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Failed);
}

/// An active investigation is not reclaimed. Without the threshold check the watchdog
/// kills in-flight investigations — a worse state than having no watchdog.
#[sqlx::test(migrations = "../../migrations")]
async fn a_fresh_running_investigation_is_left_alone(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;

    let n = sweep_once(&store, &StepBus::new(), Duration::from_secs(15 * 60))
        .await
        .unwrap();

    assert_eq!(n, 0, "it reclaimed an investigation that just started");
    assert_eq!(
        store.get_investigation(id).await.unwrap().status,
        InvestigationStatus::Running
    );
}

/// F1 — **the watchdog's reclamation is owned by the terminal transaction too.**
/// Splitting into `append_step` plus a separate `UPDATE` would reproduce for a fourth time
/// the defect plan 2 met three times: lose the race and the status is someone else's while
/// the terminal row is yours. A reclaimed investigation must have **exactly one** terminal step.
#[sqlx::test(migrations = "../../migrations")]
async fn f1_reclaimed_investigation_has_exactly_one_terminal_step(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    store
        .append_step(
            id,
            Phase::Triage,
            &StepKind::Text {
                text: "work".into(),
            },
        )
        .await
        .unwrap();
    backdate(&store, id, Duration::from_secs(20 * 60)).await;

    sweep_once(&store, &StepBus::new(), Duration::from_secs(15 * 60))
        .await
        .unwrap();

    let steps = store.steps_after(id, -1).await.unwrap();
    let terminal = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Terminated { .. }))
        .count();
    assert_eq!(terminal, 1, "there are {terminal} terminal steps");
}

/// F1 — **verified with real concurrency.** The single-execution test above shows only
/// that the normal path leaves one terminal step and never constructs the race F1 prevents
/// (a second writer intervening) — the team lead's review pointed at exactly that gap.
/// The watchdog's reclamation and another terminal writer (a direct `fail_investigation`
/// call imitating shutdown) are launched against the same investigation with
/// `tokio::join!` (the same pattern as `jobs.rs::test_13_...` and `shutdown.rs::test_13_...`).
/// Because `fail_investigation` writes the status and the terminal step together in one
/// transaction, exactly one must win and **the winner's reason must match the surviving
/// terminal step's reason.** Split into `append_step` plus a separate `UPDATE`, the loser's
/// already-written terminal row survives and contradicts the winner's status — the fourth
/// place the defect plan 2 met three times reproduces.
///
/// **A known limitation (measured, recorded after re-testing at the team lead's
/// instruction):** reverting `sweep_once` to the F1 violation (mutation 1) and re-testing
/// with this test 15 consecutive times **never caught it once.** The cause is a timing
/// bias — the split write runs only two statements with no `pool.begin()` and its round
/// trips are short, while an honest `fail_investigation` goes through four:
/// `BEGIN`, a conditional `UPDATE`, an `INSERT`, `COMMIT`. As a result the split write
/// finishes the status transition entirely first every time, `other_writer`'s conditional
/// `UPDATE` always loses with `Conflict`, and the two assertions this test requires
/// ("exactly one won", "the winning reason matches the terminal step") **hold by accident**
/// even with the defect present — not because they really contend but because the split
/// write wins overwhelmingly every time. A whole-call race at the `tokio::join!` level
/// cannot detect F1: the real defect occurs in **the window between the two statements**,
/// and opening that window deterministically needs a synchronization point like
/// `jobs.rs::ShutdownHook` or `stream.rs::Hook`, which would change `sweep_once`'s
/// internal structure — outside this task's scope, so it is not built and only the fact is
/// recorded. The test still has value — in a **correct implementation** it pins the contract "exactly one wins and the reason matches" under real contention and prevents regressions.
#[sqlx::test(migrations = "../../migrations")]
async fn f1_concurrent_reclaim_and_another_terminal_writer_leave_exactly_one_winner(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    backdate(&store, id, Duration::from_secs(20 * 60)).await;

    let watchdog = {
        let store = store.clone();
        async move { sweep_once(&store, &StepBus::new(), Duration::from_secs(15 * 60)).await }
    };
    let other_writer = {
        let store = store.clone();
        async move {
            store
                .fail_investigation(id, &TerminalReason::ShutdownRequested)
                .await
        }
    };

    let (watchdog_result, other_result) = tokio::join!(watchdog, other_writer);

    let watchdog_won = matches!(watchdog_result, Ok(1));
    let other_won = other_result.is_ok();
    assert_ne!(
        watchdog_won, other_won,
        "exactly one must win (watchdog={watchdog_result:?}, other={other_result:?})"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Terminated { .. }))
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there are {} terminal steps: {terminated:#?}",
        terminated.len()
    );

    // F1's core assertion: the winner's reason must match the persisted terminal step's
    // reason. With a split writer (append_step plus a separate UPDATE) the loser's
    // earlier-written terminal row can survive and contradict the winner's status.
    let expected_reason = if watchdog_won {
        TerminalReason::WallClockExceeded
    } else {
        TerminalReason::ShutdownRequested
    };
    assert!(
        matches!(
            &terminated[0].kind,
            StepKind::Terminated { reason, .. } if *reason == expected_reason
        ),
        "the winner's ({}) reason disagrees with the surviving terminal step: {:?}",
        if watchdog_won { "watchdog" } else { "other" },
        terminated[0].kind
    );
}

/// The SSE subscriber of a reclaimed investigation receives `terminal`. Without it the
/// browser holds a finished investigation's connection forever with only heartbeats flowing.
///
/// **It is wrapped in a timeout** — an earlier round measured that this `recv()` without a
/// timeout (on a regression deleting `publish_terminal`, say) becomes an infinite hang
/// rather than a clean FAIL, making CI look like an infrastructure problem instead of a
/// failure. The same pattern as `stream.rs::collect_stream_for_test` — it panics naming
/// what failed to arrive.
#[sqlx::test(migrations = "../../migrations")]
async fn reclaimed_investigations_notify_their_subscribers(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    backdate(&store, id, Duration::from_secs(20 * 60)).await;
    let bus = StepBus::new();
    let mut rx = bus.subscribe(id);

    sweep_once(&store, &bus, Duration::from_secs(15 * 60))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("the Terminal event did not arrive within 20 seconds"))
        .unwrap();
    assert!(matches!(
        event,
        agentops_server::bus::BusEvent::Terminal { .. }
    ));
}

/// Boot cleanup — `running` becomes `failed` and `queued` is rescheduled.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_4_boot_recovery_fails_running_and_requeues_queued(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let orphan = running(&store).await;
    let waiting = queued(&store).await;

    let jm = manager(store.clone(), ok_provider());
    let report = recover_on_boot(&store, &jm).await.unwrap();

    assert_eq!(report.failed, 1);
    assert_eq!(report.requeued, 1);
    assert_eq!(
        store.get_investigation(orphan).await.unwrap().status,
        InvestigationStatus::Failed
    );
    jm.wait_idle(Duration::from_secs(20)).await;
    assert_eq!(
        store.get_investigation(waiting).await.unwrap().status,
        InvestigationStatus::Completed,
        "the queued investigation was not rescheduled"
    );
}

/// M2 — can steps still be appended to a terminated investigation?
/// **The behavior is not changed.** The current behavior is pinned by this test and handed
/// to the final review's triage (plan 1's contract is not changed in this task).
#[sqlx::test(migrations = "../../migrations")]
async fn m2_appending_to_a_terminal_investigation(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;
    store
        .fail_investigation(id, &TerminalReason::ShutdownRequested)
        .await
        .unwrap();
    let r = store
        .append_step(
            id,
            Phase::Triage,
            &StepKind::Text {
                text: "after".into(),
            },
        )
        .await;
    // This **records** the current behavior. If this assertion flips, the behavior changed.
    assert!(r.is_ok(), "today it succeeds — this fact is pinned");
}

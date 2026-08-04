use agentops_agent::limits::Limits;
use agentops_agent::phase::DeltaSink;
use agentops_core::{
    BoxStream, Investigation, InvestigationStatus, LlmError, LlmEvent, LlmProvider, LlmRequest,
    NewArtifact, Phase, StepKind, Store, TriggeredBy,
};
use agentops_server::bus::StepBus;
use agentops_server::jobs::{JobDeps, JobManager};
use agentops_server::stream::{clamp_after, collect_stream_for_test, StreamCtx};
use agentops_store::PgStore;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

/// This test file spawns no investigation, so `stream()` is never called — a placeholder
/// needed only to fill `StreamCtx.jobs` (the same reason as the identically named type in
/// `tests/pages.rs`).
struct NullProvider;

#[async_trait]
impl LlmProvider for NullProvider {
    fn model_id(&self) -> &str {
        "null"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
}

/// A helper that fills `StreamCtx.jobs`. All most tests need is that `cancel_token()`
/// yields a valid (not yet cancelled) token, so its contents do not matter — only the
/// test that verifies cancellation itself fires `jobs.cancel_token()` directly.
///
fn test_jobs(store: Arc<PgStore>, bus: StepBus) -> JobManager<PgStore> {
    JobManager::new(
        store,
        bus,
        JobDeps {
            provider: Arc::new(NullProvider),
            connections: Vec::new(),
            limits: Limits::default(),
        },
    )
}

async fn running(store: &PgStore) -> Uuid {
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
    store.mark_running(inv.id).await.unwrap();
    inv.id
}

async fn append(store: &PgStore, id: Uuid, n: i64) -> i64 {
    store
        .append_step(
            id,
            Phase::Triage,
            &StepKind::Text {
                text: format!("t{n}"),
            },
        )
        .await
        .unwrap()
}

/// TEST-18 — clamping `after`. Opening with a negative or oversized value is not an error.
/// A client arriving with a stale value must still get a stream (spec Section 10.2.1).
#[test]
fn test_18_after_is_clamped_not_rejected() {
    // With no steps, max is None, so any requested value gives -1 (a full replay)
    assert_eq!(clamp_after(-999, None), -1);
    assert_eq!(clamp_after(50, None), -1);
    // When the maximum seq is 4
    assert_eq!(clamp_after(-1, Some(4)), -1, "-1 means from the beginning");
    assert_eq!(clamp_after(-999, Some(4)), -1, "a negative clamps to -1");
    assert_eq!(clamp_after(2, Some(4)), 2);
    assert_eq!(clamp_after(4, Some(4)), 4);
    assert_eq!(
        clamp_after(9999, Some(4)),
        4,
        "an oversized value clamps to the maximum seq"
    );
}

/// TEST-1 — the subscribe-replay race. The `after_replay` hook runs **after replay ends
/// and before the live loop begins** — not between subscription and replay. In that
/// position the database row the hook writes always commits before the replay query (the
/// hook completes with `.await`) and replay picks that row up directly regardless of where
/// the subscription sits — which cannot verify the subscribe-replay order (confirmed
/// empirically; see the implementation note below). Moving the hook after replay means an
/// event the hook writes and broadcasts can only arrive through the live channel, so when
/// the subscription is late (mutation A) its loss can actually be observed.
///
///
/// This test is reproducible because `collect_stream_for_test` invokes the hook
/// deterministically. It does not depend on real timing.
///
/// Implementation note: the original brief placed the hook immediately after subscribing
/// and before replay. With that placement, applying mutation A (moving the subscription
/// after replay) left this test passing and broke `terminal_event_closes_the_stream`
/// instead — the hook's database write always finished before the replay query and replay
/// read the row directly, so the subscription's position could not affect the result.
/// That finding moved the hook to after replay and before the live loop (see
/// `docs/log.md` and task-3-report.md).
#[sqlx::test(migrations = "../../migrations")]
async fn test_1_step_written_between_subscribe_and_replay_arrives_exactly_once(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    append(&store, id, 0).await;

    let bus = StepBus::new();
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };

    // After replay emits 0 (the subscription must already have happened), it writes and
    // broadcasts seq=1. If the subscription came before replay this event is delivered
    // through the live channel as-is. If the subscription is late (mutation A) it is sent
    // with no subscriber and is lost.
    let after_replay = {
        let store = store.clone();
        let bus = bus.clone();
        move || {
            let store = store.clone();
            let bus = bus.clone();
            Box::pin(async move {
                let seq = append(&store, id, 1).await;
                let steps = store.steps_after(id, seq - 1).await.unwrap();
                bus.publish_step(steps.into_iter().next().unwrap());
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };

    let seqs = collect_stream_for_test(ctx, Some(Box::new(after_replay)), 2).await;
    assert_eq!(
        seqs,
        vec![0, 1],
        "a step arriving on the live channel after replay was lost or duplicated"
    );
}

/// INV-2 — subscription must happen before replay. The same underlying scenario as
/// TEST-1, narrowed to a minimal configuration with nothing to replay at all (an entirely
/// empty database), checking only that the single event which can arrive solely through
/// the live channel arrives without loss. `check_spec_test_ids.py` confirms ID presence by
/// the `inv_2_` prefix in the function name, so a separate function is needed even when
/// verifying the same property as TEST-1 — spec Section 12.1 declares `[TEST-1]` and
/// `[INV-2]` as different IDs (Section 6.1's INV-4 already has this 1:N pattern).
#[sqlx::test(migrations = "../../migrations")]
async fn inv_2_subscribe_happens_before_replay_or_the_live_step_is_lost(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    let bus = StepBus::new();
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };

    // With nothing to replay (an empty database) the only item must pass through the live
    // channel — if the subscription came after this hook it is sent with no subscriber and
    // is lost forever.
    let after_replay = {
        let store = store.clone();
        let bus = bus.clone();
        move || {
            let store = store.clone();
            let bus = bus.clone();
            Box::pin(async move {
                let seq = append(&store, id, 0).await;
                let steps = store.steps_after(id, seq - 1).await.unwrap();
                bus.publish_step(steps.into_iter().next().unwrap());
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };

    let seqs = collect_stream_for_test(ctx, Some(Box::new(after_replay)), 1).await;
    assert_eq!(
        seqs,
        vec![0],
        "with the subscription after replay, the live channel's only step is lost"
    );
}

/// TEST-2 — deduplication. When the replay range and the channel range overlap, the same
/// `seq` is not delivered twice.
///
/// The `after_replay` hook re-pushes the already-replayed 1 and 2 and **then** writes and
/// pushes a genuinely new step 3. `want=4` keeps the collector from stopping at replay's
/// three (0, 1, 2) and forces it into the live loop. Without deduplication (mutation B)
/// the live loop emits the duplicate (seq=1) as-is, it becomes the collector's fourth
/// item giving `[0,1,2,1]`, which disagrees with `[0,1,2,3]`.
///
#[sqlx::test(migrations = "../../migrations")]
async fn test_2_overlapping_replay_and_live_ranges_do_not_duplicate(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    for n in 0..3 {
        append(&store, id, n).await;
    }
    let bus = StepBus::new();

    let replayed = store.steps_after(id, -1).await.unwrap();
    let dup: Vec<_> = replayed.iter().filter(|s| s.seq >= 1).cloned().collect();

    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };
    let after_replay = {
        let store = store.clone();
        let bus = bus.clone();
        move || {
            let store = store.clone();
            let bus = bus.clone();
            let dup = dup.clone();
            Box::pin(async move {
                for s in dup {
                    bus.publish_step(s);
                }
                let seq = append(&store, id, 3).await;
                let steps = store.steps_after(id, seq - 1).await.unwrap();
                bus.publish_step(steps.into_iter().next().unwrap());
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };

    let seqs = collect_stream_for_test(ctx, Some(Box::new(after_replay)), 4).await;
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3],
        "a duplicate was delivered in the overlapping range"
    );
}

/// TEST-3 — `Lagged` recovery. It creates a slow consumer by exceeding the channel
/// capacity and checks that no step is lost thanks to the database resync after `Lagged`.
///
/// The database starts empty so replay emits nothing — `want=400` cannot be satisfied by
/// replay alone and the collector must enter the live loop. The `after_replay` hook writes
/// and broadcasts 400 items at once — far beyond the channel capacity of 256 — with
/// nobody draining. Because the hook itself runs entirely inside the producer task,
/// `Lagged` necessarily occurs (deterministic by construction, not by race).
///
///
/// **Ignoring `Lagged` makes this test fail** — which is the point of this test.
#[sqlx::test(migrations = "../../migrations")]
async fn test_3_lagged_recovers_by_resyncing_from_the_database(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    let bus = StepBus::new();

    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };

    let total = 400i64;
    let after_replay = {
        let store = store.clone();
        let bus = bus.clone();
        move || {
            let store = store.clone();
            let bus = bus.clone();
            Box::pin(async move {
                for n in 0..total {
                    let seq = append(&store, id, n).await;
                    let s = store
                        .steps_after(id, seq - 1)
                        .await
                        .unwrap()
                        .into_iter()
                        .next()
                        .unwrap();
                    bus.publish_step(s);
                }
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };

    let seqs = collect_stream_for_test(ctx, Some(Box::new(after_replay)), total as usize).await;
    assert_eq!(
        seqs,
        (0..total).collect::<Vec<_>>(),
        "the resync after Lagged lost steps (it must be 0..{total} with no gaps)"
    );
}

/// INV-3 — broadcast lag (`Lagged`) must be handled. It reuses the same deterministic lag
/// construction as TEST-3 (pushing past the capacity of 256 with nobody draining) but with
/// a count of 300 to leave a separate observation. A separate function is needed for the
/// same reason as `inv_2_...` — the spec declares `[TEST-3]` and `[INV-3]` as different
/// IDs.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_3_lagged_must_resync_or_steps_vanish_silently(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    let bus = StepBus::new();

    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };

    let total = 300i64;
    let after_replay = {
        let store = store.clone();
        let bus = bus.clone();
        move || {
            let store = store.clone();
            let bus = bus.clone();
            Box::pin(async move {
                for n in 0..total {
                    let seq = append(&store, id, n).await;
                    let s = store
                        .steps_after(id, seq - 1)
                        .await
                        .unwrap()
                        .into_iter()
                        .next()
                        .unwrap();
                    bus.publish_step(s);
                }
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };

    let seqs = collect_stream_for_test(ctx, Some(Box::new(after_replay)), total as usize).await;
    assert_eq!(
        seqs,
        (0..total).collect::<Vec<_>>(),
        "the resync after Lagged lost steps (it must be 0..{total} with no gaps)"
    );
}

/// The SSE-layer counterpart of TEST-4 — opening with `after={seq}` receives only what
/// follows. Plan 1 verified `steps_after` at the store layer; here the question is
/// **whether the stream actually uses that value**.
#[sqlx::test(migrations = "../../migrations")]
async fn test_4_stream_opened_with_after_replays_only_later_steps(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    for n in 0..5 {
        append(&store, id, n).await;
    }
    let bus = StepBus::new();
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: 2,
        jobs: test_jobs(store.clone(), bus.clone()),
    };
    let seqs = collect_stream_for_test(ctx, None, 2).await;
    assert_eq!(seqs, vec![3, 4]);
}

/// The stream closes when `terminal` arrives. Without that the browser keeps the
/// connection open with only heartbeats flowing after the investigation ended (spec Section 10.2.1).
#[sqlx::test(migrations = "../../migrations")]
async fn terminal_event_closes_the_stream(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    let bus = StepBus::new();
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };
    let hook = {
        let bus = bus.clone();
        move || {
            let bus = bus.clone();
            Box::pin(async move { bus.publish_terminal(id) })
                as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };
    // It asks for 100 but terminal ends it early and none arrive.
    let seqs = collect_stream_for_test(ctx, Some(Box::new(hook)), 100).await;
    assert!(seqs.is_empty(), "the stream stayed alive after terminal");
}

/// Task 3 review Important — connecting anew to an already-terminated investigation (a
/// reconnect, an old link) must close the stream by itself. Before the fix, no future
/// event arrived on this channel, the live loop's `sub.recv().await` blocked forever, and
/// the task leaked — a scenario the review reproduced directly with a temporary test.
///
/// It transitions to `completed` through a real transaction with
/// `store.complete_investigation` (unlike merely imitating `publish_terminal` on the bus,
/// this also verifies the status check reads a genuinely committed database value).
/// With `want` set large it fails on a 20-second timeout before the fix and, after it,
/// receives only the two replayed steps (text plus `ArtifactWritten`) and closes at once on `Terminal`.
#[sqlx::test(migrations = "../../migrations")]
async fn stream_opened_on_a_completed_investigation_closes_instead_of_hanging(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    append(&store, id, 0).await;
    store
        .complete_investigation(
            id,
            &NewArtifact {
                title: "t".into(),
                body: "b".into(),
            },
        )
        .await
        .unwrap();

    let bus = StepBus::new();
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };
    // It asks for 100 but, once fixed, receives only the two replayed items (text plus the
    // artifact) and closes on Terminal — before the fix this timed out here at 20 seconds.
    let seqs = collect_stream_for_test(ctx, None, 100).await;
    assert_eq!(
        seqs.len(),
        2,
        "an already-terminated investigation must close immediately after replay (it must not block forever in the live loop)"
    );
}

/// Task 3 re-review Minor, fixed — the stream must close even when `Terminal` itself is
/// pushed out of the channel by `Lagged`. `Terminal` uses the same broadcast channel as
/// `Step` and `Delta` (`bus.rs`), so if deltas (per token, no database round trip) pour
/// past the channel capacity of 256 right after `publish_terminal` with nobody draining,
/// `Terminal` is evicted as the oldest item.
///
/// The hook commits the status as `completed` through a real `complete_investigation`
/// transaction, **then** calls `publish_terminal`, and immediately pushes enough deltas
/// (300, comfortably past the capacity of 256) to evict `Terminal`. The subscriber has
/// drained nothing yet, so the first `recv()` necessarily gives `Lagged`, and by then
/// `Terminal` is already gone — resyncing steps alone cannot catch this situation (the
/// `agent_steps` being resynced never carried this information). Only a status recheck in
/// the `Lagged` handler closes it.
#[sqlx::test(migrations = "../../migrations")]
async fn lagged_recovery_rechecks_terminal_status_when_the_terminal_event_itself_is_evicted(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    let bus = StepBus::new();
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };

    let after_replay = {
        let store = store.clone();
        let bus = bus.clone();
        move || {
            let store = store.clone();
            let bus = bus.clone();
            Box::pin(async move {
                // Commit the status through a real transaction — verifying that the
                // `Lagged` handler's recheck reads a genuinely committed value, not a simulation.
                store
                    .complete_investigation(
                        id,
                        &NewArtifact {
                            title: "t".into(),
                            body: "b".into(),
                        },
                    )
                    .await
                    .unwrap();
                bus.publish_terminal(id);
                // Push far past the channel capacity (256) so `Terminal` (the oldest item)
                // is evicted. Deltas have no database round trip and accumulate far faster
                // than `Step`s — an amount one streaming response commonly produces.
                for _ in 0..300 {
                    bus.text(id, Phase::Triage, "x");
                }
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };

    // The `ArtifactWritten` step (seq=0) that `complete_investigation` commits is not
    // broadcast (broadcasting is the responsibility of the not-yet-existing JobManager),
    // so only the `Lagged` resync catches it — which means the resync works. The point is
    // not to satisfy `want=50` but that after receiving that one it closes on `Terminal`
    // rather than timing out.
    let seqs = collect_stream_for_test(ctx, Some(Box::new(after_replay)), 50).await;
    assert_eq!(
        seqs,
        vec![0],
        "the Lagged resync should have caught the remaining step, and without the Terminal recheck the stream never closes afterwards"
    );
}

/// Final review C-1 regression guard — even when the investigation does not end on its
/// own, once `cancel_token()` fires the live loop must send `Terminal` and close rather
/// than park forever at `sub.recv()`.
///
/// The investigation is left `running` (nobody terminates it) and the hook fires
/// `jobs.cancel_token().cancel()` after replay and just before entering the live loop.
/// Before the fix, `async_stream_impl` could not see cancellation at `sub.recv().await`
/// and this test failed on a 20-second timeout (inside `collect_stream_for_test`) — in
/// reality it manifests as axum's SSE connection task never ending and graceful shutdown
/// blocking indefinitely (reproduced by `tests/serve_shutdown.rs` against a real
/// `axum::serve`).
#[sqlx::test(migrations = "../../migrations")]
async fn cancellation_ends_the_live_loop_instead_of_parking_forever(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running(&store).await;
    let bus = StepBus::new();
    let jobs = test_jobs(store.clone(), bus.clone());
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: id,
        after: -1,
        jobs: jobs.clone(),
    };

    let cancel_after_replay = {
        let jobs = jobs.clone();
        move || {
            Box::pin(async move {
                jobs.cancel_token().cancel();
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    };

    // With no steps to replay (an empty database) the only way to satisfy `want` is an
    // early exit on `Terminal` — `collect_stream_for_test` returns immediately on
    // `Terminal` or `None`.
    let seqs = tokio::time::timeout(
        Duration::from_secs(10),
        collect_stream_for_test(ctx, Some(Box::new(cancel_after_replay)), 1),
    )
    .await
    .expect("the stream did not end within 10 seconds of cancellation — a C-1 regression");
    assert!(
        seqs.is_empty(),
        "cancellation produces no step — there must be only Terminal"
    );
}

/// Final review I-3 regression guard — opening a stream on a nonexistent investigation id
/// (an old link, a bookmark pointing at a deleted investigation) must close at once.
///
/// Before the fix, `close_if_investigation_is_terminal` treated `StoreError::NotFound`
/// exactly like a `Backend` error as "no evidence of termination" and entered the live
/// loop, where no `JobManager` task runs for this id and `sub.recv()` blocked forever —
/// leaking a 256-slot channel and a parked task per request. It asks for `want=1`, but the
/// only item that actually arrives is `Terminal`, so `collect_stream_for_test` must return
/// immediately.
#[sqlx::test(migrations = "../../migrations")]
async fn stream_opened_on_a_nonexistent_investigation_closes_instead_of_leaking(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    let missing_id = Uuid::new_v4(); // `create_investigation` was never called — it is not in the database.
    let bus = StepBus::new();
    let ctx = StreamCtx {
        store: store.clone(),
        bus: bus.clone(),
        investigation_id: missing_id,
        after: -1,
        jobs: test_jobs(store.clone(), bus.clone()),
    };

    let seqs = tokio::time::timeout(
        Duration::from_secs(10),
        collect_stream_for_test(ctx, None, 1),
    )
    .await
    .expect("a stream opened on a nonexistent investigation did not close within 10 seconds — an I-3 regression");
    assert!(
        seqs.is_empty(),
        "a nonexistent investigation can produce no step — there must be only Terminal"
    );
}

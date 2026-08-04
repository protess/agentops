//! The SSE stream — an investigation's observer.
//!
//! It implements the ordering of spec Section 6.1, invariant 2, exactly:
//!
//! ```text
//! 1. subscribe to the broadcast channel first        <- the order matters
//! 2. replay with Store::steps_after(id, after) and update emitted_seq
//! 3. drain the already-subscribed channel, discarding seq <= emitted_seq (deduplication)
//! 4. then deliver live
//! 5. RecvError::Lagged -> resync with Store::steps_after(id, emitted_seq) and continue
//! ```
//!
//! **Subscribe before replaying.** In the opposite order, any step written between the
//! query and the subscription is lost — precisely when the investigation is most active.
//!
//! **The test hook runs after replay and before the live loop.** It was first placed
//! immediately after subscribing and before replay, where the database row the hook wrote
//! always commits before the replay query (the hook completes with `.await`) and replay
//! picks that row up directly, regardless of where the subscription sits. As a result the
//! mutation inverting the subscribe-replay order (A), the one deleting deduplication (B),
//! and the one ignoring `Lagged` (C) all had their effect absorbed by the replay path, and
//! `collect_stream_for_test` reached its target count and returned before ever entering the
//! live loop, which is where deduplication and `Lagged` recovery actually live — none of
//! the three broke any test (observed: see `docs/log.md` and task-3-report.md). Moving the
//! hook after replay means an event the hook writes and broadcasts can only arrive through
//! the live channel — which is what makes the three invariants actually verified.
//!
//! **Immediately after replay it checks whether the investigation has already terminated
//! (Task 3 review, Important).** Connecting anew to an investigation already `completed`
//! or `failed` (a reconnect, an old link) means no future event will ever arrive on its
//! channel — entering the live loop without checking blocks `sub.recv().await` forever,
//! and because the producer task has no occasion to attempt a `send()` it never detects
//! the disconnect and leaks until the process exits. `Store::get_investigation`'s `status`
//! is the authority — `complete_investigation` and `fail_investigation` (plan 1) commit
//! the status transition and the terminal step insert in one transaction, so status and
//! terminal step always agree, and there is one more query's worth of grounds to trust
//! than the already-committed replay result (more direct than inferring from the last replayed step's kind).
//!
//! The race window: what if the investigation terminates between this check and entering
//! the live loop? **There is none.** The subscription (stage 1) already happened before
//! this check, so even if the terminal transaction commits and `publish_terminal` fires in
//! between, that event is buffered in the already-active subscription channel — on
//! entering the live loop `sub.recv()` receives that `Terminal` and closes normally.
//! Whether the check saw `running` or saw `completed` on the commit-timing side, either
//! way the stream eventually closes.
//!
//! **`Lagged` recovery must recheck the terminal status too (re-review Minor, fixed).**
//! `Terminal` shares **the same channel** as `Step` and `Delta` (`bus.rs`) — deltas are
//! per token and go out far faster than a `Step`, which needs a database round trip, so
//! 256 deltas (the capacity) pouring out during the two or three database round trips of
//! subscription setup (replay query, status check, hook) is not exceptional — if `Terminal`
//! is pushed out of the channel by that flood (the oldest item is discarded first), the
//! subscriber sees only `Lagged`. If the `Lagged` handler resyncs steps without asking
//! about the status again, it blocks forever once more, waiting on a `Terminal` that was
//! already broadcast and lost — **the very same leak** fixed above, left on this one path.
//! So the same status check is repeated after the `Lagged` resync.

use crate::bus::{BusEvent, StepBus};
use crate::jobs::JobManager;
use crate::render::{delta_html, step_html};
use crate::AppState;
use agentops_core::{Store, StoreError};
use agentops_store::PgStore;
use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    response::IntoResponse,
};
use futures_core::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

/// Spec Section 10.2.1 — stops proxies closing an idle connection.
const HEARTBEAT: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize)]
pub struct AfterParam {
    #[serde(default = "default_after")]
    pub after: i64,
}

fn default_after() -> i64 {
    -1
}

/// Spec Section 10.2.1 — an out-of-range value is clamped rather than made an error.
/// A client arriving with a stale value must still get a stream.
///
/// `-1` means "from the beginning". `steps_after` queries with `seq > after`, so passing
/// `0` would skip the first step.
pub fn clamp_after(requested: i64, max_seq: Option<i64>) -> i64 {
    match max_seq {
        None => -1,
        Some(max) => requested.clamp(-1, max),
    }
}

pub struct StreamCtx {
    pub store: Arc<PgStore>,
    pub bus: StepBus,
    pub investigation_id: Uuid,
    pub after: i64,
    /// The producer task is put here rather than detached with `tokio::spawn`, so graceful
    /// shutdown can wait for it, and `cancel_token()` breaks the live loop's indefinite
    /// parking (final review C-1 — this file had the same defect as
    /// `routes::chat::stream`).
    pub jobs: JobManager<PgStore>,
}

/// The type of the hook a test interposes **after replay and before the live loop**.
/// Only in that position can an event the hook creates arrive solely through the live
/// channel — placed immediately after subscribing and before replay, the hook's database
/// write always commits before the replay query and replay picks the row up directly, so
/// breaking the subscription order, the deduplication, or the `Lagged` handling produces no test reaction.
pub type Hook =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// An item to emit over SSE. Tests inspect only `seq`, so it is built in pre-render form.
enum Item {
    Step { seq: i64, html: String },
    Delta { html: String },
    Terminal,
}

/// If the investigation has already terminated (`completed`/`failed`) it sends `Terminal`
/// and returns `true` — the caller must then close the stream.
///
/// It is called in **two places**: immediately after replay and immediately after a
/// `Lagged` resync. Without the latter, a `Terminal` pushed out of the channel by `Lagged`
/// blocks the stream forever again (see the module documentation).
///
/// **It distinguishes `NotFound` from other errors** (final review I-3). `NotFound` is a
/// permanent fact — a stream opened on a nonexistent id gets an empty list from
/// `steps_after` (not an error) and this check will only ever return `NotFound`, so
/// treating it as "no evidence of termination" and continuing blocks `sub.recv()` forever
/// and leaks a 256-slot channel, a parked task, and (before the fix) a connection per
/// request. Other errors (`Backend` and the like, a transient database failure) are still
/// treated as "no evidence of termination", logged, and continued — safer than closing
/// wrongly and cutting a live stream. The same distinction Task 10 made between 404 and
/// 500 in `investigation_detail`, at the place that fix had not been generalized to.
async fn close_if_investigation_is_terminal(
    store: &PgStore,
    investigation_id: Uuid,
    tx: &tokio::sync::mpsc::Sender<Item>,
) -> bool {
    match store.get_investigation(investigation_id).await {
        Ok(inv) if inv.status.is_terminal() => {
            let _ = tx.send(Item::Terminal).await;
            true
        }
        Ok(_) => false,
        Err(StoreError::NotFound) => {
            let _ = tx.send(Item::Terminal).await;
            true
        }
        Err(e) => {
            tracing::error!(error = %e, "investigation status check failed; continuing");
            false
        }
    }
}

/// Performs stages 1 through 5 of invariant 2 exactly, producing items as it goes.
///
/// `hook` runs **immediately after replay and immediately before the live loop.** `None`
/// in production. It is the only way a test can reproduce the race deterministically.
fn items(ctx: StreamCtx, hook: Option<Hook>) -> impl Stream<Item = Item> + Send {
    async_stream_impl(ctx, hook)
}

fn async_stream_impl(ctx: StreamCtx, hook: Option<Hook>) -> impl Stream<Item = Item> + Send {
    // Implemented with a channel plus a task to avoid adding an `async_stream` dependency.
    let (tx, rx) = tokio::sync::mpsc::channel::<Item>(64);
    // **It does not use `tokio::spawn` directly** — putting it on `ctx.jobs.spawn_task`
    // makes graceful shutdown wait for this task. **`cancel_token()` is watched together
    // with the live loop's `sub.recv()` in a `select!`** — without it, when the premise
    // that `Terminal` arrives only from investigation completion (not from shutdown) is
    // broken (the chat stream's situation: if nobody terminates this investigation),
    // `sub.recv()` parks forever, the SSE body never ends, and axum's graceful shutdown
    // waits on that connection task indefinitely (final review C-1).
    let jobs = ctx.jobs.clone();
    let cancel = jobs.cancel_token();
    // **`spawn_task` itself guarantees the task's lifetime** — this async block need not
    // reference `ctx.jobs` (need not be captured by disjoint capture). Previously
    // `keep_alive` was carried here directly, and one of the three call sites (the
    // `reply()` spawned by `routes/chat.rs::send`) missed it, leading to a real defect
    // where the response was not stored (final review N-1). It was therefore moved into
    // `JobManager::spawn_task` so nothing relies on caller discipline.
    jobs.spawn_task(async move {
        // 1. Subscribe first.
        let mut sub = ctx.bus.subscribe(ctx.investigation_id);

        // 2. Replay.
        let mut emitted_seq = ctx.after;
        match ctx.store.steps_after(ctx.investigation_id, ctx.after).await {
            Ok(steps) => {
                for s in steps {
                    emitted_seq = emitted_seq.max(s.seq);
                    if tx
                        .send(Item::Step {
                            seq: s.seq,
                            html: step_html(&s),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "replay failed");
                let _ = tx.send(Item::Terminal).await;
                return;
            }
        }

        // If the investigation has already terminated, no future event will arrive on this
        // channel — continuing without the check blocks the live loop forever and leaks
        // the task (Task 3 review Important; see the module documentation). The
        // subscription already precedes this check, so there is no race window.
        if close_if_investigation_is_terminal(&ctx.store, ctx.investigation_id, &tx).await {
            return;
        }

        // Immediately after replay and before the live loop — the hook must run here for
        // the events it creates to necessarily pass through the live channel (see the module documentation).
        if let Some(h) = hook {
            h().await;
        }

        // 3 through 5. Drain, live delivery, and Lagged recovery.
        loop {
            let recv = tokio::select! {
                // Shutdown cancellation — unless this investigation terminates on its own,
                // `sub.recv()` never returns. See this function's opening documentation.
                _ = cancel.cancelled() => {
                    let _ = tx.send(Item::Terminal).await;
                    return;
                }
                r = sub.recv() => r,
            };
            match recv {
                Ok(BusEvent::Step(s)) => {
                    // 3. Discard a seq already emitted.
                    if s.seq <= emitted_seq {
                        continue;
                    }
                    emitted_seq = s.seq;
                    if tx
                        .send(Item::Step {
                            seq: s.seq,
                            html: step_html(&s),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(BusEvent::Delta { kind, text, .. }) => {
                    if tx
                        .send(Item::Delta {
                            html: delta_html(kind, &text),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(BusEvent::Terminal { .. }) => {
                    let _ = tx.send(Item::Terminal).await;
                    return;
                }
                // 5. Lagged — resync from the database. Ignoring it makes steps vanish
                //    silently from the UI (spec Section 6.1, invariant 3).
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "sse subscriber lagged; resyncing from db");
                    match ctx
                        .store
                        .steps_after(ctx.investigation_id, emitted_seq)
                        .await
                    {
                        Ok(steps) => {
                            for s in steps {
                                emitted_seq = emitted_seq.max(s.seq);
                                if tx
                                    .send(Item::Step {
                                        seq: s.seq,
                                        html: step_html(&s),
                                    })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            // `Terminal` is the only durable signal that `Lagged` can lose
                            // (it has no database fallback) — resyncing steps alone cannot
                            // catch that loss. It is checked again here (see the module
                            // documentation).
                            if close_if_investigation_is_terminal(
                                &ctx.store,
                                ctx.investigation_id,
                                &tx,
                            )
                            .await
                            {
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "resync failed");
                            let _ = tx.send(Item::Terminal).await;
                            return;
                        }
                    }
                }
                Err(RecvError::Closed) => {
                    let _ = tx.send(Item::Terminal).await;
                    return;
                }
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// Test-only — collects `want` `seq` values of `Step`s from the stream.
/// It returns early on encountering a `Terminal`.
pub async fn collect_stream_for_test(ctx: StreamCtx, hook: Option<Hook>, want: usize) -> Vec<i64> {
    use tokio_stream::StreamExt;
    let mut out = Vec::new();
    let s = items(ctx, hook);
    tokio::pin!(s);
    while out.len() < want {
        match tokio::time::timeout(Duration::from_secs(20), s.next()).await {
            Ok(Some(Item::Step { seq, .. })) => out.push(seq),
            Ok(Some(Item::Delta { .. })) => continue,
            Ok(Some(Item::Terminal)) | Ok(None) => break,
            Err(_) => panic!(
                "the stream did not produce {want} items within 20 seconds (currently {})",
                out.len()
            ),
        }
    }
    out
}

/// `GET /api/investigations/{id}/stream?after={seq}`
pub async fn investigation_stream(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<AfterParam>,
) -> impl IntoResponse {
    let max = st.store.max_step_seq(id).await.ok().flatten();
    let after = clamp_after(q.after, max);
    let ctx = StreamCtx {
        store: st.store.clone(),
        bus: st.bus.clone(),
        investigation_id: id,
        after,
        jobs: st.jobs.clone(),
    };
    let s = items(ctx, None);
    let events = {
        use tokio_stream::StreamExt;
        s.map(|item| {
            let ev = match item {
                // `id` is attached only to `step` (spec Section 10.2.1).
                Item::Step { seq, html } => Event::default()
                    .event("step")
                    .id(seq.to_string())
                    .data(html),
                Item::Delta { html } => Event::default().event("delta").data(html),
                Item::Terminal => Event::default().event("terminal").data(""),
            };
            Ok::<Event, Infallible>(ev)
        })
    };
    Sse::new(events).keep_alive(KeepAlive::new().interval(HEARTBEAT))
}

//! The owner of investigation tasks.
//!
//! **No detached tasks are created** — graceful shutdown would then have nothing to wait
//! on (spec Section 6.1). Every task is owned by the `JoinSet` and cancellation propagates
//! through a shared `CancellationToken`.
//!
//! **It is generic over `Store`.** Production uses `JobManager<PgStore>`, but the M1 test
//! must inject into the runner a wrapper that fails only `append_step`, and that scenario
//! cannot be built with the field pinned to the concrete `PgStore`.

use crate::bus::StepBus;
use agentops_agent::limits::Limits;
use agentops_agent::mcp::McpToolRegistry;
use agentops_agent::mcp_client::McpConnection;
use agentops_agent::phase::DeltaSink;
use agentops_agent::policy::PolicyGate;
use agentops_agent::runner::{run_investigation, RunnerDeps};
use agentops_core::{LlmProvider, Store, StoreError, TerminalReason};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The grace period before calling `retire()`.
///
/// **It must come after this wait, not immediately after `publish_terminal`** — an SSE
/// subscriber may still be draining `Terminal`. Deleting it at once with no grace leaves
/// subscribers that never received the last event. Extracted as a constant so tests
/// reference it rather than copying the number — if the two diverged, one of them would
/// be lying.
pub const RETIRE_GRACE: Duration = Duration::from_secs(5);

/// The type of the hook a test interposes **between** shutdown stage 1 (closing the gate)
/// and stage 2 (firing cancellation). The same pattern as `stream.rs::Hook` — the
/// production path (`shutdown`) always passes `None`.
///
/// **This is the only way a test can observe that window directly.** `close_gate()` and
/// `cancel.cancel()` are both synchronous calls with no `.await`, so the real gap between
/// the two statements is nanoseconds — trying to distinguish their order from outside
/// with a `sleep` verifies nothing, because in either order both have finished by the
/// time the sleep ends (in the first implementation
/// `shutdown_closes_the_gate_before_cancelling` made exactly that mistake — it did not react to a mutation swapping the order).
pub type ShutdownHook =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("the server is shutting down and is not accepting new investigations")]
    GateClosed,
}

#[derive(Clone)]
pub struct JobDeps {
    pub provider: Arc<dyn LlmProvider>,
    pub connections: Vec<Arc<dyn McpConnection>>,
    pub limits: Limits,
}

pub struct JobManager<S: Store + Send + Sync + 'static> {
    store: Arc<S>,
    bus: StepBus,
    deps: JobDeps,
    tasks: Arc<Mutex<JoinSet<()>>>,
    gate_open: Arc<AtomicBool>,
    cancel: CancellationToken,
}

// Implemented by hand — `#[derive(Clone)]` would require `S: Clone`, but this struct
// holds only an `Arc<S>`, so `S` itself need not be `Clone`.
impl<S: Store + Send + Sync + 'static> Clone for JobManager<S> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            bus: self.bus.clone(),
            deps: self.deps.clone(),
            tasks: self.tasks.clone(),
            gate_open: self.gate_open.clone(),
            cancel: self.cancel.clone(),
        }
    }
}

impl<S: Store + Send + Sync + 'static> JobManager<S> {
    pub fn new(store: Arc<S>, bus: StepBus, deps: JobDeps) -> Self {
        Self {
            store,
            bus,
            deps,
            tasks: Arc::new(Mutex::new(JoinSet::new())),
            gate_open: Arc::new(AtomicBool::new(true)),
            cancel: CancellationToken::new(),
        }
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// The provider accessor for non-investigation tasks such as chat response generation.
    pub fn provider(&self) -> Arc<dyn LlmProvider> {
        self.deps.provider.clone()
    }

    /// Puts a non-investigation task (Task 12's chat response generation) onto the same
    /// `JoinSet`.
    ///
    /// **Why it lives here.** A global convention forbids detached tasks — using
    /// `tokio::spawn` directly leaves graceful shutdown with no way to wait for it.
    /// `JobManager` already holds a `JoinSet` for that purpose (for investigation tasks),
    /// so putting the chat response task in the same place makes `wait_idle` and
    /// `abort_all` handle it automatically — a second `JoinSet` would require wiring the
    /// fact that shutdown must wait for it separately, and forgetting that quietly returns
    /// to detached.
    ///
    /// **It does not check the gate — that is not this function's job.** The first version
    /// deliberately skipped the gate check on the grounds that "chat stores the user
    /// message first, so refusing here would strand it", and the Task 12 review
    /// (Important-1) showed that reasoning was wrong: `main.rs`'s
    /// `axum::serve(...).with_graceful_shutdown(shutdown)` closes the listener only when
    /// the `shutdown` future **completes** — and that future contains all of
    /// `jm2.shutdown(deadline).await` after `ctrl_c().await`, so the HTTP listener keeps
    /// accepting new connections for the seconds or tens of seconds
    /// `JobManager::shutdown` runs. `close_gate()` is stage 1 of `shutdown_with_hook` and
    /// `cancel.cancel()` is stage 2 nanoseconds later, so a task spawned in that window
    /// starts holding an already-cancelled `cancel_token()`, finishes its database queries
    /// (`instructions_for`, `chat_messages`) and opens `provider.stream()`, and then
    /// cancels itself the instant it reaches the `select!` — the outcome (a message with
    /// no response) is identical to honoring the gate, with only the wasted database and
    /// LLM work added. So **the gate check is not this function's job but the caller's
    /// decision, before it causes the side effect of storing** —
    /// `routes::chat::send` checks with [`JobManager::is_accepting`] **before** storing
    /// the message (unlike investigations, chat has no reboot retry path, so refusing
    /// after storing would leave a message that can never receive a response — blocking
    /// before storing is what keeps that state from arising at all).
    pub fn spawn_task<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        // **The task holds one more reference to this `JobManager` itself.** If the `fut`
        // the caller passed does not reference `self` (or its origin, such as `AppState`),
        // Rust 2021's disjoint capture does not capture it — the reference count of
        // `tasks: Arc<Mutex<JoinSet<()>>>` can then hit zero the moment the caller's scope
        // ends, and `JoinSet::drop` immediately aborts everything including the task just
        // spawned. Final review N-1 — leaving each caller to carry its own `keep_alive`
        // meant one of the three sites (the `reply()` spawned by `chat.rs::send`) missed
        // it, and dropping the router really did stop the response being stored. Rather
        // than relying on caller discipline, it is guaranteed once here.
        //
        let keep_alive = self.clone();
        self.tasks
            .lock()
            .expect("joinset mutex poisoned")
            .spawn(async move {
                let _keep_alive = keep_alive;
                fut.await;
            });
    }

    /// Checks whether the gate is open (whether new work is accepted). `spawn` checks the
    /// same condition internally, but `spawn_task` knows nothing of the gate (see its
    /// documentation above), so this is public for a caller
    /// (`routes::chat::send`) that must check before causing a side effect.
    pub fn is_accepting(&self) -> bool {
        self.gate_open.load(Ordering::SeqCst)
    }

    /// Shutdown stage 1 — stop accepting new investigations.
    pub fn close_gate(&self) {
        self.gate_open.store(false, Ordering::SeqCst);
    }

    /// Puts one investigation on an independent task. **It returns immediately** — this
    /// function does not wait for the investigation to finish (INV-1). The HTTP handler
    /// must return a response right after calling it, so an investigation does not stall
    /// there even across a server restart.
    pub fn spawn(&self, id: Uuid) -> Result<(), SpawnError> {
        if !self.gate_open.load(Ordering::SeqCst) {
            return Err(SpawnError::GateClosed);
        }
        let store = self.store.clone();
        let bus = self.bus.clone();
        let deps = self.deps.clone();
        let cancel = self.cancel.clone();

        self.tasks
            .lock()
            .expect("joinset mutex poisoned")
            .spawn(async move {
                // I5 — **a fresh registry per investigation.** Connections are shared while
                // the `known` cache is not. A process-global cache would let investigation
                // B's `list()` overwrite the tool set A saw, and A would receive `NotFound`
                // for an allowed tool.
                let gate = PolicyGate::new(store.clone());
                let tools = McpToolRegistry::new(deps.connections.clone(), gate);

                let sink: &dyn DeltaSink = &bus;
                let rd = RunnerDeps {
                    store: store.as_ref(),
                    provider: deps.provider.as_ref(),
                    tools: &tools,
                    sink,
                    limits: deps.limits,
                };

                // **The panic is caught here.** `JoinSet` does not return task IDs, so
                // detecting a panic from outside (`wait_idle`) gives no way to know which
                // investigation it was — catching it while this task still knows its own
                // `id` is what makes calling `fail_investigation` possible.
                let result = tokio::select! {
                    r = AssertUnwindSafe(run_investigation(&rd, id)).catch_unwind() => r,
                    _ = cancel.cancelled() => {
                        // For cancellation, shutdown stage 6 owns the terminal transition.
                        // Writing here would create two writers.
                        bus.publish_terminal(id);
                        return;
                    }
                };

                // M1 — a runner exiting with a `StoreError` would leave the investigation
                // `running` forever. The conditional terminal transition is applied here.
                // `fail_investigation` writes the status and the terminal step together
                // inside a transaction, so the terminal row has one owner. `Conflict` means
                // another party (shutdown or the watchdog) already terminated it, so we
                // back off — that is normal operation rather than an error and is not
                // logged as an error (review Important 1 — the same judgment as
                // `runner.rs::terminate_failed`: a `Conflict` is merely losing the race).
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        // Review Important 3 — a Postgres failure is not a panic. Lumping
                        // it under `TaskPanicked` makes a code defect and a database failure
                        // indistinguishable in the durable record. It uses
                        // `DependencyUnavailable`, which Task 4 created for exactly this —
                        // "a dependency that is external and recoverable was unavailable".
                        tracing::error!(investigation = %id, error = %e, "runner returned a store error");
                        let reason = TerminalReason::DependencyUnavailable {
                            what: format!("store: {e}"),
                        };
                        if let Err(fail_err) = store.fail_investigation(id, &reason).await {
                            if !matches!(fail_err, StoreError::Conflict) {
                                tracing::error!(investigation = %id, error = %fail_err, "failed to record the terminal transition after a store error");
                            }
                        }
                    }
                    Err(_) => {
                        tracing::error!(investigation = %id, "investigation task panicked");
                        if let Err(fail_err) =
                            store.fail_investigation(id, &TerminalReason::TaskPanicked).await
                        {
                            if !matches!(fail_err, StoreError::Conflict) {
                                tracing::error!(investigation = %id, error = %fail_err, "failed to record the terminal transition after a panic");
                            }
                        }
                    }
                }

                bus.publish_terminal(id);

                // **The channel is reclaimed.** `StepBus`'s map holds one
                // `broadcast::Sender` with a 256-slot buffer per investigation, so without
                // reclamation it grows for the process's lifetime. On a long-running server
                // this is the only point of unbounded growth.
                //
                // **It must be here, not immediately after `publish_terminal`** —
                // subscribers may still be draining. A short grace gives the SSE handler
                // time to receive `Terminal` and close the stream. Deleting it at once with
                // no grace leaves subscribers that never received the last event.
                //
                // Review Minor 2 — **the grace is selected against cancellation.** Shutdown
                // is already closing every stream, so the grace's reason (subscribers may
                // still be draining) no longer holds. Without the select, every finished
                // task lingers 5 seconds in the `JoinSet`, and when `wait_idle`'s deadline
                // is shorter than that grace, shutdown times out on tasks that are sleeping
                // and doing nothing.
                tokio::select! {
                    _ = tokio::time::sleep(RETIRE_GRACE) => {}
                    _ = cancel.cancelled() => {}
                }
                bus.retire(id);
            });
        Ok(())
    }

    /// Performs the shutdown order of spec Section 6.1 exactly:
    ///
    /// ```text
    /// 1. stop accepting new investigations (close the JobManager intake)
    /// 2. fire the CancellationToken
    /// 3. send a terminal event on SSE connections and close them
    /// 4. wait on the JoinSet until the deadline
    /// 5. abort anything past the deadline, then join
    /// 6. for investigations still running, conditionally transition Running to Failed and append a Terminated step
    /// ```
    ///
    /// **1 comes before 2.** In the opposite order, an investigation arriving in between
    /// starts with an already-cancelled token and dies with no record.
    pub async fn shutdown(&self, deadline: Duration) {
        self.shutdown_with_hook(deadline, None).await
    }

    /// A test-only entry point. `hook` runs **between** stages 1 and 2 — `shutdown` always
    /// passes `None`. It exists for the same reason as `stream.rs::items`: the window
    /// between the two stages is nanoseconds, so there is no way to observe it from
    /// outside other than a hook.
    pub async fn shutdown_with_hook(&self, deadline: Duration, hook: Option<ShutdownHook>) {
        // 1.
        self.close_gate();
        if let Some(h) = hook {
            h().await;
        }
        // 2.
        self.cancel.cancel();
        // 3. Each task, on receiving cancellation, fires a terminal for its own
        //    investigation (the select branch in `spawn`). There is no list to fire at in
        //    bulk here — each task knows its own ID.
        // 4.
        self.wait_idle(deadline).await;
        // 5.
        {
            let mut g = self.tasks.lock().expect("joinset mutex poisoned");
            g.abort_all();
        }
        self.wait_idle(Duration::from_secs(5)).await;
        // 6. The conditional transition — an already-terminated investigation backs off
        //    with Conflict. A `Conflict` means another party (the runner or the watchdog)
        //    already terminated it, so it is not logged as an error (the same judgment as `spawn`'s M1 handling).
        match self.store.stale_running_ids(Duration::ZERO).await {
            Ok(ids) => {
                for id in ids {
                    if let Err(e) = self
                        .store
                        .fail_investigation(id, &TerminalReason::ShutdownRequested)
                        .await
                    {
                        if !matches!(e, StoreError::Conflict) {
                            tracing::error!(investigation = %id, error = %e, "shutdown transition failed");
                        }
                    }
                    self.bus.publish_terminal(id);
                }
            }
            Err(e) => tracing::error!(error = %e, "shutdown sweep failed"),
        }
    }

    /// Waits until no in-flight task remains. Used by tests and by shutdown stage 4.
    /// **A panicking task is reaped here too** — the `catch_unwind` inside `spawn` catches
    /// a panic in `run_investigation` itself, but a panic outside it (in registry
    /// construction, say) is reported by the `JoinHandle` as-is. In that case only a log
    /// is left here — which investigation it was is unknown, so no terminal transition can
    /// be applied (INV-4's second layer is owned by the `catch_unwind` inside `spawn`).
    pub async fn wait_idle(&self, deadline: Duration) {
        let start = std::time::Instant::now();
        loop {
            let joined = {
                let mut g = self.tasks.lock().expect("joinset mutex poisoned");
                g.try_join_next()
            };
            match joined {
                Some(Ok(())) => continue,
                Some(Err(e)) if e.is_panic() => {
                    tracing::error!("investigation task panicked outside the catch_unwind guard");
                    continue;
                }
                Some(Err(_)) => continue,
                None => {
                    let empty = self
                        .tasks
                        .lock()
                        .expect("joinset mutex poisoned")
                        .is_empty();
                    if empty || start.elapsed() >= deadline {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }
}

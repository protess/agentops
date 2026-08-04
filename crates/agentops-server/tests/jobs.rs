//! `JobManager` — verifies INV-1 (an investigation is not tied to an HTTP request), M1
//! (store error propagation), panic reaping, shutdown stage 1's intake closure, and channel reclamation.

use agentops_core::{
    BoxStream, ChatMessage, ChatRole, ChatSession, Instruction, Investigation, InvestigationPage,
    InvestigationStatus, ListFilter, LlmError, LlmEvent, LlmProvider, LlmRequest, McpServer,
    NewArtifact, Phase, StepKind, StopReason, Store, StoreError, TerminalReason, ToolPolicy,
    ToolPolicyKind, TriggeredBy, Usage,
};
use agentops_server::bus::{BusEvent, StepBus};
use agentops_server::jobs::{JobDeps, JobManager, RETIRE_GRACE};
use agentops_store::PgStore;
use async_trait::async_trait;
use std::sync::Arc;
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

fn manager_with_bus(
    store: Arc<PgStore>,
    bus: StepBus,
    provider: Arc<dyn LlmProvider>,
) -> JobManager<PgStore> {
    JobManager::new(store, bus, job_deps(provider))
}

fn manager_with_store(
    failing: Arc<FailingStore>,
    _real: Arc<PgStore>,
    provider: Arc<dyn LlmProvider>,
) -> JobManager<FailingStore> {
    JobManager::new(failing, StepBus::new(), job_deps(provider))
}

// --- Provider fixtures -------------------------------------------------

/// Delays briefly on each call, then terminates cleanly with a single text block. For
/// `inv_1_...` to see whether spawn really does not wait, the provider must not finish
/// instantly — without the delay even mutation 1 (synchronous execution) would stay under
/// the 100ms threshold and have no detection power.
struct SlowProvider;

#[async_trait]
impl LlmProvider for SlowProvider {
    fn model_id(&self) -> &str {
        "slow"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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

fn slow_provider() -> Arc<dyn LlmProvider> {
    Arc::new(SlowProvider)
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

/// Panics the instant a stream is requested — reproducing the task panic reaping path
/// deterministically.
struct PanickingProvider;

#[async_trait]
impl LlmProvider for PanickingProvider {
    fn model_id(&self) -> &str {
        "panicking"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        panic!("simulated provider panic");
    }
}

fn panicking_provider() -> Arc<dyn LlmProvider> {
    Arc::new(PanickingProvider)
}

// --- FailingStore -------------------------------------------------------

/// A `Store` wrapper that fails only `append_step`. Everything else delegates to the inner
/// `PgStore`, so `fail_investigation` still leaves a real database row.
pub struct FailingStore {
    inner: Arc<PgStore>,
}

impl FailingStore {
    fn new(inner: Arc<PgStore>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Store for FailingStore {
    async fn create_investigation(&self, inv: &Investigation) -> Result<(), StoreError> {
        self.inner.create_investigation(inv).await
    }
    async fn get_investigation(&self, id: Uuid) -> Result<Investigation, StoreError> {
        self.inner.get_investigation(id).await
    }
    async fn list_investigations(
        &self,
        filter: &ListFilter,
    ) -> Result<InvestigationPage, StoreError> {
        self.inner.list_investigations(filter).await
    }
    async fn mark_running(&self, id: Uuid) -> Result<(), StoreError> {
        self.inner.mark_running(id).await
    }
    async fn fail_orphaned_running(&self, reason: &TerminalReason) -> Result<u64, StoreError> {
        self.inner.fail_orphaned_running(reason).await
    }
    async fn stale_running_ids(
        &self,
        idle_for: std::time::Duration,
    ) -> Result<Vec<Uuid>, StoreError> {
        self.inner.stale_running_ids(idle_for).await
    }
    async fn queued_ids(&self) -> Result<Vec<Uuid>, StoreError> {
        self.inner.queued_ids().await
    }
    async fn append_step(
        &self,
        _investigation_id: Uuid,
        _phase: Phase,
        _kind: &StepKind,
    ) -> Result<i64, StoreError> {
        Err(StoreError::Backend("simulated append_step failure".into()))
    }
    async fn steps_after(
        &self,
        id: Uuid,
        after_seq: i64,
    ) -> Result<Vec<agentops_core::AgentStep>, StoreError> {
        self.inner.steps_after(id, after_seq).await
    }
    async fn max_step_seq(&self, id: Uuid) -> Result<Option<i64>, StoreError> {
        self.inner.max_step_seq(id).await
    }
    async fn instructions_for(&self, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError> {
        self.inner.instructions_for(phases).await
    }
    async fn upsert_instruction(&self, ins: &Instruction) -> Result<(), StoreError> {
        self.inner.upsert_instruction(ins).await
    }
    async fn update_instruction(&self, ins: &Instruction) -> Result<(), StoreError> {
        self.inner.update_instruction(ins).await
    }
    async fn delete_instruction(&self, id: Uuid) -> Result<(), StoreError> {
        self.inner.delete_instruction(id).await
    }
    async fn get_artifact(&self, id: Uuid) -> Result<agentops_core::Artifact, StoreError> {
        self.inner.get_artifact(id).await
    }
    async fn list_artifacts(&self, limit: i64) -> Result<Vec<agentops_core::Artifact>, StoreError> {
        self.inner.list_artifacts(limit).await
    }
    async fn complete_investigation(
        &self,
        id: Uuid,
        artifact: &NewArtifact,
    ) -> Result<Uuid, StoreError> {
        self.inner.complete_investigation(id, artifact).await
    }
    async fn fail_investigation(
        &self,
        id: Uuid,
        reason: &TerminalReason,
    ) -> Result<(), StoreError> {
        self.inner.fail_investigation(id, reason).await
    }
    async fn create_chat_session(&self, s: &ChatSession) -> Result<(), StoreError> {
        self.inner.create_chat_session(s).await
    }
    async fn list_chat_sessions(&self, limit: i64) -> Result<Vec<ChatSession>, StoreError> {
        self.inner.list_chat_sessions(limit).await
    }
    async fn chat_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError> {
        self.inner.chat_messages(session_id).await
    }
    async fn append_chat_message(
        &self,
        session_id: Uuid,
        role: ChatRole,
        content: &serde_json::Value,
    ) -> Result<i64, StoreError> {
        self.inner
            .append_chat_message(session_id, role, content)
            .await
    }
    async fn enabled_mcp_servers(&self) -> Result<Vec<McpServer>, StoreError> {
        self.inner.enabled_mcp_servers().await
    }
    async fn tool_policies_for(&self, server_name: &str) -> Result<Vec<ToolPolicy>, StoreError> {
        self.inner.tool_policies_for(server_name).await
    }
    async fn tool_policy(
        &self,
        server_name: &str,
        tool_name: &str,
    ) -> Result<ToolPolicyKind, StoreError> {
        self.inner.tool_policy(server_name, tool_name).await
    }
    async fn upsert_tool_policy(&self, policy: &ToolPolicy) -> Result<(), StoreError> {
        self.inner.upsert_tool_policy(policy).await
    }
}

// --- Tests ---------------------------------------------------------------

/// INV-1 — an investigation is not tied to an HTTP request.
/// The investigation keeps going after `spawn` returns and reaches `completed`.
/// If spawn waited for the investigation, this test would time out.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_1_spawn_returns_before_the_investigation_finishes(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    let jm = manager(store.clone(), slow_provider());

    let t0 = std::time::Instant::now();
    jm.spawn(id).unwrap();
    let spawn_took = t0.elapsed();

    assert!(
        spawn_took < std::time::Duration::from_millis(100),
        "spawn waited for the investigation ({spawn_took:?}) — the handler blocks"
    );

    // The investigation keeps going and finishes.
    jm.wait_idle(std::time::Duration::from_secs(20)).await;
    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Completed);
}

/// M1 — an investigation does not stay `running` when the runner exits with a `StoreError`.
/// Minor 1 of plan 2's final review: a store error propagated by `?` had no terminal
/// transition. Without this, only the watchdog reclaims that investigation — 15 minutes later.
#[sqlx::test(migrations = "../../migrations")]
async fn m1_a_store_error_escaping_the_runner_still_fails_the_investigation(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    // A store wrapper that errors on the first append_step.
    let failing = Arc::new(FailingStore::new(store.clone()));
    let jm = manager_with_store(failing, store.clone(), ok_provider());

    jm.spawn(id).unwrap();
    jm.wait_idle(std::time::Duration::from_secs(20)).await;

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(
        inv.status,
        InvestigationStatus::Failed,
        "the runner exited on a store error but the investigation stayed running"
    );

    // Review Important 3 — a store error must be recorded as `DependencyUnavailable`,
    // not `TaskPanicked`: a database failure is not a panic, and lumping them together
    // leaves an operator unable to tell a code defect from an infrastructure failure.
    // The exact reason is asserted with `matches!` — looking only at the `Failed` status
    // could not tell whether this defect was fixed or reverted.
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Terminated { .. }))
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one terminal row: {terminated:#?}"
    );
    assert!(
        matches!(
            &terminated[0].kind,
            StepKind::Terminated {
                reason: TerminalReason::DependencyUnavailable { .. },
                ..
            }
        ),
        "a store error was lumped under TaskPanicked: {:#?}",
        terminated[0].kind
    );
}

/// A panicking task still marks the investigation `failed` (spec Section 6.1, INV-4's second layer).
#[sqlx::test(migrations = "../../migrations")]
async fn a_panicking_task_marks_the_investigation_failed(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    let jm = manager(store.clone(), panicking_provider());

    jm.spawn(id).unwrap();
    jm.wait_idle(std::time::Duration::from_secs(20)).await;

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Failed);
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Terminated { .. }))
        .count();
    assert_eq!(
        terminated, 1,
        "a panicked investigation must have exactly one terminal step"
    );
}

/// After the intake closes, no new investigation is accepted (shutdown stage 1).
#[sqlx::test(migrations = "../../migrations")]
async fn spawn_is_refused_after_the_gate_closes(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    let jm = manager(store.clone(), ok_provider());
    jm.close_gate();
    assert!(
        jm.spawn(id).is_err(),
        "an investigation was accepted after the intake closed"
    );
}

/// A finished investigation returns its broadcast channel. Without that, `StepBus`'s map
/// grows for the process's lifetime — one 256-slot buffer per investigation, forever.
///
/// The `retire` grace is `RETIRE_GRACE`, so `wait_idle`'s deadline is set comfortably
/// above it — too short and the test passes without waiting out the grace and always fails.
#[sqlx::test(migrations = "../../migrations")]
async fn finished_investigations_release_their_channel(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    let bus = StepBus::new();
    let jm = manager_with_bus(store.clone(), bus.clone(), ok_provider());

    jm.spawn(id).unwrap();
    jm.wait_idle(RETIRE_GRACE + std::time::Duration::from_secs(25))
        .await;

    assert_eq!(
        bus.live_channels(),
        0,
        "a finished investigation's channel was not returned — the map grows without bound"
    );
}

/// Review Minor 2 — the `retire` grace after an investigation ends must be selected
/// against cancellation. Shutdown is already closing every stream, so the grace's reason
/// (subscribers may still be draining) no longer holds. Without the select, every finished
/// task lingers `RETIRE_GRACE` in the `JoinSet`, and when shutdown's `wait_idle` deadline
/// is shorter, it times out on tasks that are sleeping and doing nothing.
///
///
/// It must cancel only after the `Terminal` event appears (that is, after the
/// investigation ends and enters the grace wait) — cancelling earlier takes the other
/// branch that cancels `run_investigation` itself and never verifies this grace at all.
#[sqlx::test(migrations = "../../migrations")]
async fn cancellation_short_circuits_the_retire_grace(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    let bus = StepBus::new();
    // INV-2 — subscription comes before spawn.
    let mut sub = bus.subscribe(id);
    let jm = manager_with_bus(store.clone(), bus.clone(), ok_provider());

    jm.spawn(id).unwrap();

    // Wait for the signal that the investigation ended and entered the grace wait.
    loop {
        match sub.recv().await {
            Ok(BusEvent::Terminal { .. }) => break,
            Ok(_) => continue,
            Err(_) => panic!("the Terminal event never arrived"),
        }
    }

    jm.cancel_token().cancel();

    let t0 = std::time::Instant::now();
    jm.wait_idle(RETIRE_GRACE + std::time::Duration::from_secs(10))
        .await;
    let waited = t0.elapsed();

    assert!(
        waited < RETIRE_GRACE,
        "it waited out the whole grace ({RETIRE_GRACE:?}) despite cancellation: {waited:?}"
    );
    assert_eq!(
        bus.live_channels(),
        0,
        "the channel was not returned even after cancellation"
    );
}

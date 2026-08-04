use agentops_agent::limits::Limits;
use agentops_agent::phase::NoopSink;
use agentops_agent::runner::{render_artifact, run_investigation, RunnerDeps};
use agentops_core::{
    BoxStream, Instruction, Investigation, InvestigationStatus, LlmError, LlmEvent, LlmProvider,
    LlmRequest, Phase, StepKind, StopReason, Store, TerminalReason, ToolDef, ToolError, ToolOutput,
    ToolRegistry, TriggeredBy, Usage,
};
use agentops_store::PgStore;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use uuid::Uuid;

/// A provider ending each turn with short text. Records how often it was called and each request's system.
struct Chatty {
    calls: Arc<AtomicUsize>,
    systems: Arc<Mutex<Vec<String>>>,
    refuse_on_call: Option<usize>,
    /// Ends on `end_turn` alone with no content. Reproduces a turn that thought but never
    /// spoke — `ThinkingBlock` does not contribute to `summary`.
    silent: bool,
}

impl Chatty {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            systems: Arc::new(Mutex::new(Vec::new())),
            refuse_on_call: None,
            silent: false,
        }
    }
}

#[async_trait]
impl LlmProvider for Chatty {
    fn model_id(&self) -> &str {
        "fake"
    }
    async fn stream(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.systems.lock().unwrap().push(req.system.clone());

        let events: Vec<Result<LlmEvent, LlmError>> = if self.silent {
            vec![Ok(LlmEvent::Stopped {
                reason: StopReason::EndTurn,
                usage: Usage::default(),
                refusal_category: None,
            })]
        } else if self.refuse_on_call == Some(n) {
            vec![Ok(LlmEvent::Stopped {
                reason: StopReason::Refusal,
                usage: Usage::default(),
                refusal_category: None,
            })]
        } else {
            vec![
                Ok(LlmEvent::TextBlock {
                    text: format!("finding-{n}"),
                }),
                Ok(LlmEvent::Stopped {
                    reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    refusal_category: None,
                }),
            ]
        };
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

/// Emits a first delta then stalls forever — reproducing a stalled connection. Makes the
/// idle timeout path deterministic without a real socket.
struct StallingProvider;

#[async_trait]
impl LlmProvider for StallingProvider {
    fn model_id(&self) -> &str {
        "stalling"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Ok(Box::pin(futures_util::stream::unfold(
            0u8,
            |i| async move {
                if i == 0 {
                    Some((
                        Ok(LlmEvent::TextDelta {
                            text: "starting".into(),
                        }),
                        1u8,
                    ))
                } else {
                    std::future::pending::<()>().await;
                    None
                }
            },
        )))
    }
}

struct NoTools;

#[async_trait]
impl ToolRegistry for NoTools {
    async fn list(&self) -> Result<Vec<ToolDef>, ToolError> {
        Ok(vec![])
    }
    async fn call(&self, name: &str, _i: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Err(ToolError::NotFound(name.into()))
    }
}

/// `tools/list` fails — reproducing a policy store failure. `McpToolRegistry` swallows
/// per-server failures individually, so the only thing that actually becomes this `Err`
/// is a policy store error.
struct FailingTools;

#[async_trait]
impl ToolRegistry for FailingTools {
    async fn list(&self) -> Result<Vec<ToolDef>, ToolError> {
        Err(ToolError::Transport("policy store unavailable".into()))
    }
    async fn call(&self, name: &str, _i: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Err(ToolError::NotFound(name.into()))
    }
}

/// Collects terminal rows as (reason, phase).
fn terminal_rows(steps: &[agentops_core::AgentStep]) -> Vec<(TerminalReason, Phase)> {
    steps
        .iter()
        .filter_map(|s| match &s.kind {
            StepKind::Terminated { reason, .. } => Some((reason.clone(), s.phase)),
            _ => None,
        })
        .collect()
}

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

/// Follows the fixture shape in `crates/agentops-store/tests/instructions.rs`.
/// `title` is unused, so `body` goes in directly and the marker is found either way.
fn instruction(phase: Phase, body: &str) -> Instruction {
    Instruction {
        id: Uuid::new_v4(),
        phase,
        position: 0,
        title: body.to_string(),
        body: body.to_string(),
        enabled: true,
        updated_at: OffsetDateTime::now_utc(),
    }
}

/// All three phases always run and the investigation ends completed (spec Section 5.3).
#[sqlx::test(migrations = "../../migrations")]
async fn runs_all_three_phases_and_completes(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let provider = Chatty::new();
    let systems = Arc::clone(&provider.systems);
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits::default(),
    };

    run_investigation(&deps, id).await.unwrap();

    // It made three round trips and each phase's system prompt must differ.
    // Holding a MutexGuard across the `.await` below is clippy::await_holding_lock.
    // Take the value out and drop the guard immediately.
    let sys: Vec<String> = systems.lock().unwrap().clone();
    assert_eq!(sys.len(), 3, "it must run three phases");
    assert!(sys[0].contains("triage"), "{}", sys[0]);
    assert!(sys[1].contains("rca"), "{}", sys[1]);
    assert!(sys[2].contains("mitigation"), "{}", sys[2]);

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Completed);
    assert!(inv.finished_at.is_some());
    assert!(
        inv.started_at.is_some(),
        "mark_running must have been called"
    );
}

/// TEST-10 — the three phases run in order and each receives **only its own instructions**.
/// Mixed instructions put Mitigation directives into the Triage prompt, and the prompt
/// cache misses on every phase too (spec Sections 5.3 and 9).
#[sqlx::test(migrations = "../../migrations")]
async fn test_10_each_phase_gets_only_its_own_instructions(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    for (phase, body) in [
        (Phase::All, "ALL-MARK"),
        (Phase::Triage, "TRIAGE-MARK"),
        (Phase::Rca, "RCA-MARK"),
        (Phase::Mitigation, "MITIGATION-MARK"),
    ] {
        store
            .upsert_instruction(&instruction(phase, body))
            .await
            .unwrap();
    }

    let provider = Chatty::new();
    let systems = Arc::clone(&provider.systems);
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits::default(),
    };
    run_investigation(&deps, id).await.unwrap();

    let sys: Vec<String> = systems.lock().unwrap().clone();
    assert_eq!(
        sys.len(),
        3,
        "each of the three phases must issue exactly one request"
    );

    assert!(sys[0].contains("TRIAGE-MARK") && sys[0].contains("ALL-MARK"));
    assert!(
        !sys[0].contains("RCA-MARK"),
        "Triage received RCA instructions"
    );
    assert!(
        !sys[0].contains("MITIGATION-MARK"),
        "Triage received Mitigation instructions"
    );

    assert!(sys[1].contains("RCA-MARK"));
    assert!(
        !sys[1].contains("TRIAGE-MARK"),
        "RCA received Triage instructions"
    );

    assert!(sys[2].contains("MITIGATION-MARK"));
    assert!(
        !sys[2].contains("RCA-MARK"),
        "Mitigation received RCA instructions"
    );
}

/// The artifact is stored and an ArtifactWritten step is left behind.
#[sqlx::test(migrations = "../../migrations")]
async fn completion_writes_an_artifact_and_a_step(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let provider = Chatty::new();
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits::default(),
    };
    run_investigation(&deps, id).await.unwrap();

    // **Count, do not probe for presence.** `find_map` looks only at the first item and
    // would pass a regression where the terminal transaction runs twice — a shape repeated in this repository.
    let steps = store.steps_after(id, -1).await.unwrap();
    let written: Vec<Uuid> = steps
        .iter()
        .filter_map(|s| match &s.kind {
            StepKind::ArtifactWritten { artifact_id } => Some(*artifact_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        written.len(),
        1,
        "there must be exactly one ArtifactWritten step: {written:?}"
    );
    let artifact_id = written[0];

    let art = store.get_artifact(artifact_id).await.unwrap();
    // All three phases' results must be in the artifact — one missing means that phase's
    // work was thrown away.
    for n in 0..3 {
        assert!(
            art.body.contains(&format!("finding-{n}")),
            "phase {n}'s result is missing from the artifact:\n{}",
            art.body
        );
    }
}

/// A refusal fails the investigation and the remaining phases do not run (spec Section 8.4).
#[sqlx::test(migrations = "../../migrations")]
async fn refusal_in_the_first_phase_fails_the_investigation(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let mut provider = Chatty::new();
    provider.refuse_on_call = Some(0);
    let calls = Arc::clone(&provider.calls);
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits::default(),
    };
    run_investigation(&deps, id).await.unwrap();

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Failed);
    assert!(inv.finished_at.is_some());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "it ran the next phase after a refusal"
    );

    // **There must be exactly one terminal row, and the terminal transaction must have
    // written it.** A `Terminated` row is owned by the writer that flips the status with
    // it — when a writer that does not flip the status leaves one outside the transaction,
    // shutdown winning the race leaves that row alone, contradicting the real termination
    // reason. Without counting, that duplication has no detection — the same gap occurred on the wall-clock path.
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::Refusal { .. },
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one Refusal terminal row: {terminated:#?}"
    );
    assert_eq!(
        terminated[0].phase,
        Phase::All,
        "the terminal transaction did not write the terminal row — the phase tag is the evidence"
    );
}

/// The runner does not overwrite an already-terminated investigation (spec Section 6.1 — conditional transitions).
/// If shutdown marked it failed first, the runner receives Conflict and backs off.
#[sqlx::test(migrations = "../../migrations")]
async fn runner_does_not_overwrite_a_terminal_status(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;
    store.mark_running(id).await.unwrap();

    // Shutdown terminated it first.
    store
        .fail_investigation(id, &TerminalReason::ShutdownRequested)
        .await
        .unwrap();

    let provider = Chatty::new();
    let calls = Arc::clone(&provider.calls);
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits::default(),
    };
    // The runner must back off without panicking or raising an Err.
    run_investigation(&deps, id).await.unwrap();

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(
        inv.status,
        InvestigationStatus::Failed,
        "the runner overwrote the terminal status"
    );
    // Status alone cannot distinguish "it backed off" from "it ran and then failed to
    // overwrite". Never calling the LLM is the observable evidence of backing off.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "it ran phases on an already-terminated investigation"
    );
}

/// Only exceeding the wall clock fails an investigation (spec Section 5.4).
#[sqlx::test(migrations = "../../migrations")]
async fn wall_clock_exhaustion_fails_the_investigation(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let provider = Chatty::new();
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits {
            wall_clock: std::time::Duration::from_nanos(1),
            ..Default::default()
        },
    };
    run_investigation(&deps, id).await.unwrap();

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Failed);

    // **Count.** `any()` cannot tell one row from two. The terminal row is owned by
    // `fail_investigation` inside the terminal transaction — the runner writing one more
    // before that would produce two identical rows on the normal path, and because that
    // write is outside the transaction, an intervening shutdown leaves a contradictory record even after backing off.
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::WallClockExceeded,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one WallClockExceeded terminal row: {terminated:#?}"
    );
    // That one was written by the terminal transaction, so it is `Phase::All`.
    assert_eq!(terminated[0].phase, Phase::All);
}

/// The turn counter resets at a phase boundary (spec Section 5.4).
///
/// **The observable is the provider call count.** Status cannot catch this — exceeding
/// the turn limit does not fail an investigation (Section 5.4), so deleting
/// `reset_phase()` still ends it `completed`. The termination reason is no use either:
/// several limits return the same `TurnLimitExceeded`, so which one fired cannot be
/// distinguished. Without the reset the first phase spends the budget and the other two
/// end without calling the LLM even once — a call count of 1 versus 3 is that difference.
#[sqlx::test(migrations = "../../migrations")]
async fn the_turn_budget_resets_at_each_phase_boundary(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let provider = Chatty::new();
    let calls = Arc::clone(&provider.calls);
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        // One turn per phase. Without the reset the whole investigation gets one turn.
        limits: Limits {
            max_turns_per_phase: 1,
            ..Default::default()
        },
    };
    run_investigation(&deps, id).await.unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "the turn budget did not reset per phase"
    );
    // Exceeding the turn limit does not fail an investigation (Section 5.4) — only the wall clock does.
    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Completed);
}

/// Even when a stalled stream pushes past the investigation wall clock, there must be one terminal row.
///
/// **`WallClockExceeded` is the only reason with two writers.** Other reasons reach the
/// runner only through `out.failed` (FailInvestigation) or not at all, whereas the wall
/// clock is **independently re-derived** by the runner through `budget.check_wall_clock()`.
/// The moment a late-firing idle timeout accumulates past the investigation limit, both
/// writers hold at once — the fact that `out.terminated` is not read does not by itself reveal this collision.
#[sqlx::test(migrations = "../../migrations")]
async fn a_stalled_stream_records_exactly_one_wall_clock_terminal_row(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let provider = StallingProvider;
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        // The idle limit is **longer** than the wall clock — by the time the idle limit
        // fires on a stalled stream, the accumulated time has passed the investigation limit and the runner's re-derivation fires.
        limits: Limits {
            stream_idle_timeout: std::time::Duration::from_millis(150),
            wall_clock: std::time::Duration::from_millis(100),
            ..Default::default()
        },
    };
    run_investigation(&deps, id).await.unwrap();

    let steps = store.steps_after(id, -1).await.unwrap();

    // **First confirm the idle path was actually taken.** If setup is slow enough that the
    // wall-clock check at the top of the loop fires first, the stream is never reached, and
    // that path leaves only one row anyway, so the assertions below would pass verifying nothing.
    assert!(
        steps.iter().any(|s| matches!(
            &s.kind,
            StepKind::Error { message } if message.contains("idle")
        )),
        "the idle timeout path was not taken — this test verifies nothing: {steps:?}"
    );

    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::WallClockExceeded,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one WallClockExceeded terminal row: {terminated:#?}"
    );
    assert_eq!(
        terminated[0].phase,
        Phase::All,
        "the terminal transaction did not write the terminal row"
    );
}

/// An investigation whose three phases all hung on a stalled stream does not end `completed`.
///
/// **This is the early-firing idle timeout** — the investigation wall clock (600s) is
/// nowhere near, so the runner's re-derivation does not fire. If nobody writes a terminal
/// row then, all three phases did nothing while the investigation becomes `completed` with
/// an artifact of three empty sections. An operator receiving that at 03:00 cannot tell it from a real investigation.
#[sqlx::test(migrations = "../../migrations")]
async fn an_investigation_where_every_phase_stalled_is_not_completed(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let provider = StallingProvider;
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        // The idle limit is **far shorter** than the investigation limit — every phase is
        // cut by idleness while the investigation wall clock survives, so the runner's re-derivation never fires.
        limits: Limits {
            stream_idle_timeout: std::time::Duration::from_millis(40),
            wall_clock: std::time::Duration::from_secs(600),
            ..Default::default()
        },
    };
    run_investigation(&deps, id).await.unwrap();

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(
        inv.status,
        InvestigationStatus::Failed,
        "an investigation where no phase did any work ended completed"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let rows = terminal_rows(&steps);

    // One terminal row per phase, with the real phase tag. Spec Section 5.4 says an
    // exceeded limit ends that phase with `Terminated`.
    for phase in Phase::INVESTIGATION_ORDER {
        assert_eq!(
            rows.iter()
                .filter(|(r, p)| *r == TerminalReason::WallClockExceeded && *p == phase)
                .count(),
            1,
            "phase {} does not have exactly one terminal row: {rows:#?}",
            phase.as_str()
        );
    }
    // The investigation's own terminal row is the single one the terminal transaction left as `Phase::All`.
    assert_eq!(
        rows.iter().filter(|(_, p)| *p == Phase::All).count(),
        1,
        "the investigation does not have exactly one terminal row: {rows:#?}"
    );
}

/// When a policy store failure prevents obtaining tools, the phase ends and a durable record is left.
///
/// Running three phases without tools makes the model write a plausible report from prior
/// knowledge alone, and in the database that row is indistinguishable from a real
/// investigation — the worst output an incident-response tool can produce.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tools_list_failure_is_recorded_and_does_not_yield_a_completed_report(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let provider = Chatty::new();
    let calls = Arc::clone(&provider.calls);
    let tools = FailingTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits::default(),
    };
    run_investigation(&deps, id).await.unwrap();

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(
        inv.status,
        InvestigationStatus::Failed,
        "an investigation that ran without tools ended completed"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the LLM was called without obtaining the tool list — it writes a report from prior knowledge alone"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    // The cause must survive in the durable record. `tracing::warn!` stays in the process
    // log and is invisible to the UI reading `agent_steps`.
    let errors = steps
        .iter()
        .filter(
            |s| matches!(&s.kind, StepKind::Error { message } if message.contains("tools/list")),
        )
        .count();
    assert_eq!(
        errors, 3,
        "the cause was not recorded per phase: {steps:#?}"
    );
}

/// Artifact rendering is a pure function, so it is verified on its own.
#[test]
fn artifact_includes_every_phase_in_order() {
    let art = render_artifact(&[
        (Phase::Triage, "T".into()),
        (Phase::Rca, "R".into()),
        (Phase::Mitigation, "M".into()),
    ]);
    let t = art.body.find('T').unwrap();
    let r = art.body.find('R').unwrap();
    let m = art.body.find('M').unwrap();
    assert!(
        t < r && r < m,
        "the phase order was reversed:\n{}",
        art.body
    );
    assert!(!art.title.trim().is_empty(), "the title is empty");
}

/// Three phases that terminated cleanly with no content still do not become `completed`.
///
/// **A clean termination is not an achievement.** `end_turn` leaves `terminated` as
/// `None`, so it counts as "this phase finished well", but with neither a text block nor a
/// tool call there is nothing to carry forward. Because `ThinkingBlock` does not
/// contribute to `summary`, a turn that only thinks produces exactly this state — no machine failure needed.
#[sqlx::test(migrations = "../../migrations")]
async fn three_content_free_phases_do_not_produce_a_completed_report(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = queued(&store).await;

    let mut provider = Chatty::new();
    provider.silent = true;
    let calls = Arc::clone(&provider.calls);
    let tools = NoTools;
    let sink = NoopSink;
    let deps = RunnerDeps {
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
        limits: Limits::default(),
    };
    run_investigation(&deps, id).await.unwrap();

    // The point of this fixture is that it ran three phases **normally** — no limit fired
    // and no stream broke.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "it did not run three phases"
    );

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(
        inv.status,
        InvestigationStatus::Failed,
        "an investigation with no content at all ended completed"
    );

    // No empty artifact may remain — `complete_investigation` itself is never called, so
    // there is no `ArtifactWritten` either.
    let steps = store.steps_after(id, -1).await.unwrap();
    assert!(
        !steps
            .iter()
            .any(|s| matches!(&s.kind, StepKind::ArtifactWritten { .. })),
        "an empty artifact was stored: {steps:#?}"
    );
}

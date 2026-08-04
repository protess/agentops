use agentops_agent::limits::{Budget, Limits};
use agentops_agent::phase::{run_phase_loop, DeltaSink, NoopSink, PhaseCtx, Termination};
use agentops_core::{
    BoxStream, Investigation, InvestigationStatus, LlmContent, LlmError, LlmEvent, LlmProvider,
    LlmRequest, Phase, StepKind, StopReason, Store, TerminalReason, ToolDef, ToolError, ToolOutput,
    ToolRegistry, TriggeredBy, Usage,
};
use agentops_store::PgStore;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use time::OffsetDateTime;
use uuid::Uuid;

// ---------- Fake provider ----------

/// Returns predetermined turns in order. Once the turns run out it repeats the last one —
/// building a provider that never terminates so limits can be observed firing (TEST-15).
struct FakeProvider {
    turns: Vec<Vec<LlmEvent>>,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

impl FakeProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns,
            calls: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// How many times `stream()` was actually called — that is, how many turns the
    /// provider was asked to run.
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// A snapshot of every request sent so far.
    fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for FakeProvider {
    fn model_id(&self) -> &str {
        "fake"
    }
    async fn stream(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(req);
        let idx = n.min(self.turns.len().saturating_sub(1));
        let events: Vec<Result<LlmEvent, LlmError>> =
            self.turns[idx].iter().cloned().map(Ok).collect();
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

/// Emits a first event then stalls forever — reproducing a stalled connection. Makes the
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

fn stopped(r: StopReason) -> LlmEvent {
    LlmEvent::Stopped {
        reason: r,
        usage: Usage::default(),
        refusal_category: None,
    }
}

/// The `stream()` call itself fails — reproducing an initial connection failure.
/// `FakeProvider` cannot create this path (`stream()` always returns `Ok` and wraps only
/// the events in `Result`), so this is a separate small provider.
struct FailingConnectProvider;

#[async_trait]
impl LlmProvider for FailingConnectProvider {
    fn model_id(&self) -> &str {
        "failing-connect"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Err(LlmError::Transport("connection refused".into()))
    }
}

/// Emits an `Err` item mid-stream — reproducing a transport error. `FakeProvider` holds
/// only `LlmEvent`s and wraps them all in `Ok`, so it cannot create this path either.
struct MidStreamTransportErrorProvider;

#[async_trait]
impl LlmProvider for MidStreamTransportErrorProvider {
    fn model_id(&self) -> &str {
        "mid-stream-transport-error"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let events: Vec<Result<LlmEvent, LlmError>> = vec![
            Ok(LlmEvent::TextDelta {
                text: "partial".into(),
            }),
            Err(LlmError::Transport("connection reset".into())),
        ];
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

// ---------- Fake tools ----------

struct FakeTools {
    defs: Vec<ToolDef>,
    fail: bool,
    /// Returns `ToolError::Timeout` — the individual tool call timeout of spec Section 5.4.
    time_out: bool,
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeTools {
    fn new(names: &[&str]) -> Self {
        Self {
            defs: names
                .iter()
                .map(|n| ToolDef {
                    name: (*n).into(),
                    description: "d".into(),
                    input_schema: serde_json::json!({"type":"object"}),
                })
                .collect(),
            fail: false,
            time_out: false,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ToolRegistry for FakeTools {
    async fn list(&self) -> Result<Vec<ToolDef>, ToolError> {
        Ok(self.defs.clone())
    }
    async fn call(&self, name: &str, _input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        self.calls.lock().unwrap().push(name.to_string());
        if self.time_out {
            return Err(ToolError::Timeout {
                tool: name.to_string(),
                seconds: 60,
            });
        }
        if self.fail {
            return Err(ToolError::Transport("boom".into()));
        }
        Ok(ToolOutput {
            text: format!("{name} result"),
            is_error: false,
            truncated: false,
        })
    }
}

// ---------- A sink that counts deltas ----------

#[derive(Default)]
struct CountingSink {
    text: AtomicUsize,
    thinking: AtomicUsize,
}

impl DeltaSink for CountingSink {
    fn text(&self, _id: Uuid, _p: Phase, _d: &str) {
        self.text.fetch_add(1, Ordering::SeqCst);
    }
    fn thinking(&self, _id: Uuid, _p: Phase, _d: &str) {
        self.thinking.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------- Shared setup ----------

async fn running_investigation(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: "t".into(),
        prompt: "why is latency high".into(),
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

/// Extracts only the `reason` from a `Termination`. **It does not weaken the
/// assertions** — the existing tests checked the exact reason with
/// `out.terminated == Some(TerminalReason::X)`, and that detection power is preserved now
/// that `Termination` also carries `detail`. Lowering it to `is_some()` would stop
/// distinguishing which reason ended it.
fn reason(t: &Option<Termination>) -> Option<TerminalReason> {
    t.as_ref().map(|t| t.reason.clone())
}

// ---------- Tests ----------

/// TEST-16 — pushing 1000 deltas grows `agent_steps` only by the number of semantic units.
/// Calling append_step per delta would make one investigation tens of thousands of rows (spec Section 6.1.3).
#[sqlx::test(migrations = "../../migrations")]
async fn test_16_a_thousand_deltas_produce_only_meaningful_steps(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let mut turn: Vec<LlmEvent> = (0..1000)
        .map(|i| LlmEvent::TextDelta {
            text: format!("{i}"),
        })
        .collect();
    turn.push(LlmEvent::TextBlock {
        text: "final".into(),
    });
    turn.push(stopped(StopReason::EndTurn));

    let provider = FakeProvider::new(vec![turn]);
    let tools = FakeTools::new(&[]);
    let sink = CountingSink::default();
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert!(out.terminated.is_none());
    assert!(out.failed.is_none());

    // The deltas went to the sink 1000 times.
    assert_eq!(sink.text.load(Ordering::SeqCst), 1000);

    // Only one completed block remains in the database.
    let steps = store.steps_after(id, -1).await.unwrap();
    let text_steps = steps
        .iter()
        .filter(|s| matches!(s.kind, StepKind::Text { .. }))
        .count();
    assert_eq!(
        text_steps,
        1,
        "deltas leaked into the database: {} steps in total",
        steps.len()
    );
}

/// TEST-11 — when three tool calls arrive in one turn, three tool_results go into **one**
/// user message with exactly matching IDs. Omitting one makes the API reject the request.
#[sqlx::test(migrations = "../../migrations")]
async fn test_11_parallel_tool_results_land_in_one_user_message_with_matching_ids(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let turn1 = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__a".into(),
            input: serde_json::json!({}),
        },
        LlmEvent::ToolCall {
            tool_use_id: "tu_2".into(),
            tool: "s__b".into(),
            input: serde_json::json!({}),
        },
        LlmEvent::ToolCall {
            tool_use_id: "tu_3".into(),
            tool: "s__c".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::ToolUse),
    ];
    let turn2 = vec![
        LlmEvent::TextBlock {
            text: "done".into(),
        },
        stopped(StopReason::EndTurn),
    ];

    let provider = FakeProvider::new(vec![turn1, turn2]);
    let requests = Arc::clone(&provider.requests);
    let tools = FakeTools::new(&["s__a", "s__b", "s__c"]);
    let mut budget = Budget::new(Limits::default(), Instant::now());
    let sink = NoopSink;

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    // The last message of the second request must be a user message holding three tool_results.
    let reqs = requests.lock().unwrap();
    assert_eq!(reqs.len(), 2, "it must make two round trips");
    let last = reqs[1].messages.last().expect("there are no messages");
    assert_eq!(last.role, agentops_core::LlmRole::User);

    let ids: Vec<&str> = last
        .content
        .iter()
        .filter_map(|c| match c {
            agentops_core::LlmContent::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec!["tu_1", "tu_2", "tu_3"],
        "the three results must go into one user message in order"
    );
}

/// The assistant message of a turn that called tools holds **every `tool_use` block**, and
/// the user message that follows holds **the same number of `tool_result`s**.
/// Any mismatch makes the next API request be rejected (spec Section 6.1.2).
///
/// The `TEST-11` test above looks only at the second request's **last message**
/// (user/tool_result). This test also looks at the **assistant message** before it (the
/// tool_use side) and checks that the two messages' ID sets correspond exactly — one
/// `ToolUse` block leaking from `assistant` makes the API reject even with the tool_results intact.
#[sqlx::test(migrations = "../../migrations")]
async fn the_assistant_turn_carries_every_tool_use_and_the_reply_matches_one_to_one(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;
    let turn = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__a".into(),
            input: serde_json::json!({}),
        },
        LlmEvent::ToolCall {
            tool_use_id: "tu_2".into(),
            tool: "s__b".into(),
            input: serde_json::json!({}),
        },
        LlmEvent::ToolCall {
            tool_use_id: "tu_3".into(),
            tool: "s__c".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::ToolUse),
    ];
    let provider = FakeProvider::new(vec![turn, vec![stopped(StopReason::EndTurn)]]);
    let tools = FakeTools::new(&["s__a", "s__b", "s__c"]);
    let mut budget = Budget::new(Limits::default(), Instant::now());
    let sink = NoopSink;
    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    let reqs = provider.requests();
    assert_eq!(
        reqs.len(),
        2,
        "it must issue one more request after executing tools"
    );
    let second = &reqs[1];

    let uses: Vec<&str> = second
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|c| match c {
            LlmContent::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let results: Vec<&str> = second
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|c| match c {
            LlmContent::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        uses,
        vec!["tu_1", "tu_2", "tu_3"],
        "the assistant turn lost a tool call"
    );
    assert_eq!(
        results,
        vec!["tu_1", "tu_2", "tu_3"],
        "the tool_results do not correspond one to one"
    );
}

/// Failed tools must be answered too, with is_error: true — omitting one makes the API reject.
#[sqlx::test(migrations = "../../migrations")]
async fn failed_tools_are_still_reported_with_is_error(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let turn1 = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__boom".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::ToolUse),
    ];
    let turn2 = vec![stopped(StopReason::EndTurn)];

    let provider = FakeProvider::new(vec![turn1, turn2]);
    let requests = Arc::clone(&provider.requests);
    let mut tools = FakeTools::new(&["s__boom"]);
    tools.fail = true;
    let mut budget = Budget::new(Limits::default(), Instant::now());
    let sink = NoopSink;

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    // Wrap the guard in a block so it drops before the `.await` below (clippy::await_holding_lock).
    let errored: Vec<bool> = {
        let reqs = requests.lock().unwrap();
        let last = reqs[1].messages.last().unwrap();
        last.content
            .iter()
            .filter_map(|c| match c {
                agentops_core::LlmContent::ToolResult { is_error, .. } => Some(*is_error),
                _ => None,
            })
            .collect()
    };
    assert_eq!(errored, vec![true], "the failed tool was not answered");

    // is_error must be recorded in the step too.
    let steps = store.steps_after(id, -1).await.unwrap();
    assert!(steps
        .iter()
        .any(|s| matches!(&s.kind, StepKind::ToolResult { is_error: true, .. })));
}

/// TEST-15 — the turn limit fires against a provider that never terminates.
/// Without this test an infinite loop would surface only in production.
#[sqlx::test(migrations = "../../migrations")]
async fn test_15_a_never_ending_provider_hits_the_turn_limit(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    // It demands a tool call every turn — it never ends on its own.
    let forever = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu".into(),
            tool: "s__loop".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::ToolUse),
    ];

    let provider = FakeProvider::new(vec![forever]);
    let tools = FakeTools::new(&["s__loop"]);
    let mut budget = Budget::new(
        Limits {
            max_turns_per_phase: 4,
            ..Default::default()
        },
        Instant::now(),
    );
    let sink = NoopSink;

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    assert_eq!(
        reason(&out.terminated),
        Some(TerminalReason::TurnLimitExceeded),
        "the limit did not fire and the loop ran on"
    );
    assert!(
        out.failed.is_none(),
        "exceeding a limit does not fail the investigation"
    );

    // **`out.terminated` alone cannot distinguish which limit fired.** `charge_turn` and
    // `charge_tool_calls` both return the same `TerminalReason::TurnLimitExceeded` on
    // failure. `charge_tool_calls(0)` cannot fail at any reachable `tool_calls` value
    // (with `n=0`, `tool_calls + 0 > max` is always false), so if `charge_turn` were
    // deleted, in this test's scenario of one tool call per turn `charge_tool_calls`
    // would fire instead and end with the same reason — an `out.terminated` assertion
    // alone would not catch that defect (measured: deleting `budget.charge_turn()` still
    // passed up to this assertion). How many times the provider was called is the only
    // discriminator.
    //
    // With `max_turns_per_phase: 4` and one tool call per turn (the default
    // `max_tool_calls_per_phase: 120` has far more headroom), a working `charge_turn`
    // catches the limit at turn 4 and `stream()` must be called exactly 4 times — the
    // fifth turn is cut when `charge_turn` fails, before `stream()` is called.
    //
    assert_eq!(
        provider.call_count(),
        4,
        "something other than the charge_turn limit stopped the loop — whether the limit \
         actually fired cannot be distinguished"
    );

    // A terminal step must remain.
    let steps = store.steps_after(id, -1).await.unwrap();
    assert!(steps.iter().any(|s| matches!(
        &s.kind,
        StepKind::Terminated {
            reason: TerminalReason::TurnLimitExceeded,
            ..
        }
    )));
}

/// A refusal fails the investigation (spec Section 8.4).
#[sqlx::test(migrations = "../../migrations")]
async fn refusal_fails_the_investigation(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let provider = FakeProvider::new(vec![vec![stopped(StopReason::Refusal)]]);
    let tools = FakeTools::new(&[]);
    let mut budget = Budget::new(Limits::default(), Instant::now());
    let sink = NoopSink;

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(
        reason(&out.failed),
        Some(TerminalReason::Refusal { category: None })
    );
}

/// The terminal step of the `Terminate` arm **is owned by this file** (spec Section 8.4).
///
/// This pins the asymmetry with `FailInvestigation` as intentional. That one sets `failed`
/// and the runner calls `fail_investigation`, which leaves the terminal row inside the
/// terminal transaction, so writing here would duplicate. This one sets `terminated`,
/// which the runner does not read before moving on, so **there is no second writer** —
/// without writing here the fact of termination survives nowhere.
///
/// Without this test, whoever removed the duplication in `FailInvestigation` would apply
/// the same cleanup to this arm and no test would fail (measured: deleting
/// `append_terminated` left all 20 passing).
#[sqlx::test(migrations = "../../migrations")]
async fn max_tokens_records_its_terminal_step_here_tagged_with_the_actual_phase(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let provider = FakeProvider::new(vec![vec![stopped(StopReason::MaxTokens)]]);
    let tools = FakeTools::new(&[]);
    let mut budget = Budget::new(Limits::default(), Instant::now());
    let sink = NoopSink;

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(reason(&out.terminated), Some(TerminalReason::MaxTokens));
    assert_eq!(
        out.failed, None,
        "MaxTokens does not fail the investigation"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::MaxTokens,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one MaxTokens terminal row: {terminated:#?}"
    );
    // **The real phase, not `Phase::All`** — evidence that this file wrote it rather than
    // the terminal transaction, distinguishing it from the runner path (`Phase::All`).
    assert_eq!(terminated[0].phase, Phase::Triage);
    // `detail` is not discarded — why it was truncated is the operator's only clue.
    let StepKind::Terminated { detail, .. } = &terminated[0].kind else {
        unreachable!()
    };
    assert!(detail.is_some(), "the Terminate arm discarded detail");
}

/// carried (the previous phases' summaries) goes into **a user message, not the system prompt**.
/// Putting it in system would change the prefix at every phase transition and invalidate the cache (spec Section 8.3).
#[sqlx::test(migrations = "../../migrations")]
async fn carried_summaries_go_into_messages_not_the_system_prompt(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let provider = FakeProvider::new(vec![vec![
        LlmEvent::TextBlock { text: "ok".into() },
        stopped(StopReason::EndTurn),
    ]]);
    let requests = Arc::clone(&provider.requests);
    let tools = FakeTools::new(&[]);
    let mut budget = Budget::new(Limits::default(), Instant::now());
    let sink = NoopSink;

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    run_phase_loop(
        &ctx,
        &mut budget,
        Phase::Rca,
        &[(Phase::Triage, "TRIAGE_SUMMARY_MARKER".into())],
    )
    .await
    .unwrap();

    let reqs = requests.lock().unwrap();
    assert!(
        !reqs[0].system.contains("TRIAGE_SUMMARY_MARKER"),
        "the previous phases' summaries went into the system prompt — the cache is invalidated"
    );
    let in_messages = reqs[0].messages.iter().any(|m| {
        m.content.iter().any(|c| match c {
            agentops_core::LlmContent::Text { text } => text.contains("TRIAGE_SUMMARY_MARKER"),
            _ => false,
        })
    });
    assert!(in_messages, "the previous phases' summaries are nowhere");
}

/// TEST-15 — the seventh limit of spec Section 5.4. A stalled stream must hit the idle timeout.
/// Without it an investigation hangs indefinitely past the 30-minute wall clock —
/// `check_wall_clock()` is called only at turn start, so a stall within one turn cuts nothing off.
#[sqlx::test(migrations = "../../migrations")]
async fn test_15_a_stalled_stream_hits_the_idle_timeout(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let provider = StallingProvider;
    let tools = FakeTools::new(&[]);
    let sink = NoopSink;
    // Inject a short idle ceiling so nothing actually waits 120 seconds.
    let mut budget = Budget::new(
        Limits {
            stream_idle_timeout: std::time::Duration::from_millis(80),
            ..Default::default()
        },
        Instant::now(),
    );

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    assert_eq!(
        reason(&out.terminated),
        Some(TerminalReason::WallClockExceeded),
        "the idle timeout did not fire and it hung on the stream"
    );
    assert!(
        out.failed.is_none(),
        "an idle timeout does not fail the investigation"
    );

    // **This file leaves no terminal row.** `WallClockExceeded` is the only reason the
    // runner re-derives independently through `budget.check_wall_clock()`, so two writers
    // exist. Firing late produces two rows; firing early produces a false record claiming
    // exhaustion while the investigation limit is nowhere near (spec Section 5.4 pins the
    // reason to `WallClockExceeded`, so no true reason can be chosen).
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Terminated { .. }))
        .collect();
    assert!(
        terminated.is_empty(),
        "the idle timeout left a terminal row — either duplicating the runner's re-derivation or false: {terminated:#?}"
    );

    // The cause must survive in a step — the UI must be able to show why it was cut.
    // With no terminal row, **this is the only record.**
    let errors = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Error { message } if message.contains("idle")))
        .count();
    assert_eq!(
        errors, 1,
        "the idle timeout cause was not recorded: {steps:?}"
    );
}

/// The idle timeout's real cause is propagated through `Termination.detail`.
/// Without it the UI sees only `WallClockExceeded` and shows "timed out" even with the
/// 30-minute budget intact — the real cause (a stalled stream) sits only in the adjacent
/// `Error` step and is never connected to the termination reason.
#[sqlx::test(migrations = "../../migrations")]
async fn idle_timeout_carries_its_real_cause_in_the_termination_detail(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;
    let provider = StallingProvider;
    let tools = FakeTools::new(&[]);
    let mut budget = Budget::new(
        Limits {
            stream_idle_timeout: std::time::Duration::from_millis(80),
            ..Default::default()
        },
        Instant::now(),
    );
    let sink = NoopSink;
    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    let t = out.terminated.expect("the idle timeout must end the phase");
    assert_eq!(t.reason, TerminalReason::WallClockExceeded);
    let d = t.detail.expect("the real cause must be in detail");
    assert!(
        d.contains("idle"),
        "detail must say it was an idle timeout: {d}"
    );
}

/// **A database-layer invariant: every `ToolCall` step has a matching `ToolResult` step.**
///
/// `TEST-11` looks only at pairing in the protocol message, not at `agent_steps`.
/// A `ToolCall` step is appended the moment it streams, so a path that ends without
/// executing the tool can violate this invariant silently. Here the tool call budget is
/// set to 1 so the second turn is cut by budget exhaustion.
#[sqlx::test(migrations = "../../migrations")]
async fn every_tool_call_step_has_a_matching_tool_result_step(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let one_call = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_a".into(),
            tool: "s__t".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::ToolUse),
    ];
    // One tool call per turn. With a budget of 1 it is cut at the second turn.
    let provider = FakeProvider::new(vec![one_call]);
    let tools = FakeTools::new(&["s__t"]);
    let sink = NoopSink;
    let mut budget = Budget::new(
        Limits {
            max_tool_calls_per_phase: 1,
            ..Default::default()
        },
        Instant::now(),
    );

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(
        reason(&out.terminated),
        Some(TerminalReason::TurnLimitExceeded)
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let mut calls: Vec<&str> = Vec::new();
    let mut results: Vec<&str> = Vec::new();
    for s in &steps {
        match &s.kind {
            StepKind::ToolCall { tool_use_id, .. } => calls.push(tool_use_id),
            StepKind::ToolResult { tool_use_id, .. } => results.push(tool_use_id),
            _ => {}
        }
    }
    calls.sort_unstable();
    results.sort_unstable();
    assert!(
        !calls.is_empty(),
        "no tool call was recorded at all, making this test meaningless"
    );
    assert_eq!(
        calls, results,
        "an unpaired ToolCall step remained in the database — calls {calls:?} vs results {results:?}"
    );

    // **This file is the sole writer of `charge_tool_calls`'s terminal row.** The runner
    // does not read `out.terminated`, so there is no second writer, and deleting it leaves
    // the fact of termination nowhere. The reason for counting is that the duplication on
    // the wall-clock and `FailInvestigation` paths hid behind presence assertions.
    //
    // This assertion alone cannot distinguish **which limit fired** — all three return
    // `TurnLimitExceeded`. That discrimination is done by the tool call pairing above and
    // the `max_tool_calls_per_phase: 1` fixture.
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::TurnLimitExceeded,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one terminal row for tool call budget exhaustion: {terminated:#?}"
    );
    assert_eq!(terminated[0].phase, Phase::Triage);
}

/// **Review C1 regression test.** `content_block_stop` streams a completed `tool_use`
/// block regardless of `stop_reason` — a turn can be truncated by `MaxTokens` with a tool
/// call already completed. `every_tool_call_step_has_a_matching_tool_result_step` covers
/// only the `charge_tool_calls` exhaustion path and missed this one (the `Terminate`
/// branch of `classify`) — the `PhaseDone` and `FailInvestigation` branches carried the same defect.
#[sqlx::test(migrations = "../../migrations")]
async fn tool_call_before_max_tokens_still_gets_a_matching_tool_result(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let turn = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__t".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::MaxTokens),
    ];
    let provider = FakeProvider::new(vec![turn]);
    let tools = FakeTools::new(&["s__t"]);
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(
        reason(&out.terminated),
        Some(TerminalReason::MaxTokens),
        "MaxTokens must end the phase"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let mut calls: Vec<&str> = Vec::new();
    let mut results: Vec<&str> = Vec::new();
    for s in &steps {
        match &s.kind {
            StepKind::ToolCall { tool_use_id, .. } => calls.push(tool_use_id),
            StepKind::ToolResult { tool_use_id, .. } => results.push(tool_use_id),
            _ => {}
        }
    }
    calls.sort_unstable();
    results.sort_unstable();
    assert!(
        !calls.is_empty(),
        "no tool call was recorded at all, making this test meaningless"
    );
    assert_eq!(
        calls, results,
        "a turn ended by MaxTokens left an unpaired ToolCall step — calls {calls:?} vs results {results:?}"
    );
}

/// **Review C1 regression test (the `PhaseDone` branch only).**
/// `every_tool_call_step_has_a_matching_tool_result_step` covers the `charge_tool_calls`
/// exhaustion path and `tool_call_before_max_tokens_still_gets_a_matching_tool_result`
/// covers the `Terminate` branch — neither verifies the `PhaseDone` branch (a completed
/// tool call with `stop_reason` of `EndTurn`). "A sibling branch is tested" does not mean
/// this branch is tested (CLAUDE.md, "presence is not multiplicity") — all three branches
/// use the same `reject_pending` pattern, but without this test, deleting or breaking one
/// would pass silently while the other two branches' tests still pass.
#[sqlx::test(migrations = "../../migrations")]
async fn tool_call_before_end_turn_still_gets_a_matching_tool_result(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let turn = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__t".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::EndTurn),
    ];
    let provider = FakeProvider::new(vec![turn]);
    let tools = FakeTools::new(&["s__t"]);
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert!(
        out.terminated.is_none(),
        "EndTurn must terminate the phase cleanly"
    );
    assert!(
        out.failed.is_none(),
        "EndTurn does not fail the investigation"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let mut calls: Vec<&str> = Vec::new();
    let mut results: Vec<&str> = Vec::new();
    for s in &steps {
        match &s.kind {
            StepKind::ToolCall { tool_use_id, .. } => calls.push(tool_use_id),
            StepKind::ToolResult { tool_use_id, .. } => results.push(tool_use_id),
            _ => {}
        }
    }
    calls.sort_unstable();
    results.sort_unstable();
    assert!(
        !calls.is_empty(),
        "no tool call was recorded at all, making this test meaningless"
    );
    assert_eq!(
        calls, results,
        "a turn ended by EndTurn left an unpaired ToolCall step — calls {calls:?} vs results {results:?}"
    );
}

/// **Review C1 regression test (the `FailInvestigation` branch only).** The same reason as
/// above — `refusal_fails_the_investigation` uses a turn with **no** tool call and so
/// cannot verify this branch's `reject_pending` call.
#[sqlx::test(migrations = "../../migrations")]
async fn tool_call_before_refusal_still_gets_a_matching_tool_result(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let turn = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__t".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::Refusal),
    ];
    let provider = FakeProvider::new(vec![turn]);
    let tools = FakeTools::new(&["s__t"]);
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(
        reason(&out.failed),
        Some(TerminalReason::Refusal { category: None }),
        "a Refusal must fail the investigation"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let mut calls: Vec<&str> = Vec::new();
    let mut results: Vec<&str> = Vec::new();
    for s in &steps {
        match &s.kind {
            StepKind::ToolCall { tool_use_id, .. } => calls.push(tool_use_id),
            StepKind::ToolResult { tool_use_id, .. } => results.push(tool_use_id),
            _ => {}
        }
    }
    calls.sort_unstable();
    results.sort_unstable();
    assert!(
        !calls.is_empty(),
        "no tool call was recorded at all, making this test meaningless"
    );
    assert_eq!(
        calls, results,
        "a turn ended by Refusal left an unpaired ToolCall step — calls {calls:?} vs results {results:?}"
    );
}

/// A mid-stream error event terminates the phase and leaves an Error step.
#[sqlx::test(migrations = "../../migrations")]
async fn stream_error_terminates_the_phase_and_is_recorded(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let provider = FakeProvider::new(vec![vec![LlmEvent::StreamError {
        message: "overloaded_error: slow down".into(),
    }]]);
    let tools = FakeTools::new(&[]);
    let mut budget = Budget::new(Limits::default(), Instant::now());
    let sink = NoopSink;

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert!(
        out.terminated.is_some(),
        "a stream error must end the phase"
    );

    // **Count, do not probe for presence.** `any()` cannot tell one row from two and so
    // has no detection power against a duplicate-record regression — the duplication on
    // the wall-clock (`9d27771`) and `FailInvestigation` (`230c78f`) paths hid in exactly
    // that way. Not because this path carries a duplication risk today, but so this
    // assertion catches it if one appears.
    let steps = store.steps_after(id, -1).await.unwrap();
    let errors = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Error { .. }))
        .count();
    assert_eq!(errors, 1, "there must be exactly one Error step: {steps:?}");

    // **Review I1.** Like the other paths that end in `TaskPanicked` (initial connection
    // failure, transport error, a stream ending with no stop_reason), an `Error` step
    // alone leaves a UI reading only `agent_steps` unable to reconstruct a structured
    // termination reason — consistently with every other termination path (exceeded
    // limits, MaxTokens) leaving a `Terminated` step, `StreamError` must leave one too
    // (spec Section 8.4: "a broken turn is treated as Terminated").
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::TaskPanicked,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one TaskPanicked terminal row: {steps:?}"
    );
    // This path sets `terminated`, which the runner does not read — there is no second
    // writer, and what this file leaves with the real phase tag is the only record.
    assert_eq!(terminated[0].phase, Phase::Triage);
}

/// **Review I1 regression test (the initial connection failure path).** The re-examination
/// confirmed in the code that all four `TaskPanicked` paths leave exactly one `Terminated`
/// step each, but the three paths other than `StreamError` had nothing verifying it — this
/// path, where the `stream()` call itself fails, cannot be built with `FakeProvider`
/// (`stream()` always returns `Ok`) and needed a dedicated provider.
#[sqlx::test(migrations = "../../migrations")]
async fn initial_connect_failure_records_exactly_one_terminated_step(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let provider = FailingConnectProvider;
    let tools = FakeTools::new(&[]);
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(reason(&out.terminated), Some(TerminalReason::TaskPanicked));
    assert!(
        out.failed.is_none(),
        "a connection failure does not fail the investigation"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let errors = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Error { .. }))
        .count();
    assert_eq!(errors, 1, "there must be exactly one Error step: {steps:?}");

    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::TaskPanicked,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one TaskPanicked terminal row: {steps:?}"
    );
    let StepKind::Terminated { detail, .. } = &terminated[0].kind else {
        unreachable!()
    };
    assert_eq!(
        detail.as_deref(),
        Some("failed to start the LLM stream"),
        "the connection failure's detail vanished or changed"
    );
}

/// **Review I1 regression test (the mid-stream transport error path).** The same reason as
/// above — the case where a few `content_block_delta` and `content_block_stop` events
/// arrive normally and then the stream itself is cut by an `Err` item. `FakeProvider`
/// wraps every event in `Ok`, so this path needed a separate provider too.
#[sqlx::test(migrations = "../../migrations")]
async fn mid_stream_transport_error_records_exactly_one_terminated_step(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let provider = MidStreamTransportErrorProvider;
    let tools = FakeTools::new(&[]);
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(reason(&out.terminated), Some(TerminalReason::TaskPanicked));
    assert!(
        out.failed.is_none(),
        "a transport error does not fail the investigation"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let errors = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Error { .. }))
        .count();
    assert_eq!(errors, 1, "there must be exactly one Error step: {steps:?}");

    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::TaskPanicked,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one TaskPanicked terminal row: {steps:?}"
    );
    let StepKind::Terminated { detail, .. } = &terminated[0].kind else {
        unreachable!()
    };
    assert_eq!(
        detail.as_deref(),
        Some("stream transport error"),
        "the transport error's detail vanished or changed"
    );
}

/// **Review I1 regression test (the stream-ended-without-stop_reason path).** The same
/// reason as above — this path could have been built with `FakeProvider` all along (just
/// omit `Stopped` from the turn), but no test used it.
#[sqlx::test(migrations = "../../migrations")]
async fn stream_ended_without_stop_reason_records_exactly_one_terminated_step(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    // A turn with no `Stopped` — the stream emits events and ends with no stop_reason.
    let turn = vec![LlmEvent::TextBlock {
        text: "partial output".into(),
    }];
    let provider = FakeProvider::new(vec![turn]);
    let tools = FakeTools::new(&[]);
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();
    assert_eq!(reason(&out.terminated), Some(TerminalReason::TaskPanicked));
    assert!(
        out.failed.is_none(),
        "a stream ending without stop_reason does not fail the investigation"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let errors = steps
        .iter()
        .filter(|s| matches!(&s.kind, StepKind::Error { .. }))
        .count();
    assert_eq!(errors, 1, "there must be exactly one Error step: {steps:?}");

    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::TaskPanicked,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one TaskPanicked terminal row: {steps:?}"
    );
    let StepKind::Terminated { detail, .. } = &terminated[0].kind else {
        unreachable!()
    };
    assert_eq!(
        detail.as_deref(),
        Some("stream ended without stop_reason"),
        "the missing-stop_reason detail vanished or changed"
    );
}

/// **Review I2.** `charge_turn`, `charge_tool_calls`, and `charge_pause_resume` all return
/// the same `TerminalReason::TurnLimitExceeded` on failure. Round 2 distinguished
/// `charge_turn` from `charge_tool_calls` by the provider call count, but the third
/// counter, `charge_pause_resume`, is still undistinguished at the `run_phase_loop` level
/// — low priority since `PauseTurn` is a defensive path that should not occur in v0.1
/// (spec Section 8.6), but it is the remaining third of the same masking problem.
///
/// With a provider that returns only `PauseTurn` every turn and calls no tools,
/// `charge_tool_calls` is never called (there are no tools) and `max_turns_per_phase` is
/// left comfortably at its default of 40 so `charge_turn` does not fire either — only
/// `charge_pause_resume` can. With `max_pause_turn_resumes: 2`: turn 1
/// (`charge_pause_resume` 0→1 succeeds), turn 2 (1→2 succeeds), turn 3 (2 >= 2 fails,
/// after `stream()` was already called) — `stream()` must be called exactly 3 times.
/// If `charge_pause_resume` were deleted, the only thing left is `charge_turn`, which cuts
/// only at turn 40 (the default `max_turns_per_phase`), so the count widens to 3 versus 40
/// and is distinguishable.
#[sqlx::test(migrations = "../../migrations")]
async fn test_15_pause_turn_resume_limit_is_discriminated_by_call_count(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let forever_pause = vec![stopped(StopReason::PauseTurn)];
    let provider = FakeProvider::new(vec![forever_pause]);
    let tools = FakeTools::new(&[]);
    let sink = NoopSink;
    let mut budget = Budget::new(
        Limits {
            max_pause_turn_resumes: 2,
            ..Default::default()
        },
        Instant::now(),
    );

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    assert_eq!(
        reason(&out.terminated),
        Some(TerminalReason::TurnLimitExceeded),
        "the pause_resume limit did not fire"
    );
    assert!(
        out.failed.is_none(),
        "exceeding a limit does not fail the investigation"
    );
    assert_eq!(
        provider.call_count(),
        3,
        "cannot distinguish whether charge_pause_resume fired rather than charge_turn or \
         charge_tool_calls — had another counter fired instead it would have been called \
         far more times"
    );

    // **This file is the sole writer of `charge_pause_resume`'s terminal row too.** The
    // same reason as `charge_tool_calls` above, and which limit fired is distinguished by
    // the call count assertion just above — all three use the same `TurnLimitExceeded`.
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::TurnLimitExceeded,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there must be exactly one terminal row for the pause_resume limit: {terminated:#?}"
    );
    assert_eq!(terminated[0].phase, Phase::Triage);
}

/// Spec Section 5.4 — the individual tool call timeout ends the phase with `ToolTimeout { tool }`.
///
/// The only limit the table exempts from termination is the parallel call limit. Passing a
/// timeout along as an `is_error` result would leave the termination reason only as a
/// string inside the `ToolResult` text, and `TerminalReason::ToolTimeout` would never be constructed in production.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tool_timeout_terminates_the_phase_with_a_structured_reason(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let turn = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__slow".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::ToolUse),
    ];
    let provider = FakeProvider::new(vec![turn]);
    let mut tools = FakeTools::new(&["s__slow"]);
    tools.time_out = true;
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    let out = run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    assert_eq!(
        reason(&out.terminated),
        Some(TerminalReason::ToolTimeout {
            tool: "s__slow".into()
        }),
        "the tool timeout did not end the phase"
    );
    assert!(
        out.failed.is_none(),
        "a timeout does not fail the investigation"
    );

    let steps = store.steps_after(id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| {
            matches!(
                &s.kind,
                StepKind::Terminated {
                    reason: TerminalReason::ToolTimeout { .. },
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        terminated.len(),
        1,
        "there is no structured ToolTimeout terminal row: {steps:#?}"
    );
    assert_eq!(terminated[0].phase, Phase::Triage);

    // Even when ending on a timeout, **the pairing invariant must hold.**
    let mut calls: Vec<&str> = Vec::new();
    let mut results: Vec<&str> = Vec::new();
    for s in &steps {
        match &s.kind {
            StepKind::ToolCall { tool_use_id, .. } => calls.push(tool_use_id),
            StepKind::ToolResult { tool_use_id, .. } => results.push(tool_use_id),
            _ => {}
        }
    }
    calls.sort_unstable();
    results.sort_unstable();
    assert_eq!(calls, results, "the timeout path left an unpaired ToolCall");
}

/// **A `pause_turn` resumption does not send an unpaired `tool_use`** (spec Section 6.1.2).
///
/// `reject_pending` only leaves refused `ToolResult` **steps** in the database and puts
/// nothing into the protocol message. Resending with the `ToolUse` still in `assistant`
/// makes the durable record say "not executed" while the wire says "a call is in
/// progress", and the API rejects the next request — the very situation this arm claims to prevent.
#[sqlx::test(migrations = "../../migrations")]
async fn pause_turn_resume_does_not_resend_an_unpaired_tool_use(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let first = vec![
        LlmEvent::ToolCall {
            tool_use_id: "tu_1".into(),
            tool: "s__t".into(),
            input: serde_json::json!({}),
        },
        stopped(StopReason::PauseTurn),
    ];
    let second = vec![stopped(StopReason::EndTurn)];
    let provider = FakeProvider::new(vec![first, second]);
    let tools = FakeTools::new(&["s__t"]);
    let sink = NoopSink;
    let mut budget = Budget::new(Limits::default(), Instant::now());

    let ctx = PhaseCtx {
        investigation_id: id,
        store: store.as_ref(),
        provider: &provider,
        tools: &tools,
        sink: &sink,
    };
    run_phase_loop(&ctx, &mut budget, Phase::Triage, &[])
        .await
        .unwrap();

    // Look at the messages carried in the resumption request.
    let reqs: Vec<LlmRequest> = provider.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2, "no resumption request went out");

    let mut tool_use: Vec<&str> = Vec::new();
    let mut tool_result: Vec<&str> = Vec::new();
    for m in &reqs[1].messages {
        for c in &m.content {
            match c {
                LlmContent::ToolUse { id, .. } => tool_use.push(id),
                LlmContent::ToolResult { tool_use_id, .. } => tool_result.push(tool_use_id),
                LlmContent::Text { .. } => {}
            }
        }
    }
    assert_eq!(
        tool_use, tool_result,
        "the resumption request carried an unpaired tool_use — tool_use {tool_use:?} vs tool_result {tool_result:?}"
    );
}

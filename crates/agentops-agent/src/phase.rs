//! The phase sub-loop (spec Sections 5.3, 6.1.3, and 8.4).
//!
//! **Each phase is an independent LLM conversation.** `messages[]` starts fresh per phase
//! and only a summary carries forward. This bounds context growth per phase.
//!
//! **Persisted steps and deltas are separated** (Section 6.1.3). Deltas stream to the
//! browser through `DeltaSink` and are never written to the database. Only completed
//! blocks, tool calls, tool results, and termination reasons are `append_step`ed. Inserting per delta would make one investigation tens of thousands of rows.

use crate::limits::{cap_parallel, Budget};
use crate::outcome::{classify, TurnOutcome};
use crate::prompt::{assemble_system, sort_tools};
use crate::MAX_TOKENS;
use agentops_core::{
    LlmContent, LlmError, LlmEvent, LlmMessage, LlmProvider, LlmRequest, LlmRole, Phase, StepKind,
    Store, StoreError, TerminalReason, ToolError, ToolRegistry,
};
use futures_util::StreamExt;
use uuid::Uuid;

/// The receiver of transient deltas. **It never writes to the database** (spec Section 6.1.3).
///
/// Plan 2 tests with `NoopSink` and plan 3 connects it to broadcast. Without this trait
/// the runner would have to discard deltas and plan 3 would have to modify this file again.
pub trait DeltaSink: Send + Sync {
    fn text(&self, investigation_id: Uuid, phase: Phase, delta: &str);
    fn thinking(&self, investigation_id: Uuid, phase: Phase, delta: &str);
}

pub struct NoopSink;

impl DeltaSink for NoopSink {
    fn text(&self, _investigation_id: Uuid, _phase: Phase, _delta: &str) {}
    fn thinking(&self, _investigation_id: Uuid, _phase: Phase, _delta: &str) {}
}

pub struct PhaseCtx<'a> {
    pub investigation_id: Uuid,
    pub store: &'a dyn Store,
    pub provider: &'a dyn LlmProvider,
    pub tools: &'a dyn ToolRegistry,
    pub sink: &'a dyn DeltaSink,
}

/// Why a phase ended, with its elaboration.
///
/// **`detail` is not a separate field on `PhaseOutcome`.** That would make all but the
/// one site the runner reads, of the eleven that set `terminated`, into
/// "set-but-never-read values" — plan 2's C1 was exactly that shape, and it left
/// investigations recorded as `completed` after doing nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Termination {
    pub reason: TerminalReason,
    pub detail: Option<String>,
}

impl Termination {
    pub fn new(reason: TerminalReason) -> Self {
        Self {
            reason,
            detail: None,
        }
    }

    pub fn with_detail(reason: TerminalReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseOutcome {
    /// The summary carried to the next phase. The text blocks this phase produced, concatenated.
    pub summary: String,
    /// The phase ended on an exceeded limit, truncation, or a stream error. **The investigation continues.**
    pub terminated: Option<Termination>,
    /// The whole investigation must be marked `failed`.
    pub failed: Option<Termination>,
}

/// Runs one phase to completion.
///
/// `carried` holds the previous phases' summaries and **goes into the first user
/// message.** Putting it in the system prompt would change the prefix at every phase transition and invalidate the prompt cache (spec Section 8.3).
pub async fn run_phase_loop(
    ctx: &PhaseCtx<'_>,
    budget: &mut Budget,
    phase: Phase,
    carried: &[(Phase, String)],
) -> Result<PhaseOutcome, StoreError> {
    let id = ctx.investigation_id;

    // **The tool list is fetched once at phase start and frozen.** Changing it
    // mid-conversation invalidates the whole cache, since tools render at the front of the prompt prefix (spec Section 9).
    // **An investigation does not proceed without tools.** This is not a degraded mode
    // but a different product — with no tools the model writes a plausible root-cause
    // narrative from prior knowledge alone, and in the database that is indistinguishable
    // from a real investigation. The worst output an incident-response tool can produce is not a crash but **a plausible wrong answer**.
    //
    // The nature of the failure that reaches here is the basis for this judgment.
    // `McpToolRegistry::list` already swallows per-server `tools/list` failures
    // individually (spec Section 9 — one dead server does not stop the rest), so this
    // `Err` is effectively **only a policy store error**. That is not "a few tools are
    // missing" but "we do not know what is allowed", and proceeding with an empty list
    // under a deny-by-default policy (Section 9.1) is equivalent to bypassing the policy.
    //
    // It does not fail the whole investigation — being `terminated`, execution continues
    // if the store recovers in the next phase. If all three phases end this way, the runner fails the investigation.
    let tools = match ctx.tools.list().await {
        Ok(t) => sort_tools(t),
        Err(e) => {
            tracing::warn!(error = %e, "tools/list failed; ending the phase");
            // Recorded durably. `tracing::warn!` stays in the process log only and is
            // invisible to the UI and the operator reading `agent_steps`.
            ctx.store
                .append_step(
                    id,
                    phase,
                    &StepKind::Error {
                        message: format!("tools/list failed: {e}"),
                    },
                )
                .await?;
            // **Not a panic.** It means a dependency that is external and recoverable —
            // the policy store, an MCP server — was unavailable for this phase alone.
            // Lumping it under `TaskPanicked` widens that gap as dependencies grow in plan 3.
            let reason = TerminalReason::DependencyUnavailable {
                what: "tool policy store".into(),
            };
            let detail = "tools/list failed; refusing to run a phase with no tools".to_string();
            append_terminated(ctx, phase, &reason, Some(detail.clone())).await?;
            return Ok(PhaseOutcome {
                summary: String::new(),
                terminated: Some(Termination::with_detail(reason, detail)),
                failed: None,
            });
        }
    };

    let instructions = ctx.store.instructions_for(&[Phase::All, phase]).await?;
    let system = assemble_system(&instructions, phase);

    // The first user message: the investigation prompt plus the previous phases'
    // summaries. Putting carried here is Section 8.3's "dynamic context toward the end of messages".
    let inv = ctx.store.get_investigation(id).await?;
    let mut opening = inv.prompt.clone();
    for (p, summary) in carried {
        opening.push_str("\n\n## Previous phase: ");
        opening.push_str(p.as_str());
        opening.push('\n');
        opening.push_str(summary);
    }
    let mut messages = vec![LlmMessage::user_text(opening)];

    let mut summary = String::new();
    let mut terminated: Option<Termination> = None;
    let mut failed: Option<Termination> = None;

    'phase: loop {
        if let Err(reason) = budget.check_wall_clock() {
            terminated = Some(Termination::new(reason));
            break 'phase;
        }
        if let Err(reason) = budget.charge_turn() {
            append_terminated(ctx, phase, &reason, None).await?;
            terminated = Some(Termination::new(reason));
            break 'phase;
        }

        let req = LlmRequest {
            system: system.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            max_tokens: MAX_TOKENS,
        };

        let mut stream = match ctx.provider.stream(req).await {
            Ok(s) => s,
            Err(e) => {
                ctx.store
                    .append_step(
                        id,
                        phase,
                        &StepKind::Error {
                            message: e.to_string(),
                        },
                    )
                    .await?;
                // Spec Section 8.4: a broken turn is recorded as `Terminated` and ends the
                // phase. Leaving only an `Error` step would make this the one path where a
                // UI reconstructing from `agent_steps` alone sees no structured termination reason.
                let detail = "failed to start the LLM stream".to_string();
                append_terminated(
                    ctx,
                    phase,
                    &TerminalReason::TaskPanicked,
                    Some(detail.clone()),
                )
                .await?;
                terminated = Some(Termination::with_detail(
                    TerminalReason::TaskPanicked,
                    detail,
                ));
                break 'phase;
            }
        };

        // Collect this turn's assistant blocks and tool calls. **Every** tool call must be
        // included in the assistant turn — if a tool_use_id loses its pair the follow-up
        // request is rejected (spec Section 6.1.2).
        let mut assistant: Vec<LlmContent> = Vec::new();
        let mut pending: Vec<(String, String, serde_json::Value)> = Vec::new();
        // The refusal category is carried along — it must reach `classify` for
        // `TerminalReason::Refusal { category }` to hold a real value (spec Section 8.4).
        let mut stop: Option<(agentops_core::StopReason, Option<String>)> = None;

        // **The seventh limit of spec Section 5.4: the LLM stream idle timeout (120s by default).**
        // Without it, `stream.next()` awaits forever on a stalled connection, and because
        // `check_wall_clock()` is called only at turn start, a stall within one turn cuts
        // nothing off — the investigation hangs indefinitely past the 30-minute limit.
        // Plan 1 created `LlmError::IdleTimeout` for exactly this.
        let idle = budget.limits().stream_idle_timeout;
        loop {
            let item = match tokio::time::timeout(idle, stream.next()).await {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_elapsed) => {
                    let e = LlmError::IdleTimeout {
                        seconds: idle.as_secs(),
                    };
                    let detail = e.to_string();
                    ctx.store
                        .append_step(
                            id,
                            phase,
                            &StepKind::Error {
                                message: detail.clone(),
                            },
                        )
                        .await?;
                    reject_pending(ctx, phase, &mut pending, "stream idle timeout").await?;
                    // **No `Terminated` row is written here.**
                    //
                    // The criterion: **`WallClockExceeded` is the only reason the runner
                    // re-derives independently.** Other reasons reach the runner only
                    // through `out.failed` (`FailInvestigation`) or not at all (`Terminate`,
                    // `TaskPanicked` — the runner does not read `out.terminated`), so this
                    // file is the sole writer and must write. Only the wall clock is
                    // recomputed by the runner through `budget.check_wall_clock()`, which
                    // creates two writers.
                    //
                    // A late-firing idle timeout has accumulated past the investigation
                    // limit too, so the runner's re-derivation fires and writing here would
                    // produce two rows (the same shape as `9d27771` and `230c78f`).
                    //
                    // **The early-firing case is worse.** Writing
                    // `Terminated { WallClockExceeded }` while the investigation limit is
                    // nowhere near is **a false record** — it claims the 30-minute budget is
                    // exhausted when in fact one stream stalled for 120 seconds and the
                    // investigation continues. Spec Section 5.4 pins the idle timeout's
                    // reason to `WallClockExceeded` (the table at line 297), so the reason
                    // cannot be changed. With no true reason available, no terminal **row** is written.
                    //
                    // **`detail` is kept.** Not writing a row and discarding the cause are
                    // different — a runner reading `PhaseOutcome::terminated` and seeing
                    // only `WallClockExceeded` with no `detail` makes the UI show "the
                    // 30-minute budget is exhausted" when the real cause was one stream
                    // stalling for 120 seconds. It reuses the same string already written to
                    // the `Error` step just above — two copies would be two sources of truth.
                    terminated = Some(Termination::with_detail(
                        TerminalReason::WallClockExceeded,
                        detail,
                    ));
                    break 'phase;
                }
            };
            match item {
                Err(e) => {
                    ctx.store
                        .append_step(
                            id,
                            phase,
                            &StepKind::Error {
                                message: e.to_string(),
                            },
                        )
                        .await?;
                    reject_pending(ctx, phase, &mut pending, "stream transport error").await?;
                    // Spec Section 8.4: a broken turn is recorded as `Terminated`.
                    let detail = "stream transport error".to_string();
                    append_terminated(
                        ctx,
                        phase,
                        &TerminalReason::TaskPanicked,
                        Some(detail.clone()),
                    )
                    .await?;
                    terminated = Some(Termination::with_detail(
                        TerminalReason::TaskPanicked,
                        detail,
                    ));
                    break 'phase;
                }
                // Deltas go only to the sink. **No append_step** (spec Section 6.1.3) —
                // inserting per delta would make one investigation tens of thousands of rows.
                Ok(LlmEvent::TextDelta { text }) => ctx.sink.text(id, phase, &text),
                Ok(LlmEvent::ThinkingDelta { text }) => ctx.sink.thinking(id, phase, &text),

                Ok(LlmEvent::TextBlock { text }) => {
                    ctx.store
                        .append_step(id, phase, &StepKind::Text { text: text.clone() })
                        .await?;
                    if !summary.is_empty() {
                        summary.push('\n');
                    }
                    summary.push_str(&text);
                    assistant.push(LlmContent::Text { text });
                }
                Ok(LlmEvent::ThinkingBlock { summary: s }) => {
                    ctx.store
                        .append_step(id, phase, &StepKind::Thinking { summary: s })
                        .await?;
                    // Thinking is not echoed back into messages — the server retains it.
                }
                Ok(LlmEvent::ToolCall {
                    tool_use_id,
                    tool,
                    input,
                }) => {
                    ctx.store
                        .append_step(
                            id,
                            phase,
                            &StepKind::ToolCall {
                                tool_use_id: tool_use_id.clone(),
                                tool: tool.clone(),
                                input: input.clone(),
                            },
                        )
                        .await?;
                    assistant.push(LlmContent::ToolUse {
                        id: tool_use_id.clone(),
                        name: tool.clone(),
                        input: input.clone(),
                    });
                    pending.push((tool_use_id, tool, input));
                }
                Ok(LlmEvent::StreamError { message }) => {
                    ctx.store
                        .append_step(
                            id,
                            phase,
                            &StepKind::Error {
                                message: message.clone(),
                            },
                        )
                        .await?;
                    reject_pending(ctx, phase, &mut pending, "stream error").await?;
                    // Spec Section 8.4: a broken turn is recorded as `Terminated`.
                    append_terminated(
                        ctx,
                        phase,
                        &TerminalReason::TaskPanicked,
                        Some(message.clone()),
                    )
                    .await?;
                    terminated = Some(Termination::with_detail(
                        TerminalReason::TaskPanicked,
                        message,
                    ));
                    break 'phase;
                }
                Ok(LlmEvent::Stopped {
                    reason,
                    refusal_category,
                    ..
                }) => stop = Some((reason, refusal_category)),
            }
        }

        // The stream ended with no stop_reason, meaning the network was cut midway.
        // **No retry after partial streaming** (spec Section 8.4) — text, tool calls, and
        // side effects could be duplicated.
        let Some((stop, refusal_category)) = stop else {
            ctx.store
                .append_step(
                    id,
                    phase,
                    &StepKind::Error {
                        message: "stream ended without stop_reason".into(),
                    },
                )
                .await?;
            reject_pending(ctx, phase, &mut pending, "stream ended without stop_reason").await?;
            // Spec Section 8.4: a broken turn is recorded as `Terminated`.
            let detail = "stream ended without stop_reason".to_string();
            append_terminated(
                ctx,
                phase,
                &TerminalReason::TaskPanicked,
                Some(detail.clone()),
            )
            .await?;
            terminated = Some(Termination::with_detail(
                TerminalReason::TaskPanicked,
                detail,
            ));
            break 'phase;
        };

        match classify(&stop, refusal_category.as_deref()) {
            TurnOutcome::PhaseDone => {
                // **An `EndTurn` with a non-empty `pending` is the same contradiction as
                // `Continue` (PauseTurn).** `content_block_stop` streams a completed
                // `tool_use` block regardless of `stop_reason`, so a turn can end with a
                // completed tool call while `stop_reason` is not `tool_use` — that tool is
                // never executed. Discarding it silently leaves the `ToolCall` step in the
                // database with its matching `ToolResult` missing forever (a violation of the global invariant).
                if !pending.is_empty() {
                    tracing::warn!(
                        pending = pending.len(),
                        "end_turn observed with pending tool calls; rejecting them so no ToolCall step is left unpaired"
                    );
                    reject_pending(
                        ctx,
                        phase,
                        &mut pending,
                        "phase ended before pending tool calls could run",
                    )
                    .await?;
                }
                break 'phase;
            }

            TurnOutcome::Terminate { reason, detail } => {
                // The same reason as `PhaseDone` above. `MaxTokens` can follow a completed
                // `tool_use` block — and as `classify`'s comment says, a truncated turn is
                // not resent, so an already-recorded `ToolCall` not refused here stays a
                // result-less call forever.
                reject_pending(
                    ctx,
                    phase,
                    &mut pending,
                    "phase terminated before pending tool calls could run",
                )
                .await?;
                append_terminated(ctx, phase, &reason, detail.clone()).await?;
                terminated = Some(match detail {
                    Some(d) => Termination::with_detail(reason, d),
                    None => Termination::new(reason),
                });
                break 'phase;
            }

            TurnOutcome::FailInvestigation { reason } => {
                // The same reason as above — `Refusal`, `ContextWindowExceeded`, and
                // `UnknownStopReason` can also follow a completed `tool_use` block.
                reject_pending(
                    ctx,
                    phase,
                    &mut pending,
                    "investigation failed before pending tool calls could run",
                )
                .await?;
                // **`append_terminated` is not called here.**
                //
                // The general rule: **a `Terminated` row is owned by the writer that flips
                // the status with it.** When a writer that does not flip the status leaves a
                // terminal row outside the transaction, losing the race leaves that row
                // alone, contradicting the real termination reason.
                //
                // This arm is the **only** place that sets `failed`, and `failed` is the
                // only field the runner actually reads. The runner takes it, calls
                // `fail_investigation`, and that writes `Terminated { reason }` as
                // `Phase::All` inside the terminal transaction. Writing again here would
                // (1) produce two rows with the same reason on the normal path (differing
                // only in the phase tag), and (2) leave this row already written
                // unconditionally even when shutdown wins first and `fail_investigation`
                // backs off with `Conflict`, leaving a `Refusal` termination record on an investigation that ended by shutdown.
                //
                // **The other arms are left alone.** `Terminate` and the four
                // `TaskPanicked` paths set `terminated`, which the runner does not read
                // before moving on — with no second writer, this is the only record, and
                // deleting it would erase the fact of termination itself.
                // Evidence: `9d27771` applied the same judgment to the wall-clock path.
                failed = Some(Termination::new(reason));
                break 'phase;
            }

            TurnOutcome::Continue => {
                // **A PauseTurn with a non-empty `pending` is a contradictory state.**
                // Resending the partial response as-is leaves a `tool_use` unpaired and the
                // next request is rejected (spec Section 6.1.2). v0.1 uses no server-side
                // tools, so this path should not occur (Section 8.6), and an occurrence
                // signals something leaked in. Rather than discarding silently, they are drained as refused results.
                // Spec Section 8.6 — v0.1 uses no server-side tools, so `pause_turn` itself
                // should not occur. A signal is left whether or not `pending` is empty.
                // Passing over it silently when empty would erase, without a trace, the fact
                // that a path which should not occur did occur.
                tracing::warn!(
                    pending = pending.len(),
                    "pause_turn observed; v0.1 uses no server-side tools so this path should not fire"
                );
                if !pending.is_empty() {
                    reject_pending(
                        ctx,
                        phase,
                        &mut pending,
                        "pause_turn with pending tool calls",
                    )
                    .await?;
                }
                if let Err(reason) = budget.charge_pause_resume() {
                    append_terminated(ctx, phase, &reason, None).await?;
                    terminated = Some(Termination::new(reason));
                    break 'phase;
                }
                // **The refused tool calls are removed from the assistant turn too.**
                // `reject_pending` above only leaves refused `ToolResult` **steps** in the
                // database and puts nothing into the protocol message. Resending with the
                // `ToolUse` blocks still in `assistant` would send an unpaired `tool_use`
                // and the API would reject the next request — the very situation the comment
                // just above says it prevents (citing Section 6.1.2). It is also a mismatch
                // where the durable record says "not executed" while the wire says "a call
                // is in progress".

                assistant.retain(|c| !matches!(c, LlmContent::ToolUse { .. }));

                // The partial response is **appended** as an assistant turn before
                // resending. Resending the same request without appending duplicates work (spec Section 8.4).
                if !assistant.is_empty() {
                    messages.push(LlmMessage {
                        role: LlmRole::Assistant,
                        content: assistant,
                    });
                }
                continue 'phase;
            }

            TurnOutcome::RunTools => {
                // The per-turn parallel call limit. **Exceeding it does not end the phase**
                // — only the excess is answered with is_error (spec Section 5.4).
                let (run, reject) = cap_parallel(pending.len(), budget.limits());
                if reject > 0 {
                    tracing::warn!(
                        rejected = reject,
                        cap = budget.limits().max_parallel_tool_calls,
                        "turn exceeded the parallel tool-call cap"
                    );
                }
                if let Err(reason) = budget.charge_tool_calls(run as u32) {
                    // **No unpaired `ToolCall` step is left behind.** A ToolCall step is
                    // appended immediately during streaming, so simply breaking here would
                    // leave a result-less call in the database forever. `TEST-11` looks only
                    // at pairing in the protocol message, not at `agent_steps`, and missed this.
                    reject_pending(ctx, phase, &mut pending, "phase tool-call budget exhausted")
                        .await?;
                    append_terminated(ctx, phase, &reason, None).await?;
                    terminated = Some(Termination::new(reason));
                    break 'phase;
                }

                messages.push(LlmMessage {
                    role: LlmRole::Assistant,
                    content: assistant,
                });

                // **Every tool_result goes into one user message.** Splitting them across
                // messages suppresses parallel tool calls (spec Section 8.4).
                let mut results: Vec<LlmContent> = Vec::with_capacity(pending.len());
                // The sixth limit of spec Section 5.4: the individual tool call timeout. The
                // table says exceeding it **ends that phase** with `ToolTimeout { tool }`.
                // The only limit the table exempts from termination is the parallel call limit, and this is not it.
                let mut timed_out: Option<String> = None;
                for (i, (tool_use_id, tool, input)) in pending.into_iter().enumerate() {
                    let (text, is_error) = if timed_out.is_some() {
                        // A timeout has already decided to end the phase. The remaining
                        // calls are not executed but **their results must still be
                        // returned** — an unpaired `tool_use` makes the next request be
                        // rejected, and an unpaired `ToolCall` step breaks the database invariant.
                        (
                            "not executed: an earlier tool call in this turn timed out".to_string(),
                            true,
                        )
                    } else if i < run {
                        match ctx.tools.call(&tool, input).await {
                            Ok(o) => (o.text, o.is_error),
                            // **Only a timeout ends the phase.** Other tool errors are
                            // answered with `is_error` and the loop continues — the model
                            // may try another approach.
                            Err(ToolError::Timeout { tool: t, seconds }) => {
                                let msg = format!("tool timed out after {seconds}s: {t}");
                                timed_out = Some(t);
                                (msg, true)
                            }
                            // Failed tools **must** be answered too — omitting one makes
                            // the API reject the next request (spec Section 8.4).
                            Err(e) => (e.to_string(), true),
                        }
                    } else {
                        (
                            format!(
                                "rejected: more than {} parallel tool calls in one turn",
                                budget.limits().max_parallel_tool_calls
                            ),
                            true,
                        )
                    };

                    // **Tool results are recorded in the summary.** A summary built only
                    // from TextBlocks would let a phase cut short by the tool call budget
                    // (structurally, a tool-heavy phase) pass an evidence-free summary
                    // forward. The full text would explode the context, so only the beginning is kept.
                    push_tool_evidence(&mut summary, &tool, is_error, &text);

                    ctx.store
                        .append_step(
                            id,
                            phase,
                            &StepKind::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                tool: tool.clone(),
                                output: text.clone(),
                                is_error,
                            },
                        )
                        .await?;

                    results.push(LlmContent::ToolResult {
                        tool_use_id,
                        content: text,
                        is_error,
                    });
                }
                let _ = reject;

                messages.push(LlmMessage {
                    role: LlmRole::User,
                    content: results,
                });

                // **A tool timeout ends the phase** (spec Section 5.4). It ends **after**
                // every result has been returned, so no unpaired call is left.
                if let Some(tool) = timed_out {
                    let reason = TerminalReason::ToolTimeout { tool };
                    append_terminated(ctx, phase, &reason, None).await?;
                    terminated = Some(Termination::new(reason));
                    break 'phase;
                }
                continue 'phase;
            }
        }
    }

    Ok(PhaseOutcome {
        summary,
        terminated,
        failed,
    })
}

/// How much of a tool's output goes into the summary.
const TOOL_EVIDENCE_CHARS: usize = 400;

fn push_tool_evidence(summary: &mut String, tool: &str, is_error: bool, output: &str) {
    if !summary.is_empty() {
        summary.push('\n');
    }
    summary.push_str("- tool ");
    summary.push_str(tool);
    summary.push_str(if is_error { " FAILED: " } else { " ok: " });
    let head: String = output.chars().take(TOOL_EVIDENCE_CHARS).collect();
    summary.push_str(head.trim());
    if output.chars().count() > TOOL_EVIDENCE_CHARS {
        summary.push_str(" …(truncated)");
    }
}

/// Leaves a refused `ToolResult` step for each `ToolCall` step that would lose its pair, and clears `pending`.
///
/// A `ToolCall` step is appended the moment it streams, so if this turn ends without
/// executing the tool, a result-less call stays in the database forever. Several
/// termination paths could violate this silently, so they are gathered into one function.
async fn reject_pending(
    ctx: &PhaseCtx<'_>,
    phase: Phase,
    pending: &mut Vec<(String, String, serde_json::Value)>,
    why: &str,
) -> Result<(), StoreError> {
    for (tool_use_id, tool, _input) in pending.drain(..) {
        ctx.store
            .append_step(
                ctx.investigation_id,
                phase,
                &StepKind::ToolResult {
                    tool_use_id,
                    tool,
                    output: format!("not executed: {why}"),
                    is_error: true,
                },
            )
            .await?;
    }
    Ok(())
}

async fn append_terminated(
    ctx: &PhaseCtx<'_>,
    phase: Phase,
    reason: &TerminalReason,
    detail: Option<String>,
) -> Result<(), StoreError> {
    ctx.store
        .append_step(
            ctx.investigation_id,
            phase,
            &StepKind::Terminated {
                reason: reason.clone(),
                detail,
            },
        )
        .await?;
    Ok(())
}

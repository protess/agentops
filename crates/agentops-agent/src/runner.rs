//! The investigation runner (spec Sections 5.3 and 6.1).
//!
//! **This is the sole writer for this investigation.** `JobManager` (plan 3) spawns this
//! function as an independent tokio task, and the HTTP request does not wait for it (INV-1).
//!
//! **All three phases always run.** A branch like "skip mitigation when there is nothing
//! to act on" requires structured output or a separate adjudication mechanism, so it is
//! not in v0.1. Instead the mitigation phase's instructions say "if there is nothing to do, state that".

use crate::limits::{Budget, Limits};
use crate::phase::{run_phase_loop, DeltaSink, PhaseCtx, Termination};
use agentops_core::{
    LlmProvider, NewArtifact, Phase, StepKind, Store, StoreError, TerminalReason, ToolRegistry,
};
use std::time::Instant;
use uuid::Uuid;

pub struct RunnerDeps<'a> {
    pub store: &'a dyn Store,
    pub provider: &'a dyn LlmProvider,
    pub tools: &'a dyn ToolRegistry,
    pub sink: &'a dyn DeltaSink,
    pub limits: Limits,
}

/// Renders the three phase summaries into a single artifact.
///
/// Why it is extracted as a pure function: one phase missing from the artifact means that
/// phase's work was thrown away, and that must be verifiable without running the whole runner.
pub fn render_artifact(carried: &[(Phase, String)]) -> NewArtifact {
    let mut body = String::new();
    for (phase, summary) in carried {
        body.push_str("## ");
        body.push_str(phase.as_str());
        body.push_str("\n\n");
        body.push_str(summary);
        body.push_str("\n\n");
    }
    NewArtifact {
        title: "Investigation report".to_string(),
        body,
    }
}

/// Runs one investigation to completion.
///
/// **Backs off when a terminal transition returns `Conflict`.** That means shutdown or
/// the watchdog terminated the investigation first, and overwriting would nullify their judgment (spec Section 6.1).
pub async fn run_investigation(deps: &RunnerDeps<'_>, id: Uuid) -> Result<(), StoreError> {
    // A Conflict arrives when it has already terminated or another party made it running.
    // In that case it is not ours to run, so we back off quietly.
    match deps.store.mark_running(id).await {
        Ok(()) => {}
        Err(StoreError::Conflict) => return Ok(()),
        Err(e) => return Err(e),
    }

    let mut budget = Budget::new(deps.limits, Instant::now());
    let mut carried: Vec<(Phase, String)> = Vec::new();
    // **This is the reader of `out.terminated`.** Six paths set that field and nobody read
    // it, which was the defect — "the runner does not read it, so there is no second
    // writer" was used as a safety argument, but the signal that field carries is **"this
    // phase could not do its job"**. Unread, an investigation ends `completed` even when
    // all three phases failed.
    let mut last_terminated: Option<TerminalReason> = None;

    for phase in Phase::INVESTIGATION_ORDER {
        // Reset the turn, tool call, and pause resumption counters at the phase boundary.
        // **The wall clock is not reset** — it is the whole-investigation limit, and
        // resetting it would restart the 30-minute limit per phase, making it 90 in effect.
        budget.reset_phase();

        let ctx = PhaseCtx {
            investigation_id: id,
            store: deps.store,
            provider: deps.provider,
            tools: deps.tools,
            sink: deps.sink,
        };
        let out = run_phase_loop(&ctx, &mut budget, phase, &carried).await?;

        if let Some(termination) = out.failed {
            return terminate_failed(deps.store, id, &termination.reason).await;
        }

        if let Some(termination) = &out.terminated {
            last_terminated = Some(termination.reason.clone());
        }
        carried.push((phase, out.summary));

        // **The single adjudication point for the wall-clock terminal row.**
        // `WallClockExceeded` is the only reason the runner re-derives independently, so
        // two writers can arise. Which of them writes is decided by **one evaluation here**:
        //
        // - exhausted: `fail_investigation` in `terminate_failed` below writes it inside
        //   the terminal transaction. Nothing is written here.
        // - not yet: the runner writes it. `phase.rs` leaves no row for this reason
        //   (because a late-firing idle timeout collides with the failure path below), so
        //   if it is not written here **nobody writes it** — that is how the state arose
        //   where all three phases hung on a stalled stream with no terminal row at all
        //
        // Checking twice (once on the phase side, once in the runner) means both write if
        // the limit passes in between. **Evaluating once and using that result in both
        // branches** removes that race.
        let wall_clock_exhausted = budget.check_wall_clock().is_err();

        if !wall_clock_exhausted {
            // For other reasons (`MaxTokens`, `TaskPanicked`, `TurnLimitExceeded`)
            // `phase.rs` is the sole writer, so writing here would duplicate.
            if let Some(Termination {
                reason: TerminalReason::WallClockExceeded,
                detail,
            }) = out.terminated
            {
                // **Pass `detail` along.** `phase.rs` carried the idle timeout's real
                // cause (for instance "stream idle timeout after Ns") in `Termination`,
                // and overwriting it with `None` here would erase that cause from the only
                // `Terminated` row this path leaves in the UI.
                deps.store
                    .append_step(
                        id,
                        phase,
                        &StepKind::Terminated {
                            reason: TerminalReason::WallClockExceeded,
                            detail,
                        },
                    )
                    .await?;
            }
        }

        // Only exceeding the wall clock fails an investigation (spec Section 5.4). Other
        // exceeded limits arrive in `out.terminated` but execution continues to the next phase.
        //
        // **When exhausted, no terminal step is written here.** `fail_investigation`
        // already writes the same `Terminated { WallClockExceeded }` row as `Phase::All`
        // inside the terminal transaction. Writing again here would (1) produce two
        // identical rows on the normal path, and (2) because that write is **outside** the
        // transaction, an intervening shutdown makes `fail_investigation` back off with
        // `Conflict` while this row remains, contradicting the termination reason another
        // party left — violating at the record layer the very rule this runner upholds:
        // never overwrite the judgment of whoever terminated first. The terminal row is owned by the terminal transaction.
        if wall_clock_exhausted {
            return terminate_failed(deps.store, id, &TerminalReason::WallClockExceeded).await;
        }
    }

    // **An investigation where no phase did its job does not end `completed`.**
    // Spec Section 5.4 makes exceeding a limit a normal termination path, but that rests
    // on the premise of proceeding *with what was collected*. If all three phases
    // terminated abnormally with nothing to carry forward, that premise collapses —
    // leaving an empty artifact as `completed` means an operator at 03:00 cannot tell a baseless report from a real investigation.
    //
    // **The criterion is the output alone.** The first version also required "no phase
    // terminated cleanly", but a single phase ending on `end_turn` with no content defeats
    // that condition — `terminated` is `None`, so it counts as a clean termination. No
    // machine failure is even needed: `ThinkingBlock` does not contribute to `summary`
    // (only text blocks and tool evidence do), so a turn that only thinks produces exactly
    // that state.
    //
    // **An empty artifact is never a useful `completed` investigation.** An empty
    // `summary` means that phase left neither a text block nor a tool call (everything
    // that passed through the `RunTools` loop, including refusals, leaves evidence). If
    // all three are like that, there is nothing to carry forward. Even the conclusion
    // "there is nothing to do" requires the model to **say** so, which produces a text block and never reaches this branch.
    //
    // A phase truncated by `MaxTokens` leaves the text it had, so it is not caught here —
    // spec Section 5.4's "proceed with the results up to this phase" is honored as written.
    if carried.iter().all(|(_, s)| s.trim().is_empty()) {
        // With every phase terminating cleanly there is no reason to carry. `TaskPanicked`
        // is the existing convention this branch uses for the unexpected (the same as the five stream failure paths).
        let reason = last_terminated.unwrap_or(TerminalReason::TaskPanicked);
        return terminate_failed(deps.store, id, &reason).await;
    }

    let artifact = render_artifact(&carried);
    match deps.store.complete_investigation(id, &artifact).await {
        Ok(_artifact_id) => Ok(()),
        // Another party terminated it first. Back off.
        Err(StoreError::Conflict) => Ok(()),
        Err(e) => Err(e),
    }
}

async fn terminate_failed(
    store: &dyn Store,
    id: Uuid,
    reason: &TerminalReason,
) -> Result<(), StoreError> {
    match store.fail_investigation(id, reason).await {
        Ok(()) => Ok(()),
        Err(StoreError::Conflict) => Ok(()),
        Err(e) => Err(e),
    }
}

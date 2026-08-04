//! `stop_reason` to next action (spec Section 8.4).
//!
//! **The branching is gathered into one pure function.** Scattered through the loop,
//! there would be no way to know where to change things when Anthropic adds a new
//! `stop_reason`, and testing would require running the whole loop.

use agentops_core::{StopReason, TerminalReason};

/// What the loop does after a turn ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// Append the partial response as an assistant turn and resend the same request.
    Continue,
    /// Execute the tools and reply with every `tool_result` in **one** user message.
    RunTools,
    /// This phase ended normally. Move to the next phase.
    PhaseDone,
    /// End this phase with a terminal step but **keep the investigation going.**
    Terminate {
        reason: TerminalReason,
        detail: Option<String>,
    },
    /// Mark the whole investigation `failed`.
    FailInvestigation { reason: TerminalReason },
}

/// `refusal_category` is `stop_details.category`. Spec Section 8.4 — `stop_details` can
/// be `null` even on a refusal, so the caller has already guarded it and passes an `Option`.
pub fn classify(reason: &StopReason, refusal_category: Option<&str>) -> TurnOutcome {
    match reason {
        // No pending tool calls means the phase ends (spec Section 5.3).
        StopReason::EndTurn => TurnOutcome::PhaseDone,

        // We never send stop_sequences, so this is effectively the same as EndTurn.
        StopReason::StopSequence => TurnOutcome::PhaseDone,

        StopReason::ToolUse => TurnOutcome::RunTools,

        // v0.1 uses no server-side tools, so this should not occur (spec Section 8.6).
        // Implemented defensively, but an occurrence is logged as a warning — it signals
        // that a server-side tool leaked in somewhere.
        StopReason::PauseTurn => TurnOutcome::Continue,

        // The output was truncated. **Never use a truncated turn in a reply** — it may
        // contain an incomplete tool_use block. Only the phase ends; execution moves to
        // the next phase and the investigation still ends completed. This is a choice not to throw away earlier phases' work.
        StopReason::MaxTokens => TurnOutcome::Terminate {
            reason: TerminalReason::MaxTokens,
            detail: Some(
                "Output was truncated. Proceeding with the results up to this phase.".to_string(),
            ),
        },

        StopReason::Refusal => TurnOutcome::FailInvestigation {
            reason: TerminalReason::Refusal {
                category: refusal_category.map(str::to_owned),
            },
        },

        StopReason::ModelContextWindowExceeded => TurnOutcome::FailInvestigation {
            reason: TerminalReason::ContextWindowExceeded,
        },

        StopReason::Unknown(s) => TurnOutcome::FailInvestigation {
            reason: TerminalReason::UnknownStopReason {
                stop_reason: s.clone(),
            },
        },
    }
}

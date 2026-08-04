use agentops_agent::outcome::{classify, TurnOutcome};
use agentops_core::{StopReason, TerminalReason};

/// TEST-5 — covers **seven** of the eight variants. `Refusal` must be seen together with
/// the presence or absence of a category, so the next test takes it. Together they complete TEST-5.
/// If even one is missing, nobody knows what the agent does the day that stop_reason arrives.
#[test]
fn test_5_every_stop_reason_maps_to_an_outcome() {
    assert_eq!(classify(&StopReason::EndTurn, None), TurnOutcome::PhaseDone);
    assert_eq!(classify(&StopReason::ToolUse, None), TurnOutcome::RunTools);
    assert_eq!(
        classify(&StopReason::StopSequence, None),
        TurnOutcome::PhaseDone
    );
    assert_eq!(
        classify(&StopReason::PauseTurn, None),
        TurnOutcome::Continue
    );

    // **The detail string is not pinned byte for byte.** Spec Section 8.4 requires only
    // "a clear message" and specifies no wording, so an edit that polishes it must not
    // look like a regression. Only the variant shape and the presence of detail are asserted.
    match classify(&StopReason::MaxTokens, None) {
        TurnOutcome::Terminate { reason, detail } => {
            assert_eq!(reason, TerminalReason::MaxTokens);
            assert!(
                detail.is_some(),
                "there must be wording that tells the UI it was truncated"
            );
        }
        other => panic!("expected Terminate, got {other:?}"),
    }

    assert_eq!(
        classify(&StopReason::ModelContextWindowExceeded, None),
        TurnOutcome::FailInvestigation {
            reason: TerminalReason::ContextWindowExceeded
        }
    );

    assert_eq!(
        classify(&StopReason::Unknown("brand_new".into()), None),
        TurnOutcome::FailInvestigation {
            reason: TerminalReason::UnknownStopReason {
                stop_reason: "brand_new".into()
            }
        }
    );
}

/// TEST-5 — a refusal's category may be absent. Spec Section 8.4:
/// "stop_details can be null even on a refusal, so guard before reading .category."
/// Unwrapping here would kill the process on every refusal.
#[test]
fn test_5_refusal_tolerates_a_missing_category() {
    assert_eq!(
        classify(&StopReason::Refusal, None),
        TurnOutcome::FailInvestigation {
            reason: TerminalReason::Refusal { category: None }
        }
    );
    assert_eq!(
        classify(&StopReason::Refusal, Some("cyber")),
        TurnOutcome::FailInvestigation {
            reason: TerminalReason::Refusal {
                category: Some("cyber".into())
            }
        }
    );
}

/// MaxTokens does not fail the investigation — it ends only the phase.
/// A deliberate choice not to throw away earlier phases' work (spec Section 8.4).
#[test]
fn max_tokens_terminates_the_phase_but_not_the_investigation() {
    let got = classify(&StopReason::MaxTokens, None);
    assert!(
        matches!(got, TurnOutcome::Terminate { .. }),
        "FailInvestigation would discard the earlier phases' results: {got:?}"
    );
}

/// Exactly three things fail an investigation. Adding MaxTokens or TurnLimitExceeded
/// here would violate Section 5.4, where exceeding a limit is a normal termination path.
#[test]
fn exactly_three_reasons_fail_the_investigation() {
    let failing: Vec<StopReason> = [
        StopReason::EndTurn,
        StopReason::ToolUse,
        StopReason::MaxTokens,
        StopReason::PauseTurn,
        StopReason::Refusal,
        StopReason::StopSequence,
        StopReason::ModelContextWindowExceeded,
        StopReason::Unknown("x".into()),
    ]
    .into_iter()
    .filter(|r| matches!(classify(r, None), TurnOutcome::FailInvestigation { .. }))
    .collect();

    assert_eq!(
        failing,
        vec![
            StopReason::Refusal,
            StopReason::ModelContextWindowExceeded,
            StopReason::Unknown("x".into()),
        ],
        "the set of reasons that fail an investigation has changed"
    );
}

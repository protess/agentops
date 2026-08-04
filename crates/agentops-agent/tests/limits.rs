use agentops_agent::limits::{cap_parallel, Budget, Limits};
use agentops_core::TerminalReason;
use std::time::{Duration, Instant};

/// The same 60 seconds lives in both `Limits::default().tool_call_timeout` and Task 8's
/// `mcp::TOOL_CALL_TIMEOUT`. Changing one alone diverges silently, so their agreement is
/// asserted.
///
/// **This test lives in Task 10.** Task 8 defines only `TOOL_CALL_TIMEOUT` and knows
/// nothing of `Limits` — when an earlier task imports a later task's module, that task
/// cannot compile standalone. The later task checking agreement with the earlier constant
/// is the correct dependency direction.
#[test]
fn tool_call_timeout_matches_the_mcp_constant() {
    assert_eq!(
        Limits::default().tool_call_timeout,
        agentops_agent::mcp::TOOL_CALL_TIMEOUT,
        "the two defaults diverged — the basis for the value plan 3 injects is unstable"
    );
}

/// The defaults from spec Section 5.4. A change must be visible — a silent relaxation
/// lets the agent call tools forever.
#[test]
fn defaults_match_the_spec_table() {
    let l = Limits::default();
    assert_eq!(l.max_turns_per_phase, 40);
    assert_eq!(l.max_tool_calls_per_phase, 120);
    assert_eq!(l.max_parallel_tool_calls, 16);
    assert_eq!(l.wall_clock, Duration::from_secs(30 * 60));
    assert_eq!(l.tool_call_timeout, Duration::from_secs(60));
    assert_eq!(l.stream_idle_timeout, Duration::from_secs(120));
    assert_eq!(l.max_pause_turn_resumes, 5);
}

/// TEST-15 — the turn limit must fire.
#[test]
fn test_15_turn_limit_fires() {
    let l = Limits {
        max_turns_per_phase: 3,
        ..Default::default()
    };
    let mut b = Budget::new(l, Instant::now());
    for i in 0..3 {
        b.charge_turn()
            .unwrap_or_else(|e| panic!("turn {i} rejected: {e:?}"));
    }
    assert_eq!(b.charge_turn(), Err(TerminalReason::TurnLimitExceeded));
}

/// TEST-15 — the tool call limit must fire. It is a counter **separate** from turns.
#[test]
fn test_15_tool_call_limit_fires_independently_of_turns() {
    let l = Limits {
        max_turns_per_phase: 1000,
        max_tool_calls_per_phase: 5,
        ..Default::default()
    };
    let mut b = Budget::new(l, Instant::now());
    b.charge_tool_calls(3).unwrap();
    b.charge_tool_calls(2).unwrap();
    assert_eq!(
        b.charge_tool_calls(1),
        Err(TerminalReason::TurnLimitExceeded),
        "the tool call limit was masked by the turn limit"
    );
}

/// TEST-15 — the wall-clock limit must fire.
#[test]
fn test_15_wall_clock_limit_fires() {
    let l = Limits {
        wall_clock: Duration::from_millis(0),
        ..Default::default()
    };
    // Give a past start instant so nothing actually waits.
    let b = Budget::new(l, Instant::now() - Duration::from_secs(1));
    assert_eq!(b.check_wall_clock(), Err(TerminalReason::WallClockExceeded));
}

#[test]
fn wall_clock_is_ok_before_the_deadline() {
    let b = Budget::new(Limits::default(), Instant::now());
    assert_eq!(b.check_wall_clock(), Ok(()));
}

/// TEST-15 — the PauseTurn resumption limit must fire.
#[test]
fn test_15_pause_turn_resume_limit_fires() {
    let l = Limits {
        max_pause_turn_resumes: 2,
        ..Default::default()
    };
    let mut b = Budget::new(l, Instant::now());
    b.charge_pause_resume().unwrap();
    b.charge_pause_resume().unwrap();
    assert_eq!(
        b.charge_pause_resume(),
        Err(TerminalReason::TurnLimitExceeded)
    );
}

/// The parallel tool call limit **does not end the phase** — only the excess is answered
/// with is_error (the third row of the spec Section 5.4 table). Emitting Terminate here
/// would let a healthy tool-heavy turn kill the phase.
#[test]
fn parallel_cap_rejects_the_excess_without_terminating() {
    let l = Limits {
        max_parallel_tool_calls: 16,
        ..Default::default()
    };
    assert_eq!(cap_parallel(3, &l), (3, 0));
    assert_eq!(cap_parallel(16, &l), (16, 0));
    assert_eq!(cap_parallel(20, &l), (16, 4));
}

/// It must keep going while under the limit — a limit of 0 or an off-by-one would kill
/// it on the first turn.
#[test]
fn default_limits_allow_a_normal_phase_to_run() {
    let mut b = Budget::new(Limits::default(), Instant::now());
    for _ in 0..40 {
        b.charge_turn()
            .expect("40 turns is within the default limit");
    }
    assert!(b.charge_turn().is_err(), "the 41st must be blocked");
}

/// On a phase change the turn and tool counters reset while the wall clock persists.
/// The wall clock is the **whole-investigation** limit (spec Section 5.4).
#[test]
fn phase_reset_clears_per_phase_counters_but_not_the_clock() {
    let l = Limits {
        max_turns_per_phase: 2,
        // A limit already passed. If `reset_phase` touched `started`, time would come
        // back and the assertion below would flip to Ok — which is what this test catches.
        wall_clock: Duration::from_millis(50),
        ..Default::default()
    };
    // Start from an instant already past the limit. Nothing actually waits.
    let mut b = Budget::new(l, Instant::now() - Duration::from_millis(200));

    b.charge_turn().unwrap();
    b.charge_turn().unwrap();
    assert!(
        b.charge_turn().is_err(),
        "the per-phase turn limit must fire"
    );
    assert_eq!(
        b.check_wall_clock(),
        Err(TerminalReason::WallClockExceeded),
        "the start instant is already past the limit"
    );

    b.reset_phase();

    b.charge_turn()
        .expect("the turn counter must reset in a new phase");
    assert_eq!(
        b.check_wall_clock(),
        Err(TerminalReason::WallClockExceeded),
        "reset_phase reset the wall clock — the whole-investigation limit is disabled"
    );
}

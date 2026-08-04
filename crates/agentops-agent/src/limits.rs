//! Loop limits (spec Section 5.4).
//!
//! **All seven limits are required.** Limiting only `PauseTurn` resumption and leaving
//! the rest open lets the agent call tools forever.
//!
//! **Exceeding a limit is a normal termination path, not a failure.** The phase summary
//! is built from what was collected and execution moves to the next phase. Only exceeding
//! the whole-investigation wall clock marks it `failed` — a judgment `crate::runner` makes.
//!
//! **All counters are in-memory.** v0.1 does not support resuming an investigation, so
//! they need not survive a process restart.

use agentops_core::TerminalReason;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_turns_per_phase: u32,
    pub max_tool_calls_per_phase: u32,
    pub max_parallel_tool_calls: usize,
    pub wall_clock: Duration,
    pub tool_call_timeout: Duration,
    pub stream_idle_timeout: Duration,
    pub max_pause_turn_resumes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_turns_per_phase: 40,
            max_tool_calls_per_phase: 120,
            max_parallel_tool_calls: 16,
            wall_clock: Duration::from_secs(30 * 60),
            tool_call_timeout: Duration::from_secs(60),
            stream_idle_timeout: Duration::from_secs(120),
            max_pause_turn_resumes: 5,
        }
    }
}

/// One investigation's budget. The turn and tool counters reset per phase; the wall clock
/// persists across the whole investigation.
#[derive(Debug)]
pub struct Budget {
    limits: Limits,
    started: Instant,
    turns: u32,
    tool_calls: u32,
    pause_resumes: u32,
}

impl Budget {
    /// `started` is injected so a test can pass a past instant and exercise the wall-clock
    /// path without actually waiting.
    pub fn new(limits: Limits, started: Instant) -> Self {
        Self {
            limits,
            started,
            turns: 0,
            tool_calls: 0,
            pause_resumes: 0,
        }
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Called at a phase boundary. **The wall clock is not reset** — it is the whole-investigation limit.
    pub fn reset_phase(&mut self) {
        self.turns = 0;
        self.tool_calls = 0;
        self.pause_resumes = 0;
    }

    pub fn charge_turn(&mut self) -> Result<(), TerminalReason> {
        if self.turns >= self.limits.max_turns_per_phase {
            return Err(TerminalReason::TurnLimitExceeded);
        }
        self.turns += 1;
        Ok(())
    }

    /// Tool calls have **a counter separate from turns.** One turn can call several tools,
    /// so a turn limit alone does not stop a tool-call runaway.
    pub fn charge_tool_calls(&mut self, n: u32) -> Result<(), TerminalReason> {
        if self.tool_calls + n > self.limits.max_tool_calls_per_phase {
            return Err(TerminalReason::TurnLimitExceeded);
        }
        self.tool_calls += n;
        Ok(())
    }

    pub fn charge_pause_resume(&mut self) -> Result<(), TerminalReason> {
        if self.pause_resumes >= self.limits.max_pause_turn_resumes {
            return Err(TerminalReason::TurnLimitExceeded);
        }
        self.pause_resumes += 1;
        Ok(())
    }

    pub fn check_wall_clock(&self) -> Result<(), TerminalReason> {
        if self.started.elapsed() >= self.limits.wall_clock {
            return Err(TerminalReason::WallClockExceeded);
        }
        Ok(())
    }
}

/// The per-turn parallel tool call limit. Returns `(how many to run, how many to reject)`.
///
/// **Exceeding it does not end the phase** — only the excess is answered with `is_error`
/// (spec Section 5.4). Terminating here would let a healthy tool-heavy turn kill the phase.
pub fn cap_parallel(calls: usize, limits: &Limits) -> (usize, usize) {
    let run = calls.min(limits.max_parallel_tool_calls);
    (run, calls - run)
}

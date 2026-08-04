use serde::{Deserialize, Serialize};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// Schema version of the `agent_steps.payload` JSONB. Bump it when the shape changes.
pub const STEP_PAYLOAD_VERSION: i32 = 1;

/// Investigation phase. The same set as the scope axis of `instructions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    All,
    Chat,
    Triage,
    Rca,
    Mitigation,
}

impl Phase {
    /// The phases an investigation passes through in order. The fixed sequential sub-loop of design spec Section 5.3.
    pub const INVESTIGATION_ORDER: [Phase; 3] = [Phase::Triage, Phase::Rca, Phase::Mitigation];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Chat => "chat",
            Self::Triage => "triage",
            Self::Rca => "rca",
            Self::Mitigation => "mitigation",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown phase: {0}")]
pub struct ParsePhaseError(String);

impl FromStr for Phase {
    type Err = ParsePhaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "chat" => Ok(Self::Chat),
            "triage" => Ok(Self::Triage),
            "rca" => Ok(Self::Rca),
            "mitigation" => Ok(Self::Mitigation),
            other => Err(ParsePhaseError(other.to_owned())),
        }
    }
}

/// Why a phase or an investigation ended. Without structure, values like
/// `stop_details.category` end up crammed into a string (design spec, Section 5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum TerminalReason {
    Refusal {
        category: Option<String>,
    },
    ContextWindowExceeded,
    MaxTokens,
    TurnLimitExceeded,
    WallClockExceeded,
    ToolTimeout {
        tool: String,
    },
    ShutdownRequested,
    TaskPanicked,
    /// A dependency this phase needs was unavailable. Not a panic —
    /// something **external and recoverable**, such as the policy store or an MCP server.
    DependencyUnavailable {
        what: String,
    },
    /// **Must be a struct variant.** A newtype variant such as
    /// `UnknownStopReason(String)` cannot be serialized by an internally-tagged
    /// (`tag = "reason"`) enum — serde returns `cannot serialize tagged newtype
    /// variant ... containing a string` and `payload_json()`'s `.expect` panics.
    /// Plan 1's tests never take this path, but plan 2's agent loop does the moment
    /// an unknown `stop_reason` arrives — which is this variant's entire reason to exist.
    UnknownStopReason {
        stop_reason: String,
    },
}

/// A durable event produced by the agent loop. Not produced per delta, but only
/// at semantic boundaries (design spec, Section 6.1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StepKind {
    Thinking {
        summary: String,
    },
    Text {
        text: String,
    },
    /// `tool_use_id` is the ID Anthropic issued. A single turn can carry several
    /// parallel tool calls, and without this there is no way to pair a call with its result.
    ToolCall {
        tool_use_id: String,
        tool: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        tool: String,
        output: String,
        is_error: bool,
    },
    ArtifactWritten {
        artifact_id: Uuid,
    },
    Terminated {
        reason: TerminalReason,
        detail: Option<String>,
    },
    Error {
        message: String,
    },
}

impl StepKind {
    /// The value of the database's `agent_steps.kind` column. The same set as the CHECK constraint.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Thinking { .. } => "thinking",
            Self::Text { .. } => "text",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::ArtifactWritten { .. } => "artifact",
            Self::Terminated { .. } => "terminated",
            Self::Error { .. } => "error",
        }
    }

    /// The key pairing a tool call with its result.
    pub fn tool_use_id(&self) -> Option<&str> {
        match self {
            Self::ToolCall { tool_use_id, .. } | Self::ToolResult { tool_use_id, .. } => {
                Some(tool_use_id)
            }
            _ => None,
        }
    }

    /// Restores from the JSONB payload.
    pub fn from_payload_json(v: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(v.clone())
    }
}

/// One row of an investigation's durable event log. `seq` increases monotonically within an investigation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStep {
    pub investigation_id: Uuid,
    pub seq: i64,
    pub phase: Phase,
    pub kind: StepKind,
    pub created_at: OffsetDateTime,
}

impl AgentStep {
    /// The JSON stored in `agent_steps.payload`. Includes the version field.
    pub fn payload_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(&self.kind).expect("StepKind is always serializable");
        if let serde_json::Value::Object(map) = &mut v {
            map.insert("v".into(), serde_json::json!(STEP_PAYLOAD_VERSION));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn phase_round_trips() {
        for p in [
            Phase::All,
            Phase::Chat,
            Phase::Triage,
            Phase::Rca,
            Phase::Mitigation,
        ] {
            assert_eq!(p.as_str().parse::<Phase>().unwrap(), p);
        }
    }

    #[test]
    fn investigation_order_is_triage_rca_mitigation() {
        assert_eq!(
            Phase::INVESTIGATION_ORDER,
            [Phase::Triage, Phase::Rca, Phase::Mitigation]
        );
    }

    #[test]
    fn tool_call_carries_tool_use_id() {
        let k = StepKind::ToolCall {
            tool_use_id: "toolu_abc".into(),
            tool: "prom__query".into(),
            input: serde_json::json!({"q": "up"}),
        };
        assert_eq!(k.kind_str(), "tool_call");
        assert_eq!(k.tool_use_id(), Some("toolu_abc"));
    }

    #[test]
    fn tool_result_pairs_with_same_id() {
        let r = StepKind::ToolResult {
            tool_use_id: "toolu_abc".into(),
            tool: "prom__query".into(),
            output: "1".into(),
            is_error: false,
        };
        assert_eq!(r.kind_str(), "tool_result");
        assert_eq!(r.tool_use_id(), Some("toolu_abc"));
    }

    #[test]
    fn non_tool_kinds_have_no_tool_use_id() {
        assert_eq!(StepKind::Text { text: "hi".into() }.tool_use_id(), None);
    }

    #[test]
    fn terminated_kind_carries_structured_reason() {
        let k = StepKind::Terminated {
            reason: TerminalReason::Refusal {
                category: Some("cyber".into()),
            },
            detail: None,
        };
        assert_eq!(k.kind_str(), "terminated");
    }

    /// Confirms every TerminalReason variant serializes. An internally-tagged enum
    /// cannot serialize a newtype variant wrapping a string, so without this test
    /// it would be discovered as a runtime panic in plan 2.
    #[test]
    fn every_terminal_reason_serializes() {
        let reasons = [
            TerminalReason::Refusal {
                category: Some("cyber".into()),
            },
            TerminalReason::Refusal { category: None },
            TerminalReason::ContextWindowExceeded,
            TerminalReason::MaxTokens,
            TerminalReason::TurnLimitExceeded,
            TerminalReason::WallClockExceeded,
            TerminalReason::ToolTimeout {
                tool: "prom__query".into(),
            },
            TerminalReason::ShutdownRequested,
            TerminalReason::TaskPanicked,
            TerminalReason::DependencyUnavailable {
                what: "tool policy store".into(),
            },
            TerminalReason::UnknownStopReason {
                stop_reason: "future_variant".into(),
            },
        ];
        for r in reasons {
            let step = AgentStep {
                investigation_id: Uuid::nil(),
                seq: 0,
                phase: Phase::All,
                kind: StepKind::Terminated {
                    reason: r.clone(),
                    detail: None,
                },
                created_at: time::OffsetDateTime::UNIX_EPOCH,
            };
            // payload_json uses expect internally, so a non-serializable variant panics
            let payload = step.payload_json();
            let back = StepKind::from_payload_json(&payload).unwrap();
            assert_eq!(
                back,
                StepKind::Terminated {
                    reason: r,
                    detail: None
                }
            );
        }
    }

    /// The payload carries a version field — matching the database's `payload ? 'v'` CHECK.
    #[test]
    fn payload_serialization_includes_version() {
        let step = AgentStep {
            investigation_id: Uuid::nil(),
            seq: 0,
            phase: Phase::Triage,
            kind: StepKind::Text {
                text: "hello".into(),
            },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let payload = step.payload_json();
        assert_eq!(payload["v"], STEP_PAYLOAD_VERSION);
        assert_eq!(payload["text"], "hello");
    }

    #[test]
    fn payload_round_trips() {
        let kind = StepKind::ToolCall {
            tool_use_id: "toolu_1".into(),
            tool: "t".into(),
            input: serde_json::json!({"a": 1}),
        };
        let step = AgentStep {
            investigation_id: Uuid::nil(),
            seq: 3,
            phase: Phase::Rca,
            kind: kind.clone(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let payload = step.payload_json();
        assert_eq!(StepKind::from_payload_json(&payload).unwrap(), kind);
    }
}

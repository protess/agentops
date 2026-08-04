//! The LLM provider protocol. **Our own canonical representation, not the wire format.**
//!
//! Spec Section 5.2 — these types cross `.await` boundaries and pass through
//! `Arc<dyn LlmProvider>`, so they are defined as owned types with no borrows and live
//! in core. Anthropic's own JSON shapes exist only inside `agentops-agent`.

use crate::{BoxStream, LlmError, ToolDef};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// One LLM round-trip request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRequest {
    /// The assembled system prompt. **Must be byte-identical across requests** (spec Section 8.3).
    pub system: String,
    pub messages: Vec<LlmMessage>,
    /// Must be sorted by namespaced name (spec Section 9).
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmRole {
    User,
    Assistant,
}

impl LlmRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub content: Vec<LlmContent>,
}

impl LlmMessage {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: LlmRole::User,
            content: vec![LlmContent::Text { text: text.into() }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Failed tools must be answered too — omitting one makes the API reject the next request (spec Section 8.4).
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// An event normalized from the stream.
///
/// **The distinction between `*Delta` and `*Block` is spec Section 6.1.3.** A delta is a
/// transient fragment streamed to the browser and never written to the database. A block is
/// a completed semantic unit and becomes a step. Calling `append_step` per delta would make one investigation tens of thousands of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmEvent {
    ThinkingDelta {
        text: String,
    },
    TextDelta {
        text: String,
    },
    ThinkingBlock {
        summary: String,
    },
    TextBlock {
        text: String,
    },
    ToolCall {
        tool_use_id: String,
        tool: String,
        input: serde_json::Value,
    },
    Stopped {
        reason: StopReason,
        usage: Usage,
        /// `stop_details.category`. **Can be `null` even on a `refusal`** (spec Section 8.4)
        /// — hence the `Option`, and it must be guarded before reading. Without this value,
        /// `TerminalReason::Refusal { category }` is always `None` and an operator cannot
        /// tell why the refusal happened. Opus 5's safeguards can produce false positives on
        /// SRE and security-adjacent work (Section 8.4), which makes this especially important here.
        refusal_category: Option<String>,
    },
    /// A mid-stream `error` event. It terminates that phase (spec Section 8.5).
    StreamError {
        message: String,
    },
}

/// `stop_reason`. **`Unknown` preserves the raw value.**
///
/// `#[serde(other)]` accepts only a unit variant and cannot carry the raw value, so unknown
/// values are handled by a custom `Deserialize` rather than an `untagged` fallback — losing
/// the raw value leaves nothing to put in `TerminalReason::UnknownStopReason { stop_reason }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    PauseTurn,
    Refusal,
    StopSequence,
    ModelContextWindowExceeded,
    Unknown(String),
}

impl StopReason {
    /// The wire representation. Must be exactly symmetric with `Deserialize`.
    pub fn as_wire(&self) -> &str {
        match self {
            Self::EndTurn => "end_turn",
            Self::ToolUse => "tool_use",
            Self::MaxTokens => "max_tokens",
            Self::PauseTurn => "pause_turn",
            Self::Refusal => "refusal",
            Self::StopSequence => "stop_sequence",
            Self::ModelContextWindowExceeded => "model_context_window_exceeded",
            Self::Unknown(s) => s,
        }
    }
}

impl<'de> Deserialize<'de> for StopReason {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "end_turn" => Self::EndTurn,
            "tool_use" => Self::ToolUse,
            "max_tokens" => Self::MaxTokens,
            "pause_turn" => Self::PauseTurn,
            "refusal" => Self::Refusal,
            "stop_sequence" => Self::StopSequence,
            "model_context_window_exceeded" => Self::ModelContextWindowExceeded,
            _ => Self::Unknown(s),
        })
    }
}

/// **Do not use `derive(Serialize)`.** External tagging turns a newtype variant into
/// `{"unknown": "..."}`, which disagrees with the bare string the hand-written
/// `Deserialize` accepts — leaving the type unable to deserialize what it serialized.
impl Serialize for StopReason {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_wire())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
}

/// Streaming only. Investigations run for a long time, so there is no non-streaming path (spec Section 5.2).
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn model_id(&self) -> &str;

    /// **A stream can end without `Stopped`.** A 200 response with an empty body or a
    /// non-SSE body ends quietly with neither an event nor an error — a misconfigured proxy,
    /// a wrong content-type, and a cleanly dropped connection all do this. The parser discards
    /// bytes that are not `event:`/`data:`, so it cannot distinguish this from "a healthy stream
    /// whose `Stopped` has not arrived yet".
    ///
    /// **The consumer must treat a stream exhausted without `Stopped` as an error.**
    /// The provider does not synthesize an error because the context that judgment needs
    /// (which investigation, which phase, how to record a step) exists only at the caller.
    async fn stream(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All eight variants must round-trip **in both directions**. Checking only one
    /// direction lets a state pass where `Unknown` serializes as `{"unknown": ...}` and
    /// cannot deserialize itself — which is exactly how it did pass.
    #[test]
    fn every_stop_reason_round_trips_in_both_directions() {
        let all = [
            ("end_turn", StopReason::EndTurn),
            ("tool_use", StopReason::ToolUse),
            ("max_tokens", StopReason::MaxTokens),
            ("pause_turn", StopReason::PauseTurn),
            ("refusal", StopReason::Refusal),
            ("stop_sequence", StopReason::StopSequence),
            (
                "model_context_window_exceeded",
                StopReason::ModelContextWindowExceeded,
            ),
            (
                "some_future_reason",
                StopReason::Unknown("some_future_reason".into()),
            ),
        ];
        for (wire, want) in all {
            let json = format!("\"{wire}\"");

            let got: StopReason = serde_json::from_str(&json).expect(wire);
            assert_eq!(got, want, "deserialize {wire}");

            let out = serde_json::to_string(&want).expect(wire);
            assert_eq!(out, json, "serialize {wire} must produce a bare string");

            let back: StopReason = serde_json::from_str(&out).expect(wire);
            assert_eq!(back, want, "round-trip {wire}");
        }
    }

    /// Protocol types cross `.await` boundaries, so they must be Send + 'static (spec Section 5.2).
    /// Compilation itself is the test, not a runtime assertion.
    #[test]
    fn protocol_types_are_send_static() {
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<LlmRequest>();
        assert_send_static::<LlmEvent>();
        assert_send_static::<StopReason>();
        assert_send_static::<Usage>();
    }

    #[test]
    fn llm_provider_is_object_safe() {
        fn assert_provider(_: std::sync::Arc<dyn LlmProvider>) {}
        let _ = assert_provider;
    }
}

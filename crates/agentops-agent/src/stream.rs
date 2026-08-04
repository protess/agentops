//! The Anthropic SSE to `LlmEvent` state machine (spec Section 8.4).
//!
//! Two rules are load-bearing.
//!
//! 1. **Never access blocks by index.** No `content[0]`. Anthropic's `index` can be
//!    sparse and guarantees no ordering, so they are held in a map.
//! 2. **`stop_reason` does not arrive until the `message_delta` at the end of the
//!    stream.** "Check stop_reason before reading content" is a non-streaming way of
//!    thinking and does not hold here. Deciding termination earlier misreads partial output.
//! 3. **`phase` is used only by `is_done()` and duplicate `message_delta` detection.**
//!    Every other branch acts on the event name alone and enforces no ordering — a state
//!    check would instead reject healthy frames when receiving from mid-stream. The state
//!    diagram is documentation; only these two points are enforced.

use crate::sse::SseFrame;
use agentops_core::{LlmError, LlmEvent, StopReason, Usage};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    Text {
        text: String,
    },
    Thinking {
        summary: String,
    },
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    /// A block type we do not know. Deltas are discarded and nothing is emitted on completion.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Streaming,
    Finalizing,
    Done,
}

#[derive(Debug)]
pub struct StreamMachine {
    phase: Phase,
    blocks: HashMap<u32, Block>,
    usage: Usage,
}

impl Default for StreamMachine {
    fn default() -> Self {
        Self::new()
    }
}

fn bad(e: impl std::fmt::Display) -> LlmError {
    LlmError::MalformedEvent(e.to_string())
}

impl StreamMachine {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            blocks: HashMap::new(),
            usage: Usage::default(),
        }
    }

    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Feed one frame and receive the events it produced.
    ///
    /// Unknown event names are discarded silently — spec Section 8.5. **Broken JSON in a
    /// known event, by contrast, is raised as an error.** Discarding that too would make a
    /// half-processed stream look successful.
    pub fn feed(&mut self, frame: &SseFrame) -> Result<Vec<LlmEvent>, LlmError> {
        match frame.event.as_str() {
            "message_start" => {
                let v: Value = serde_json::from_str(&frame.data).map_err(bad)?;
                if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                    self.usage = serde_json::from_value(u.clone()).map_err(bad)?;
                }
                self.phase = Phase::Streaming;
                Ok(vec![])
            }

            "content_block_start" => {
                let v: Value = serde_json::from_str(&frame.data).map_err(bad)?;
                let idx = block_index(&v)?;
                let cb = v
                    .get("content_block")
                    .ok_or_else(|| bad("no content_block"))?;
                let block = match cb.get("type").and_then(Value::as_str) {
                    Some("text") => Block::Text {
                        text: String::new(),
                    },
                    Some("thinking") => Block::Thinking {
                        summary: String::new(),
                    },
                    Some("tool_use") => Block::ToolUse {
                        id: str_field(cb, "id")?,
                        name: str_field(cb, "name")?,
                        json: String::new(),
                    },
                    _ => Block::Unknown,
                };
                self.blocks.insert(idx, block);
                Ok(vec![])
            }

            "content_block_delta" => {
                let v: Value = serde_json::from_str(&frame.data).map_err(bad)?;
                let idx = block_index(&v)?;
                let delta = v.get("delta").ok_or_else(|| bad("no delta"))?;
                // A delta for a block whose start event was missed is discarded. Making it
                // an error would kill the whole stream when receiving from mid-reconnect.
                let Some(block) = self.blocks.get_mut(&idx) else {
                    return Ok(vec![]);
                };
                match (block, delta.get("type").and_then(Value::as_str)) {
                    (Block::Text { text }, Some("text_delta")) => {
                        let d = str_field(delta, "text")?;
                        text.push_str(&d);
                        Ok(vec![LlmEvent::TextDelta { text: d }])
                    }
                    (Block::Thinking { summary }, Some("thinking_delta")) => {
                        let d = str_field(delta, "thinking")?;
                        summary.push_str(&d);
                        Ok(vec![LlmEvent::ThinkingDelta { text: d }])
                    }
                    (Block::ToolUse { json, .. }, Some("input_json_delta")) => {
                        json.push_str(&str_field(delta, "partial_json")?);
                        // Partial JSON is not streamed out — the browser cannot use it.
                        Ok(vec![])
                    }
                    // A delta we do not use, such as signature_delta, or a mismatch between
                    // the block type and the delta type. Discarded.
                    _ => Ok(vec![]),
                }
            }

            "content_block_stop" => {
                let v: Value = serde_json::from_str(&frame.data).map_err(bad)?;
                let idx = block_index(&v)?;
                // **Unlike deltas, this is not lenient.** Discarding one delta loses only
                // intermediate text that will soon be replaced, but discarding a `stop`
                // makes an entire completed semantic unit vanish — had that block been a
                // `tool_use`, the tool call would disappear without a trace and the agent
                // would carry on as though the model never requested it. This design has no
                // mid-stream resumption (Task 5 opens a fresh POST per turn), so a `stop` with no `start` is a protocol violation or a parser bug.
                let Some(block) = self.blocks.remove(&idx) else {
                    return Err(bad(format!(
                        "content_block_stop for unknown block index {idx}"
                    )));
                };
                Ok(match block {
                    Block::Text { text } => vec![LlmEvent::TextBlock { text }],
                    Block::Thinking { summary } => vec![LlmEvent::ThinkingBlock { summary }],
                    Block::ToolUse { id, name, json } => {
                        // A tool with no arguments sends no partial_json at all.
                        let input = if json.trim().is_empty() {
                            Value::Object(Default::default())
                        } else {
                            serde_json::from_str(&json).map_err(bad)?
                        };
                        vec![LlmEvent::ToolCall {
                            tool_use_id: id,
                            tool: name,
                            input,
                        }]
                    }
                    Block::Unknown => vec![],
                })
            }

            "message_delta" => {
                // `stop_reason` is determined once. A second `message_delta` would re-emit
                // `Stopped`, and a consumer taking the last one would let the wrong
                // termination reason win.
                if self.phase != Phase::Streaming {
                    return Err(bad("message_delta arrived twice"));
                }
                let v: Value = serde_json::from_str(&frame.data).map_err(bad)?;
                if let Some(u) = v.get("usage") {
                    // The usage in message_delta is a partial update. Only output_tokens arrives.
                    let partial: Usage = serde_json::from_value(u.clone()).map_err(bad)?;
                    if partial.output_tokens > 0 {
                        self.usage.output_tokens = partial.output_tokens;
                    }
                }
                let reason: StopReason = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .filter(|r| !r.is_null())
                    .map(|r| serde_json::from_value(r.clone()))
                    .transpose()
                    .map_err(bad)?
                    .ok_or_else(|| bad("message_delta has no stop_reason"))?;
                // **Both locations are read.** The documentation describes `stop_details`
                // as a top-level field of a non-streaming Message (a sibling of
                // `stop_reason`), with no example placing it inside an SSE `message_delta`.
                // Which it is cannot be settled, so both are read — betting on one means
                // that if it is wrong, `category` is permanently `None` and **nothing
                // fails.** `category` and `explanation` can be null even on a refusal (stated in the documentation).
                let find_category = |base: &Value| -> Option<String> {
                    base.get("stop_details")
                        .and_then(|sd| sd.get("category"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                let refusal_category = v
                    .get("delta")
                    .and_then(find_category)
                    .or_else(|| find_category(&v));
                self.phase = Phase::Finalizing;
                Ok(vec![LlmEvent::Stopped {
                    reason,
                    usage: self.usage,
                    refusal_category,
                }])
            }

            "message_stop" => {
                self.phase = Phase::Done;
                Ok(vec![])
            }

            "error" => {
                let v: Value = serde_json::from_str(&frame.data).map_err(bad)?;
                let e = v.get("error");
                let kind = e
                    .and_then(|e| e.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                let msg = e
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                self.phase = Phase::Done;
                Ok(vec![LlmEvent::StreamError {
                    message: format!("{kind}: {msg}"),
                }])
            }

            // Pings and every event we do not know. Spec Section 8.5 — failing here kills
            // the stream at the first keepalive.
            _ => Ok(vec![]),
        }
    }
}

fn block_index(v: &Value) -> Result<u32, LlmError> {
    v.get("index")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .ok_or_else(|| bad("no index"))
}

fn str_field(v: &Value, key: &str) -> Result<String, LlmError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| bad(format!("no {key}")))
}

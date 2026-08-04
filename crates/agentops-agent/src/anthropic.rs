//! The Anthropic Messages API provider (spec Sections 8.2 through 8.5).
//!
//! There is no official Rust SDK, so it calls through `reqwest` directly. All three
//! providers share the same wire format, so the differences are confined to the endpoint,
//! the authentication, and the model ID prefix; v0.2's AWS providers copy this file and change only those three (spec Section 8.1).

use crate::sse::SseParser;
use crate::stream::StreamMachine;
use crate::ApiKey;
use agentops_core::{BoxStream, LlmContent, LlmError, LlmEvent, LlmProvider, LlmRequest};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_MODEL: &str = "claude-opus-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The ceiling on raw bytes one stream may read from the socket.
///
/// Task 2's `SseParser` grows its internal buffer without bound until it sees a newline —
/// if the server never sends one, memory grows forever. This ceiling is that defense.
///
/// Chosen against a `max_tokens` of 32000: at roughly 4 bytes per token in English the
/// pure text is only ~128KB, but SSE repeats a JSON skeleton per delta (`event: ...\ndata:
/// {"type":...,"index":...,"delta":{...}}\n\n`), so the real byte count is far larger. In
/// the pathological worst case — the model emitting one delta per token (32000 of them,
/// ~100 bytes of overhead each) — the overhead alone is ~3.2MB. 16MiB leaves roughly 5x
/// headroom above that while staying small enough to cap memory in the single-digit MB
/// range when a response really does run away (an infinite repetition, say).
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub struct AnthropicProvider {
    api_key: ApiKey,
    base_url: String,
    model: String,
    http: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: ApiKey) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Lets a test point at a mock socket. Never called in production.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Exposes the request body so tests can verify it directly. A forbidden parameter
    /// leaking out is the kind of defect that must be caught without a real call.
    pub fn build_body_for_test(req: &LlmRequest, model: &str) -> Value {
        build_body(req, model)
    }
}

/// The fixed shape from spec Section 8.3.
///
/// **Never include `budget_tokens`, `temperature`, `top_p`, or `top_k`** — they return
/// 400 on Opus 5. Assistant prefill is prohibited too. `fallbacks` is unsupported on
/// Bedrock and Vertex, so it is unused for provider portability (spec Section 8.6).
fn build_body(req: &LlmRequest, model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": req.max_tokens,
        "stream": true,
        // The system prompt must be sent as a block array to carry cache_control.
        "system": [{
            "type": "text",
            "text": req.system,
            "cache_control": { "type": "ephemeral" }
        }],
        // Without display: summarized the thinking text is an empty string and no
        // progress appears in the UI.
        "thinking": { "type": "adaptive", "display": "summarized" },
        "output_config": { "effort": "high" },
        "tools": req.tools.iter().map(|t| json!({
            "name": t.name,
            "description": t.description,
            "input_schema": t.input_schema,
        })).collect::<Vec<_>>(),
        "messages": req.messages.iter().map(|m| json!({
            "role": m.role.as_str(),
            "content": m.content.iter().map(content_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn content_json(c: &LlmContent) -> Value {
    match c {
        LlmContent::Text { text } => json!({ "type": "text", "text": text }),
        LlmContent::ToolUse { id, name, input } => {
            json!({ "type": "tool_use", "id": id, "name": name, "input": input })
        }
        LlmContent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        }),
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    async fn stream(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", self.api_key.expose())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&build_body(&req, &self.model))
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            // Always include the body — discarding it hides the cause of a 400.
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Status {
                status: status.as_u16(),
                body,
            });
        }

        let mut parser = SseParser::new();
        let mut machine = StreamMachine::new();
        let bytes = resp.bytes_stream();

        // Count cumulative bytes per chunk, and once past the ceiling emit one error and
        // end the stream (`scan` returns `None` on later polls). This is the defense
        // against `SseParser`'s unbounded buffer growth — the Task 2 review assigned it to
        // this file, which owns the connection.
        let sized = bytes.scan((0usize, false), |(total, done), chunk| {
            if *done {
                return futures_util::future::ready(None);
            }
            let item = match chunk {
                Err(e) => {
                    *done = true;
                    Err(LlmError::Transport(e.to_string()))
                }
                Ok(b) => {
                    *total += b.len();
                    if *total > MAX_RESPONSE_BYTES {
                        *done = true;
                        Err(LlmError::Transport(format!(
                            "response exceeded {MAX_RESPONSE_BYTES}-byte limit"
                        )))
                    } else {
                        Ok(b)
                    }
                }
            };
            futures_util::future::ready(Some(item))
        });

        // One chunk can produce zero or many frames, so flat_map flattens them.
        let events = sized.flat_map(move |chunk| {
            let out: Vec<Result<LlmEvent, LlmError>> = match chunk {
                Err(e) => vec![Err(e)],
                Ok(b) => {
                    let mut acc = Vec::new();
                    for frame in parser.push(&b) {
                        // **Frames after `message_stop` (or `error`) are discarded**
                        // (review M6). A healthy connection ends here so it is harmless
                        // today, but a misbehaving proxy or a replay/multiplexing bug
                        // appending extra frames would, without this check, be processed
                        // as completed blocks and produce phantom `TextBlock`s and
                        // `ToolCall`s. `machine` is state that lives across chunks, so this
                        // check applies to every later chunk as well.
                        if machine.is_done() {
                            break;
                        }
                        match machine.feed(&frame) {
                            Ok(evs) => acc.extend(evs.into_iter().map(Ok)),
                            Err(e) => acc.push(Err(e)),
                        }
                    }
                    acc
                }
            };
            futures_util::stream::iter(out)
        });

        Ok(Box::pin(events))
    }
}

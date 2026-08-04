//! Chat sessions, sending, and the token stream (Task 12, spec Section 6.2).
//!
//! **Chat creates no investigation record.** `agentops_agent::phase::run_phase_loop`
//! takes a `PhaseCtx` requiring an `investigation_id`, so it is not used here — a thin
//! loop calling `LlmProvider::stream` directly ([`reply`]) lives in this file instead.
//!
//!
//! **v0.1 chat uses no tools.** Spec Section 6.2 says only "the same LLM pipeline" and
//! does not require tools — adding them leaves no `agent_steps` to record a `ToolCall`
//! in and nowhere to uphold the step invariant (no unpaired `tool_use`).
//!
//!
//! **Reconnect semantics are weaker than an investigation's** (spec Section 6.2). Deltas
//! are not persisted, so a refresh loses the partial response in flight — a deliberate
//! trade-off. `panel` re-renders only completed messages and `stream` carries only what
//! follows (the next exchange). Having no database replay on reconnect is what differs from `crate::stream`.
//!
//! **LLM output is untrusted data** (spec Section 13) — `render::chat_message_html` and
//! `render::chat_delta_html` escape it.

use crate::bus::{ChatEvent, StepBus};
use crate::render::{chat_delta_html, chat_message_html};
use crate::AppState;
use agentops_agent::prompt::assemble_system;
use agentops_agent::MAX_TOKENS;
use agentops_core::{
    ChatRole, ChatSession, LlmContent, LlmEvent, LlmMessage, LlmProvider, LlmRequest, LlmRole,
    Phase, Store, StoreError,
};
use agentops_store::PgStore;
use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::Form;
use futures_util::StreamExt;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Spec Section 10.2.1 — stops proxies closing an idle connection. The same value as `crate::stream`.
const HEARTBEAT: Duration = Duration::from_secs(15);

/// v0.1 has no UI for choosing a session — the Interfaces require only `send`, `stream`,
/// and `panel`, and session creation and listing screens are outside this task's scope.
/// `base.html` is a static template shared by every page (it receives no per-handler
/// context), so the sidebar has to know for itself which session to open — hence one
/// fixed session ID shared across every page. When `panel` is first called with this ID,
/// [`ensure_session`] creates it lazily.
pub const DEFAULT_SESSION: Uuid = Uuid::from_u128(1);

/// Creates the session row if absent. **Best-effort** — if it already exists (the most
/// common case, since it is called on every page load) the INSERT fails on a primary key
/// conflict, which is a normal state and is recorded at `debug` only. A genuine backend
/// failure makes the immediately following `chat_messages` or `append_chat_message` call
/// fail in its own place and log at `error` — logging here too would record the same
/// failure twice.
async fn ensure_session(store: &PgStore, id: Uuid) {
    let now = time::OffsetDateTime::now_utc();
    if let Err(e) = store
        .create_chat_session(&ChatSession {
            id,
            title: "Chat".into(),
            created_at: now,
            updated_at: now,
        })
        .await
    {
        tracing::debug!(error = %e, session = %id, "chat session create skipped (likely already exists)");
    }
}

#[derive(Debug, Deserialize)]
pub struct SendForm {
    pub content: String,
}

/// The v0.1 shape of `chat_messages.content` is only `{"text": "..."}` (identical to the
/// brief's test fixture). Any other key is treated as an empty string — no panic; this
/// value came through the database, and the page must not die if the schema shifts.
///
fn message_text(content: &serde_json::Value) -> String {
    content
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

#[derive(Template)]
#[template(path = "chat_panel.html")]
pub struct ChatPanel {
    pub session_id: Uuid,
    pub messages_html: String,
}

/// `GET /api/chat/{sid}/panel` — re-renders every completed message.
///
/// **A partial response in flight is not here** — deltas are not persisted (spec
/// Section 6.2). This is where the meaning differs from the investigation detail page's
/// `steps_html` (step replay).
///
/// Failures follow the list fragment's convention: a database error is not swallowed
/// silently but logged, then it continues with an empty list — this fragment's failure
/// must not bring the whole page down.
pub async fn panel(State(st): State<AppState>, Path(sid): Path<Uuid>) -> impl IntoResponse {
    // `base.html` calls this handler with [`DEFAULT_SESSION`] on every page load — if that
    // session does not exist yet it is created lazily here. It is also safe for a session
    // that already exists (including one the caller made directly, as this task's tests
    // do) — a duplicate creation is ignored silently.
    ensure_session(&st.store, sid).await;
    let messages = match st.store.chat_messages(sid).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, session = %sid, "chat message list failed");
            Vec::new()
        }
    };
    let messages_html = messages
        .iter()
        .map(|m| chat_message_html(m.role, &message_text(&m.content)))
        .collect::<String>();
    Html(
        ChatPanel {
            session_id: sid,
            messages_html,
        }
        .render()
        .expect("template render"),
    )
}

/// `POST /api/chat/{sid}/messages` — sends a message.
///
/// **The user message is stored before response generation.** Storing it after waiting
/// for the assistant response would lose what the user said if they refreshed in between.
///
///
/// **It does not wait for response generation to finish** (the same reason as an
/// investigation's INV-1) — a request must not stay open for the length of an LLM stream.
/// The response generation task goes on `JobManager::spawn_task` rather than a direct
/// `tokio::spawn`, so graceful shutdown waits for it too (a global convention: no
/// detached tasks).
///
/// **During shutdown it refuses with `503` before storing.** As the Task 12 review
/// (Important-1) pointed out, `main.rs`'s
/// `axum::serve(...).with_graceful_shutdown(shutdown)` keeps accepting new connections
/// until `JobManager::shutdown` finishes — that is, this handler can still be called
/// after the gate closes. Storing first and refusing later (inside the response
/// generation task) leaves the user message already persisted while the cancelled
/// `cancel_token` makes response generation waste a database query and an LLM stream open
/// before cancelling itself — leaving **a message that can never receive a response.**
/// Investigations survive this because `recover_on_boot` requeues at the next boot, but
/// chat has no such retry path — so unlike an investigation's `GateClosed` (create the
/// row and 303, Task 9's open item in final review triage), for chat **not storing at all
/// is the right call.** Why `503` was chosen: it is the standard status code that clearly
/// signals the client should retry, and it beats an ambiguous state (silently ignoring
/// it, say) where the user has to work out from the UI whether the message went through.
///
pub async fn send(
    State(st): State<AppState>,
    Path(sid): Path<Uuid>,
    Form(f): Form<SendForm>,
) -> Result<Html<String>, StatusCode> {
    let content = f.content.trim();
    if content.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    if !st.jobs.is_accepting() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    st.store
        .append_chat_message(sid, ChatRole::User, &serde_json::json!({ "text": content }))
        .await
        .map_err(|e| {
            // `StoreError::NotFound` gives 404; other errors are logged then 500 (a global
            // convention — Task 10 fixed a handler that collapsed database failures into
            // 404).
            if matches!(e, StoreError::NotFound) {
                StatusCode::NOT_FOUND
            } else {
                tracing::error!(error = %e, session = %sid, "chat message insert failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    let store = st.store.clone();
    let bus = st.bus.clone();
    let provider = st.jobs.provider();
    let cancel = st.jobs.cancel_token();
    st.jobs
        .spawn_task(async move { reply(store, bus, provider, cancel, sid).await });

    // So htmx can append the user's message bubble immediately, the same text just stored
    // is rendered and returned. It renders through the same function
    // (`chat_message_html`) as `panel` and SSE's `message` event, so the escaping and
    // markup of the three paths cannot diverge.
    Ok(Html(chat_message_html(ChatRole::User, content)))
}

/// The body of the response generation task [`send`] spawns.
///
/// The reason it does not use `run_phase_loop` is in the module documentation. Since it
/// uses no tools it sends `tools: Vec::new()`, and if the model calls a tool anyway it is
/// ignored (there is nothing to execute) with only a warning left behind.
async fn reply(
    store: Arc<PgStore>,
    bus: StepBus,
    provider: Arc<dyn LlmProvider>,
    cancel: CancellationToken,
    sid: Uuid,
) {
    let instructions = match store.instructions_for(&[Phase::All, Phase::Chat]).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, session = %sid, "chat instruction fetch failed");
            bus.publish_chat(sid, ChatEvent::Terminal);
            return;
        }
    };
    let system = assemble_system(&instructions, Phase::Chat);

    // The session's whole conversation history is moved into messages. The user message
    // `send` just stored is already included — the store precedes the spawn.
    let history = match store.chat_messages(sid).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, session = %sid, "chat history fetch failed");
            bus.publish_chat(sid, ChatEvent::Terminal);
            return;
        }
    };
    let messages: Vec<LlmMessage> = history
        .iter()
        .map(|m| LlmMessage {
            role: match m.role {
                ChatRole::User => LlmRole::User,
                ChatRole::Assistant => LlmRole::Assistant,
            },
            content: vec![LlmContent::Text {
                text: message_text(&m.content),
            }],
        })
        .collect();

    let req = LlmRequest {
        system,
        messages,
        tools: Vec::new(),
        max_tokens: MAX_TOKENS,
    };

    let mut stream = match provider.stream(req).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, session = %sid, "chat llm stream failed to start");
            bus.publish_chat(sid, ChatEvent::Terminal);
            return;
        }
    };

    let mut reply_text = String::new();
    let mut stopped = false;

    'stream: loop {
        tokio::select! {
            // Shutdown cancellation — because `JobManager::spawn_task` applies no gate
            // (see the module documentation), this select is the only place cancellation is received.
            _ = cancel.cancelled() => {
                bus.publish_chat(sid, ChatEvent::Terminal);
                return;
            }
            item = stream.next() => {
                match item {
                    None => break 'stream,
                    Some(Err(e)) => {
                        tracing::error!(error = %e, session = %sid, "chat llm stream error");
                        bus.publish_chat(sid, ChatEvent::Terminal);
                        return;
                    }
                    Some(Ok(LlmEvent::TextDelta { text })) => {
                        bus.publish_chat(sid, ChatEvent::Delta { text: chat_delta_html(&text) });
                    }
                    Some(Ok(LlmEvent::TextBlock { text })) => {
                        if !reply_text.is_empty() {
                            reply_text.push('\n');
                        }
                        reply_text.push_str(&text);
                    }
                    // The v0.1 chat panel does not show thinking — ignored.
                    Some(Ok(LlmEvent::ThinkingDelta { .. })) | Some(Ok(LlmEvent::ThinkingBlock { .. })) => {}
                    Some(Ok(LlmEvent::ToolCall { tool, .. })) => {
                        // Since `tools: Vec::new()` was sent this should not occur normally.
                        // There is neither a tool to execute nor `agent_steps` to pair with,
                        // so it is ignored with only a signal left behind.
                        tracing::warn!(session = %sid, tool = %tool, "chat stream emitted a tool call; v0.1 chat sends no tools");
                    }
                    Some(Ok(LlmEvent::StreamError { message })) => {
                        tracing::error!(session = %sid, message = %message, "chat stream error event");
                        bus.publish_chat(sid, ChatEvent::Terminal);
                        return;
                    }
                    Some(Ok(LlmEvent::Stopped { .. })) => {
                        stopped = true;
                        break 'stream;
                    }
                }
            }
        }
    }

    // `LlmProvider::stream`'s contract — a stream exhausted without `Stopped` is an error
    // (a cut proxy, an empty body). The partial response is not stored.
    if !stopped {
        tracing::error!(session = %sid, "chat stream ended without stop_reason");
        bus.publish_chat(sid, ChatEvent::Terminal);
        return;
    }

    if reply_text.is_empty() {
        // An empty response is not stored — storing it would leave a contentless assistant row.
        bus.publish_chat(sid, ChatEvent::Terminal);
        return;
    }

    match store
        .append_chat_message(
            sid,
            ChatRole::Assistant,
            &serde_json::json!({ "text": reply_text }),
        )
        .await
    {
        Ok(_) => {
            bus.publish_chat(
                sid,
                ChatEvent::Message {
                    html: chat_message_html(ChatRole::Assistant, &reply_text),
                },
            );
        }
        Err(e) => {
            tracing::error!(error = %e, session = %sid, "chat assistant message insert failed");
        }
    }
    bus.publish_chat(sid, ChatEvent::Terminal);
}

/// `GET /api/chat/{sid}/stream` — follows the same SSE pattern as [`crate::stream`] but
/// **has no replay**. Chat persists only completed messages, so on reconnect [`panel`]
/// re-renders everything and this stream carries only the live events that follow the
/// subscription.
///
/// **Receiving `Terminal` does not close the connection.** Unlike an investigation, a chat
/// session is not one-shot — the user can keep sending messages to the same session, so
/// `Terminal` only signals "this response is finished", not "there will never be another
/// event".
///
/// **It does not resync on `Lagged`.** `crate::stream` recovers an investigation's
/// `Lagged` from the database (spec Section 6.1, invariant 3), but chat deltas are not
/// persisted so there is no source to recover from — spec Section 6.2's trade-off, that
/// reconnect semantics are weaker than an investigation's, applies here too and a lagged
/// event is skipped silently.
pub async fn stream(State(st): State<AppState>, Path(sid): Path<Uuid>) -> impl IntoResponse {
    // It does not use `tokio_stream::wrappers::BroadcastStream` — that would need
    // tokio-stream's `sync` feature, which the workspace does not enable (spec Section 14,
    // pinned versions — an unverified feature flag is not added silently). Instead, by the
    // same convention as `crate::stream::async_stream_impl`, one task is spawned to drive
    // `recv()` directly and move it onto an mpsc channel.
    //
    // **It goes on `JobManager::spawn_task` — not a direct `tokio::spawn`** (a global
    // convention, spec Section 6.1: "never create a detached task"). And **`cancel_token()`
    // is watched together with `sub.recv()` in a `select!`** — [`reply`] just above already
    // has the same shape. Without these two, `sub.recv()` parks forever: the chat channel
    // (`StepBus.chat`) has no `retire` (see `bus.rs`) and `StepBus` lives for the router's
    // whole lifetime, so `RecvError::Closed` never arrives. If the SSE response body never
    // ends, axum's connection task never ends, and `WithGracefulShutdown` waits
    // indefinitely for every connection task to finish — a single open browser tab makes
    // `Ctrl-C` never return (final review C-1, confirmed by independent reproduction).
    //
    //
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(64);
    let mut sub = st.bus.subscribe_chat(sid);
    let cancel = st.jobs.cancel_token();
    // **`spawn_task` itself guarantees the task's lifetime** — this async block may
    // reference only `cancel`, `sub`, and `tx` (it need not capture `st.jobs` by disjoint
    // capture). Final review N-1: the form that carried `keep_alive` directly here was
    // missed at one of the three call sites (the `reply()` spawned by `send`, at line 197
    // just above) and responses really were not stored — so it was moved into
    // `JobManager::spawn_task` so nothing relies on caller discipline.
    st.jobs.spawn_task(async move {
        loop {
            tokio::select! {
                // Shutdown cancellation. It sends `terminal` to end the SSE body — a chat
                // session normally does not close on `Terminal` (see this function's
                // documentation), but the server is about to exit as a process and there is
                // no listener left to accept a reconnect. That is why the asymmetry of
                // omitting `sse-close` on this stream, unlike `investigation.html`, is
                // intentional.
                _ = cancel.cancelled() => {
                    let _ = tx.send(Event::default().event("terminal").data("")).await;
                    return;
                }
                r = sub.recv() => {
                    let ev = match r {
                        Ok(ChatEvent::Delta { text }) => Event::default().event("delta").data(text),
                        Ok(ChatEvent::Message { html }) => Event::default().event("message").data(html),
                        Ok(ChatEvent::Terminal) => Event::default().event("terminal").data(""),
                        // Chat does not recover from lag (see this function's
                        // documentation) — it skips and keeps subscribing.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        // The sender (StepBus) disappearing entirely happens only in the
                        // process-exit scenario — the connection is closed.
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    };
                    if tx.send(ev).await.is_err() {
                        return;
                    }
                }
            }
        }
    });
    let events = tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<Event, Infallible>);
    Sse::new(events).keep_alive(KeepAlive::new().interval(HEARTBEAT))
}

//! Per-investigation `broadcast` fan-out.
//!
//! **Each investigation gets its own channel.** Pushing every investigation through one
//! global channel lets a single busy investigation fill it and hit even a slow subscriber
//! watching a quiet investigation with `Lagged`. The unit of isolation is the investigation.
//!
//! **An `Err` from `send` is not an error.** It occurs with zero subscribers, and an
//! investigation nobody is watching is a normal state — the runner must keep going
//! (spec Section 6.1, INV-1: an investigation is not tied to an HTTP request).

use agentops_agent::phase::DeltaSink;
use agentops_core::{AgentStep, Phase};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Spec Section 6.1, invariant 3 — capacity 256.
const CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaKind {
    Text,
    Thinking,
}

#[derive(Debug, Clone)]
pub enum BusEvent {
    /// A persisted step. It has a `seq` and is subject to replay.
    Step(Box<AgentStep>),
    /// A transient token. Absent from the database and never replayed (spec Section 6.1.3).
    ///
    /// **`text` is raw — not rendered.** HTML escaping happens later, when the event
    /// becomes an SSE `Event`, in `render::delta_html` (`stream.rs`). `ChatEvent::Delta`
    /// does not follow this rule (see its own documentation), so the two types are never
    /// used interchangeably.
    Delta {
        investigation_id: Uuid,
        phase: Phase,
        kind: DeltaKind,
        text: String,
    },
    /// A signal to close this investigation's stream.
    Terminal { investigation_id: Uuid },
}

/// Chat stream events (Task 12, spec Section 6.2).
///
/// **Reconnect semantics are weaker than an investigation's.** Deltas are not persisted,
/// just as with investigations — a refresh loses the partial response in flight (a
/// deliberate trade-off). Only completed messages are stored in `chat_messages`, and on
/// reconnect `panel` re-renders all of them.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// A token delta. Not persisted (spec Section 6.2).
    ///
    /// **Unlike `BusEvent::Delta`, `text` here is already-rendered HTML** —
    /// `routes::chat::reply` escapes it with `render::chat_delta_html` before putting it
    /// in this field, and `stream()` emits it to SSE as-is (without escaping again). If a
    /// second consumer of this type appears (a log sink, say), it is easy to mistake this
    /// field for raw text and either escape it twice or fail to strip the markup — the name
    /// matches `BusEvent::Delta` but "when it is rendered" is the opposite.
    ///
    Delta { text: String },
    /// The completed message's HTML.
    Message { html: String },
    /// A signal that this response generation finished. **It does not mean close the
    /// connection** — unlike an investigation, a chat session is not one-shot (more
    /// messages can arrive on the same session), so the `stream` handler keeps the SSE
    /// connection open and waits for the next exchange.
    Terminal,
}

#[derive(Clone, Default)]
pub struct StepBus {
    channels: Arc<Mutex<HashMap<Uuid, broadcast::Sender<BusEvent>>>>,
    /// **A separate map from the investigation channels (`channels`).** Putting both in
    /// one map keyed by UUID means the two streams mix whenever a session ID and an
    /// investigation ID coincide — the collision probability is low in practice, but the
    /// problem is that the structure does not prevent it (the Task 12 brief).
    ///
    /// **There is no `retire` here.** An investigation channel has a clear end point
    /// (`JobManager::spawn`'s termination path) at which it can be reclaimed, but a chat
    /// session has no "end" — the user can send another message to the same session at any
    /// time. This map only grows as sessions are created (a known limitation; session
    /// retention and cleanup policy is outside this task's scope).
    chat: Arc<Mutex<HashMap<Uuid, broadcast::Sender<ChatEvent>>>>,
}

impl StepBus {
    pub fn new() -> Self {
        Self::default()
    }

    fn sender(&self, id: Uuid) -> broadcast::Sender<BusEvent> {
        let mut g = self.channels.lock().expect("bus mutex poisoned");
        g.entry(id)
            .or_insert_with(|| broadcast::channel(CAPACITY).0)
            .clone()
    }

    fn chat_sender(&self, id: Uuid) -> broadcast::Sender<ChatEvent> {
        let mut g = self.chat.lock().expect("bus mutex poisoned");
        g.entry(id)
            .or_insert_with(|| broadcast::channel(CAPACITY).0)
            .clone()
    }

    /// The same shape as the investigation `subscribe` — to miss no events, subscription
    /// must happen before response generation is spawned.
    pub fn subscribe_chat(&self, session_id: Uuid) -> broadcast::Receiver<ChatEvent> {
        self.chat_sender(session_id).subscribe()
    }

    pub fn publish_chat(&self, session_id: Uuid, ev: ChatEvent) {
        // Zero subscribers gives an Err. Ignored for the same reason as investigations —
        // a response arriving on a session nobody is watching is a normal state.
        let _ = self.chat_sender(session_id).send(ev);
    }

    /// **Subscribe before replaying** (spec Section 6.1, invariant 2). In the opposite
    /// order, any step written between the query and the subscription is lost.
    pub fn subscribe(&self, id: Uuid) -> broadcast::Receiver<BusEvent> {
        self.sender(id).subscribe()
    }

    fn emit(&self, id: Uuid, ev: BusEvent) {
        // Zero subscribers gives an Err. Ignored — see the module comment above.
        let _ = self.sender(id).send(ev);
    }

    pub fn publish_step(&self, step: AgentStep) {
        let id = step.investigation_id;
        self.emit(id, BusEvent::Step(Box::new(step)));
    }

    pub fn publish_terminal(&self, id: Uuid) {
        self.emit(
            id,
            BusEvent::Terminal {
                investigation_id: id,
            },
        );
    }

    /// Removes the channel after an investigation ends so the map does not grow forever.
    /// **Do not call it immediately after `publish_terminal`** — subscribers may still be
    /// draining. `JobManager` calls it after joining the task.
    pub fn retire(&self, id: Uuid) {
        self.channels
            .lock()
            .expect("bus mutex poisoned")
            .remove(&id);
    }

    /// How many channels are currently live. Tests observe through this value whether
    /// `retire` really stops the map growing — without calling `retire`, one channel per
    /// investigation survives for the process's lifetime.
    pub fn live_channels(&self) -> usize {
        self.channels.lock().expect("bus mutex poisoned").len()
    }
}

impl DeltaSink for StepBus {
    fn text(&self, investigation_id: Uuid, phase: Phase, delta: &str) {
        self.emit(
            investigation_id,
            BusEvent::Delta {
                investigation_id,
                phase,
                kind: DeltaKind::Text,
                text: delta.to_string(),
            },
        );
    }

    fn thinking(&self, investigation_id: Uuid, phase: Phase, delta: &str) {
        self.emit(
            investigation_id,
            BusEvent::Delta {
                investigation_id,
                phase,
                kind: DeltaKind::Thinking,
                text: delta.to_string(),
            },
        );
    }
}

//! `AgentStep` to an HTML fragment. SSE and the detail page use **the same function**.
//!
//! If the two paths rendered separately, the screen would differ across a refresh, and
//! that difference would surface only on the replay path, where it goes unnoticed.
//!
//! **LLM responses and tool output are untrusted data** (spec Section 13).
//! They are escaped here.

use agentops_core::{AgentStep, ChatRole, StepKind};

/// Escapes HTML special characters. The same rules as askama's default behavior.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn step_html(step: &AgentStep) -> String {
    let phase = format!("{:?}", step.phase).to_lowercase();
    let body = match &step.kind {
        StepKind::Text { text } => format!(r#"<div class="step-text">{}</div>"#, esc(text)),
        StepKind::Thinking { summary } => {
            format!(r#"<div class="step-thinking">{}</div>"#, esc(summary))
        }
        StepKind::ToolCall {
            tool, tool_use_id, ..
        } => format!(
            r#"<div class="step-toolcall" data-tool-use-id="{}">{}</div>"#,
            esc(tool_use_id),
            esc(tool)
        ),
        StepKind::ToolResult {
            tool,
            is_error,
            output,
            ..
        } => format!(
            r#"<div class="step-toolresult{}">{}: {}</div>"#,
            if *is_error { " is-error" } else { "" },
            esc(tool),
            esc(output)
        ),
        StepKind::ArtifactWritten { artifact_id } => format!(
            r#"<div class="step-artifact"><a href="/artifacts/{}">Artifact</a></div>"#,
            artifact_id
        ),
        StepKind::Terminated { reason, detail } => format!(
            r#"<div class="step-terminated">{}{}</div>"#,
            esc(&format!("{reason:?}")),
            detail
                .as_deref()
                .map(|d| format!(" — {}", esc(d)))
                .unwrap_or_default()
        ),
        StepKind::Error { message } => {
            format!(r#"<div class="step-error">{}</div>"#, esc(message))
        }
    };
    format!(
        r#"<li class="step phase-{phase}" data-seq="{}">{body}</li>"#,
        step.seq
    )
}

pub fn delta_html(kind: crate::bus::DeltaKind, text: &str) -> String {
    let cls = match kind {
        crate::bus::DeltaKind::Text => "delta-text",
        crate::bus::DeltaKind::Thinking => "delta-thinking",
    };
    format!(r#"<span class="{cls}">{}</span>"#, esc(text))
}

/// One chat message to HTML. `panel` (the initial render) and SSE's `message` event use
/// **the same function** — the same reason as `step_html` above: rendering separately
/// would make the screen differ across a refresh.
///
/// **LLM responses are untrusted data** (spec Section 13) — escaped with `esc`.
pub fn chat_message_html(role: ChatRole, text: &str) -> String {
    let cls = match role {
        ChatRole::User => "chat-msg-user",
        ChatRole::Assistant => "chat-msg-assistant",
    };
    format!(r#"<div class="chat-msg {cls}">{}</div>"#, esc(text))
}

/// A chat token delta to HTML. Unlike `delta_html` there is no kind distinction — chat
/// does not surface thinking deltas in the UI (`routes::chat::reply`).
pub fn chat_delta_html(text: &str) -> String {
    format!(r#"<span class="chat-delta">{}</span>"#, esc(text))
}

/// **Final review I-2** — `step_html` is the only path this data reaches the screen by
/// (LLM responses and tool output, the data spec Section 13 names as untrusted).
/// Every existing escaping test targets a different renderer — investigation titles and
/// instruction bodies go through askama's automatic escaping, chat through
/// `chat_message_html`. No test on this branch guarded `step_html` itself: deleting
/// `esc()` at `render.rs:50` (`ToolResult.output`) left all 69 passing, confirmed by
/// measurement (it reproduces exactly on regression — `tool_result_output_is_escaped`
/// below catches that mutation).
#[cfg(test)]
mod tests {
    use super::*;
    use agentops_core::{Phase, TerminalReason};
    use time::OffsetDateTime;
    use uuid::Uuid;

    const PAYLOAD: &str = r#"<script>alert(1)</script>"#;
    const ESCAPED: &str = "&lt;script&gt;alert(1)&lt;/script&gt;";

    fn step(kind: StepKind) -> AgentStep {
        AgentStep {
            investigation_id: Uuid::new_v4(),
            seq: 0,
            phase: Phase::Triage,
            kind,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn text_is_escaped() {
        let html = step_html(&step(StepKind::Text {
            text: PAYLOAD.into(),
        }));
        assert!(html.contains(ESCAPED), "escaped payload missing: {html}");
        assert!(!html.contains(PAYLOAD), "raw payload leaked: {html}");
    }

    #[test]
    fn thinking_summary_is_escaped() {
        let html = step_html(&step(StepKind::Thinking {
            summary: PAYLOAD.into(),
        }));
        assert!(html.contains(ESCAPED), "escaped payload missing: {html}");
        assert!(!html.contains(PAYLOAD), "raw payload leaked: {html}");
    }

    /// The measured mutation of deleting `esc(output)` at `render.rs:50` — only this test
    /// catches it. `ToolResult.output` is the only "tool output" field among the data spec
    /// Section 13 names that passes through `step_html`.
    #[test]
    fn tool_result_output_is_escaped() {
        let html = step_html(&step(StepKind::ToolResult {
            tool_use_id: "tu_1".into(),
            tool: "shell".into(),
            output: PAYLOAD.into(),
            is_error: false,
        }));
        assert!(html.contains(ESCAPED), "escaped payload missing: {html}");
        assert!(!html.contains(PAYLOAD), "raw payload leaked: {html}");
    }

    #[test]
    fn error_message_is_escaped() {
        let html = step_html(&step(StepKind::Error {
            message: PAYLOAD.into(),
        }));
        assert!(html.contains(ESCAPED), "escaped payload missing: {html}");
        assert!(!html.contains(PAYLOAD), "raw payload leaked: {html}");
    }

    #[test]
    fn terminated_detail_is_escaped() {
        let html = step_html(&step(StepKind::Terminated {
            reason: TerminalReason::TaskPanicked,
            detail: Some(PAYLOAD.into()),
        }));
        assert!(html.contains(ESCAPED), "escaped payload missing: {html}");
        assert!(!html.contains(PAYLOAD), "raw payload leaked: {html}");
    }
}

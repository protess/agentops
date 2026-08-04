use agentops_agent::anthropic::AnthropicProvider;
use agentops_agent::ApiKey;
use agentops_core::{LlmEvent, LlmMessage, LlmProvider, LlmRequest, StopReason, ToolDef};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn req() -> LlmRequest {
    LlmRequest {
        system: "you are an SRE".into(),
        messages: vec![LlmMessage::user_text("why is latency high")],
        tools: vec![ToolDef {
            name: "k8s__nodes".into(),
            description: "list nodes".into(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
        }],
        max_tokens: 32000,
    }
}

/// A minimal HTTP/1.1 server returning a fixed response body. It passes the request body over a channel.
/// Built by hand to avoid adding a dependency — all we need is one socket.
async fn serve_once(body: &'static str) -> (String, tokio::sync::oneshot::Receiver<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 64 * 1024];
        let n = sock.read(&mut buf).await.unwrap();
        let raw = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tx.send(raw);

        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
        sock.flush().await.unwrap();
    });

    (format!("http://{addr}"), rx)
}

// ---------- Request shape ----------

/// If even one parameter spec Section 8.3 forbids goes out, Opus 5 returns 400.
/// Without this test we would not find out until the first real call.
#[test]
fn forbidden_parameters_are_never_sent() {
    let body = AnthropicProvider::build_body_for_test(&req(), "claude-opus-5");
    let obj = body.as_object().unwrap();
    for k in [
        "budget_tokens",
        "temperature",
        "top_p",
        "top_k",
        "fallbacks",
    ] {
        assert!(!obj.contains_key(k), "sending {k} returns a 400");
    }
    // budget_tokens must be absent inside thinking too.
    assert!(
        body["thinking"].get("budget_tokens").is_none(),
        "thinking.budget_tokens is a 400 on Opus 5"
    );
}

/// The fixed shape from spec Section 8.3. Missing any one breaks streaming or the thinking display.
#[test]
fn request_body_matches_the_spec_shape() {
    let body = AnthropicProvider::build_body_for_test(&req(), "claude-opus-5");
    assert_eq!(body["model"], "claude-opus-5");
    assert_eq!(body["max_tokens"], 32000);
    assert_eq!(body["stream"], true);
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["display"], "summarized");
    assert_eq!(body["output_config"]["effort"], "high");
}

/// The system prompt is sent as a block array carrying cache_control — without it the
/// prompt cache does not work at all (spec Section 8.3).
#[test]
fn system_prompt_carries_ephemeral_cache_control() {
    let body = AnthropicProvider::build_body_for_test(&req(), "claude-opus-5");
    let sys = body["system"].as_array().expect("system is a block array");
    assert_eq!(sys.len(), 1);
    assert_eq!(sys[0]["type"], "text");
    assert_eq!(sys[0]["text"], "you are an SRE");
    assert_eq!(sys[0]["cache_control"]["type"], "ephemeral");
}

/// tools[] passes MCP's JSON Schema through verbatim as input_schema (spec Section 9).
#[test]
fn tools_serialize_with_the_mcp_schema_verbatim() {
    let body = AnthropicProvider::build_body_for_test(&req(), "claude-opus-5");
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "k8s__nodes");
    assert_eq!(tools[0]["description"], "list nodes");
    assert_eq!(
        tools[0]["input_schema"],
        serde_json::json!({"type":"object","properties":{}})
    );
}

/// Identical input must produce a byte-identical body — the cache's precondition.
#[test]
fn body_is_byte_identical_for_identical_input() {
    let a = serde_json::to_string(&AnthropicProvider::build_body_for_test(
        &req(),
        "claude-opus-5",
    ))
    .unwrap();
    for _ in 0..5 {
        let b = serde_json::to_string(&AnthropicProvider::build_body_for_test(
            &req(),
            "claude-opus-5",
        ))
        .unwrap();
        assert_eq!(a, b);
    }
}

// ---------- Streaming ----------

#[tokio::test]
async fn streams_events_from_a_real_socket() {
    let sse = "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":5}}}\n\n\
               event: ping\ndata: {}\n\n\
               event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n\
               event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
               event: content_block_stop\ndata: {\"index\":0}\n\n\
               event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n\
               event: message_stop\ndata: {}\n\n";
    let (base, got_req) = serve_once(sse).await;

    let p = AnthropicProvider::new(ApiKey::new("test-key")).with_base_url(base);
    let mut s = p.stream(req()).await.expect("stream must open");

    let mut events = Vec::new();
    while let Some(e) = s.next().await {
        events.push(e.expect("no stream error"));
    }

    assert_eq!(
        events,
        vec![
            LlmEvent::TextDelta { text: "ok".into() },
            LlmEvent::TextBlock { text: "ok".into() },
            LlmEvent::Stopped {
                reason: StopReason::EndTurn,
                usage: agentops_core::Usage {
                    input_tokens: 5,
                    output_tokens: 3,
                    ..Default::default()
                },
                // Task 3 added `refusal_category` to `Stopped` (stop_details.category,
                // spec Section 8.4). This SSE fixture has no `stop_details`, so `None` is correct.
                refusal_category: None,
            },
        ]
    );

    // Confirm from the raw request that all three headers actually went out.
    let raw = got_req.await.unwrap();
    assert!(raw.contains("x-api-key: test-key"), "raw:\n{raw}");
    assert!(raw.contains("anthropic-version: 2023-06-01"), "raw:\n{raw}");
    assert!(
        raw.to_lowercase()
            .contains("content-type: application/json"),
        "raw:\n{raw}"
    );
}

/// **Review M6.** Frames arriving after `message_stop` must be ignored — this does not
/// happen when a connection closes normally, but a misbehaving proxy or a
/// replay/multiplexing bug appending frames after `message_stop` would have
/// `StreamMachine` process them as completed blocks and produce phantom `TextBlock`s and
/// `ToolCall`s. `is_done()` already carried that signal but nothing consumed it.
#[tokio::test]
async fn frames_after_message_stop_are_ignored() {
    let sse = "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":5}}}\n\n\
               event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n\
               event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
               event: content_block_stop\ndata: {\"index\":0}\n\n\
               event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n\
               event: message_stop\ndata: {}\n\n\
               event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"text\"}}\n\n\
               event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"ghost\"}}\n\n\
               event: content_block_stop\ndata: {\"index\":1}\n\n";
    let (base, _got_req) = serve_once(sse).await;

    let p = AnthropicProvider::new(ApiKey::new("test-key")).with_base_url(base);
    let mut s = p.stream(req()).await.expect("stream must open");

    let mut events = Vec::new();
    while let Some(e) = s.next().await {
        events.push(e.expect("frames after message_stop must not surface as errors either"));
    }

    assert_eq!(
        events,
        vec![
            LlmEvent::TextDelta { text: "ok".into() },
            LlmEvent::TextBlock { text: "ok".into() },
            LlmEvent::Stopped {
                reason: StopReason::EndTurn,
                usage: agentops_core::Usage {
                    input_tokens: 5,
                    output_tokens: 3,
                    ..Default::default()
                },
                refusal_category: None,
            },
        ],
        "a content_block frame arriving after message_stop leaked out as an event"
    );
}

/// A non-200 response must become a Status error carrying the body — discarding the body
/// hides the cause of a 400 (which parameter was the problem).
#[tokio::test]
async fn non_200_becomes_a_status_error_with_the_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = sock.read(&mut buf).await;
        let body =
            r#"{"error":{"type":"invalid_request_error","message":"temperature: unsupported"}}"#;
        let resp = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
    });

    let p = AnthropicProvider::new(ApiKey::new("k")).with_base_url(format!("http://{addr}"));
    // `expect_err` requires Debug on the Ok type and `Box<dyn Stream>` is not Debug, so
    // this uses let-else.
    let Err(err) = p.stream(req()).await else {
        panic!("400 must be an error");
    };
    match err {
        agentops_core::LlmError::Status { status, body } => {
            assert_eq!(status, 400);
            assert!(
                body.contains("temperature: unsupported"),
                "body dropped: {body}"
            );
        }
        other => panic!("expected Status, got {other:?}"),
    }
}

/// Confirms the ceiling that stops a response growing without bound and without newlines
/// actually fires (the Task 2 review assigned it to this file). When the server sends one
/// SSE comment line exceeding 16MiB (starting with `:`, a line the parser ignores), it
/// must end with `LlmError::Transport` the moment the ceiling is passed — the decision is
/// on total bytes alone, so this test verifies the ceiling regardless of newlines.
#[tokio::test]
async fn response_exceeding_the_size_ceiling_becomes_a_transport_error() {
    // Take 1MiB above the 16MiB ceiling.
    const OVER_LIMIT: usize = 17 * 1024 * 1024;
    let padding = "x".repeat(OVER_LIMIT);
    let body: &'static str = Box::leak(format!(": {padding}\n\n").into_boxed_str());

    let (base, _got_req) = serve_once(body).await;

    let p = AnthropicProvider::new(ApiKey::new("test-key")).with_base_url(base);
    let mut s = p.stream(req()).await.expect("stream must open");

    // Until the ceiling is passed there is only one newline-free SSE comment line, so no
    // frame is produced — the first item emitted the moment the ceiling is passed must be
    // exactly this error.
    let e = s.next().await.expect("stream must yield the ceiling error");
    match e {
        Err(agentops_core::LlmError::Transport(msg)) => {
            assert!(
                msg.contains("16777216"),
                "message should name the limit: {msg}"
            );
        }
        Err(other) => panic!("expected Transport, got {other:?}"),
        Ok(ev) => panic!("did not expect a normal event before the ceiling: {ev:?}"),
    }
    assert!(
        s.next().await.is_none(),
        "stream must terminate after the ceiling error"
    );
}

/// A 200 with an empty body ends with neither an event nor an error. **This is a
/// documented contract, not a defect** — the consumer (Task 11's phase loop) treats
/// "exhausted without `Stopped`" as an error. This test pins that behavior so that if
/// someone later makes the provider synthesize an error, the change is deliberate.
#[tokio::test]
async fn a_200_with_an_empty_body_yields_no_events_and_no_error() {
    let (base, _req) = serve_once("").await;
    let p = AnthropicProvider::new(ApiKey::new("k")).with_base_url(base);
    let mut s = p.stream(req()).await.expect("200 opens the stream");

    let mut n = 0usize;
    while let Some(item) = s.next().await {
        item.expect("an empty body is not an error");
        n += 1;
    }
    assert_eq!(n, 0, "an event came out of an empty body");
}

/// A non-SSE body is the same — the parser discards lines that are not `event:`/`data:`.
#[tokio::test]
async fn a_200_with_a_non_sse_body_yields_no_events_and_no_error() {
    let (base, _req) = serve_once("<html><body>proxy error</body></html>\n").await;
    let p = AnthropicProvider::new(ApiKey::new("k")).with_base_url(base);
    let mut s = p.stream(req()).await.expect("200 opens the stream");

    let mut n = 0usize;
    while let Some(item) = s.next().await {
        item.expect("a non-SSE body is not an error");
        n += 1;
    }
    assert_eq!(n, 0, "an event came out of a non-SSE body");
}

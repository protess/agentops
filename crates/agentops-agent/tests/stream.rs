use agentops_agent::sse::SseFrame;
use agentops_agent::stream::StreamMachine;
use agentops_core::{LlmEvent, StopReason};

fn f(event: &str, data: &str) -> SseFrame {
    SseFrame {
        event: event.into(),
        data: data.into(),
    }
}

fn run(frames: &[SseFrame]) -> Vec<LlmEvent> {
    let mut m = StreamMachine::new();
    let mut out = Vec::new();
    for fr in frames {
        out.extend(m.feed(fr).expect("machine must not error"));
    }
    out
}

/// TEST-12 — stop_reason does not arrive until message_delta. No termination decision
/// may be made before that. This reproduces that ordering.
#[test]
fn test_12_stop_reason_arrives_only_in_message_delta() {
    let events = run(&[
        f(
            "message_start",
            r#"{"message":{"usage":{"input_tokens":10}}}"#,
        ),
        f(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text"}}"#,
        ),
        f(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        ),
        f("content_block_stop", r#"{"index":0}"#),
    ]);

    // Up to here there must be no Stopped at all.
    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Stopped { .. })),
        "Stopped must not be emitted before message_delta: {events:?}"
    );

    let mut m = StreamMachine::new();
    for fr in [
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text"}}"#,
        ),
        f("content_block_stop", r#"{"index":0}"#),
    ] {
        m.feed(&fr).unwrap();
    }
    assert!(!m.is_done(), "it is not done before message_stop");

    let got = m
        .feed(&f(
            "message_delta",
            r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
        ))
        .unwrap();
    assert!(matches!(
        got.as_slice(),
        [LlmEvent::Stopped {
            reason: StopReason::EndTurn,
            ..
        }]
    ));

    m.feed(&f("message_stop", "{}")).unwrap();
    assert!(m.is_done());
}

/// TEST-12 — blocks are not accessed by index. Even when index does not arrive from 0 in
/// order (sparse or reversed), each block must complete with its own content.
#[test]
fn test_12_sparse_out_of_order_block_indexes_are_handled() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "content_block_start",
            r#"{"index":5,"content_block":{"type":"text"}}"#,
        ),
        f(
            "content_block_start",
            r#"{"index":2,"content_block":{"type":"thinking"}}"#,
        ),
        f(
            "content_block_delta",
            r#"{"index":2,"delta":{"type":"thinking_delta","thinking":"pondering"}}"#,
        ),
        f(
            "content_block_delta",
            r#"{"index":5,"delta":{"type":"text_delta","text":"answer"}}"#,
        ),
        f("content_block_stop", r#"{"index":2}"#),
        f("content_block_stop", r#"{"index":5}"#),
    ]);

    assert!(
        events.contains(&LlmEvent::ThinkingBlock {
            summary: "pondering".into()
        }),
        "the thinking block at index 2 must complete with its own content: {events:?}"
    );
    assert!(
        events.contains(&LlmEvent::TextBlock {
            text: "answer".into()
        }),
        "the text block at index 5 must complete with its own content: {events:?}"
    );
}

/// Deltas flow straight out (for the browser), and a separate event is emitted when a
/// block completes. Spec Section 6.1.3 — without that distinction we would write to the database per delta.
#[test]
fn deltas_and_completed_blocks_are_separate_events() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text"}}"#,
        ),
        f(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"text_delta","text":"a"}}"#,
        ),
        f(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"text_delta","text":"b"}}"#,
        ),
        f("content_block_stop", r#"{"index":0}"#),
    ]);

    assert_eq!(
        events,
        vec![
            LlmEvent::TextDelta { text: "a".into() },
            LlmEvent::TextDelta { text: "b".into() },
            LlmEvent::TextBlock { text: "ab".into() },
        ]
    );
}

/// A tool_use block concatenates input_json_delta and parses at completion.
/// Trying to parse partial JSON midway fails.
#[test]
fn tool_use_input_is_assembled_from_partial_json() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"gh__search"}}"#,
        ),
        f(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":"}}"#,
        ),
        f(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"\"oom\"}"}}"#,
        ),
        f("content_block_stop", r#"{"index":0}"#),
    ]);

    assert_eq!(
        events,
        vec![LlmEvent::ToolCall {
            tool_use_id: "toolu_1".into(),
            tool: "gh__search".into(),
            input: serde_json::json!({ "q": "oom" }),
        }]
    );
}

/// A tool with no arguments sends no partial_json at all. It must be read as an empty object.
#[test]
fn tool_use_with_no_input_deltas_becomes_an_empty_object() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"tool_use","id":"toolu_2","name":"k8s__nodes"}}"#,
        ),
        f("content_block_stop", r#"{"index":0}"#),
    ]);
    assert_eq!(
        events,
        vec![LlmEvent::ToolCall {
            tool_use_id: "toolu_2".into(),
            tool: "k8s__nodes".into(),
            input: serde_json::json!({}),
        }]
    );
}

/// The counterpart to TEST-6 — the state machine discards the unknown events the parser passed on.
#[test]
fn unknown_events_and_pings_are_dropped_not_errors() {
    let events = run(&[
        f("ping", "{}"),
        f("some_future_event", r#"{"whatever":1}"#),
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f("ping", "{}"),
    ]);
    assert!(
        events.is_empty(),
        "an event that should have been ignored leaked out: {events:?}"
    );
}

/// A mid-stream error event surfaces as StreamError and terminates the phase.
#[test]
fn stream_error_event_becomes_stream_error() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "error",
            r#"{"error":{"type":"overloaded_error","message":"slow down"}}"#,
        ),
    ]);
    assert_eq!(
        events,
        vec![LlmEvent::StreamError {
            message: "overloaded_error: slow down".into()
        }]
    );
}

/// An unknown stop_reason must arrive with its raw value preserved — it is the value that goes into TerminalReason.
#[test]
fn unknown_stop_reason_survives_the_machine() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "message_delta",
            r#"{"delta":{"stop_reason":"brand_new_reason"},"usage":{}}"#,
        ),
    ]);
    assert_eq!(
        events,
        vec![LlmEvent::Stopped {
            reason: StopReason::Unknown("brand_new_reason".into()),
            usage: Default::default(),
            refusal_category: None,
        }]
    );
}

/// Broken JSON is raised as MalformedEvent — passing over it silently would make a
/// half-processed stream look successful.
#[test]
fn malformed_json_is_an_error_not_a_silent_skip() {
    let mut m = StreamMachine::new();
    let err = m
        .feed(&f("message_start", "{not json"))
        .expect_err("broken JSON must be an error");
    assert!(
        matches!(err, agentops_core::LlmError::MalformedEvent(_)),
        "got {err:?}"
    );
}

/// Unlike a delta, `content_block_stop` does not pass over an unknown block silently.
/// An entire completed block would vanish, and had it been a `tool_use` the tool call
/// would disappear without a trace.
#[test]
fn content_block_stop_for_an_unknown_block_is_an_error() {
    let mut m = StreamMachine::new();
    m.feed(&f("message_start", r#"{"message":{"usage":{}}}"#))
        .unwrap();
    let err = m
        .feed(&f("content_block_stop", r#"{"index":7}"#))
        .expect_err("a stop with no start must be an error");
    assert!(
        matches!(err, agentops_core::LlmError::MalformedEvent(_)),
        "got {err:?}"
    );
}

/// A delta, by contrast, is lenient — discarding it loses only intermediate text that will soon be replaced.
#[test]
fn content_block_delta_for_an_unknown_block_is_dropped() {
    let mut m = StreamMachine::new();
    m.feed(&f("message_start", r#"{"message":{"usage":{}}}"#))
        .unwrap();
    let got = m
        .feed(&f(
            "content_block_delta",
            r#"{"index":7,"delta":{"type":"text_delta","text":"x"}}"#,
        ))
        .expect("a delta is not an error");
    assert!(got.is_empty(), "got {got:?}");
}

/// `stop_reason` is determined once. Allowing a second `message_delta` would let a
/// consumer taking the last one make the wrong termination reason win.
#[test]
fn a_second_message_delta_is_an_error() {
    let mut m = StreamMachine::new();
    m.feed(&f("message_start", r#"{"message":{"usage":{}}}"#))
        .unwrap();
    let first = m
        .feed(&f(
            "message_delta",
            r#"{"delta":{"stop_reason":"end_turn"},"usage":{}}"#,
        ))
        .unwrap();
    assert_eq!(first.len(), 1, "the first emits Stopped");
    let err = m
        .feed(&f(
            "message_delta",
            r#"{"delta":{"stop_reason":"refusal"},"usage":{}}"#,
        ))
        .expect_err("the second must be an error");
    assert!(
        matches!(err, agentops_core::LlmError::MalformedEvent(_)),
        "got {err:?}"
    );
}

/// Spec Section 8.4 — the refusal category must actually ride on the event. Without it,
/// `TerminalReason::Refusal { category }` is always None and the operator cannot tell
/// why the refusal happened.
#[test]
fn refusal_category_is_extracted_when_present() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "message_delta",
            r#"{"delta":{"stop_reason":"refusal","stop_details":{"category":"cyber"}},"usage":{}}"#,
        ),
    ]);
    assert_eq!(
        events,
        vec![LlmEvent::Stopped {
            reason: StopReason::Refusal,
            usage: Default::default(),
            refusal_category: Some("cyber".into()),
        }]
    );
}

/// The documentation describes `stop_details` as a top-level Message field. Its position
/// in SSE cannot be settled, so both paths are read — betting on one means that if it is
/// wrong, the category is permanently None and nothing fails.
#[test]
fn refusal_category_is_extracted_from_the_top_level_too() {
    let events = run(&[
        f("message_start", r#"{"message":{"usage":{}}}"#),
        f(
            "message_delta",
            r#"{"delta":{"stop_reason":"refusal"},"stop_details":{"type":"refusal","category":"cyber"},"usage":{}}"#,
        ),
    ]);
    assert_eq!(
        events,
        vec![LlmEvent::Stopped {
            reason: StopReason::Refusal,
            usage: Default::default(),
            refusal_category: Some("cyber".into()),
        }]
    );
}

/// An absent or null `stop_details` is not an error — spec Section 8.4 states so.
#[test]
fn a_refusal_without_stop_details_is_not_an_error() {
    for data in [
        r#"{"delta":{"stop_reason":"refusal"},"usage":{}}"#,
        r#"{"delta":{"stop_reason":"refusal","stop_details":null},"usage":{}}"#,
        r#"{"delta":{"stop_reason":"refusal","stop_details":{}},"usage":{}}"#,
        r#"{"delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":null}},"usage":{}}"#,
        r#"{"delta":{"stop_reason":"refusal"},"stop_details":{"type":"refusal","category":null},"usage":{}}"#,
    ] {
        let events = run(&[
            f("message_start", r#"{"message":{"usage":{}}}"#),
            f("message_delta", data),
        ]);
        assert_eq!(
            events,
            vec![LlmEvent::Stopped {
                reason: StopReason::Refusal,
                usage: Default::default(),
                refusal_category: None,
            }],
            "data: {data}"
        );
    }
}

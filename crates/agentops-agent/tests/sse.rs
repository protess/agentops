use agentops_agent::sse::{SseFrame, SseParser};

fn parse_all(chunks: &[&str]) -> Vec<SseFrame> {
    let mut p = SseParser::new();
    let mut out = Vec::new();
    for c in chunks {
        out.extend(p.push(c.as_bytes()));
    }
    out
}

#[test]
fn parses_a_single_frame() {
    let got = parse_all(&["event: message_start\ndata: {\"a\":1}\n\n"]);
    assert_eq!(
        got,
        vec![SseFrame {
            event: "message_start".into(),
            data: "{\"a\":1}".into()
        }]
    );
}

/// TEST-6 — the parser must keep going with pings and unknown events mixed in.
/// If the parser interpreted names, every new event type would break it.
#[test]
fn test_6_unknown_events_and_pings_do_not_stop_the_parser() {
    let got = parse_all(&[
        "event: ping\ndata: {}\n\n",
        "event: some_future_event\ndata: {\"x\":true}\n\n",
        ": this is a comment line\n\n",
        "event: message_stop\ndata: {}\n\n",
    ]);
    let names: Vec<&str> = got.iter().map(|f| f.event.as_str()).collect();
    assert_eq!(
        names,
        vec!["ping", "some_future_event", "message_stop"],
        "the parser does not interpret names and passes them all through"
    );
}

/// The result must be the same even when a chunk boundary falls mid-frame or mid-line.
/// TCP guarantees no boundaries, so this is a real failure mode.
#[test]
fn frames_survive_arbitrary_chunk_boundaries() {
    let whole = "event: content_block_delta\ndata: {\"i\":0}\n\nevent: message_stop\ndata: {}\n\n";
    let want = parse_all(&[whole]);
    assert_eq!(want.len(), 2);

    for split in 1..whole.len() {
        let (a, b) = whole.split_at(split);
        assert_eq!(parse_all(&[a, b]), want, "split at {split}");
    }
}

/// Multiple data: lines are joined with newlines (the SSE spec).
#[test]
fn multiple_data_lines_join_with_newline() {
    let got = parse_all(&["event: e\ndata: line1\ndata: line2\n\n"]);
    assert_eq!(got[0].data, "line1\nline2");
}

/// A frame with no event: defaults to the name "message" (the SSE spec).
#[test]
fn missing_event_name_defaults_to_message() {
    let got = parse_all(&["data: {}\n\n"]);
    assert_eq!(got[0].event, "message");
}

/// With no data: no frame is produced — a keepalive of comments and blank lines only.
#[test]
fn frame_without_data_is_not_emitted() {
    assert!(parse_all(&[": keepalive\n\n"]).is_empty());
}

/// Some intermediaries use CRLF.
#[test]
fn crlf_line_endings_parse() {
    let got = parse_all(&["event: e\r\ndata: {}\r\n\r\n"]);
    assert_eq!(got[0].event, "e");
    assert_eq!(got[0].data, "{}");
}

/// The differing whitespace handling of `event:` and `data:` is **deliberate**.
///
/// The value of `event` is the name `stream` dispatches on, so a single trailing space
/// makes the match fail silently — `trim()` is right. The value of `data` is content,
/// where byte fidelity matters — one leading space is stripped, per the spec.
/// Without this test someone could change one of them in the name of "consistency".
#[test]
fn event_names_are_trimmed_but_data_keeps_its_bytes() {
    let got = parse_all(&["event:   ping   \ndata:  {\"k\": 1}  \n\n"]);
    assert_eq!(got[0].event, "ping", "the name is trimmed");
    assert_eq!(
        got[0].data, " {\"k\": 1}  ",
        "content strips one leading space and preserves the rest"
    );
}

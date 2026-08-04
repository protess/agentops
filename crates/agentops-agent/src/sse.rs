//! The SSE frame parser.
//!
//! **It does not interpret event names.** It only groups `event:` and `data:` into
//! frames, leaving unknown names to the state machine (`crate::stream`). If the parser
//! had to know the names, it would break the day Anthropic adds an event — dying on the
//! very first stream from a single keepalive `ping` is that failure mode (spec Section 8.5).

/// One completed SSE frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub event: String,
    pub data: String,
}

/// An incremental parser that takes byte chunks and emits only completed frames.
///
/// TCP guarantees no chunk boundaries, so it must carry state.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a chunk and receive the frames it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();

        // Process only completed lines. Anything after the last newline stays buffered.
        while let Some(nl) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            let line = line.strip_suffix('\r').unwrap_or(&line);

            if line.is_empty() {
                // An empty line is a frame boundary. With no data at all it is not a frame.
                if !self.data.is_empty() {
                    out.push(SseFrame {
                        event: self.event.take().unwrap_or_else(|| "message".to_string()),
                        data: self.data.join("\n"),
                    });
                }
                self.event = None;
                self.data.clear();
            } else if let Some(rest) = line.strip_prefix("event:") {
                // **The differing whitespace handling of `event:` and `data:` is deliberate.**
                // The SSE spec says to strip one leading space from both, but the value of
                // `event` is the **name** `crate::stream` dispatches on, so a single
                // trailing space makes the match fail silently. `trim()` is used so the
                // name survives an intermediary reformatting whitespace.
                self.event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                // The value of `data`, by contrast, is **content** (JSON), where byte
                // fidelity matters. One leading space is stripped, per the spec — `trim()`
                // would lose meaningful whitespace in the payload.
                self.data
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // Other lines (`:` comments, `id:`, `retry:`) are ignored.
        }
        out
    }
}

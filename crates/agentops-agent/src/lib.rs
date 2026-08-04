//! The agent layer — LLM streaming, MCP tools, the phase loop.
//!
//! It sits on plan 1's `Store`, and the HTTP surface (plan 3) calls into this crate.

use std::fmt;

pub mod anthropic;
pub mod limits;
pub mod mcp;
pub mod mcp_client;
pub mod outcome;
pub mod phase;
pub mod policy;
pub mod prompt;
pub mod runner;
pub mod sse;
pub mod stream;

/// The fixed value from spec Section 8.3 — the combined ceiling for thinking plus response text.
///
/// Task 11's `run_phase_loop` puts this into every request's `LlmRequest::max_tokens`,
/// and Task 5's `build_body` serializes it as-is. It exceeds 16K, so `stream: true` is
/// required — non-streaming hits an HTTP timeout.
pub const MAX_TOKENS: u32 = 32_000;

/// The API key. Its `Debug` never exposes the value.
///
/// Spec Section 8.2 named `SecretString`, but `secrecy` is absent from the version table
/// in Section 14, so its version could not be verified when the plan was written. The
/// requirement (the key never leaks into a log or a panic message) is met by these 15
/// lines, so no unverified dependency is added. Swapping in `secrecy` later would not change this type's call sites.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Used only when putting it in a header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed value from spec Section 8.3. A change must be visible — thinking is on
    /// so headroom is needed, and lowering it truncates long investigation output.
    #[test]
    fn max_tokens_matches_the_spec() {
        assert_eq!(MAX_TOKENS, 32_000);
    }

    #[test]
    fn debug_does_not_leak_the_key() {
        let k = ApiKey::new("sk-ant-secret-value");
        let shown = format!("{k:?}");
        assert_eq!(shown, "ApiKey(***)");
        assert!(
            !shown.contains("secret"),
            "key must not appear in Debug output"
        );
    }
}

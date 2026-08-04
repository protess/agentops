/// Storage-layer errors. Flattened to a string so the backend type
/// (`sqlx::Error`) never leaks into core, which knows no I/O crate (design spec, Section 4.1).
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    /// A conditional transition affected zero rows — another party already terminated it.
    #[error("conflicting state transition")]
    Conflict,
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("store backend error: {0}")]
    Backend(String),
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("provider returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("malformed stream event: {0}")]
    MalformedEvent(String),
    #[error("stream idle timeout after {seconds}s")]
    IdleTimeout { seconds: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    /// The policy is `deny` — the default for a newly discovered tool (design spec, Section 9.1).
    #[error("tool denied by policy: {0}")]
    Denied(String),
    #[error("tool timed out after {seconds}s: {tool}")]
    Timeout { tool: String, seconds: u64 },
    #[error("tool transport error: {0}")]
    Transport(String),
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("job queue is shutting down")]
    ShuttingDown,
    #[error("store error: {0}")]
    Store(#[from] StoreError),
}

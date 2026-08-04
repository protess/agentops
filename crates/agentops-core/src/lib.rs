//! agentops domain types and traits. Contains no I/O.
//!
//! Never add `sqlx`, `reqwest`, or `tokio` as dependencies of this crate.
//! Preventing exactly that is the purpose of the crate boundary (design spec, Section 4.1).

pub mod chat;
pub mod error;
pub mod investigation;
pub mod knowledge;
pub mod llm;
pub mod mcp;
pub mod step;
pub mod traits;

pub use chat::{ChatMessage, ChatRole, ChatSession};
pub use error::{JobError, LlmError, StoreError, ToolError};
pub use investigation::{Investigation, InvestigationStatus, TriggeredBy};
pub use knowledge::{Artifact, Instruction, NewArtifact};
pub use llm::{
    LlmContent, LlmEvent, LlmMessage, LlmProvider, LlmRequest, LlmRole, StopReason, Usage,
};
pub use mcp::{McpServer, McpTransport, ToolPolicy, ToolPolicyKind};
pub use step::{AgentStep, Phase, StepKind, TerminalReason, STEP_PAYLOAD_VERSION};
pub use traits::{
    BoxStream, Cursor, InvestigationPage, JobManager, ListFilter, Store, ToolDef, ToolOutput,
    ToolRegistry,
};

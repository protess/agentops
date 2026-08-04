use crate::chat::{ChatMessage, ChatRole, ChatSession};
use crate::error::{JobError, StoreError, ToolError};
use crate::investigation::{Investigation, InvestigationStatus};
use crate::knowledge::{Artifact, Instruction, NewArtifact};
use crate::mcp::{McpServer, ToolPolicy, ToolPolicyKind};
use crate::step::{AgentStep, Phase, StepKind, TerminalReason};
use async_trait::async_trait;
use std::pin::Pin;
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

/// A stream alias carrying owned types only. Being `'static`, it can cross `.await`
/// boundaries, and being boxed it does not break trait object safety.
pub type BoxStream<'a, T> = Pin<Box<dyn futures_core::Stream<Item = T> + Send + 'a>>;

/// A keyset pagination cursor. The `(queued_at, id)` tuple breaks ties on identical timestamps.
pub type Cursor = (OffsetDateTime, Uuid);

#[derive(Debug, Clone)]
pub struct ListFilter {
    pub status: Option<InvestigationStatus>,
    pub cursor: Option<Cursor>,
    pub limit: i64,
}

impl Default for ListFilter {
    fn default() -> Self {
        Self {
            status: None,
            cursor: None,
            limit: 50,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InvestigationPage {
    pub items: Vec<Investigation>,
    pub next_cursor: Option<Cursor>,
}

#[async_trait]
pub trait Store: Send + Sync {
    // --- Investigations ---
    async fn create_investigation(&self, inv: &Investigation) -> Result<(), StoreError>;
    async fn get_investigation(&self, id: Uuid) -> Result<Investigation, StoreError>;
    async fn list_investigations(
        &self,
        filter: &ListFilter,
    ) -> Result<InvestigationPage, StoreError>;

    /// `queued` → `running`. `Conflict` if it is already in another state.
    async fn mark_running(&self, id: Uuid) -> Result<(), StoreError>;

    /// Boot cleanup: every `running` becomes `failed`. Returns how many were cleaned.
    async fn fail_orphaned_running(&self, reason: &TerminalReason) -> Result<u64, StoreError>;

    /// **`idle_for` is interpreted by the database** — `WHERE updated_at < now() - $1::interval`.
    /// Having the caller compute the threshold on the app clock would compare two clocks
    /// against an `updated_at` written on the DB clock, reclaiming investigations early
    /// whenever the app clock runs ahead. This closes an item plan 1 deferred.
    async fn stale_running_ids(&self, idle_for: Duration) -> Result<Vec<Uuid>, StoreError>;

    /// Queue restoration at boot.
    async fn queued_ids(&self) -> Result<Vec<Uuid>, StoreError>;

    // --- Steps ---
    /// **`seq` is allocated by the database.** The crux is that the caller does not pass
    /// a `seq` — allowing it would let the watchdog and the live task each compute one and
    /// collide (spec Section 6.1.1). Returns the allocated `seq`.
    ///
    /// Also refreshes `investigations.updated_at` in the same transaction —
    /// the INV-4 watchdog uses that value to judge whether it has stalled.
    async fn append_step(
        &self,
        investigation_id: Uuid,
        phase: Phase,
        kind: &StepKind,
    ) -> Result<i64, StoreError>;

    async fn steps_after(&self, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError>;

    /// The `after` value the investigation detail page embeds in the stream URL (INV-2).
    async fn max_step_seq(&self, id: Uuid) -> Result<Option<i64>, StoreError>;

    // --- Instructions ---
    /// **Must use `ORDER BY position, title`.** Without the ordering the prompt cache breaks.
    async fn instructions_for(&self, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError>;
    async fn upsert_instruction(&self, ins: &Instruction) -> Result<(), StoreError>;
    /// **Selects the update target by `id`.** `upsert_instruction` selects its target by a
    /// `(phase, title)` conflict and silently discards the caller's `id` (intended behavior —
    /// plan 1, `upsert_preserves_original_id_on_conflict`), so it cannot be used for a call
    /// like `PUT /{id}` that means "change the row with this id". `NotFound` if the `id`
    /// does not exist, `Conflict` if the updated values collide with **another** row's
    /// `(phase, title)`.
    async fn update_instruction(&self, ins: &Instruction) -> Result<(), StoreError>;
    async fn delete_instruction(&self, id: Uuid) -> Result<(), StoreError>;

    // --- Artifacts ---
    async fn get_artifact(&self, id: Uuid) -> Result<Artifact, StoreError>;
    async fn list_artifacts(&self, limit: i64) -> Result<Vec<Artifact>, StoreError>;

    // --- Termination (transactional) ---
    /// Saving the artifact, the `ArtifactWritten` step, and `running` → `completed` in
    /// one transaction. No partial success survives (design spec, Section 6.1).
    ///
    /// There is no `seq` parameter — the database allocates it inside the transaction.
    /// Letting the caller pass one would collide with the watchdog and lose the terminal step (spec Section 6.1.1).
    async fn complete_investigation(
        &self,
        id: Uuid,
        artifact: &NewArtifact,
    ) -> Result<Uuid, StoreError>;

    /// The `Terminated` step and `running` → `failed` in one transaction.
    async fn fail_investigation(&self, id: Uuid, reason: &TerminalReason)
        -> Result<(), StoreError>;

    // --- Chat ---
    async fn create_chat_session(&self, s: &ChatSession) -> Result<(), StoreError>;
    async fn list_chat_sessions(&self, limit: i64) -> Result<Vec<ChatSession>, StoreError>;
    async fn chat_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError>;

    /// The store allocates `seq` atomically. Chat has two writers, so computing
    /// `MAX(seq)+1` in the application races (design spec, Section 6.1.1).
    async fn append_chat_message(
        &self,
        session_id: Uuid,
        role: ChatRole,
        content: &serde_json::Value,
    ) -> Result<i64, StoreError>;

    // --- MCP servers and tool policy ---
    /// Returns only `enabled` servers, **ordered by name**. An unstable order makes the
    /// `tools[]` order unstable and the prompt cache misses every time (design spec, Section 9).
    async fn enabled_mcp_servers(&self) -> Result<Vec<McpServer>, StoreError>;

    /// Returns them ordered by tool name.
    async fn tool_policies_for(&self, server_name: &str) -> Result<Vec<ToolPolicy>, StoreError>;

    /// **No row means `Deny`.** The default for a newly discovered tool is not represented
    /// as a database row — reading absence as deny is what keeps a missing registration from becoming exposure (Section 9.1).
    async fn tool_policy(
        &self,
        server_name: &str,
        tool_name: &str,
    ) -> Result<ToolPolicyKind, StoreError>;

    async fn upsert_tool_policy(&self, policy: &ToolPolicy) -> Result<(), StoreError>;
}

/// The MCP tool registry. Implemented in plan 2.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    async fn list(&self) -> Result<Vec<ToolDef>, ToolError>;
    async fn call(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDef {
    /// The `{server}__{tool}` namespace. Serialization sorts by this name.
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}

/// The investigation execution scheduler. Implemented in plan 3.
#[async_trait]
pub trait JobManager: Send + Sync {
    async fn enqueue(&self, investigation_id: Uuid) -> Result<(), JobError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// If the trait were not object-safe, this function would not compile.
    /// Compilation itself is the test, not a runtime assertion.
    #[test]
    fn traits_are_object_safe() {
        fn assert_store(_: Arc<dyn Store>) {}
        fn assert_tools(_: Arc<dyn ToolRegistry>) {}
        fn assert_jobs(_: Arc<dyn JobManager>) {}
        let _ = assert_store;
        let _ = assert_tools;
        let _ = assert_jobs;
    }

    /// A value crossing an `.await` boundary must be `Send + 'static`.
    #[test]
    fn error_types_are_send_static() {
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<crate::error::StoreError>();
        assert_send_static::<crate::error::ToolError>();
        assert_send_static::<crate::error::JobError>();
    }
}

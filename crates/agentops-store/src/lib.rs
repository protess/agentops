//! The Postgres implementation of the `Store` trait.

use agentops_core::{
    AgentStep, Artifact, ChatMessage, ChatRole, ChatSession, Instruction, Investigation,
    InvestigationPage, ListFilter, McpServer, NewArtifact, Phase, StepKind, Store, StoreError,
    TerminalReason, ToolPolicy, ToolPolicyKind,
};
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

mod artifacts;
mod chat;
mod instructions;
mod investigations;
mod mcp;
mod steps;

/// Maps an sqlx error onto `StoreError`.
///
/// **Raises `RowNotFound` to `NotFound`.** In plan 1 every call site used
/// `fetch_optional` or an aggregate and never reached this path, so flattening was harmless.
/// Plans 2 and 3 add call sites, so this prevents the `NotFound` contract from
/// silently collapsing into `Backend(String)` the moment a single `fetch_one` appears.
pub(crate) fn backend(e: sqlx::Error) -> StoreError {
    match e {
        sqlx::Error::RowNotFound => StoreError::NotFound,
        other => StoreError::Backend(other.to_string()),
    }
}

#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl Store for PgStore {
    async fn create_investigation(&self, inv: &Investigation) -> Result<(), StoreError> {
        investigations::create(&self.pool, inv).await
    }

    async fn get_investigation(&self, id: Uuid) -> Result<Investigation, StoreError> {
        investigations::get(&self.pool, id).await
    }

    async fn list_investigations(&self, f: &ListFilter) -> Result<InvestigationPage, StoreError> {
        investigations::list(&self.pool, f).await
    }

    async fn mark_running(&self, id: Uuid) -> Result<(), StoreError> {
        investigations::mark_running(&self.pool, id).await
    }

    async fn fail_orphaned_running(&self, reason: &TerminalReason) -> Result<u64, StoreError> {
        // Lives in the steps module because it inserts a terminal step (Task 8)
        steps::fail_orphaned_running(&self.pool, reason).await
    }

    async fn stale_running_ids(
        &self,
        idle_for: std::time::Duration,
    ) -> Result<Vec<Uuid>, StoreError> {
        investigations::stale_running_ids(&self.pool, idle_for).await
    }

    async fn queued_ids(&self) -> Result<Vec<Uuid>, StoreError> {
        investigations::queued_ids(&self.pool).await
    }

    async fn append_step(
        &self,
        investigation_id: Uuid,
        phase: Phase,
        kind: &StepKind,
    ) -> Result<i64, StoreError> {
        steps::append(&self.pool, investigation_id, phase, kind).await
    }

    async fn steps_after(&self, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError> {
        steps::after(&self.pool, id, after_seq).await
    }

    async fn max_step_seq(&self, id: Uuid) -> Result<Option<i64>, StoreError> {
        steps::max_seq(&self.pool, id).await
    }

    async fn instructions_for(&self, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError> {
        instructions::for_phases(&self.pool, phases).await
    }

    async fn upsert_instruction(&self, ins: &Instruction) -> Result<(), StoreError> {
        instructions::upsert(&self.pool, ins).await
    }

    async fn update_instruction(&self, ins: &Instruction) -> Result<(), StoreError> {
        instructions::update(&self.pool, ins).await
    }

    async fn delete_instruction(&self, id: Uuid) -> Result<(), StoreError> {
        instructions::delete(&self.pool, id).await
    }

    async fn get_artifact(&self, id: Uuid) -> Result<Artifact, StoreError> {
        artifacts::get(&self.pool, id).await
    }

    async fn list_artifacts(&self, limit: i64) -> Result<Vec<Artifact>, StoreError> {
        artifacts::list(&self.pool, limit).await
    }

    async fn complete_investigation(
        &self,
        id: Uuid,
        artifact: &NewArtifact,
    ) -> Result<Uuid, StoreError> {
        artifacts::complete_investigation(&self.pool, id, artifact).await
    }

    async fn fail_investigation(
        &self,
        id: Uuid,
        reason: &TerminalReason,
    ) -> Result<(), StoreError> {
        artifacts::fail_investigation(&self.pool, id, reason).await
    }

    async fn create_chat_session(&self, s: &ChatSession) -> Result<(), StoreError> {
        chat::create_session(&self.pool, s).await
    }

    async fn list_chat_sessions(&self, limit: i64) -> Result<Vec<ChatSession>, StoreError> {
        chat::list_sessions(&self.pool, limit).await
    }

    async fn chat_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError> {
        chat::messages(&self.pool, session_id).await
    }

    async fn append_chat_message(
        &self,
        session_id: Uuid,
        role: ChatRole,
        content: &serde_json::Value,
    ) -> Result<i64, StoreError> {
        chat::append_message(&self.pool, session_id, role, content).await
    }

    async fn enabled_mcp_servers(&self) -> Result<Vec<McpServer>, StoreError> {
        mcp::enabled_servers(&self.pool).await
    }

    async fn tool_policies_for(&self, server_name: &str) -> Result<Vec<ToolPolicy>, StoreError> {
        mcp::policies_for(&self.pool, server_name).await
    }

    async fn tool_policy(
        &self,
        server_name: &str,
        tool_name: &str,
    ) -> Result<ToolPolicyKind, StoreError> {
        mcp::policy(&self.pool, server_name, tool_name).await
    }

    async fn upsert_tool_policy(&self, policy: &ToolPolicy) -> Result<(), StoreError> {
        mcp::upsert_policy(&self.pool, policy).await
    }
}

//! Instructions CRUD — the fragment the Knowledge page uses (Task 11).
//!
//! **Never use `|safe` on a value originating from the LLM or a tool** (spec Section 13).
//! `body` is user input that goes into the system prompt, so it is always escaped in the
//! list — that is askama's default behavior, and this does not depart from it.

use crate::AppState;
use agentops_core::{Instruction, Phase, Store};
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Form,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct InstructionForm {
    pub phase: String,
    pub position: i32,
    pub title: String,
    pub body: String,
}

#[derive(Template)]
#[template(path = "instruction_list.html")]
pub struct InstructionList {
    pub items: Vec<Instruction>,
}

/// **Every Phase is named explicitly.** `instructions_for(&[])` returns an empty list,
/// which would leave the UI empty while instructions still go into the prompt.
const ALL_PHASES: [Phase; 5] = [
    Phase::All,
    Phase::Chat,
    Phase::Triage,
    Phase::Rca,
    Phase::Mitigation,
];

pub async fn list_fragment(State(st): State<AppState>) -> impl IntoResponse {
    // Failures are handled **the same way** as Task 9's investigation list. Silently
    // returning an empty list would make "there are no instructions" and "the database is down" indistinguishable on screen.
    let items = match st.store.instructions_for(&ALL_PHASES).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "instruction list failed");
            Vec::new()
        }
    };
    Html(InstructionList { items }.render().expect("template render"))
}

/// The form-to-domain conversion `create` and `update` share. **The store call is not
/// shared** — see `update`'s comment below.
fn form_to_instruction(id: Uuid, f: InstructionForm) -> Result<Instruction, StatusCode> {
    let phase: Phase = f.phase.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    // Plan 1's actual fields: beyond id, phase, position, title, and body there are
    // `enabled: bool` and `updated_at: OffsetDateTime`. `enabled` is left
    // `true` — with `false` it appears in the UI while being absent from the prompt, and
    // that mismatch surfaces only in the LLM's output.
    Ok(Instruction {
        id,
        phase,
        position: f.position,
        title: f.title,
        body: f.body,
        enabled: true,
        updated_at: time::OffsetDateTime::now_utc(),
    })
}

pub async fn create(
    State(st): State<AppState>,
    Form(f): Form<InstructionForm>,
) -> Result<StatusCode, StatusCode> {
    let ins = form_to_instruction(Uuid::new_v4(), f)?;
    st.store.upsert_instruction(&ins).await.map_err(|e| {
        tracing::error!(error = %e, "instruction upsert failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::CREATED)
}

/// `PUT /api/instructions/{id}` — updates **exactly the row** the path's `id` points at.
/// **It does not use `upsert_instruction`** — that selects its update target by a
/// `(phase, title)` conflict and silently discards the caller's `id` (plan 1's intended
/// behavior, `upsert_preserves_original_id_on_conflict`). An earlier round wrongly assumed
/// this function "upserts by id" and reused the same upsert as `create`, which meant that
/// when another row already held the submitted `(phase, title)`, **that other row** was
/// silently overwritten rather than the one the path pointed at — and the response still
/// reported success (task-11-review.md Critical, pinned as a regression by
/// `put_never_rewrites_a_different_row`). `Store::update_instruction` fixes the target
/// with `WHERE id = $1` and has no such trap: `NotFound` if the `id` does not exist,
/// `Conflict` if the submitted `(phase, title)` collides with another row.
pub async fn update(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
    Form(f): Form<InstructionForm>,
) -> Result<StatusCode, StatusCode> {
    let ins = form_to_instruction(id, f)?;
    st.store
        .update_instruction(&ins)
        .await
        .map_err(|e| match e {
            agentops_core::StoreError::NotFound => StatusCode::NOT_FOUND,
            agentops_core::StoreError::Conflict => StatusCode::CONFLICT,
            e => {
                tracing::error!(error = %e, "instruction update failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    st.store.delete_instruction(id).await.map_err(|e| {
        if matches!(e, agentops_core::StoreError::NotFound) {
            StatusCode::NOT_FOUND
        } else {
            tracing::error!(error = %e, "instruction delete failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;
    Ok(StatusCode::NO_CONTENT)
}

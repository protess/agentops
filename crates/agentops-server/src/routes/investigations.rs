//! Investigation launch and the list fragment (spec Section 10.2).
//!
//! **The handler does not wait for the investigation to run** (INV-1). `create` makes the
//! row, puts a task on `JobManager::spawn`, and sends a `303 See Other` to the detail page
//! immediately. Returning HTML with a `200` would resubmit the form on refresh and create
//! a duplicate investigation.

use crate::AppState;
use agentops_core::{Investigation, InvestigationStatus, ListFilter, Store, TriggeredBy};
use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateForm {
    pub prompt: String,
}

#[derive(Template)]
#[template(path = "investigation_list.html")]
pub struct ListFragment {
    pub items: Vec<Investigation>,
}

/// Spec Section 10.2 — **`303 See Other`** after launching. The handler does not run the investigation.
pub async fn create(
    State(st): State<AppState>,
    Form(f): Form<CreateForm>,
) -> Result<Redirect, StatusCode> {
    let prompt = f.prompt.trim();
    if prompt.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let now = OffsetDateTime::now_utc();
    let id = Uuid::new_v4();
    let inv = Investigation {
        id,
        // The title comes from the beginning of the prompt. Cut on a character boundary —
        // cutting on bytes panics on a multi-byte character.
        title: prompt.chars().take(80).collect(),
        prompt: prompt.to_string(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    };
    st.store.create_investigation(&inv).await.map_err(|e| {
        tracing::error!(error = %e, "investigation insert failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // INV-1 — spawn and return immediately. A request arriving in the shutdown window (the
    // gate is closed but investigations are not yet being refused) is rejected with
    // `GateClosed`, and since the row already exists it sits quietly until the next boot's
    // `recover_on_boot` requeue handles it — at minimum a log is left so the cause can be
    // traced (this is the brief's pseudo-code behavior, so the response is not changed
    // here — a final review triage item).
    if let Err(e) = st.jobs.spawn(id) {
        tracing::error!(investigation = %id, error = %e, "failed to spawn investigation after creating it");
    }
    Ok(Redirect::to(&format!("/investigations/{id}")))
}

/// **`ListFilter` has no search field.** Spec Section 10.2 requires search, a status
/// filter, and pagination on the list fragment, but the store does not support search.
/// This task does not implement search and records that fact — changing the store
/// contract is plan 1's territory, and quietly faking it with a client-side filter here
/// produces wrong results once combined with pagination. A final review triage item.
///
pub async fn list_fragment(State(st): State<AppState>) -> impl IntoResponse {
    // `InvestigationPage` does not implement `Default` (plan 1 derives only
    // `#[derive(Debug, Clone)]`). `unwrap_or_default()` would not compile, so it falls
    // back to an empty list explicitly.
    let items = match st.store.list_investigations(&ListFilter::default()).await {
        Ok(page) => page.items,
        Err(e) => {
            tracing::error!(error = %e, "investigation list failed");
            Vec::new()
        }
    };
    Html(ListFragment { items }.render().expect("template render"))
}

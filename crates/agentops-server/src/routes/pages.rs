//! HTML page handlers — they render the three-pane layout of spec Section 4.2.
//!
//! **Never use `|safe` on a value originating from the LLM or a tool** (spec Section 13,
//! Global Constraints). `chart_svg` is the exception — it is SVG we generated.

use crate::AppState;
use agentops_core::{Artifact, McpServer, Store};
use askama::Template;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect};
use uuid::Uuid;

#[derive(Template)]
#[template(path = "incidents.html")]
pub struct IncidentsPage {
    pub chart_svg: String,
}

/// `/` redirects to `/incidents` with a 303 See Other (spec Section 10.1).
pub async fn index() -> impl IntoResponse {
    Redirect::to("/incidents")
}

pub async fn incidents(
    axum::extract::State(st): axum::extract::State<AppState>,
) -> impl IntoResponse {
    // **Cut by a calendar window, not a 168-hour clock window.** `chart::daily_bars` takes
    // exactly `BARS` calendar dates with `days_ago in 0..=BARS-1` from "today". `now()` is
    // effectively never midnight, so `queued_at >= now() - INTERVAL '7 days'` returns rows
    // dated `today - 7 days` on almost every day (from that day's midnight to that time).
    // By `daily_bars`'s reckoning such a row is `days_ago == BARS`, so it is discarded as
    // outside the window and a warning fires daily — the cause was the two windows
    // measuring on different bases (clock versus date), which the review caught by
    // measurement. Comparing on `queued_at::date` aligns it to calendar dates.
    //
    // **The day count is not written as a literal twice.** The first fix hardcoded
    // `INTERVAL '6 days'` into the SQL, and the third review pointed out that this is a
    // literal separate from `chart::BARS` and could drift apart again — a shape (two
    // literals meaning the same value) this task had already been caught by twice.
    // Binding a value derived from `chart::BARS` through `make_interval(days => $1)` makes
    // both windows come from the same value without assembling a SQL string dynamically
    // (this repository's sqlx blocks that at compile time — `stale_running_ids` uses
    // `secs => $1` for the same reason).
    let window_days = crate::chart::BARS as i32 - 1;
    let rows: Vec<(time::Date, i64)> = sqlx::query_as(
        "SELECT queued_at::date AS d, count(*) FROM investigations
          WHERE queued_at::date >= (now()::date - make_interval(days => $1))::date
          GROUP BY d ORDER BY d",
    )
    .bind(window_days)
    .fetch_all(st.store.pool())
    .await
    .unwrap_or_else(|e| {
        // **Never swallow it silently.** An empty chart and a dead database are
        // indistinguishable on screen, so the log is the only clue.
        tracing::error!(error = %e, "chart query failed");
        Vec::new()
    });
    let counts: Vec<(time::Date, u32)> = rows.into_iter().map(|(d, c)| (d, c as u32)).collect();
    let page = IncidentsPage {
        chart_svg: crate::chart::daily_bars(&counts),
    };
    // askama validates templates at compile time — a runtime render failure is a bug, not
    // a normal runtime condition.
    Html(page.render().expect("template render"))
}

#[derive(Template)]
#[template(path = "investigation.html")]
pub struct InvestigationPage {
    pub inv: agentops_core::Investigation,
    /// INV-2 — the value embedded in the stream URL. -1 when there are no steps.
    pub after: i64,
    pub steps_html: String,
}

/// `/investigations/{id}` — the detail page. It server-renders existing steps with
/// `render::step_html` and embeds that list's last `seq` as the stream URL's `after`
/// (INV-2).
///
/// **It does not call `Store::max_step_seq` separately.** That would open a window in
/// which a new step could commit between the step query and that one — the step would be
/// absent from the page while `after` had already passed it, so a new stream would not
/// replay it. Taking the last `seq` directly from the rendered list closes that window.
pub async fn investigation_detail(
    axum::extract::State(st): axum::extract::State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, StatusCode> {
    let inv = st.store.get_investigation(id).await.map_err(|e| {
        // `NotFound` is a genuine 404. Collapsing other errors (`Backend`, a database
        // failure) into 404 here too would make an investigation that exists look missing
        // while the database is briefly unstable, leaving no clue in the log — the
        // on-call engineer sees "the investigation vanished" with nowhere to read the
        // cause. This matches the `steps_after` handling four lines below: anything other
        // 500.
        if matches!(e, agentops_core::StoreError::NotFound) {
            StatusCode::NOT_FOUND
        } else {
            tracing::error!(error = %e, "investigation fetch failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;
    let steps = st.store.steps_after(id, -1).await.map_err(|e| {
        tracing::error!(error = %e, "step fetch failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let after = steps.last().map_or(-1, |s| s.seq);
    let steps_html = steps
        .iter()
        .map(crate::render::step_html)
        .collect::<String>();
    Ok(Html(
        InvestigationPage {
            inv,
            after,
            steps_html,
        }
        .render()
        .expect("template render"),
    ))
}

#[derive(Template)]
#[template(path = "knowledge.html")]
pub struct KnowledgePage;

/// `/knowledge` — the instruction management page. `#list` fetches the list fragment
/// itself with `hx-get /api/instructions` (`routes::instructions::list_fragment`).
pub async fn knowledge() -> impl IntoResponse {
    Html(KnowledgePage.render().expect("template render"))
}

#[derive(Template)]
#[template(path = "artifacts.html")]
pub struct ArtifactsPage {
    pub items: Vec<Artifact>,
}

/// `/artifacts` — the list is rendered directly on the server. Unlike the investigations
/// list fragment it does not refresh often, so there is no reason to split it into a separate htmx fragment.
pub async fn artifacts(
    axum::extract::State(st): axum::extract::State<AppState>,
) -> impl IntoResponse {
    // Failures are handled the same way as the investigation and instruction lists: not
    // swallowed silently, logged, then continued with an empty list — this fragment's
    // failure does not turn the whole page into a 500.
    let items = match st.store.list_artifacts(50).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "artifact list failed");
            Vec::new()
        }
    };
    Html(ArtifactsPage { items }.render().expect("template render"))
}

#[derive(Template)]
#[template(path = "artifact.html")]
pub struct ArtifactPage {
    pub artifact: Artifact,
}

/// `/artifacts/{id}` — the artifact body page. `NotFound` is a 404 and any other error (a
/// database failure) is logged then 500 — the same convention as `investigation_detail`.
pub async fn artifact_detail(
    axum::extract::State(st): axum::extract::State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, StatusCode> {
    let artifact = st.store.get_artifact(id).await.map_err(|e| {
        if matches!(e, agentops_core::StoreError::NotFound) {
            StatusCode::NOT_FOUND
        } else {
            tracing::error!(error = %e, "artifact fetch failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;
    Ok(Html(
        ArtifactPage { artifact }.render().expect("template render"),
    ))
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsPage {
    pub servers: Vec<McpServer>,
}

/// `/settings` — shows the MCP servers **read-only** (spec Section 10.1).
/// v0.1 has no write form — configuration changes are out of scope.
pub async fn settings(
    axum::extract::State(st): axum::extract::State<AppState>,
) -> impl IntoResponse {
    let servers = match st.store.enabled_mcp_servers().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "mcp server list failed");
            Vec::new()
        }
    };
    Html(SettingsPage { servers }.render().expect("template render"))
}

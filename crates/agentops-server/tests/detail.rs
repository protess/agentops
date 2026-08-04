//! The investigation detail page (Task 10, INV-2) — it server-renders existing steps and
//! embeds the last `seq` in the stream URL.

use agentops_agent::limits::Limits;
use agentops_core::{
    BoxStream, Investigation, InvestigationStatus, LlmError, LlmEvent, LlmProvider, LlmRequest,
    Phase, StepKind, Store, TriggeredBy,
};
use agentops_server::bus::StepBus;
use agentops_server::jobs::{JobDeps, JobManager};
use agentops_server::routes;
use agentops_server::AppState;
use agentops_store::PgStore;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::Router;
use std::sync::Arc;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

/// This test file spawns no investigation, so `stream()` is never called — a placeholder
/// needed only to assemble `JobManager`.
struct NullProvider;

#[async_trait]
impl LlmProvider for NullProvider {
    fn model_id(&self) -> &str {
        "null"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
}

fn test_app(pool: sqlx::PgPool) -> Router {
    let store = Arc::new(PgStore::new(pool));
    let bus = StepBus::new();
    let jobs = JobManager::new(
        store.clone(),
        bus.clone(),
        JobDeps {
            provider: Arc::new(NullProvider),
            connections: Vec::new(),
            limits: Limits::default(),
        },
    );
    let state = AppState { store, bus, jobs };
    routes::router(state)
}

async fn body_string(res: Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn running(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: "t".into(),
        prompt: "p".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    };
    store.create_investigation(&inv).await.unwrap();
    store.mark_running(inv.id).await.unwrap();
    inv.id
}

/// INV-2 — the detail page **server-renders existing steps and embeds the last `seq` in
/// the stream URL.** Without it, a client on refresh does not know `after`, and
/// `Last-Event-ID` does not arrive because it is a new `EventSource` — replay becomes
/// impossible in precisely the situation that needs it.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_2_detail_page_renders_steps_and_embeds_the_last_seq(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let id = running(&store).await;
    for n in 0..3 {
        store
            .append_step(
                id,
                Phase::Triage,
                &StepKind::Text {
                    text: format!("body{n}"),
                },
            )
            .await
            .unwrap();
    }

    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri(format!("/investigations/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;

    // Existing steps are rendered on the server.
    for n in 0..3 {
        assert!(
            body.contains(&format!("body{n}")),
            "step {n} was not rendered"
        );
    }
    // The last seq is embedded in the stream URL.
    assert!(
        body.contains(&format!("/api/investigations/{id}/stream?after=2")),
        "after=2 was not embedded in the stream URL"
    );
}

/// With no steps at all it embeds `after=-1`. Embedding `after=0` would hide the first
/// step (`seq=0`) forever — `steps_after` uses `seq > after`.
#[sqlx::test(migrations = "../../migrations")]
async fn an_investigation_with_no_steps_embeds_after_minus_one(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let id = running(&store).await;
    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri(format!("/investigations/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(
        body.contains("after=-1"),
        "with no steps it must be after=-1 to receive the first step"
    );
}

/// The detail page and SSE use **the same render function**. Rendering separately would
/// make the screen differ across a refresh, and that difference surfaces only on the replay path.
#[sqlx::test(migrations = "../../migrations")]
async fn the_page_and_the_stream_render_steps_identically(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let id = running(&store).await;
    store
        .append_step(
            id,
            Phase::Rca,
            &StepKind::Text {
                text: "shared".into(),
            },
        )
        .await
        .unwrap();
    let step = store.steps_after(id, -1).await.unwrap().remove(0);
    let direct = agentops_server::render::step_html(&step);

    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri(format!("/investigations/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(
        body.contains(&direct),
        "the page did not use render::step_html"
    );
}

/// A missing investigation is a 404. A 500 or an empty page would make a typo'd URL look like a server error.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_investigation_is_not_found(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/investigations/{}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

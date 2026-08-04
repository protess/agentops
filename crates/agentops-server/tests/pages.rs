use agentops_agent::limits::Limits;
use agentops_core::{
    BoxStream, Investigation, InvestigationStatus, LlmError, LlmEvent, LlmProvider, LlmRequest,
    Store, TriggeredBy,
};
use agentops_server::bus::StepBus;
use agentops_server::jobs::{JobDeps, JobManager};
use agentops_server::routes;
use agentops_server::AppState;
use agentops_store::PgStore;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use std::sync::Arc;
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

/// `/` redirects to `/incidents` (spec Section 10.1).
#[sqlx::test(migrations = "../../migrations")]
async fn root_redirects_to_incidents(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers()["location"], "/incidents");
}

/// Static assets come from our server, not a CDN (Global Constraints).
/// It fails if the page has a `<script src>` pointing at an external host.
#[sqlx::test(migrations = "../../migrations")]
async fn pages_do_not_reference_any_cdn(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/incidents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    for host in [
        "unpkg.com",
        "cdn.jsdelivr.net",
        "cdnjs.cloudflare.com",
        "cdn.tailwindcss.com",
    ] {
        assert!(!body.contains(host), "the page references a CDN ({host})");
    }
    assert!(
        body.contains("/static/htmx.min.js"),
        "it does not load the vendored htmx"
    );
}

/// Static files really are served.
#[sqlx::test(migrations = "../../migrations")]
async fn static_assets_are_served(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/static/htmx.min.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

async fn seed_investigation_at(store: &PgStore, queued_at: time::OffsetDateTime) -> Uuid {
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: "boundary".into(),
        prompt: "boundary".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at,
        started_at: None,
        finished_at: None,
        updated_at: queued_at,
    };
    store.create_investigation(&inv).await.unwrap();
    inv.id
}

fn chart_bar_heights(body: &str) -> Vec<f64> {
    let start = body.find("<svg").expect("the chart svg is missing");
    let end = body[start..]
        .find("</svg>")
        .expect("the svg was not closed")
        + start;
    let svg = &body[start..end];
    svg.split("height=\"")
        .skip(1)
        .map(|cap| cap.split('"').next().unwrap().parse().unwrap())
        .collect()
}

/// Review Important 2 — `pages::incidents`'s SQL was narrowed from a clock basis
/// (`now() - INTERVAL '7 days'`) to a calendar basis
/// (`queued_at::date >= now()::date - INTERVAL '6 days'`). This measures whether an
/// investigation seeded on the window's oldest day (six days ago) appears in exactly the
/// first slot of the real HTTP response — that is, whether the SQL window and
/// `chart::daily_bars`'s calendar window really agree.
///
/// Mutation check: narrowing the SQL's `INTERVAL '6 days'` to `'5 days'` (a window one
/// day shorter) was confirmed to fail this test — see the report below.
/// (A mutation reverting to the clock basis `now() - INTERVAL '7 days'` is not caught
/// here — six days ago is always included by both expressions, so there is no observable
/// difference. The clock/calendar mismatch actually surfaces at exactly the seven-day
/// boundary, and there `daily_bars`'s defensive drop makes the HTTP response identical in
/// both cases, leaving the log as the only observable difference — which
/// `daily_bars_does_not_warn_at_the_window_boundary_but_does_just_outside`
/// the `chart.rs` unit test catches directly.)
#[sqlx::test(migrations = "../../migrations")]
async fn the_chart_shows_investigations_from_the_oldest_day_in_the_window(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let six_days_ago = time::OffsetDateTime::now_utc() - time::Duration::days(6);
    seed_investigation_at(&store, six_days_ago).await;

    let app = test_app(pool);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/incidents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    let heights = chart_bar_heights(&body);
    assert_eq!(heights.len(), 7, "there are not 7 bars");
    assert!(
        heights[0] > 0.0,
        "the investigation seeded on the window's oldest day (six days ago) is not in the first slot: {heights:?}"
    );
    for (i, h) in heights.iter().enumerate().skip(1) {
        assert_eq!(*h, 0.0, "a value leaked into another slot {i}: {heights:?}");
    }
}

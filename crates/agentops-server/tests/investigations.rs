//! Investigation launch and the list fragment (Task 9, spec Section 10.2).

use agentops_agent::limits::Limits;
use agentops_core::{
    BoxStream, Investigation, InvestigationStatus, ListFilter, LlmError, LlmEvent, LlmProvider,
    LlmRequest, Store, TriggeredBy,
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
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

/// A minimal `Write`/`MakeWriter` adapter that captures `tracing` output into a buffer.
/// The same pattern the tests in `routes/health.rs` use — needed to assert on log content
/// directly rather than probe for presence.
#[derive(Clone, Default)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BufWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Terminates cleanly at once with a single text block. Used by `test_app` — most tests
/// do not care how the investigation actually runs.
struct OkProvider;

#[async_trait]
impl LlmProvider for OkProvider {
    fn model_id(&self) -> &str {
        "ok"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(LlmEvent::TextBlock {
                text: "finding".into(),
            }),
            Ok(LlmEvent::Stopped {
                reason: agentops_core::StopReason::EndTurn,
                usage: agentops_core::Usage::default(),
                refusal_category: None,
            }),
        ])))
    }
}

/// Returns a stream that never ends. For verifying INV-1 — a handler that waited for this
/// investigation to finish would make that test time out.
struct SlowProvider;

#[async_trait]
impl LlmProvider for SlowProvider {
    fn model_id(&self) -> &str {
        "slow"
    }
    async fn stream(
        &self,
        _req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}

fn job_deps(provider: Arc<dyn LlmProvider>) -> JobDeps {
    JobDeps {
        provider,
        connections: Vec::new(),
        limits: Limits::default(),
    }
}

fn app_with_provider(pool: sqlx::PgPool, provider: Arc<dyn LlmProvider>) -> Router {
    let store = Arc::new(PgStore::new(pool));
    let bus = StepBus::new();
    let jobs = JobManager::new(store.clone(), bus.clone(), job_deps(provider));
    let state = AppState { store, bus, jobs };
    routes::router(state)
}

fn test_app(pool: sqlx::PgPool) -> Router {
    app_with_provider(pool, Arc::new(OkProvider))
}

fn test_app_with_slow_provider(pool: sqlx::PgPool) -> Router {
    app_with_provider(pool, Arc::new(SlowProvider))
}

fn post_form(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/investigations")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
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

/// Seeds an investigation directly into the store with both prompt and title set to
/// `title` — checking whether the list fragment really escapes needs a row with the
/// desired title without going through the launch path.
async fn create_with_title(store: &PgStore, title: &str) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: title.into(),
        prompt: title.into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    };
    store.create_investigation(&inv).await.unwrap();
    inv.id
}

/// Launching sends a `303 See Other` to the detail page (spec Section 10.2).
/// Returning HTML with a `200` would resubmit the form on refresh and create a duplicate investigation.
#[sqlx::test(migrations = "../../migrations")]
async fn creating_an_investigation_redirects_with_303(pool: sqlx::PgPool) {
    let app = test_app(pool.clone());
    let res = app.oneshot(post_form("prompt=disk+is+full")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res.headers()["location"].to_str().unwrap().to_string();
    assert!(
        loc.starts_with("/investigations/"),
        "it must send to the detail page: {loc}"
    );

    // Confirm a row really was created. Passing on the redirect alone would not catch a
    // failed store.
    let store = PgStore::new(pool);
    let page = store
        .list_investigations(&ListFilter::default())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].prompt, "disk is full");
}

/// INV-1 — the launch request does not wait for the investigation to complete.
/// If the handler blocked, this test would time out.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_1_create_returns_while_the_investigation_is_still_queued_or_running(
    pool: sqlx::PgPool,
) {
    let app = test_app_with_slow_provider(pool.clone());
    let t0 = std::time::Instant::now();
    let res = app.oneshot(post_form("prompt=x")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(
        t0.elapsed() < std::time::Duration::from_millis(300),
        "the launch request waited for the investigation"
    );
}

/// An empty prompt is refused. Creating an empty investigation would run the LLM through three phases on empty input.
#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_prompt_is_rejected(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let res = app.oneshot(post_form("prompt=")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// The list fragment carries the status and title, and **escapes the title**.
/// The title is user input (derived from the prompt) and therefore untrusted (spec Section 13).
#[sqlx::test(migrations = "../../migrations")]
async fn the_list_fragment_escapes_user_supplied_text(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    create_with_title(&store, "<script>alert(1)</script>").await;
    let app = test_app(pool);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/investigations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(res).await;
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "the title was not escaped"
    );
    // askama 0.16 escapes `<` and `>` as numeric character references (`&#60;`/`&#62;`),
    // not the `&lt;`/`&gt;` the brief wrote — measured: this version uses numeric entities
    // rather than named ones. The safety is identical, but the assertion must match the
    // real output.
    assert!(
        body.contains("&#60;script&#62;"),
        "the escaped form is absent: {body}"
    );
}

/// The chart emits seven days of bars. The axis must appear even with no data —
/// returning an empty string would leave a hole in the page.
#[test]
fn the_chart_always_renders_seven_bars() {
    let svg = agentops_server::chart::daily_bars(&[]);
    assert!(svg.starts_with("<svg"), "it is not SVG");
    assert_eq!(svg.matches("<rect").count(), 7, "there are not 7 bars");
}

/// The largest value does not exceed the chart height. Without scaling a bar would spill
/// out of the viewBox and break the layout.
///
/// **It uses "today" rather than a fixed future date.** `daily_bars` now discards entries
/// outside the seven-day window measured from the wall clock
/// `OffsetDateTime::now_utc().date()` (the review Important 1 fix), so leaving the brief's
/// fixed literal `2026-08-01` in place would become a time bomb: the moment the real clock
/// passes seven days beyond that date the entry falls outside the window, every bar
/// becomes 0, and this assertion silently always passes while verifying no scaling at
/// all. Using "today" makes the property independent of time.
#[test]
fn bars_are_scaled_within_the_viewbox() {
    let today = time::OffsetDateTime::now_utc().date();
    let svg = agentops_server::chart::daily_bars(&[(today, 1000)]);
    for cap in svg.split("height=\"").skip(1) {
        let v: f64 = cap.split('"').next().unwrap().parse().unwrap();
        assert!(v <= 100.0, "bar height {v} exceeds the viewBox (100)");
    }
}

/// Review Important 1 — `daily_bars` must bucket **by date**, not fill by array position.
/// The `GROUP BY d` query gives no row at all for a day with no investigations (absence,
/// not a zero-count row), so this checks that with exactly two entries — six days ago and
/// today — each value lands in its own real date slot even with the five middle slots
/// empty. The previous implementation (filling by array position) drew today's value in
/// the "five days ago" slot — a defect the review caught by measurement.
///
/// Mutation check: reverting `daily_bars` to the old array-position code
/// (`counts.get(counts.len().saturating_sub(BARS) + i)`) was confirmed to fail this test —
/// see the report below.
#[test]
fn counts_are_bucketed_by_actual_date_not_array_position() {
    let today = time::OffsetDateTime::now_utc().date();
    let six_days_ago = today - time::Duration::days(6);
    let svg = agentops_server::chart::daily_bars(&[(six_days_ago, 5), (today, 9)]);

    let heights: Vec<f64> = svg
        .split("height=\"")
        .skip(1)
        .map(|cap| cap.split('"').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(heights.len(), 7, "there are not 7 bars");

    // Today (9, the maximum) must draw at the viewBox's full height in the last slot.
    assert!(
        (heights[6] - 100.0).abs() < 0.1,
        "today's value did not scale to the maximum in the last slot: {heights:?}"
    );
    // Six days ago (5) must scale to five ninths in the first slot.
    let expected_first = (5.0 / 9.0) * 100.0;
    assert!(
        (heights[0] - expected_first).abs() < 0.2,
        "the six-days-ago value is not in the first slot: {heights:?} (expected: {expected_first:.1})"
    );
    // The five middle slots (1..=5) are empty days and must be 0 — a shifted value would
    // be caught here.
    for (i, h) in heights.iter().enumerate().take(6).skip(1) {
        assert_eq!(
            *h, 0.0,
            "a value leaked into empty-day slot {i}: {heights:?}"
        );
    }
}

/// Review Important 2 — the SQL window (clock-based `now() - INTERVAL '7 days'`) and
/// `daily_bars`'s calendar window (`0..=6`) disagreed. `now()` is effectively never
/// midnight, so that query returned rows dated `today - 7 days` on almost every day, and
/// `daily_bars` computed those as `days_ago == 7`, discarded them as outside the window,
/// and emitted a `tracing::warn!` daily — the review's core point being that a warning on
/// healthy data cries wolf and loses its value as a signal.
///
/// `pages::incidents`'s query was narrowed to a calendar basis,
/// `queued_at::date >= now()::date - INTERVAL '6 days'`, aligning the two windows — the
/// SQL itself now returns only the seven dates `daily_bars` accepts, so `daily_bars` has
/// no occasion to warn on healthy data. This test pins that boundary directly at the
/// `daily_bars` level: the window's oldest day (six days ago) lands in the correct slot
/// without a warning, and just outside it (seven days ago) really does warn.
///
#[test]
fn daily_bars_does_not_warn_at_the_window_boundary_but_does_just_outside() {
    let today = OffsetDateTime::now_utc().date();
    let six_days_ago = today - time::Duration::days(6);
    let seven_days_ago = today - time::Duration::days(7);

    // The window's inside edge (six days ago) — no warning, and exactly in the first slot.
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_ansi(false)
        .finish();
    let svg = tracing::subscriber::with_default(subscriber, || {
        agentops_server::chart::daily_bars(&[(six_days_ago, 7)])
    });
    let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.is_empty(),
        "a warning fired at the window's inside boundary (six days ago): {log:?}"
    );
    let heights: Vec<f64> = svg
        .split("height=\"")
        .skip(1)
        .map(|cap| cap.split('"').next().unwrap().parse().unwrap())
        .collect();
    assert!(
        (heights[0] - 100.0).abs() < 0.1,
        "the six-days-ago value is not in the first slot: {heights:?}"
    );

    // Just outside the window (seven days ago) — genuinely outside, so it must warn.
    let buf2 = BufWriter::default();
    let subscriber2 = tracing_subscriber::fmt()
        .with_writer(buf2.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber2, || {
        agentops_server::chart::daily_bars(&[(seven_days_ago, 7)])
    });
    let log2 = String::from_utf8(buf2.0.lock().unwrap().clone()).unwrap();
    assert!(
        log2.contains("chart:"),
        "seven days ago is genuinely outside the window but no warning fired: {log2:?}"
    );
}

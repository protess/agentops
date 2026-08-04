//! Chat sessions, sending, and the token stream (Task 12, spec Section 6.2).

use agentops_agent::limits::Limits;
use agentops_core::{
    BoxStream, ChatRole, ChatSession, LlmError, LlmEvent, LlmProvider, LlmRequest, Store,
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

/// Terminates cleanly at once with a single text block. Used by `test_app`.
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
                text: "hi there".into(),
            }),
            Ok(LlmEvent::Stopped {
                reason: agentops_core::StopReason::EndTurn,
                usage: agentops_core::Usage::default(),
                refusal_category: None,
            }),
        ])))
    }
}

/// Returns a stream that never ends. Used to verify that response generation does not
/// hold the request (the same reason as INV-1).
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

fn state_with_provider(pool: sqlx::PgPool, provider: Arc<dyn LlmProvider>) -> AppState {
    let store = Arc::new(PgStore::new(pool));
    let bus = StepBus::new();
    let jobs = JobManager::new(store.clone(), bus.clone(), job_deps(provider));
    AppState { store, bus, jobs }
}

fn app_with_provider(pool: sqlx::PgPool, provider: Arc<dyn LlmProvider>) -> Router {
    routes::router(state_with_provider(pool, provider))
}

fn test_app(pool: sqlx::PgPool) -> Router {
    app_with_provider(pool, Arc::new(OkProvider))
}

fn test_app_with_slow_provider(pool: sqlx::PgPool) -> Router {
    app_with_provider(pool, Arc::new(SlowProvider))
}

/// An app with the gate already closed — it reproduces only stage 1 of
/// `JobManager::shutdown` (`close_gate()`) synchronously. It can be built
/// deterministically without a shutdown hook: `close_gate()` is a public method
/// `tests/jobs.rs` already calls directly.
fn test_app_with_closed_gate(pool: sqlx::PgPool) -> Router {
    let state = state_with_provider(pool, Arc::new(OkProvider));
    state.jobs.close_gate();
    routes::router(state)
}

fn post_form_to(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
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

async fn session(store: &PgStore) -> Uuid {
    let id = Uuid::new_v4();
    let now = OffsetDateTime::now_utc();
    store
        .create_chat_session(&ChatSession {
            id,
            title: "t".into(),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    id
}

fn step(id: Uuid, seq: i64) -> agentops_core::AgentStep {
    agentops_core::AgentStep {
        investigation_id: id,
        seq,
        phase: agentops_core::Phase::Triage,
        kind: agentops_core::StepKind::Text {
            text: format!("t{seq}"),
        },
        created_at: OffsetDateTime::now_utc(),
    }
}

/// Sending a message stores the user message **immediately**.
/// Storing it after waiting for the assistant response would lose what a user said if
/// they refreshed in between.
#[sqlx::test(migrations = "../../migrations")]
async fn the_user_message_is_persisted_before_the_reply_starts(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let sid = session(&store).await;
    let app = test_app_with_slow_provider(pool.clone());

    let res = app
        .oneshot(post_form_to(
            &format!("/api/chat/{sid}/messages"),
            "content=hello",
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let msgs = store.chat_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 1, "the user message was not stored immediately");
    assert_eq!(msgs[0].role, ChatRole::User);
}

/// Final review N-1 regression guard — the response generation task (the `reply()`
/// `send` spawns) must survive to the end **on its own**, not merely while `AppState`
/// (and therefore `Router`) is alive.
///
/// `send()` only spawns and returns immediately (the same reason as INV-1) — response
/// generation must keep going after the handler returns. This test verifies that by
/// dropping the `Router`. `app.oneshot(req)` takes `app` by value into a
/// `tower::util::Oneshot`, and that implementation (`tower-0.5.3`) replaces the prior
/// state holding `svc` with `State::Called{ fut }` **immediately after calling
/// `Service::call` on the first poll**, dropping the original `Router` (and therefore
/// `AppState` and `JobManager`) right there — long before the response completes,
/// effectively just after routing. So by the time this test receives `res`, the original
/// `AppState` clone is already dead, and the request-scoped clone the `send()` handler
/// borrowed dies with the handler's return. Unless the `reply()` task guarantees its own
/// lifetime through `JobManager::spawn_task` (which it did not before N-1),
/// `JoinSet::drop` aborts it immediately at this point and the assistant response is
/// never stored — this path was quiet in production only because axum's `Router` stays
/// alive for the server's lifetime, and this test reproduces the moment code relying on
/// that accident really dies.
#[sqlx::test(migrations = "../../migrations")]
async fn the_reply_task_survives_the_router_that_spawned_it_being_dropped(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let sid = session(&store).await;
    let app = test_app(pool.clone()); // `OkProvider` — a fast, deterministic "hi there" response.

    let res = app
        .oneshot(post_form_to(
            &format!("/api/chat/{sid}/messages"),
            "content=hello",
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Polls for up to 4 seconds — before the fix the assistant message never appears and
    // this loop always runs to the deadline.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    let msgs = loop {
        let msgs = store.chat_messages(sid).await.unwrap();
        if msgs.len() >= 2 || tokio::time::Instant::now() >= deadline {
            break msgs;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };

    assert_eq!(
        msgs.len(),
        2,
        "the assistant response was not stored after the router dropped — the reply() task appears to have been aborted (an N-1 regression)"
    );
    assert_eq!(msgs[0].role, ChatRole::User);
    assert_eq!(msgs[1].role, ChatRole::Assistant);
}

/// Section 6.2 — on reconnect it re-renders **only completed messages**.
/// A partial response in flight is lost. That is a deliberate trade-off, so the assertion
/// is "only completed ones arrive", not "the partial response survives".
#[sqlx::test(migrations = "../../migrations")]
async fn reconnecting_replays_only_completed_messages(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let sid = session(&store).await;
    store
        .append_chat_message(sid, ChatRole::User, &serde_json::json!({"text": "q"}))
        .await
        .unwrap();
    store
        .append_chat_message(sid, ChatRole::Assistant, &serde_json::json!({"text": "a"}))
        .await
        .unwrap();

    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri(format!("/api/chat/{sid}/panel"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(body.contains('q') && body.contains('a'));
}

/// The chat stream uses **a different channel** from the investigation stream.
/// Sharing one channel would make investigation deltas appear in the chat panel.
#[tokio::test]
async fn chat_and_investigation_channels_are_separate() {
    let bus = StepBus::new();
    let sid = Uuid::new_v4();
    let mut chat_rx = bus.subscribe_chat(sid);
    // Pushing an investigation step with the same UUID does not reach the chat subscriber.
    bus.publish_step(step(sid, 0));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), chat_rx.recv())
            .await
            .is_err(),
        "an investigation event leaked into the chat channel"
    );
}

/// Task 12 review (Important-1) — after the gate closes the user message is not stored at
/// all and a `503` is returned at once. `main.rs`'s `with_graceful_shutdown` keeps
/// accepting new connections until `JobManager::shutdown` finishes, so storing first and
/// refusing later (inside the response generation task, say) leaves an already-stored
/// message that can never receive a response (chat has no retry path like an
/// investigation's `recover_on_boot`) — blocking before storing is what keeps that state
/// from arising at all.
#[sqlx::test(migrations = "../../migrations")]
async fn sending_after_the_gate_closes_is_rejected_before_anything_is_saved(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let sid = session(&store).await;
    let app = test_app_with_closed_gate(pool);
    let res = app
        .oneshot(post_form_to(
            &format!("/api/chat/{sid}/messages"),
            "content=hello",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

    let msgs = store.chat_messages(sid).await.unwrap();
    assert!(
        msgs.is_empty(),
        "the user message was stored despite the gate being closed — a message that can never receive a response remains"
    );
}

/// An empty message is refused.
#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_chat_message_is_rejected(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let sid = session(&store).await;
    let app = test_app(pool);
    let res = app
        .oneshot(post_form_to(
            &format!("/api/chat/{sid}/messages"),
            "content=",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// Spec Section 13 — LLM and user input are untrusted data. This checks directly that the
/// message text `panel` renders is escaped (the HTML is inserted through the store
/// directly, keeping it independent of form URL encoding — the same approach as
/// `the_list_fragment_escapes_user_supplied_text` in `investigations.rs`).
///
/// Unlike askama, `render::esc` uses named entities (`&lt;`/`&gt;`) because it reuses the
/// same functions as `step_html` and `delta_html`. Expecting askama's numeric entities
/// (`&#60;`) would make this assertion fail.
#[sqlx::test(migrations = "../../migrations")]
async fn panel_escapes_stored_message_text(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let sid = session(&store).await;
    store
        .append_chat_message(
            sid,
            ChatRole::User,
            &serde_json::json!({"text": "<script>alert(1)</script>"}),
        )
        .await
        .unwrap();

    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri(format!("/api/chat/{sid}/panel"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "the message was not escaped: {body}"
    );
    assert!(
        body.contains("&lt;script&gt;"),
        "the escaped form is absent: {body}"
    );
}

/// The user bubble fragment `send` returns is rendered by the same function
/// (`render::chat_message_html`) and must therefore be escaped too.
#[sqlx::test(migrations = "../../migrations")]
async fn send_echoes_the_user_message_escaped(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let sid = session(&store).await;
    let app = test_app(pool);
    let res = app
        .oneshot(post_form_to(
            &format!("/api/chat/{sid}/messages"),
            "content=%3Cb%3Ehi%3C%2Fb%3E",
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body = body_string(res).await;
    assert!(
        !body.contains("<b>hi</b>"),
        "the echoed message was not escaped: {body}"
    );
    assert!(
        body.contains("&lt;b&gt;"),
        "the escaped form is absent: {body}"
    );
}

/// Sending to a nonexistent session gives `404`. `append_chat_message` returns
/// `StoreError::NotFound` when the session row is absent (the store layer), and the
/// handler must not collapse that into a 500 (the same class of defect Task 10 fixed).
#[sqlx::test(migrations = "../../migrations")]
async fn sending_to_a_missing_session_is_not_found(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let missing = Uuid::new_v4();
    let res = app
        .oneshot(post_form_to(
            &format!("/api/chat/{missing}/messages"),
            "content=hello",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Checks that the session ID literal hardcoded in `base.html` does not diverge from the
/// `DEFAULT_SESSION` constant. Writing the same value in two places separately was a
/// judgment call (there is no way to pass Rust context to the template), so a change to
/// the constant must break this test — the same reason `pages.rs` avoids duplicating the
/// `window_days` literal.
#[test]
fn default_session_id_matches_the_literal_wired_into_base_html() {
    let expected = agentops_server::routes::chat::DEFAULT_SESSION.to_string();
    let base_html =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/base.html"))
            .unwrap();
    assert!(
        base_html.contains(&expected),
        "the session ID hardcoded in base.html differs from DEFAULT_SESSION({expected})"
    );
}

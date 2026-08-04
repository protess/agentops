//! The Knowledge, Artifacts, and Settings pages, plus Instructions CRUD (Task 11).

use agentops_agent::limits::Limits;
use agentops_core::{
    BoxStream, Instruction, LlmError, LlmEvent, LlmProvider, LlmRequest, Phase, Store,
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

fn post_form_to(uri: &str, body: &str) -> Request<Body> {
    form_request("POST", uri, body)
}

fn form_request(method: &str, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn instruction(phase: Phase, position: i32, title: &str, body: &str) -> Instruction {
    Instruction {
        id: Uuid::new_v4(),
        phase,
        position,
        title: title.into(),
        body: body.into(),
        enabled: true,
        updated_at: OffsetDateTime::now_utc(),
    }
}

/// Seeds an artifact directly — a direct INSERT is needed to check the body render
/// without going through `complete_investigation` (the same pattern as
/// `list_artifacts_is_newest_first` in `agentops-store`).
async fn artifact_with_body(store: &PgStore, body: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO artifacts (id, investigation_id, title, body) VALUES ($1, NULL, 't', $2)",
    )
    .bind(id)
    .bind(body)
    .execute(store.pool())
    .await
    .unwrap();
    id
}

/// A created instruction appears in the list and **the ordering is deterministic**.
/// Breaking the `position` → `title` → `id` order makes the prompt cache miss every time
/// (spec Section 9, the UI-layer counterpart of TEST-9).
#[sqlx::test(migrations = "../../migrations")]
async fn instructions_are_listed_in_deterministic_order(pool: sqlx::PgPool) {
    let app = test_app(pool.clone());
    for (pos, title) in [(2, "b"), (1, "a"), (2, "a")] {
        app.clone()
            .oneshot(post_form_to(
                "/api/instructions",
                &format!("phase=all&position={pos}&title={title}&body=x"),
            ))
            .await
            .unwrap();
    }
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri("/api/instructions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    let ia = body.find(">a<").unwrap();
    let ib = body.find(">b<").unwrap();
    assert!(ia < ib, "the item with position 1 must come first");
}

/// **Every Phase is named explicitly** (the `ALL_PHASES` comment in instructions.rs).
/// The ordering test above seeds only `Phase::All`, so cutting `ALL_PHASES` down to
/// `[Phase::All]` still shows every item and does not catch this defect — measured; see
/// the mutation check below. This seeds one per phase and checks all five appear.
#[sqlx::test(migrations = "../../migrations")]
async fn list_fragment_enumerates_every_phase(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let phases = [
        Phase::All,
        Phase::Chat,
        Phase::Triage,
        Phase::Rca,
        Phase::Mitigation,
    ];
    for (i, phase) in phases.iter().enumerate() {
        let title = format!("only-on-{}", phase.as_str());
        store
            .upsert_instruction(&instruction(*phase, i as i32, &title, "x"))
            .await
            .unwrap();
    }

    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri("/api/instructions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    for phase in phases {
        let title = format!("only-on-{}", phase.as_str());
        assert!(
            body.contains(&title),
            "the {phase:?} instruction is absent from the list — ALL_PHASES omitted this phase"
        );
    }
}

/// The body is escaped. Instructions go into the system prompt, so HTML a user entered
/// must not be rendered as-is.
#[sqlx::test(migrations = "../../migrations")]
async fn instruction_bodies_are_escaped_in_the_list(pool: sqlx::PgPool) {
    let app = test_app(pool.clone());
    app.clone()
        .oneshot(post_form_to(
            "/api/instructions",
            "phase=all&position=1&title=t&body=%3Cimg+src%3Dx+onerror%3Dalert(1)%3E",
        ))
        .await
        .unwrap();
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri("/api/instructions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(
        !body.contains("<img src=x onerror"),
        "the body was not escaped"
    );
}

/// Deleting removes it from the list.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_an_instruction_removes_it(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let ins = instruction(Phase::All, 1, "gone", "x");
    store.upsert_instruction(&ins).await.unwrap();

    let app = test_app(pool);
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/instructions/{}", ins.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri("/api/instructions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(!body.contains("gone"));
}

/// `PUT` updates exactly the row the path's `id` points at — the other fields stay and
/// only `body` changes.
#[sqlx::test(migrations = "../../migrations")]
async fn put_updates_the_row_at_the_path_id(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let ins = instruction(Phase::Chat, 0, "edit-me", "original");
    store.upsert_instruction(&ins).await.unwrap();

    let app = test_app(pool.clone());
    let res = app
        .oneshot(form_request(
            "PUT",
            &format!("/api/instructions/{}", ins.id),
            "phase=chat&position=0&title=edit-me&body=revised",
        ))
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "a PUT with the same (phase,title) must succeed: {:?}",
        res.status()
    );

    let got = store.instructions_for(&[Phase::Chat]).await.unwrap();
    assert_eq!(got.len(), 1, "rows must not increase");
    assert_eq!(got[0].id, ins.id, "the id must not churn");
    assert_eq!(got[0].body, "revised");
}

/// A `PUT` to a nonexistent id is a 404 — it must not silently create a new row.
#[sqlx::test(migrations = "../../migrations")]
async fn put_on_a_missing_id_is_not_found(pool: sqlx::PgPool) {
    let app = test_app(pool.clone());
    let missing = Uuid::new_v4();
    let res = app
        .oneshot(form_request(
            "PUT",
            &format!("/api/instructions/{missing}"),
            "phase=chat&position=0&title=nope&body=x",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let store = PgStore::new(pool);
    let got = store.instructions_for(&[Phase::Chat]).await.unwrap();
    assert!(
        got.is_empty(),
        "a PUT to a nonexistent id must not create a new row"
    );
}

/// Regression test — task-11-review.md Critical. If `PUT` selected its target by
/// `upsert_instruction`'s `(phase, title)` conflict key rather than the path's `id`, then
/// when another row already holds that `(phase, title)` **that other** row is silently
/// overwritten while the row the path pointed at stays untouched. With
/// `update_instruction`, which selects by `id`, this request must be rejected on the
/// unique index violation and both rows must keep their original values.
#[sqlx::test(migrations = "../../migrations")]
async fn put_never_rewrites_a_different_row(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let a = instruction(Phase::Chat, 0, "foo", "a-original");
    let b = instruction(Phase::Chat, 1, "bar", "b-original");
    store.upsert_instruction(&a).await.unwrap();
    store.upsert_instruction(&b).await.unwrap();

    let app = test_app(pool.clone());
    // PUT with B's id while submitting the (phase, title) A already holds.
    let res = app
        .oneshot(form_request(
            "PUT",
            &format!("/api/instructions/{}", b.id),
            "phase=chat&position=0&title=foo&body=corrupted",
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::CONFLICT,
        "a PUT with a (phase,title) another row already holds must be rejected as a conflict: {:?}",
        res.status()
    );

    let rows = store.instructions_for(&[Phase::Chat]).await.unwrap();
    assert_eq!(rows.len(), 2, "rows must not merge or increase");
    let a_after = rows.iter().find(|i| i.id == a.id).unwrap();
    assert_eq!(
        a_after.body, "a-original",
        "A's body must not change silently"
    );
    let b_after = rows.iter().find(|i| i.id == b.id).unwrap();
    assert_eq!(
        b_after.body, "b-original",
        "B must not change either — the request was rejected"
    );
}

/// The artifact body page emits its content.
#[sqlx::test(migrations = "../../migrations")]
async fn an_artifact_page_shows_its_body(pool: sqlx::PgPool) {
    let store = PgStore::new(pool.clone());
    let id = artifact_with_body(&store, "## Conclusion\nThe disk filled up").await;
    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri(format!("/artifacts/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(body.contains("The disk filled up"));
}

/// A nonexistent artifact is a 404.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_artifact_is_not_found(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/artifacts/{}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Settings shows the MCP servers **read-only** (spec Section 10.1).
/// A write form would exceed v0.1's scope.
#[sqlx::test(migrations = "../../migrations")]
async fn settings_shows_mcp_servers_read_only(pool: sqlx::PgPool) {
    let app = test_app(pool);
    let body = body_string(
        app.oneshot(
            Request::builder()
                .uri("/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(
        !body.contains("<form"),
        "Settings has a write form — v0.1 is read-only"
    );
}

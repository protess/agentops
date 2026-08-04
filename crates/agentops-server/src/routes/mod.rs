pub mod chat;
pub mod health;
pub mod instructions;
pub mod investigations;
pub mod pages;

use crate::AppState;
use axum::{routing::get, Router};
use tower_http::services::ServeDir;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(pages::index))
        .route("/incidents", get(pages::incidents))
        .route("/investigations/{id}", get(pages::investigation_detail))
        .route("/knowledge", get(pages::knowledge))
        .route("/artifacts", get(pages::artifacts))
        .route("/artifacts/{id}", get(pages::artifact_detail))
        .route("/settings", get(pages::settings))
        .route("/api/health", get(health::health))
        .route(
            "/api/investigations",
            get(investigations::list_fragment).post(investigations::create),
        )
        .route(
            "/api/investigations/{id}/stream",
            get(crate::stream::investigation_stream),
        )
        .route(
            "/api/instructions",
            get(instructions::list_fragment).post(instructions::create),
        )
        .route(
            "/api/instructions/{id}",
            axum::routing::put(instructions::update).delete(instructions::delete),
        )
        .route("/api/chat/{sid}/messages", axum::routing::post(chat::send))
        .route("/api/chat/{sid}/stream", get(chat::stream))
        .route("/api/chat/{sid}/panel", get(chat::panel))
        // `ServeDir::new("static")` resolves against the process's current directory, so it
        // behaves differently under `cargo test` and `cargo run` — the manifest directory is
        // pinned to avoid the failure mode of 404s only in tests.
        .nest_service(
            "/static",
            ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static")),
        )
        .with_state(state)
}

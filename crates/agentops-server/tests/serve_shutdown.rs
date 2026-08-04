//! Final review C-1 regression guard — it really starts `axum::serve` and observes shutdown.
//!
//! **No other shutdown test on this branch goes through a real connection lifetime.**
//! `tests/shutdown.rs` calls `JobManager::shutdown` directly, and `tests/pages.rs` drives
//! only the router with `ServiceExt::oneshot` for a single request — no test on the whole
//! branch binds a listener, holds a connection open, and watches whether graceful shutdown
//! really waits for it. That blind spot is exactly why C-1 (an open SSE connection
//! blocking graceful shutdown forever) passed twelve task reviews.
//!
//!
//! **It uses the chat SSE (`/api/chat/{sid}/stream`)** — the very path C-1 reproduced on.
//! It is the same stream `base.html` opens on every page, and a subscription works even
//! with no session row in the database (`bus.rs::subscribe_chat` performs no existence
//! check), so there is no need to create an investigation or prepare a session in advance.
//!
//! This test uses `tokio::net::TcpStream` rather than the std one — opening it with
//! synchronous blocking I/O would leave the server task no chance to make progress on
//! `#[sqlx::test]`'s runtime (which may be single-threaded by default) and the test itself
//! would stall.

use agentops_agent::limits::Limits;
use agentops_core::{BoxStream, LlmError, LlmEvent, LlmProvider, LlmRequest};
use agentops_server::bus::StepBus;
use agentops_server::jobs::{JobDeps, JobManager};
use agentops_server::routes::chat::DEFAULT_SESSION;
use agentops_server::{routes, AppState};
use agentops_store::PgStore;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// This test does not actually generate a chat response — all it needs is that the
/// connection is open. A placeholder for the same reason as the identically named type in
/// `tests/pages.rs`.
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

/// Verifies that `axum::serve(...).with_graceful_shutdown(..)` returns within a bounded
/// time even with one SSE connection held open.
///
/// Before the fix (C-1) this test does not pass — the `server` task never ends, so
/// `tokio::time::timeout` always expires. Independent reproduction already confirmed that
/// the same shape on axum 0.8.9 (a parking producer plus `Sse::new` plus `KeepAlive`)
/// really does hang (`final-review.md`).
#[sqlx::test(migrations = "../../migrations")]
async fn graceful_shutdown_completes_with_an_open_sse_connection(pool: sqlx::PgPool) {
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
    let state = AppState {
        store,
        bus,
        jobs: jobs.clone(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // The same wiring as `main.rs`, except the signal is fired by this test rather than
    // `ctrl_c()`, to trigger it deterministically.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let jm = jobs.clone();
    let shutdown = async move {
        let _ = shutdown_rx.await;
        jm.shutdown(Duration::from_secs(5)).await;
    };

    let server = tokio::spawn(async move {
        axum::serve(listener, routes::router(state))
            .with_graceful_shutdown(shutdown)
            .await
    });

    // Open the SSE connection and read as far as the response header to confirm it is
    // really established. The socket is not closed here — C-1 is precisely the scenario
    // of shutting down "while the client holds the connection".
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /api/chat/{DEFAULT_SESSION}/stream HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n"
    );
    sock.write_all(req.as_bytes()).await.unwrap();

    let mut buf = [0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
        .await
        .expect("the SSE response header did not arrive within 5 seconds")
        .unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "unexpected response: {resp}"
    );

    // Trigger shutdown with the connection still open.
    shutdown_tx
        .send(())
        .expect("shutdown receiver dropped early");

    // If C-1 were unfixed this timeout would always expire — the server task would never
    // return.
    let joined = tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .expect("axum::serve did not return within 15 seconds — graceful shutdown hung on an open SSE connection (a C-1 regression)");
    let served = joined.expect("server task panicked");
    assert!(
        served.is_ok(),
        "server returned an error: {:?}",
        served.err()
    );

    // The socket value itself stays alive to here even after the server finishes first —
    // the drop is stated last to make clear that the connection was ended by the server
    // (graceful shutdown), not the client.
    drop(sock);
}

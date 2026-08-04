use crate::AppState;
use axum::{extract::State, Json};
use serde_json::json;

/// Spec Section 10.2 — database, LLM, and MCP status. v0.1 actually checks only the
/// database connection. LLM and MCP are filled in once dependencies are assembled in Task 5.
pub async fn health(State(st): State<AppState>) -> Json<serde_json::Value> {
    // **Never swallow it silently.** From `{"db": false}` alone, "the database is down"
    // and "the query is wrong" are indistinguishable on screen, so the log is the only clue.
    let db = match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(st.store.pool())
        .await
    {
        Ok(_) => true,
        Err(e) => {
            tracing::error!(error = %e, "health check db query failed");
            false
        }
    };
    Json(json!({ "db": db }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_store::PgStore;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// A minimal `Write`/`MakeWriter` adapter that captures `tracing` output into a buffer.
    /// It verifies "an error log really is emitted" using only `tracing-subscriber`, which
    /// is already a workspace dependency, with no external crate such as `tracing-test`.
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

    /// A placeholder needed only to assemble `JobManager` — this test spawns no
    /// investigation, so `stream()` is never called.
    struct NullProvider;

    #[async_trait::async_trait]
    impl agentops_core::LlmProvider for NullProvider {
        fn model_id(&self) -> &str {
            "null"
        }
        async fn stream(
            &self,
            _req: agentops_core::LlmRequest,
        ) -> Result<
            agentops_core::BoxStream<
                'static,
                Result<agentops_core::LlmEvent, agentops_core::LlmError>,
            >,
            agentops_core::LlmError,
        > {
            Ok(Box::pin(futures_util::stream::empty()))
        }
    }

    /// What this test catches: if someone reverts the `match` back to `.is_ok()` (the
    /// original brief's defect), this test fails because the log buffer is empty.
    /// The `{"db": false}` response alone cannot catch the regression — hence the direct
    /// assertion on the log content.
    ///
    /// It does not use `#[tokio::test]` — `tracing::subscriber::with_default` installs the
    /// subscriber for a synchronous closure scope. Calling `block_on` inside an `.await`
    /// already on a tokio runtime panics with "Cannot start a runtime from within a
    /// runtime". Instead it builds a runtime just for this test and blocks outside that
    /// scope.
    #[test]
    fn db_failure_is_logged_not_swallowed() {
        let buf = BufWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();

        let body = tracing::subscriber::with_default(subscriber, || {
            let rt = tokio::runtime::Runtime::new().expect("build a throwaway runtime");
            rt.block_on(async {
                // Port 1 is effectively guaranteed to have no listener on loopback (an
                // unprivileged process cannot bind it) — the connection is refused
                // immediately and the query fails. `connect_lazy` connects to nothing at
                // pool creation time but spawns a background maintenance task, so it must
                // be called *inside* a tokio context — the failure happens at `fetch_one`.
                //
                // A short `acquire_timeout` is given — with the default (30s), a port 1
                // connection in this environment is dropped silently rather than refused
                // and one test takes 30 seconds (measured). This test verifies the failure
                // itself, so *how* it fails does not matter.
                let pool = sqlx::postgres::PgPoolOptions::new()
                    .acquire_timeout(std::time::Duration::from_millis(500))
                    .connect_lazy("postgres://nouser:nopass@127.0.0.1:1/nodb")
                    .expect("connect_lazy does no I/O, must not fail here");
                let store = Arc::new(PgStore::new(pool));
                let bus = crate::bus::StepBus::new();
                let jobs = crate::jobs::JobManager::new(
                    store.clone(),
                    bus.clone(),
                    crate::jobs::JobDeps {
                        provider: Arc::new(NullProvider),
                        connections: Vec::new(),
                        limits: agentops_agent::limits::Limits::default(),
                    },
                );
                let state = AppState { store, bus, jobs };
                health(State(state)).await
            })
        });

        assert_eq!(
            body.0["db"],
            serde_json::json!(false),
            "with no database connection the db field must be false"
        );

        let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(
            log.contains("health check db query failed"),
            "a database error must be recorded with tracing::error! (never swallowed) — actual log: {log:?}"
        );
    }
}

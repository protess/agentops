use agentops_agent::anthropic::AnthropicProvider;
use agentops_agent::limits::Limits;
use agentops_agent::ApiKey;
use agentops_server::jobs::{JobDeps, JobManager};
use agentops_server::{config::Config, routes, AppState};
use agentops_store::PgStore;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&cfg.database_url)
        .await?;
    let store = Arc::new(PgStore::new(pool));
    let bus = agentops_server::bus::StepBus::new();

    // `JobManager` — this state goes into `AppState` so the `POST /api/investigations`
    // handler spawns investigations onto it (Task 9). Assembling it here is what makes
    // graceful shutdown actually wait on something — this is where `shutdown_deadline`
    // stops being the dead default it has been since Task 1.
    let jm = JobManager::new(
        store.clone(),
        bus.clone(),
        JobDeps {
            provider: Arc::new(AnthropicProvider::new(ApiKey::new(
                cfg.anthropic_api_key.clone(),
            ))),
            connections: Vec::new(),
            limits: Limits::default(),
        },
    );

    let state = AppState {
        store: store.clone(),
        bus: bus.clone(),
        jobs: jm.clone(),
    };

    // **Boot cleanup must finish before the server accepts requests.** In the opposite
    // order, requests arriving during cleanup race with its targets (spec Section 6.1,
    // INV-4). The `jm` Task 6 built is reused, not recreated. A second `JobManager` would
    // split the `JoinSet` and make graceful shutdown wait for only half.
    //
    let report = agentops_server::watchdog::recover_on_boot(&store, &jm).await?;
    tracing::info!(
        failed = report.failed,
        requeued = report.requeued,
        "boot recovery"
    );

    let wd = agentops_server::watchdog::spawn_watchdog(
        store.clone(),
        bus.clone(),
        cfg.watchdog_idle,
        cfg.watchdog_interval,
        jm.cancel_token(),
    );

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    tracing::info!(bind = %cfg.bind, "listening");

    // The shutdown ordering of spec Section 6.1 lives inside `JobManager::shutdown` — here
    // the signal is only wired to it.
    //
    // **It listens for SIGTERM as well as `ctrl_c()`** (final review M-1). In the
    // `docker-compose.yml` deployment spec Section 15 requires, Docker and systemd take a
    // container down with **SIGTERM** — the earlier version listening only for `ctrl_c()`
    // never ran the six-stage shutdown on that path and died with investigations still
    // `running` (data is safe because `recover_on_boot` cleans up at the next start, but
    // the entire shutdown machinery this plan built becomes useless).
    let jm2 = jm.clone();
    let shutdown = async move {
        wait_for_shutdown_signal().await;
        tracing::info!("shutdown signal received");
        jm2.shutdown(cfg.shutdown_deadline).await;
    };
    axum::serve(listener, routes::router(state))
        .with_graceful_shutdown(shutdown)
        .await?;

    // The watchdog shares `jm.cancel_token()`, so stage 2 of `shutdown()` has already
    // fired cancellation — here we wait for that task to actually finish so it does not
    // remain a detached task.
    wd.await?;
    Ok(())
}

/// Waits for whichever comes first, `Ctrl-C` (SIGINT) or SIGTERM.
///
/// Non-Unix targets (Windows, for instance) have no concept of `SIGTERM`, so
/// `tokio::signal::unix` is not compiled there — it waits on `ctrl_c()` alone.
///
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

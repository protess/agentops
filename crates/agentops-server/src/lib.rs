//! The agentops HTTP surface.
//!
//! **Handlers do not run investigations.** `JobManager` owns the tasks and the handler
//! returns immediately (spec Section 6.1, INV-1).

pub mod bus;
pub mod chart;
pub mod config;
pub mod jobs;
pub mod render;
pub mod routes;
pub mod stream;
pub mod watchdog;

use agentops_store::PgStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<PgStore>,
    pub bus: bus::StepBus,
    pub jobs: jobs::JobManager<PgStore>,
}

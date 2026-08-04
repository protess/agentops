//! The two-layer zombie investigation defense (spec Section 6.1, INV-4).
//!
//! If the process stays alive while only the task panics or stalls, boot cleanup never
//! fires. Without this second layer an investigation stays `Running` forever.
//!
//!
//! **F1 — reclamation is owned by the terminal transaction too.** It calls only
//! `fail_investigation`. Following `append_step(Terminated)` with a separate status
//! `UPDATE` means that on losing the race the status is someone else's while the terminal
//! row is ours, and the log contradicts the real termination reason. Plan 2 met this defect three times.

use crate::bus::StepBus;
use crate::jobs::JobManager;
use agentops_core::{Store, StoreError, TerminalReason};
use agentops_store::PgStore;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootReport {
    pub failed: u64,
    pub requeued: usize,
}

/// Reclaims `running` investigations stalled longer than the threshold. Returns how many were reclaimed.
pub async fn sweep_once(
    store: &PgStore,
    bus: &StepBus,
    idle: Duration,
) -> Result<usize, StoreError> {
    // `idle_for` is interpreted by the database — computing the threshold on the app clock
    // and passing it in would compare two clocks against an `updated_at` written on the DB
    // clock, reclaiming investigations early when the app clock runs ahead (plan 1's decision).
    let ids = store.stale_running_ids(idle).await?;
    let mut n = 0;
    for id in ids {
        match store
            .fail_investigation(id, &TerminalReason::WallClockExceeded)
            .await
        {
            Ok(()) => {
                n += 1;
                bus.publish_terminal(id);
            }
            // Conflict means another party terminated it in between. Back off.
            Err(StoreError::Conflict) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

pub fn spawn_watchdog(
    store: Arc<PgStore>,
    bus: StepBus,
    idle: Duration,
    interval: Duration,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    match sweep_once(&store, &bus, idle).await {
                        Ok(0) => {}
                        Ok(n) => tracing::warn!(reclaimed = n, "watchdog reclaimed stalled investigations"),
                        Err(e) => tracing::error!(error = %e, "watchdog sweep failed"),
                    }
                }
                _ = cancel.cancelled() => return,
            }
        }
    })
}

/// Boot cleanup — `running` becomes `failed` and `queued` is rescheduled.
///
/// **Must be called before the server accepts requests.** In the opposite order, requests
/// arriving during cleanup race with its targets.
pub async fn recover_on_boot(
    store: &PgStore,
    jm: &JobManager<PgStore>,
) -> anyhow::Result<BootReport> {
    let failed = store
        .fail_orphaned_running(&TerminalReason::TaskPanicked)
        .await?;
    let queued = store.queued_ids().await?;
    let mut requeued = 0;
    for id in queued {
        if jm.spawn(id).is_ok() {
            requeued += 1;
        }
    }
    Ok(BootReport { failed, requeued })
}

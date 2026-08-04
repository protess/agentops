use agentops_core::{
    Cursor, Investigation, InvestigationPage, InvestigationStatus, ListFilter, StoreError,
    TriggeredBy,
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Database row to domain type. A string parse failure means the schema and the code
/// disagree, so it is raised as a `Backend` error.
///
/// It is `pub(crate)` because Task 8's `fail_orphaned_running` uses it too.
pub(crate) fn row_to_investigation(
    row: &sqlx::postgres::PgRow,
) -> Result<Investigation, StoreError> {
    let status: String = row.try_get("status").map_err(crate::backend)?;
    let triggered_by: String = row.try_get("triggered_by").map_err(crate::backend)?;
    let trigger_source: Option<String> = row.try_get("trigger_source").map_err(crate::backend)?;

    let triggered_by = match triggered_by.as_str() {
        "user" => TriggeredBy::User,
        "alarm" => TriggeredBy::Alarm {
            source: trigger_source
                .ok_or_else(|| StoreError::Backend("alarm row without trigger_source".into()))?,
        },
        other => {
            return Err(StoreError::Backend(format!(
                "unknown triggered_by: {other}"
            )))
        }
    };

    Ok(Investigation {
        id: row.try_get("id").map_err(crate::backend)?,
        title: row.try_get("title").map_err(crate::backend)?,
        prompt: row.try_get("prompt").map_err(crate::backend)?,
        status: status
            .parse::<InvestigationStatus>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        triggered_by,
        queued_at: row.try_get("queued_at").map_err(crate::backend)?,
        started_at: row.try_get("started_at").map_err(crate::backend)?,
        finished_at: row.try_get("finished_at").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

pub async fn create(pool: &PgPool, inv: &Investigation) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO investigations
           (id, title, prompt, status, triggered_by, trigger_source,
            queued_at, started_at, finished_at, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(inv.id)
    .bind(&inv.title)
    .bind(&inv.prompt)
    .bind(inv.status.as_str())
    .bind(inv.triggered_by.kind_str())
    .bind(inv.triggered_by.source())
    .bind(inv.queued_at)
    .bind(inv.started_at)
    .bind(inv.finished_at)
    .bind(inv.updated_at)
    .execute(pool)
    .await
    .map_err(crate::backend)?;
    Ok(())
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Investigation, StoreError> {
    let row = sqlx::query("SELECT * FROM investigations WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(crate::backend)?
        .ok_or(StoreError::NotFound)?;
    row_to_investigation(&row)
}

/// Keyset pagination. Ordering by `(queued_at, id)` breaks ties on identical timestamps.
pub async fn list(pool: &PgPool, f: &ListFilter) -> Result<InvestigationPage, StoreError> {
    // Clamp so a limit of zero or below, or an oversized one, becomes neither a SQL error nor an overflow.
    let limit = f.limit.clamp(1, 500);
    // Read limit + 1 to determine whether a next page exists.
    let fetch = limit + 1;
    let rows = match (&f.status, &f.cursor) {
        (Some(s), Some((ts, id))) => {
            sqlx::query(
                "SELECT * FROM investigations
                 WHERE status = $1 AND (queued_at, id) < ($2, $3)
                 ORDER BY queued_at DESC, id DESC LIMIT $4",
            )
            .bind(s.as_str())
            .bind(ts)
            .bind(id)
            .bind(fetch)
            .fetch_all(pool)
            .await
        }
        (Some(s), None) => {
            sqlx::query(
                "SELECT * FROM investigations WHERE status = $1
                 ORDER BY queued_at DESC, id DESC LIMIT $2",
            )
            .bind(s.as_str())
            .bind(fetch)
            .fetch_all(pool)
            .await
        }
        (None, Some((ts, id))) => {
            sqlx::query(
                "SELECT * FROM investigations WHERE (queued_at, id) < ($1, $2)
                 ORDER BY queued_at DESC, id DESC LIMIT $3",
            )
            .bind(ts)
            .bind(id)
            .bind(fetch)
            .fetch_all(pool)
            .await
        }
        (None, None) => {
            sqlx::query("SELECT * FROM investigations ORDER BY queued_at DESC, id DESC LIMIT $1")
                .bind(fetch)
                .fetch_all(pool)
                .await
        }
    }
    .map_err(crate::backend)?;

    let mut items: Vec<Investigation> = rows
        .iter()
        .map(row_to_investigation)
        .collect::<Result<_, _>>()?;

    let next_cursor: Option<Cursor> = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|i| (i.queued_at, i.id))
    } else {
        None
    };

    Ok(InvestigationPage { items, next_cursor })
}

/// `queued` → `running`. Being conditional, a second call returns `Conflict`.
pub async fn mark_running(pool: &PgPool, id: Uuid) -> Result<(), StoreError> {
    let res = sqlx::query(
        "UPDATE investigations SET status = 'running', started_at = now()
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(crate::backend)?;

    if res.rows_affected() == 0 {
        return Err(StoreError::Conflict);
    }
    Ok(())
}

/// **The database computes the threshold.** A caller building a timestamp on the app
/// clock would compare it against an `updated_at` written on the DB clock, reclaiming
/// investigations early when the app clock runs ahead. Passing an `interval` puts both sides on the DB clock.
pub async fn stale_running_ids(
    pool: &PgPool,
    idle_for: std::time::Duration,
) -> Result<Vec<Uuid>, StoreError> {
    sqlx::query_scalar(
        "SELECT id FROM investigations
          WHERE status = 'running'
            AND updated_at < now() - make_interval(secs => $1)
          ORDER BY updated_at",
    )
    .bind(idle_for.as_secs_f64())
    .fetch_all(pool)
    .await
    .map_err(crate::backend)
}

pub async fn queued_ids(pool: &PgPool) -> Result<Vec<Uuid>, StoreError> {
    sqlx::query_scalar("SELECT id FROM investigations WHERE status = 'queued' ORDER BY queued_at")
        .fetch_all(pool)
        .await
        .map_err(crate::backend)
}

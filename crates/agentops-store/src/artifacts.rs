use agentops_core::{
    AgentStep, Artifact, NewArtifact, Phase, StepKind, StoreError, TerminalReason,
};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::steps::{allocate_seq, insert_step_strict};

fn row_to_artifact(row: &sqlx::postgres::PgRow) -> Result<Artifact, StoreError> {
    Ok(Artifact {
        id: row.try_get("id").map_err(crate::backend)?,
        investigation_id: row.try_get("investigation_id").map_err(crate::backend)?,
        title: row.try_get("title").map_err(crate::backend)?,
        body: row.try_get("body").map_err(crate::backend)?,
        created_at: row.try_get("created_at").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Artifact, StoreError> {
    let row = sqlx::query("SELECT * FROM artifacts WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(crate::backend)?
        .ok_or(StoreError::NotFound)?;
    row_to_artifact(&row)
}

pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<Artifact>, StoreError> {
    // Clamp so a limit of zero or below, or an oversized one, becomes neither a SQL
    // error nor an overflow (the same convention as investigations::list).
    let limit = limit.clamp(1, 500);
    let rows = sqlx::query("SELECT * FROM artifacts ORDER BY created_at DESC, id DESC LIMIT $1")
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(crate::backend)?;
    rows.iter().map(row_to_artifact).collect()
}

/// Saving the artifact, the `ArtifactWritten` step, and `running` → `completed`,
/// **in one transaction.** Committing them separately leaves a partial success.
///
/// **The status transition is attempted first.** If the conditional UPDATE catches no
/// row, another party already terminated it, so there is no need to create an artifact
/// or consume a `seq`. Reversing the order produces a pointless insert and rollback on the conflict path.
///
/// There is no `seq` parameter — the database allocates it inside the transaction (spec Section 6.1.1).
pub async fn complete_investigation(
    pool: &PgPool,
    id: Uuid,
    artifact: &NewArtifact,
) -> Result<Uuid, StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;

    // 1. The conditional transition first. If it catches here, nothing else is done.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE investigations SET status = 'completed', finished_at = now()
          WHERE id = $1 AND status = 'running'
        RETURNING id",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(crate::backend)?;

    if claimed.is_none() {
        tx.rollback().await.map_err(crate::backend)?;
        return Err(StoreError::Conflict);
    }

    // 2. The artifact
    let artifact_id = Uuid::new_v4();
    sqlx::query("INSERT INTO artifacts (id, investigation_id, title, body) VALUES ($1,$2,$3,$4)")
        .bind(artifact_id)
        .bind(id)
        .bind(&artifact.title)
        .bind(&artifact.body)
        .execute(&mut *tx)
        .await
        .map_err(crate::backend)?;

    // 3. The terminal step. It uses no ON CONFLICT, so it cannot be lost.
    let seq = allocate_seq(&mut tx, id).await?;
    let step = AgentStep {
        investigation_id: id,
        seq,
        phase: Phase::All,
        kind: StepKind::ArtifactWritten { artifact_id },
        created_at: OffsetDateTime::now_utc(),
    };
    insert_step_strict(&mut tx, &step).await?;

    tx.commit().await.map_err(crate::backend)?;
    Ok(artifact_id)
}

/// The `Terminated` step and `running` → `failed` in one transaction.
/// The transition is attempted first for the same reason as `complete_investigation`.
pub async fn fail_investigation(
    pool: &PgPool,
    id: Uuid,
    reason: &TerminalReason,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;

    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE investigations SET status = 'failed', finished_at = now()
          WHERE id = $1 AND status = 'running'
        RETURNING id",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(crate::backend)?;

    if claimed.is_none() {
        tx.rollback().await.map_err(crate::backend)?;
        return Err(StoreError::Conflict);
    }

    let seq = allocate_seq(&mut tx, id).await?;
    let step = AgentStep {
        investigation_id: id,
        seq,
        phase: Phase::All,
        kind: StepKind::Terminated {
            reason: reason.clone(),
            detail: None,
        },
        created_at: OffsetDateTime::now_utc(),
    };
    insert_step_strict(&mut tx, &step).await?;

    tx.commit().await.map_err(crate::backend)?;
    Ok(())
}

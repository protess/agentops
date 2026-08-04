use agentops_core::{Instruction, Phase, StoreError};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn row_to_instruction(row: &sqlx::postgres::PgRow) -> Result<Instruction, StoreError> {
    let phase: String = row.try_get("phase").map_err(crate::backend)?;
    Ok(Instruction {
        id: row.try_get("id").map_err(crate::backend)?,
        phase: phase
            .parse::<Phase>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        position: row.try_get("position").map_err(crate::backend)?,
        title: row.try_get("title").map_err(crate::backend)?,
        body: row.try_get("body").map_err(crate::backend)?,
        enabled: row.try_get("enabled").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

/// **`ORDER BY position, title, id` is required.** Without the ordering the assembled
/// system prompt differs per request and the prompt cache hit rate goes to zero (Section 8.3).
///
/// Why `id` is the final tiebreaker: the unique index is only on `(phase, title)`, so it
/// does not prevent different phases from sharing the same `(position, title)` — for
/// instance `(all, 0, "safety")` and `(triage, 0, "safety")` are both valid, and querying
/// several phases at once, as in `for_phases(&[All, Triage])`, is a normal call pattern
/// the agent uses constantly. In that case `position, title` alone is not a total order
/// and Postgres may return an arbitrary order. `id` is the primary key, so it is
/// unconditionally unique, and that uniqueness survives even if the `(phase, title)`
/// unique index changes later.
pub async fn for_phases(pool: &PgPool, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError> {
    let names: Vec<String> = phases.iter().map(|p| p.as_str().to_owned()).collect();
    let rows = sqlx::query(
        "SELECT * FROM instructions
         WHERE enabled AND phase = ANY($1)
         ORDER BY position, title, id",
    )
    .bind(&names)
    .fetch_all(pool)
    .await
    .map_err(crate::backend)?;
    rows.iter().map(row_to_instruction).collect()
}

/// On a `(phase, title)` conflict it updates the existing row. **`id` is absent from the
/// SET clause, so the existing row's `id` is preserved and the caller's `ins.id` is
/// silently discarded.** This is intended — instruction ids must not churn.
pub async fn upsert(pool: &PgPool, ins: &Instruction) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO instructions (id, phase, position, title, body, enabled, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (phase, title) DO UPDATE
           SET position = EXCLUDED.position,
               body = EXCLUDED.body,
               enabled = EXCLUDED.enabled",
    )
    .bind(ins.id)
    .bind(ins.phase.as_str())
    .bind(ins.position)
    .bind(&ins.title)
    .bind(&ins.body)
    .bind(ins.enabled)
    .bind(ins.updated_at)
    .execute(pool)
    .await
    .map_err(crate::backend)?;
    Ok(())
}

/// For `PUT /api/instructions/{id}` only. Unlike `upsert`, it selects the update target
/// by **`id`**, not by a `(phase, title)` conflict. `NotFound` if no row has that `id`.
/// If the submitted `(phase, title)` is already held by **another** row, it fails on the
/// `instructions_phase_title_idx` unique index violation and is raised as `Conflict` —
/// the same idiom as `steps.rs::insert_step_strict` (discriminated with
/// `is_unique_violation()` rather than flattened into `Backend(String)`).
pub async fn update(pool: &PgPool, ins: &Instruction) -> Result<(), StoreError> {
    let res = sqlx::query(
        "UPDATE instructions
           SET phase = $2, position = $3, title = $4, body = $5, enabled = $6, updated_at = $7
         WHERE id = $1",
    )
    .bind(ins.id)
    .bind(ins.phase.as_str())
    .bind(ins.position)
    .bind(&ins.title)
    .bind(&ins.body)
    .bind(ins.enabled)
    .bind(ins.updated_at)
    .execute(pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.is_unique_violation() {
                return StoreError::Conflict;
            }
        }
        crate::backend(e)
    })?;
    if res.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), StoreError> {
    let res = sqlx::query("DELETE FROM instructions WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(crate::backend)?;
    if res.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

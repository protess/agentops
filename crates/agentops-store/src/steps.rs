use agentops_core::{AgentStep, Phase, StepKind, StoreError, TerminalReason};
use sqlx::{PgConnection, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

fn row_to_step(row: &sqlx::postgres::PgRow) -> Result<AgentStep, StoreError> {
    let phase: String = row.try_get("phase").map_err(crate::backend)?;
    let payload: serde_json::Value = row.try_get("payload").map_err(crate::backend)?;
    Ok(AgentStep {
        investigation_id: row.try_get("investigation_id").map_err(crate::backend)?,
        seq: row.try_get("seq").map_err(crate::backend)?,
        phase: phase
            .parse::<Phase>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        kind: StepKind::from_payload_json(&payload)?,
        created_at: row.try_get("created_at").map_err(crate::backend)?,
    })
}

/// **The single point of `seq` allocation.** Allocation and the `updated_at` refresh the
/// watchdog needs happen in one statement — there is no separate UPDATE to forget, and the
/// row lock serializes appends per investigation.
pub(crate) async fn allocate_seq(conn: &mut PgConnection, id: Uuid) -> Result<i64, StoreError> {
    sqlx::query_scalar(
        "UPDATE investigations
            SET next_step_seq = next_step_seq + 1, updated_at = now()
          WHERE id = $1
        RETURNING next_step_seq - 1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(crate::backend)?
    .ok_or(StoreError::NotFound)
}

/// An ordinary step append. The database allocates `seq`, so the caller does not pass one.
pub async fn append(
    pool: &PgPool,
    investigation_id: Uuid,
    phase: Phase,
    kind: &StepKind,
) -> Result<i64, StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;
    let seq = allocate_seq(&mut tx, investigation_id).await?;
    let step = AgentStep {
        investigation_id,
        seq,
        phase,
        kind: kind.clone(),
        created_at: OffsetDateTime::now_utc(),
    };
    insert_step_strict(&mut tx, &step).await?;
    tx.commit().await.map_err(crate::backend)?;
    Ok(seq)
}

/// **Uses no `ON CONFLICT`.** Because the database allocates `seq`, a conflict cannot
/// occur, and if one does it is a bug — so it must be raised as an error rather than
/// swallowed silently. Terminal steps (Task 10) use this function too.
pub(crate) async fn insert_step_strict(
    conn: &mut PgConnection,
    step: &AgentStep,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO agent_steps (investigation_id, seq, phase, kind, payload, created_at)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(step.investigation_id)
    .bind(step.seq)
    .bind(step.phase.as_str())
    .bind(step.kind.kind_str())
    .bind(step.payload_json())
    .bind(step.created_at)
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        // A PK violation means seq allocation was bypassed — do not pass over it silently
        if let sqlx::Error::Database(db) = &e {
            if db.is_unique_violation() {
                return StoreError::Conflict;
            }
        }
        crate::backend(e)
    })?;
    Ok(())
}

/// Boot-time orphan cleanup. The ID set `select_orphans` locked is passed straight to
/// `terminate_orphans`, which terminates only that set. Why these are split into two
/// separate `pub(crate)` functions: exercising the interleaving between the lock and the
/// UPDATE requires holding a mid-transaction state, and with this one function a test
/// cannot see that point (see `steps::tests::test_20_race_interleaving_leaves_newly_running_investigation_untouched`).
///
///
/// Under READ COMMITTED a fresh snapshot is taken per statement, so an investigation that
/// `mark_running` moved to `running` after the `SELECT ... FOR UPDATE` is not among the
/// locked rows. An unconditional UPDATE with `WHERE status = 'running'` would fail that
/// investigation without a `Terminated` step.
pub async fn fail_orphaned_running(
    pool: &PgPool,
    reason: &TerminalReason,
) -> Result<u64, StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;
    let ids = select_orphans(&mut tx).await?;
    let updated = terminate_orphans(&mut tx, &ids, reason).await?;
    tx.commit().await.map_err(crate::backend)?;
    Ok(updated.len() as u64)
}

/// Locks the cleanup targets. The ID set this statement locks is the only set
/// `terminate_orphans` may afterwards touch — **the UPDATE is confined to the ID set
/// captured at selection time.**
pub(crate) async fn select_orphans(conn: &mut PgConnection) -> Result<Vec<Uuid>, StoreError> {
    sqlx::query_scalar("SELECT id FROM investigations WHERE status = 'running' FOR UPDATE")
        .fetch_all(&mut *conn)
        .await
        .map_err(crate::backend)
}

/// Leaves a `Terminated` step and moves to `failed` only for the set passed as `ids`.
/// The UPDATE scope must be confined to the set `select_orphans` locked — otherwise an
/// investigation that became `running` after the lock is swept along without a
/// `Terminated` step.
///
/// **Contract: call this only inside the transaction where `select_orphans` took its row locks.**
/// Calling it on a connection taken straight from the pool breaks silently — a failure
/// partway through does not roll back the steps already inserted for earlier IDs, losing
/// the all-or-nothing guarantee, and with no lock the scope confinement is meaningless.
pub(crate) async fn terminate_orphans(
    conn: &mut PgConnection,
    ids: &[Uuid],
    reason: &TerminalReason,
) -> Result<Vec<Uuid>, StoreError> {
    for id in ids {
        let seq = allocate_seq(&mut *conn, *id).await?;
        let step = AgentStep {
            investigation_id: *id,
            seq,
            phase: Phase::All,
            kind: StepKind::Terminated {
                reason: reason.clone(),
                detail: None,
            },
            created_at: OffsetDateTime::now_utc(),
        };
        insert_step_strict(&mut *conn, &step).await?;
    }

    let updated: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE investigations SET status = 'failed', finished_at = now()
          WHERE id = ANY($1) AND status = 'running'
        RETURNING id",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(crate::backend)?;

    // Unreachable under the current invariants: `FOR UPDATE` holds row locks on this
    // `ids` set for the whole transaction, and no other statement in this transaction
    // changes their status. This is not an error-recovery path but a defensive assertion
    // against a future edit to this function that breaks that invariant.
    //
    if updated.len() != ids.len() {
        return Err(StoreError::Backend(format!(
            "boot cleanup locked {} rows but updated {}",
            ids.len(),
            updated.len()
        )));
    }

    Ok(updated)
}

pub async fn after(pool: &PgPool, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError> {
    let rows = sqlx::query(
        "SELECT * FROM agent_steps WHERE investigation_id = $1 AND seq > $2 ORDER BY seq",
    )
    .bind(id)
    .bind(after_seq)
    .fetch_all(pool)
    .await
    .map_err(crate::backend)?;
    rows.iter().map(row_to_step).collect()
}

pub async fn max_seq(pool: &PgPool, id: Uuid) -> Result<Option<i64>, StoreError> {
    sqlx::query_scalar("SELECT MAX(seq) FROM agent_steps WHERE investigation_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(crate::backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_core::{Investigation, InvestigationStatus, Store, TriggeredBy};

    /// The same shape as the helper in the Task 7/8 integration tests.
    fn queued(title: &str) -> Investigation {
        let now = OffsetDateTime::now_utc();
        Investigation {
            id: Uuid::new_v4(),
            title: title.into(),
            prompt: "p".into(),
            status: InvestigationStatus::Queued,
            triggered_by: TriggeredBy::User,
            queued_at: now,
            started_at: None,
            finished_at: None,
            updated_at: now,
        }
    }

    /// TEST-20 — a real race. *After* `select_orphans` locks only `a`, another connection
    /// transitions `b` from queued to running. If `terminate_orphans`'s UPDATE is not
    /// scoped to `select_orphans`'s locked set, `b` is swept along without a `Terminated`
    /// step as well.
    ///
    /// Constructing this interleaving requires calling `select_orphans` and
    /// `terminate_orphans` directly in mid-transaction. Both are `pub(crate)` and
    /// invisible from the integration test crate (`tests/steps.rs`), so this test lives
    /// inside the crate as a unit test.
    #[sqlx::test(migrations = "../../migrations")]
    async fn test_20_race_interleaving_leaves_newly_running_investigation_untouched(
        pool: sqlx::PgPool,
    ) {
        let store = crate::PgStore::new(pool.clone());
        let a = queued("a");
        let b = queued("b");
        store.create_investigation(&a).await.unwrap();
        store.create_investigation(&b).await.unwrap();
        store.mark_running(a.id).await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let ids = select_orphans(&mut tx).await.unwrap();
        assert_eq!(ids, vec![a.id], "only a is running when the lock is taken");

        // b was not running at lock time, so it is not in the locked set — this call runs
        // on a different connection and must not block.
        store.mark_running(b.id).await.unwrap();

        let updated = terminate_orphans(&mut tx, &ids, &TerminalReason::ShutdownRequested)
            .await
            .unwrap();
        assert_eq!(updated, vec![a.id]);
        tx.commit().await.unwrap();

        let gb = store.get_investigation(b.id).await.unwrap();
        assert_eq!(
            gb.status,
            InvestigationStatus::Running,
            "b must survive the boot cleanup untouched"
        );
        assert!(
            store.steps_after(b.id, -1).await.unwrap().is_empty(),
            "b must get no Terminated step"
        );

        let ga = store.get_investigation(a.id).await.unwrap();
        assert_eq!(ga.status, InvestigationStatus::Failed);
        assert!(
            ga.finished_at.is_some(),
            "finished_at must be set when failing"
        );
        let steps = store.steps_after(a.id, -1).await.unwrap();
        let terminated = steps
            .iter()
            .filter(|s| matches!(s.kind, StepKind::Terminated { .. }))
            .count();
        assert_eq!(terminated, 1, "a gets exactly one Terminated step");
    }
}

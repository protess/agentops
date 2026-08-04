use agentops_core::{
    Investigation, InvestigationStatus, NewArtifact, StepKind, Store, StoreError, TerminalReason,
    TriggeredBy,
};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

async fn running(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: "t".into(),
        prompt: "p".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    };
    store.create_investigation(&inv).await.unwrap();
    store.mark_running(inv.id).await.unwrap();
    inv.id
}

/// TEST-14 — termination is one transaction. The success path.
#[sqlx::test(migrations = "../../migrations")]
async fn test_14_complete_commits_all_three_writes(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;

    let artifact_id = store
        .complete_investigation(
            id,
            &NewArtifact {
                title: "RCA".into(),
                body: "# Conclusion\nThe cause is X".into(),
            },
        )
        .await
        .unwrap();

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Completed);
    assert!(inv.finished_at.is_some());

    let a = store.get_artifact(artifact_id).await.unwrap();
    assert_eq!(a.title, "RCA");
    assert_eq!(a.investigation_id, Some(id));

    let steps = store.steps_after(id, -1).await.unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::ArtifactWritten { artifact_id });
}

/// TEST-14 — that no partial commit survives. **Verified by injecting an error** — a
/// test that only exercises the success path does not verify this item (spec Section 12.1).
///
/// The artifact insert succeeds and then the terminal step insert is made to fail.
/// Pre-inserting a row at the same `seq` that violates `agent_steps`'s `payload ? 'v'`
/// CHECK makes the next insert fail on the primary key — at which point both the
/// artifact and the status transition must roll back.
#[sqlx::test(migrations = "../../migrations")]
async fn test_14_partial_failure_rolls_back_everything(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;

    // Leaving next_step_seq at 0, occupy seq 0 in advance.
    // complete_investigation gets 0 from allocate_seq and collides on insert.
    sqlx::query(
        "INSERT INTO agent_steps (investigation_id, seq, phase, kind, payload)
         VALUES ($1, 0, 'all', 'text', '{\"v\":1,\"kind\":\"text\",\"text\":\"squatter\"}')",
    )
    .bind(id)
    .execute(store.pool())
    .await
    .unwrap();

    let err = store
        .complete_investigation(
            id,
            &NewArtifact {
                title: "lost".into(),
                body: "b".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Conflict),
        "a seq collision must surface, not be swallowed; got {err:?}"
    );

    // All three must roll back
    assert_eq!(
        store.get_investigation(id).await.unwrap().status,
        InvestigationStatus::Running,
        "status must not have transitioned"
    );
    assert!(
        store.list_artifacts(10).await.unwrap().is_empty(),
        "artifact must not survive the rollback"
    );
    let steps = store.steps_after(id, -1).await.unwrap();
    assert_eq!(steps.len(), 1, "only the pre-inserted squatter remains");
}

/// TEST-13 — the shutdown race. **Verified with real concurrency** — sequential calls do
/// not verify this item (spec Section 12.1).
#[sqlx::test(migrations = "../../migrations")]
async fn test_13_concurrent_terminal_writes_yield_exactly_one_winner(pool: sqlx::PgPool) {
    let store = std::sync::Arc::new(PgStore::new(pool));
    let id = running(&store).await;

    let s1 = std::sync::Arc::clone(&store);
    let s2 = std::sync::Arc::clone(&store);
    let (completed, failed) = tokio::join!(
        async move {
            s1.complete_investigation(
                id,
                &NewArtifact {
                    title: "c".into(),
                    body: "b".into(),
                },
            )
            .await
        },
        async move {
            s2.fail_investigation(id, &TerminalReason::ShutdownRequested)
                .await
        },
    );

    let winners = [completed.is_ok(), failed.is_ok()]
        .iter()
        .filter(|ok| **ok)
        .count();
    assert_eq!(winners, 1, "exactly one terminal write may succeed");

    let loser_err = if completed.is_err() {
        format!("{:?}", completed.unwrap_err())
    } else {
        format!("{:?}", failed.unwrap_err())
    };
    assert!(
        loser_err.contains("Conflict"),
        "loser must get Conflict, got {loser_err}"
    );

    // TEST-19 — exactly one terminal step from the winner survives
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminal = steps
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                StepKind::Terminated { .. } | StepKind::ArtifactWritten { .. }
            )
        })
        .count();
    assert_eq!(terminal, 1, "exactly one terminal step must land");
    assert!(store
        .get_investigation(id)
        .await
        .unwrap()
        .status
        .is_terminal());
}

/// On failure no artifact is created, but a Terminated step is left behind.
#[sqlx::test(migrations = "../../migrations")]
async fn fail_records_structured_reason(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;

    store
        .fail_investigation(
            id,
            &TerminalReason::Refusal {
                category: Some("cyber".into()),
            },
        )
        .await
        .unwrap();

    let steps = store.steps_after(id, -1).await.unwrap();
    assert_eq!(steps.len(), 1);
    match &steps[0].kind {
        StepKind::Terminated { reason, .. } => assert_eq!(
            reason,
            &TerminalReason::Refusal {
                category: Some("cyber".into())
            }
        ),
        other => panic!("expected Terminated, got {other:?}"),
    }
    assert!(store.list_artifacts(10).await.unwrap().is_empty());
}

/// The artifact survives even when the investigation is deleted (ON DELETE SET NULL).
#[sqlx::test(migrations = "../../migrations")]
async fn artifact_survives_investigation_deletion(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;
    let aid = store
        .complete_investigation(
            id,
            &NewArtifact {
                title: "keep".into(),
                body: "b".into(),
            },
        )
        .await
        .unwrap();

    sqlx::query("DELETE FROM investigations WHERE id = $1")
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();

    let a = store.get_artifact(aid).await.unwrap();
    assert_eq!(a.investigation_id, None);
}

/// On an identical `created_at`, `id DESC` breaks the tie. A check that only looks for
/// monotonic non-increase (`>=`) passes even without the tiebreaker, so this forces an
/// identical `created_at` and pins the exact returned order.
#[sqlx::test(migrations = "../../migrations")]
async fn list_artifacts_is_newest_first(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let tie = OffsetDateTime::now_utc();
    let mut ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
    ids.sort(); // pin ascending, then check the DESC query returns exactly the reverse

    for (n, id) in ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO artifacts (id, investigation_id, title, body, created_at)
             VALUES ($1, NULL, $2, 'b', $3)",
        )
        .bind(id)
        .bind(format!("a{n}"))
        .bind(tie)
        .execute(store.pool())
        .await
        .unwrap();
    }

    let list = store.list_artifacts(10).await.unwrap();
    let got: Vec<Uuid> = list.iter().map(|a| a.id).collect();
    let mut expected = ids.clone();
    expected.reverse();
    assert_eq!(
        got, expected,
        "created_at ties must be broken by id DESC — full order must be pinned"
    );
}

/// `list_artifacts` must clamp a degenerate limit by the same convention as
/// `investigations::list` — a limit of zero, negative, or oversized must neither error
/// nor read the whole table.
#[sqlx::test(migrations = "../../migrations")]
async fn list_artifacts_tolerates_degenerate_limits(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;
    store
        .complete_investigation(
            id,
            &NewArtifact {
                title: "only".into(),
                body: "b".into(),
            },
        )
        .await
        .unwrap();

    for limit in [0_i64, -1, i64::MAX] {
        let list = store
            .list_artifacts(limit)
            .await
            .unwrap_or_else(|e| panic!("limit {limit} must not error, got {e:?}"));
        assert!(list.len() <= 1);
    }
}

/// `UnknownStopReason` must be a struct variant to serialize — this is where an
/// internally-tagged enum used to panic on a newtype variant wrapping a string, so this
/// confirms the variant survives a real Postgres round trip.
#[sqlx::test(migrations = "../../migrations")]
async fn fail_records_unknown_stop_reason(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;

    store
        .fail_investigation(
            id,
            &TerminalReason::UnknownStopReason {
                stop_reason: "future_variant".into(),
            },
        )
        .await
        .unwrap();

    let steps = store.steps_after(id, -1).await.unwrap();
    assert_eq!(steps.len(), 1);
    match &steps[0].kind {
        StepKind::Terminated { reason, .. } => assert_eq!(
            reason,
            &TerminalReason::UnknownStopReason {
                stop_reason: "future_variant".into()
            }
        ),
        other => panic!("expected Terminated, got {other:?}"),
    }
}

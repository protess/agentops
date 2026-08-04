use agentops_core::{
    Investigation, InvestigationStatus, Phase, StepKind, Store, TerminalReason, TriggeredBy,
    STEP_PAYLOAD_VERSION,
};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

/// The same shape as the helper in the Task 7 tests. Duplicated per file — not large
/// enough to justify a shared test-utility crate.
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

async fn running_investigation(store: &PgStore) -> Uuid {
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

/// Section 6.1.1 — the database allocates `seq`, so the caller does not manage 0, 1, 2, and so on.
#[sqlx::test(migrations = "../../migrations")]
async fn seq_is_allocated_by_the_database(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;

    let a = store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "a".into() })
        .await
        .unwrap();
    let b = store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "b".into() })
        .await
        .unwrap();
    assert_eq!((a, b), (0, 1), "the store, not the caller, assigns seq");
}

/// TEST-4 — replay by the after parameter. A new connection receives only what follows after.
#[sqlx::test(migrations = "../../migrations")]
async fn test_4_steps_after_returns_only_later_seqs(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    for n in 0..5 {
        store
            .append_step(
                id,
                Phase::Triage,
                &StepKind::Text {
                    text: format!("t{n}"),
                },
            )
            .await
            .unwrap();
    }

    let from_start = store.steps_after(id, -1).await.unwrap();
    assert_eq!(from_start.len(), 5);
    assert_eq!(from_start[0].seq, 0, "must be ordered ascending");

    let after_2 = store.steps_after(id, 2).await.unwrap();
    assert_eq!(
        after_2.iter().map(|s| s.seq).collect::<Vec<_>>(),
        vec![3, 4]
    );

    let after_end = store.steps_after(id, 99).await.unwrap();
    assert!(
        after_end.is_empty(),
        "out-of-range after yields empty, not an error"
    );
}

/// TEST-11 — the tool_use_id of parallel tool calls survives the round trip.
#[sqlx::test(migrations = "../../migrations")]
async fn test_11_tool_use_ids_survive_round_trip(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;

    for (i, tid) in ["toolu_a", "toolu_b", "toolu_c"].iter().enumerate() {
        store
            .append_step(
                id,
                Phase::Triage,
                &StepKind::ToolCall {
                    tool_use_id: (*tid).into(),
                    tool: "prom__query".into(),
                    input: serde_json::json!({"i": i}),
                },
            )
            .await
            .unwrap();
    }

    let steps = store.steps_after(id, -1).await.unwrap();
    let ids: Vec<_> = steps.iter().filter_map(|s| s.kind.tool_use_id()).collect();
    assert_eq!(ids, vec!["toolu_a", "toolu_b", "toolu_c"]);
}

/// Concurrent appends never receive the same seq. Without database allocation this fails.
#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_appends_get_distinct_seqs(pool: sqlx::PgPool) {
    let store = std::sync::Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let mut handles = Vec::new();
    for n in 0..20 {
        let store = std::sync::Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            store
                .append_step(
                    id,
                    Phase::Triage,
                    &StepKind::Text {
                        text: format!("{n}"),
                    },
                )
                .await
        }));
    }
    let mut seqs = Vec::new();
    for h in handles {
        seqs.push(h.await.unwrap().expect("concurrent append must not fail"));
    }
    seqs.sort_unstable();
    assert_eq!(
        seqs,
        (0..20).collect::<Vec<i64>>(),
        "no gaps, no duplicates"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn payload_carries_version_field(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "x".into() })
        .await
        .unwrap();

    let v: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM agent_steps WHERE investigation_id = $1 AND seq = 0",
    )
    .bind(id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(v["v"], STEP_PAYLOAD_VERSION);
}

/// INV-4 — the updated_at the watchdog reads must advance on a step append too.
/// Without that, an in-flight investigation is misjudged as stalled.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_4_append_step_touches_investigation_updated_at(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    let before = store.get_investigation(id).await.unwrap().updated_at;

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "x".into() })
        .await
        .unwrap();

    let after = store.get_investigation(id).await.unwrap().updated_at;
    assert!(
        after > before,
        "append_step must bump investigations.updated_at (before={before}, after={after})"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn max_seq_reports_none_then_highest(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    assert_eq!(store.max_step_seq(id).await.unwrap(), None);

    for _ in 0..3 {
        store
            .append_step(id, Phase::Triage, &StepKind::Text { text: "x".into() })
            .await
            .unwrap();
    }
    assert_eq!(store.max_step_seq(id).await.unwrap(), Some(2));
}

// The real race test for INV-4 / TEST-20 lives in
// `crates/agentops-store/src/steps.rs`, in
// `tests::test_20_race_interleaving_leaves_newly_running_investigation_untouched`.
// Constructing the interleaving between the lock and the UPDATE requires calling the
// `pub(crate)` `select_orphans` and `terminate_orphans` directly in mid-transaction, and
// neither is visible from this integration test crate.

/// TEST-19 — cleaning several investigations at once still gives each exactly one terminal step.
#[sqlx::test(migrations = "../../migrations")]
async fn test_19_every_cleaned_investigation_gets_exactly_one_terminal_step(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let mut ids = Vec::new();
    for n in 0..5 {
        let inv = queued(&format!("r{n}"));
        store.create_investigation(&inv).await.unwrap();
        store.mark_running(inv.id).await.unwrap();
        // Mix in ordinary steps before cleanup so seq is not 0
        store
            .append_step(
                inv.id,
                Phase::Triage,
                &StepKind::Text {
                    text: "work".into(),
                },
            )
            .await
            .unwrap();
        ids.push(inv.id);
    }

    let n = store
        .fail_orphaned_running(&TerminalReason::TaskPanicked)
        .await
        .unwrap();
    assert_eq!(n, 5);

    for id in ids {
        let steps = store.steps_after(id, -1).await.unwrap();
        let terminated = steps
            .iter()
            .filter(|s| matches!(s.kind, StepKind::Terminated { .. }))
            .count();
        assert_eq!(
            terminated, 1,
            "investigation {id} must have exactly one Terminated step"
        );
        assert_eq!(
            store.get_investigation(id).await.unwrap().status,
            InvestigationStatus::Failed
        );
    }
}

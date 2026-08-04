use agentops_core::{Investigation, InvestigationStatus, ListFilter, Store, TriggeredBy};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

fn queued(title: &str) -> Investigation {
    let now = OffsetDateTime::now_utc();
    Investigation {
        id: Uuid::new_v4(),
        title: title.into(),
        prompt: "why is latency high".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_then_get_round_trips(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let inv = queued("first");
    store.create_investigation(&inv).await.unwrap();

    let got = store.get_investigation(inv.id).await.unwrap();
    assert_eq!(got.id, inv.id);
    assert_eq!(got.status, InvestigationStatus::Queued);
    assert_eq!(got.triggered_by, TriggeredBy::User);
    assert!(got.started_at.is_none());
    assert!(got.finished_at.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn alarm_trigger_preserves_source(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let mut inv = queued("alarm");
    inv.triggered_by = TriggeredBy::Alarm {
        source: "cpu-high".into(),
    };
    store.create_investigation(&inv).await.unwrap();

    let got = store.get_investigation(inv.id).await.unwrap();
    assert_eq!(
        got.triggered_by,
        TriggeredBy::Alarm {
            source: "cpu-high".into()
        }
    );
}

/// INV-4: the terminal transition is conditional. `queued` → `running` succeeds only once.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_4_mark_running_is_conditional(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let inv = queued("once");
    store.create_investigation(&inv).await.unwrap();

    store.mark_running(inv.id).await.unwrap();
    let got = store.get_investigation(inv.id).await.unwrap();
    assert_eq!(got.status, InvestigationStatus::Running);
    assert!(
        got.started_at.is_some(),
        "started_at must be set on running"
    );

    // The second call is a Conflict
    let err = store.mark_running(inv.id).await.unwrap_err();
    assert!(
        matches!(err, agentops_core::StoreError::Conflict),
        "second mark_running must conflict, got {err:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn queued_ids_restores_the_queue(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let a = queued("a");
    let b = queued("b");
    store.create_investigation(&a).await.unwrap();
    store.create_investigation(&b).await.unwrap();
    store.mark_running(a.id).await.unwrap();

    let ids = store.queued_ids().await.unwrap();
    assert_eq!(ids, vec![b.id]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_by_status_and_pages_by_cursor(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    for i in 0..5 {
        let mut inv = queued(&format!("inv-{i}"));
        // Give each a different queued_at so the ordering is deterministic
        inv.queued_at = OffsetDateTime::now_utc() - time::Duration::seconds(i);
        store.create_investigation(&inv).await.unwrap();
    }

    let page1 = store
        .list_investigations(&ListFilter {
            limit: 2,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next_cursor.is_some());

    let page2 = store
        .list_investigations(&ListFilter {
            limit: 2,
            cursor: page1.next_cursor,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page2.items.len(), 2);
    let ids1: Vec<_> = page1.items.iter().map(|i| i.id).collect();
    let ids2: Vec<_> = page2.items.iter().map(|i| i.id).collect();
    assert!(
        ids1.iter().all(|id| !ids2.contains(id)),
        "pages must not overlap"
    );

    let only_queued = store
        .list_investigations(&ListFilter {
            status: Some(InvestigationStatus::Running),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(only_queued.items.is_empty());
}

/// Whether `id` breaks the tie on an identical `queued_at`. The previous test gives every
/// row a distinct timestamp and never takes this path.
#[sqlx::test(migrations = "../../migrations")]
async fn list_breaks_ties_on_id_at_identical_timestamps(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let same = OffsetDateTime::now_utc();
    let mut ids = Vec::new();
    for i in 0..4 {
        let mut inv = queued(&format!("tie-{i}"));
        inv.queued_at = same;
        ids.push(inv.id);
        store.create_investigation(&inv).await.unwrap();
    }

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .list_investigations(&ListFilter {
                limit: 2,
                cursor,
                ..Default::default()
            })
            .await
            .unwrap();
        seen.extend(page.items.iter().map(|i| i.id));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    ids.sort_unstable();
    let mut got = seen.clone();
    got.sort_unstable();
    assert_eq!(got, ids, "every row must appear exactly once across pages");
    assert_eq!(
        seen.len(),
        4,
        "no row duplicated or skipped at a tie boundary"
    );
}

/// A `limit` of zero or below, or an oversized one, neither panics nor produces a SQL error.
#[sqlx::test(migrations = "../../migrations")]
async fn list_tolerates_degenerate_limits(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    store.create_investigation(&queued("one")).await.unwrap();

    for limit in [0_i64, -1, i64::MAX] {
        let page = store
            .list_investigations(&ListFilter {
                limit,
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("limit {limit} must not error, got {e:?}"));
        assert!(page.items.len() <= 1);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_missing_investigation_is_not_found(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let err = store.get_investigation(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, agentops_core::StoreError::NotFound));
}

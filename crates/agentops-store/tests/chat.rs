use agentops_core::{ChatRole, ChatSession, Store, StoreError};
use agentops_store::PgStore;
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

async fn session(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let s = ChatSession {
        id: Uuid::new_v4(),
        title: "chat".into(),
        created_at: now,
        updated_at: now,
    };
    store.create_chat_session(&s).await.unwrap();
    s.id
}

#[sqlx::test(migrations = "../../migrations")]
async fn append_returns_monotonic_seq(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let sid = session(&store).await;

    let a = store
        .append_chat_message(sid, ChatRole::User, &serde_json::json!("hi"))
        .await
        .unwrap();
    let b = store
        .append_chat_message(sid, ChatRole::Assistant, &serde_json::json!("hello"))
        .await
        .unwrap();
    assert_eq!((a, b), (0, 1));
}

/// TEST-7 — seq does not collide when there are two writers.
/// If the store did not allocate atomically, this test would fail on a PK violation.
#[sqlx::test(migrations = "../../migrations")]
async fn test_7_concurrent_appends_do_not_collide(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let sid = session(&store).await;

    let mut handles = Vec::new();
    for n in 0..20 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let role = if n % 2 == 0 {
                ChatRole::User
            } else {
                ChatRole::Assistant
            };
            store
                .append_chat_message(sid, role, &serde_json::json!(n))
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
        "seqs must be 0..20 with no gaps or dupes"
    );

    let msgs = store.chat_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 20);
}

#[sqlx::test(migrations = "../../migrations")]
async fn messages_are_ordered_by_seq(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let sid = session(&store).await;
    for n in 0..5 {
        store
            .append_chat_message(sid, ChatRole::User, &serde_json::json!(n))
            .await
            .unwrap();
    }
    let msgs = store.chat_messages(sid).await.unwrap();
    assert_eq!(
        msgs.iter().map(|m| m.seq).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
}

/// The updated_at used for sidebar ordering advances when a message is appended.
#[sqlx::test(migrations = "../../migrations")]
async fn append_touches_session_updated_at(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let sid = session(&store).await;
    let before = store.list_chat_sessions(10).await.unwrap()[0].updated_at;

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store
        .append_chat_message(sid, ChatRole::User, &serde_json::json!("x"))
        .await
        .unwrap();

    let after = store.list_chat_sessions(10).await.unwrap()[0].updated_at;
    assert!(after > before);
}

/// This test never uses the two sessions' `created_at` or initial `updated_at` (the
/// application clock, bound by `session()`) in any comparison. The `updated_at` being
/// compared is, in both cases, the value after
/// `UPDATE chat_sessions SET updated_at = now()` (the DB clock) inside `append_message`
/// has overwritten it — so no amount of clock skew between the host and the database
/// server can affect whether this test passes. The sleep between the two appends only
/// separates the two values within the DB clock's own resolution (microseconds); it is
/// not slack for comparing two different clocks.
#[sqlx::test(migrations = "../../migrations")]
async fn sessions_are_newest_updated_first(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let old = session(&store).await;
    let new = session(&store).await;

    store
        .append_chat_message(new, ChatRole::User, &serde_json::json!("bump-new"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store
        .append_chat_message(old, ChatRole::User, &serde_json::json!("bump-old"))
        .await
        .unwrap();

    let list = store.list_chat_sessions(10).await.unwrap();
    assert_eq!(
        list[0].id, old,
        "the session touched most recently comes first"
    );
    assert_eq!(list[1].id, new);
}

/// For a nonexistent session, when the `FOR UPDATE` lock attempt fails, `append_message`
/// rolls back explicitly and returns `StoreError::NotFound` (`chat.rs:87-90`). This
/// checks not only the error variant but that the rollback really left no side
/// effect.
#[sqlx::test(migrations = "../../migrations")]
async fn append_to_missing_session_is_not_found(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let missing = Uuid::new_v4();

    let sessions_before = store.list_chat_sessions(10).await.unwrap();

    let err = store
        .append_chat_message(missing, ChatRole::User, &serde_json::json!("hi"))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound));

    // Whether the rollback actually happened: no message row under the nonexistent
    // session, and the session list unchanged by any side effect.
    let msgs = store.chat_messages(missing).await.unwrap();
    assert!(
        msgs.is_empty(),
        "a failed append must not leave a chat_messages row behind"
    );

    let sessions_after = store.list_chat_sessions(10).await.unwrap();
    assert_eq!(
        sessions_after.len(),
        sessions_before.len(),
        "a failed append must not create or touch any session"
    );
}

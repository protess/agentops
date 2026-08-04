use agentops_core::{McpServer, McpTransport, Store, StoreError, ToolPolicy, ToolPolicyKind};
use agentops_store::PgStore;
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

fn server(name: &str, enabled: bool) -> McpServer {
    McpServer {
        id: Uuid::new_v4(),
        name: name.into(),
        transport: McpTransport::Stdio,
        // Holds no secrets — environment variable names only (spec Section 9.1).
        config: serde_json::json!({ "env": { "GITHUB_TOKEN": "$GITHUB_TOKEN" } }),
        enabled,
        created_at: OffsetDateTime::now_utc(),
    }
}

async fn insert_server(store: &PgStore, s: &McpServer) {
    sqlx::query(
        "INSERT INTO mcp_servers (id, name, transport, config, enabled)
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(s.id)
    .bind(&s.name)
    .bind(s.transport.as_str())
    .bind(&s.config)
    .bind(s.enabled)
    .execute(store.pool())
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn enabled_servers_exclude_disabled_ones(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    insert_server(&store, &server("gh", true)).await;
    insert_server(&store, &server("k8s", false)).await;
    insert_server(&store, &server("aws", true)).await;

    let got = store.enabled_mcp_servers().await.unwrap();
    let names: Vec<&str> = got.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["aws", "gh"],
        "must be deterministic, ordered by name"
    );
}

/// An unstable server list order makes the tools[] order unstable and the prompt cache
/// misses every time (spec Section 9). Repeated queries must be identical.
#[sqlx::test(migrations = "../../migrations")]
async fn enabled_servers_are_byte_stable_across_reads(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    for n in ["z", "a", "m"] {
        insert_server(&store, &server(n, true)).await;
    }
    let first: Vec<String> = store
        .enabled_mcp_servers()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    for _ in 0..5 {
        let again: Vec<String> = store
            .enabled_mcp_servers()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(first, again);
    }
    assert_eq!(first, vec!["a", "m", "z"]);
}

/// Spec Section 9.1 — no policy row means deny. The default is not stored in the database;
/// absence is read as deny. In a design where a row is required to make it deny, a newly
/// discovered tool would be exposed with no policy.
#[sqlx::test(migrations = "../../migrations")]
async fn a_missing_policy_row_reads_as_deny(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    insert_server(&store, &server("gh", true)).await;

    let got = store.tool_policy("gh", "never_configured").await.unwrap();
    assert_eq!(got, ToolPolicyKind::Deny, "no row means deny");
}

#[sqlx::test(migrations = "../../migrations")]
async fn upsert_then_read_policy_round_trips(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    insert_server(&store, &server("gh", true)).await;

    store
        .upsert_tool_policy(&ToolPolicy {
            server_name: "gh".into(),
            tool_name: "search".into(),
            policy: ToolPolicyKind::Allow,
            mutating: false,
            updated_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();

    assert_eq!(
        store.tool_policy("gh", "search").await.unwrap(),
        ToolPolicyKind::Allow
    );

    // Upserting the same (server, tool) again updates it.
    store
        .upsert_tool_policy(&ToolPolicy {
            server_name: "gh".into(),
            tool_name: "search".into(),
            policy: ToolPolicyKind::Deny,
            mutating: true,
            updated_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();

    assert_eq!(
        store.tool_policy("gh", "search").await.unwrap(),
        ToolPolicyKind::Deny
    );

    let all = store.tool_policies_for("gh").await.unwrap();
    assert_eq!(all.len(), 1, "an upsert must not add a row");
    assert!(all[0].mutating);
}

#[sqlx::test(migrations = "../../migrations")]
async fn policies_are_ordered_by_tool_name(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    insert_server(&store, &server("gh", true)).await;
    for t in ["zeta", "alpha", "mid"] {
        store
            .upsert_tool_policy(&ToolPolicy {
                server_name: "gh".into(),
                tool_name: t.into(),
                policy: ToolPolicyKind::Allow,
                mutating: false,
                updated_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    }
    let names: Vec<String> = store
        .tool_policies_for("gh")
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.tool_name)
        .collect();
    assert_eq!(names, vec!["alpha", "mid", "zeta"]);
}

/// Inserting a policy for a nonexistent server is an FK violation. Rather than being
/// flattened into Backend, it must be a variant the caller can distinguish.
#[sqlx::test(migrations = "../../migrations")]
async fn policy_for_unknown_server_is_rejected(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let err = store
        .upsert_tool_policy(&ToolPolicy {
            server_name: "no_such_server".into(),
            tool_name: "t".into(),
            policy: ToolPolicyKind::Allow,
            mutating: false,
            updated_at: OffsetDateTime::now_utc(),
        })
        .await
        .expect_err("must be an FK violation");
    assert!(matches!(err, StoreError::NotFound), "got {err:?}");
}

/// Plan 1's first unresolved item — the database computes the threshold. It never
/// compares the app clock against the DB clock, so clock skew cannot change the result.
#[sqlx::test(migrations = "../../migrations")]
async fn stale_running_uses_the_database_clock(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let inv = agentops_core::Investigation {
        id: Uuid::new_v4(),
        title: "t".into(),
        prompt: "p".into(),
        status: agentops_core::InvestigationStatus::Queued,
        triggered_by: agentops_core::TriggeredBy::User,
        queued_at: OffsetDateTime::now_utc(),
        started_at: None,
        finished_at: None,
        updated_at: OffsetDateTime::now_utc(),
    };
    store.create_investigation(&inv).await.unwrap();
    store.mark_running(inv.id).await.unwrap();

    // It just became running, so against a one-hour threshold it is not stale.
    assert!(store
        .stale_running_ids(Duration::from_secs(3600))
        .await
        .unwrap()
        .is_empty());

    // Pushing updated_at into the past on the DB clock makes it stale.
    //
    // **Briefly disable the `touch_updated_at` trigger.** It unconditionally overwrites
    // `updated_at = now()` on every UPDATE of `investigations`, so the UPDATE below that
    // pushes the value into the past is undone immediately — confirmed with psql.
    //
    // The irony: the very trigger that makes the watchdog design work (append_step marks
    // an investigation as alive) also blocks the backdating needed to test the watchdog.
    // In production this is not a problem — nobody UPDATEs a stalled investigation, so
    // `updated_at` ages naturally. The trigger fires only on UPDATE.
    //
    // **The first safety net is the transaction.** `ALTER TABLE ... DISABLE TRIGGER` is
    // transactional DDL in Postgres, so a panic before the commit rolls the DISABLE back
    // too and the trigger is restored **within the same test run.** `#[sqlx::test]`'s
    // per-test database isolation is the second layer, covering the case where the trigger somehow commits disabled.
    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query("ALTER TABLE investigations DISABLE TRIGGER t_investigations_touch")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE investigations SET updated_at = now() - interval '2 hours' WHERE id = $1")
        .bind(inv.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE investigations ENABLE TRIGGER t_investigations_touch")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        store
            .stale_running_ids(Duration::from_secs(3600))
            .await
            .unwrap(),
        vec![inv.id]
    );
}

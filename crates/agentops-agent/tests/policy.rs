use agentops_agent::policy::{split_namespaced, PolicyGate};
use agentops_core::{McpTransport, Store, ToolDef, ToolError, ToolPolicy, ToolPolicyKind};
use agentops_store::PgStore;
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

fn def(name: &str) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: "d".into(),
        input_schema: serde_json::json!({"type":"object"}),
    }
}

async fn server(store: &PgStore, name: &str) {
    sqlx::query(
        "INSERT INTO mcp_servers (id, name, transport, config, enabled) VALUES ($1,$2,$3,$4,true)",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(McpTransport::Stdio.as_str())
    .bind(serde_json::json!({}))
    .execute(store.pool())
    .await
    .unwrap();
}

async fn set_policy(store: &PgStore, srv: &str, tool: &str, p: ToolPolicyKind) {
    store
        .upsert_tool_policy(&ToolPolicy {
            server_name: srv.into(),
            tool_name: tool.into(),
            policy: p,
            mutating: false,
            updated_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
}

#[test]
fn namespaced_names_split_on_the_double_underscore() {
    assert_eq!(split_namespaced("gh__search"), Some(("gh", "search")));
    // Split on the first separator only, even when the tool name itself contains __.
    assert_eq!(split_namespaced("k8s__get__pod"), Some(("k8s", "get__pod")));
    assert_eq!(split_namespaced("no_separator"), None);
    assert_eq!(
        split_namespaced("__leading"),
        None,
        "an empty server name is invalid"
    );
    assert_eq!(
        split_namespaced("trailing__"),
        None,
        "an empty tool name is invalid"
    );
}

/// TEST-17 — a newly discovered tool defaults to deny and does not appear in list.
#[sqlx::test(migrations = "../../migrations")]
async fn test_17_newly_discovered_tools_are_denied_and_absent_from_list(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    server(&store, "gh").await;
    set_policy(&store, "gh", "search", ToolPolicyKind::Allow).await;

    let gate = PolicyGate::new(Arc::clone(&store));
    // brand_new has no policy row — a tool just discovered from an MCP server.
    let got = gate
        .filter_allowed(vec![def("gh__search"), def("gh__brand_new")])
        .await
        .unwrap();

    let names: Vec<&str> = got.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["gh__search"],
        "a tool with no policy was exposed"
    );

    // The call path must behave the same.
    let err = gate.assert_allowed("gh__brand_new").await.unwrap_err();
    assert!(matches!(err, ToolError::Denied(_)), "got {err:?}");
}

/// TEST-17 — changing the policy to deny after list must make call refuse.
/// Caching the list result without re-checking lets a forbidden tool run through this window.
#[sqlx::test(migrations = "../../migrations")]
async fn test_17_policy_flipped_after_list_blocks_the_call(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    server(&store, "gh").await;
    set_policy(&store, "gh", "search", ToolPolicyKind::Allow).await;

    let gate = PolicyGate::new(Arc::clone(&store));
    let listed = gate.filter_allowed(vec![def("gh__search")]).await.unwrap();
    assert_eq!(listed.len(), 1, "it was allow, so it must be in list");

    // The operator changed it to deny in Settings.
    set_policy(&store, "gh", "search", ToolPolicyKind::Deny).await;

    let err = gate.assert_allowed("gh__search").await.unwrap_err();
    assert!(
        matches!(err, ToolError::Denied(_)),
        "a policy change after list must block the call: got {err:?}"
    );
}

/// A name with no namespace can look up no server's policy, so it is denied.
/// Letting it through would create a path that executes with no policy.
#[sqlx::test(migrations = "../../migrations")]
async fn unnamespaced_tool_names_are_denied(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let gate = PolicyGate::new(store);

    let err = gate.assert_allowed("bare_tool").await.unwrap_err();
    assert!(matches!(err, ToolError::Denied(_)), "got {err:?}");

    let got = gate.filter_allowed(vec![def("bare_tool")]).await.unwrap();
    assert!(got.is_empty(), "a tool with no namespace was exposed");
}

/// **Review M4 regression test.** `filter_allowed` was changed to batch one
/// `tool_policies_for` per server instead of querying per tool. This server has **no
/// rows at all** in `tool_policies` — the case where the batched query returns an empty
/// `Vec`. If that flips into "the result is empty, so let it through", every tool of a
/// server whose policy registration was missed is exposed (Section 9.1). Both tools must
/// be denied.
#[sqlx::test(migrations = "../../migrations")]
async fn filter_allowed_denies_every_tool_when_the_server_has_no_policy_rows_at_all(
    pool: sqlx::PgPool,
) {
    let store = Arc::new(PgStore::new(pool));
    server(&store, "brand_new_server").await;
    // Deliberately upsert no policy at all — this exposes the path where
    // `tool_policies_for` returns an empty result.

    let gate = PolicyGate::new(Arc::clone(&store));
    let got = gate
        .filter_allowed(vec![
            def("brand_new_server__list_pods"),
            def("brand_new_server__delete_pod"),
        ])
        .await
        .unwrap();

    assert!(
        got.is_empty(),
        "a tool of a server with no policy rows was exposed through the batched path: {got:?}"
    );

    // The call path must have the same default.
    let err = gate
        .assert_allowed("brand_new_server__list_pods")
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::Denied(_)), "got {err:?}");
}

/// filter_allowed must preserve the input order — Task 8 does the sorting, but if the
/// gate disturbs the order that sort becomes meaningless.
#[sqlx::test(migrations = "../../migrations")]
async fn filter_preserves_input_order(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    server(&store, "s").await;
    for t in ["c", "a", "b"] {
        set_policy(&store, "s", t, ToolPolicyKind::Allow).await;
    }
    let gate = PolicyGate::new(store);
    let got = gate
        .filter_allowed(vec![def("s__c"), def("s__a"), def("s__b")])
        .await
        .unwrap();
    let names: Vec<&str> = got.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["s__c", "s__a", "s__b"]);
}

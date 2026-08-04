use agentops_agent::mcp::{McpToolDef, McpToolRegistry, MAX_TOOL_OUTPUT_CHARS, TRUNCATION_MARKER};
use agentops_agent::mcp_client::{McpCallResult, McpConnection};
use agentops_agent::policy::PolicyGate;
use agentops_core::{McpTransport, Store, ToolError, ToolPolicy, ToolPolicyKind, ToolRegistry};
use agentops_store::PgStore;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use uuid::Uuid;

fn tool_def(name: &str) -> McpToolDef {
    McpToolDef {
        tool_name: name.into(),
        description: format!("{name} desc"),
        input_schema: serde_json::json!({"type":"object","properties":{}}),
    }
}

/// A fake connection. Verifies the whole registry without rmcp.
struct Fake {
    name: String,
    /// `Mutex` — `push_tool` must be able to change the tool set from behind an
    /// `Arc<Fake>` to imitate "the server exposes a new tool mid-phase".
    tools: Mutex<Vec<McpToolDef>>,
    output: String,
    fail_list: bool,
    /// Injects a delay to create the timeout path.
    delay: Option<std::time::Duration>,
    /// A failure the server reports through `CallToolResult.is_error` (spec Section 9, MCP).
    is_error: bool,
    calls: Arc<AtomicUsize>,
}

impl Fake {
    fn new(name: &str, tools: &[&str]) -> Self {
        Self {
            name: name.into(),
            tools: Mutex::new(tools.iter().map(|t| tool_def(t)).collect()),
            output: "ok".into(),
            fail_list: false,
            delay: None,
            is_error: false,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Creates the situation where the server exposes a new tool after the phase started.
    fn push_tool(&self, name: &str) {
        self.tools.lock().unwrap().push(tool_def(name));
    }

    /// Replaces the exposed set wholesale — imitating a tool disappearing through a
    /// deployment or configuration change (`push_tool` only adds and cannot remove).
    fn set_tools(&self, names: &[&str]) {
        *self.tools.lock().unwrap() = names.iter().map(|t| tool_def(t)).collect();
    }
}

#[async_trait]
impl McpConnection for Fake {
    fn server_name(&self) -> &str {
        &self.name
    }
    async fn list_tools(&self) -> Result<Vec<McpToolDef>, ToolError> {
        if self.fail_list {
            return Err(ToolError::Transport("server is down".into()));
        }
        Ok(self.tools.lock().unwrap().clone())
    }
    async fn call_tool(
        &self,
        _tool: &str,
        _input: serde_json::Value,
    ) -> Result<McpCallResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        Ok(McpCallResult {
            text: self.output.clone(),
            is_error: self.is_error,
        })
    }
}

async fn seed(store: &PgStore, server: &str, allow: &[&str]) {
    sqlx::query(
        "INSERT INTO mcp_servers (id, name, transport, config, enabled) VALUES ($1,$2,$3,$4,true)",
    )
    .bind(Uuid::new_v4())
    .bind(server)
    .bind(McpTransport::Stdio.as_str())
    .bind(serde_json::json!({}))
    .execute(store.pool())
    .await
    .unwrap();

    for t in allow {
        store
            .upsert_tool_policy(&ToolPolicy {
                server_name: server.into(),
                tool_name: (*t).into(),
                policy: ToolPolicyKind::Allow,
                mutating: false,
                updated_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    }
}

/// Tool names are namespaced as `{server}__{tool}` (spec Section 9).
/// Without it, two servers holding the same tool name collide.
#[sqlx::test(migrations = "../../migrations")]
async fn tools_are_namespaced_by_server(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["search"]).await;
    seed(&store, "k8s", &["search"]).await;

    let reg = McpToolRegistry::new(
        vec![
            Arc::new(Fake::new("gh", &["search"])) as Arc<dyn McpConnection>,
            Arc::new(Fake::new("k8s", &["search"])),
        ],
        PolicyGate::new(Arc::clone(&store)),
    );

    let names: Vec<String> = reg
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, vec!["gh__search", "k8s__search"]);
}

/// **`tools[]` must be sorted by namespaced name** (spec Section 9).
/// If the server connection order or the tools/list response order is unstable, the tools
/// at the front of the prompt prefix change and the cache misses every time.
#[sqlx::test(migrations = "../../migrations")]
async fn tools_are_sorted_deterministically_regardless_of_connection_order(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "zeta", &["b", "a"]).await;
    seed(&store, "alpha", &["y", "x"]).await;

    let want = vec!["alpha__x", "alpha__y", "zeta__a", "zeta__b"];

    // Reversing the connection order must give the same result.
    for order in [vec!["zeta", "alpha"], vec!["alpha", "zeta"]] {
        let conns: Vec<Arc<dyn McpConnection>> = order
            .iter()
            .map(|s| {
                let tools: &[&str] = if *s == "zeta" {
                    &["b", "a"]
                } else {
                    &["y", "x"]
                };
                Arc::new(Fake::new(s, tools)) as Arc<dyn McpConnection>
            })
            .collect();
        let reg = McpToolRegistry::new(conns, PolicyGate::new(Arc::clone(&store)));
        let names: Vec<String> = reg
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(
            names, want,
            "the order differed under connection order {order:?}"
        );
    }
}

/// One dead server does not stop the rest of the tools being offered (spec Section 9).
/// Failing the whole thing would let one server's failure block an entire investigation.
#[sqlx::test(migrations = "../../migrations")]
async fn one_dead_server_does_not_hide_the_others(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "up", &["good"]).await;
    seed(&store, "down", &["never"]).await;

    let mut dead = Fake::new("down", &["never"]);
    dead.fail_list = true;

    let reg = McpToolRegistry::new(
        vec![
            Arc::new(dead) as Arc<dyn McpConnection>,
            Arc::new(Fake::new("up", &["good"])),
        ],
        PolicyGate::new(Arc::clone(&store)),
    );

    let names: Vec<String> = reg
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, vec!["up__good"]);
}

/// The policy gate must actually be attached to the registry — a tool that was not
/// allowed appearing in list would disable Section 9.1.
#[sqlx::test(migrations = "../../migrations")]
async fn list_excludes_tools_without_an_allow_policy(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["allowed"]).await;

    let reg = McpToolRegistry::new(
        vec![Arc::new(Fake::new("gh", &["allowed", "not_allowed"])) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    let names: Vec<String> = reg
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, vec!["gh__allowed"]);
}

/// call re-checks the policy and, when denied, must not call the connection at all.
/// Calling and then discarding the result means the side effect already happened.
#[sqlx::test(migrations = "../../migrations")]
async fn denied_call_never_reaches_the_connection(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &[]).await;

    let fake = Arc::new(Fake::new("gh", &["danger"]));
    let calls = Arc::clone(&fake.calls);
    let reg = McpToolRegistry::new(
        vec![fake as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    let err = reg
        .call("gh__danger", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::Denied(_)), "got {err:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a denied tool was executed — the side effect already happened"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_tool_is_not_found(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["exists"]).await;

    let reg = McpToolRegistry::new(
        vec![Arc::new(Fake::new("gh", &["exists"])) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    // The policy is allow but no such tool exists.
    store
        .upsert_tool_policy(&ToolPolicy {
            server_name: "gh".into(),
            tool_name: "ghost".into(),
            policy: ToolPolicyKind::Allow,
            mutating: false,
            updated_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();

    // In the order plan 3's runner actually uses: list() once at phase start.
    reg.list().await.unwrap();

    let err = reg
        .call("gh__ghost", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::NotFound(_)), "got {err:?}");
}

/// Spec Section 9 — the tool list is frozen for the duration of a phase. Even if the
/// server exposes a new tool after `list()`, that phase's `call()` does not know it.
/// Re-listing per call would change the tool set within one phase and break the prompt cache.
#[sqlx::test(migrations = "../../migrations")]
async fn the_tool_set_is_fixed_at_list_time(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    // Allow "new_tool" in advance — what is checked later is not the policy but
    // "does call() know a tool that appeared after list()".
    seed(&store, "gh", &["search", "new_tool"]).await;

    let fake = Arc::new(Fake::new("gh", &["search"]));
    let reg = McpToolRegistry::new(
        vec![Arc::clone(&fake) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    // Phase start — at this point only "search" exists.
    let names: Vec<String> = reg
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, vec!["gh__search"]);

    // The server exposes a new tool mid-phase.
    fake.push_tool("new_tool");

    // This phase's call() still knows only the set as of list().
    let err = reg
        .call("gh__new_tool", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::NotFound(_)), "got {err:?}");
}

/// Two registries sharing the same connection do not overwrite each other's `known` cache.
/// If B erased the tool set A saw while two investigations run concurrently, A would
/// receive `NotFound` for an allowed tool — whether `JobManager` (plan 3) creating a
/// fresh registry per investigation while sharing the connections is safe rests on this.
///
/// **The fixture the brief supplied had no detection power.** In the original, A and B
/// both allow the same two tools (`s__a`, `s__b`) under the same policy, so the set the
/// two registries compute is always identical whether `known` is per-instance or shared
/// somewhere — "it passes whether or not it is overwritten", verifying nothing (the same
/// shape as CLAUDE.md's "presence is not multiplicity"). Instead, after A calls `list()`
/// the server exposes fewer tools (`set_tools`) so B's `list()` result differs from A's —
/// if `known` were shared anywhere, B's `list()` would overwrite A's cache with
/// `{gh__b}` and `ra.call("gh__a")` below would fail with `NotFound`.
#[sqlx::test(migrations = "../../migrations")]
async fn two_registries_sharing_connections_do_not_clobber_each_others_cache(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["a", "b"]).await;

    let fake = Arc::new(Fake::new("gh", &["a", "b"]));
    let conn: Arc<dyn McpConnection> = Arc::clone(&fake) as Arc<dyn McpConnection>;

    let ra = McpToolRegistry::new(vec![Arc::clone(&conn)], PolicyGate::new(Arc::clone(&store)));
    let rb = McpToolRegistry::new(vec![Arc::clone(&conn)], PolicyGate::new(Arc::clone(&store)));

    // A gets the list first — its cache at this point is {gh__a, gh__b}.
    let names_a: Vec<String> = ra
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names_a, vec!["gh__a", "gh__b"]);

    // The server stops exposing "a" (before B gets its list).
    fake.set_tools(&["b"]);

    // B gets its list later — B's cache is {gh__b} alone.
    let names_b: Vec<String> = rb
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names_b, vec!["gh__b"]);

    // A must still be able to call "a" from its own cache — if B's list() had
    // overwritten A's cache, this call would be `NotFound`.
    assert!(
        ra.call("gh__a", serde_json::json!({})).await.is_ok(),
        "B's list() overwrote A's cache"
    );
}

/// Output over 100K characters is truncated and marked truncated (spec Section 9).
#[sqlx::test(migrations = "../../migrations")]
async fn oversized_output_is_truncated_and_flagged(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["big"]).await;

    let mut fake = Fake::new("gh", &["big"]);
    fake.output = "x".repeat(MAX_TOOL_OUTPUT_CHARS + 500);

    let reg = McpToolRegistry::new(
        vec![Arc::new(fake) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    let out = reg.call("gh__big", serde_json::json!({})).await.unwrap();
    assert!(out.truncated, "the truncated marker is missing");
    assert!(!out.is_error, "truncation is not an error");
    assert!(
        out.text.chars().count() <= MAX_TOOL_OUTPUT_CHARS,
        "it was not truncated: {} chars",
        out.text.chars().count()
    );
}

/// **Multi-byte output must be cut on a character boundary.**
///
/// What this test protects is not the comparison operator but **the slicing line.** The
/// current code cuts with `raw.chars().take(N).collect()`, so character boundaries are
/// structurally safe, but changing it to a byte index such as `&raw[..N]` would panic on
/// a multi-byte boundary or break UTF-8. An ASCII-only fixture would not catch that change.
#[sqlx::test(migrations = "../../migrations")]
async fn multibyte_output_is_truncated_on_a_character_boundary(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["wide"]).await;

    let mut fake = Fake::new("gh", &["wide"]);
    // '한' is 3 bytes in UTF-8. It exceeds the ceiling by character count, and cutting by byte breaks the boundary.
    fake.output = "한".repeat(MAX_TOOL_OUTPUT_CHARS + 100);

    let reg = McpToolRegistry::new(
        vec![Arc::new(fake) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    let out = reg.call("gh__wide", serde_json::json!({})).await.unwrap();
    assert!(out.truncated, "the truncation marker is missing");
    assert_eq!(
        out.text.chars().count(),
        MAX_TOOL_OUTPUT_CHARS,
        "it must be cut by character count — including the marker it stays under the ceiling"
    );

    // **Strip the marker and look only at the content.** With the marker attached, the
    // `len() == N*3` assertion no longer holds (the marker is ASCII). What that assertion
    // protected was the **slicing line**, not the arithmetic, so the content is inspected
    // directly to keep the same protection — cutting by byte index would panic before reaching here, or mix in bytes that are not '한'.
    let content = out
        .text
        .strip_suffix(TRUNCATION_MARKER)
        .expect("truncated output must carry the marker (spec Section 9)");
    assert!(
        content.chars().all(|c| c == '한'),
        "the character boundary broke and characters other than '한' mixed in"
    );
    assert_eq!(
        content.len(),
        content.chars().count() * 3,
        "3-byte characters must be preserved intact"
    );
}

/// Normally sized output is not truncated.
#[sqlx::test(migrations = "../../migrations")]
async fn normal_output_is_not_flagged_as_truncated(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["small"]).await;
    let reg = McpToolRegistry::new(
        vec![Arc::new(Fake::new("gh", &["small"])) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );
    let out = reg.call("gh__small", serde_json::json!({})).await.unwrap();
    assert_eq!(out.text, "ok");
    assert!(!out.truncated);
}

/// A non-responding tool must hit the timeout (spec Section 5.4, 60 seconds).
/// The test injects a short ceiling so nothing actually waits.
#[sqlx::test(migrations = "../../migrations")]
async fn a_hanging_tool_times_out(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["slow"]).await;

    let mut fake = Fake::new("gh", &["slow"]);
    fake.delay = Some(std::time::Duration::from_secs(30));

    let reg = McpToolRegistry::new(
        vec![Arc::new(fake) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    )
    .with_call_timeout(std::time::Duration::from_millis(50));

    let err = reg
        .call("gh__slow", serde_json::json!({}))
        .await
        .unwrap_err();
    match err {
        ToolError::Timeout { tool, .. } => assert_eq!(tool, "gh__slow"),
        other => panic!("expected Timeout, got {other:?}"),
    }
}

/// A failure the server reported (`CallToolResult.is_error`) is propagated as-is.
///
/// Flattening it to `false` here would record `permission denied` as a success and render
/// it green in the UI — the operator misreads the investigation's result.
#[sqlx::test(migrations = "../../migrations")]
async fn a_server_reported_tool_failure_is_not_recorded_as_success(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["risky"]).await;

    let mut fake = Fake::new("gh", &["risky"]);
    fake.output = "permission denied".into();
    fake.is_error = true;

    let reg = McpToolRegistry::new(
        vec![Arc::new(fake) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    let out = reg.call("gh__risky", serde_json::json!({})).await.unwrap();
    assert!(
        out.is_error,
        "a failure the server reported was recorded as a success: {out:?}"
    );
    assert_eq!(out.text, "permission denied");
}

/// Truncated output carries a marker (spec Section 9).
///
/// The `truncated` flag alone does not tell the model — it receives exactly the ceiling's
/// worth of text and sees it as cut cleanly mid-line.
#[sqlx::test(migrations = "../../migrations")]
async fn truncated_output_carries_a_marker_the_model_can_see(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    seed(&store, "gh", &["big"]).await;

    let mut fake = Fake::new("gh", &["big"]);
    fake.output = "x".repeat(MAX_TOOL_OUTPUT_CHARS + 500);

    let reg = McpToolRegistry::new(
        vec![Arc::new(fake) as Arc<dyn McpConnection>],
        PolicyGate::new(Arc::clone(&store)),
    );

    let out = reg.call("gh__big", serde_json::json!({})).await.unwrap();
    assert!(out.truncated);
    assert!(
        out.text.ends_with(TRUNCATION_MARKER),
        "it was truncated but the text carries no marker — the model has no way to know"
    );
    assert!(
        out.text.chars().count() <= MAX_TOOL_OUTPUT_CHARS,
        "attaching the marker pushed it past the ceiling: {} chars",
        out.text.chars().count()
    );
}

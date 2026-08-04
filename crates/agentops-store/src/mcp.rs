//! Queries over `mcp_servers` and `tool_policies`.

use agentops_core::{McpServer, McpTransport, StoreError, ToolPolicy, ToolPolicyKind};
use sqlx::{PgPool, Row};

fn row_to_server(row: &sqlx::postgres::PgRow) -> Result<McpServer, StoreError> {
    let transport: String = row.try_get("transport").map_err(crate::backend)?;
    Ok(McpServer {
        id: row.try_get("id").map_err(crate::backend)?,
        name: row.try_get("name").map_err(crate::backend)?,
        transport: transport
            .parse::<McpTransport>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        config: row.try_get("config").map_err(crate::backend)?,
        enabled: row.try_get("enabled").map_err(crate::backend)?,
        created_at: row.try_get("created_at").map_err(crate::backend)?,
    })
}

fn row_to_policy(row: &sqlx::postgres::PgRow) -> Result<ToolPolicy, StoreError> {
    let policy: String = row.try_get("policy").map_err(crate::backend)?;
    Ok(ToolPolicy {
        server_name: row.try_get("server_name").map_err(crate::backend)?,
        tool_name: row.try_get("tool_name").map_err(crate::backend)?,
        policy: policy
            .parse::<ToolPolicyKind>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        mutating: row.try_get("mutating").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

/// **`ORDER BY name` is required.** `name` is UNIQUE, so it is a total order; an
/// unstable order makes the `tools[]` order unstable and the prompt cache misses every time (Section 9).
pub async fn enabled_servers(pool: &PgPool) -> Result<Vec<McpServer>, StoreError> {
    let rows = sqlx::query("SELECT * FROM mcp_servers WHERE enabled ORDER BY name")
        .fetch_all(pool)
        .await
        .map_err(crate::backend)?;
    rows.iter().map(row_to_server).collect()
}

pub async fn policies_for(pool: &PgPool, server_name: &str) -> Result<Vec<ToolPolicy>, StoreError> {
    let rows = sqlx::query("SELECT * FROM tool_policies WHERE server_name = $1 ORDER BY tool_name")
        .bind(server_name)
        .fetch_all(pool)
        .await
        .map_err(crate::backend)?;
    rows.iter().map(row_to_policy).collect()
}

/// **No row means `Deny`** (Section 9.1). Without reading absence as deny, a tool whose
/// policy registration was missed is exposed to the agent as-is.
pub async fn policy(
    pool: &PgPool,
    server_name: &str,
    tool_name: &str,
) -> Result<ToolPolicyKind, StoreError> {
    let found: Option<String> = sqlx::query_scalar(
        "SELECT policy FROM tool_policies WHERE server_name = $1 AND tool_name = $2",
    )
    .bind(server_name)
    .bind(tool_name)
    .fetch_optional(pool)
    .await
    .map_err(crate::backend)?;

    match found {
        None => Ok(ToolPolicyKind::Deny),
        Some(s) => s
            .parse::<ToolPolicyKind>()
            .map_err(|e| StoreError::Backend(e.to_string())),
    }
}

pub async fn upsert_policy(pool: &PgPool, p: &ToolPolicy) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO tool_policies (server_name, tool_name, policy, mutating)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT (server_name, tool_name) DO UPDATE
           SET policy = EXCLUDED.policy, mutating = EXCLUDED.mutating",
    )
    .bind(&p.server_name)
    .bind(&p.tool_name)
    .bind(p.policy.as_str())
    .bind(p.mutating)
    .execute(pool)
    .await
    .map_err(|e| {
        // A nonexistent server is an FK violation. Flattening it into a Backend string
        // leaves the caller unable to tell a misconfiguration from a database failure.
        if let sqlx::Error::Database(db) = &e {
            if db.is_foreign_key_violation() {
                return StoreError::NotFound;
            }
        }
        crate::backend(e)
    })?;
    Ok(())
}

//! The tool policy gate (spec Section 9.1).
//!
//! **It does not depend on the read-only annotation an MCP server reports.** That is an
//! advisory hint and the server may be an untrusted party. The judgment comes only from
//! our own explicit decision in the `tool_policies` table.
//!
//! Two requirements are distinct:
//! - `list` returns only tools with an `allow` policy
//! - `call` **re-checks immediately before execution** — the policy can change after list

use agentops_core::{Store, ToolDef, ToolError, ToolPolicyKind};
use std::collections::HashMap;
use std::sync::Arc;

/// `"gh__search"` → `("gh", "search")`.
///
/// Splits on the first `__` only — a tool name may itself contain `__`. If either the
/// server name or the tool name is empty it returns `None`, and the caller treats that as deny.
pub fn split_namespaced(name: &str) -> Option<(&str, &str)> {
    let (server, tool) = name.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

pub struct PolicyGate<S: Store> {
    store: Arc<S>,
}

impl<S: Store> PolicyGate<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    /// Keeps only `allow` tools. **Preserves the input order** — the caller may already
    /// have sorted them, and disturbing it here would make that sort meaningless (Section 9).
    ///
    /// **One batched query per server** (review M4). Calling `tool_policy` individually
    /// per tool would query the same `tool_policies` table 20 times when one server
    /// exposes 20 tools — an N+1. `Store::tool_policies_for` already existed and was
    /// used nowhere else.
    pub async fn filter_allowed(&self, defs: Vec<ToolDef>) -> Result<Vec<ToolDef>, ToolError> {
        let mut policies: HashMap<String, HashMap<String, ToolPolicyKind>> = HashMap::new();
        let mut out = Vec::with_capacity(defs.len());
        for d in defs {
            let Some((server, tool)) = split_namespaced(&d.name) else {
                continue;
            };
            if !policies.contains_key(server) {
                let rows = self
                    .store
                    .tool_policies_for(server)
                    .await
                    .map_err(|e| ToolError::Transport(e.to_string()))?;
                let map: HashMap<String, ToolPolicyKind> =
                    rows.into_iter().map(|p| (p.tool_name, p.policy)).collect();
                policies.insert(server.to_string(), map);
            }
            // **No row means deny** (Section 9.1). `policies[server]` is an empty map
            // when that server has no policy rows at all, `.get(tool)` is `None`, and that
            // does not equal `Some(&Allow)`, so it is denied automatically — moving to a
            // batched query does not flip "no row" into "let it through".
            let allowed = policies.get(server).and_then(|m| m.get(tool)).copied()
                == Some(ToolPolicyKind::Allow);
            if allowed {
                out.push(d);
            }
        }
        Ok(out)
    }

    /// The re-check immediately before execution. It does not trust the result of `list`.
    pub async fn assert_allowed(&self, name: &str) -> Result<(), ToolError> {
        if self.is_allowed(name).await? {
            Ok(())
        } else {
            Err(ToolError::Denied(name.to_string()))
        }
    }

    async fn is_allowed(&self, name: &str) -> Result<bool, ToolError> {
        // Without a namespace no server's policy can be looked up. Letting it through
        // would create a path that executes with no policy, so it is denied.
        let Some((server, tool)) = split_namespaced(name) else {
            return Ok(false);
        };
        let kind = self
            .store
            .tool_policy(server, tool)
            .await
            .map_err(|e| ToolError::Transport(e.to_string()))?;
        Ok(kind == ToolPolicyKind::Allow)
    }
}

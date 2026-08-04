//! The MCP tool registry (spec Section 9).

use crate::mcp_client::McpConnection;
use crate::policy::PolicyGate;
use agentops_core::{Store, ToolDef, ToolError, ToolOutput, ToolRegistry};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::mcp_client::McpToolDef;

/// Spec Section 9 — this ceiling is **a rough defense and guarantees nothing about the
/// context budget.** Character count is not token count. The real defense is the per-phase tool call limit (Section 5.4).
pub const MAX_TOOL_OUTPUT_CHARS: usize = 100_000;

/// The marker appended to truncated output (spec Section 9). It is the only signal that
/// lets the model tell "the output ended here" from "the output was truncated".
pub const TRUNCATION_MARKER: &str = "\n\n[truncated: tool output exceeded the limit]";

/// Spec Section 5.4 — the individual tool call timeout.
///
/// **Must equal `Limits::default().tool_call_timeout`.** The same value in two places is
/// a drift trap, so a Task 8 test asserts their agreement. Plan 3 injects the real
/// setting at assembly time with `with_call_timeout(limits.tool_call_timeout)` — until
/// then, the two defaults agreeing is enough.
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(60);

pub struct McpToolRegistry<S: Store> {
    conns: HashMap<String, Arc<dyn McpConnection>>,
    gate: PolicyGate<S>,
    call_timeout: Duration,
    /// The set of namespaced names `list()` last returned.
    ///
    /// **Spec Section 9 — the tool list is frozen for the duration of a phase.** Calling
    /// `tools/list` afresh per `call()` would let the tool set change within one phase,
    /// which is the drift Section 9 forbids. `tools[]` renders at the very front of the
    /// prompt prefix, so an unstable tool set makes the cache miss on every request (Section 8.3).
    ///
    /// **`None` and "an empty set" are different.** `None` means "`list()` has not been
    /// called even once in this phase", and only then does `call()` fill it exceptionally.
    /// If a `Some(set)` already exists and a name is not in it, that does not mean the
    /// server does not know the name — **it may mean the name appeared after this phase
    /// began**, and re-listing for that reason would break the freeze guarantee.
    known: Mutex<Option<HashSet<String>>>,
}

impl<S: Store> McpToolRegistry<S> {
    pub fn new(conns: Vec<Arc<dyn McpConnection>>, gate: PolicyGate<S>) -> Self {
        Self {
            conns: conns
                .into_iter()
                .map(|c| (c.server_name().to_string(), c))
                .collect(),
            gate,
            call_timeout: TOOL_CALL_TIMEOUT,
            known: Mutex::new(None),
        }
    }

    /// Tests inject a short ceiling. Production uses the default.
    pub fn with_call_timeout(mut self, d: Duration) -> Self {
        self.call_timeout = d;
        self
    }

    /// `list()` has not been called even once in this phase — only then does `call()`
    /// fill it exceptionally.
    fn not_yet_listed(&self) -> bool {
        self.known
            .lock()
            .expect("known set lock poisoned")
            .is_none()
    }

    fn is_known(&self, name: &str) -> bool {
        self.known
            .lock()
            .expect("known set lock poisoned")
            .as_ref()
            .is_some_and(|set| set.contains(name))
    }
}

#[async_trait]
impl<S: Store> ToolRegistry for McpToolRegistry<S> {
    /// **Returns them sorted by namespaced name.** `tools` renders at the very front of
    /// the prompt prefix, so if the server connection order or the `tools/list` response
    /// order varies between runs, the cache misses every time (spec Section 9).
    async fn list(&self) -> Result<Vec<ToolDef>, ToolError> {
        let mut defs = Vec::new();
        for (server, conn) in &self.conns {
            match conn.list_tools().await {
                Ok(tools) => defs.extend(tools.into_iter().map(|t| ToolDef {
                    name: format!("{server}__{}", t.tool_name),
                    description: t.description,
                    input_schema: t.input_schema,
                })),
                // **One dead server does not stop the rest of the tools being offered**
                // (spec Section 9). Failing the whole thing would let one server's failure block an investigation.
                Err(e) => {
                    tracing::warn!(server, error = %e, "mcp server failed tools/list");
                }
            }
        }
        // **Sorting lives only in `crate::prompt::sort_tools`.** Implementing the same
        // invariant in two places leaves nothing to guarantee they stay in sync — this is
        // a rule that prompt prefix stability depends on, so only one implementation may exist (spec Section 9).
        let defs = crate::prompt::sort_tools(defs);
        let allowed = self.gate.filter_allowed(defs).await?;

        // Replace wholesale the set `call()` will judge against for this phase.
        // It holds only the names **after** the policy filter, so a tool the server offers
        // but policy denies is absent here — `call()` filters such a tool out as `Denied`
        // at the gate first (see the ordering below).
        *self.known.lock().expect("known set lock poisoned") =
            Some(allowed.iter().map(|d| d.name.clone()).collect());

        Ok(allowed)
    }

    async fn call(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // **The policy is checked before calling the connection.** Calling and then
        // discarding the result means the side effect already happened. The order is: policy gate, existence check, connection.
        self.gate.assert_allowed(name).await?;

        let Some((server, tool)) = crate::policy::split_namespaced(name) else {
            return Err(ToolError::NotFound(name.to_string()));
        };
        let conn = self
            .conns
            .get(server)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        // **Existence is checked against the cache `list()` filled at phase start** (spec
        // Section 9 — the tool list is frozen for the duration of a phase). Calling
        // `tools/list` afresh per call would let the tool set change within one phase, and
        // since `tools[]` renders at the front of the prompt prefix that breaks the cache every time (Section 8.3).
        //
        // It fills once, exceptionally, only when `list()` has never been called — plan
        // 3's runner always calls `list()` at phase start, so this path is not taken in
        // the normal flow. Treating it as a failure would turn a healthy call that merely
        // arrived out of order into a spurious `NotFound`.
        if self.not_yet_listed() {
            self.list().await?;
        }
        if !self.is_known(name) {
            return Err(ToolError::NotFound(name.to_string()));
        }

        let fut = conn.call_tool(tool, input);
        let raw = match tokio::time::timeout(self.call_timeout, fut).await {
            Err(_) => {
                return Err(ToolError::Timeout {
                    tool: name.to_string(),
                    seconds: self.call_timeout.as_secs(),
                })
            }
            Ok(r) => r?,
        };

        // Cut on a character boundary — cutting on bytes breaks UTF-8.
        //
        // **Append the fact of truncation to the text** (spec Section 9). Setting only the
        // `truncated` flag leaves the model receiving exactly 100,000 characters with no
        // way to know it was truncated — it looks like `kubectl logs` output cut cleanly
        // mid-line. The cut is shortened by the marker's length, so the total stays under the ceiling.
        let mut truncated = false;
        let text = if raw.text.chars().count() > MAX_TOOL_OUTPUT_CHARS {
            truncated = true;
            let keep = MAX_TOOL_OUTPUT_CHARS - TRUNCATION_MARKER.chars().count();
            let mut t: String = raw.text.chars().take(keep).collect();
            t.push_str(TRUNCATION_MARKER);
            t
        } else {
            raw.text
        };

        Ok(ToolOutput {
            text,
            // **Propagate the failure the tool reported.** Fixing this at `false` would
            // record `permission denied` as a success and render it green in the UI.
            is_error: raw.is_error,
            truncated,
        })
    }
}

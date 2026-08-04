//! The MCP connection contract.
//!
//! **This trait references rmcp not at all.** The registry and the policy see only this
//! contract, and the code touching rmcp is isolated in `RmcpConnection` alone. That is
//! what keeps the registry tests alive across a change of MCP client library or API.

use agentops_core::{McpServer, McpTransport, ToolError};
use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, ContentBlock, PaginatedRequestParams};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::RoleClient;

/// One tool reported by one server. The name is the value **before** namespacing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolDef {
    pub tool_name: String,
    pub description: String,
    /// Carries MCP's JSON Schema **verbatim**. Transforming it makes the schema the model
    /// receives disagree with the one the server validates against (spec Section 9).
    pub input_schema: serde_json::Value,
}

#[async_trait]
pub trait McpConnection: Send + Sync {
    fn server_name(&self) -> &str;
    async fn list_tools(&self) -> Result<Vec<McpToolDef>, ToolError>;
    async fn call_tool(
        &self,
        tool: &str,
        input: serde_json::Value,
    ) -> Result<McpCallResult, ToolError>;
}

/// The result of one tool call.
///
/// **`is_error` is not flattened into a string.** MCP distinguishes "the call succeeded
/// but the tool reported a failure" (`CallToolResult.is_error`) from a transport failure.
/// Passing the former along as plain text records `permission denied` as a success and
/// renders it green in the UI — the operator misreads the investigation's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCallResult {
    pub text: String,
    pub is_error: bool,
}

/// Obtains, from one entry of `config.env`, the real value to pass to the child process.
///
/// **The database holds environment variable *names* only** (spec Section 9.1) — `{"env":{"GITHUB_TOKEN":"$GITHUB_TOKEN"}}`.
/// If the value starts with `$`, what follows is the variable name; otherwise the whole
/// value is the name. Either way the real secret is read from our own process environment
/// with `std::env::var`. A missing variable is raised as `ToolError::Transport` rather
/// than silently passing an empty string — that is what keeps an authentication failure
/// from appearing as a tool error with its cause hidden.
fn resolve_env_value(referenced: &str) -> Result<String, ToolError> {
    let var_name = referenced.strip_prefix('$').unwrap_or(referenced);
    std::env::var(var_name).map_err(|_| {
        ToolError::Transport(format!(
            "mcp server config references env var ${var_name} but it is not set in this process"
        ))
    })
}

/// Flattens `McpServer::config`'s `env` map into `(name, value)` pairs.
fn resolve_env_map(config: &serde_json::Value) -> Result<Vec<(String, String)>, ToolError> {
    let Some(env) = config.get("env").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };
    env.iter()
        .map(|(name, referenced)| {
            let referenced = referenced.as_str().ok_or_else(|| {
                ToolError::Transport(format!("mcp server config.env.{name} is not a string"))
            })?;
            Ok((name.clone(), resolve_env_value(referenced)?))
        })
        .collect()
}

/// The only type that touches rmcp.
///
/// The `RunningService` must be held for the connection (a child process or an HTTP
/// session) to stay alive — dropping it makes rmcp begin tearing it down.
pub struct RmcpConnection {
    server_name: String,
    running: RunningService<RoleClient, ()>,
}

impl RmcpConnection {
    /// Connects from a single `McpServer`.
    ///
    /// **Secrets are never read from the database** (spec Section 9.1). `config.env` holds
    /// **names only**, so seeing `{"env":{"GITHUB_TOKEN":"$GITHUB_TOKEN"}}` it obtains the
    /// real value with `std::env::var("GITHUB_TOKEN")` and passes it to the child process.
    /// A missing variable is raised as `ToolError::Transport` — silently passing an empty
    /// value would make an authentication failure look like a tool error and hide the cause.
    ///
    /// The rest of `config`'s shape is decided here for the first time (the plan did not fix it):
    /// - `Stdio`: `config.command` (string, required), `config.args` (string array, optional)
    /// - `Http`: `config.url` (string, required)
    pub async fn connect(server: &McpServer) -> Result<Self, ToolError> {
        let env_vars = resolve_env_map(&server.config)?;

        match server.transport {
            McpTransport::Stdio => {
                let command = server
                    .config
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::Transport(format!(
                            "mcp server '{}': stdio transport requires config.command",
                            server.name
                        ))
                    })?;
                let args: Vec<String> = server
                    .config
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();

                let cmd = tokio::process::Command::new(command).configure(|cmd| {
                    cmd.args(&args);
                    for (name, value) in &env_vars {
                        cmd.env(name, value);
                    }
                });

                let transport = TokioChildProcess::new(cmd).map_err(|e| {
                    ToolError::Transport(format!(
                        "mcp server '{}': failed to spawn child process: {e}",
                        server.name
                    ))
                })?;

                let running = ().serve(transport).await.map_err(|e| {
                    ToolError::Transport(format!(
                        "mcp server '{}': failed to initialize: {e}",
                        server.name
                    ))
                })?;

                Ok(Self {
                    server_name: server.name.clone(),
                    running,
                })
            }
            McpTransport::Http => {
                let url = server
                    .config
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::Transport(format!(
                            "mcp server '{}': http transport requires config.url",
                            server.name
                        ))
                    })?;

                let transport = StreamableHttpClientTransport::from_uri(url);

                let running = ().serve(transport).await.map_err(|e| {
                    ToolError::Transport(format!(
                        "mcp server '{}': failed to initialize: {e}",
                        server.name
                    ))
                })?;

                Ok(Self {
                    server_name: server.name.clone(),
                    running,
                })
            }
        }
    }
}

#[async_trait]
impl McpConnection for RmcpConnection {
    fn server_name(&self) -> &str {
        &self.server_name
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDef>, ToolError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .take()
                .map(|c| PaginatedRequestParams::default().with_cursor(Some(c)));
            let result = self.running.peer().list_tools(params).await.map_err(|e| {
                ToolError::Transport(format!("{}: tools/list failed: {e}", self.server_name))
            })?;

            out.extend(result.tools.into_iter().map(|t| {
                McpToolDef {
                    tool_name: t.name.to_string(),
                    description: t
                        .description
                        .as_ref()
                        .map(|d| d.to_string())
                        .unwrap_or_default(),
                    input_schema: t.schema_as_json_value(),
                }
            }));

            match result.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(out)
    }

    async fn call_tool(
        &self,
        tool: &str,
        input: serde_json::Value,
    ) -> Result<McpCallResult, ToolError> {
        let arguments = match input {
            serde_json::Value::Null => None,
            serde_json::Value::Object(map) => Some(map),
            other => {
                return Err(ToolError::Transport(format!(
                    "{}: tool call arguments must be a JSON object, got {other}",
                    self.server_name
                )));
            }
        };

        let mut params = CallToolRequestParams::new(tool.to_string());
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }

        let result = self.running.peer().call_tool(params).await.map_err(|e| {
            ToolError::Transport(format!("{}: tools/call failed: {e}", self.server_name))
        })?;

        // **Non-text blocks are not discarded silently.** Some tools return only images
        // or resources, and filtering those out leaves an empty string indistinguishable
        // from "the tool returned nothing". Even when it cannot see the content, the model
        // must know **that something arrived.**
        //
        // The separator is `\n`. Joining with an empty string would run the last word of
        // one text block into the first word of the next.
        let text = result
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text(t) => t.text.clone(),
                ContentBlock::Image(_) => "[image content omitted]".to_string(),
                ContentBlock::Audio(_) => "[audio content omitted]".to_string(),
                ContentBlock::Resource(_) => "[embedded resource omitted]".to_string(),
                ContentBlock::ResourceLink(r) => format!("[resource link: {}]", r.uri),
                // `ContentBlock` is `#[non_exhaustive]` — when rmcp adds a block kind it
                // leaves a placeholder rather than vanishing silently.
                _ => "[unsupported content block omitted]".to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");

        // **Propagate `is_error`.** rmcp does not turn it into a transport error, so
        // failing to read it here records a tool-reported failure as a success. An absent
        // field (an older protocol) is treated as success — the MCP spec's default.
        Ok(McpCallResult {
            text,
            is_error: result.is_error.unwrap_or(false),
        })
    }
}

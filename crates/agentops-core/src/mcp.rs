//! MCP server registration and tool policy (spec Sections 7 and 9.1).
//!
//! **The policy is our own explicit decision.** The read-only annotation an MCP
//! server reports about itself is an advisory hint, and the server may be an
//! untrusted party, so no judgment depends on it. Annotations are shown in the UI as reference only.

use serde::{Deserialize, Serialize};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Http,
}

impl McpTransport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown mcp transport: {0}")]
pub struct ParseTransportError(String);

impl FromStr for McpTransport {
    type Err = ParseTransportError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stdio" => Ok(Self::Stdio),
            "http" => Ok(Self::Http),
            other => Err(ParseTransportError(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub id: Uuid,
    pub name: String,
    pub transport: McpTransport,
    /// **Holds no secrets.** It holds environment variable names only; the values are
    /// read from the process environment: `{"env": {"GITHUB_TOKEN": "$GITHUB_TOKEN"}}` (spec Section 9.1).
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyKind {
    Allow,
    Deny,
}

impl ToolPolicyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown tool policy: {0}")]
pub struct ParsePolicyError(String);

impl FromStr for ToolPolicyKind {
    type Err = ParsePolicyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            other => Err(ParsePolicyError(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicy {
    pub server_name: String,
    pub tool_name: String,
    pub policy: ToolPolicyKind,
    /// Enabling a tool marked `true` raises a warning in the UI. This is a value the
    /// operator marked, not an annotation the server reported.
    pub mutating: bool,
    pub updated_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_round_trips_through_the_schema_strings() {
        for t in [McpTransport::Stdio, McpTransport::Http] {
            assert_eq!(t.as_str().parse::<McpTransport>().unwrap(), t);
        }
    }

    #[test]
    fn policy_round_trips_through_the_schema_strings() {
        for p in [ToolPolicyKind::Allow, ToolPolicyKind::Deny] {
            assert_eq!(p.as_str().parse::<ToolPolicyKind>().unwrap(), p);
        }
    }

    /// A mismatch with the schema's CHECK list only surfaces at runtime.
    #[test]
    fn schema_strings_match_the_check_constraints() {
        assert_eq!(McpTransport::Stdio.as_str(), "stdio");
        assert_eq!(McpTransport::Http.as_str(), "http");
        assert_eq!(ToolPolicyKind::Allow.as_str(), "allow");
        assert_eq!(ToolPolicyKind::Deny.as_str(), "deny");
    }
}

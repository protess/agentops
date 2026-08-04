use serde::{Deserialize, Serialize};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown chat role: {0}")]
pub struct ParseRoleError(String);

impl FromStr for ChatRole {
    type Err = ParseRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            other => Err(ParseRoleError(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: Uuid,
    pub title: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub session_id: Uuid,
    pub seq: i64,
    pub role: ChatRole,
    pub content: serde_json::Value,
    pub created_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_role_round_trips() {
        for r in [ChatRole::User, ChatRole::Assistant] {
            assert_eq!(r.as_str().parse::<ChatRole>().unwrap(), r);
        }
    }

    #[test]
    fn chat_role_rejects_system() {
        assert!("system".parse::<ChatRole>().is_err());
    }
}

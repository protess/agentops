use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// Investigation status. Maps one-to-one to the database string and is the same set as the CHECK constraint in `migrations`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl InvestigationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// A state that no longer transitions.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

impl fmt::Display for InvestigationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown investigation status: {0}")]
pub struct ParseStatusError(String);

impl FromStr for InvestigationStatus {
    type Err = ParseStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(ParseStatusError(other.to_owned())),
        }
    }
}

/// What triggered the investigation. Only `Alarm` carries a `source` — the
/// `trigger_source_iff_alarm` CHECK constraint in the database enforces this invariant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TriggeredBy {
    User,
    Alarm { source: String },
}

impl TriggeredBy {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Alarm { .. } => "alarm",
        }
    }

    pub fn source(&self) -> Option<&str> {
        match self {
            Self::User => None,
            Self::Alarm { source } => Some(source),
        }
    }
}

/// One investigation. Three distinct timestamps: queued, started, finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Investigation {
    pub id: Uuid,
    pub title: String,
    pub prompt: String,
    pub status: InvestigationStatus,
    pub triggered_by: TriggeredBy,
    pub queued_at: OffsetDateTime,
    pub started_at: Option<OffsetDateTime>,
    pub finished_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_through_db_string() {
        for s in [
            InvestigationStatus::Queued,
            InvestigationStatus::Running,
            InvestigationStatus::Completed,
            InvestigationStatus::Failed,
        ] {
            assert_eq!(s.as_str().parse::<InvestigationStatus>().unwrap(), s);
        }
    }

    #[test]
    fn status_rejects_unknown_string() {
        assert!("bogus".parse::<InvestigationStatus>().is_err());
    }

    #[test]
    fn triggered_by_user_has_no_source() {
        let t = TriggeredBy::User;
        assert_eq!(t.kind_str(), "user");
        assert_eq!(t.source(), None);
    }

    #[test]
    fn triggered_by_alarm_carries_source() {
        let t = TriggeredBy::Alarm {
            source: "cpu-high".into(),
        };
        assert_eq!(t.kind_str(), "alarm");
        assert_eq!(t.source(), Some("cpu-high"));
    }

    #[test]
    fn terminal_statuses_are_identified() {
        assert!(!InvestigationStatus::Queued.is_terminal());
        assert!(!InvestigationStatus::Running.is_terminal());
        assert!(InvestigationStatus::Completed.is_terminal());
        assert!(InvestigationStatus::Failed.is_terminal());
    }
}

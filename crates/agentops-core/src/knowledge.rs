use crate::step::Phase;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// A phase-scoped instruction. `position` fixes the prompt assembly order —
/// without the ordering the system prompt's bytes differ per request and the
/// prompt cache misses 100% of the time (design spec, Section 8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instruction {
    pub id: Uuid,
    pub phase: Phase,
    pub position: i32,
    pub title: String,
    pub body: String,
    pub enabled: bool,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Uuid,
    pub investigation_id: Option<Uuid>,
    pub title: String,
    pub body: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// An artifact not yet stored. The terminal transaction issues its ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact {
    pub title: String,
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_sort_by_position_then_title() {
        let mut v = vec![("b", 0), ("a", 1), ("a", 0)];
        v.sort_by(|l, r| l.1.cmp(&r.1).then(l.0.cmp(r.0)));
        assert_eq!(v, vec![("a", 0), ("b", 0), ("a", 1)]);
    }

    #[test]
    fn new_artifact_has_no_id_until_stored() {
        let a = NewArtifact {
            title: "t".into(),
            body: "b".into(),
        };
        assert_eq!(a.title, "t");
    }
}

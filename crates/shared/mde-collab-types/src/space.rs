//! The kind of a space and a member's role in it.

use serde::{Deserialize, Serialize};

/// What a [`SpaceId`](crate::SpaceId) *is* — the four space shapes the
/// Communications suite offers.
///
/// A `Direct` space is the 1:1 successor to a chat contact's conversation; the
/// other three are the multi-party successors to ad-hoc rooms and the auto
/// system rooms (All Fleet, per-severity) surveyed in the parity ledger.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum SpaceKind {
    /// A 1:1 conversation between two members (the Direct-message successor).
    #[default]
    Direct,
    /// A durable multi-party team space.
    Team,
    /// A time-bounded incident space (elevated urgency, alert-forward).
    Incident,
    /// A project space grouping documents, files, and discussion.
    Project,
}

/// A member's authority within a space.
///
/// Deliberately two-valued: an `Owner` may delete the space and manage
/// membership; a `Member` participates. (The append-only chat "creator-only
/// dissolve" rule maps to `Owner`-gated delete.)
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum SpaceRole {
    /// Full control: delete the space, manage membership and roles.
    Owner,
    /// A participating member.
    #[default]
    Member,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_kind_round_trips_and_names_are_stable() {
        for (kind, name) in [
            (SpaceKind::Direct, "\"direct\""),
            (SpaceKind::Team, "\"team\""),
            (SpaceKind::Incident, "\"incident\""),
            (SpaceKind::Project, "\"project\""),
        ] {
            let json = serde_json::to_string(&kind).expect("serialize");
            assert_eq!(json, name, "stable wire name");
            let back: SpaceKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn space_role_round_trips() {
        for role in [SpaceRole::Owner, SpaceRole::Member] {
            let json = serde_json::to_string(&role).expect("serialize");
            let back: SpaceRole = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, role);
        }
    }
}

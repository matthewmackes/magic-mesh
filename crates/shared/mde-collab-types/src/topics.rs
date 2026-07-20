//! Bus topic helpers for the Communications suite.
//!
//! Three lanes, each with a builder so a caller never hand-concatenates a
//! topic string:
//!
//! * **Commands** — `action/collab/<verb>` (the [`CollabCommand`] intents).
//! * **Retained read models** — `state/collab/<projection>` (latest-wins), and
//!   the per-space form `state/collab/<projection>/<space>`.
//! * **Live signed events** — `collab/event/<space>/<actor>` (the signed
//!   [`CollabEventEnvelope`] delivery lane).

use crate::command::CollabCommand;
use crate::ids::SpaceId;
use crate::ActorId;

/// Prefix for command topics.
pub const ACTION_PREFIX: &str = "action/collab/";
/// Prefix for retained read-model topics.
pub const STATE_PREFIX: &str = "state/collab/";
/// Prefix for the live signed-event lane.
pub const EVENT_PREFIX: &str = "collab/event/";

/// Canonical projection names for the `state/collab/*` retained read models.
/// Kept as `&str` consts so command builders and the worker agree on spelling.
pub mod projection {
    /// The space rail directory.
    pub const SPACE_DIRECTORY: &str = "directory";
    /// A space's Activity feed.
    pub const ACTIVITY: &str = "activity";
    /// A conversation timeline.
    pub const CONVERSATION: &str = "conversation";
    /// A thread timeline.
    pub const THREAD: &str = "thread";
    /// Live document co-edit sessions.
    pub const DOCUMENT_SESSIONS: &str = "document-sessions";
    /// A space's linked file references.
    pub const FILE_REFERENCES: &str = "file-references";
    /// The transfer jobs mirror.
    pub const TRANSFER_JOBS: &str = "transfer-jobs";
    /// The global alert inbox.
    pub const ALERT_INBOX: &str = "alert-inbox";
    /// A space's clipboard lane.
    pub const CLIPBOARD_LANE: &str = "clipboard-lane";
    /// The presence board.
    pub const PRESENCE: &str = "presence";
    /// The active call state.
    pub const CALL_STATE: &str = "call-state";
    /// The launcher/dock badge rollup.
    pub const BADGES: &str = "badges";
}

/// The command topic for a bare verb: `action/collab/<verb>`.
#[must_use]
pub fn command_topic(verb: &str) -> String {
    format!("{ACTION_PREFIX}{verb}")
}

/// The command topic for a specific [`CollabCommand`] (uses its
/// [`verb`](CollabCommand::verb)).
#[must_use]
pub fn command_topic_for(command: &CollabCommand) -> String {
    command_topic(command.verb())
}

/// A fleet-wide retained read-model topic: `state/collab/<projection>`.
#[must_use]
pub fn state_topic(projection: &str) -> String {
    format!("{STATE_PREFIX}{projection}")
}

/// A per-space retained read-model topic: `state/collab/<projection>/<space>`.
#[must_use]
pub fn space_state_topic(projection: &str, space: SpaceId) -> String {
    format!("{STATE_PREFIX}{projection}/{space}")
}

/// The live signed-event topic for a space + author:
/// `collab/event/<space>/<actor>`.
#[must_use]
pub fn event_topic(space: SpaceId, actor: &ActorId) -> String {
    format!("{EVENT_PREFIX}{space}/{actor}")
}

/// Parse a `collab/event/<space>/<actor>` topic back into its parts, or `None`
/// if the shape/space-uuid does not match. The inverse of [`event_topic`].
#[must_use]
pub fn parse_event_topic(topic: &str) -> Option<(SpaceId, ActorId)> {
    let rest = topic.strip_prefix(EVENT_PREFIX)?;
    let (space_str, actor_str) = rest.split_once('/')?;
    if actor_str.is_empty() {
        return None;
    }
    let space: SpaceId = space_str.parse().ok()?;
    Some((space, ActorId::new(actor_str)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::SpaceKind;

    #[test]
    fn command_topics_are_action_collab_scoped() {
        assert_eq!(command_topic("send_message"), "action/collab/send_message");
        let cmd = CollabCommand::CreateSpace {
            kind: SpaceKind::Team,
            name: "ops".into(),
        };
        assert_eq!(command_topic_for(&cmd), "action/collab/create_space");
    }

    #[test]
    fn state_topics_are_state_collab_scoped() {
        assert_eq!(
            state_topic(projection::ALERT_INBOX),
            "state/collab/alert-inbox"
        );
        let space = SpaceId::new();
        assert_eq!(
            space_state_topic(projection::CONVERSATION, space),
            format!("state/collab/conversation/{space}")
        );
    }

    #[test]
    fn event_topic_round_trips_through_parse() {
        let space = SpaceId::new();
        let actor = ActorId::new("eagle");
        let topic = event_topic(space, &actor);
        assert_eq!(topic, format!("collab/event/{space}/eagle"));
        let (parsed_space, parsed_actor) = parse_event_topic(&topic).expect("parse");
        assert_eq!(parsed_space, space, "space id survives the round-trip");
        assert_eq!(parsed_actor, actor);
    }

    #[test]
    fn parse_event_topic_rejects_malformed() {
        assert!(parse_event_topic("state/collab/directory").is_none());
        assert!(parse_event_topic("collab/event/not-a-uuid/eagle").is_none());
        assert!(parse_event_topic(&format!("collab/event/{}/", SpaceId::new())).is_none());
    }
}

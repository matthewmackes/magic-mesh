//! [`Conversation`] — the append-only, bounded **ring-buffer** message log
//! (lock 8), and [`Room`] — a named multi-party conversation with a membership
//! descriptor (lock 7/22/25).
//!
//! Two design points from the locks:
//!
//!   * **Bounded, evicts oldest** (lock 8): "recent-only ring buffer per
//!     conversation (bounded, no long archive)." Pushing past the cap drops the
//!     oldest message — the ring window is the whole history.
//!   * **Stable total order** (lock 22): "one canonical ordered log … sender
//!     timestamp + signature." Messages can arrive out of order (a Bus live
//!     message vs a Syncthing backfill), so insertion keeps the log sorted by
//!     `(ts_unix_ms, signature, id)` rather than by arrival — every node folds
//!     the same messages into the same order.
//!
//! Pure + headless: this is just the ordered ring; the Syncthing replication and
//! Bus fan-out that feed it are the NOTIFY-CHAT-2 worker's.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::alert::Severity;
use crate::message::Message;

/// The default per-conversation ring capacity (lock 8 — recent-only). Large
/// enough for a working scrollback, bounded so a security-alert flood can't grow
/// a node's memory without limit (design "Risks": ring-buffer sizing).
pub const DEFAULT_CAPACITY: usize = 500;

/// The total-order key for a message in the ring (lock 22): primary sender
/// timestamp, then the signature as the tiebreak, then the id for a final,
/// always-defined total order (two unsigned same-ms messages still order
/// deterministically). An unsigned message sorts before a signed one at the same
/// timestamp (empty tiebreak), which is stable across nodes.
fn order_key(m: &Message) -> (i64, &str, &str) {
    let sig = m.signature.as_ref().map_or("", |s| s.sig_hex.as_str());
    (m.ts_unix_ms, sig, m.id.as_str())
}

/// An append-only, bounded ring-buffer conversation log (lock 8), kept in a
/// stable total order (lock 22). Used for a 1:1 contact timeline and, inside a
/// [`Room`], for the shared room log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// The conversation key — a contact hostname (1:1) or a room id.
    id: String,
    /// Max retained messages; pushing past it evicts the oldest.
    cap: usize,
    /// The messages, always sorted by [`order_key`], oldest at the front.
    messages: VecDeque<Message>,
}

impl Conversation {
    /// A conversation keyed by `id` with the [`DEFAULT_CAPACITY`] ring.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self::with_capacity(id, DEFAULT_CAPACITY)
    }

    /// A conversation with an explicit ring capacity (min 1).
    #[must_use]
    pub fn with_capacity(id: impl Into<String>, cap: usize) -> Self {
        Self {
            id: id.into(),
            cap: cap.max(1),
            messages: VecDeque::new(),
        }
    }

    /// The conversation key (contact host or room id).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The ring capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.cap
    }

    /// Current message count (≤ [`capacity`](Self::capacity)).
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// The messages in canonical order (oldest first).
    #[must_use]
    pub const fn messages(&self) -> &VecDeque<Message> {
        &self.messages
    }

    /// The most recent message (for the roster's preview row), if any.
    #[must_use]
    pub fn latest(&self) -> Option<&Message> {
        self.messages.back()
    }

    /// Insert `msg` in canonical order (lock 22), evicting the oldest if the ring
    /// is full (lock 8). Idempotent by id: re-inserting a message already present
    /// (e.g. a Bus live copy then its Syncthing backfill) is a no-op, so the same
    /// message never doubles. Returns `true` when it was actually stored.
    pub fn insert(&mut self, msg: Message) -> bool {
        if self.messages.iter().any(|m| m.id == msg.id) {
            return false; // already have it — dedup the live/backfill overlap
        }
        let key = order_key(&msg);
        // Find the first element that sorts *after* msg and insert before it.
        let pos = self
            .messages
            .iter()
            .position(|m| order_key(m) > key)
            .unwrap_or(self.messages.len());
        self.messages.insert(pos, msg);
        self.evict_to_cap();
        true
    }

    /// Drop oldest messages until within the ring cap (lock 8).
    fn evict_to_cap(&mut self) {
        while self.messages.len() > self.cap {
            self.messages.pop_front();
        }
    }
}

/// How a room came to exist (NOTIFY-CHAT-5, lock 7/25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomKind {
    /// An operator-created named room — its `creator` can dissolve it.
    AdHoc,
    /// A permanent auto room (All Fleet + the per-severity alert rooms): open
    /// self-join, has no human creator, and **cannot be dissolved**.
    System,
}

/// A room's **replicated membership descriptor** (lock 7/25): the id, the display
/// name, how it came to be, who created it, and the member hostnames.
///
/// Rooms are *open* — anyone can join — and every change is an attributable
/// descriptor update; the model carries the data, the worker signs + replicates
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomDescriptor {
    /// Stable room id (the conversation key + Syncthing log name).
    pub id: String,
    /// Operator-facing room name.
    pub name: String,
    /// Ad-hoc (dissolvable by its creator) or a permanent system room.
    #[serde(default = "default_room_kind")]
    pub kind: RoomKind,
    /// The hostname of the operator who created an ad-hoc room (empty for a
    /// system room). Only this host may [`Room::dissolve`] it (lock 25).
    #[serde(default)]
    pub creator: String,
    /// Member hostnames. A `BTreeSet`-like invariant (sorted, unique) is kept by
    /// [`Room::join`]/[`Room::leave`] so the descriptor is canonical across
    /// nodes.
    pub members: Vec<String>,
}

/// Older descriptors (pre-NOTIFY-CHAT-5) carried no kind — treat them as ad-hoc.
const fn default_room_kind() -> RoomKind {
    RoomKind::AdHoc
}

/// The stable id of the **All Fleet** auto room — every node is welcome; it
/// carries fleet-wide chatter (lock 7).
pub const SYS_ALL_FLEET_ID: &str = "sys:all-fleet";

/// The stable id of the auto **per-severity alert room** for `severity`.
///
/// The firehose split into `sys:sev-critical` / `-warning` / `-info` (lock 7/16):
/// a folded alert is fanned into the matching room so an operator can watch one
/// severity band without muting the rest.
#[must_use]
pub fn severity_room_id(severity: Severity) -> String {
    format!("sys:sev-{}", severity.tag())
}

/// The full set of **auto system room descriptors** (All Fleet + one per
/// severity), open-join and undissolvable. The worker seeds these at start so
/// they always exist to self-join (lock 7).
#[must_use]
pub fn system_room_descriptors() -> Vec<RoomDescriptor> {
    let mut rooms = vec![RoomDescriptor {
        id: SYS_ALL_FLEET_ID.to_string(),
        name: "All Fleet".to_string(),
        kind: RoomKind::System,
        creator: String::new(),
        members: Vec::new(),
    }];
    for (sev, name) in [
        (Severity::Critical, "Critical Alerts"),
        (Severity::Warning, "Warnings"),
        (Severity::Info, "Info"),
    ] {
        rooms.push(RoomDescriptor {
            id: severity_room_id(sev),
            name: name.to_string(),
            kind: RoomKind::System,
            creator: String::new(),
            members: Vec::new(),
        });
    }
    rooms
}

/// A named multi-party conversation: a [`RoomDescriptor`] plus the shared,
/// canonically-ordered room ring log (lock 22 — one ordered log per room).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    /// Membership + naming.
    pub descriptor: RoomDescriptor,
    /// The shared room message ring.
    pub log: Conversation,
}

impl Room {
    /// An **ad-hoc** room `id`/`name` created by `creator`, with the
    /// [`DEFAULT_CAPACITY`] shared log. The `creator` is joined immediately and
    /// is the only host that may [`dissolve`](Self::dissolve) it (lock 25).
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, creator: impl Into<String>) -> Self {
        let id = id.into();
        let creator = creator.into();
        let mut room = Self {
            descriptor: RoomDescriptor {
                id: id.clone(),
                name: name.into(),
                kind: RoomKind::AdHoc,
                creator: creator.clone(),
                members: Vec::new(),
            },
            log: Conversation::new(id),
        };
        room.join(creator);
        room
    }

    /// A **system** room (All Fleet / a per-severity alert room) from a seed
    /// [`RoomDescriptor`]: permanent, open-join, undissolvable, no auto-member.
    #[must_use]
    pub fn from_descriptor(descriptor: RoomDescriptor) -> Self {
        let log = Conversation::new(descriptor.id.clone());
        Self { descriptor, log }
    }

    /// Whether `host` is allowed to dissolve this room: only an **ad-hoc** room's
    /// own creator (a system room is never dissolvable — lock 25).
    #[must_use]
    pub fn can_dissolve(&self, host: &str) -> bool {
        self.descriptor.kind == RoomKind::AdHoc && self.descriptor.creator == host
    }

    /// Dissolve the room on `host`'s request. Returns `true` only when `host` is
    /// permitted ([`can_dissolve`](Self::can_dissolve)) — the caller then drops
    /// the room + its log. A non-creator (or any host against a system room) gets
    /// `false` and the room is untouched.
    pub fn dissolve(&mut self, host: &str) -> bool {
        if !self.can_dissolve(host) {
            return false;
        }
        self.descriptor.members.clear();
        true
    }

    /// Add `host` to the room (open-join, lock 25). Idempotent + kept sorted, so
    /// the descriptor is byte-identical across nodes. Returns `true` if newly
    /// added.
    pub fn join(&mut self, host: impl Into<String>) -> bool {
        let host = host.into();
        match self.descriptor.members.binary_search(&host) {
            Ok(_) => false,
            Err(pos) => {
                self.descriptor.members.insert(pos, host);
                true
            }
        }
    }

    /// Remove `host` from the room. Returns `true` if it was a member.
    pub fn leave(&mut self, host: &str) -> bool {
        match self
            .descriptor
            .members
            .binary_search_by(|m| m.as_str().cmp(host))
        {
            Ok(pos) => {
                self.descriptor.members.remove(pos);
                true
            }
            Err(_) => false,
        }
    }

    /// Whether `host` is currently a member.
    #[must_use]
    pub fn is_member(&self, host: &str) -> bool {
        self.descriptor
            .members
            .binary_search_by(|m| m.as_str().cmp(host))
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use crate::message::{Message, MessageId, MessageKind};

    use super::*;

    fn at(id: &str, ts: i64) -> Message {
        Message {
            id: MessageId::new(id),
            sender: "eagle".into(),
            ts_unix_ms: ts,
            kind: MessageKind::Text(format!("m{ts}")),
            signature: None,
        }
    }

    #[test]
    fn ring_evicts_the_oldest_past_capacity() {
        let mut c = Conversation::with_capacity("nyc3", 3);
        for i in 0..5 {
            c.insert(at(&format!("id{i}"), i));
        }
        assert_eq!(c.len(), 3, "bounded to the cap");
        // Oldest two (ts 0,1) evicted; 2,3,4 remain in order.
        let ts: Vec<i64> = c.messages().iter().map(|m| m.ts_unix_ms).collect();
        assert_eq!(ts, vec![2, 3, 4]);
        assert_eq!(c.latest().unwrap().ts_unix_ms, 4);
    }

    #[test]
    fn out_of_order_inserts_land_in_canonical_order() {
        let mut c = Conversation::new("nyc3");
        // Arrive scrambled (a backfill after a live message).
        c.insert(at("b", 20));
        c.insert(at("a", 10));
        c.insert(at("c", 30));
        c.insert(at("mid", 15));
        let ts: Vec<i64> = c.messages().iter().map(|m| m.ts_unix_ms).collect();
        assert_eq!(
            ts,
            vec![10, 15, 20, 30],
            "sorted by timestamp regardless of arrival"
        );
    }

    #[test]
    fn insert_is_idempotent_by_id() {
        let mut c = Conversation::new("nyc3");
        assert!(c.insert(at("same", 10)));
        assert!(
            !c.insert(at("same", 10)),
            "same id is a no-op (live/backfill overlap)"
        );
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn signature_breaks_ties_at_equal_timestamps() {
        let mut c = Conversation::new("r");
        let mut lo = at("x", 10);
        lo.signature = Some(crate::message::Signature {
            pubkey_hex: "aa".into(),
            sig_hex: "01".into(),
        });
        let mut hi = at("y", 10);
        hi.signature = Some(crate::message::Signature {
            pubkey_hex: "bb".into(),
            sig_hex: "02".into(),
        });
        // Insert the higher-sig one first; ordering must still put "01" before "02".
        c.insert(hi);
        c.insert(lo);
        let sigs: Vec<&str> = c
            .messages()
            .iter()
            .map(|m| m.signature.as_ref().unwrap().sig_hex.as_str())
            .collect();
        assert_eq!(sigs, vec!["01", "02"], "signature tiebreak at equal ts");
    }

    #[test]
    fn room_open_join_and_leave_keeps_a_canonical_member_set() {
        let mut room = Room::new("all-fleet", "All Fleet", "eagle");
        assert!(room.is_member("eagle"), "creator is joined");
        assert!(room.join("nyc3"));
        assert!(room.join("fra1"));
        assert!(!room.join("nyc3"), "join is idempotent");
        // Sorted + unique regardless of join order.
        assert_eq!(room.descriptor.members, vec!["eagle", "fra1", "nyc3"]);
        assert!(room.leave("fra1"));
        assert!(!room.leave("ghost"), "leaving a non-member is a no-op");
        assert_eq!(room.descriptor.members, vec!["eagle", "nyc3"]);
    }

    #[test]
    fn room_shares_one_ordered_log() {
        let mut room = Room::new("ops", "Ops", "eagle");
        room.log.insert(at("b", 20));
        room.log.insert(at("a", 10));
        let ts: Vec<i64> = room.log.messages().iter().map(|m| m.ts_unix_ms).collect();
        assert_eq!(ts, vec![10, 20]);
    }

    #[test]
    fn only_the_creator_can_dissolve_an_adhoc_room() {
        let mut room = Room::new("ops", "Ops", "eagle");
        room.join("nyc3");
        assert!(!room.can_dissolve("nyc3"), "a joiner cannot dissolve");
        assert!(!room.dissolve("nyc3"), "a non-creator dissolve is a no-op");
        assert!(
            room.is_member("nyc3"),
            "the room survives a rejected dissolve"
        );
        // The creator can.
        assert!(room.can_dissolve("eagle"));
        assert!(room.dissolve("eagle"));
        assert!(
            room.descriptor.members.is_empty(),
            "dissolve clears members"
        );
    }

    #[test]
    fn system_rooms_are_open_join_and_never_dissolvable() {
        let descriptors = system_room_descriptors();
        // All Fleet + one room per severity level.
        assert_eq!(descriptors.len(), 4);
        assert_eq!(descriptors[0].id, SYS_ALL_FLEET_ID);
        assert!(descriptors
            .iter()
            .any(|d| d.id == severity_room_id(Severity::Critical)));
        let mut all_fleet = Room::from_descriptor(descriptors[0].clone());
        assert_eq!(all_fleet.descriptor.kind, RoomKind::System);
        // Anyone self-joins; nobody — not even a would-be creator — dissolves it.
        assert!(all_fleet.join("eagle"));
        assert!(all_fleet.join("nyc3"));
        assert!(!all_fleet.can_dissolve("eagle"));
        assert!(!all_fleet.dissolve("eagle"));
        assert!(all_fleet.is_member("eagle"), "a system room is permanent");
    }

    #[test]
    fn severity_room_ids_are_stable_and_distinct() {
        assert_eq!(severity_room_id(Severity::Critical), "sys:sev-critical");
        assert_eq!(severity_room_id(Severity::Warning), "sys:sev-warning");
        assert_eq!(severity_room_id(Severity::Info), "sys:sev-info");
    }

    #[test]
    fn descriptor_round_trips_and_defaults_legacy_kind_to_adhoc() {
        let room = Room::new("ops", "Ops", "eagle");
        let json = serde_json::to_string(&room.descriptor).expect("serialize");
        let back: RoomDescriptor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(room.descriptor, back);
        // A pre-NOTIFY-CHAT-5 descriptor (no kind/creator) hydrates as ad-hoc.
        let legacy = r#"{"id":"x","name":"X","members":["eagle"]}"#;
        let d: RoomDescriptor = serde_json::from_str(legacy).expect("legacy descriptor");
        assert_eq!(d.kind, RoomKind::AdHoc);
        assert!(d.creator.is_empty());
    }
}

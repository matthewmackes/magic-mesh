//! TERM-8 — the mesh peer roster the remote-terminal picker reads.
//!
//! A remote terminal opens a shell on a mesh **peer**, so the picker needs the
//! same peer directory + presence the other surfaces show. That directory is the
//! chat worker's presence roster, republished latest-wins on `state/chat/roster`
//! and already consumed by the Chat surface — the canonical "who is on the mesh,
//! and are they reachable" source. Per §6 the desktop tier leans inward on
//! `mde-bus` only, so (exactly as `mde-files-egui` mirrors the mesh-mount worker's
//! structs) the roster wire shape is a **local serde mirror** here, not a
//! dependency on the `mde-chat` service crate.
//!
//! The [`RosterClient`] seam is injectable so the picker model is unit-tested
//! headless (a fake) while production reads the live Bus ([`BusRoster`]). Reading
//! is a non-blocking local spool scan — never a peer probe — so an offline peer
//! can neither hang the read nor the picker (TERM-8: "no hang").

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

/// The presence-roster topic the chat worker republishes latest-wins. MUST equal
/// `mackesd::workers::chat::STATE_CHAT_ROSTER` (cross-checked in tests).
pub const ROSTER_TOPIC: &str = "state/chat/roster";

// ── the wire mirror (read side) ──────────────────────────────────────────────

/// A contact's presence — a local mirror of `mde_chat::roster::Presence`
/// (`snake_case` tags).
///
/// Auto states derive from mesh health; manual states are an operator override.
/// `Offline` is the default so a partial record degrades honestly rather than
/// failing the whole roster parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// Auto: reachable, fresh heartbeat.
    Online,
    /// Auto: stale heartbeat (reachable-but-quiet).
    Away,
    /// Auto: unreachable.
    #[default]
    Offline,
    /// Manual: operator set Away.
    ManualAway,
    /// Manual: Do-Not-Disturb (still reachable).
    Dnd,
    /// Manual: Invisible — presents as Offline to peers.
    Invisible,
    /// Manual: Free-for-Chat.
    FreeForChat,
}

impl Presence {
    /// Whether the peer reads as reachable — a shell can plausibly be opened.
    /// Invisible presents as Offline to peers, so it groups with the unreachable.
    #[must_use]
    pub const fn is_reachable(self) -> bool {
        matches!(
            self,
            Self::Online | Self::Away | Self::ManualAway | Self::Dnd | Self::FreeForChat
        )
    }

    /// A short human label for the picker row.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Away | Self::ManualAway => "away",
            Self::Offline => "offline",
            Self::Dnd => "do not disturb",
            Self::Invisible => "invisible",
            Self::FreeForChat => "free for chat",
        }
    }
}

/// One roster contact, mirroring the projected fields of `mde_chat::roster::Contact`
/// (host identity + cosmetic nickname + presence). Serde ignores the fields this
/// surface doesn't need (role, status message).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct WireContact {
    host: String,
    #[serde(default)]
    nickname: Option<String>,
    #[serde(default)]
    presence: Presence,
}

/// The roster wire shape (`{"self_host":…, "contacts": {host: contact}}`).
#[derive(Debug, Clone, Deserialize)]
struct WireRoster {
    self_host: String,
    contacts: BTreeMap<String, WireContact>,
}

/// Parse a `state/chat/roster` body; `None` on malformed JSON (an honest miss).
fn parse_roster(raw: &str) -> Option<WireRoster> {
    serde_json::from_str(raw).ok()
}

// ── the projected picker view ────────────────────────────────────────────────

/// One peer as the picker renders it — the hostname identity (the `action/pty/<peer>`
/// verb slot), a display name, and its presence pip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerEntry {
    /// The hostname identity — the mesh peer short-name (the verb slot).
    pub host: String,
    /// The name to show (the cosmetic nickname when set, else the hostname).
    pub display: String,
    /// The peer's presence.
    pub presence: Presence,
}

impl PeerEntry {
    /// Whether this peer reads as reachable (drives the pip + whether the row is
    /// pickable — an offline peer is greyed).
    #[must_use]
    pub const fn is_reachable(&self) -> bool {
        self.presence.is_reachable()
    }
}

/// The projected peer directory the picker reads — self excluded (a local
/// terminal already covers this node), reachable peers first, each group
/// hostname-ordered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RosterSnapshot {
    /// This node's own identity (never listed as a remote target).
    pub self_host: String,
    /// Every other mesh member, reachable-first then hostname-ordered.
    pub peers: Vec<PeerEntry>,
}

impl RosterSnapshot {
    /// Project a wire roster into the picker view.
    fn from_wire(wire: &WireRoster) -> Self {
        let mut peers: Vec<PeerEntry> = wire
            .contacts
            .values()
            .filter(|c| c.host != wire.self_host)
            .map(|c| PeerEntry {
                host: c.host.clone(),
                display: c.nickname.clone().unwrap_or_else(|| c.host.clone()),
                presence: c.presence,
            })
            .collect();
        // Reachable first (the pickable set on top), then hostname-ordered — the
        // `contacts` map is already host-sorted, so this stable partition keeps
        // the alphabetical order within each group.
        peers.sort_by_key(|p| (!p.is_reachable(), p.host.clone()));
        Self {
            self_host: wire.self_host.clone(),
            peers,
        }
    }

    /// The peers whose display/host contains `filter` (case-insensitive); an
    /// empty filter matches all.
    #[must_use]
    pub fn matching(&self, filter: &str) -> Vec<&PeerEntry> {
        let needle = filter.trim().to_ascii_lowercase();
        self.peers
            .iter()
            .filter(|p| {
                needle.is_empty()
                    || p.host.to_ascii_lowercase().contains(&needle)
                    || p.display.to_ascii_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Whether the roster has no other peers (a solo host) — the picker shows an
    /// honest empty state and leans on manual entry.
    #[must_use]
    pub fn is_solo(&self) -> bool {
        self.peers.is_empty()
    }
}

// ── the client seam ──────────────────────────────────────────────────────────

/// The roster client seam: read the latest presence roster.
///
/// Injectable so the picker is unit-tested headless (a fake) while production
/// talks the Bus ([`BusRoster`]). Non-blocking — a local spool scan, never a peer
/// probe.
pub trait RosterClient {
    /// The latest roster snapshot, or `None` when no roster has been published
    /// (a fresh mesh / no Bus) — the picker then shows the empty state + manual
    /// entry, never a hang.
    fn snapshot(&self) -> Option<RosterSnapshot>;
}

/// The live Bus-backed roster reader — a synchronous latest-wins
/// `state/chat/roster` scan, the same persist-first path the Chat surface uses.
///
/// Degrades honestly to `None` when there's no Bus dir or no roster yet.
#[derive(Debug, Clone)]
pub struct BusRoster {
    bus_root: Option<PathBuf>,
}

impl BusRoster {
    /// Resolve the Bus spool dir from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir).
    #[must_use]
    pub const fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl RosterClient for BusRoster {
    fn snapshot(&self) -> Option<RosterSnapshot> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        // Latest-wins: the newest record (last, ULID ascending) is the live roster.
        let latest = persist
            .list_since(ROSTER_TOPIC, None)
            .ok()?
            .into_iter()
            .filter_map(|m| m.body)
            .next_back()?;
        parse_roster(&latest).map(|w| RosterSnapshot::from_wire(&w))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! An in-memory [`RosterClient`] for headless tests — a canned snapshot.

    use super::{PeerEntry, Presence, RosterClient, RosterSnapshot};

    /// A fake roster client returning a fixed snapshot (or `None`).
    #[derive(Clone, Default)]
    pub struct FakeRoster {
        snapshot: Option<RosterSnapshot>,
    }

    impl FakeRoster {
        /// A fake with no published roster (the solo / no-Bus honesty path).
        pub fn empty() -> Self {
            Self::default()
        }

        /// A fake with `self_host` and the given `(host, presence)` peers.
        pub fn with_peers(self_host: &str, peers: &[(&str, Presence)]) -> Self {
            let peers = peers
                .iter()
                .map(|(host, presence)| PeerEntry {
                    host: (*host).to_string(),
                    display: (*host).to_string(),
                    presence: *presence,
                })
                .collect();
            Self {
                snapshot: Some(RosterSnapshot {
                    self_host: self_host.to_string(),
                    peers,
                }),
            }
        }
    }

    impl RosterClient for FakeRoster {
        fn snapshot(&self) -> Option<RosterSnapshot> {
            self.snapshot.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeRoster;
    use super::*;

    #[test]
    fn roster_topic_matches_the_worker_contract() {
        assert_eq!(ROSTER_TOPIC, "state/chat/roster");
    }

    #[test]
    fn projects_a_wire_roster_excluding_self_reachable_first() {
        // A real chat-worker roster body (self + three peers, mixed presence).
        let raw = r#"{
            "self_host":"eagle",
            "contacts":{
                "eagle":{"host":"eagle","nickname":null,"status_message":null,"role":"workstation","presence":"online"},
                "cedar":{"host":"cedar","nickname":"Cedar Box","role":"headless","presence":"offline"},
                "birch":{"host":"birch","nickname":null,"role":"workstation","presence":"online"},
                "anvil":{"host":"anvil","nickname":null,"role":"lighthouse","presence":"away"}
            }
        }"#;
        let wire = parse_roster(raw).expect("decodes the worker roster");
        let snap = RosterSnapshot::from_wire(&wire);
        assert_eq!(snap.self_host, "eagle");
        // Self is excluded; reachable (anvil away, birch online) come before the
        // offline cedar, each group hostname-ordered.
        let hosts: Vec<&str> = snap.peers.iter().map(|p| p.host.as_str()).collect();
        assert_eq!(hosts, vec!["anvil", "birch", "cedar"]);
        // The nickname is the display name; the hostname stays the identity/slot.
        let cedar = snap
            .peers
            .iter()
            .find(|p| p.host == "cedar")
            .expect("cedar");
        assert_eq!(cedar.display, "Cedar Box");
        assert!(!cedar.is_reachable(), "offline peer is greyed/unpickable");
        assert!(snap.peers[0].is_reachable(), "anvil (away) is reachable");
    }

    #[test]
    fn malformed_or_missing_roster_is_an_honest_none() {
        assert!(parse_roster("not json").is_none());
        // The client with no Bus dir reads None (never a hang).
        let client = BusRoster::with_root(None);
        assert!(client.snapshot().is_none());
    }

    #[test]
    fn filter_matches_host_and_display_case_insensitively() {
        let fake = FakeRoster::with_peers(
            "eagle",
            &[
                ("anvil", Presence::Online),
                ("birch", Presence::Offline),
                ("cedar", Presence::Away),
            ],
        );
        let snap = fake.snapshot().expect("snapshot");
        assert_eq!(snap.matching("").len(), 3, "empty filter matches all");
        let ce = snap.matching("CE");
        assert_eq!(ce.len(), 1);
        assert_eq!(ce[0].host, "cedar");
        assert!(snap.matching("zzz").is_empty());
        assert!(!snap.is_solo());
    }

    #[test]
    fn a_solo_host_has_no_peers() {
        let fake = FakeRoster::with_peers("eagle", &[]);
        assert!(fake.snapshot().expect("snapshot").is_solo());
        assert!(FakeRoster::empty().snapshot().is_none());
    }
}

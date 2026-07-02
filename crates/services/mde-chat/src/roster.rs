//! [`Roster`] / [`Contact`] / [`Presence`] — the peer directory *is* the roster
//! (lock 6): one contact per enrolled mesh member (plus self + VM guests), keyed
//! by its unforgeable hostname identity (lock 21).
//!
//!   * A [`Contact`] is a mesh member: the `host` (identity, what signing binds
//!     to) plus an optional **cosmetic** `nickname` and a free-text
//!     `status_message` — both gossiped, neither load-bearing (lock 21) — plus a
//!     [`NodeRole`] badge (lock 6) and current [`Presence`].
//!   * [`Presence`] is **auto** (Online / Away / Offline, derived from mesh
//!     health) ∪ **manual** (Away / DND / Invisible / Free-for-Chat, an operator
//!     override gossiped to peers — lock 5). DND suppresses sound + toast.
//!
//! Pure model: reachability and gossip are the worker's; here presence is just a
//! value the worker sets and the UI reads.

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

/// A node's role badge in the roster (lock 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    /// A lighthouse / control node.
    Lighthouse,
    /// A workstation (has a desktop seat).
    Workstation,
    /// A headless node (emits + relays, no UI).
    Headless,
    /// A VM guest that is its own mesh peer.
    VmGuest,
}

impl NodeRole {
    /// A short stable tag for the badge.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Lighthouse => "lighthouse",
            Self::Workstation => "workstation",
            Self::Headless => "headless",
            Self::VmGuest => "vm",
        }
    }
}

/// A contact's presence — auto from mesh health ∪ manual override (lock 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// Auto: reachable, fresh heartbeat.
    Online,
    /// Auto: stale heartbeat (reachable-but-quiet).
    Away,
    /// Auto: unreachable.
    Offline,
    /// Manual: operator set Away on their own node.
    ManualAway,
    /// Manual: Do-Not-Disturb — suppresses sound + toast (lock 5/12/13).
    Dnd,
    /// Manual: Invisible — appears Offline to peers but is reachable.
    Invisible,
    /// Manual: Free-for-Chat.
    FreeForChat,
}

impl Presence {
    /// Whether this is an operator-set manual override (vs auto from mesh
    /// health). The worker keeps a manual state until cleared; auto states are
    /// recomputed from reachability each snapshot.
    #[must_use]
    pub const fn is_manual(self) -> bool {
        matches!(
            self,
            Self::ManualAway | Self::Dnd | Self::Invisible | Self::FreeForChat
        )
    }

    /// Whether the contact reads as reachable in the roster's Online group.
    /// Invisible deliberately reads as *not* available (it presents as Offline to
    /// peers), so it is grouped with Offline.
    #[must_use]
    pub const fn is_available(self) -> bool {
        matches!(
            self,
            Self::Online | Self::Away | Self::ManualAway | Self::Dnd | Self::FreeForChat
        )
    }

    /// Whether sound + toast are suppressed for this presence (lock 5/12/13):
    /// only Do-Not-Disturb silences alerts.
    #[must_use]
    pub const fn suppresses_alerts(self) -> bool {
        matches!(self, Self::Dnd)
    }

    /// The ICQ status label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Online => "Online",
            Self::Away => "Away",
            Self::Offline => "Offline",
            Self::ManualAway => "Away (manual)",
            Self::Dnd => "Do Not Disturb",
            Self::Invisible => "Invisible",
            Self::FreeForChat => "Free for Chat",
        }
    }
}

/// One roster contact — a mesh member (lock 6/21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// The **unforgeable identity**: hostname = username, what signing binds to
    /// (lock 2/21). Never renamed cosmetically — that's `nickname`.
    pub host: String,
    /// An optional cosmetic nickname (gossiped, not load-bearing, lock 21).
    pub nickname: Option<String>,
    /// An optional ICQ-style free-text status message beside the name (lock 21).
    pub status_message: Option<String>,
    /// Role badge (lock 6).
    pub role: NodeRole,
    /// Current presence.
    pub presence: Presence,
}

impl Contact {
    /// A contact for mesh member `host` with `role`, presence Offline, no
    /// cosmetics — the state before the first presence snapshot / gossip.
    #[must_use]
    pub fn new(host: impl Into<String>, role: NodeRole) -> Self {
        Self {
            host: host.into(),
            nickname: None,
            status_message: None,
            role,
            presence: Presence::Offline,
        }
    }

    /// Set the cosmetic nickname (builder style).
    #[must_use]
    pub fn with_nickname(mut self, nickname: impl Into<String>) -> Self {
        self.nickname = Some(nickname.into());
        self
    }

    /// Set the free-text status message (builder style).
    #[must_use]
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status_message = Some(status.into());
        self
    }

    /// Set presence (builder style).
    #[must_use]
    pub const fn with_presence(mut self, presence: Presence) -> Self {
        self.presence = presence;
        self
    }

    /// The name to show: the cosmetic nickname when set, else the hostname
    /// identity. The hostname always remains the real key.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.nickname.as_deref().unwrap_or(&self.host)
    }
}

/// The roster: the peer directory keyed by hostname (lock 6), with one contact
/// flagged as **self** (the local host, which carries local alerts/clips —
/// lock 17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    /// The local host's identity — its contact is the pinned self-contact.
    self_host: String,
    /// host → contact.
    contacts: BTreeMap<String, Contact>,
}

impl Roster {
    /// A roster for the local host `self_host`, seeded with a self-contact
    /// (Workstation, Online). The worker replaces the role/presence from real
    /// host policy + mesh health.
    #[must_use]
    pub fn new(self_host: impl Into<String>) -> Self {
        let self_host = self_host.into();
        let mut contacts = BTreeMap::new();
        contacts.insert(
            self_host.clone(),
            Contact::new(self_host.clone(), NodeRole::Workstation).with_presence(Presence::Online),
        );
        Self {
            self_host,
            contacts,
        }
    }

    /// The local host's identity.
    #[must_use]
    pub fn self_host(&self) -> &str {
        &self.self_host
    }

    /// The pinned self-contact (lock 17).
    ///
    /// # Panics
    /// Never in practice: the self-contact is inserted in [`Roster::new`] and no
    /// API removes it, so the lookup is total.
    #[must_use]
    pub fn self_contact(&self) -> &Contact {
        // The self-contact is inserted at construction and never removed.
        self.contacts
            .get(&self.self_host)
            .expect("self-contact is always present")
    }

    /// Whether `host` is the local self.
    #[must_use]
    pub fn is_self(&self, host: &str) -> bool {
        host == self.self_host
    }

    /// Insert or replace a contact (the worker upserts from enrollment + gossip).
    pub fn upsert(&mut self, contact: Contact) {
        self.contacts.insert(contact.host.clone(), contact);
    }

    /// Look up a contact by host.
    #[must_use]
    pub fn get(&self, host: &str) -> Option<&Contact> {
        self.contacts.get(host)
    }

    /// Set a contact's presence, if it exists. Returns `true` when applied.
    pub fn set_presence(&mut self, host: &str, presence: Presence) -> bool {
        if let Some(c) = self.contacts.get_mut(host) {
            c.presence = presence;
            true
        } else {
            false
        }
    }

    /// Number of contacts (including self).
    #[must_use]
    pub fn len(&self) -> usize {
        self.contacts.len()
    }

    /// Whether the roster is empty (never true — self is always present).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.contacts.is_empty()
    }

    /// All contacts, hostname-ordered.
    pub fn contacts(&self) -> impl Iterator<Item = &Contact> {
        self.contacts.values()
    }

    /// The ICQ **Online** group: available contacts, hostname-ordered (lock 4).
    #[must_use]
    pub fn online(&self) -> Vec<&Contact> {
        self.contacts
            .values()
            .filter(|c| c.presence.is_available())
            .collect()
    }

    /// The ICQ **Offline** group: unavailable contacts (Offline + Invisible),
    /// hostname-ordered (lock 4).
    #[must_use]
    pub fn offline(&self) -> Vec<&Contact> {
        self.contacts
            .values()
            .filter(|c| !c.presence.is_available())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_prefers_nickname_but_host_stays_the_key() {
        let c = Contact::new("nyc3", NodeRole::Lighthouse).with_nickname("New York");
        assert_eq!(c.display_name(), "New York");
        assert_eq!(c.host, "nyc3", "identity is unchanged by the cosmetic name");
        // No nickname → the hostname is the display name.
        let plain = Contact::new("fra1", NodeRole::Headless);
        assert_eq!(plain.display_name(), "fra1");
    }

    #[test]
    fn presence_manual_available_and_dnd_semantics() {
        assert!(Presence::Dnd.is_manual());
        assert!(!Presence::Online.is_manual());
        // Only DND suppresses sound + toast.
        assert!(Presence::Dnd.suppresses_alerts());
        assert!(!Presence::Away.suppresses_alerts());
        // Invisible presents as unavailable (groups with Offline).
        assert!(!Presence::Invisible.is_available());
        assert!(Presence::Dnd.is_available(), "DND is reachable, just quiet");
    }

    #[test]
    fn roster_seeds_a_pinned_self_contact() {
        let r = Roster::new("eagle");
        assert!(r.is_self("eagle"));
        assert_eq!(r.self_contact().host, "eagle");
        assert_eq!(r.self_contact().presence, Presence::Online);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn online_and_offline_groups_partition_by_availability() {
        let mut r = Roster::new("eagle"); // self Online
        r.upsert(Contact::new("nyc3", NodeRole::Lighthouse).with_presence(Presence::Online));
        r.upsert(Contact::new("fra1", NodeRole::Headless).with_presence(Presence::Offline));
        r.upsert(Contact::new("ghost", NodeRole::Workstation).with_presence(Presence::Invisible));

        let online: Vec<&str> = r.online().iter().map(|c| c.host.as_str()).collect();
        let offline: Vec<&str> = r.offline().iter().map(|c| c.host.as_str()).collect();
        assert_eq!(online, vec!["eagle", "nyc3"]);
        assert_eq!(offline, vec!["fra1", "ghost"], "Offline + Invisible");
    }

    #[test]
    fn set_presence_moves_a_contact_between_groups() {
        let mut r = Roster::new("eagle");
        r.upsert(Contact::new("nyc3", NodeRole::Lighthouse)); // defaults Offline
        assert_eq!(r.offline().len(), 1);
        assert!(r.set_presence("nyc3", Presence::Online));
        assert_eq!(r.offline().len(), 0);
        assert_eq!(r.online().iter().filter(|c| c.host == "nyc3").count(), 1);
        assert!(
            !r.set_presence("nobody", Presence::Online),
            "unknown host → false"
        );
    }

    #[test]
    fn roster_round_trips_through_serde() {
        let mut r = Roster::new("eagle");
        r.upsert(
            Contact::new("nyc3", NodeRole::Lighthouse)
                .with_nickname("NYC")
                .with_status("deploying")
                .with_presence(Presence::Dnd),
        );
        let json = serde_json::to_string(&r).expect("serialize");
        let back: Roster = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, back);
    }
}

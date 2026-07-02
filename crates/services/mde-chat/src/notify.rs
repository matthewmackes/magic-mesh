//! [`NotifyPrefs`] — the pure **notification policy** (NOTIFY-CHAT-5): per-contact
//! and per-room **mute** plus a **global per-severity threshold**, and the pure
//! decision of whether an incoming event should *ring* (raise a KIRON chyron +
//! its sound) or stay silent.
//!
//! The one invariant the worker leans on (design lock 16, taming the machine-
//! alert firehose): a muted / below-threshold / DND-suppressed event is **silent
//! but STILL logged** — this module only answers "does it ring?"; the caller
//! always inserts the message into its conversation ring regardless. So a muted
//! contact never surfaces a toast yet its full history is intact when the
//! operator opens the conversation.
//!
//! DND breakthrough mirrors KIRON lock 10: Do-Not-Disturb hushes an Info/Warning
//! event, but a **Critical alert always breaks through** (safety over quiet).
//! Mute + threshold have no Critical exception — an explicitly muted source is
//! silenced at every severity (the operator asked for it), matching the ICQ
//! per-contact/room mute.
//!
//! Pure + serde-round-trippable: the worker persists it per node and reloads it
//! on start, the same headless pattern as the presence gossip. Zero I/O here.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::alert::Severity;

/// The default global alert threshold: **Warning** — Info alerts fold to the
/// durable ring-log but never ring (the firehose default), while Warning+ do.
/// This matches the worker's pre-mute behaviour so an operator who never touches
/// the setting sees no change.
const fn default_threshold() -> Severity {
    Severity::Warning
}

/// The operator's notification policy for **this seat** (NOTIFY-CHAT-5, lock 16).
///
/// Three independent axes, all silence-only (they never *add* a toast, only
/// withhold one — the message is logged either way):
///   * `muted_contacts` — hostnames whose messages + alerts never ring.
///   * `muted_rooms` — room ids whose messages never ring.
///   * `threshold` — the least-severe alert that still rings; anything less
///     severe (a higher [`Severity`] value) is silent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyPrefs {
    /// Muted contact hostnames (sorted, unique — canonical across nodes).
    #[serde(default)]
    muted_contacts: BTreeSet<String>,
    /// Muted room ids.
    #[serde(default)]
    muted_rooms: BTreeSet<String>,
    /// The global per-severity gate for **alerts**: an alert rings only if it is
    /// at least this severe. Human chat messages are not gated by it.
    #[serde(default = "default_threshold")]
    threshold: Severity,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        Self {
            muted_contacts: BTreeSet::new(),
            muted_rooms: BTreeSet::new(),
            threshold: default_threshold(),
        }
    }
}

impl NotifyPrefs {
    /// A fresh policy: nothing muted, the [`default_threshold`] (Warning).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mute a contact by hostname. Returns `true` if it was newly muted.
    pub fn mute_contact(&mut self, host: impl Into<String>) -> bool {
        self.muted_contacts.insert(host.into())
    }

    /// Unmute a contact. Returns `true` if it had been muted.
    pub fn unmute_contact(&mut self, host: &str) -> bool {
        self.muted_contacts.remove(host)
    }

    /// Whether `host` is muted.
    #[must_use]
    pub fn is_contact_muted(&self, host: &str) -> bool {
        self.muted_contacts.contains(host)
    }

    /// Mute a room by id. Returns `true` if newly muted.
    pub fn mute_room(&mut self, room_id: impl Into<String>) -> bool {
        self.muted_rooms.insert(room_id.into())
    }

    /// Unmute a room. Returns `true` if it had been muted.
    pub fn unmute_room(&mut self, room_id: &str) -> bool {
        self.muted_rooms.remove(room_id)
    }

    /// Whether room `room_id` is muted.
    #[must_use]
    pub fn is_room_muted(&self, room_id: &str) -> bool {
        self.muted_rooms.contains(room_id)
    }

    /// The current global alert threshold.
    #[must_use]
    pub const fn threshold(&self) -> Severity {
        self.threshold
    }

    /// Set the global alert threshold (Info = ring everything, Critical = only
    /// the most severe).
    pub const fn set_threshold(&mut self, threshold: Severity) {
        self.threshold = threshold;
    }

    /// Whether `severity` clears the global threshold (is at least as severe).
    /// [`Severity`] is ordered most-severe-first, so "at least as severe" is
    /// `severity <= threshold`.
    #[must_use]
    const fn passes_threshold(&self, severity: Severity) -> bool {
        // Ord on Severity: Critical < Warning < Info.
        (severity as u8) <= (self.threshold as u8)
    }

    /// Should a **folded alert** from `origin_host` at `severity` ring (raise a
    /// chyron + sound)? It rings unless the origin is muted, the severity is
    /// below the global threshold, or DND is active — except a **Critical always
    /// breaks through DND** (lock 10). Either way the caller still logs it.
    #[must_use]
    pub fn should_ring_alert(&self, origin_host: &str, severity: Severity, dnd: bool) -> bool {
        if self.is_contact_muted(origin_host) {
            return false;
        }
        if !self.passes_threshold(severity) {
            return false;
        }
        // DND hushes non-critical; Critical is the safety breakthrough.
        !(dnd && severity != Severity::Critical)
    }

    /// Should a **human chat message** from `sender` (optionally in room
    /// `room_id`) ring? A message is Info-tier and not gated by the alert
    /// threshold — only by the per-contact / per-room mute and DND (no Critical
    /// exception; a plain message never breaks DND). Always logged regardless.
    #[must_use]
    pub fn should_ring_message(&self, sender: &str, room_id: Option<&str>, dnd: bool) -> bool {
        if dnd {
            return false;
        }
        if self.is_contact_muted(sender) {
            return false;
        }
        room_id.is_none_or(|id| !self.is_room_muted(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muted_contact_is_silent_at_every_severity_but_the_caller_still_logs() {
        let mut p = NotifyPrefs::new();
        assert!(p.mute_contact("nyc3"));
        assert!(!p.mute_contact("nyc3"), "muting twice is idempotent");
        // Silent across the whole severity range — including Critical (an explicit
        // per-contact mute has no breakthrough; the operator asked for silence).
        assert!(!p.should_ring_alert("nyc3", Severity::Critical, false));
        assert!(!p.should_ring_alert("nyc3", Severity::Warning, false));
        assert!(!p.should_ring_message("nyc3", None, false));
        // A different contact is unaffected.
        assert!(p.should_ring_alert("fra1", Severity::Warning, false));
        // Unmute restores the ring.
        assert!(p.unmute_contact("nyc3"));
        assert!(p.should_ring_alert("nyc3", Severity::Warning, false));
    }

    #[test]
    fn severity_threshold_gates_alerts_but_not_messages() {
        let mut p = NotifyPrefs::new();
        // Default Warning threshold: Warning+ ring, Info stays silent (firehose).
        assert!(p.should_ring_alert("h", Severity::Critical, false));
        assert!(p.should_ring_alert("h", Severity::Warning, false));
        assert!(!p.should_ring_alert("h", Severity::Info, false));
        // Raise the bar to Critical: a Warning now stays silent too.
        p.set_threshold(Severity::Critical);
        assert!(p.should_ring_alert("h", Severity::Critical, false));
        assert!(!p.should_ring_alert("h", Severity::Warning, false));
        // Lower it to Info: everything rings.
        p.set_threshold(Severity::Info);
        assert!(p.should_ring_alert("h", Severity::Info, false));
        // The threshold never gates a human message — a chat line is not a machine
        // alert, so it rings even at the strictest threshold.
        p.set_threshold(Severity::Critical);
        assert!(p.should_ring_message("h", None, false));
    }

    #[test]
    fn dnd_hushes_non_critical_but_a_critical_alert_breaks_through() {
        let p = NotifyPrefs::new();
        // Under DND: Warning alert + message go silent, Critical rings.
        assert!(!p.should_ring_alert("h", Severity::Warning, true));
        assert!(!p.should_ring_message("h", None, true));
        assert!(
            p.should_ring_alert("h", Severity::Critical, true),
            "a Critical alert breaks through DND (safety over quiet)"
        );
    }

    #[test]
    fn per_room_mute_silences_that_rooms_messages_only() {
        let mut p = NotifyPrefs::new();
        assert!(p.mute_room("room:ops"));
        assert!(!p.should_ring_message("nyc3", Some("room:ops"), false));
        // The same sender in another room (or a DM) still rings.
        assert!(p.should_ring_message("nyc3", Some("room:build"), false));
        assert!(p.should_ring_message("nyc3", None, false));
        assert!(p.unmute_room("room:ops"));
        assert!(p.should_ring_message("nyc3", Some("room:ops"), false));
    }

    #[test]
    fn prefs_round_trip_through_serde() {
        let mut p = NotifyPrefs::new();
        p.mute_contact("nyc3");
        p.mute_room("room:ops");
        p.set_threshold(Severity::Critical);
        let json = serde_json::to_string(&p).expect("serialize");
        let back: NotifyPrefs = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, back);
        // An empty object hydrates to the defaults (forward/backward compatible).
        let fresh: NotifyPrefs = serde_json::from_str("{}").expect("empty object");
        assert_eq!(fresh, NotifyPrefs::new());
    }
}

//! KDC2-2.10 mpris plugin — `kdeconnect.mpris` body.
//!
//! Lets paired peers control each other's media players (play /
//! pause / next / seek). Upstream multiplexes "state report" and
//! "command" through one packet kind with conditional fields —
//! KDC2 matches.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.mpris` body. Two-way packet — used both for the
/// player→peer state report and the peer→player remote command.
/// Field combinations determine which direction:
///
///   * `player` + `is_playing` + `length` + `pos` populated →
///     state report (player tells peer "I'm at 1:23 of a 3:45
///     track").
///   * `action` populated → remote command (peer tells player to
///     play / pause / next / previous).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MprisBody {
    /// Player identifier (e.g. `spotify`, `firefox`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub player: String,
    /// Current playback state. Optional in commands.
    #[serde(default)]
    pub is_playing: bool,
    /// Track length in milliseconds. Zero/absent in commands.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub length: i64,
    /// Current position in milliseconds.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pos: i64,
    /// Command for the player to execute (`Play`, `Pause`,
    /// `Next`, `Previous`, `Stop`). Empty in state reports.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Discriminator for the two MPRIS body shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MprisKind {
    /// Player state report.
    State,
    /// Remote command.
    Command,
    /// Neither — malformed or probe.
    Empty,
}

impl MprisBody {
    /// Classify which direction this body represents.
    #[must_use]
    pub fn kind(&self) -> MprisKind {
        if !self.action.is_empty() {
            MprisKind::Command
        } else if !self.player.is_empty() {
            MprisKind::State
        } else {
            MprisKind::Empty
        }
    }
}

/// Build a remote MPRIS command packet (peer→player).
#[must_use]
pub fn mpris_command_packet(id_ms: i64, action: String) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.mpris".to_string(),
        body: serde_json::to_value(MprisBody {
            action,
            ..Default::default()
        })
        .expect("MprisBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpris_command_kind_is_command() {
        let p = mpris_command_packet(1, "Pause".to_string());
        let body: MprisBody = serde_json::from_value(p.body).unwrap();
        assert_eq!(body.kind(), MprisKind::Command);
    }

    #[test]
    fn mpris_state_body_kind_is_state() {
        let body = MprisBody {
            player: "spotify".to_string(),
            is_playing: true,
            length: 245_000,
            pos: 80_000,
            action: String::new(),
        };
        assert_eq!(body.kind(), MprisKind::State);
    }

    #[test]
    fn mpris_empty_body_kind_is_empty() {
        assert_eq!(MprisBody::default().kind(), MprisKind::Empty);
    }

    #[test]
    fn mpris_command_omits_state_fields_on_wire() {
        let p = mpris_command_packet(1, "Next".to_string());
        let s = serde_json::to_string(&p).unwrap();
        assert!(!s.contains(r#""player""#));
        assert!(!s.contains(r#""length""#));
        assert!(!s.contains(r#""pos""#));
        // is_playing is bool; defaults to false but does NOT have
        // skip_serializing_if (it's part of every state report).
        // Document this in the assertion message so a future edit
        // doesn't silently flip the behavior.
        assert!(
            s.contains(r#""isPlaying":false"#),
            "isPlaying must always serialize so state reports stay byte-identical: {s}",
        );
        assert!(s.contains(r#""action":"Next""#));
    }

    #[test]
    fn mpris_packet_kind_matches_plugin_token() {
        let p = mpris_command_packet(1, "Play".to_string());
        assert_eq!(p.kind, crate::plugins::PluginKind::Mpris.packet_kind());
    }

    #[test]
    fn mpris_body_round_trips_via_wire() {
        let body = MprisBody {
            player: "firefox".to_string(),
            is_playing: true,
            length: 100_000,
            pos: 25_000,
            action: String::new(),
        };
        let s = serde_json::to_string(&body).unwrap();
        let back: MprisBody = serde_json::from_str(&s).unwrap();
        assert_eq!(back, body);
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-2.17 — MprisPlugin (Plugin trait impl)
    // ─────────────────────────────────────────────────────────

    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn mpris_plugin_kind_and_handles_match_token() {
        let p = MprisPlugin::new();
        assert_eq!(p.kind(), PluginKind::Mpris);
        assert_eq!(p.handles(), &["kdeconnect.mpris"]);
    }

    #[test]
    fn mpris_plugin_queues_state_reports() {
        let mut plugin = MprisPlugin::new();
        let ctx = PluginContext::new("alice", true);
        let body = MprisBody {
            player: "spotify".to_string(),
            is_playing: true,
            length: 245_000,
            pos: 80_000,
            action: String::new(),
        };
        let pkt = mpris_command_packet(1, String::new());
        // Reuse the same packet kind path; substitute body.
        let pkt = Packet {
            body: serde_json::to_value(&body).unwrap(),
            ..pkt
        };
        plugin.process(&pkt, &ctx);
        assert_eq!(plugin.take_received().len(), 1);
    }
}

// ────────────────────────────────────────────────────────────────
// KDC2-2.17b — MprisPlugin (Plugin trait impl, adapter pattern)
// ────────────────────────────────────────────────────────────────

/// `Plugin` impl that mirrors MPRIS state reports + remote
/// commands. Host (`mde-kdc`) drains via `take_received()`;
/// peer-card's media section renders the latest state.
#[derive(Debug, Default)]
pub struct MprisPlugin {
    received: Vec<MprisBody>,
    handles: [&'static str; 1],
}

impl MprisPlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            handles: ["kdeconnect.mpris"],
        }
    }

    /// Drain every received MPRIS body.
    #[must_use]
    pub fn take_received(&mut self) -> Vec<MprisBody> {
        std::mem::take(&mut self.received)
    }

    /// Items currently queued.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }
}

impl crate::plugins::Plugin for MprisPlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Mpris
    }

    fn handles(&self) -> &[&'static str] {
        &self.handles
    }

    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        if let Ok(body) = crate::plugins::from_packet_body::<MprisBody>(packet) {
            self.received.push(body);
        }
        Vec::new()
    }
}

//! `mde-panel-egui` — the MCNF E12 "Quasar" egui **panel client** (E12-7), the
//! egui replacement for the retired libcosmic cosmic-applet.
//!
//! The panel shows two things, both off live mesh state:
//!
//! 1. a **worst-of mesh-health pip** — green only when every lighthouse is up,
//!    red the moment one is degraded/offline, hidden when none are in view; and
//! 2. one **working quick action** — a mesh-wide **Do-Not-Disturb** toggle.
//!
//! This module is the **render-agnostic, fully-tested model** ([`PanelModel`]).
//! It is built from the two live sources the panel subscribes to — the
//! world-readable mesh-status snapshot JSON and the bus-replicated DND state —
//! and exposes accessors the eframe shell ([`mod@crate`]'s `main.rs`) maps to
//! [`mde_egui::Style`]-themed draws. There is **no egui [`Context`], no file IO,
//! and no GPU here**, so the whole model is unit-tested in isolation.
//!
//! Reuse, not reimplementation (§6): the pip's parsing + worst-of decision is the
//! cosmic-applet's [`lighthouse_health_from_snapshot`] (itself built on
//! `mackes-mesh-types`' `LIGHTHOUSE_ROLE`), and the quick action's state is
//! `mde_bus`'s [`DndState`]. This crate is the egui glue that renders them.
//!
//! [`Context`]: mde_egui::egui::Context

use mde_egui::egui::Color32;
use mde_egui::Style;

use mde_bus::dnd::DndState;

// Reuse the cosmic-applet's render-agnostic LIGHTHOUSE-7 model: the snapshot
// parser + the worst-of health enum. `LighthouseHealth` is re-exported so the
// eframe shell can match on it (and the unit tests assert the Style mapping).
use mde_cosmic_applet::lighthouse_health_from_snapshot;
pub use mde_cosmic_applet::LighthouseHealth;

/// The Do-Not-Disturb quick action's button label.
///
/// A const so the bin and its tests read the one string — the action is a toggle;
/// its on/off state is carried by [`PanelModel::dnd_active`] +
/// [`PanelModel::dnd_status`], not the label.
pub const DND_LABEL: &str = "Do Not Disturb";

/// The render-agnostic panel model: the worst-of mesh-health pip plus the
/// mesh-wide Do-Not-Disturb quick-action state.
///
/// Build it from the live sources with [`PanelModel::from_state`] and query the
/// accessors from the render path; flip DND with [`PanelModel::toggled_dnd`] (a
/// pure transform the shell persists to the bus, then adopts via
/// [`PanelModel::set_dnd`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelModel {
    /// Worst-of lighthouse health driving the pip.
    health: LighthouseHealth,
    /// Number of lighthouses reporting online.
    healthy: usize,
    /// Total lighthouses in view.
    total: usize,
    /// Mesh-wide Do-Not-Disturb state (the quick action).
    dnd: DndState,
}

impl PanelModel {
    /// Build the model from the two live sources the panel subscribes to: the
    /// world-readable mesh-status snapshot JSON (parsed via the reused
    /// [`lighthouse_health_from_snapshot`] — a missing/garbage snapshot yields
    /// "no lighthouses", never a panic) and the mesh-wide bus [`DndState`].
    #[must_use]
    pub fn from_state(snapshot: &str, dnd: DndState) -> Self {
        let (health, healthy, total) = lighthouse_health_from_snapshot(snapshot);
        Self {
            health,
            healthy,
            total,
            dnd,
        }
    }

    /// The worst-of lighthouse health behind the pip.
    #[must_use]
    pub const fn health(&self) -> LighthouseHealth {
        self.health
    }

    /// `(healthy, total)` lighthouse counts — rendered inline beside the pip and
    /// in its tooltip.
    #[must_use]
    pub const fn counts(&self) -> (usize, usize) {
        (self.healthy, self.total)
    }

    /// The pip's [`Style`] colour: [`Style::OK`] green when every lighthouse is
    /// up, [`Style::DANGER`] red the moment one is degraded/offline, and `None`
    /// when the snapshot names no lighthouses — the pip is then hidden (an honest
    /// empty state, never a fake dot, mirroring the cosmic-applet it replaces).
    ///
    /// The mapping is kept in lock-step with the reused
    /// [`LighthouseHealth::token`] by a unit test, so the egui colour can't
    /// silently diverge from the rest of the fleet's health verdict.
    #[must_use]
    pub const fn pip_color(&self) -> Option<Color32> {
        match self.health {
            LighthouseHealth::AllHealthy => Some(Style::OK),
            LighthouseHealth::Degraded => Some(Style::DANGER),
            LighthouseHealth::None => None,
        }
    }

    /// The pip's hover tooltip (a `healthy/total` summary), or `None` when there
    /// is no pip. Delegates to the reused [`LighthouseHealth::tooltip`].
    #[must_use]
    pub fn pip_tooltip(&self) -> Option<String> {
        self.health.tooltip(self.healthy, self.total)
    }

    /// Whether mesh-wide Do-Not-Disturb is currently active.
    #[must_use]
    pub const fn dnd_active(&self) -> bool {
        self.dnd.active
    }

    /// A short status line for the DND quick action: when active, who set it plus
    /// the emergency-bypass note (genuine `override=dnd` alerts still surface);
    /// when inactive, that notifications are delivering.
    #[must_use]
    pub fn dnd_status(&self) -> String {
        if !self.dnd.active {
            return "Off — notifications delivering".to_string();
        }
        if self.dnd.set_by_peer.is_empty() {
            "On — emergencies still bypass".to_string()
        } else {
            format!(
                "On — set by {} · emergencies still bypass",
                self.dnd.set_by_peer
            )
        }
    }

    /// The DND state this panel would write when the operator flips the quick
    /// action: `active` inverted, the toggle stamped with `now_unix_ms` and the
    /// local `peer`, and any fleet-wide snoozes preserved. Pure — the eframe
    /// shell persists the result to the bus (`mde_bus::dnd::save_default`, which
    /// replicates it mesh-wide) and then adopts it via [`PanelModel::set_dnd`].
    #[must_use]
    pub fn toggled_dnd(&self, peer: &str, now_unix_ms: i64) -> DndState {
        DndState {
            active: !self.dnd.active,
            since_unix_ms: now_unix_ms,
            set_by_peer: peer.to_string(),
            snoozes: self.dnd.snoozes.clone(),
        }
    }

    /// Adopt a new DND state — after the shell persists a toggle, or on the next
    /// poll of the bus-replicated state.
    pub fn set_dnd(&mut self, dnd: DndState) {
        self.dnd = dnd;
    }
}

#[cfg(test)]
mod tests {
    use super::{DndState, LighthouseHealth, PanelModel, Style, DND_LABEL};
    use mde_bus::dnd::TopicSnooze;

    /// Two lighthouses (one identified by `role`, one by `lighthouse_ips`
    /// membership — exactly the two paths the reused parser honours) plus an
    /// ordinary workstation, each at the chosen presence.
    fn snapshot(lh_role: &str, lh_ip: &str, peer: &str) -> String {
        format!(
            r#"{{"nodes":[
                {{"overlay_ip":"10.42.0.1","presence":"{lh_role}","role":"lighthouse"}},
                {{"overlay_ip":"10.42.0.2","presence":"{lh_ip}","role":"server"}},
                {{"overlay_ip":"10.42.0.50","presence":"{peer}","role":"workstation"}}
            ],"network":{{"lighthouse_ips":["10.42.0.1","10.42.0.2"]}}}}"#
        )
    }

    /// A snapshot with nodes but no lighthouses at all.
    const NO_LIGHTHOUSES: &str = r#"{"nodes":[{"overlay_ip":"10.42.0.50","presence":"online","role":"workstation"}],"network":{"lighthouse_ips":[]}}"#;

    #[test]
    fn pip_colour_tracks_and_agrees_with_the_reused_health_token() {
        // All up → green; the colour must agree with the reused token verdict.
        let all = PanelModel::from_state(
            &snapshot("online", "online", "offline"),
            DndState::default(),
        );
        assert_eq!(all.health(), LighthouseHealth::AllHealthy);
        assert_eq!(all.pip_color(), Some(Style::OK));
        assert_eq!(all.health().token(), Some("beacon_healthy"));

        // Any lighthouse down → red (worst-of), agreeing with the "danger" token.
        let degraded =
            PanelModel::from_state(&snapshot("online", "idle", "online"), DndState::default());
        assert_eq!(degraded.health(), LighthouseHealth::Degraded);
        assert_eq!(degraded.pip_color(), Some(Style::DANGER));
        assert_eq!(degraded.health().token(), Some("danger"));

        // No lighthouses → no pip (hidden), agreeing with the absent token.
        let none = PanelModel::from_state(NO_LIGHTHOUSES, DndState::default());
        assert_eq!(none.health(), LighthouseHealth::None);
        assert_eq!(none.pip_color(), None);
        assert_eq!(none.health().token(), None);
    }

    #[test]
    fn counts_and_tooltip_track_the_snapshot() {
        let degraded = PanelModel::from_state(
            &snapshot("online", "offline", "online"),
            DndState::default(),
        );
        assert_eq!(degraded.counts(), (1, 2));
        let tip = degraded
            .pip_tooltip()
            .expect("a degraded pip has a tooltip");
        assert!(
            tip.contains("1/2"),
            "tooltip should carry the counts: {tip}"
        );

        // No pip → no counts, no tooltip.
        let none = PanelModel::from_state(NO_LIGHTHOUSES, DndState::default());
        assert_eq!(none.counts(), (0, 0));
        assert_eq!(none.pip_tooltip(), None);
    }

    #[test]
    fn garbage_or_missing_snapshot_is_empty_not_a_panic() {
        for bad in ["", "not json", "{}", r#"{"nodes":[]}"#] {
            let m = PanelModel::from_state(bad, DndState::default());
            assert_eq!(m.health(), LighthouseHealth::None);
            assert_eq!(m.counts(), (0, 0));
            assert_eq!(m.pip_color(), None);
            assert_eq!(m.pip_tooltip(), None);
        }
    }

    #[test]
    fn dnd_toggle_flips_stamps_and_preserves_snoozes() {
        // Off, carrying a fleet-wide snooze.
        let snooze = TopicSnooze {
            topic: "fleet/sec".to_string(),
            until_unix_ms: 9_999,
            set_by_peer: "lh-01".to_string(),
        };
        let off = {
            let mut m = PanelModel::from_state(NO_LIGHTHOUSES, DndState::default());
            m.set_dnd(DndState {
                snoozes: vec![snooze.clone()],
                ..DndState::default()
            });
            m
        };
        assert!(!off.dnd_active());

        // Flip on: active, stamped with the instant + the local peer, snooze kept.
        let on = off.toggled_dnd("ws-7", 1_700_000_000_000);
        assert!(on.active);
        assert_eq!(on.since_unix_ms, 1_700_000_000_000);
        assert_eq!(on.set_by_peer, "ws-7");
        assert_eq!(on.snoozes, vec![snooze]);

        // Flip back off from an on-state.
        let mut m = off;
        m.set_dnd(on);
        let back = m.toggled_dnd("ws-7", 1_700_000_000_001);
        assert!(!back.active);
    }

    #[test]
    fn dnd_status_and_active_read_the_state() {
        let off = PanelModel::from_state(NO_LIGHTHOUSES, DndState::default());
        assert!(!off.dnd_active());
        assert!(off.dnd_status().starts_with("Off"));

        let mut on = PanelModel::from_state(NO_LIGHTHOUSES, DndState::default());
        on.set_dnd(DndState {
            active: true,
            since_unix_ms: 5,
            set_by_peer: "lh-02".to_string(),
            ..DndState::default()
        });
        assert!(on.dnd_active());
        let status = on.dnd_status();
        assert!(status.starts_with("On"));
        assert!(status.contains("lh-02"));
        assert!(status.contains("bypass"));

        assert_eq!(DND_LABEL, "Do Not Disturb");
    }
}

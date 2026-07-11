//! `mde-panel-egui` — the MCNF E12 "Quasar" egui **panel client** (E12-7), the
//! egui replacement for the retired Cosmic-era applet.
//!
//! The panel shows two things, both off live mesh state:
//!
//! 1. a **worst-of mesh-health pip** — green only when every lighthouse is up,
//!    red the moment one is degraded/offline, **amber while connecting** (no
//!    mesh-status snapshot has been read yet — e.g. a fresh boot, before the
//!    root timer's first write to tmpfs, or the status writer being down), and
//!    hidden once a snapshot IS in hand but names no lighthouses; and
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
//! mde-lighthouse-health crate's [`lighthouse_health_from_snapshot`] (itself built on
//! `mackes-mesh-types`' `LIGHTHOUSE_ROLE`), and the quick action's state is
//! `mde_bus`'s [`DndState`]. This crate is the egui glue that renders them.
//!
//! [`Context`]: mde_egui::egui::Context

use mde_egui::egui::Color32;
use mde_egui::Style;

use mde_bus::dnd::DndState;

// Reuse mde-lighthouse-health's render-agnostic LIGHTHOUSE-7 model: the snapshot
// parser + the worst-of health enum. `LighthouseHealth` is re-exported so the
// eframe shell can match on it (and the unit tests assert the Style mapping).
use mde_lighthouse_health::lighthouse_health_from_snapshot;
pub use mde_lighthouse_health::LighthouseHealth;

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
    /// Whether a mesh-status snapshot has actually been read this poll. `false`
    /// means "no snapshot yet" (fresh boot / status writer down) — the honest
    /// **connecting** state, which the reused [`LighthouseHealth::None`] can't be
    /// told apart from "a snapshot IS in hand but names no lighthouses".
    snapshot_seen: bool,
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
    /// world-readable mesh-status snapshot JSON and the mesh-wide bus [`DndState`].
    ///
    /// `snapshot` is `Some(json)` when a snapshot was actually read (parsed via
    /// the reused [`lighthouse_health_from_snapshot`] — even a *present* but
    /// garbage/empty-of-lighthouses snapshot yields "no lighthouses", never a
    /// panic), and `None` when no snapshot could be read at all — the honest
    /// **connecting** state ([`PipState::Connecting`]), kept distinct from
    /// "snapshot in hand, but it names no lighthouses" ([`PipState::NoLighthouses`]).
    #[must_use]
    pub fn from_state(snapshot: Option<&str>, dnd: DndState) -> Self {
        let (health, healthy, total) = snapshot.map_or(
            (LighthouseHealth::None, 0, 0),
            lighthouse_health_from_snapshot,
        );
        Self {
            snapshot_seen: snapshot.is_some(),
            health,
            healthy,
            total,
            dnd,
        }
    }

    /// The worst-of lighthouse health behind the pip. (The presentation state the
    /// shell renders is [`PanelModel::pip`], which widens this with the connecting
    /// state; this raw verdict is kept for the token lock-step test.)
    #[must_use]
    pub const fn health(&self) -> LighthouseHealth {
        self.health
    }

    /// The resolved pip presentation state: the reused worst-of lighthouse verdict
    /// widened with [`PipState::Connecting`] when no snapshot has been read yet.
    /// The shell matches this once for the dot colour, pulse, and label.
    #[must_use]
    pub const fn pip(&self) -> PipState {
        PipState::resolve(self.snapshot_seen, self.health)
    }

    /// `(healthy, total)` lighthouse counts — rendered inline beside the pip and
    /// in its tooltip.
    #[must_use]
    pub const fn counts(&self) -> (usize, usize) {
        (self.healthy, self.total)
    }

    /// The pip's hover tooltip: a `healthy/total` summary once a snapshot is in
    /// hand (delegating to the reused [`LighthouseHealth::tooltip`]), a plain
    /// "waiting" note while connecting, or `None` when there is no pip.
    #[must_use]
    pub fn pip_tooltip(&self) -> Option<String> {
        match self.pip() {
            PipState::Connecting => Some("Waiting for the first mesh-status snapshot".to_string()),
            _ => self.health.tooltip(self.healthy, self.total),
        }
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

/// What the mesh-health pip should show, resolved from snapshot availability plus
/// the reused worst-of lighthouse verdict.
///
/// It is the reused [`LighthouseHealth`] widened with one presentation-only state
/// the render-agnostic verdict structurally cannot express — [`PipState::Connecting`],
/// for "no mesh-status snapshot has been read yet". This is glue, not a second
/// health engine (§6): the parsing and the worst-of decision stay in the reused
/// [`lighthouse_health_from_snapshot`]; this enum only adds the honest "we don't
/// know yet" state and carries every pip's [`Style`] look in one place so the
/// shell matches once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipState {
    /// No snapshot yet — fresh boot before the root timer's first tmpfs write, or
    /// the status writer being down. Distinct from [`PipState::NoLighthouses`].
    Connecting,
    /// A snapshot IS in hand, but it names no lighthouses — the pip is hidden.
    NoLighthouses,
    /// One or more lighthouses, and every one is up.
    AllHealthy,
    /// One or more lighthouses, at least one degraded/offline.
    Degraded,
}

impl PipState {
    /// Resolve the pip state from whether a snapshot was read and the reused
    /// verdict: no snapshot → [`PipState::Connecting`]; otherwise the reused
    /// [`LighthouseHealth`] maps 1:1.
    #[must_use]
    pub const fn resolve(snapshot_seen: bool, health: LighthouseHealth) -> Self {
        if !snapshot_seen {
            return Self::Connecting;
        }
        match health {
            LighthouseHealth::AllHealthy => Self::AllHealthy,
            LighthouseHealth::Degraded => Self::Degraded,
            LighthouseHealth::None => Self::NoLighthouses,
        }
    }

    /// The pip dot's [`Style`] colour, or `None` when there is no dot: [`Style::OK`]
    /// green all-healthy, [`Style::DANGER`] red degraded, [`Style::WARN`] amber
    /// connecting, and no dot at all when a snapshot names no lighthouses (an
    /// honest empty state, never a fake dot).
    ///
    /// The health-derived colours are kept in lock-step with the reused
    /// [`LighthouseHealth::token`] by a unit test, so the egui colour can't
    /// silently diverge from the rest of the fleet's health verdict.
    #[must_use]
    pub const fn dot_color(self) -> Option<Color32> {
        match self {
            Self::AllHealthy => Some(Style::OK),
            Self::Degraded => Some(Style::DANGER),
            Self::Connecting => Some(Style::WARN),
            Self::NoLighthouses => None,
        }
    }

    /// Whether the dot should pulse: degraded pulses to draw the eye to a problem,
    /// connecting pulses to show it is actively coming up. The two steady states
    /// (all-healthy, no-lighthouses) don't, so the panel is zero-CPU idle there.
    #[must_use]
    pub const fn pulses(self) -> bool {
        matches!(self, Self::Degraded | Self::Connecting)
    }

    /// The pip's status line and its [`Style`] text colour.
    #[must_use]
    pub const fn label(self) -> (&'static str, Color32) {
        match self {
            Self::AllHealthy => ("All lighthouses healthy", Style::TEXT),
            Self::Degraded => ("Mesh degraded — a lighthouse is down", Style::DANGER),
            Self::Connecting => ("Connecting to mesh…", Style::TEXT_DIM),
            Self::NoLighthouses => ("No lighthouses in view", Style::TEXT_DIM),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DndState, LighthouseHealth, PanelModel, PipState, Style, DND_LABEL};
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
            Some(&snapshot("online", "online", "offline")),
            DndState::default(),
        );
        assert_eq!(all.health(), LighthouseHealth::AllHealthy);
        assert_eq!(all.pip().dot_color(), Some(Style::OK));
        assert_eq!(all.health().token(), Some("beacon_healthy"));

        // Any lighthouse down → red (worst-of), agreeing with the "danger" token.
        let degraded = PanelModel::from_state(
            Some(&snapshot("online", "idle", "online")),
            DndState::default(),
        );
        assert_eq!(degraded.health(), LighthouseHealth::Degraded);
        assert_eq!(degraded.pip().dot_color(), Some(Style::DANGER));
        assert_eq!(degraded.health().token(), Some("danger"));

        // Snapshot in hand but no lighthouses → no pip (hidden), agreeing with the
        // absent token.
        let none = PanelModel::from_state(Some(NO_LIGHTHOUSES), DndState::default());
        assert_eq!(none.health(), LighthouseHealth::None);
        assert_eq!(none.pip(), PipState::NoLighthouses);
        assert_eq!(none.pip().dot_color(), None);
        assert_eq!(none.health().token(), None);
    }

    #[test]
    fn counts_and_tooltip_track_the_snapshot() {
        let degraded = PanelModel::from_state(
            Some(&snapshot("online", "offline", "online")),
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
        let none = PanelModel::from_state(Some(NO_LIGHTHOUSES), DndState::default());
        assert_eq!(none.counts(), (0, 0));
        assert_eq!(none.pip_tooltip(), None);
    }

    #[test]
    fn present_but_garbage_snapshot_is_no_lighthouses_not_a_panic() {
        // A snapshot that WAS read but is unparseable/empty-of-lighthouses is
        // "no lighthouses" (we heard from the mesh) — never a panic, and never
        // mistaken for the not-yet-connected state.
        for bad in ["", "not json", "{}", r#"{"nodes":[]}"#] {
            let m = PanelModel::from_state(Some(bad), DndState::default());
            assert_eq!(m.health(), LighthouseHealth::None);
            assert_eq!(m.pip(), PipState::NoLighthouses);
            assert_eq!(m.counts(), (0, 0));
            assert_eq!(m.pip().dot_color(), None);
            assert_eq!(m.pip_tooltip(), None);
        }
    }

    #[test]
    fn absent_snapshot_is_connecting_not_no_lighthouses() {
        // No snapshot read at all (fresh boot / status writer down) is the honest
        // connecting state — an amber, pulsing pip with a "connecting" line, NOT
        // the alarming "no lighthouses in view" a present-but-empty snapshot gives.
        let connecting = PanelModel::from_state(None, DndState::default());
        assert_eq!(connecting.pip(), PipState::Connecting);
        assert_eq!(connecting.pip().dot_color(), Some(Style::WARN));
        assert!(connecting.pip().pulses());
        assert!(connecting.pip().label().0.contains("Connecting"));
        assert_eq!(connecting.counts(), (0, 0));
        assert!(
            connecting
                .pip_tooltip()
                .is_some_and(|t| t.to_lowercase().contains("waiting")),
            "connecting has a waiting tooltip"
        );

        // The whole point: connecting must NOT read the same as a present snapshot
        // that simply names no lighthouses.
        let empty = PanelModel::from_state(Some(NO_LIGHTHOUSES), DndState::default());
        assert_ne!(connecting.pip(), empty.pip());
        assert_ne!(connecting.pip().dot_color(), empty.pip().dot_color());
        assert_ne!(connecting.pip().label(), empty.pip().label());
    }

    #[test]
    fn pip_state_presentation_is_distinct_and_styled() {
        use PipState::{AllHealthy, Connecting, Degraded, NoLighthouses};

        // Dot colours read from Style (or no dot), each state distinct.
        assert_eq!(AllHealthy.dot_color(), Some(Style::OK));
        assert_eq!(Degraded.dot_color(), Some(Style::DANGER));
        assert_eq!(Connecting.dot_color(), Some(Style::WARN));
        assert_eq!(NoLighthouses.dot_color(), None);

        // Only the two transient/attention states animate; the stable ones idle.
        assert!(Degraded.pulses() && Connecting.pulses());
        assert!(!AllHealthy.pulses() && !NoLighthouses.pulses());

        // Every label is a distinct, non-empty line.
        let labels = [
            AllHealthy.label().0,
            Degraded.label().0,
            Connecting.label().0,
            NoLighthouses.label().0,
        ];
        for l in labels {
            assert!(!l.is_empty(), "a pip label is empty");
        }
        for i in 0..labels.len() {
            for j in (i + 1)..labels.len() {
                assert_ne!(labels[i], labels[j], "two pip labels collide");
            }
        }

        // resolve() maps the reused verdict 1:1 once a snapshot is seen.
        assert_eq!(
            PipState::resolve(true, LighthouseHealth::AllHealthy),
            AllHealthy
        );
        assert_eq!(
            PipState::resolve(true, LighthouseHealth::Degraded),
            Degraded
        );
        assert_eq!(
            PipState::resolve(true, LighthouseHealth::None),
            NoLighthouses
        );
        assert_eq!(
            PipState::resolve(false, LighthouseHealth::AllHealthy),
            Connecting
        );
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
            let mut m = PanelModel::from_state(Some(NO_LIGHTHOUSES), DndState::default());
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
        let off = PanelModel::from_state(Some(NO_LIGHTHOUSES), DndState::default());
        assert!(!off.dnd_active());
        assert!(off.dnd_status().starts_with("Off"));

        let mut on = PanelModel::from_state(Some(NO_LIGHTHOUSES), DndState::default());
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

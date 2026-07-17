//! GUI-6 (Q46/47) — the MCNF lighthouse-health logic layer (LIGHTHOUSE-7).
//! (arch-13: renamed from `mde-cosmic-applet`; the crate is render-agnostic and
//! has no Cosmic-toolkit ties — only the retired name lingered.)
//!
//! This module is the **render-agnostic, fully-tested core** — the
//! lighthouse-health panel indicator (LIGHTHOUSE-7). E12-14b stripped the
//! Cosmic-era panel-shell binary; MCNF 12.0 "Quazar" reuses this core from the
//! egui replacements (`mde-panel-egui` + `mde-shell-egui`) as thin glue, not
//! logic. (APPLAUNCH-9, 2026-06-27: the app-launcher model that used to live
//! here retired into the Front Door — one launcher.)

// ──────────────────── LIGHTHOUSE-7 — panel health indicator ────────────────
//
// A worst-of green/red lighthouse-health indicator for the panel applet surface
// (LIGHTHOUSE-7's "applet" surface): a single dot that is green only when every
// lighthouse is up, red the moment any one is degraded/offline, and absent when
// the snapshot names no lighthouses. Clicking it deep-links into the Workbench
// Lighthouses tab (the same tab the Hub footer + LIGHTHOUSE-4 deep-link reach).
//
// The applet runs as the desktop user and cannot read the root-only replicated
// peer directory, so — exactly like the NEB-CRYPTO-LABEL cipher text — the data
// comes from the **world-readable** mesh-status snapshot (`/run/mde/
// mesh-status.json`), written by the root snapshot timer. This render-agnostic
// layer parses that JSON; the panel surface (today the egui panel/shell) only
// renders the result + spawns the deep-link. Lighthouse identification mirrors the Workbench
// `enrich_roles` (LIGHTHOUSE-9): a node is a lighthouse when its `role` is
// `"lighthouse"` OR its `overlay_ip` is in `network.lighthouse_ips`.

/// The Workbench focus slug that opens the Lighthouses tab (Mesh group). The Hub
/// footer presses `mesh.lighthouses:<host>` for a specific lighthouse; the
/// applet indicator is fleet-wide (worst-of), so it opens the tab with no
/// per-host focus suffix.
pub const LIGHTHOUSE_FOCUS_SLUG: &str = "mesh.lighthouses";

/// The `role` value that marks a lighthouse node — the canonical constant
/// (`mackes_mesh_types::lighthouse::LIGHTHOUSE_ROLE`) the Workbench tab + mackesd
/// also use, imported (not duplicated) so the applet's snapshot-based indicator
/// can't silently diverge from the rest of the fleet if the string ever changes.
use mackes_mesh_types::lighthouse::LIGHTHOUSE_ROLE;

/// The worst-of lighthouse-health the panel indicator renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LighthouseHealth {
    /// One or more lighthouses, and EVERY one is online → green.
    AllHealthy,
    /// One or more lighthouses but at least one is degraded/offline → red.
    Degraded,
    /// The snapshot names no lighthouses (or is absent/unparseable) → the
    /// indicator is hidden (honest empty state, never a fake green/red dot).
    None,
}

impl LighthouseHealth {
    /// The legacy theme-token name the panel surface maps to the dot color
    /// (`beacon_healthy` green / `danger` red — the dedicated lighthouse-beacon
    /// hues, Q13/Q14). `None` has no dot, so no token.
    #[must_use]
    pub fn token(self) -> Option<&'static str> {
        match self {
            LighthouseHealth::AllHealthy => Some("beacon_healthy"),
            LighthouseHealth::Degraded => Some("danger"),
            LighthouseHealth::None => Option::None,
        }
    }

    /// The panel tooltip (`(healthy, total)` of the lighthouse set). `None` →
    /// no indicator, so no tooltip.
    #[must_use]
    pub fn tooltip(self, healthy: usize, total: usize) -> Option<String> {
        match self {
            LighthouseHealth::AllHealthy => {
                Some(format!("Lighthouses: {healthy}/{total} healthy — all up"))
            }
            LighthouseHealth::Degraded => Some(format!(
                "Lighthouses: {healthy}/{total} healthy — open the Lighthouses tab"
            )),
            LighthouseHealth::None => Option::None,
        }
    }
}

/// `(health, healthy, total)` of the lighthouse set, parsed from the mesh-status
/// snapshot JSON. A node is a lighthouse when its `role == "lighthouse"` OR its
/// `overlay_ip` is in `network.lighthouse_ips` (LIGHTHOUSE-9). A lighthouse is
/// counted healthy iff its `presence == "online"` (the snapshot already folds
/// the directory health tier → presence: `healthy→online`, `degraded→idle`,
/// else `offline`). The indicator is worst-of: green only when `healthy ==
/// total` (and `total > 0`), red otherwise; `None` when no lighthouses exist.
/// A missing/garbage snapshot yields `(None, 0, 0)` — never a panic.
#[must_use]
pub fn lighthouse_health_from_snapshot(snapshot: &str) -> (LighthouseHealth, usize, usize) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return (LighthouseHealth::None, 0, 0);
    };
    // The lighthouse overlay-IP set (LIGHTHOUSE-9) — empty when absent.
    let lighthouse_ips: Vec<&str> = v
        .get("network")
        .and_then(|n| n.get("lighthouse_ips"))
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    let Some(nodes) = v.get("nodes").and_then(serde_json::Value::as_array) else {
        return (LighthouseHealth::None, 0, 0);
    };
    let mut total = 0usize;
    let mut healthy = 0usize;
    for node in nodes {
        let role = node.get("role").and_then(serde_json::Value::as_str);
        let overlay_ip = node
            .get("overlay_ip")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let is_lighthouse = role == Some(LIGHTHOUSE_ROLE)
            || (!overlay_ip.is_empty() && lighthouse_ips.contains(&overlay_ip));
        if !is_lighthouse {
            continue;
        }
        total += 1;
        if node.get("presence").and_then(serde_json::Value::as_str) == Some("online") {
            healthy += 1;
        }
    }
    let health = if total == 0 {
        LighthouseHealth::None
    } else if healthy == total {
        LighthouseHealth::AllHealthy
    } else {
        LighthouseHealth::Degraded
    };
    (health, healthy, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────── LIGHTHOUSE-7 — panel health indicator ──────────────

    /// A snapshot with one lighthouse (by role) + one (by overlay-IP membership)
    /// + one ordinary node, each at a chosen presence.
    fn lh_snapshot(lh_role: &str, lh_ip: &str, peer: &str) -> String {
        format!(
            r#"{{"nodes":[
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"{lh_role}","role":"lighthouse"}},
                {{"hostname":"lh-02","overlay_ip":"10.42.0.2","presence":"{lh_ip}","role":"server"}},
                {{"hostname":"ws-1","overlay_ip":"10.42.0.50","presence":"{peer}","role":"workstation"}}
            ],"network":{{"lighthouse_ips":["10.42.0.1","10.42.0.2"]}}}}"#
        )
    }

    #[test]
    fn lighthouse_all_healthy_is_green() {
        // Both lighthouses online (one by role, one by overlay-IP membership);
        // the workstation's state is irrelevant.
        let (h, healthy, total) =
            lighthouse_health_from_snapshot(&lh_snapshot("online", "online", "offline"));
        assert_eq!(h, LighthouseHealth::AllHealthy);
        assert_eq!((healthy, total), (2, 2));
        assert_eq!(h.token(), Some("beacon_healthy"));
    }

    #[test]
    fn lighthouse_any_down_is_red() {
        // The role lighthouse is online but the overlay-IP lighthouse is idle →
        // worst-of red.
        let (h, healthy, total) =
            lighthouse_health_from_snapshot(&lh_snapshot("online", "idle", "online"));
        assert_eq!(h, LighthouseHealth::Degraded);
        assert_eq!((healthy, total), (1, 2));
        assert_eq!(h.token(), Some("danger"));
        // An offline lighthouse is just as red.
        let (h2, _, _) =
            lighthouse_health_from_snapshot(&lh_snapshot("offline", "online", "online"));
        assert_eq!(h2, LighthouseHealth::Degraded);
    }

    #[test]
    fn no_lighthouses_means_no_indicator() {
        // A snapshot with nodes but none that are lighthouses → hidden, no token.
        let none = r#"{"nodes":[{"hostname":"ws-1","overlay_ip":"10.42.0.50","presence":"online","role":"workstation"}],"network":{"lighthouse_ips":[]}}"#;
        let (h, healthy, total) = lighthouse_health_from_snapshot(none);
        assert_eq!(h, LighthouseHealth::None);
        assert_eq!((healthy, total), (0, 0));
        assert_eq!(h.token(), None);
        assert_eq!(h.tooltip(0, 0), None);
    }

    #[test]
    fn lighthouse_role_identifies_even_without_ip_membership() {
        // role=="lighthouse" counts even when network.lighthouse_ips is absent
        // (older snapshots before LIGHTHOUSE-9 wrote the IP set).
        let by_role = r#"{"nodes":[{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online","role":"lighthouse"}],"network":{}}"#;
        let (h, healthy, total) = lighthouse_health_from_snapshot(by_role);
        assert_eq!(h, LighthouseHealth::AllHealthy);
        assert_eq!((healthy, total), (1, 1));
    }

    #[test]
    fn missing_or_garbage_snapshot_is_none_not_a_panic() {
        assert_eq!(
            lighthouse_health_from_snapshot("not json").0,
            LighthouseHealth::None
        );
        assert_eq!(
            lighthouse_health_from_snapshot("{}").0,
            LighthouseHealth::None
        );
        assert_eq!(
            lighthouse_health_from_snapshot(r#"{"nodes":[]}"#).0,
            LighthouseHealth::None
        );
    }

    #[test]
    fn lighthouse_tooltips_present_only_when_an_indicator_shows() {
        assert_eq!(
            LighthouseHealth::AllHealthy.tooltip(3, 3),
            Some("Lighthouses: 3/3 healthy — all up".to_string())
        );
        assert_eq!(
            LighthouseHealth::Degraded.tooltip(1, 3),
            Some("Lighthouses: 1/3 healthy — open the Lighthouses tab".to_string())
        );
        assert_eq!(LighthouseHealth::None.tooltip(0, 0), None);
    }

    #[test]
    fn lighthouse_focus_slug_matches_the_workbench_tab() {
        // Must match the Mesh-group "lighthouses" panel slug the Workbench
        // resolves (model.rs::view_from_focus_slug) — the Hub uses the same
        // base slug (LIGHTHOUSE-4).
        assert_eq!(LIGHTHOUSE_FOCUS_SLUG, "mesh.lighthouses");
    }
}

//! Sidebar group + panel model, ported from
//! `mackes/workbench/shell/sidebar_window.py::_build_nav`.
//!
//! The v1.x nav was a GTK [`NavGroup`] list with lazy panel-import
//! lambdas; CB-1 retires that surface in favour of a pure-data
//! [`nav_model`] that the Iced sidebar consumes.

use std::fmt;

/// One of the top-level sidebar groups per `.claude/CLAUDE.md`
/// §4 Index ("Sidebar shell" row) and the CB-1.2 lock ("9 groups
/// (Dashboard / Apps / Devices / Fleet / Look & Feel / Maintain /
/// Network / System / Help)"), extended by **Compute** (E6.10 —
/// local + fleet VMs / pods, placed next to Fleet as a sibling
/// infra-ops domain). Order is load-bearing — it drives the
/// Ctrl+digit keyboard hotkey dispatch (CB-1.2 keyboard nav lock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Dashboard,
    Apps,
    Devices,
    Fleet,
    Compute,
    LookAndFeel,
    Maintain,
    Network,
    System,
    Help,
}

impl Group {
    /// Stable kebab-case slug used in deep-link URLs
    /// (`mde --focus <group>.<panel>`).
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Apps => "apps",
            Self::Devices => "devices",
            Self::Fleet => "fleet",
            Self::Compute => "compute",
            Self::LookAndFeel => "look_and_feel",
            Self::Maintain => "maintain",
            Self::Network => "network",
            Self::System => "system",
            Self::Help => "help",
        }
    }

    /// Sentence-case label shown in the sidebar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Overview",
            Self::Apps => "Apps",
            Self::Devices => "Devices",
            Self::Fleet => "Fleet",
            Self::Compute => "Compute",
            Self::LookAndFeel => "Look & Feel",
            Self::Maintain => "Maintain",
            Self::Network => "Network",
            Self::System => "System",
            Self::Help => "Help",
        }
    }

    /// Stable display order (drives the Ctrl+1..9 hotkey dispatch).
    #[must_use]
    pub const fn all() -> [Self; 10] {
        [
            Self::Dashboard,
            Self::Apps,
            Self::Devices,
            Self::Fleet,
            Self::Compute,
            Self::LookAndFeel,
            Self::Maintain,
            Self::Network,
            Self::System,
            Self::Help,
        ]
    }

    /// The groups EXPOSED in the sidebar + the Ctrl+digit hotkeys —
    /// `all()` minus `Network` (E4.15). The Network panels migrated to
    /// Settings ▸ Network, so the Workbench no longer exposes the group
    /// as a sidebar/hotkey destination; the panels themselves stay in
    /// `all()`/`nav_model()` and remain reachable via
    /// `mde-workbench --focus network.<panel>` (the Settings deep-links).
    #[must_use]
    pub fn sidebar_groups() -> Vec<Self> {
        Self::all()
            .into_iter()
            .filter(|g| *g != Self::Network)
            .collect()
    }

    /// Parse a kebab-case slug back into the matching group.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::all().into_iter().find(|g| g.slug() == slug)
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Per-group leaf panel. Slug + label are stable — the Iced view
/// layer indexes panels by [`Panel::slug`] for deep-link routing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Panel {
    slug: &'static str,
    label: &'static str,
}

impl Panel {
    #[must_use]
    pub const fn new(slug: &'static str, label: &'static str) -> Self {
        Self { slug, label }
    }

    #[must_use]
    pub const fn slug(&self) -> &'static str {
        self.slug
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.label
    }
}

/// One full sidebar row: a group plus its ordered leaf panels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEntry {
    pub group: Group,
    pub panels: Vec<Panel>,
}

/// Active view in the right pane. Either a group landing page
/// (no leaf selected) or a specific panel under that group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Group(Group),
    Panel { group: Group, panel: &'static str },
}

impl Default for View {
    fn default() -> Self {
        Self::Group(Group::Dashboard)
    }
}

impl View {
    /// Active group regardless of whether a leaf panel is selected.
    #[must_use]
    pub const fn group(self) -> Group {
        match self {
            Self::Group(g) | Self::Panel { group: g, .. } => g,
        }
    }

    /// Selected panel slug, if any.
    #[must_use]
    pub const fn panel_slug(self) -> Option<&'static str> {
        match self {
            Self::Group(_) => None,
            Self::Panel { panel, .. } => Some(panel),
        }
    }
}

/// Canonical sidebar nav model — the source of truth for the
/// sidebar widget + keyboard dispatch + deep-link routing.
///
/// Panel lists mirror the v1.x `_build_nav` shape except for
/// surfaces the CB-1 lock retires:
///   * Look & Feel drops `polybar_editor` (CB-1.6 lock — sway
///     replaces polybar; the panel surface is gone).
///   * Apps drops the legacy `search` panel (subsumed by the
///     unified `installed` panel in CB-1.3).
#[must_use]
pub fn nav_model() -> Vec<NavEntry> {
    vec![
        NavEntry {
            group: Group::Dashboard,
            panels: vec![Panel::new("home", "Home")],
        },
        NavEntry {
            group: Group::Apps,
            panels: vec![
                // E6.3 — install/remove were wired in panel_body but absent
                // from nav_model (so the role card + sidebar never surfaced
                // them); default_apps moved here from System per the E6.3
                // acceptance. Order follows the acceptance; `panel` (Panel
                // Apps) stays as a bonus link.
                Panel::new("install", "Install"),
                Panel::new("installed", "Installed"),
                Panel::new("remove", "Remove"),
                Panel::new("sources", "Sources"),
                Panel::new("default_apps", "Default Apps"),
                Panel::new("panel", "Panel Apps"),
            ],
        },
        NavEntry {
            group: Group::Devices,
            panels: vec![
                Panel::new("displays", "Displays"),
                Panel::new("keyboard", "Keyboard"),
                Panel::new("mouse", "Mouse & Touchpad"),
                Panel::new("power", "Power"),
                // E6.3/E6.4 — Session (logout/lock/session settings) sits
                // with Power under Devices per the E6.4 acceptance (moved
                // here from System, which E6.8's acceptance omits it from).
                Panel::new("session", "Session"),
                Panel::new("sound", "Sound"),
                Panel::new("printers", "Printers"),
                Panel::new("music", "Music"),
                Panel::new("removable", "Removable Media"),
                // v4.0.1 WB-1 (Phase 0.7 rescue 2026-05-23):
                // wire the Connected Devices surface that
                // crates/mde-workbench/src/panels/connect.rs
                // has been shipping in #![allow(dead_code)]
                // form since KDC2-5.4..5.7. The previous nav
                // model retired the standalone panel in favor
                // of mde-peer-card — that produced a missing-
                // modal report from the operator, so the
                // Workbench surface is back.
                Panel::new("connect", "Connected Devices"),
            ],
        },
        NavEntry {
            group: Group::Fleet,
            panels: vec![
                Panel::new("inventory", "Inventory"),
                Panel::new("playbooks", "Playbooks"),
                Panel::new("run_history", "Run History"),
                Panel::new("settings", "Settings"),
                Panel::new("revisions", "Revisions"),
            ],
        },
        NavEntry {
            group: Group::Compute,
            // E6.10 — the Compute group root renders the bespoke instance
            // list (local + fleet VMs / pods); "Instances" is its sidebar
            // sub-entry of the same view. Templates / wizard / migration
            // surfaces land as further panels in later E6.10 slices.
            panels: vec![Panel::new("instances", "Instances")],
        },
        NavEntry {
            group: Group::LookAndFeel,
            panels: vec![
                Panel::new("themes", "Themes"),
                Panel::new("fonts", "Fonts"),
                Panel::new("wallpaper", "Wallpaper"),
                // v4.0.1 (2026-05-23) — panel.toml sync-status
                // surface. Reads mackesd healthz JSON + the local
                // panel.toml mtime to surface "synced to revision
                // N at HH:MM by peer-X" or "drifted by N keys".
                Panel::new("sync_status", "Panel Sync Status"),
            ],
        },
        NavEntry {
            group: Group::Maintain,
            panels: vec![
                Panel::new("hub", "Hub"),
                Panel::new("snapshots", "Snapshots"),
                Panel::new("debloat", "Debloat"),
                Panel::new("health_check", "Health Check"),
                Panel::new("repair", "Repair"),
                Panel::new("drift", "Drift"),
            ],
        },
        NavEntry {
            group: Group::Network,
            panels: vec![
                Panel::new("wifi", "Wi-Fi"),
                Panel::new("mesh_control", "Mesh Control"),
                Panel::new("mesh_pending", "Mesh Pending"),
                Panel::new("mesh_history", "Mesh History"),
                Panel::new("mesh_join", "Mesh Join"),
                Panel::new("mesh_topology", "Mesh Topology"),
                Panel::new("mesh_services", "Mesh Services"),
                // MESHFS-13.1 (v5.0.0) — Mesh Storage panel (per-peer
                // chunkserver status). Already wired in app.rs's panel
                // router + reached by the Overview's File Sharing row;
                // listed here so `--focus network.mesh_storage` resolves
                // and the curated label renders.
                Panel::new("mesh_storage", "Mesh Storage"),
                Panel::new("network_hosts", "Network Hosts"),
                Panel::new("mesh_bus", "Mackes Bus"),
                Panel::new("mesh_federation", "Mesh Federation"),
                // NF-13.8 (v2.5) — service-publishing surface
                // pairs naturally with Mesh Services. Best-choice
                // deviation from the worklist note that put this
                // under Fleet: Fleet is for cluster-wide ops,
                // Network is where every mesh_* panel lives, and
                // the operator looks for "what this peer publishes"
                // alongside "what daemons are running".
                Panel::new("service_publishing", "Service Publishing"),
                Panel::new("vpn", "VPN"),
                Panel::new("firewall", "Firewall"),
                Panel::new("remote_desktop", "Remote Access"),
                // KDC2-5.8 (v2.1, 2026-05-22): "KDE Connect"
                // standalone panel retired. KDC integration
                // surfaces through `mde-peer-card` under the
                // Mesh sidebar group + conditional phone
                // sections (KDC2-5.4..5.7). The v13.0 wrapper
                // approach was superseded by KDC2 native; the
                // sidebar entry would have led to a broken
                // panel.
            ],
        },
        NavEntry {
            group: Group::System,
            panels: vec![
                Panel::new("datetime", "Date & Time"),
                // E6.8 — logs/resources/system_update were wired under
                // Maintain but orphaned from its nav (reachable only via a
                // direct deep-link); surfaced here under System per the E6.8
                // acceptance. (default_apps→Apps E6.3, session→Devices E6.4.)
                Panel::new("logs", "Logs"),
                Panel::new("resources", "Resources"),
                Panel::new("system_update", "System Update"),
                Panel::new("notifications", "Notifications"),
            ],
        },
        NavEntry {
            group: Group::Help,
            panels: vec![
                Panel::new("index", "Help Topics"),
                // E6.9 — About/Help: the single-source project
                // disclaimer/mission (embedded DISCLAIMER.md).
                Panel::new("about", "About"),
            ],
        },
    ]
}

/// Resolve `(group, panel_slug)` into the matching panel's
/// curated display label per the [`nav_model`]. Returns `None`
/// when the slug doesn't match any panel in that group — callers
/// fall back to a title-cased slug or a generic group label.
///
/// Added v4.0.1 BUG-19 (2026-05-23) so the "panel under
/// construction" catch-all can render the curated label
/// ("Mesh Topology", "Wi-Fi") instead of the raw slug
/// ("mesh_topology", "wifi") when a panel view is requested
/// before its reducer ships.
#[must_use]
pub fn resolve_panel_label(group: Group, panel_slug: &str) -> Option<&'static str> {
    nav_model()
        .into_iter()
        .find(|e| e.group == group)
        .and_then(|e| {
            e.panels
                .iter()
                .find(|p| p.slug() == panel_slug)
                .map(|p| p.label())
        })
}

/// Resolve a deep-link slug into the matching [`View`]. Accepts
/// `<group>` or `<group>.<panel>` forms (e.g. `network` or
/// `network.remote_desktop`). Unknown slugs return `None`.
/// `network.mesh_ssh` (the retired B1 entry) aliases to the
/// Remote Access panel that absorbed it (SVC-1).
#[must_use]
pub fn view_from_focus_slug(slug: &str) -> Option<View> {
    let slug = if slug == "network.mesh_ssh" {
        "network.remote_desktop"
    } else {
        slug
    };
    let (group_slug, panel_slug) = slug
        .split_once('.')
        .map_or((slug, None), |(g, p)| (g, Some(p)));
    let group = Group::from_slug(group_slug)?;
    match panel_slug {
        None => Some(View::Group(group)),
        Some(p) => nav_model()
            .into_iter()
            .find(|e| e.group == group)
            .and_then(|e| {
                e.panels
                    .iter()
                    .find(|panel| panel.slug() == p)
                    .map(|panel| View::Panel {
                        group,
                        panel: panel.slug(),
                    })
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_model_has_all_groups_in_locked_order() {
        let nav = nav_model();
        // 10 groups since E6.10 added Compute next to Fleet.
        assert_eq!(nav.len(), 10);
        let order: Vec<Group> = nav.iter().map(|e| e.group).collect();
        assert_eq!(order, Group::all().to_vec());
    }

    #[test]
    fn every_group_has_at_least_one_panel() {
        for entry in nav_model() {
            assert!(
                !entry.panels.is_empty(),
                "group {:?} has no panels — sidebar would render a dead row",
                entry.group
            );
        }
    }

    #[test]
    fn group_slugs_are_unique_and_kebab_case() {
        let slugs: Vec<&str> = Group::all().iter().map(|g| g.slug()).collect();
        let mut sorted = slugs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(slugs.len(), sorted.len(), "duplicate group slug");
        for slug in slugs {
            assert!(
                slug.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "group slug {slug} must be lowercase + underscore only"
            );
        }
    }

    #[test]
    fn panel_slugs_unique_within_each_group() {
        for entry in nav_model() {
            let slugs: Vec<&str> = entry.panels.iter().map(Panel::slug).collect();
            let mut sorted = slugs.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                slugs.len(),
                sorted.len(),
                "duplicate panel slug under {:?}: {slugs:?}",
                entry.group
            );
        }
    }

    #[test]
    fn group_from_slug_round_trips() {
        for g in Group::all() {
            assert_eq!(Group::from_slug(g.slug()), Some(g));
        }
        assert_eq!(Group::from_slug("not-a-group"), None);
    }

    #[test]
    fn view_default_is_dashboard_group() {
        assert_eq!(View::default(), View::Group(Group::Dashboard));
    }

    #[test]
    fn view_group_extractor_works_for_both_variants() {
        assert_eq!(View::Group(Group::Apps).group(), Group::Apps);
        assert_eq!(
            View::Panel {
                group: Group::Network,
                panel: "remote_desktop"
            }
            .group(),
            Group::Network
        );
    }

    #[test]
    fn view_panel_slug_extractor_distinguishes_variants() {
        assert_eq!(View::Group(Group::Help).panel_slug(), None);
        assert_eq!(
            View::Panel {
                group: Group::Help,
                panel: "index"
            }
            .panel_slug(),
            Some("index")
        );
    }

    #[test]
    fn focus_slug_resolves_group_only() {
        assert_eq!(
            view_from_focus_slug("network"),
            Some(View::Group(Group::Network))
        );
    }

    #[test]
    fn focus_slug_resolves_group_and_panel() {
        assert_eq!(
            view_from_focus_slug("network.remote_desktop"),
            Some(View::Panel {
                group: Group::Network,
                panel: "remote_desktop"
            })
        );
        // The retired mesh_ssh slug aliases to Remote Access (SVC-1/B1).
        assert_eq!(
            view_from_focus_slug("network.mesh_ssh"),
            Some(View::Panel {
                group: Group::Network,
                panel: "remote_desktop"
            })
        );
    }

    #[test]
    fn focus_slug_rejects_unknown_group() {
        assert_eq!(view_from_focus_slug("not-a-group"), None);
    }

    #[test]
    fn focus_slug_rejects_unknown_panel_under_known_group() {
        assert_eq!(view_from_focus_slug("network.not-a-panel"), None);
    }

    #[test]
    fn group_display_renders_label() {
        assert_eq!(format!("{}", Group::LookAndFeel), "Look & Feel");
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-5.8 — "KDE Connect" panel retirement.
    // The v13.0 standalone panel was superseded by the KDC2
    // native re-implementation; integration surfaces through
    // mde-peer-card now. The sidebar entry would have led to
    // a broken panel — regression tests below catch any future
    // re-addition.
    // ─────────────────────────────────────────────────────────

    #[test]
    fn kde_connect_panel_id_absent_from_nav_model() {
        let nav = nav_model();
        for entry in &nav {
            for panel in &entry.panels {
                assert_ne!(
                    panel.slug, "kde_connect",
                    "kde_connect panel must not reappear in nav (KDC2-5.8)",
                );
            }
        }
    }

    #[test]
    fn kde_connect_focus_slug_no_longer_resolves() {
        // `mde --focus network.kde_connect` must return None
        // now that the panel id is gone. Operators who had
        // muscle memory for the old slug see a clean miss
        // rather than a broken panel.
        assert_eq!(
            view_from_focus_slug("network.kde_connect"),
            None,
            "stale kde_connect slug must not resolve",
        );
    }
}

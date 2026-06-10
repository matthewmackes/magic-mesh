//! Sidebar group + panel model, ported from
//! `mackes/workbench/shell/sidebar_window.py::_build_nav`.
//!
//! The v1.x nav was a GTK [`NavGroup`] list with lazy panel-import
//! lambdas; CB-1 retires that surface in favour of a pure-data
//! [`nav_model`] that the Iced sidebar consumes.

use std::fmt;

/// One of the top-level sidebar groups. **PLANES-1 (the five-plane
/// re-IA, `docs/design/planes.md`)** rebuilds the nav top-to-bottom as
/// the operator's tree: a **Peers** Front Door, then the five planes
/// (**This Node · Controller · Network · Fleet · Provisioning**), then
/// the personal **Desktop** cluster (Dashboard / Apps / Devices /
/// Compute / Look & Feel / Maintain / System / Help) grouped last (W4–
/// W16). Network + Fleet survive the old IA as planes in their own
/// right; ThisNode / Controller / Provisioning / Peers are new.
/// Order is load-bearing — it drives the Ctrl+digit hotkey dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    // ── Front Door ──
    Peers,
    // ── The five planes ──
    ThisNode,
    Controller,
    Network,
    Fleet,
    Provisioning,
    // ── Desktop cluster (personal panels, grouped last) ──
    Dashboard,
    Apps,
    Devices,
    Compute,
    LookAndFeel,
    Maintain,
    System,
    Help,
}

impl Group {
    /// Stable kebab-case slug used in deep-link URLs
    /// (`mde --focus <group>.<panel>`). PLANES-1 is a clean break
    /// (W11): the plane slugs are new ids, no back-compat aliases.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Peers => "peers",
            Self::ThisNode => "node",
            Self::Controller => "controller",
            Self::Network => "network",
            Self::Fleet => "fleet",
            Self::Provisioning => "provisioning",
            Self::Dashboard => "dashboard",
            Self::Apps => "apps",
            Self::Devices => "devices",
            Self::Compute => "compute",
            Self::LookAndFeel => "look_and_feel",
            Self::Maintain => "maintain",
            Self::System => "system",
            Self::Help => "help",
        }
    }

    /// Sentence-case label shown in the sidebar (short plane labels
    /// per W4: "This Node · Controller · Network · Fleet · Provisioning").
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Peers => "Peers",
            Self::ThisNode => "This Node",
            Self::Controller => "Controller",
            Self::Network => "Network",
            Self::Fleet => "Fleet",
            Self::Provisioning => "Provisioning",
            Self::Dashboard => "Overview",
            Self::Apps => "Apps",
            Self::Devices => "Devices",
            Self::Compute => "Compute",
            Self::LookAndFeel => "Look & Feel",
            Self::Maintain => "Maintain",
            Self::System => "System",
            Self::Help => "Help",
        }
    }

    /// Stable display order (drives the Ctrl+1..9 hotkey dispatch —
    /// the first nine sidebar groups, Front Door + planes first).
    #[must_use]
    pub const fn all() -> [Self; 14] {
        [
            Self::Peers,
            Self::ThisNode,
            Self::Controller,
            Self::Network,
            Self::Fleet,
            Self::Provisioning,
            Self::Dashboard,
            Self::Apps,
            Self::Devices,
            Self::Compute,
            Self::LookAndFeel,
            Self::Maintain,
            Self::System,
            Self::Help,
        ]
    }

    /// The groups EXPOSED in the sidebar + the Ctrl+digit hotkeys.
    /// PLANES-1 (W4) promotes **Network** back to a first-class plane
    /// in the sidebar — the old E4.15 hide (Network folded into
    /// Settings) is superseded by the five-plane IA, so the full tree
    /// is shown day-one (W16). Equal to [`Self::all`] now.
    #[must_use]
    pub fn sidebar_groups() -> Vec<Self> {
        Self::all().into_iter().collect()
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
        // PD-4 / D2 / PLANES-1 — the Peers directory is the platform
        // Front Door: a plain launch lands on it (deep-links still
        // override). It now lives in its own `Peers` plane group.
        Self::Panel {
            group: Group::Peers,
            panel: "peers",
        }
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
        // ── Front Door ──────────────────────────────────────────────
        // PLANES-1 (W7) — the Peers directory, one component two doors
        // (also surfaced as Controller/Inventory below). Mesh Map rides
        // along as the live graph view.
        NavEntry {
            group: Group::Peers,
            panels: vec![
                Panel::new("peers", "Peers"),
                Panel::new("mesh_topology", "Mesh Map"),
            ],
        },
        // ── Plane 1: This Node (the local box, W9/W17–W28) ──────────
        NavEntry {
            group: Group::ThisNode,
            panels: vec![
                // PLANES-4 — enrollment identity + cert lifecycle.
                Panel::new("registration", "Registration"),
                // PLANES-5 — replicated PeerProbe hardware view (W19).
                Panel::new("hardware", "Inventory"),
                // PLANES-6 — ENT-7 doctor + service controls + alarms.
                Panel::new("health_check", "Health"),
                // Mesh Services folds into This Node/Health (W4).
                Panel::new("mesh_services", "Mesh Services"),
                // PLANES-7 — applied vs newest revision + reconcile.
                Panel::new("config_apply", "Config"),
                // PLANES-8 — journald mesh-unit view + Netdata strip.
                Panel::new("mesh_logs", "Logs & Metrics"),
            ],
        },
        // ── Plane 2: Controller (a plane, not a place — W3/W29–W52) ──
        NavEntry {
            group: Group::Controller,
            panels: vec![
                // Peers = Controller/Inventory, the second door (W7).
                Panel::new("peers", "Inventory"),
                // Mesh Control gets its own Controller entry (W52).
                Panel::new("mesh_control", "Mesh Control"),
                // PLANES-10 — job templates + run history (absorbs
                // Playbooks, W40).
                Panel::new("jobs", "Jobs"),
                Panel::new("playbooks", "Playbooks"),
                Panel::new("run_history", "Run History"),
                // PLANES-11 — Drift folds into Remediation (W13/W41).
                Panel::new("drift", "Remediation"),
                // PLANES-12 — hash-chained audit timeline + verify.
                Panel::new("audit", "Audit"),
                // PLANES-13 — declarative TOML policy (W46–W51, NEW).
                Panel::new("policy", "Policy"),
                // PLANES-14 — fleet-wide structured-log search (OBS-5).
                Panel::new("fleet_logs", "Fleet Logs"),
                // Fleet Revisions folds into Controller/Config (W4).
                Panel::new("revisions", "Config"),
                Panel::new("settings", "Settings"),
            ],
        },
        // ── Plane 3: Network (full plane now, W65–W80) ──────────────
        NavEntry {
            group: Group::Network,
            panels: vec![
                // PLANES-15 — nmstate desired-vs-actual (W65–W68, NEW).
                Panel::new("interfaces", "Interfaces"),
                Panel::new("wifi", "Wi-Fi"),
                // PLANES-16 — VPN topology + client profiles (W72/W73).
                Panel::new("vpn", "VPN"),
                // PLANES-17 — firewalld zones (W69–W71).
                Panel::new("firewall", "Firewall"),
                // PLANES-18 — mesh DNS roster → resolved (W74/W75, NEW).
                Panel::new("dns", "Mesh DNS"),
                // PLANES-19 — routing display + validation (W76/W79, NEW).
                Panel::new("routing", "Routing"),
                Panel::new("remote_desktop", "Remote Access"),
                Panel::new("network_hosts", "Network Hosts"),
                Panel::new("mesh_storage", "Mesh Storage"),
                Panel::new("mesh_bus", "Mackes Bus"),
                Panel::new("mesh_federation", "Mesh Federation"),
                Panel::new("service_publishing", "Service Publishing"),
                Panel::new("mesh_pending", "Mesh Pending"),
                Panel::new("mesh_history", "Mesh History"),
                Panel::new("mesh_join", "Mesh Join"),
            ],
        },
        // ── Plane 4: Fleet (rollup lens, not config — W81–W87) ──────
        NavEntry {
            group: Group::Fleet,
            panels: vec![
                // PLANES-20 — role-grouped fleet rollup dashboard.
                Panel::new("fleet_rollup", "Fleet Rollup"),
                Panel::new("inventory", "Fleet Inventory"),
                // Capability tags: hop/execution/headless (W82, NEW).
                Panel::new("tags", "Capability Tags"),
            ],
        },
        // ── Plane 5: Provisioning (after PKG core, W53–W64) ─────────
        NavEntry {
            group: Group::Provisioning,
            panels: vec![
                // PLANES-23 — node role pins + capability tags (W58).
                Panel::new("node_roles", "Node Roles"),
                // PLANES-21 — install profiles (W56/W57/W60, NEW).
                Panel::new("profiles", "Install Profiles"),
                // PLANES-22 — images: ISO/VM/container/USB (W53, NEW).
                Panel::new("images", "Images"),
                // PLANES-24 — GitHub-RPM mirrors on LizardFS (W61, NEW).
                Panel::new("mirrors", "Mirrors"),
            ],
        },
        // ── Desktop cluster (personal panels, grouped last) ─────────
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
            // PLANES-1 — the mesh-facing Maintain panels re-homed into
            // the planes (health_check→This Node, drift/audit/fleet_logs
            // →Controller, mesh_logs→This Node); what stays is personal
            // workstation upkeep.
            panels: vec![
                Panel::new("hub", "Hub"),
                Panel::new("snapshots", "Snapshots"),
                Panel::new("debloat", "Debloat"),
                Panel::new("repair", "Repair"),
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
        // PLANES-1 — 14 groups: Peers Front Door + 5 planes + the
        // 8-group Desktop cluster.
        assert_eq!(nav.len(), 14);
        let order: Vec<Group> = nav.iter().map(|e| e.group).collect();
        assert_eq!(order, Group::all().to_vec());
    }

    #[test]
    fn planes_are_present_with_short_labels() {
        // PLANES-1 (W4) — the five plane sections exist with the
        // locked short labels, Peers Front Door first.
        assert_eq!(Group::all()[0], Group::Peers);
        assert_eq!(Group::ThisNode.label(), "This Node");
        assert_eq!(Group::Controller.label(), "Controller");
        assert_eq!(Group::Provisioning.label(), "Provisioning");
        // Peers = Controller/Inventory dual-door (W7): the directory
        // slug resolves under both groups.
        assert!(view_from_focus_slug("peers.peers").is_some());
        assert!(view_from_focus_slug("controller.peers").is_some());
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
    fn view_default_is_the_peers_front_door() {
        // PD-4 / D2 / PLANES-1 — a plain launch lands on the Peers
        // directory, now in its own Front Door plane.
        assert_eq!(
            View::default(),
            View::Panel {
                group: Group::Peers,
                panel: "peers"
            }
        );
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

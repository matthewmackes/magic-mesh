//! Sidebar group + panel model, ported from
//! `mackes/workbench/shell/sidebar_window.py::_build_nav`.
//!
//! The v1.x nav was a GTK [`NavGroup`] list with lazy panel-import
//! lambdas; CB-1 retires that surface in favour of a pure-data
//! [`nav_model`] that the Iced sidebar consumes.

use std::fmt;

/// One of the top-level sidebar groups. **NAV-1 (the grouping redesign,
/// `docs/design/workbench-nav-grouping.md`)** rebuilds the nav as seven
/// scope→function sections — **Overview · This Node · Mesh · Fleet ·
/// Provisioning · Monitoring · System** (System = Config + Maintain +
/// Help) — and defers the desktop-settings panels to Cosmic Settings
/// (Q2). NAV-1.2 deleted the 17 desktop-settings panels (Cosmic Store/
/// Settings owns them) and relocated the 4 mesh-specific kept panels
/// (wallpaper + notifications → This Node; system update + sync status →
/// System), retiring the hidden Desktop group entirely. Order is
/// load-bearing — it drives the Ctrl+digit hotkey dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    // ── The seven mesh sections (locked order, Q15) ──
    Dashboard, // "Overview"
    ThisNode,
    Mesh,
    MeshProvisioning, // "MESH: PROVISIONING" — enrollment/federation (operator 2026-06-16)
    Fleet,            // labelled "OTHER NODES"
    Provisioning,     // labelled "MESH: VIRTUAL WORKLOADS"
    Monitoring,
    System, // Config + Maintain + Help
}

impl Group {
    /// Stable kebab-case slug used in deep-link URLs
    /// (`mde --focus <group>.<panel>`). NAV-1 is a clean break: the
    /// section slugs are new ids, no back-compat aliases.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::ThisNode => "node",
            Self::Mesh => "mesh",
            Self::MeshProvisioning => "mesh_provisioning",
            Self::Fleet => "fleet",
            Self::Provisioning => "provisioning",
            Self::Monitoring => "monitoring",
            Self::System => "system",
        }
    }

    /// Sentence-case label shown in the sidebar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Overview",
            Self::ThisNode => "This Node",
            Self::Mesh => "Mesh",
            Self::MeshProvisioning => "MESH: PROVISIONING",
            Self::Fleet => "OTHER NODES",
            Self::Provisioning => "MESH: VIRTUAL WORKLOADS",
            Self::Monitoring => "Monitoring",
            Self::System => "System",
        }
    }

    /// Stable display order (drives the Ctrl+1..8 hotkey dispatch).
    /// The eight visible sections map cleanly to Ctrl+1..8.
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::Dashboard,
            Self::ThisNode,
            Self::Mesh,
            Self::MeshProvisioning,
            Self::Fleet,
            Self::Provisioning,
            Self::Monitoring,
            Self::System,
        ]
    }

    /// The groups EXPOSED in the sidebar + the Ctrl+digit hotkeys —
    /// the seven mesh sections (NAV-1). NAV-1.2 retired the hidden
    /// Desktop group, so this is now identical to [`Self::all`].
    #[must_use]
    pub fn sidebar_groups() -> Vec<Self> {
        Self::all().to_vec()
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
        // NAV-1 (Q14) — a plain launch lands on Overview/Home (deep-links
        // still override). The Peers directory is one click away at the top
        // of the Mesh section.
        Self::Panel {
            group: Group::Dashboard,
            panel: "home",
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
        // ── Overview (default landing, Q14) ─────────────────────────
        NavEntry {
            group: Group::Dashboard,
            panels: vec![Panel::new("home", "Home")],
        },
        // ── This Node — the local box + its networking (Q7) ─────────
        NavEntry {
            group: Group::ThisNode,
            panels: vec![
                Panel::new("hardware", "Hardware"),
                Panel::new("mesh_services", "Mesh Services"),
                // Local networking stays under This Node (Q7).
                Panel::new("interfaces", "Interfaces"),
                Panel::new("wifi", "Wi-Fi"),
                Panel::new("vpn", "VPN"),
                Panel::new("firewall", "Firewall"),
                Panel::new("remote_desktop", "Remote Access"),
                // NAV-1.2 — mesh-specific desktop panels relocated here from
                // the retired Desktop group (wallpaper + notifications are
                // mesh-synced surfaces, not Cosmic-owned settings).
                Panel::new("wallpaper", "Wallpaper"),
                Panel::new("notifications", "Notifications"),
            ],
        },
        // ── Mesh — Peers front door + all mesh-wide services (Q5/Q9) ─
        NavEntry {
            group: Group::Mesh,
            panels: vec![
                // Peers directory first (Q9).
                Panel::new("peers", "Peers"),
                Panel::new("mesh_control", "Mesh Control"),
                Panel::new("mesh_storage", "Mesh Storage"),
                Panel::new("dns", "Mesh DNS"),
                Panel::new("routing", "Routing"),
                // Plain-language renames (Q13).
                Panel::new("mesh_bus", "Message Bus"),
                Panel::new("service_publishing", "Published Services"),
                Panel::new("network_hosts", "Discovered Hosts"),
                // Mesh-relevant peer/device services kept here (Q2 exception).
                Panel::new("connect", "Connected Devices"),
                Panel::new("music", "Music"),
            ],
        },
        // ── MESH: PROVISIONING — enrollment + federation (operator 2026-06-16):
        //    the join/registration/pending/federation flow lifted out of Mesh
        //    into its own top-level section.
        NavEntry {
            group: Group::MeshProvisioning,
            panels: vec![
                Panel::new("registration", "Registration"),
                Panel::new("mesh_join", "Mesh Join"),
                Panel::new("mesh_pending", "Mesh Pending"),
                Panel::new("mesh_federation", "Mesh Federation"),
            ],
        },
        // ── OTHER NODES (Group::Fleet) — roster + orchestration (Q6) ─
        NavEntry {
            group: Group::Fleet,
            panels: vec![
                Panel::new("fleet_rollup", "Fleet Rollup"),
                Panel::new("inventory", "Fleet Roster"),
                Panel::new("tags", "Tags"),
                // Orchestration (was the Controller plane, Q6).
                Panel::new("jobs", "Jobs"),
                Panel::new("playbooks", "Playbooks"),
                Panel::new("drift", "Remediation"),
            ],
        },
        // ── Provisioning — onboard/build artifacts + compute ────────
        NavEntry {
            group: Group::Provisioning,
            panels: vec![
                Panel::new("node_roles", "Node Roles"),
                Panel::new("profiles", "Install Profiles"),
                Panel::new("images", "Images"),
                Panel::new("mirrors", "Mirrors"),
                // Compute folded into Provisioning.
                Panel::new("instances", "Instances"),
            ],
        },
        // ── Monitoring — all observability across scopes (Q11) ──────
        NavEntry {
            group: Group::Monitoring,
            panels: vec![
                Panel::new("health_check", "Health"),
                Panel::new("mesh_logs", "Logs & Metrics"),
                Panel::new("fleet_logs", "Fleet Logs"),
                Panel::new("run_history", "Run History"),
                Panel::new("audit", "Audit"),
                Panel::new("mesh_history", "Mesh History"),
                Panel::new("resources", "Resources"),
                Panel::new("logs", "System Logs"),
            ],
        },
        // ── System — Config + Maintenance + Help (Q12 + follow-up) ──
        NavEntry {
            group: Group::System,
            panels: vec![
                // Config (unified Local / Fleet / Policy, Q12).
                Panel::new("config_apply", "Config"),
                Panel::new("revisions", "Fleet Config"),
                Panel::new("policy", "Policy"),
                Panel::new("settings", "Settings"),
                // Maintenance.
                Panel::new("hub", "Hub"),
                Panel::new("snapshots", "Snapshots"),
                Panel::new("repair", "Repair"),
                // Maintenance — NAV-1.2 relocated System Update + Panel Sync
                // Status here from the retired Desktop group (mesh-synced
                // maintenance surfaces).
                Panel::new("system_update", "System Update"),
                Panel::new("sync_status", "Panel Sync Status"),
                // Help.
                Panel::new("index", "Help Topics"),
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
    let slug = if slug == "network.mesh_ssh" || slug == "node.mesh_ssh" {
        // NAV-1 — Remote Access moved under This Node; the retired
        // mesh_ssh slug aliases to it (SVC-1/B1).
        "node.remote_desktop"
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
        // 8 visible sections (2026-06-16: MESH: PROVISIONING split out of Mesh).
        assert_eq!(nav.len(), 8);
        let order: Vec<Group> = nav.iter().map(|e| e.group).collect();
        assert_eq!(order, Group::all().to_vec());
    }

    #[test]
    fn kept_desktop_panels_relocated_into_visible_groups() {
        // NAV-1.2 — the 4 mesh-specific panels kept from the retired Desktop
        // group must be deep-link-reachable in their new homes.
        for (group_panel, want) in [
            ("node.wallpaper", (Group::ThisNode, "wallpaper")),
            ("node.notifications", (Group::ThisNode, "notifications")),
            ("system.system_update", (Group::System, "system_update")),
            ("system.sync_status", (Group::System, "sync_status")),
        ] {
            assert_eq!(
                view_from_focus_slug(group_panel),
                Some(View::Panel {
                    group: want.0,
                    panel: want.1,
                }),
                "{group_panel} must resolve after NAV-1.2 relocation",
            );
        }
    }

    #[test]
    fn deleted_desktop_settings_panels_no_longer_resolve() {
        // NAV-1.2 — the 17 deleted desktop-settings slugs (Cosmic owns them)
        // must NOT resolve under any group anymore.
        for slug in [
            "displays",
            "keyboard",
            "mouse",
            "power",
            "session",
            "sound",
            "printers",
            "removable",
            "themes",
            "fonts",
            "datetime",
            "install",
            "installed",
            "remove",
            "sources",
            "default_apps",
            "panel",
        ] {
            for group in ["node", "system", "mesh"] {
                assert_eq!(
                    view_from_focus_slug(&format!("{group}.{slug}")),
                    None,
                    "deleted desktop slug {slug} must not resolve under {group}",
                );
            }
        }
    }

    #[test]
    fn nav_sections_are_the_seven_locked_in_order() {
        // NAV-1 (Q15) — the visible sidebar is exactly the 7 sections in
        // the locked order; Desktop is hidden (Q2).
        assert_eq!(
            Group::sidebar_groups(),
            vec![
                Group::Dashboard,
                Group::ThisNode,
                Group::Mesh,
                Group::MeshProvisioning,
                Group::Fleet,
                Group::Provisioning,
                Group::Monitoring,
                Group::System,
            ]
        );
        assert_eq!(Group::Dashboard.label(), "Overview");
        assert_eq!(Group::Mesh.label(), "Mesh");
        // Peers is the first item under Mesh (Q9).
        assert!(view_from_focus_slug("mesh.peers").is_some());
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
    fn view_default_is_overview_home() {
        // NAV-1 (Q14) — a plain launch lands on Overview/Home.
        assert_eq!(
            View::default(),
            View::Panel {
                group: Group::Dashboard,
                panel: "home"
            }
        );
    }

    #[test]
    fn view_group_extractor_works_for_both_variants() {
        assert_eq!(View::Group(Group::Mesh).group(), Group::Mesh);
        assert_eq!(
            View::Panel {
                group: Group::ThisNode,
                panel: "remote_desktop"
            }
            .group(),
            Group::ThisNode
        );
    }

    #[test]
    fn view_panel_slug_extractor_distinguishes_variants() {
        assert_eq!(View::Group(Group::System).panel_slug(), None);
        assert_eq!(
            View::Panel {
                group: Group::System,
                panel: "index"
            }
            .panel_slug(),
            Some("index")
        );
    }

    #[test]
    fn focus_slug_resolves_group_only() {
        assert_eq!(view_from_focus_slug("mesh"), Some(View::Group(Group::Mesh)));
    }

    #[test]
    fn focus_slug_resolves_group_and_panel() {
        assert_eq!(
            view_from_focus_slug("node.remote_desktop"),
            Some(View::Panel {
                group: Group::ThisNode,
                panel: "remote_desktop"
            })
        );
        // The retired mesh_ssh slug aliases to Remote Access (SVC-1/B1).
        assert_eq!(
            view_from_focus_slug("node.mesh_ssh"),
            Some(View::Panel {
                group: Group::ThisNode,
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
        assert_eq!(view_from_focus_slug("node.not-a-panel"), None);
    }

    #[test]
    fn group_display_renders_label() {
        assert_eq!(format!("{}", Group::Mesh), "Mesh");
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

//! Sidebar group + panel model, ported from
//! `mackes/workbench/shell/sidebar_window.py::_build_nav`.
//!
//! The v1.x nav was a GTK [`NavGroup`] list with lazy panel-import
//! lambdas; CB-1 retires that surface in favour of a pure-data
//! [`nav_model`] that the Iced sidebar consumes.

use std::fmt;

/// One of the top-level sidebar groups. **CTRLSURF-6 (the control-surface
/// redesign, `docs/design/workbench-control-surface.md`)** folds the nav to the
/// seven scope-first sections — **Overview · This Node · Mesh · Fleet ·
/// Datacenter · Monitoring · System** — with plain-language, sub-grouped labels
/// (no SHOUTING). It dissolves the two prior shouting sections: `MESH:
/// PROVISIONING`'s enrollment/federation panels fold into **Mesh ▸ Join the
/// Mesh**, and `MESH: VIRTUAL WORKLOADS` (the old Provisioning section) splits
/// into **Fleet ▸ Node Templates** (node-build artifacts) + the new
/// **Datacenter** section (compute). Retired group slugs (`mesh_provisioning`,
/// `provisioning`) still resolve via [`view_from_focus_slug`] redirects so deep-
/// links + the Front Door search don't break (§7 reachability). Order is
/// load-bearing — it drives the Ctrl+digit hotkey dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    // ── The seven scope-first sections (locked order, CTRLSURF-6) ──
    Dashboard, // "Overview"
    ThisNode,
    Mesh,
    Fleet,
    Datacenter, // compute — VM spawner + the datacenter plane (highest blast radius)
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
            Self::Fleet => "fleet",
            Self::Datacenter => "datacenter",
            Self::Monitoring => "monitoring",
            Self::System => "system",
        }
    }

    /// Sentence-case label shown in the sidebar (CTRLSURF-6 — plain-language,
    /// no SHOUTING).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Overview",
            Self::ThisNode => "This Node",
            Self::Mesh => "Mesh",
            Self::Fleet => "Fleet",
            Self::Datacenter => "Datacenter",
            Self::Monitoring => "Monitoring",
            Self::System => "System",
        }
    }

    /// Stable display order (drives the Ctrl+1..7 hotkey dispatch).
    /// The seven visible sections map cleanly to Ctrl+1..7.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Dashboard,
            Self::ThisNode,
            Self::Mesh,
            Self::Fleet,
            Self::Datacenter,
            Self::Monitoring,
            Self::System,
        ]
    }

    /// The groups EXPOSED in the sidebar + the Ctrl+digit hotkeys —
    /// the seven scope-first sections (CTRLSURF-6).
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
///
/// CTRLSURF-6 — a panel may carry an optional `subgroup` header (e.g.
/// "Hardware & Desktop", "Join the Mesh"). Panels sharing a subgroup render
/// under one plain-language sub-heading in the universal sidebar; a `None`
/// subgroup renders flat (Overview / Datacenter / Monitoring).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Panel {
    slug: &'static str,
    label: &'static str,
    subgroup: Option<&'static str>,
}

impl Panel {
    /// A flat panel with no sub-group header.
    #[must_use]
    pub const fn new(slug: &'static str, label: &'static str) -> Self {
        Self {
            slug,
            label,
            subgroup: None,
        }
    }

    /// CTRLSURF-6 — a panel filed under a named sub-group header.
    #[must_use]
    pub const fn sub(slug: &'static str, label: &'static str, subgroup: &'static str) -> Self {
        Self {
            slug,
            label,
            subgroup: Some(subgroup),
        }
    }

    #[must_use]
    pub const fn slug(&self) -> &'static str {
        self.slug
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.label
    }

    /// CTRLSURF-6 — this panel's sub-group header, or `None` when it renders flat.
    #[must_use]
    pub const fn subgroup(&self) -> Option<&'static str> {
        self.subgroup
    }
}

/// One full sidebar row: a group plus its ordered leaf panels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEntry {
    pub group: Group,
    pub panels: Vec<Panel>,
}

impl NavEntry {
    /// CTRLSURF-6 — group this entry's panels into consecutive
    /// `(subgroup, panels)` runs, preserving order. A run of panels sharing a
    /// `Some(header)` renders under one sub-heading; a `None` run renders flat.
    /// The sidebar walks these to emit a sub-group header per `Some(_)` run.
    #[must_use]
    pub fn subgroups(&self) -> Vec<(Option<&'static str>, Vec<&Panel>)> {
        let mut out: Vec<(Option<&'static str>, Vec<&Panel>)> = Vec::new();
        for panel in &self.panels {
            match out.last_mut() {
                Some((header, panels)) if *header == panel.subgroup() => panels.push(panel),
                _ => out.push((panel.subgroup(), vec![panel])),
            }
        }
        out
    }
}

/// DATACENTER-25 — which surface is showing inside the **Datacenter** panel.
///
/// Six panels that used to be standalone sidebar entries (`compute`/Instances,
/// `snapshots`, `images`, `lighthouses`, `build_farm`) are now folded in as tabs
/// of the Datacenter panel; `Native` is Datacenter's own multi-lens surface
/// (Overview / Topology / Resources / Tofu / Audit). The fold is a NAV + VIEW
/// routing change: each folded surface keeps its own panel state + reducer +
/// subscription in `app.rs` and is selected by this tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DatacenterTab {
    /// Datacenter's own surface (the existing Overview/Topology/Resources/Tofu/
    /// Audit lenses + the prod/dev zone tabs). The default landing tab.
    #[default]
    Native,
    /// The Compute/Instances panel — local VMs/pods + the fleet QNM inventory
    /// (carries its embedded VM-create wizard).
    Instances,
    /// The Snapshots panel — capture/restore the live config.
    Snapshots,
    /// The Images panel — the bootable-image catalog.
    Images,
    /// The Lighthouses panel — the lighthouse ops cards + beacon beam.
    Lighthouses,
    /// The Build Farm panel — farm jobs queued/passed/failed + test tiers.
    BuildFarm,
}

impl DatacenterTab {
    /// The sidebar/CLI slug a folded surface used to own as a standalone panel.
    /// Returned for the folded tabs only — `Native` is Datacenter's own surface
    /// and has no folded-panel slug (it routes through the `datacenter` panel
    /// slug directly), so it yields `None`. Used to keep deep-link / focus-slug
    /// compatibility: a request to focus one of these retired slugs redirects to
    /// the Datacenter panel with this tab selected.
    #[must_use]
    pub const fn folded_slug(self) -> Option<&'static str> {
        match self {
            Self::Native => None,
            // Compute's panel slug was "instances".
            Self::Instances => Some("instances"),
            Self::Snapshots => Some("snapshots"),
            Self::Images => Some("images"),
            Self::Lighthouses => Some("lighthouses"),
            // Build Farm's panel slug was the kebab "build-farm".
            Self::BuildFarm => Some("build-farm"),
        }
    }

    /// The tab's label in the Datacenter fold bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Native => "Datacenter",
            Self::Instances => "Instances",
            Self::Snapshots => "Snapshots",
            Self::Images => "Images",
            Self::Lighthouses => "Lighthouses",
            Self::BuildFarm => "Build Farm",
        }
    }

    /// The fold bar's left-to-right tab order (Datacenter's own surface first,
    /// then the folded surfaces). Drives the fold-bar render in `app.rs`.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::Native,
            Self::Instances,
            Self::Snapshots,
            Self::Images,
            Self::Lighthouses,
            Self::BuildFarm,
        ]
    }

    /// Map a (now-retired) standalone panel slug to the Datacenter tab that
    /// absorbed it. `None` for any slug the Datacenter panel doesn't host.
    /// DATACENTER-25 keeps deep links / focus slugs working: the routing layer
    /// resolves these to `provisioning.datacenter` + this tab.
    #[must_use]
    pub fn from_folded_slug(slug: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|t| t.folded_slug() == Some(slug))
    }
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
        // ── Overview (default landing) ──────────────────────────────
        NavEntry {
            group: Group::Dashboard,
            panels: vec![Panel::new("home", "Home")],
        },
        // ── This Node — Hardware & Desktop / Network (CTRLSURF-6) ────
        NavEntry {
            group: Group::ThisNode,
            panels: vec![
                Panel::sub("hardware", "Hardware", "Hardware & Desktop"),
                // NAV-1.2 — mesh-specific desktop panels (wallpaper + notifications
                // are mesh-synced surfaces, not Cosmic-owned settings).
                Panel::sub("wallpaper", "Wallpaper", "Hardware & Desktop"),
                Panel::sub("notifications", "Notifications", "Hardware & Desktop"),
                // Network — this node's mesh + local networking.
                Panel::sub("mesh_services", "Mesh Connection", "Network"),
                Panel::sub("interfaces", "Interfaces", "Network"),
                Panel::sub("wifi", "Wi-Fi", "Network"),
                Panel::sub("vpn", "VPN", "Network"),
                Panel::sub("firewall", "Firewall", "Network"),
                Panel::sub("remote_desktop", "Remote Access", "Network"),
            ],
        },
        // ── Mesh — Fabric / Shared Resources / Services / Local Network /
        //    Join the Mesh (CTRLSURF-6 — the old MESH: PROVISIONING section folds
        //    in as the "Join the Mesh" sub-group). ─────────────────────────────
        NavEntry {
            group: Group::Mesh,
            panels: vec![
                // Fabric — the core mesh plumbing.
                Panel::sub("peers", "Peers", "Fabric"),
                Panel::sub("mesh_control", "Mesh Control", "Fabric"),
                Panel::sub("dns", "Mesh DNS", "Fabric"),
                Panel::sub("routing", "Routing", "Fabric"),
                // ROUTER-5 — per-node router/firewall (EdgeRouter/VyOS) controls.
                Panel::sub("router", "Routers", "Fabric"),
                // Shared Resources — what the mesh shares.
                Panel::sub("mesh_storage", "Shared Storage", "Shared Resources"),
                Panel::sub("music", "Music", "Shared Resources"),
                // Services — mesh-wide service surfaces.
                Panel::sub("mesh_bus", "Message Bus", "Services"),
                Panel::sub("service_publishing", "Published Services", "Services"),
                // COMPUTE/SVC-VIEW — unified view of all three service sources.
                Panel::sub("all_services", "Service Directory", "Services"),
                // CONNECT-6 — the unified exposure matrix (mesh-only vs public).
                Panel::sub("connectivity", "Connectivity", "Services"),
                Panel::sub("sip_gateway", "Voice Gateway", "Services"),
                // Local Network — LAN discovery + paired devices.
                Panel::sub("network_hosts", "Discovered Hosts", "Local Network"),
                Panel::sub("connect", "Connected Devices", "Local Network"),
                // Join the Mesh — enrollment + federation (was MESH: PROVISIONING).
                // Genesis first: founding a brand-new mesh precedes joining one.
                Panel::sub("genesis", "Create a Mesh", "Join the Mesh"),
                Panel::sub("registration", "Registration", "Join the Mesh"),
                Panel::sub("mesh_join", "Mesh Join", "Join the Mesh"),
                Panel::sub("mesh_pending", "Join Requests", "Join the Mesh"),
                Panel::sub("mesh_federation", "Linked Meshes", "Join the Mesh"),
            ],
        },
        // ── Fleet — Roster / Orchestration / Node Templates (CTRLSURF-6 — the
        //    old Provisioning section's node-build artifacts fold in as "Node
        //    Templates"). ─────────────────────────────────────────────────────
        NavEntry {
            group: Group::Fleet,
            panels: vec![
                // Roster.
                Panel::sub("fleet_rollup", "Fleet Rollup", "Roster"),
                Panel::sub("inventory", "Fleet Roster", "Roster"),
                Panel::sub("tags", "Tags", "Roster"),
                // Orchestration (was the Controller plane, Q6).
                Panel::sub("jobs", "Jobs", "Orchestration"),
                Panel::sub("playbooks", "Playbooks", "Orchestration"),
                Panel::sub("drift", "Remediation", "Orchestration"),
                // Node Templates — node-build artifacts (from the old Provisioning).
                Panel::sub("node_roles", "Node Roles", "Node Templates"),
                Panel::sub("profiles", "Install Profiles", "Node Templates"),
                Panel::sub("mirrors", "Mirrors", "Node Templates"),
            ],
        },
        // ── Datacenter — compute (CTRLSURF-6, highest blast radius, last).
        //    The VM spawner + the datacenter plane. DATACENTER-25 — the datacenter
        //    panel also hosts the folded Instances / Images / Build Farm /
        //    Lighthouses / Snapshots surfaces as fold-bar tabs; those slugs deep-
        //    link-redirect here. Flat (no sub-groups), per the taxonomy. ─────────
        NavEntry {
            group: Group::Datacenter,
            panels: vec![
                // XCP-4 — the VM Spawner (A-plane MDE-VMs over XCP-ng dom0s).
                Panel::new("provisioning", "New Virtual Machine"),
                // DATACENTER-8 — datacenter plane (DO/Xen resources via event/dc/*).
                Panel::new("datacenter", "Datacenter"),
            ],
        },
        // ── Monitoring — all observability across scopes, unified (Q11) ──
        NavEntry {
            group: Group::Monitoring,
            panels: vec![
                Panel::new("health_check", "Health Check"),
                Panel::new("mesh_logs", "Logs & Metrics"),
                Panel::new("fleet_logs", "Fleet Logs"),
                Panel::new("run_history", "Run History"),
                Panel::new("audit", "Audit"),
                Panel::new("mesh_history", "Mesh History"),
                Panel::new("resources", "Resource Usage"),
                Panel::new("logs", "System Logs"),
            ],
        },
        // ── System — Configuration / Maintenance / Preferences & Help ──
        NavEntry {
            group: Group::System,
            panels: vec![
                // Configuration (unified Local / Fleet / Policy, Q12).
                Panel::sub("config_apply", "Apply Configuration", "Configuration"),
                Panel::sub("revisions", "Fleet Config", "Configuration"),
                Panel::sub("policy", "Policy", "Configuration"),
                // Maintenance. DATACENTER-25 — Snapshots folded into the Datacenter
                // panel (deep link `system.snapshots` redirects there).
                Panel::sub("hub", "Hub", "Maintenance"),
                Panel::sub("repair", "Repair", "Maintenance"),
                // NAV-1.2 relocated System Update + Panel Sync Status here.
                Panel::sub("system_update", "System Update", "Maintenance"),
                Panel::sub("sync_status", "Panel Sync Status", "Maintenance"),
                // Preferences & Help.
                Panel::sub("settings", "Settings", "Preferences & Help"),
                Panel::sub("index", "Help Topics", "Preferences & Help"),
                Panel::sub("about", "About", "Preferences & Help"),
            ],
        },
    ]
}

/// CTRLSURF-6 — the universal sidebar's **Pinned** quick-links: the shortcuts the
/// Front Door's retired left rail used to carry (its Pinned pins + the
/// predominant DevOps / Data-Center Surfaces), folded into the one sidebar so
/// there is no second in-content rail. Each is a real `(label, group, panel)`
/// route that resolves — either a `nav_model` panel or a Datacenter fold-bar tab
/// (`build-farm`) — so there is no dead shortcut (§7). The richer
/// favorites/groups pin store is a later CTRLSURF phase.
#[must_use]
pub fn pinned_links() -> [(&'static str, Group, &'static str); 4] {
    [
        ("Peers", Group::Mesh, "peers"),
        ("Message Bus", Group::Mesh, "mesh_bus"),
        // DevOps / Data Center — the Front Door rail's predominant Surfaces. Build
        // Farm is a Datacenter fold-bar tab (the route redirects to the datacenter
        // panel + that tab).
        ("Build Farm", Group::Datacenter, "build-farm"),
        ("Datacenter", Group::Datacenter, "datacenter"),
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

/// CTRLSURF-6 — rewrite a retired top-level group slug to its new home. The
/// taxonomy dissolved the two SHOUTING sections: `mesh_provisioning` folded into
/// **Mesh ▸ Join the Mesh**, and `provisioning` split into **Fleet ▸ Node
/// Templates** (the node-build artifacts) + the new **Datacenter** section
/// (compute). The split is panel-aware: a `provisioning.<node-template>` slug
/// lands on Fleet, everything else on Datacenter. A slug that isn't a retired
/// group is returned unchanged. Pure — used only by [`view_from_focus_slug`].
fn redirect_retired_group_slug<'a>(group_slug: &'a str, panel_slug: Option<&str>) -> &'a str {
    match group_slug {
        "mesh_provisioning" => "mesh",
        "provisioning" => match panel_slug {
            // Node-build artifacts moved under Fleet ▸ Node Templates.
            Some("node_roles" | "profiles" | "mirrors") => "fleet",
            // VM spawner, the datacenter plane, the folded compute tabs, and the
            // bare section all land on the new Datacenter section.
            _ => "datacenter",
        },
        other => other,
    }
}

/// Resolve a deep-link slug into the matching [`View`]. Accepts
/// `<group>` or `<group>.<panel>` forms (e.g. `node` or
/// `node.remote_desktop`). Unknown slugs return `None`.
/// `network.mesh_ssh` (the retired B1 entry) aliases to the
/// Remote Access panel that absorbed it (SVC-1). CTRLSURF-6 — retired group
/// slugs (`mesh_provisioning`, `provisioning`) redirect to their new sections.
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
    // CTRLSURF-6 — the two SHOUTING sections were dissolved (their panels moved):
    // `mesh_provisioning.*` → Mesh, and `provisioning.*` → Fleet (node templates)
    // or Datacenter (compute). Rewrite the retired group slug to its new home so
    // old deep-links + the Front Door search keep resolving (§7 reachability).
    let group_slug = redirect_retired_group_slug(group_slug, panel_slug);
    // DATACENTER-25 — the five folded surfaces no longer have standalone nav
    // entries, but their deep-link slugs must keep working: redirect any
    // `<group>.<folded>` (e.g. `mesh.lighthouses`, `datacenter.instances`,
    // `system.snapshots`) to the Datacenter panel, where the surface now lives as
    // a tab. The tab itself is selected by the caller (`app.rs` reads
    // `DatacenterTab::from_folded_slug`). Resolving the View to `datacenter` here
    // means a dangling folded slug never renders nothing.
    if let Some(p) = panel_slug {
        if DatacenterTab::from_folded_slug(p).is_some() {
            return Some(View::Panel {
                group: Group::Datacenter,
                panel: "datacenter",
            });
        }
    }
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
        // CTRLSURF-6 — seven scope-first sections (MeshProvisioning + Provisioning
        // dissolved; Datacenter added).
        assert_eq!(nav.len(), 7);
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
        // CTRLSURF-6 (Q6) — the visible sidebar is exactly the 7 scope-first
        // sections in the locked, blast-radius order.
        assert_eq!(
            Group::sidebar_groups(),
            vec![
                Group::Dashboard,
                Group::ThisNode,
                Group::Mesh,
                Group::Fleet,
                Group::Datacenter,
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

    // ─────────────────────────────────────────────────────────
    // DATACENTER-25 — the six panels folded into Datacenter tabs.
    // Their standalone nav entries are gone, but the deep-link
    // slugs must redirect to the Datacenter panel (never render
    // nothing), and the tab mapping must round-trip.
    // ─────────────────────────────────────────────────────────

    #[test]
    fn folded_panels_have_no_standalone_nav_entry() {
        // The six folded slugs must NOT appear as their own sidebar panel under
        // any group anymore — they live as Datacenter tabs now.
        let nav = nav_model();
        for folded in [
            "instances",
            "snapshots",
            "images",
            "lighthouses",
            "build-farm",
        ] {
            for entry in &nav {
                assert!(
                    entry.panels.iter().all(|p| p.slug() != folded),
                    "folded slug {folded} must not have a standalone entry under {:?}",
                    entry.group
                );
            }
        }
        // Datacenter itself stays — it's the host.
        assert!(view_from_focus_slug("provisioning.datacenter").is_some());
    }

    #[test]
    fn folded_deep_link_slugs_redirect_to_the_datacenter_panel() {
        // Every retired standalone slug, addressed under the group it used to
        // live in, redirects to the Datacenter panel rather than dangling. The
        // old `provisioning.*` form keeps working via the CTRLSURF-6 group
        // redirect on top of the DATACENTER-25 fold.
        let dc = View::Panel {
            group: Group::Datacenter,
            panel: "datacenter",
        };
        for slug in [
            "mesh.lighthouses",
            "provisioning.instances",
            "provisioning.images",
            "provisioning.build-farm",
            "datacenter.instances",
            "datacenter.build-farm",
            "system.snapshots",
        ] {
            assert_eq!(
                view_from_focus_slug(slug),
                Some(dc),
                "{slug} must redirect to the Datacenter panel after the fold",
            );
        }
    }

    #[test]
    fn datacenter_tab_folded_slug_round_trips() {
        for tab in DatacenterTab::all() {
            match tab.folded_slug() {
                None => assert_eq!(tab, DatacenterTab::Native),
                Some(slug) => assert_eq!(DatacenterTab::from_folded_slug(slug), Some(tab)),
            }
        }
        assert_eq!(DatacenterTab::from_folded_slug("not-a-folded-panel"), None);
        // The default tab is Datacenter's own surface.
        assert_eq!(DatacenterTab::default(), DatacenterTab::Native);
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

    // ─────────────────────────────────────────────────────────
    // CTRLSURF-6 — scope-first taxonomy: sub-groups, plain-language
    // renames (no SHOUTING), retired-section redirects, the Pinned
    // quick-links, and the section-slug round-trip.
    // ─────────────────────────────────────────────────────────

    #[test]
    fn group_from_slug_round_trips_every_section() {
        // DoD — every one of the seven section slugs round-trips.
        for g in Group::all() {
            assert_eq!(
                Group::from_slug(g.slug()),
                Some(g),
                "section slug {} must round-trip",
                g.slug()
            );
        }
        let slugs: Vec<&str> = Group::all().iter().map(|g| g.slug()).collect();
        assert_eq!(
            slugs,
            vec![
                "dashboard",
                "node",
                "mesh",
                "fleet",
                "datacenter",
                "monitoring",
                "system",
            ]
        );
    }

    #[test]
    fn no_section_or_panel_or_subgroup_label_is_shouting() {
        // CTRLSURF-6 — no all-caps multi-word labels (the old "OTHER NODES" /
        // "MESH: PROVISIONING" / "MESH: VIRTUAL WORKLOADS"). A bare acronym with
        // no whitespace ("VPN") is legitimately upper-case and is NOT shouting.
        fn is_shouting(s: &str) -> bool {
            s.chars().any(char::is_whitespace)
                && s.chars().any(|c| c.is_alphabetic())
                && s == s.to_uppercase()
        }
        for g in Group::all() {
            assert!(
                !is_shouting(g.label()),
                "section label SHOUTS: {}",
                g.label()
            );
        }
        for entry in nav_model() {
            for panel in &entry.panels {
                assert!(
                    !is_shouting(panel.label()),
                    "panel label SHOUTS: {}",
                    panel.label()
                );
                if let Some(header) = panel.subgroup() {
                    assert!(!is_shouting(header), "sub-group header SHOUTS: {header}");
                }
            }
        }
    }

    #[test]
    fn grouped_sections_carry_subgroups_flat_sections_do_not() {
        for g in [Group::ThisNode, Group::Mesh, Group::Fleet, Group::System] {
            let entry = nav_model().into_iter().find(|e| e.group == g).unwrap();
            assert!(
                entry.panels.iter().all(|p| p.subgroup().is_some()),
                "{g:?} panels must all carry a sub-group header"
            );
        }
        for g in [Group::Dashboard, Group::Datacenter, Group::Monitoring] {
            let entry = nav_model().into_iter().find(|e| e.group == g).unwrap();
            assert!(
                entry.panels.iter().all(|p| p.subgroup().is_none()),
                "{g:?} renders flat (no sub-groups)"
            );
        }
        // The Mesh sub-group headers, in the locked order.
        let mesh = nav_model()
            .into_iter()
            .find(|e| e.group == Group::Mesh)
            .unwrap();
        let headers: Vec<&str> = mesh
            .subgroups()
            .into_iter()
            .filter_map(|(h, _)| h)
            .collect();
        assert_eq!(
            headers,
            vec![
                "Fabric",
                "Shared Resources",
                "Services",
                "Local Network",
                "Join the Mesh",
            ]
        );
    }

    #[test]
    fn subgroups_helper_collapses_consecutive_runs_in_order() {
        let entry = NavEntry {
            group: Group::System,
            panels: vec![
                Panel::sub("a", "A", "One"),
                Panel::sub("b", "B", "One"),
                Panel::sub("c", "C", "Two"),
                Panel::new("d", "D"),
            ],
        };
        let sg = entry.subgroups();
        assert_eq!(sg.len(), 3, "two headers + one flat run");
        assert_eq!(sg[0].0, Some("One"));
        assert_eq!(sg[0].1.len(), 2);
        assert_eq!(sg[1].0, Some("Two"));
        assert_eq!(sg[1].1.len(), 1);
        assert_eq!(sg[2].0, None);
        assert_eq!(sg[2].1.len(), 1);
    }

    #[test]
    fn retired_section_slugs_redirect_to_their_new_homes() {
        // CTRLSURF-6 — `mesh_provisioning` folds into Mesh; `provisioning` splits
        // into Fleet (node templates) + Datacenter (compute). Deep-links survive.
        assert_eq!(
            view_from_focus_slug("mesh_provisioning"),
            Some(View::Group(Group::Mesh))
        );
        for (slug, panel) in [
            ("mesh_provisioning.mesh_join", "mesh_join"),
            ("mesh_provisioning.genesis", "genesis"),
            ("mesh_provisioning.mesh_federation", "mesh_federation"),
        ] {
            assert_eq!(
                view_from_focus_slug(slug),
                Some(View::Panel {
                    group: Group::Mesh,
                    panel,
                }),
                "{slug} must fold into Mesh ▸ Join the Mesh",
            );
        }
        for (slug, panel) in [
            ("provisioning.node_roles", "node_roles"),
            ("provisioning.profiles", "profiles"),
            ("provisioning.mirrors", "mirrors"),
        ] {
            assert_eq!(
                view_from_focus_slug(slug),
                Some(View::Panel {
                    group: Group::Fleet,
                    panel,
                }),
                "{slug} must fold into Fleet ▸ Node Templates",
            );
        }
        assert_eq!(
            view_from_focus_slug("provisioning.provisioning"),
            Some(View::Panel {
                group: Group::Datacenter,
                panel: "provisioning",
            }),
            "the VM spawner moves to Datacenter",
        );
        assert_eq!(
            view_from_focus_slug("provisioning"),
            Some(View::Group(Group::Datacenter)),
            "the bare retired section lands on Datacenter",
        );
    }

    #[test]
    fn pinned_links_all_resolve_to_real_routes() {
        // §7 — no dead Pinned shortcut: each is a nav_model panel or a Datacenter
        // fold-bar tab (build-farm).
        for (label, group, slug) in pinned_links() {
            let resolves = resolve_panel_label(group, slug).is_some()
                || DatacenterTab::from_folded_slug(slug).is_some();
            assert!(
                resolves,
                "pinned link {label} ({group:?}/{slug}) must resolve to a real route"
            );
        }
    }

    #[test]
    fn plain_language_renames_applied() {
        // CTRLSURF-6 — the design's plain-language renames landed on the panels.
        for (group, slug, want) in [
            (Group::ThisNode, "mesh_services", "Mesh Connection"),
            (Group::Mesh, "mesh_storage", "Shared Storage"),
            (Group::Mesh, "sip_gateway", "Voice Gateway"),
            (Group::Mesh, "all_services", "Service Directory"),
            (Group::Mesh, "genesis", "Create a Mesh"),
            (Group::Mesh, "mesh_pending", "Join Requests"),
            (Group::Mesh, "mesh_federation", "Linked Meshes"),
            (Group::Datacenter, "provisioning", "New Virtual Machine"),
            (Group::System, "config_apply", "Apply Configuration"),
            (Group::Monitoring, "health_check", "Health Check"),
            (Group::Monitoring, "resources", "Resource Usage"),
        ] {
            assert_eq!(
                resolve_panel_label(group, slug),
                Some(want),
                "{slug} must be relabelled {want:?}"
            );
        }
        assert_eq!(Group::Fleet.label(), "Fleet");
    }
}

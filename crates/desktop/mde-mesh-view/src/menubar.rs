//! MENUBAR-ALL (Mesh View) — the shared **top menu bar** for the mesh map /
//! topology surface (design: `docs/design/menubar-all.md`).
//!
//! The Mesh View is a **viewer** (lock 13): it draws the live mesh and offers a
//! handful of genuine view controls, so its bar carries menus for the seams it
//! **really** has — no invented menus, no dead entries (§7). Hosted on the
//! shared [`mde_egui::menubar::MenuBar`] (MENUBAR-ALL-1) under the UPPERCASE
//! `MESH VIEW` title in the dock's Mesh accent ([`Style::ACCENT_MESH`]):
//!
//! * **View** — the viewer's real controls, comprehensive incl. the advanced
//!   ones (the governing principle):
//!   * **Refresh** — re-read the mesh snapshot immediately (handed back to the
//!     host surface as [`MeshOutcome::Refresh`], since the host owns the poll).
//!   * **Reduce Motion** — freeze the widget's animation (the existing
//!     [`MeshView::reduce_motion`](crate::MeshView::reduce_motion) seam), a
//!     live check item.
//!   * **Filter** — show/hide nodes by **role** (Lighthouse / Server /
//!     Workstation) and by **health** (Healthy / Degraded / Down), each a live
//!     check item over [`MeshViewOptions`]; the filter is a real, pure state
//!     projection ([`MeshViewOptions::filter`]) the host applies to the painted
//!     [`MeshState`].
//!   * **Show All Nodes** — clear every filter; **disabled** (never a no-op)
//!     when nothing is filtered (§7 context-gate).
//! * **Help** — a **Legend** window describing the map's real visual encoding
//!   (health colours, role sizes, the leader ring, the stale-version line,
//!   link activity).
//!
//! **Honestly omitted** (no landed seam, so no dead entry): a **Node** menu.
//! The widget is a live canvas with **no node-selection seam** — it paints only
//! the [`MeshState`] it is handed and returns a plain canvas [`Response`], with
//! no hit-testing, focus, or per-node action vocabulary — so a "Node ▸ Focus /
//! Details / …" menu would be entries with nothing behind them. Per lock 13 the
//! viewer gets menus for what it genuinely does; the Node menu appears only if a
//! selection seam is ever added.
//!
//! [`Response`]: mde_egui::egui::Response
//! [`Style::ACCENT_MESH`]: mde_egui::Style::ACCENT_MESH

use std::collections::HashSet;

use mde_egui::egui::{self, RichText};
use mde_egui::menubar::{Entry, Item, Menu, MenuBar as SharedMenuBar, MenuBarModel};
use mde_egui::{muted_note, ChipTone, StatusChip, Style};

use crate::state::{Health, MeshNode, MeshState, Role};

/// A solid status dot glyph (U+25CF) for the health-count chips.
const DOT: &str = "\u{25CF}";

// ───────────────────────────── the view options ─────────────────────────────

/// Which node **roles** are shown on the map. All visible by default; the View ▸
/// Filter menu toggles each one.
#[derive(Clone, Copy, Debug)]
struct RoleFilter {
    lighthouse: bool,
    server: bool,
    workstation: bool,
}

impl Default for RoleFilter {
    fn default() -> Self {
        Self {
            lighthouse: true,
            server: true,
            workstation: true,
        }
    }
}

/// Which node **health tiers** are shown on the map. All visible by default; the
/// View ▸ Filter menu toggles each one.
#[derive(Clone, Copy, Debug)]
struct HealthFilter {
    ok: bool,
    warn: bool,
    down: bool,
}

impl Default for HealthFilter {
    fn default() -> Self {
        Self {
            ok: true,
            warn: true,
            down: true,
        }
    }
}

/// The Mesh View's live, persistent view controls — the state the menu bar
/// drives across frames.
///
/// The host surface owns one of these, hands it to [`MeshMenuBar::ui`] (which
/// mutates it as the operator picks items), then applies it to the painted
/// [`MeshState`] via [`filter`](Self::filter) and to the widget via
/// [`reduce_motion`](Self::reduce_motion). Everything here is a **real** seam:
/// `reduce_motion` is the widget's existing animation freeze, and the role /
/// health filters are a pure projection of the state (§7 — no invented control).
#[derive(Clone, Debug, Default)]
pub struct MeshViewOptions {
    /// Freeze the widget's animation (the existing
    /// [`MeshView::reduce_motion`](crate::MeshView::reduce_motion) seam).
    pub reduce_motion: bool,
    /// Which roles are shown (View ▸ Filter ▸ Roles).
    roles: RoleFilter,
    /// Which health tiers are shown (View ▸ Filter ▸ Health).
    healths: HealthFilter,
}

impl MeshViewOptions {
    /// Whether nodes of `role` are currently shown.
    const fn role_visible(&self, role: Role) -> bool {
        match role {
            Role::Lighthouse => self.roles.lighthouse,
            Role::Server => self.roles.server,
            Role::Workstation => self.roles.workstation,
        }
    }

    /// Whether nodes of `health` are currently shown.
    const fn health_visible(&self, health: Health) -> bool {
        match health {
            Health::Ok => self.healths.ok,
            Health::Warn => self.healths.warn,
            Health::Down => self.healths.down,
        }
    }

    /// Toggle a role's visibility (View ▸ Filter ▸ Roles ▸ …).
    const fn toggle_role(&mut self, role: Role) {
        let slot = match role {
            Role::Lighthouse => &mut self.roles.lighthouse,
            Role::Server => &mut self.roles.server,
            Role::Workstation => &mut self.roles.workstation,
        };
        *slot = !*slot;
    }

    /// Toggle a health tier's visibility (View ▸ Filter ▸ Health ▸ …).
    const fn toggle_health(&mut self, health: Health) {
        let slot = match health {
            Health::Ok => &mut self.healths.ok,
            Health::Warn => &mut self.healths.warn,
            Health::Down => &mut self.healths.down,
        };
        *slot = !*slot;
    }

    /// Clear every filter — show all roles and all health tiers (View ▸ Show All
    /// Nodes).
    fn reset_filters(&mut self) {
        self.roles = RoleFilter::default();
        self.healths = HealthFilter::default();
    }

    /// Whether any node is currently filtered out — drives the "Show All Nodes"
    /// enable gate and the `FILTERED` status chip.
    #[must_use]
    const fn any_filter_active(&self) -> bool {
        let RoleFilter {
            lighthouse,
            server,
            workstation,
        } = self.roles;
        let HealthFilter { ok, warn, down } = self.healths;
        !(lighthouse && server && workstation && ok && warn && down)
    }

    /// Project a [`MeshState`] through the active filters: keep only nodes whose
    /// role **and** health are shown, and only links whose two endpoints both
    /// survive. Pure — the host paints (and overlays) exactly this result, so the
    /// canvas, the layout, and any overlay stay consistent. With no filter active
    /// it is a straight clone (the whole mesh).
    #[must_use]
    pub fn filter(&self, state: &MeshState) -> MeshState {
        if !self.any_filter_active() {
            return state.clone();
        }
        let nodes: Vec<MeshNode> = state
            .nodes
            .iter()
            .filter(|n| self.role_visible(n.role) && self.health_visible(n.health))
            .cloned()
            .collect();
        let kept: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let links = state
            .links
            .iter()
            .filter(|l| kept.contains(l.a.as_str()) && kept.contains(l.b.as_str()))
            .cloned()
            .collect();
        MeshState { nodes, links }
    }
}

// ─────────────────────────────── the actions ────────────────────────────────

/// One item the Mesh View menu bar can activate — the surface's action
/// vocabulary (the [`MenuBar::show`](SharedMenuBar::show) id). Each maps to a
/// real seam in [`apply`] (§7, no dead entries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MeshAction {
    /// Freeze / unfreeze the widget animation ([`MeshViewOptions::reduce_motion`]).
    ToggleReduceMotion,
    /// Show / hide a node role (View ▸ Filter ▸ Roles).
    ToggleRole(Role),
    /// Show / hide a node health tier (View ▸ Filter ▸ Health).
    ToggleHealth(Health),
    /// Clear every filter (View ▸ Show All Nodes).
    ResetFilters,
    /// Re-read the mesh snapshot now — handed back to the host as
    /// [`MeshOutcome::Refresh`] (the host owns the poll seam).
    Refresh,
    /// Open the legend window (Help ▸ Legend) — owned by the bar's own state.
    ShowLegend,
}

/// An out-of-band command the Mesh View menu bar hands back to its host surface —
/// an action the **host** (not the view options) owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshOutcome {
    /// The operator asked to re-read the mesh snapshot immediately (View ▸
    /// Refresh). The host forces its next poll.
    Refresh,
}

/// Dispatch an option-affecting [`MeshAction`] to its real seam on `options`,
/// returning any out-of-band [`MeshOutcome`] the host must handle. `ShowLegend`
/// is owned by the bar's window state (handled in [`MeshMenuBar::ui`]), so it is
/// a no-op here. Split out so the whole action → seam mapping is unit-tested
/// without egui.
fn apply(action: MeshAction, options: &mut MeshViewOptions) -> Option<MeshOutcome> {
    match action {
        MeshAction::ToggleReduceMotion => {
            options.reduce_motion = !options.reduce_motion;
            None
        }
        MeshAction::ToggleRole(role) => {
            options.toggle_role(role);
            None
        }
        MeshAction::ToggleHealth(health) => {
            options.toggle_health(health);
            None
        }
        MeshAction::ResetFilters => {
            options.reset_filters();
            None
        }
        MeshAction::Refresh => Some(MeshOutcome::Refresh),
        MeshAction::ShowLegend => None,
    }
}

// ──────────────────────────── the menu + status model ───────────────────────

/// A role's Filter-menu check item, reflecting its live visibility.
fn role_item(options: &MeshViewOptions, role: Role, label: &str) -> Entry<MeshAction> {
    Entry::Item(Item::new(MeshAction::ToggleRole(role), label).checked(options.role_visible(role)))
}

/// A health tier's Filter-menu check item, reflecting its live visibility.
fn health_item(options: &MeshViewOptions, health: Health, label: &str) -> Entry<MeshAction> {
    Entry::Item(
        Item::new(MeshAction::ToggleHealth(health), label).checked(options.health_visible(health)),
    )
}

/// The View ▸ Filter submenu: the role toggles then the health toggles, each a
/// live check item over the current [`MeshViewOptions`].
fn filter_entries(options: &MeshViewOptions) -> Vec<Entry<MeshAction>> {
    vec![
        Entry::Caption("Roles".to_owned()),
        role_item(options, Role::Lighthouse, "Lighthouses"),
        role_item(options, Role::Server, "Servers"),
        role_item(options, Role::Workstation, "Workstations"),
        Entry::Separator,
        Entry::Caption("Health".to_owned()),
        health_item(options, Health::Ok, "Healthy"),
        health_item(options, Health::Warn, "Degraded"),
        health_item(options, Health::Down, "Down"),
    ]
}

/// The View drop-down: Refresh, the Reduce-Motion toggle, the Filter submenu, and
/// the (context-gated) Show-All-Nodes clear.
fn view_menu(options: &MeshViewOptions) -> Menu<MeshAction> {
    Menu::new(
        "View",
        vec![
            Entry::Item(Item::new(MeshAction::Refresh, "Refresh")),
            Entry::Separator,
            Entry::Item(
                Item::new(MeshAction::ToggleReduceMotion, "Reduce Motion")
                    .checked(options.reduce_motion),
            ),
            Entry::Separator,
            Entry::Submenu {
                label: "Filter".to_owned(),
                mnemonic: None,
                entries: filter_entries(options),
            },
            Entry::Item(
                Item::new(MeshAction::ResetFilters, "Show All Nodes")
                    .enabled(options.any_filter_active()),
            ),
        ],
    )
}

/// The Help drop-down: the Legend window (Help ▸ Legend).
fn help_menu() -> Menu<MeshAction> {
    Menu::new(
        "Help",
        vec![Entry::Item(Item::new(
            MeshAction::ShowLegend,
            "Legend\u{2026}",
        ))],
    )
}

/// The full ordered menu tree for the Mesh View bar.
fn build_menus(options: &MeshViewOptions) -> Vec<Menu<MeshAction>> {
    vec![view_menu(options), help_menu()]
}

/// The live status cluster (lock 6): the total node count and the per-tier
/// health counts (up / degraded / down), plus a `FILTERED` chip while any node
/// is hidden — all real state read from the current [`MeshState`] + options (§7).
/// The counts are of the **whole** mesh (before filtering), so a hidden Down
/// node is still surfaced honestly.
fn build_status(state: &MeshState, options: &MeshViewOptions) -> Vec<StatusChip> {
    let mut up = 0usize;
    let mut degraded = 0usize;
    let mut down = 0usize;
    for node in &state.nodes {
        match node.health {
            Health::Ok => up += 1,
            Health::Warn => degraded += 1,
            Health::Down => down += 1,
        }
    }
    let mut chips = vec![StatusChip::new(
        format!("{} nodes", state.nodes.len()),
        ChipTone::Neutral,
    )];
    if up > 0 {
        chips.push(StatusChip::with_icon(DOT, format!("{up} up"), ChipTone::Ok));
    }
    if degraded > 0 {
        chips.push(StatusChip::with_icon(
            DOT,
            format!("{degraded} degraded"),
            ChipTone::Warn,
        ));
    }
    if down > 0 {
        chips.push(StatusChip::with_icon(
            DOT,
            format!("{down} down"),
            ChipTone::Danger,
        ));
    }
    if options.any_filter_active() {
        chips.push(StatusChip::new("FILTERED", ChipTone::Info));
    }
    chips
}

// ───────────────────────────────── the bar ──────────────────────────────────

/// The Mesh View top menu bar. Stateless but for the legend window's open flag —
/// every other bit of state lives in the [`MeshViewOptions`] it renders over.
#[derive(Default)]
pub struct MeshMenuBar {
    /// Whether the Help ▸ Legend reference window is open.
    legend_open: bool,
}

impl MeshMenuBar {
    /// A fresh bar (legend window closed).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Render the bar over the current `state` + `options`, apply the chosen item,
    /// and return any out-of-band [`MeshOutcome`] the host must handle (a manual
    /// Refresh — the host owns the poll seam).
    ///
    /// Builds the shared model from `state` + `options`, renders it, then applies
    /// the one activated item: the filter / motion toggles mutate `options`
    /// in place, Legend flips this bar's own window flag, and Refresh routes out.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &MeshState,
        options: &mut MeshViewOptions,
    ) -> Option<MeshOutcome> {
        let menus = build_menus(options);
        let status = build_status(state, options);
        let model = MenuBarModel {
            title: "Mesh View",
            accent: Style::ACCENT_MESH,
            menus: &menus,
            status: &status,
        };
        let picked = SharedMenuBar::show(ui, &model);

        let outcome = if let Some(action) = picked {
            if action == MeshAction::ShowLegend {
                self.legend_open = true;
            }
            apply(action, options)
        } else {
            None
        };

        self.legend_window(ui.ctx());
        outcome
    }

    /// The Help ▸ Legend reference window: what the map's colours, sizes, and
    /// markers mean — the viewer's real visual encoding, so the read-out is
    /// discoverable. Every colour comes from the shared status palette (§4).
    fn legend_window(&mut self, ctx: &egui::Context) {
        if !self.legend_open {
            return;
        }
        egui::Window::new("Mesh View Legend")
            .open(&mut self.legend_open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                legend_heading(ui, "Health");
                legend_swatch(ui, Style::OK, "Healthy — reachable and well");
                legend_swatch(ui, Style::WARN, "Degraded — reachable but impaired");
                legend_swatch(ui, Style::DANGER, "Down — unreachable");

                ui.add_space(Style::SP_S);
                legend_heading(ui, "Roles");
                muted_note(
                    ui,
                    "Lighthouse — the always-on mesh anchor (largest disc, centred).",
                );
                muted_note(ui, "Server — a headless service-tier box.");
                muted_note(ui, "Workstation — an interactive peer (smallest disc).");

                ui.add_space(Style::SP_S);
                legend_heading(ui, "Markers");
                muted_note(ui, "Accent ring — the elected leader (pulses live).");
                muted_note(ui, "Amber version line — a node the fleet has moved past.");
                muted_note(ui, "Travelling dots — live per-link activity.");
            });
    }
}

/// A dim section heading inside the legend window (the caption tier).
fn legend_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    );
}

/// One legend row: a tone-coloured status dot beside its description.
fn legend_swatch(ui: &mut egui::Ui, color: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(DOT).color(color));
        ui.label(text);
    });
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::{
        apply, build_menus, build_status, MeshAction, MeshMenuBar, MeshOutcome, MeshViewOptions,
    };
    use crate::state::{Health, MeshLink, MeshNode, MeshState, Role};
    use mde_egui::menubar::{Entry, Item};
    use mde_egui::{ChipTone, Style};

    /// A small mixed fixture: a leader lighthouse, a server, and two workstations
    /// (one degraded, one down), with a couple of links.
    fn fixture() -> MeshState {
        MeshState {
            nodes: vec![
                MeshNode::new("lh", "lighthouse", Role::Lighthouse, Health::Ok).leader(),
                MeshNode::new("srv", "server-01", Role::Server, Health::Ok),
                MeshNode::new("a", "peer-a", Role::Workstation, Health::Warn),
                MeshNode::new("b", "peer-b", Role::Workstation, Health::Down),
            ],
            links: vec![
                MeshLink::new("lh", "a", 0.5),
                MeshLink::new("lh", "b", 0.0),
                MeshLink::new("srv", "a", 0.2),
            ],
        }
    }

    // ── the model builds from a fixture (View + Help, real entries) ──────────

    #[test]
    fn build_menus_yields_view_and_help_only() {
        // Lock 13: a viewer gets menus for its real seams — View (its controls)
        // and Help (the legend); the Node menu is honestly omitted (no selection
        // seam), never shipped empty.
        let menus = build_menus(&MeshViewOptions::default());
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["View", "Help"], "no invented Node menu");
        for menu in &menus {
            assert!(!menu.entries.is_empty(), "{} shipped empty", menu.title);
        }
    }

    #[test]
    fn view_menu_carries_the_real_seams() {
        let menus = build_menus(&MeshViewOptions::default());
        let view = &menus[0];
        // Refresh, Reduce Motion (a check item), the Filter submenu, and the
        // Show-All-Nodes clear are all present.
        let has_submenu = view
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Submenu { label, .. } if label == "Filter"));
        assert!(has_submenu, "the Filter submenu is present");
        let items: Vec<&Item<MeshAction>> = view
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(i) => Some(i),
                _ => None,
            })
            .collect();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Refresh"));
        assert!(labels.contains(&"Reduce Motion"));
        assert!(labels.contains(&"Show All Nodes"));
    }

    // ── context-disable + live checkmarks (§7) ───────────────────────────────

    #[test]
    fn show_all_nodes_is_disabled_until_a_filter_is_active() {
        // Nothing filtered → the clear is a no-op → it renders disabled, never a
        // silent no-op (§7 context-gate).
        let mut options = MeshViewOptions::default();
        assert!(
            !clear_enabled(&options),
            "clear disabled with nothing hidden"
        );
        // Hide a role → the clear opens up.
        options.toggle_role(Role::Server);
        assert!(
            clear_enabled(&options),
            "clear enables once a role is hidden"
        );
    }

    /// The enabled state of the View ▸ Show All Nodes item for `options`.
    fn clear_enabled(options: &MeshViewOptions) -> bool {
        let menus = build_menus(options);
        menus[0]
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::Item(i) if i.label == "Show All Nodes" => Some(i.enabled),
                _ => None,
            })
            .expect("Show All Nodes item present")
    }

    #[test]
    fn reduce_motion_checkmark_reflects_the_option() {
        let mut options = MeshViewOptions::default();
        assert_eq!(reduce_motion_checked(&options), Some(false));
        options.reduce_motion = true;
        assert_eq!(reduce_motion_checked(&options), Some(true));
    }

    /// The check state of the View ▸ Reduce Motion item for `options`.
    fn reduce_motion_checked(options: &MeshViewOptions) -> Option<bool> {
        let menus = build_menus(options);
        menus[0]
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::Item(i) if i.label == "Reduce Motion" => Some(i.checked),
                _ => None,
            })
            .expect("Reduce Motion item present")
    }

    // ── item → action drives the real seam ───────────────────────────────────

    #[test]
    fn toggling_a_role_hides_and_shows_it() {
        let mut options = MeshViewOptions::default();
        assert!(options.role_visible(Role::Workstation));
        assert_eq!(
            apply(MeshAction::ToggleRole(Role::Workstation), &mut options),
            None
        );
        assert!(
            !options.role_visible(Role::Workstation),
            "the toggle drove the visibility seam"
        );
        // The filter now drops the two workstations (and their links).
        let filtered = options.filter(&fixture());
        assert_eq!(filtered.nodes.len(), 2, "two workstations hidden");
        assert!(filtered.nodes.iter().all(|n| n.role != Role::Workstation));
        assert!(
            filtered.links.iter().all(|l| l.a != "a" && l.b != "a"),
            "links to a hidden node are dropped too"
        );
    }

    #[test]
    fn toggling_a_health_tier_hides_it() {
        let mut options = MeshViewOptions::default();
        let _ = apply(MeshAction::ToggleHealth(Health::Down), &mut options);
        let filtered = options.filter(&fixture());
        assert!(
            filtered.nodes.iter().all(|n| n.health != Health::Down),
            "the Down node is hidden"
        );
        assert_eq!(filtered.nodes.len(), 3);
    }

    #[test]
    fn reset_filters_shows_everything_again() {
        let mut options = MeshViewOptions::default();
        options.toggle_role(Role::Server);
        options.toggle_health(Health::Warn);
        assert!(options.any_filter_active());
        assert_eq!(apply(MeshAction::ResetFilters, &mut options), None);
        assert!(!options.any_filter_active(), "clear restored the full view");
        let full = fixture();
        assert_eq!(options.filter(&full).nodes.len(), full.nodes.len());
    }

    #[test]
    fn reduce_motion_action_toggles_the_option() {
        let mut options = MeshViewOptions::default();
        assert!(!options.reduce_motion);
        let _ = apply(MeshAction::ToggleReduceMotion, &mut options);
        assert!(options.reduce_motion, "the item drove the animation freeze");
    }

    #[test]
    fn refresh_routes_out_to_the_host() {
        let mut options = MeshViewOptions::default();
        assert_eq!(
            apply(MeshAction::Refresh, &mut options),
            Some(MeshOutcome::Refresh),
            "Refresh is a host-owned command, handed back out"
        );
    }

    // ── the filter is a faithful projection ──────────────────────────────────

    #[test]
    fn filter_is_a_clone_when_nothing_is_hidden() {
        let options = MeshViewOptions::default();
        let full = fixture();
        let out = options.filter(&full);
        assert_eq!(out.nodes.len(), full.nodes.len());
        assert_eq!(out.links.len(), full.links.len());
    }

    // ── the status cluster reflects real live state ──────────────────────────

    #[test]
    fn status_counts_the_health_tiers_and_flags_a_filter() {
        let options = MeshViewOptions::default();
        let chips = build_status(&fixture(), &options);
        // "4 nodes", then 2 up / 1 degraded / 1 down — real counts of the mesh.
        assert_eq!(chips[0].text, "4 nodes");
        assert!(chips
            .iter()
            .any(|c| c.text == "2 up" && c.tone == ChipTone::Ok));
        assert!(chips
            .iter()
            .any(|c| c.text == "1 degraded" && c.tone == ChipTone::Warn));
        assert!(chips
            .iter()
            .any(|c| c.text == "1 down" && c.tone == ChipTone::Danger));
        // No filter yet → no FILTERED chip.
        assert!(chips.iter().all(|c| c.text != "FILTERED"));
        // Once a filter is active, the chip appears (counts stay whole-mesh).
        let mut filtered_opts = options;
        filtered_opts.toggle_health(Health::Down);
        let chips = build_status(&fixture(), &filtered_opts);
        assert_eq!(chips[0].text, "4 nodes", "counts remain of the whole mesh");
        assert!(chips
            .iter()
            .any(|c| c.text == "FILTERED" && c.tone == ChipTone::Info));
    }

    #[test]
    fn status_of_the_empty_mesh_is_just_a_zero_count() {
        let chips = build_status(&MeshState::default(), &MeshViewOptions::default());
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].text, "0 nodes");
    }

    // ── the bar renders headless ─────────────────────────────────────────────

    #[test]
    fn menu_bar_renders_headless_and_is_idle_without_a_click() {
        use mde_egui::egui::{self, pos2, vec2, Rect};
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let state = fixture();
        let mut options = MeshViewOptions::default();
        let mut bar = MeshMenuBar::new();
        let mut outcome = Some(MeshOutcome::Refresh);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                outcome = bar.ui(ui, &state, &mut options);
            });
        });
        assert!(outcome.is_none(), "nothing routes out without a click");
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mesh-view bar produced no draw primitives"
        );
    }
}

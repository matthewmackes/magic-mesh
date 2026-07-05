//! `Surface::About` → the **Device-Manager hardware inspector** (DEVMGR-2, design
//! `docs/design/about-device-manager.md`; locks #1/#2/#18/#19/#20/#24).
//!
//! The About surface body is a faithful Windows-Device-Manager **by-type** tree,
//! rendered entirely in `mde_egui::Style` dark tokens (§4): a compact brand title
//! strip (the brand shrinks off the body, #2/#24) with an ⓘ button that opens the
//! license / credits / mesh-identity dialog; a full menu bar + toolbar (#19); a
//! rich per-host header card (#20); and the all-collapsed category tree (#1/#18).
//!
//! It is a pure **consumer** of the §6 JSON contract in
//! [`mackes_mesh_types::device_inventory`] — the `hardware_probe` worker (DEVMGR-1)
//! publishes `<workgroup_root>/device-inventory/<host>.json` on every node, and
//! this surface reads THIS node's file (the local host) on a cadence + on a Scan.
//! It never enumerates hardware itself (that is the mesh-side worker) and depends
//! on no `mackesd` crate (§6): the wire is the file.
//!
//! **Honest degradation (§7):** before the first read the tree is a dim "reading…"
//! placeholder (no fabricated rows); a host with nothing published reads as an
//! honest "no inventory yet", never a faked tree; absent summary fields render as
//! an em-dash, never invented totals.
//!
//! **Scope now covers DEVMGR-2 + DEVMGR-3** — the by-type tree + header card +
//! local read (DEVMGR-2), plus the bottom **detail drawer** (General / Driver /
//! Details / Events / Resources, #9/#10), the **MDM problem-code parity** (#11 —
//! `DeviceStatus` → Windows Code 28/22/10 with the honest Linux reason beside it),
//! and the About chrome refactored onto the shared
//! [`mde_egui::menubar::MenuBar`] — About is the **14th / last MENUBAR-ALL
//! surface**, so its bespoke Action/View/Help bar is replaced by the shared
//! component (title · menus · a live status cluster), tinted with the dock's
//! **System** group accent ([`Style::ACCENT_SYSTEM`]). Each menu item is the mouse
//! twin of a real seam (Scan / view-mode / Expand-Collapse-all / the ⓘ dialog),
//! honestly disabled/omitted per §7.
//!
//! The host rail across peers (DEVMGR-4), the by-connection topology (DEVMGR-5) and
//! export (DEVMGR-6) are still later units; their seams here (the disabled view
//! modes) are left clean, not stubbed to a fake render.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, the About renderer, …); the shell body in \
              main.rs consumes them"
)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::device_inventory::{
    self, DeviceCategory, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
};
use mackes_mesh_types::peers::default_workgroup_root;
use mde_egui::egui::{self, Id, RichText};
use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
use mde_egui::{field, muted_note, status_dot, ChipTone, StatusChip, Style};
use mde_theme::brand;

use crate::about;
use crate::explorer::local_hostname;

/// Re-read THIS node's published inventory this often (design #8 — the ~30 s
/// auto-refresh; the producer republishes on its own cadence). A Scan forces an
/// immediate re-read regardless of this gate.
const REFRESH: Duration = Duration::from_secs(30);

/// How the device tree is organised (#3). DEVMGR-2 ships **By type**; By
/// connection (the PCI/USB topology, DEVMGR-5) and By node (the cross-fleet
/// flatten, DEVMGR-4) are later units. The faithful MDM View menu offers all
/// three, with the unbuilt modes **honestly disabled** (§7 — never stubbed to a
/// fake render).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(
    clippy::enum_variant_names,
    reason = "the By-prefix mirrors MDM's own 'By type / By connection / By node' \
              view-mode names — the shared prefix is the domain vocabulary, not noise"
)]
enum ViewMode {
    /// The classic by-category tree (Processors, Network adapters, …). Wired.
    #[default]
    ByType,
    /// The PCI/USB topology tree (DEVMGR-5) — not yet wired.
    ByConnection,
    /// The cross-fleet flatten of every host's devices (DEVMGR-4) — not yet wired.
    ByNode,
}

impl ViewMode {
    /// The three modes in View-menu / toolbar order.
    const ALL: [Self; 3] = [Self::ByType, Self::ByConnection, Self::ByNode];

    /// The menu / toolbar label.
    const fn label(self) -> &'static str {
        match self {
            Self::ByType => "By type",
            Self::ByConnection => "By connection",
            Self::ByNode => "By node",
        }
    }

    /// Whether this mode is wired in DEVMGR-2 (only [`ByType`](Self::ByType)). The
    /// others render as disabled controls until their unit lands (§7).
    const fn is_available(self) -> bool {
        matches!(self, Self::ByType)
    }
}

/// The bottom detail-drawer tab (#9/#10) — the full MDM property-tab set mapped to
/// Linux facts (`General` / `Driver` / `Details` sysfs+IDs / `Events` dmesg+udev /
/// `Resources` IRQ/IO/mem/DMA). Each tab renders only what the selected
/// [`DeviceRecord`] actually carries; an absent field reads as an honest empty tab
/// (§7), never a fabricated value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DrawerTab {
    /// Identity + the MDM device-status box (name/vendor/model + problem code).
    #[default]
    General,
    /// The bound kernel driver / module + its version.
    Driver,
    /// The sysfs path + `vendor:product` hardware IDs.
    Details,
    /// Recent dmesg / udev lines mentioning the device.
    Events,
    /// The IRQ / I/O-port / memory / DMA resources the device holds.
    Resources,
}

impl DrawerTab {
    /// The five tabs in MDM order.
    const ALL: [Self; 5] = [
        Self::General,
        Self::Driver,
        Self::Details,
        Self::Events,
        Self::Resources,
    ];

    /// The tab label.
    const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Driver => "Driver",
            Self::Details => "Details",
            Self::Events => "Events",
            Self::Resources => "Resources",
        }
    }
}

/// One activation from the shared [`MenuBar`] (MENUBAR-ALL) — each is the mouse
/// twin of a real DEVMGR seam, dispatched through [`DeviceManagerState::apply`]
/// (§6/§7, one seam per entry). `Copy` so the static menu tables can hold it and
/// the shared bar returns it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    /// Re-read the published inventory ([`DeviceManagerState::refresh`]).
    Scan,
    /// Switch the tree organisation ([`DeviceManagerState::view`]) — only the
    /// wired [`ViewMode::ByType`] is ever enabled (§7).
    View(ViewMode),
    /// Expand every published category ([`DeviceManagerState::expand_all`]).
    ExpandAll,
    /// Collapse every category ([`DeviceManagerState::collapse_all`]).
    CollapseAll,
    /// Open the ⓘ license / credits / mesh-identity dialog.
    About,
}

/// A stable handle to the selected device across inventory re-reads (#9). A
/// [`DeviceRecord`] carries no id, so the selection keys on its category + name +
/// sysfs path — the tuple a re-publish preserves for the same device. Resolved
/// against the live inventory each frame ([`find_device`]); a device that vanishes
/// closes the drawer rather than freezing a stale clone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceSelection {
    /// The owning category key.
    category: String,
    /// The device display name.
    name: String,
    /// The sysfs path, when the record carried one (the strong half of the key).
    sysfs_path: Option<String>,
}

impl DeviceSelection {
    /// The selection key for a device within a category.
    fn of(category: &str, dev: &DeviceRecord) -> Self {
        Self {
            category: category.to_string(),
            name: dev.name.clone(),
            sysfs_path: dev.sysfs_path.clone(),
        }
    }

    /// Whether this selection names `dev` within `category`.
    fn matches(&self, category: &str, dev: &DeviceRecord) -> bool {
        self.category == category && self.name == dev.name && self.sysfs_path == dev.sysfs_path
    }
}

/// Resolve a [`DeviceSelection`] against the live inventory, or `None` when the
/// device is no longer published (the drawer then closes — never a stale render).
fn find_device<'a>(inv: &'a DeviceInventory, sel: &DeviceSelection) -> Option<&'a DeviceRecord> {
    inv.categories
        .iter()
        .find(|c| c.key == sel.category)
        .and_then(|c| c.devices.iter().find(|d| sel.matches(&c.key, d)))
}

/// The Windows-MDM problem code a Linux [`DeviceStatus`] maps to (#11) — the
/// faithful *emulation* the design locks: no-driver → **Code 28**, disabled →
/// **Code 22**, degraded/error → **Code 10**. [`Ok`](DeviceStatus::Ok) and
/// [`Unknown`](DeviceStatus::Unknown) carry no code (an honest unknown is never
/// dressed as a fabricated Windows code — design "Risks"). Pure, so the mapping is
/// unit-tested without a render.
const fn problem_code(status: DeviceStatus) -> Option<u32> {
    match status {
        DeviceStatus::NoDriver => Some(28),
        DeviceStatus::Disabled => Some(22),
        DeviceStatus::Degraded => Some(10),
        DeviceStatus::Ok | DeviceStatus::Unknown => None,
    }
}

/// The About → Device-Manager surface state (DEVMGR-2). Holds the last-read local
/// inventory, the fixed-cadence read clock, the per-category expand set, the tree
/// organisation, and the ⓘ dialog latch. Drives no worker — a thin renderer over
/// the replicated snapshot.
pub(crate) struct DeviceManagerState {
    /// The replicated workgroup root the `device-inventory/` dir lives under
    /// (resolved once — the same substrate mount the chrome/grade fold reads).
    workgroup_root: PathBuf,
    /// This node's short hostname — the LOCAL inventory this surface reads
    /// (DEVMGR-2; the host rail across peers is DEVMGR-4).
    local_host: String,
    /// The last-read LOCAL inventory, or `None` when nothing is published yet.
    inventory: Option<DeviceInventory>,
    /// Whether the inventory has been read at least once — the honest pre-poll
    /// gate (§7): a dim "reading…" before the first read, distinct from a
    /// read-but-empty host.
    seen: bool,
    /// When the inventory was last read (drives the fixed [`REFRESH`] cadence).
    last_poll: Option<Instant>,
    /// The category keys currently expanded — empty by default (all-collapsed,
    /// #18). Expand-/Collapse-all fill/clear it; a header click toggles one.
    expanded: BTreeSet<String>,
    /// The active tree organisation (#3) — By type in DEVMGR-2.
    view: ViewMode,
    /// The device whose detail drawer is open (#9), or `None` when the drawer is
    /// closed. A stable [`DeviceSelection`] resolved against the live inventory
    /// each frame so a re-read never freezes a stale device.
    selected: Option<DeviceSelection>,
    /// Which detail-drawer tab is active (#10) — General on a fresh selection.
    active_tab: DrawerTab,
    /// The ⓘ dialog latch — license / credits / mesh-identity (#24).
    show_about: bool,
}

impl Default for DeviceManagerState {
    fn default() -> Self {
        Self {
            workgroup_root: default_workgroup_root(),
            local_host: local_hostname(),
            inventory: None,
            seen: false,
            last_poll: None,
            expanded: BTreeSet::new(),
            view: ViewMode::default(),
            selected: None,
            active_tab: DrawerTab::default(),
            show_about: false,
        }
    }
}

impl DeviceManagerState {
    /// Re-read THIS node's published inventory from the substrate now. An absent /
    /// half-replicated / unreadable file reads as an honest `None` (never a
    /// panic, via [`device_inventory::read_inventory`]); `seen` flips true so the
    /// surface leaves the pre-poll state. Both the Scan action and the cadence
    /// [`poll`](Self::poll) land here.
    fn refresh(&mut self) {
        self.inventory = device_inventory::read_inventory(&self.workgroup_root, &self.local_host);
        self.seen = true;
    }

    /// The poll seam (self-gating): re-read on the fixed cadence while the About
    /// surface is in view, then keep the repaint heartbeat alive so a fresh
    /// publish surfaces without operator input. Cheap — one local file read.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Expand every published category (Expand-all, #19).
    fn expand_all(&mut self) {
        if let Some(inv) = &self.inventory {
            self.expanded = inv.categories.iter().map(|c| c.key.clone()).collect();
        }
    }

    /// Collapse every category (Collapse-all, #19 — also the #18 default).
    fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    /// Toggle one category's expansion.
    fn toggle(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_string());
        }
    }

    /// Render the whole surface into `ui` (the body of `Surface::About`).
    ///
    /// Layout (#2/#9): the compact brand strip (#24), the shared MENUBAR-ALL bar,
    /// then the device tree filling the body — with the bottom **detail drawer**
    /// (DEVMGR-3) reserved *before* the body so the tree stays full-width above it.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        // The brand identity strip (#24) — kept beside the shared MenuBar so the
        // `◈ Magic-Mesh Quasar v<ver>` mark + the ⓘ button stay always-visible.
        self.title_strip(ui);
        // MENUBAR-ALL: the shared top bar replaces DEVMGR-2's bespoke Action/View/
        // Help chrome (About is the 14th / last surface onto the shared component).
        if let Some(action) = self.chrome_bar(ui) {
            self.apply(action);
        }
        ui.separator();
        ui.add_space(Style::SP_XS);

        // The bottom detail drawer (#9): reserved first so the tree/header body
        // below fills only the space it leaves (the tree stays full-width above).
        self.detail_drawer(ui);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                if !self.seen {
                    // Honest pre-poll (§7) — no fabricated tree before the first read.
                    pre_poll(ui, &self.local_host);
                } else if self.inventory.is_none() {
                    // Read, but this host has published nothing yet.
                    empty_host(ui, &self.local_host);
                } else {
                    // The header reads the inventory immutably, then the tree takes
                    // `&mut self` to mutate the expand/selection sets — so the header
                    // borrow is scoped closed (a plain `if let`) before `tree` runs.
                    if let Some(inv) = self.inventory.as_ref() {
                        header_card(ui, inv);
                    }
                    ui.add_space(Style::SP_S);
                    self.tree(ui);
                }
            });

        self.about_dialog(ui);
    }

    /// Dispatch a shared-[`MenuBar`] activation to its real seam (§6/§7 — every
    /// menu item is the mouse twin of an existing DEVMGR seam, never new behaviour).
    fn apply(&mut self, action: MenuAction) {
        match action {
            MenuAction::Scan => self.refresh(),
            MenuAction::View(mode) => self.view = mode,
            MenuAction::ExpandAll => self.expand_all(),
            MenuAction::CollapseAll => self.collapse_all(),
            MenuAction::About => self.show_about = true,
        }
    }

    /// The compact brand title strip (#2/#24): the `◈` mark + product name +
    /// version on the left, the ⓘ button on the right. Single-sourced from
    /// [`mde_theme::brand`] (§4/§6) so it can never drift from `--version`.
    fn title_strip(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("\u{25C8}") // ◈ — the mesh-node mark
                    .color(Style::ACCENT)
                    .size(Style::TITLE),
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(brand::logo::PRODUCT_NAME)
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.label(
                RichText::new(format!("v{}", brand::build::info().version))
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(
                        RichText::new("\u{24D8}") // ⓘ
                            .size(Style::BODY)
                            .color(Style::TEXT),
                    )
                    .on_hover_text("About \u{2014} license, credits, mesh identity")
                    .clicked()
                {
                    self.show_about = true;
                }
            });
        });
    }

    /// MENUBAR-ALL (About) — the **shared top bar** that replaces DEVMGR-2's bespoke
    /// Action/View/Help chrome. Renders the three menus (each item the mouse twin of
    /// a real seam) + a live status cluster over
    /// [`mde_egui::menubar::MenuBar`], tinted with the dock's **System** group accent
    /// ([`Style::ACCENT_SYSTEM`]), and returns the activated [`MenuAction`] (applied
    /// via [`Self::apply`]). About is the 14th / last surface onto the component.
    fn chrome_bar(&self, ui: &mut egui::Ui) -> Option<MenuAction> {
        let menus = self.build_menus();
        let status = self.status_chips(now_ms());
        let model = MenuBarModel {
            // The dock groups About/System under the categorical gold, so the bar
            // wears that accent (MENUBAR-ALL lock 2). The brand identity itself
            // stays in the strip above (design #24).
            title: "About",
            accent: Style::ACCENT_SYSTEM,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// Build the three menus from live state (#19 → MENUBAR-ALL): **Action** (Scan),
    /// **View** (the three modes as radio items — only [`ViewMode::ByType`] enabled,
    /// the others honestly disabled §7 — plus Expand/Collapse-all, gated on a loaded
    /// inventory), and **Help** (the ⓘ dialog). No invented File/Edit spine — About
    /// has no file/clipboard seam (§7).
    fn build_menus(&self) -> Vec<Menu<MenuAction>> {
        let action = Menu::new(
            "Action",
            vec![Entry::Item(Item::new(
                MenuAction::Scan,
                "Scan for hardware changes",
            ))],
        );

        let has_tree = self.inventory.is_some();
        let mut view_entries: Vec<Entry<MenuAction>> = ViewMode::ALL
            .iter()
            .map(|&mode| {
                Entry::Item(
                    Item::new(MenuAction::View(mode), mode.label())
                        .enabled(mode.is_available())
                        .checked(self.view == mode),
                )
            })
            .collect();
        view_entries.push(Entry::Separator);
        view_entries.push(Entry::Item(
            Item::new(MenuAction::ExpandAll, "Expand all").enabled(has_tree),
        ));
        view_entries.push(Entry::Item(
            Item::new(MenuAction::CollapseAll, "Collapse all").enabled(has_tree),
        ));
        let view = Menu::new("View", view_entries);

        let help = Menu::new(
            "Help",
            vec![Entry::Item(Item::new(
                MenuAction::About,
                "About Magic-Mesh",
            ))],
        );

        vec![action, view, help]
    }

    /// The live status cluster (MENUBAR-ALL lock 6): **host · N devices · M problems
    /// · scanned-time**, all off real state (§7). The host chip tints Info when an
    /// inventory has loaded, Warn once a read found nothing, Neutral before the first
    /// read; problems read Danger when any device is faulted, else an Ok "No
    /// problems"; the scanned chip humanizes the snapshot's freshness. Takes `now_ms`
    /// so the freshness read-out is unit-tested deterministically.
    fn status_chips(&self, now_ms: u64) -> Vec<StatusChip> {
        let host = self
            .inventory
            .as_ref()
            .map_or(self.local_host.as_str(), |inv| inv.host.as_str());
        let host_tone = if self.inventory.is_some() {
            ChipTone::Info
        } else if self.seen {
            ChipTone::Warn
        } else {
            ChipTone::Neutral
        };
        let mut chips = vec![StatusChip::with_icon(DOT, host.to_string(), host_tone)];

        if let Some(inv) = self.inventory.as_ref() {
            let devices = inv.device_count();
            chips.push(StatusChip::new(
                format!("{devices} {}", plural(devices, "device", "devices")),
                ChipTone::Neutral,
            ));
            let problems = inv.problem_count();
            if problems > 0 {
                chips.push(StatusChip::with_icon(
                    "\u{26A0}", // ⚠
                    format!("{problems} {}", plural(problems, "problem", "problems")),
                    ChipTone::Danger,
                ));
            } else {
                chips.push(StatusChip::new("No problems", ChipTone::Ok));
            }
            chips.push(StatusChip::new(
                scanned_label(now_ms, inv.published_at_ms),
                ChipTone::Neutral,
            ));
        }
        chips
    }

    /// The by-type device tree (#1/#18) in a vertical scroll: each category is a
    /// forced-state [`egui::CollapsingHeader`] whose open/closed is driven from
    /// [`Self::expanded`] (so Expand-/Collapse-all and per-header clicks all route
    /// through the one set), amber-tinted when it holds a problem device. A device
    /// row click opens/toggles the bottom detail drawer (DEVMGR-3).
    fn tree(&mut self, ui: &mut egui::Ui) {
        // The category a header click toggled + the device a row click selected this
        // frame — applied AFTER the read borrow ends so the immutable inventory read
        // and the mutable toggle/selection never alias. `selected` is cloned in so
        // the highlight reads current selection without borrowing `self.selected`.
        let mut toggled: Option<String> = None;
        let mut clicked: Option<DeviceSelection> = None;
        let selected = self.selected.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let Some(inv) = self.inventory.as_ref() else {
                    return;
                };
                for cat in &inv.categories {
                    let open = self.expanded.contains(cat.key.as_str());
                    let out = category_header(ui, cat, open, selected.as_ref());
                    if out.header_clicked {
                        toggled = Some(cat.key.clone());
                    }
                    if let Some(sel) = out.selected {
                        clicked = Some(sel);
                    }
                }
            });
        if let Some(key) = toggled {
            self.toggle(&key);
        }
        if let Some(sel) = clicked {
            // A click on the open device closes the drawer; a new device selects it
            // and resets to the General tab.
            if self.selected.as_ref() == Some(&sel) {
                self.selected = None;
            } else {
                self.selected = Some(sel);
                self.active_tab = DrawerTab::General;
            }
        }
    }

    /// The bottom detail drawer (#9/#10): when a device is selected, a resizable
    /// bottom panel with the five MDM tabs, populated from the live record. The
    /// selection is resolved against the current inventory each frame — a device
    /// that vanished on a re-read closes the drawer (never a stale clone, §7).
    fn detail_drawer(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = self.selected.clone() else {
            return;
        };
        // Clone the resolved record out so the panel body borrows neither `self` nor
        // the inventory — freeing the local tab/close state below to take `&mut`.
        let Some(dev) = self
            .inventory
            .as_ref()
            .and_then(|inv| find_device(inv, &sel))
            .cloned()
        else {
            self.selected = None;
            return;
        };

        let mut tab = self.active_tab;
        let mut close = false;
        egui::TopBottomPanel::bottom(ui.id().with("devmgr-detail-drawer"))
            .resizable(true)
            .min_height(Style::SP_XL * 4.0)
            .default_height(Style::SP_XL * 7.0)
            .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
            .show_inside(ui, |ui| {
                drawer_header(ui, &dev, &mut close);
                drawer_tabs(ui, &mut tab);
                ui.separator();
                ui.add_space(Style::SP_XS);
                drawer_body(ui, &dev, tab);
            });
        self.active_tab = tab;
        if close {
            self.selected = None;
        }
    }

    /// The ⓘ dialog (#24): the canonical identity screen (QBRAND-6 —
    /// [`about::about_panel`]) reused verbatim as the modal body (§6, one About
    /// renderer), with a top-bar close. Closes on the `×`, the backdrop, or Esc.
    fn about_dialog(&mut self, ui: &egui::Ui) {
        if !self.show_about {
            return;
        }
        let mut close = false;
        let modal = egui::Modal::new(Id::new("devmgr-about-dialog")).show(ui.ctx(), |ui| {
            ui.set_width(Style::SP_XL * 16.0);
            ui.set_max_height(Style::SP_XL * 18.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("About")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui
                        .button(RichText::new("\u{00D7}").size(Style::BODY)) // ×
                        .on_hover_text("Close")
                        .clicked();
                });
            });
            ui.separator();
            about::about_panel(ui);
        });
        if close || modal.should_close() {
            self.show_about = false;
        }
    }
}

/// The rich per-host header card (#20): the hostname, the device count + problem
/// badge, and the summary fields — over a [`Style`]-token group.
fn header_card(ui: &mut egui::Ui, inv: &DeviceInventory) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&inv.host)
                    .color(Style::TEXT_STRONG)
                    .size(Style::TITLE)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            muted_note(ui, format!("{} devices", inv.device_count()));
            let problems = inv.problem_count();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if problems > 0 {
                    ui.colored_label(
                        Style::DANGER,
                        RichText::new(format!("\u{26A0} {problems} with problems")) // ⚠
                            .size(Style::SMALL),
                    );
                } else {
                    ui.colored_label(
                        Style::OK,
                        RichText::new("All devices OK").size(Style::SMALL),
                    );
                }
            });
        });
        ui.add_space(Style::SP_XS);
        for (label, value) in header_lines(inv) {
            field(ui, label, &value, Style::TEXT);
        }
        // Honest hint when the deep-detail tools were missing at enumeration (#15)
        // — so a thin tree reads as "tool absent", not "hardware broken".
        if !inv.tools.lshw {
            ui.add_space(Style::SP_XS);
            muted_note(ui, "Install lshw for deep DMI / firmware details.");
        }
    });
}

/// The header-card field rows (#20), derived purely from [`HostSummary`] so the
/// mapping (uptime humanized, memory in GiB, honest em-dashes) is unit-tested
/// without a GPU. An absent optional renders as an em-dash, never a fabricated
/// value (§7). Note: the published summary carries no disk total (it is not in
/// the DEVMGR-1 schema), so disk is represented by the Disk-drives category in
/// the tree rather than a header figure — no invented capacity.
fn header_lines(inv: &DeviceInventory) -> Vec<(&'static str, String)> {
    let s = &inv.summary;
    vec![
        ("OS", s.os.clone().unwrap_or_else(dash)),
        ("Kernel", s.kernel.clone().unwrap_or_else(dash)),
        ("Uptime", s.uptime_secs.map_or_else(dash, humanize_uptime)),
        ("CPU", cpu_line(s)),
        ("Memory", s.mem_total_kb.map_or_else(dash, format_mem_kb)),
    ]
}

/// The CPU field: model + logical count, whichever the summary carries (an em-dash
/// when neither).
fn cpu_line(s: &HostSummary) -> String {
    match (&s.cpu_model, s.cpu_count) {
        (Some(m), Some(n)) => format!("{m} ({n} logical)"),
        (Some(m), None) => m.clone(),
        (None, Some(n)) => format!("{n} logical CPUs"),
        (None, None) => dash(),
    }
}

/// The em-dash placeholder for an absent field (never a blank / a fake value).
fn dash() -> String {
    "\u{2014}".to_string()
}

/// Humanize an uptime in seconds to `d h m` (dropping leading zero units), e.g.
/// `48_120` → `"13h 22m"`, `90_061` → `"1d 1h 1m"`.
fn humanize_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Format a `MemTotal` (in kB, as `/proc/meminfo` reports) to GiB with one
/// decimal — `16_072_192` → `"15.3 GiB"`.
#[allow(
    clippy::cast_precision_loss,
    reason = "RAM in kB is far below f32/f64's exact-integer range; a GiB display \
              rounded to one decimal loses no meaningful precision"
)]
fn format_mem_kb(kb: u64) -> String {
    let gib = kb as f64 / (1024.0 * 1024.0);
    format!("{gib:.1} GiB")
}

/// What a rendered [`category_header`] reports back for one frame: whether the
/// header was clicked (the caller toggles the expand set) and any device row the
/// operator selected (the caller opens the drawer).
struct CategoryOutcome {
    /// The collapsing header was clicked (toggle this category's expansion).
    header_clicked: bool,
    /// A device row was clicked (open/toggle its detail drawer).
    selected: Option<DeviceSelection>,
}

/// One category branch — a forced-state collapsing header (its open/closed driven
/// by the caller's expand set, #18). The header tints amber and carries a `⚠ N`
/// count when the category holds a problem device — a faithful MDM "attention on
/// this branch" cue.
fn category_header(
    ui: &mut egui::Ui,
    cat: &DeviceCategory,
    open: bool,
    selected: Option<&DeviceSelection>,
) -> CategoryOutcome {
    let problems = cat.problem_count();
    let tone = if problems > 0 {
        Style::WARN
    } else {
        Style::TEXT
    };
    let mut title = cat.label.clone();
    if problems > 0 {
        use std::fmt::Write as _;
        let _ = write!(title, "   \u{26A0} {problems}"); // ⚠ N
    }
    let mut clicked: Option<DeviceSelection> = None;
    let resp = egui::CollapsingHeader::new(RichText::new(title).color(tone).size(Style::BODY))
        .id_salt(("dm-cat", cat.key.as_str()))
        .open(Some(open))
        .show(ui, |ui| {
            for dev in &cat.devices {
                let is_sel = selected.is_some_and(|s| s.matches(&cat.key, dev));
                if device_row(ui, dev, is_sel) {
                    clicked = Some(DeviceSelection::of(&cat.key, dev));
                }
            }
        });
    CategoryOutcome {
        header_clicked: resp.header_response.clicked(),
        selected: clicked,
    }
}

/// One device row — a clickable selection row (DEVMGR-3): a status dot in the
/// device's [`status_tone`], the name (accent-tinted when selected), the MDM
/// **problem-code badge** for a faulted device (#11), and the honest Linux reason
/// from the schema, dimmed. Returns `true` when the row was clicked this frame (the
/// caller opens/toggles the bottom detail drawer).
fn device_row(ui: &mut egui::Ui, dev: &DeviceRecord, selected: bool) -> bool {
    let inner = ui
        .horizontal(|ui| {
            status_dot(ui, status_tone(dev.status));
            ui.add_space(Style::SP_XS);
            let name_tone = if selected { Style::ACCENT } else { Style::TEXT };
            ui.label(RichText::new(&dev.name).color(name_tone).size(Style::SMALL));
            if let Some(code) = problem_code(dev.status) {
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(format!("Code {code}"))
                        .color(status_tone(dev.status))
                        .size(Style::SMALL)
                        .strong(),
                );
            }
            if let Some(reason) = &dev.problem {
                ui.add_space(Style::SP_XS);
                muted_note(ui, format!("\u{2014} {reason}")); // — reason
            }
        })
        .response;
    // The row's labels don't sense clicks, so re-interact the whole strip as one
    // selection target (the MDM "click a device to inspect it" affordance).
    inner
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

// ─────────────────────────── the detail drawer (#9/#10) ─────────────────────

/// The filled status-dot glyph the status cluster reuses.
const DOT: &str = "\u{25CF}";

/// The drawer's title row (#9): the selected device's status dot + name, with a
/// `×` close button on the right.
fn drawer_header(ui: &mut egui::Ui, dev: &DeviceRecord, close: &mut bool) {
    ui.horizontal(|ui| {
        status_dot(ui, status_tone(dev.status));
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(&dev.name)
                .color(Style::TEXT_STRONG)
                .size(Style::BODY)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(RichText::new("\u{00D7}").size(Style::BODY)) // ×
                .on_hover_text("Close the device details")
                .clicked()
            {
                *close = true;
            }
        });
    });
}

/// The drawer's tab strip (#10): the five MDM tabs as selectable labels, updating
/// the caller's active-tab.
fn drawer_tabs(ui: &mut egui::Ui, tab: &mut DrawerTab) {
    ui.horizontal(|ui| {
        for t in DrawerTab::ALL {
            if ui.selectable_label(*tab == t, t.label()).clicked() {
                *tab = t;
            }
        }
    });
}

/// The drawer's body (#10): the active tab's fields, in a scroll so a long Events /
/// Resources list never blows the panel. Every tab renders only real record data,
/// with an honest empty state when a field is absent (§7).
fn drawer_body(ui: &mut egui::Ui, dev: &DeviceRecord, tab: DrawerTab) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match tab {
            DrawerTab::General => general_tab(ui, dev),
            DrawerTab::Driver => driver_tab(ui, dev),
            DrawerTab::Details => details_tab(ui, dev),
            DrawerTab::Events => events_tab(ui, dev),
            DrawerTab::Resources => resources_tab(ui, dev),
        });
}

/// The **General** tab (#10): identity (name / manufacturer / model) plus the MDM
/// **device-status box** (#11) — "This device is working properly." for a healthy
/// device, or the mapped problem code with the honest Linux reason beside it.
fn general_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    field(ui, "Device name", &dev.name, Style::TEXT);
    optional_field(ui, "Manufacturer", dev.vendor.as_deref());
    optional_field(ui, "Model", dev.model.as_deref());
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Device status")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    let (text, tone) = device_status_display(dev);
    ui.colored_label(tone, RichText::new(text).size(Style::SMALL));
}

/// The **Driver** tab (#10): the bound kernel driver / module + its version. An
/// honestly-empty tab when no driver is bound (which is exactly the no-driver
/// problem state, §7).
fn driver_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    if dev.driver.is_none() && dev.driver_version.is_none() {
        muted_note(ui, "No kernel driver is bound to this device.");
        return;
    }
    optional_field(ui, "Driver", dev.driver.as_deref());
    optional_field(ui, "Driver version", dev.driver_version.as_deref());
}

/// The **Details** tab (#10): the sysfs path + the `vendor:product` hardware IDs —
/// the Linux mapping of MDM's property IDs. Honestly empty when neither was read.
fn details_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    if dev.sysfs_path.is_none() && dev.ids.is_none() {
        muted_note(ui, "No sysfs path or hardware IDs were reported.");
        return;
    }
    optional_field(ui, "Hardware IDs", dev.ids.as_deref());
    optional_field(ui, "sysfs path", dev.sysfs_path.as_deref());
}

/// The **Events** tab (#10): the recent dmesg / udev lines mentioning this device,
/// in mono. Honestly empty when none were captured.
fn events_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    if dev.events.is_empty() {
        muted_note(ui, "No recent kernel or udev events for this device.");
        return;
    }
    for line in &dev.events {
        ui.label(
            RichText::new(line)
                .family(egui::FontFamily::Monospace)
                .color(Style::TEXT)
                .size(Style::SMALL),
        );
    }
}

/// The **Resources** tab (#10): the IRQ / I/O-port / memory-window / DMA resources
/// the device holds. Honestly empty when the enumerator resolved none.
fn resources_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    let r = &dev.resources;
    if r.is_empty() {
        muted_note(ui, "No IRQ, I/O, memory, or DMA resources were reported.");
        return;
    }
    if let Some(irq) = r.irq {
        field(ui, "IRQ", &irq.to_string(), Style::TEXT);
    }
    for (label, list) in [
        ("I/O ports", &r.io_ports),
        ("Memory range", &r.memory),
        ("DMA", &r.dma),
    ] {
        for value in list {
            field(ui, label, value, Style::TEXT);
        }
    }
}

/// A labelled field that renders an honest em-dash when the value is absent (§7),
/// so a drawer tab never leaves a blank or fabricates a value.
fn optional_field(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    match value {
        Some(v) => field(ui, label, v, Style::TEXT),
        None => field(ui, label, &dash(), Style::TEXT_DIM),
    }
}

/// The MDM device-status line for the General tab (#11): the problem code + the
/// honest Linux reason for a faulted device, "working properly" for a healthy one,
/// or an honest "could not be determined" for an unknown state — never a fabricated
/// Windows code. Returns the text + its [`Style`] tone. Pure, so the mapping is
/// unit-tested without a render.
fn device_status_display(dev: &DeviceRecord) -> (String, egui::Color32) {
    if let Some(code) = problem_code(dev.status) {
        let reason = dev
            .problem
            .as_deref()
            .unwrap_or("no additional detail reported");
        return (
            format!("Code {code} \u{2014} {reason}"),
            status_tone(dev.status),
        );
    }
    if dev.status == DeviceStatus::Ok {
        return ("This device is working properly.".to_string(), Style::OK);
    }
    // Unknown — an honest "could not be determined", never a fabricated code.
    let text = dev.problem.as_deref().map_or_else(
        || "Device status could not be determined.".to_string(),
        |r| format!("Device status could not be determined \u{2014} {r}"),
    );
    (text, Style::TEXT_DIM)
}

/// Singular / plural pick on a count (no faked pluralization elsewhere).
const fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// The "scanned N ago" freshness chip text (#8) from the snapshot's publish time.
/// A `0` publish time (the schema's honest "unknown") reads as an em-dash rather
/// than a fabricated age. Pure over `now_ms` so it is deterministically tested.
fn scanned_label(now_ms: u64, published_ms: u64) -> String {
    if published_ms == 0 {
        return "Scanned \u{2014}".to_string();
    }
    format!(
        "Scanned {}",
        humanize_ago(now_ms.saturating_sub(published_ms) / 1000)
    )
}

/// Humanize an elapsed span (in whole seconds) to a compact "N ago" — "just now"
/// under 5 s, then s / m / h / d rounded down.
fn humanize_ago(secs: u64) -> String {
    if secs < 5 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Wall-clock now in ms since the epoch (the status cluster's freshness read), or
/// `0` if the clock is before the epoch (an honest miss the chip renders as "—").
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The status-dot tone for a device state — the honest Linux state coloured, not
/// yet the MDM problem code (DEVMGR-3). Ok is green; a driverless device warns;
/// a degraded (error) device is danger; disabled / unknown are dim (not alarms).
const fn status_tone(status: DeviceStatus) -> egui::Color32 {
    match status {
        DeviceStatus::Ok => Style::OK,
        DeviceStatus::Degraded => Style::DANGER,
        DeviceStatus::NoDriver => Style::WARN,
        DeviceStatus::Disabled | DeviceStatus::Unknown => Style::TEXT_DIM,
    }
}

/// The honest pre-poll state (§7): a dim "?" over "reading…", drawn before the
/// first inventory read — never a fabricated tree.
fn pre_poll(ui: &mut egui::Ui, host: &str) {
    ui.add_space(Style::SP_L);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("?")
                .color(Style::TEXT_DIM)
                .size(Style::DISPLAY),
        );
        muted_note(
            ui,
            format!("Reading the device inventory for {host}\u{2026}"),
        );
    });
}

/// The read-but-empty state (§7): the inventory dir was read but this host has
/// published nothing yet — an honest note, distinct from the pre-poll dim.
fn empty_host(ui: &mut egui::Ui, host: &str) {
    ui.add_space(Style::SP_L);
    ui.vertical_centered(|ui| {
        muted_note(ui, format!("No device inventory published for {host} yet."));
        ui.add_space(Style::SP_XS);
        muted_note(
            ui,
            "The hardware probe republishes periodically \u{2014} or press Scan.",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::{
        cpu_line, device_status_display, format_mem_kb, header_lines, humanize_ago,
        humanize_uptime, problem_code, scanned_label, status_tone, DeviceManagerState,
        DeviceSelection, DrawerTab, MenuAction, ViewMode,
    };
    use mackes_mesh_types::device_inventory::{
        category, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
    };
    use mde_egui::menubar::{Entry, Menu};
    use mde_egui::{egui, ChipTone, Style};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// A state carrying a chosen inventory + seen flag, rooted at a non-existent
    /// path so `refresh` reads an honest `None` (no real substrate touched).
    fn state_with(inv: Option<DeviceInventory>, seen: bool) -> DeviceManagerState {
        DeviceManagerState {
            workgroup_root: PathBuf::from("/nonexistent-devmgr-test-root"),
            local_host: "laptop-mm".to_string(),
            inventory: inv,
            seen,
            last_poll: None,
            expanded: BTreeSet::new(),
            view: ViewMode::ByType,
            selected: None,
            active_tab: DrawerTab::General,
            show_about: false,
        }
    }

    /// Drive one headless frame of the surface (the same `Context::run` →
    /// tessellate path the DRM runner uses, minus the GPU) and return the drawn
    /// primitive count — proving it is a live render, not dead code.
    fn drive(state: &mut DeviceManagerState) -> usize {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1000.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    #[test]
    fn the_tree_renders_headless_from_a_fixture_inventory() {
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        // Expand so category bodies (the device rows) render too.
        s.expand_all();
        assert!(drive(&mut s) > 0, "the device tree drew nothing");
    }

    #[test]
    fn categories_default_all_collapsed_then_expand_and_collapse_all() {
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        // #18 — every category is collapsed on open.
        assert!(s.expanded.is_empty(), "all categories collapsed on open");
        s.expand_all();
        // The fixture publishes exactly the Display + System(PCI) categories.
        assert_eq!(s.expanded.len(), 2);
        assert!(s.expanded.contains(category::DISPLAY));
        assert!(s.expanded.contains(category::PCI_DEVICES));
        // Toggling one collapses just it; toggling again re-expands it.
        s.toggle(category::DISPLAY);
        assert!(!s.expanded.contains(category::DISPLAY));
        assert!(s.expanded.contains(category::PCI_DEVICES));
        s.toggle(category::DISPLAY);
        assert!(s.expanded.contains(category::DISPLAY));
        // Collapse-all clears everything back to the #18 default.
        s.collapse_all();
        assert!(s.expanded.is_empty());
    }

    #[test]
    fn header_card_fields_derive_from_the_summary() {
        let inv = DeviceInventory::fixture();
        let lines = header_lines(&inv);
        let get = |k: &str| {
            lines
                .iter()
                .find(|(l, _)| *l == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert!(get("OS").contains("Fedora"), "OS: {}", get("OS"));
        assert!(get("Kernel").contains("fc44"), "kernel: {}", get("Kernel"));
        assert!(get("CPU").contains("i7-8650U"), "cpu: {}", get("CPU"));
        assert!(
            get("CPU").contains('8'),
            "logical count folded in: {}",
            get("CPU")
        );
        assert!(get("Memory").ends_with("GiB"), "memory: {}", get("Memory"));
        assert_ne!(get("Uptime"), "\u{2014}", "uptime present in the fixture");
        // The header badge counts (#20) come straight off the schema helpers.
        assert_eq!(inv.device_count(), 2);
        assert_eq!(inv.problem_count(), 1);
    }

    #[test]
    fn absent_summary_fields_render_as_an_em_dash_not_a_fake_value() {
        // A shallow / non-PC host (#22) carries a bare summary — every field is an
        // honest em-dash, never a fabricated total (§7).
        let inv = DeviceInventory {
            host: "vyos-edge".to_string(),
            published_at_ms: 0,
            summary: HostSummary::default(),
            tools: mackes_mesh_types::device_inventory::ToolAvailability::default(),
            categories: vec![],
        };
        for (_, value) in header_lines(&inv) {
            assert_eq!(value, "\u{2014}", "an absent field must dash, not fake");
        }
    }

    #[test]
    fn honest_pre_poll_then_an_empty_host_read() {
        // Fresh: unseen + no inventory — the dim pre-poll (§7), no fake tree.
        let mut s = state_with(None, false);
        assert!(!s.seen);
        assert!(drive(&mut s) > 0, "the pre-poll state drew nothing");
        // A read of a missing inventory dir flips `seen` but yields an honest None.
        s.refresh();
        assert!(s.seen, "seen after the first read");
        assert!(
            s.inventory.is_none(),
            "a missing inventory reads as None, not a fabricated tree"
        );
        assert!(drive(&mut s) > 0, "the empty-host state drew nothing");
    }

    #[test]
    fn only_by_type_is_wired_the_other_modes_are_disabled_seams() {
        // #3 — the View menu offers all three modes, but DEVMGR-2 wires only By
        // type; the others are honest disabled seams (§7), not stubbed renders.
        assert_eq!(ViewMode::ALL.len(), 3);
        assert!(ViewMode::ByType.is_available());
        assert!(!ViewMode::ByConnection.is_available());
        assert!(!ViewMode::ByNode.is_available());
        assert_eq!(ViewMode::default(), ViewMode::ByType);
    }

    #[test]
    fn the_info_dialog_opens_and_renders_the_about_content() {
        // #24 — the ⓘ dialog reuses the canonical identity screen; opening it must
        // render (the modal + the About body) without panicking.
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        s.show_about = true;
        assert!(drive(&mut s) > 0, "the about dialog drew nothing");
    }

    #[test]
    fn uptime_and_memory_format_honestly() {
        assert_eq!(humanize_uptime(48_120), "13h 22m");
        assert_eq!(humanize_uptime(90_061), "1d 1h 1m");
        assert_eq!(humanize_uptime(59), "0m");
        let m = format_mem_kb(16_072_192);
        assert!(m.ends_with(" GiB"), "memory unit: {m}");
        assert!(m.starts_with("15."), "16 GB laptop reads ~15.3 GiB: {m}");
    }

    #[test]
    fn status_tones_separate_ok_from_problems() {
        // Ok is the success green; the problem states are visibly distinct tones,
        // and none of them read as Ok (so a problem never renders "healthy").
        assert_eq!(status_tone(DeviceStatus::Ok), Style::OK);
        for bad in [
            DeviceStatus::NoDriver,
            DeviceStatus::Degraded,
            DeviceStatus::Disabled,
            DeviceStatus::Unknown,
        ] {
            assert_ne!(status_tone(bad), Style::OK, "{bad:?} must not read as Ok");
        }
        // A hard error is the danger tone; a driverless device warns.
        assert_eq!(status_tone(DeviceStatus::Degraded), Style::DANGER);
        assert_eq!(status_tone(DeviceStatus::NoDriver), Style::WARN);
    }

    #[test]
    fn cpu_line_degrades_over_a_partial_summary() {
        let mut s = HostSummary {
            cpu_model: Some("Intel Xeon".to_string()),
            cpu_count: Some(16),
            ..Default::default()
        };
        assert!(cpu_line(&s).contains("Intel Xeon") && cpu_line(&s).contains("16"));
        s.cpu_count = None;
        assert_eq!(cpu_line(&s), "Intel Xeon");
        s.cpu_model = None;
        s.cpu_count = Some(4);
        assert!(cpu_line(&s).contains('4'));
        s.cpu_count = None;
        assert_eq!(cpu_line(&s), "\u{2014}");
    }

    // ── DEVMGR-3 helpers ─────────────────────────────────────────────────────

    /// The fixture's driverless PCI device (`NoDriver` + the honest Linux reason,
    /// with no driver / events / resources — the empty-tab cases).
    fn orphan() -> DeviceRecord {
        DeviceInventory::fixture()
            .categories
            .into_iter()
            .find(|c| c.key == category::PCI_DEVICES)
            .and_then(|c| c.devices.into_iter().next())
            .expect("the fixture publishes a PCI device")
    }

    /// The activation ids of a menu's items, in order.
    fn item_ids(menu: &Menu<MenuAction>) -> Vec<MenuAction> {
        menu.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(item) => Some(item.id),
                _ => None,
            })
            .collect()
    }

    // ── (b) MDM problem-code parity (#11) ────────────────────────────────────

    #[test]
    fn linux_state_maps_to_the_mdm_problem_code() {
        // The faithful emulation the design locks: no-driver→28, disabled→22,
        // degraded→10; Ok + Unknown carry no fabricated Windows code.
        assert_eq!(problem_code(DeviceStatus::NoDriver), Some(28));
        assert_eq!(problem_code(DeviceStatus::Disabled), Some(22));
        assert_eq!(problem_code(DeviceStatus::Degraded), Some(10));
        assert_eq!(problem_code(DeviceStatus::Ok), None);
        assert_eq!(problem_code(DeviceStatus::Unknown), None);
    }

    #[test]
    fn device_status_display_keeps_the_real_linux_reason_beside_the_code() {
        // A driverless device → Code 28 WITH the honest Linux reason, in the warn
        // tone — the code never stands alone (design "keep the emulation honest").
        let (text, tone) = device_status_display(&orphan());
        assert!(text.contains("Code 28"), "the MDM code: {text}");
        assert!(
            text.contains("no kernel driver bound"),
            "the honest Linux reason rides beside the code: {text}"
        );
        assert_eq!(tone, Style::WARN);
        // A healthy device reads the MDM "working properly", in the Ok tone.
        let gpu = DeviceRecord::new("Intel UHD Graphics", DeviceStatus::Ok);
        let (text, tone) = device_status_display(&gpu);
        assert_eq!(text, "This device is working properly.");
        assert_eq!(tone, Style::OK);
        // An unknown state stays honest — never dressed as a fabricated code.
        let mut unk = DeviceRecord::new("Unclaimed bus device", DeviceStatus::Unknown);
        let (text, _) = device_status_display(&unk);
        assert!(!text.contains("Code"), "unknown fabricates no code: {text}");
        unk.problem = Some("state could not be read".to_string());
        let (text, _) = device_status_display(&unk);
        assert!(
            text.contains("state could not be read"),
            "reason kept: {text}"
        );
    }

    // ── (a) the bottom detail drawer (#9/#10) ────────────────────────────────

    #[test]
    fn the_drawer_has_the_full_mdm_tab_set() {
        assert_eq!(DrawerTab::ALL.len(), 5);
        let labels: Vec<&str> = DrawerTab::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(
            labels,
            vec!["General", "Driver", "Details", "Events", "Resources"]
        );
        assert_eq!(DrawerTab::default(), DrawerTab::General);
    }

    #[test]
    fn the_five_tab_drawer_renders_for_a_selected_device() {
        // Selecting a device opens the drawer; each of the five MDM tabs renders
        // from the record without panicking (a live render, not dead code) — and
        // the orphan exercises the honest empty Driver / Events / Resources tabs.
        let inv = DeviceInventory::fixture();
        let orphan = orphan();
        for tab in DrawerTab::ALL {
            let mut s = state_with(Some(inv.clone()), true);
            s.selected = Some(DeviceSelection::of(category::PCI_DEVICES, &orphan));
            s.active_tab = tab;
            assert!(drive(&mut s) > 0, "the {} tab drew nothing", tab.label());
            assert!(
                s.selected.is_some(),
                "a live selection stays open on the {} tab",
                tab.label()
            );
        }
    }

    #[test]
    fn the_drawer_prunes_a_selection_that_vanished() {
        // A device no longer published closes the drawer rather than freezing a
        // stale clone (§7 — honest, never a fabricated render).
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        s.selected = Some(DeviceSelection {
            category: category::PCI_DEVICES.to_string(),
            name: "A device that was unplugged".to_string(),
            sysfs_path: None,
        });
        let _ = drive(&mut s);
        assert!(s.selected.is_none(), "an unresolvable selection is dropped");
    }

    #[test]
    fn a_device_selection_keys_on_category_name_and_sysfs() {
        let orphan = orphan();
        let sel = DeviceSelection::of(category::PCI_DEVICES, &orphan);
        // The same device in the same category matches (a re-publish preserves it).
        assert!(sel.matches(category::PCI_DEVICES, &orphan));
        // A different category, or a different device, does not.
        assert!(!sel.matches(category::DISPLAY, &orphan));
        let other = DeviceRecord::new("Something else entirely", DeviceStatus::Ok);
        assert!(!sel.matches(category::PCI_DEVICES, &other));
    }

    // ── (c) the shared MenuBar drives the real seams ─────────────────────────

    #[test]
    fn the_menu_bar_menus_drive_the_real_seams() {
        let s = state_with(Some(DeviceInventory::fixture()), true);
        let menus = s.build_menus();
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, vec!["Action", "View", "Help"]);
        // No invented File/Edit spine — About has no file/clipboard seam (§7).
        for banned in ["File", "Edit"] {
            assert!(!titles.contains(&banned), "{banned} shipped without a seam");
        }
        // Action → Scan.
        assert_eq!(item_ids(&menus[0]), vec![MenuAction::Scan]);
        // View → the three modes (By type live + checked, the others disabled
        // seams §7) + Expand/Collapse-all (enabled with a loaded inventory).
        let view = &menus[1];
        for entry in &view.entries {
            if let Entry::Item(item) = entry {
                if let MenuAction::View(mode) = item.id {
                    assert_eq!(
                        item.enabled,
                        mode.is_available(),
                        "{mode:?} enablement tracks whether it is wired"
                    );
                    assert_eq!(
                        item.checked,
                        Some(mode == ViewMode::ByType),
                        "the active mode is the checked one"
                    );
                }
            }
        }
        let enabled = |id| {
            view.entries
                .iter()
                .any(|e| matches!(e, Entry::Item(it) if it.id == id && it.enabled))
        };
        assert!(enabled(MenuAction::ExpandAll));
        assert!(enabled(MenuAction::CollapseAll));
        // Help → the ⓘ dialog.
        assert_eq!(item_ids(&menus[2]), vec![MenuAction::About]);
    }

    #[test]
    fn expand_collapse_disable_without_a_loaded_inventory() {
        // §7 — with nothing published there is nothing to expand, so the two are
        // honestly disabled (never a silent no-op).
        let s = state_with(None, true);
        let view = &s.build_menus()[1];
        for id in [MenuAction::ExpandAll, MenuAction::CollapseAll] {
            assert!(
                view.entries
                    .iter()
                    .any(|e| matches!(e, Entry::Item(it) if it.id == id && !it.enabled)),
                "{id:?} greys with no tree"
            );
        }
    }

    #[test]
    fn apply_dispatches_each_action_to_its_seam() {
        // Scan re-reads (seen flips true even off a fresh, empty state).
        let mut s = state_with(None, false);
        s.apply(MenuAction::Scan);
        assert!(s.seen, "Scan drove a read");
        // Expand / Collapse over the fixture.
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        s.apply(MenuAction::ExpandAll);
        assert_eq!(s.expanded.len(), 2, "Expand all filled the set");
        s.apply(MenuAction::CollapseAll);
        assert!(s.expanded.is_empty(), "Collapse all cleared it");
        // A view switch + the ⓘ dialog.
        s.apply(MenuAction::View(ViewMode::ByConnection));
        assert_eq!(s.view, ViewMode::ByConnection);
        assert!(!s.show_about);
        s.apply(MenuAction::About);
        assert!(s.show_about, "About opened the ⓘ dialog");
    }

    #[test]
    fn the_status_cluster_reflects_host_devices_and_problems() {
        let inv = DeviceInventory::fixture();
        let published = inv.published_at_ms;
        let s = state_with(Some(inv), true);
        let chips = s.status_chips(published + 90_000); // 90 s after publish
        assert!(
            chips
                .iter()
                .any(|c| c.text == "laptop-mm" && c.tone == ChipTone::Info),
            "the host chip reads Info once an inventory loads"
        );
        assert!(chips.iter().any(|c| c.text == "2 devices"), "device count");
        assert!(
            chips
                .iter()
                .any(|c| c.text.contains("1 problem") && c.tone == ChipTone::Danger),
            "the one faulted device reads a danger problem chip"
        );
        assert!(
            chips.iter().any(|c| c.text == "Scanned 1m ago"),
            "the freshness chip: {:?}",
            chips.iter().map(|c| c.text.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_status_cluster_is_honest_before_a_read_and_when_clean() {
        // Pre-read: only the host chip, neutral, no fabricated counts.
        let pre = state_with(None, false);
        let chips = pre.status_chips(0);
        assert_eq!(chips.len(), 1, "no counts before the first read");
        assert_eq!(chips[0].tone, ChipTone::Neutral);
        // A clean host reads an Ok "No problems".
        let mut inv = DeviceInventory::fixture();
        for cat in &mut inv.categories {
            for dev in &mut cat.devices {
                dev.status = DeviceStatus::Ok;
                dev.problem = None;
            }
        }
        let clean = state_with(Some(inv), true);
        let chips = clean.status_chips(0);
        assert!(
            chips
                .iter()
                .any(|c| c.text == "No problems" && c.tone == ChipTone::Ok),
            "a clean host reads an Ok 'No problems'"
        );
    }

    #[test]
    fn scanned_freshness_humanizes_and_stays_honest() {
        assert_eq!(humanize_ago(3), "just now");
        assert_eq!(humanize_ago(42), "42s ago");
        assert_eq!(humanize_ago(600), "10m ago");
        assert_eq!(humanize_ago(7_200), "2h ago");
        assert_eq!(humanize_ago(180_000), "2d ago");
        // A publish time of 0 (the schema's honest "unknown") fabricates no age.
        assert_eq!(scanned_label(1_000_000, 0), "Scanned \u{2014}");
        assert_eq!(scanned_label(1_000_000, 940_000), "Scanned 1m ago");
    }
}

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
//! **Scope now covers DEVMGR-2 + DEVMGR-3 + DEVMGR-4 + DEVMGR-5** — the by-type tree + header card +
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
//! **DEVMGR-4 adds the host rail + mesh-node switching** — a persistent left rail
//! lists every peer that has published a `device-inventory/<host>.json` (via
//! [`device_inventory::read_all`]) with a health/freshness status dot + the local
//! "you are here" marker; selecting one loads THAT host's snapshot instantly and a
//! live-refresh button re-reads it (#5/#7). The tree, header card, drawer + status
//! cluster all reflect the **selected** host; local is the default on open. A host
//! that has published nothing, or whose snapshot is old, reads an honest
//! **absent / stale** state — never fabricated data (§7).
//!
//! **DEVMGR-5 adds the By-connection view** — a second [`ViewMode`] that re-roots
//! the same devices under their **parent bus / controller** (host → PCI/USB bus
//! segment → device) instead of their function category, reconstructed from each
//! record's [`DeviceRecord::sysfs_path`] (the only topology signal the DEVMGR-1
//! schema carries — the PCI `DDDD:BB` bus segment / the USB bus number). A device
//! with no resolvable bus falls under the host root (never dropped); a host that
//! published no bus/parent data at all degrades to an honest flat list under the
//! root with a note (§7), never a fabricated hierarchy. A richer bridge/port tree
//! would need a real `parent` field in the DEVMGR-1 inventory.
//!
//! **DEVMGR-6 adds export / print** — the MDM `Action → generate a report`
//! equivalent: the Action menu grows **Export inventory (JSON)**, **Export report
//! (Markdown)**, and **Copy report to clipboard**, each rendering the **currently
//! selected host + active view mode** ([`render_json`] / [`render_report`]). JSON
//! serde-serializes the live [`DeviceInventory`] (round-trips the §6 contract); the
//! Markdown report mirrors the on-screen tree — the rich host header, then per
//! category (By type) or per bus / controller (By connection) device rows carrying
//! the same DEVMGR-3 problem-code + status text the drawer shows
//! ([`device_status_display`]). No native file-save dialog seam exists on this DRM
//! seat, so a write lands at a deterministic `$XDG_DATA_HOME`/`~/.local/share/mde/
//! device-inventory/<host>-<view>.<ext>` path ([`export_dir`]) and confirms on the
//! shared KIRON toast lane; a failed write raises an error toast, never a silent
//! no-op (§7). A host with nothing published exports an honest "no inventory yet"
//! report, never a fabricated tree.
//!
//! The cross-fleet By-node flatten is still a later unit; its seam here (the
//! disabled By-node mode) is left clean, not stubbed.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, the About renderer, …); the shell body in \
              main.rs consumes them"
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::device_inventory::{
    self, DeviceCategory, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
};
use mackes_mesh_types::peers::default_workgroup_root;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui::{self, Id, RichText};
use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
use mde_egui::{field, muted_note, status_dot, ChipTone, StatusChip, Style};
use mde_theme::brand;

use crate::about;
use crate::explorer::local_hostname;
use crate::toast_bridge::TOAST_TOPIC;

/// Re-read THIS node's published inventory this often (design #8 — the ~30 s
/// auto-refresh; the producer republishes on its own cadence). A Scan forces an
/// immediate re-read regardless of this gate.
const REFRESH: Duration = Duration::from_secs(30);

/// How long a published snapshot may age before the rail marks a host **stale**
/// (design §7 — honest dim/stale/offline). The producer republishes on its own
/// smoothed cadence (well under a minute); a host whose newest snapshot is older
/// than this has likely stopped publishing (offline / stalled), so the rail dims
/// it to amber rather than paint it as live. A snapshot with an unknown publish
/// time (`published_at_ms == 0`) is treated as stale for the same honesty reason.
const STALE_AFTER: Duration = Duration::from_secs(180);

/// How the device tree is organised (#3). DEVMGR-2 ships **By type** and
/// DEVMGR-5 ships **By connection** (the bus/controller topology); By node (the
/// cross-fleet flatten) is a later unit. The faithful MDM View menu offers all
/// three, with the unbuilt mode **honestly disabled** (§7 — never stubbed to a
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
    /// The bus/controller topology tree (DEVMGR-5) — devices re-rooted under
    /// their parent PCI/USB bus segment (host → bus → device), reconstructed from
    /// each record's sysfs path. Wired.
    ByConnection,
    /// The cross-fleet flatten of every host's devices (a later P2 unit) — not
    /// yet wired (DEVMGR-4 adds the host rail, not this flattened view).
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

    /// Whether this mode is wired: [`ByType`](Self::ByType) (DEVMGR-2) and
    /// [`ByConnection`](Self::ByConnection) (DEVMGR-5). [`ByNode`](Self::ByNode)
    /// renders as a disabled control until its unit lands (§7).
    const fn is_available(self) -> bool {
        matches!(self, Self::ByType | Self::ByConnection)
    }

    /// A filesystem-safe slug for the export filename (DEVMGR-6) — the view mode an
    /// export was taken under, so a By-type and a By-connection report of the same
    /// host never overwrite each other.
    const fn slug(self) -> &'static str {
        match self {
            Self::ByType => "by-type",
            Self::ByConnection => "by-connection",
            Self::ByNode => "by-node",
        }
    }
}

/// The machine / human export formats (DEVMGR-6, design #23) — **JSON** (serde of
/// the [`DeviceInventory`] §6 contract, round-tripping) and a human-readable
/// **Markdown** report mirroring the on-screen tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    /// Machine JSON — the serialized [`DeviceInventory`].
    Json,
    /// A human-readable Markdown report ([`render_report`]).
    Markdown,
}

impl ExportFormat {
    /// The file extension for this format.
    const fn ext(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
        }
    }

    /// The human noun for the confirmation toast ("inventory JSON" / "report").
    const fn noun(self) -> &'static str {
        match self {
            Self::Json => "inventory JSON",
            Self::Markdown => "Markdown report",
        }
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
    /// Export the selected host's inventory to a JSON file (DEVMGR-6).
    ExportJson,
    /// Export the selected host's inventory to a Markdown report file (DEVMGR-6).
    ExportMarkdown,
    /// Copy the Markdown report to the clipboard (DEVMGR-6).
    CopyReport,
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

/// How fresh a host's published inventory is (the rail's honest dim/stale/offline,
/// design §7) — derived purely from the snapshot's publish time vs now, so the
/// classification is unit-tested without a clock or a render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostFreshness {
    /// Published within [`STALE_AFTER`] — a live host, dot coloured by its health.
    Fresh,
    /// Published, but the newest snapshot is older than [`STALE_AFTER`] (or its
    /// publish time is unknown) — likely offline / no longer republishing. The
    /// rail dims it amber rather than paint a stale tree as live.
    Stale,
    /// Nothing published for this host at all — an honest offline "?", never a
    /// fabricated tree.
    Absent,
}

/// Classify a host's freshness from its snapshot publish time (`None` when the
/// host has published nothing) against `now_ms`. Pure, so the rail's dim/stale/
/// offline states are tested deterministically.
fn host_freshness(published_at_ms: Option<u64>, now_ms: u64) -> HostFreshness {
    match published_at_ms {
        None => HostFreshness::Absent,
        // An honest "unknown publish time" (the schema's `0`) can't be confirmed
        // fresh, so it reads stale rather than live.
        Some(0) => HostFreshness::Stale,
        Some(ts) => {
            let age_ms = now_ms.saturating_sub(ts);
            if u128::from(age_ms) <= STALE_AFTER.as_millis() {
                HostFreshness::Fresh
            } else {
                HostFreshness::Stale
            }
        }
    }
}

/// One row in the host rail (#5) — a peer that may or may not have published an
/// inventory. Carries just what the rail renders (name · freshness · the health
/// badge counts), decoupled from the full [`DeviceInventory`] so an absent host
/// (no published file) is still a first-class, selectable row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostEntry {
    /// The peer's short hostname (the rail key + the `read_inventory` stem).
    host: String,
    /// When this host last published (`None` = nothing published — an absent row).
    published_at_ms: Option<u64>,
    /// Device count in the newest snapshot (0 for an absent host).
    device_count: usize,
    /// Problem-status device count in the newest snapshot (0 for an absent host).
    problem_count: usize,
}

impl HostEntry {
    /// A rail row from a published inventory.
    fn from_inventory(inv: &DeviceInventory) -> Self {
        Self {
            host: inv.host.clone(),
            published_at_ms: Some(inv.published_at_ms),
            device_count: inv.device_count(),
            problem_count: inv.problem_count(),
        }
    }

    /// An absent rail row — a known host (e.g. the local "you are here" node) that
    /// has published nothing yet. Rendered as an honest offline "?" (§7).
    fn absent(host: &str) -> Self {
        Self {
            host: host.to_string(),
            published_at_ms: None,
            device_count: 0,
            problem_count: 0,
        }
    }

    /// This row's freshness against `now_ms`.
    fn freshness(&self, now_ms: u64) -> HostFreshness {
        host_freshness(self.published_at_ms, now_ms)
    }
}

/// Build the host rail from every published inventory (#5): a [`HostEntry`] per
/// host, with the local "you are here" node always present (even if it has
/// published nothing yet — an honest absent row you can still select) and **pinned
/// first**, the rest alphabetical. `all` arrives already sorted by host
/// ([`device_inventory::read_all`]), so the local-first key keeps a stable order.
/// Pure over its inputs, so the rail model is tested without a substrate.
fn build_rail(all: &[DeviceInventory], local: &str) -> Vec<HostEntry> {
    let mut entries: Vec<HostEntry> = all.iter().map(HostEntry::from_inventory).collect();
    if !entries.iter().any(|e| e.host == local) {
        entries.push(HostEntry::absent(local));
    }
    // Local pinned first (you-are-here), then the rest alphabetically.
    entries.sort_by(|a, b| {
        let a_local = a.host == local;
        let b_local = b.host == local;
        b_local.cmp(&a_local).then_with(|| a.host.cmp(&b.host))
    });
    entries
}

/// The About → Device-Manager surface state (DEVMGR-2..4). Holds the host rail
/// across every peer (DEVMGR-4), the selected host + its last-read inventory, the
/// fixed-cadence read clock, the per-category expand set, the tree organisation,
/// the open device drawer, and the ⓘ dialog latch. Drives no worker — a thin
/// renderer over the replicated snapshots.
pub(crate) struct DeviceManagerState {
    /// The replicated workgroup root the `device-inventory/` dir lives under
    /// (resolved once — the same substrate mount the chrome/grade fold reads).
    workgroup_root: PathBuf,
    /// This node's short hostname — the "you are here" rail anchor + the default
    /// selection on open (DEVMGR-4). Always present in the rail even if it has
    /// published nothing.
    local_host: String,
    /// The host currently being inspected (DEVMGR-4) — `local_host` on open, then
    /// whichever rail row the operator selects. The tree / header card / drawer /
    /// status cluster all reflect THIS host.
    selected_host: String,
    /// The host rail (#5) — one [`HostEntry`] per published peer + the local node,
    /// rebuilt from [`device_inventory::read_all`] on every read. Local is pinned
    /// first.
    hosts: Vec<HostEntry>,
    /// The last-read inventory for [`Self::selected_host`], or `None` when that
    /// host has published nothing (an honest absent read, never a fabricated tree).
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
        let local_host = local_hostname();
        Self {
            workgroup_root: default_workgroup_root(),
            selected_host: local_host.clone(),
            local_host,
            hosts: Vec::new(),
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
    /// Re-read the substrate now — the host rail (every peer's freshness + health)
    /// and the **selected** host's inventory in one [`device_inventory::read_all`]
    /// (#5/#7). An absent / half-replicated / unreadable file reads as an honest
    /// `None` (never a panic); `seen` flips true so the surface leaves the pre-poll
    /// state. The Scan action, the rail's live-refresh, host switching, and the
    /// cadence [`poll`](Self::poll) all land here.
    fn refresh(&mut self) {
        // One dir read serves both the rail (every peer's freshness/health) and the
        // selected host's tree (found in the same set — no second file read).
        let all = device_inventory::read_all(&self.workgroup_root);
        self.hosts = build_rail(&all, &self.local_host);
        self.inventory = all.into_iter().find(|inv| inv.host == self.selected_host);
        self.seen = true;
    }

    /// Switch the inspected host to `host` (a rail click, #5): the device drawer +
    /// active tab reset (a selection is per-host), then an immediate re-read loads
    /// the new host's snapshot instantly (the #7 hybrid — the published file, no
    /// wait). The category expand-set is stable across the switch (keyed on the
    /// shared taxonomy keys), so the operator's open categories persist.
    fn select_host(&mut self, host: String) {
        self.selected_host = host;
        self.selected = None;
        self.active_tab = DrawerTab::default();
        self.refresh();
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

    /// Expand every collapsible branch (Expand-all, #19) — every published
    /// category in By-type, or every bus / controller branch in By-connection
    /// (DEVMGR-5), so the one control fills whichever tree is showing.
    fn expand_all(&mut self) {
        if let Some(inv) = &self.inventory {
            self.expanded = match self.view {
                ViewMode::ByConnection => build_connection_tree(inv).bus_keys(),
                _ => inv.categories.iter().map(|c| c.key.clone()).collect(),
            };
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

    /// The persistent left **host rail** (#5): every peer that has published an
    /// inventory (plus the local "you are here" node) as a selectable row with a
    /// freshness/health status dot; local pinned first, marked with a ⌂ home glyph.
    /// A header carries a live-refresh button (#7 — re-read the selected host from
    /// the mesh). Selecting a row switches the inspected host ([`Self::select_host`]).
    fn rail(&mut self, ui: &mut egui::Ui) {
        let now = now_ms();
        let selected = self.selected_host.clone();
        let local = self.local_host.clone();
        let mut clicked: Option<String> = None;
        let mut refresh_clicked = false;
        egui::SidePanel::left(ui.id().with("devmgr-host-rail"))
            .resizable(true)
            .default_width(Style::SP_XL * 5.0)
            .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Mesh nodes")
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(
                                RichText::new("\u{21BB}") // ↻ — live-refresh this host
                                    .size(Style::SMALL)
                                    .color(Style::TEXT),
                            )
                            .on_hover_text("Refresh this host's inventory from the mesh")
                            .clicked()
                        {
                            refresh_clicked = true;
                        }
                    });
                });
                ui.separator();
                ui.add_space(Style::SP_XS);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.hosts.is_empty() {
                            muted_note(ui, "No nodes have published an inventory yet.");
                        }
                        for entry in &self.hosts {
                            let is_sel = entry.host == selected;
                            let is_local = entry.host == local;
                            if host_row(ui, entry, is_sel, is_local, now) {
                                clicked = Some(entry.host.clone());
                            }
                        }
                    });
            });
        if refresh_clicked {
            self.refresh();
        }
        if let Some(host) = clicked {
            if host != self.selected_host {
                self.select_host(host);
            }
        }
    }

    /// Render the whole surface into `ui` (the body of `Surface::About`).
    ///
    /// Layout (#2/#5/#9): the compact brand strip (#24), the shared MENUBAR-ALL
    /// bar, then **rail │ tree │ (bottom drawer)** — the persistent left host rail
    /// (DEVMGR-4) reserved first so it spans full height, then the bottom **detail
    /// drawer** (DEVMGR-3), then the tree + header card fill the remainder.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        // The brand identity strip (#24) — kept beside the shared MenuBar so the
        // `◈ Magic-Mesh Quasar v<ver>` mark + the ⓘ button stay always-visible.
        self.title_strip(ui);
        // MENUBAR-ALL: the shared top bar replaces DEVMGR-2's bespoke Action/View/
        // Help chrome (About is the 14th / last surface onto the shared component).
        if let Some(action) = self.chrome_bar(ui) {
            self.dispatch(action, ui.ctx());
        }
        ui.separator();
        ui.add_space(Style::SP_XS);

        // The persistent left host rail (#5): reserved first so it spans the full
        // body height (rail │ tree │ drawer). Switching hosts here re-reads below.
        self.rail(ui);

        // The bottom detail drawer (#9): reserved next so the tree/header body
        // below fills only the space it leaves (the tree stays full-width above).
        self.detail_drawer(ui);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                if !self.seen {
                    // Honest pre-poll (§7) — no fabricated tree before the first read.
                    pre_poll(ui, &self.selected_host);
                } else if self.inventory.is_none() {
                    // Read, but the selected host has published nothing yet.
                    empty_host(ui, &self.selected_host);
                } else {
                    // The header reads the inventory immutably, then the tree takes
                    // `&mut self` to mutate the expand/selection sets — so the header
                    // borrow is scoped closed (a plain `if let`) before `tree` runs.
                    if let Some(inv) = self.inventory.as_ref() {
                        header_card(ui, inv);
                    }
                    ui.add_space(Style::SP_S);
                    // Only the tree grouping/nesting changes between view modes
                    // (#3) — the header card, drawer + rows are shared.
                    match self.view {
                        ViewMode::ByConnection => self.connection_tree(ui),
                        _ => self.tree(ui),
                    }
                }
            });

        self.about_dialog(ui);
    }

    /// Dispatch a shared-[`MenuBar`] activation to its real seam (§6/§7 — every
    /// menu item is the mouse twin of an existing DEVMGR seam, never new behaviour).
    /// The clipboard export needs the [`egui::Context`] (the seat's copy channel);
    /// the pure-state seams route through [`Self::apply`].
    fn dispatch(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::ExportJson => self.export(ExportFormat::Json),
            MenuAction::ExportMarkdown => self.export(ExportFormat::Markdown),
            MenuAction::CopyReport => self.copy_report(ctx),
            other => self.apply(other),
        }
    }

    /// Dispatch a pure-state [`MenuBar`] activation to its real seam (§6/§7). The
    /// file/clipboard export actions are handled in [`Self::dispatch`] (they need
    /// the render context); everything else mutates state only.
    fn apply(&mut self, action: MenuAction) {
        match action {
            MenuAction::Scan => self.refresh(),
            MenuAction::View(mode) => self.view = mode,
            MenuAction::ExpandAll => self.expand_all(),
            MenuAction::CollapseAll => self.collapse_all(),
            MenuAction::About => self.show_about = true,
            // Handled in `dispatch` (they need the render context) — never reached.
            MenuAction::ExportJson | MenuAction::ExportMarkdown | MenuAction::CopyReport => {}
        }
    }

    /// Export the **selected host + active view mode** to a real file (DEVMGR-6,
    /// design #23 / §7): build the JSON or Markdown contents ([`render_json`] /
    /// [`render_report`]) and write them under [`export_dir`] (no native save
    /// dialog exists on this seat). A success confirms on the shared KIRON toast
    /// lane with the written path; a failed write raises an error toast, never a
    /// silent no-op. A host with nothing published writes an honest "no inventory"
    /// report, not a fabricated one.
    fn export(&self, format: ExportFormat) {
        let host = self.export_host();
        let inv = self.inventory.as_ref();
        let contents = match format {
            ExportFormat::Json => render_json(inv, host),
            ExportFormat::Markdown => render_report(inv, host, self.view),
        };
        let filename = format!("{host}-{}.{}", self.view.slug(), format.ext());
        match write_export(&export_dir(), &sanitize(&filename), &contents) {
            Ok(path) => raise_toast(
                "info",
                &format!("Exported {} to {}", format.noun(), path.display()),
            ),
            Err(err) => raise_toast("warning", &format!("Export failed: {err}")),
        }
    }

    /// Copy the selected host's Markdown report to the seat clipboard (DEVMGR-6,
    /// design #23) — the no-filesystem path — and confirm on the toast lane. The
    /// report reflects the active view mode, like the file export.
    fn copy_report(&self, ctx: &egui::Context) {
        ctx.copy_text(render_report(
            self.inventory.as_ref(),
            self.export_host(),
            self.view,
        ));
        raise_toast("info", "Copied the device report to the clipboard");
    }

    /// The host an export is labelled for — the loaded inventory's own host when
    /// one is present, else the selected rail host (an honest absent export).
    fn export_host(&self) -> &str {
        self.inventory
            .as_ref()
            .map_or(self.selected_host.as_str(), |inv| inv.host.as_str())
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

    /// Build the three menus from live state (#19 → MENUBAR-ALL): **Action** (Scan +
    /// the DEVMGR-6 Export/Copy report seams — MDM's `Action → generate a report`),
    /// **View** (the three modes as radio items — only [`ViewMode::ByType`] enabled,
    /// the others honestly disabled §7 — plus Expand/Collapse-all, gated on a loaded
    /// inventory), and **Help** (the ⓘ dialog). No invented File/Edit spine — the
    /// export lives under Action, exactly as Device Manager's does (§7).
    fn build_menus(&self) -> Vec<Menu<MenuAction>> {
        let action = Menu::new(
            "Action",
            vec![
                Entry::Item(Item::new(MenuAction::Scan, "Scan for hardware changes")),
                Entry::Separator,
                Entry::Item(Item::new(
                    MenuAction::ExportJson,
                    "Export inventory (JSON)\u{2026}",
                )),
                Entry::Item(Item::new(
                    MenuAction::ExportMarkdown,
                    "Export report (Markdown)\u{2026}",
                )),
                Entry::Item(Item::new(
                    MenuAction::CopyReport,
                    "Copy report to clipboard",
                )),
            ],
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
            .map_or(self.selected_host.as_str(), |inv| inv.host.as_str());
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
            self.toggle_device_selection(sel);
        }
    }

    /// Open, or toggle-closed, the detail drawer for a clicked device row —
    /// shared by the By-type [`Self::tree`] and the By-connection
    /// [`Self::connection_tree`] so a row behaves identically in both. A click on
    /// the already-open device closes the drawer; a new device selects it and
    /// resets to the General tab.
    fn toggle_device_selection(&mut self, sel: DeviceSelection) {
        if self.selected.as_ref() == Some(&sel) {
            self.selected = None;
        } else {
            self.selected = Some(sel);
            self.active_tab = DrawerTab::General;
        }
    }

    /// The **By-connection** device tree (DEVMGR-5, #3): the same devices
    /// re-rooted under their parent bus / controller instead of their function
    /// category — host → PCI/USB bus segment → device — reconstructed from each
    /// record's [`DeviceRecord::sysfs_path`] ([`build_connection_tree`]). A device
    /// with no resolvable bus renders directly under the host root (never dropped,
    /// §7); a host that published no bus/parent data at all degrades to an honest
    /// flat list under the root with a note, never a fabricated hierarchy. The
    /// per-bus branches share [`Self::expanded`] (keyed on the bus-branch id) and
    /// the device rows + selection reuse the By-type render, so only the nesting
    /// differs between modes.
    fn connection_tree(&mut self, ui: &mut egui::Ui) {
        // The bus branch a header click toggled + the device a row click selected
        // this frame — applied AFTER the read borrow ends (as in [`Self::tree`]).
        let mut toggled: Option<String> = None;
        let mut clicked: Option<DeviceSelection> = None;
        let selected = self.selected.clone();
        // Build an owned tree (clones the records) so the immutable inventory
        // borrow ends before the mutate-after-frame toggle/selection below.
        let tree = self.inventory.as_ref().map(build_connection_tree);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let Some(tree) = tree.as_ref() else {
                    return;
                };
                if tree.flat_no_bus {
                    // The honest degrade (§7): no derivable topology, so the tree
                    // is flat under the root rather than a fabricated hierarchy.
                    muted_note(
                        ui,
                        "No bus / parent topology was published for this host \u{2014} \
                         devices are listed flat under the host. A deeper by-connection \
                         tree needs a parent/bus field in the device inventory.",
                    );
                    ui.add_space(Style::SP_XS);
                }
                for node in &tree.roots {
                    if let Some(dev) = &node.device {
                        // A parentless device leaf directly under the host root
                        // (§7 — never dropped).
                        let is_sel = selected
                            .as_ref()
                            .is_some_and(|s| s.matches(&node.category, dev));
                        if device_row(ui, dev, is_sel) {
                            clicked = Some(DeviceSelection::of(&node.category, dev));
                        }
                    } else {
                        // A synthetic bus / controller branch — its devices nest
                        // beneath it (host \u{2192} bus \u{2192} device).
                        let open = self.expanded.contains(node.key.as_str());
                        let out = conn_bus_header(ui, node, open, selected.as_ref());
                        if out.header_clicked {
                            toggled = Some(node.key.clone());
                        }
                        if let Some(sel) = out.selected {
                            clicked = Some(sel);
                        }
                    }
                }
            });
        if let Some(key) = toggled {
            self.toggle(&key);
        }
        if let Some(sel) = clicked {
            self.toggle_device_selection(sel);
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

// ─────────────────── export / print the inventory (DEVMGR-6, #23) ───────────

/// The directory a device export lands in (DEVMGR-6, §7). No native file-save
/// dialog seam exists on this DRM seat, so the write is deterministic:
/// `$XDG_DATA_HOME/mde/device-inventory/`, else `~/.local/share/mde/
/// device-inventory/`, else a temp-dir fallback (an honest last resort so an
/// export never silently no-ops on a seat with no HOME). Pure over the
/// environment, so the resolution is unit-tested without touching disk.
fn export_dir() -> PathBuf {
    // A non-empty env dir joined with the `mde/device-inventory` tail, or `None`
    // when the var is unset / empty (an empty XDG var reads as unset, per spec).
    let from_env = |var: &str, tail: &[&str]| -> Option<PathBuf> {
        let val = std::env::var_os(var)?;
        if val.is_empty() {
            return None;
        }
        let mut path = PathBuf::from(val);
        path.extend(tail);
        Some(path)
    };
    from_env("XDG_DATA_HOME", &["mde", "device-inventory"])
        .or_else(|| from_env("HOME", &[".local", "share", "mde", "device-inventory"]))
        .unwrap_or_else(|| std::env::temp_dir().join("mde-device-inventory"))
}

/// Neutralize any character that is not filename-safe (a path separator, a shell
/// glyph) to `_`, keeping DNS-safe hostnames + the view slug intact — so a
/// hostile / odd hostname can never escape [`export_dir`].
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Write an export atomically into `dir` (DEVMGR-6, §7 — a real write, not a
/// stub): create the dir, write a temp sibling, then rename it over the target
/// (the tmp-then-rename pattern the shell's other JSON writers use), so a reader
/// never sees a half-written report. Returns the written path (for the
/// confirmation toast) or the honest [`std::io::Error`] (for the error toast).
fn write_export(dir: &Path, filename: &str, contents: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(filename);
    let tmp = dir.join(format!(".{filename}.tmp"));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Raise a confirmation / error chyron on the shell's ONE KIRON toast lane
/// (`event/toast/show`) — the same lane the Chat / Explorer nav toasts use — so
/// an export honestly reports its outcome (§7: a failed write is never a silent
/// no-op). `severity` is the wire token (`info` on success, `warning` on a failed
/// write). A seat with no reachable Bus simply prints nothing (the same graceful
/// degrade the other toast raises take), the file having already been written.
fn raise_toast(severity: &str, headline: &str) {
    let Some(root) = mde_bus::client_data_dir() else {
        return;
    };
    let Ok(persist) = Persist::open(root) else {
        return;
    };
    let body = serde_json::json!({
        "severity": severity,
        "flag": "DEVICE",
        "headline": headline,
    })
    .to_string();
    let _ = persist.write(TOAST_TOPIC, Priority::Default, None, Some(body.as_str()));
}

/// Serialize the selected host's inventory to pretty JSON (DEVMGR-6, #23) — the
/// machine export of the §6 [`DeviceInventory`] contract, which round-trips. A
/// host with nothing published serializes an **honest** small object (`published:
/// false` + a note), never a fabricated inventory tree (§7). Pure, so the export
/// is unit-tested without a render.
fn render_json(inv: Option<&DeviceInventory>, host: &str) -> String {
    inv.map_or_else(
        || {
            serde_json::to_string_pretty(&serde_json::json!({
                "host": host,
                "published": false,
                "note": "no device inventory has been published for this host yet",
            }))
        },
        serde_json::to_string_pretty,
    )
    .unwrap_or_else(|_| format!("{{\"host\":\"{host}\",\"published\":false}}"))
}

/// Render the human-readable Markdown report (DEVMGR-6, #23), mirroring the
/// on-screen tree: a host header + summary (the #20 header-card fields), then the
/// device section grouped to reflect the **active view mode** — per category (By
/// type) or per bus / controller (By connection). Every device row carries the
/// same DEVMGR-3 problem-code + status text the drawer shows
/// ([`device_status_display`]). A host with nothing published renders an honest
/// "no inventory yet" report (§7). Pure, so the report is unit-tested.
fn render_report(inv: Option<&DeviceInventory>, host: &str, view: ViewMode) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# Device inventory \u{2014} {host}");
    let _ = writeln!(out);
    let Some(inv) = inv else {
        let _ = writeln!(
            out,
            "No device inventory has been published for {host} yet."
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "The hardware probe republishes periodically \u{2014} or press Scan, \
             then export again."
        );
        return out;
    };
    // Provenance — the view the report was taken under (a By-connection report
    // groups differently from a By-type one, so the reader knows which).
    let mode = if view == ViewMode::ByConnection {
        "By connection"
    } else {
        "By type"
    };
    let _ = writeln!(
        out,
        "_Magic-Mesh Quasar device report \u{00B7} view: {mode}_"
    );
    let _ = writeln!(out);
    // The rich host header (mirrors the on-screen header card, #20).
    for (label, value) in header_lines(inv) {
        let _ = writeln!(out, "- **{label}:** {value}");
    }
    let devices = inv.device_count();
    let problems = inv.problem_count();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "**{devices} {}**, {problems} with problems.",
        plural(devices, "device", "devices")
    );
    let _ = writeln!(out);
    // The device section, grouped to mirror the active view mode.
    if view == ViewMode::ByConnection {
        report_by_connection(&mut out, inv);
    } else {
        report_by_type(&mut out, inv);
    }
    out
}

/// The By-type device section of the report — one `##` heading per category (the
/// on-screen tree order) with its device rows beneath, an amber `⚠ N` suffix on a
/// category holding a problem device.
fn report_by_type(out: &mut String, inv: &DeviceInventory) {
    use std::fmt::Write as _;
    if inv.categories.is_empty() {
        let _ = writeln!(out, "_No devices were enumerated for this host._");
        return;
    }
    for cat in &inv.categories {
        let _ = writeln!(
            out,
            "## {}{}",
            cat.label,
            problem_suffix(cat.problem_count())
        );
        let _ = writeln!(out);
        for dev in &cat.devices {
            let _ = writeln!(out, "{}", report_device_line(dev));
        }
        let _ = writeln!(out);
    }
}

/// The By-connection device section of the report — one `##` heading per bus /
/// controller branch (reconstructed by [`build_connection_tree`]) with its
/// devices beneath, plus any parentless device leaf directly under the host. When
/// the host published no bus topology at all, an honest flat note precedes a flat
/// device list (§7 — never a fabricated hierarchy), mirroring the on-screen view.
fn report_by_connection(out: &mut String, inv: &DeviceInventory) {
    use std::fmt::Write as _;
    let tree = build_connection_tree(inv);
    if tree.flat_no_bus {
        let _ = writeln!(
            out,
            "_No bus / parent topology was published for this host \u{2014} devices \
             are listed flat under the host._"
        );
        let _ = writeln!(out);
    }
    for node in &tree.roots {
        if let Some(dev) = &node.device {
            // A parentless device leaf directly under the host root (never dropped).
            let _ = writeln!(out, "{}", report_device_line(dev));
        } else {
            let _ = writeln!(
                out,
                "## {}{}",
                node.label,
                problem_suffix(node.problem_count())
            );
            let _ = writeln!(out);
            for child in &node.children {
                if let Some(dev) = &child.device {
                    let _ = writeln!(out, "{}", report_device_line(dev));
                }
            }
            let _ = writeln!(out);
        }
    }
}

/// One device row in the report — the device name + the same DEVMGR-3 status text
/// the General drawer tab shows ([`device_status_display`]), so a `Code 28`
/// device reads identically in the report and the UI.
fn report_device_line(dev: &DeviceRecord) -> String {
    let (status, _) = device_status_display(dev);
    format!("- {} \u{2014} {status}", dev.name)
}

/// The amber `(⚠ N)` heading suffix for a branch holding problem devices (empty
/// for a clean branch) — the report twin of the tree's category badge.
fn problem_suffix(problems: usize) -> String {
    if problems > 0 {
        format!(" (\u{26A0} {problems})")
    } else {
        String::new()
    }
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

// ───────────────────── the by-connection tree (DEVMGR-5, #3) ────────────────

/// A synthetic bus / controller grouping node the By-connection view derives from
/// a device's sysfs path — the parent a device attaches under (a PCI bus segment
/// `0000:00`, a USB bus, or another `/sys/bus/<type>`). The `key` is namespaced so
/// it never collides with a category key in the shared expand set; `label` is what
/// the branch renders.
struct BusSpec {
    /// The expand-set / id-salt key (e.g. `pci:0000:00`).
    key: String,
    /// The rendered branch label (e.g. `PCI bus 0000:00`).
    label: String,
}

/// One node in the by-connection tree ([`build_connection_tree`]): either a
/// synthetic bus / controller branch (`device == None`, its `children` the devices
/// on that bus) or a device leaf (`device == Some`, no children). A parentless
/// device is a leaf directly among the roots (§7 — never dropped).
struct ConnNode {
    /// The bus branch's expand / id key (empty for a device leaf).
    key: String,
    /// The rendered label — the bus label, or the device name for a leaf.
    label: String,
    /// The device when this node is a leaf; `None` for a bus branch.
    device: Option<DeviceRecord>,
    /// The leaf device's owning category key (selection keying); empty on a bus.
    category: String,
    /// The devices nested under a bus branch (empty for a leaf).
    children: Vec<Self>,
}

impl ConnNode {
    /// A synthetic bus / controller branch.
    fn bus(spec: BusSpec) -> Self {
        Self {
            key: spec.key,
            label: spec.label,
            device: None,
            category: String::new(),
            children: Vec::new(),
        }
    }

    /// A device leaf carrying its owning category (for selection keying).
    fn leaf(category: &str, dev: &DeviceRecord) -> Self {
        Self {
            key: String::new(),
            label: dev.name.clone(),
            device: Some(dev.clone()),
            category: category.to_string(),
            children: Vec::new(),
        }
    }

    /// Problem-status devices this node covers — a leaf's own state, or the count
    /// among a bus branch's children (its `⚠ N` badge).
    fn problem_count(&self) -> usize {
        let own = usize::from(self.device.as_ref().is_some_and(|d| d.status.is_problem()));
        let kids = self
            .children
            .iter()
            .filter(|c| c.device.as_ref().is_some_and(|d| d.status.is_problem()))
            .count();
        own + kids
    }
}

/// The whole by-connection tree for one host: the host-root children (bus branches
/// first, then any parentless device leaves) plus a flag marking the honest flat
/// degrade when the host published no bus/parent topology at all (§7).
struct ConnTree {
    /// The host-root children — bus branches (sorted) then parentless leaves.
    roots: Vec<ConnNode>,
    /// True when no device carried any derivable bus — the tree is flat under the
    /// root and the view shows an honest "no topology" note (never fabricated).
    flat_no_bus: bool,
}

impl ConnTree {
    /// The bus-branch keys (Expand-all fills these in By-connection mode).
    fn bus_keys(&self) -> BTreeSet<String> {
        self.roots
            .iter()
            .filter(|n| n.device.is_none())
            .map(|n| n.key.clone())
            .collect()
    }
}

/// Reconstruct the by-connection tree from a host's inventory (DEVMGR-5): every
/// device is re-rooted under the parent bus / controller its
/// [`DeviceRecord::sysfs_path`] resolves to ([`derive_bus`]), keeping its owning
/// category for selection keying. Devices with no resolvable bus become parentless
/// leaves under the root (never dropped); when NO device resolves a bus the tree
/// is flat and `flat_no_bus` is set (the honest degrade, §7). Pure over the
/// inventory, so the nesting is unit-tested without a render.
fn build_connection_tree(inv: &DeviceInventory) -> ConnTree {
    use std::collections::BTreeMap;
    // Bus branches keyed for a stable (sorted) order; parentless devices kept
    // aside to append under the root after the buses.
    let mut buses: BTreeMap<String, ConnNode> = BTreeMap::new();
    let mut rootless: Vec<ConnNode> = Vec::new();
    let mut device_total = 0usize;
    for cat in &inv.categories {
        for dev in &cat.devices {
            device_total += 1;
            let leaf = ConnNode::leaf(&cat.key, dev);
            if let Some(spec) = derive_bus(dev.sysfs_path.as_deref()) {
                buses
                    .entry(spec.key.clone())
                    .or_insert_with(|| ConnNode::bus(spec))
                    .children
                    .push(leaf);
            } else {
                rootless.push(leaf);
            }
        }
    }
    let flat_no_bus = device_total > 0 && buses.is_empty();
    // Bus branches first (BTreeMap already orders them by key), each with its
    // devices name-sorted; then the parentless leaves, also name-sorted.
    let mut roots: Vec<ConnNode> = buses
        .into_values()
        .map(|mut bus| {
            bus.children.sort_by(|a, b| a.label.cmp(&b.label));
            bus
        })
        .collect();
    rootless.sort_by(|a, b| a.label.cmp(&b.label));
    roots.extend(rootless);
    ConnTree { roots, flat_no_bus }
}

/// The parent bus / controller a device attaches under, derived from its sysfs
/// path — the only topology signal the DEVMGR-1 schema carries. A PCI address
/// yields its `DDDD:BB` bus segment, a USB path its bus number, any other
/// `/sys/bus/<type>` its bus kind; a `None` / unrecognized path yields `None` (the
/// device falls under the host root). A richer bridge/port hierarchy would need a
/// real `parent` field in the inventory.
fn derive_bus(sysfs: Option<&str>) -> Option<BusSpec> {
    let path = sysfs?;
    if let Some(bus) = parse_pci_bus(path) {
        return Some(BusSpec {
            key: format!("pci:{bus}"),
            label: format!("PCI bus {bus}"),
        });
    }
    if let Some(busnum) = parse_usb_bus(path) {
        return Some(BusSpec {
            key: format!("usb:{busnum}"),
            label: format!("USB bus {busnum}"),
        });
    }
    if let Some(kind) = parse_bus_kind(path) {
        return Some(BusSpec {
            key: format!("bus:{kind}"),
            label: format!("{} bus", title_case(&kind)),
        });
    }
    None
}

/// The PCI `DDDD:BB` bus segment of the last PCI address in a sysfs path (the
/// device's own address, so a `/sys/devices/...` path resolves to the device's own
/// bus, not the bridge's), or `None` when the path carries no PCI address —
/// `/sys/bus/pci/devices/0000:02:00.0` → `0000:02`.
fn parse_pci_bus(path: &str) -> Option<String> {
    path.rsplit('/').find_map(pci_bdf_bus)
}

/// The `DDDD:BB` (domain:bus) of a `DDDD:BB:DD.F` PCI address component, or `None`
/// when the component is not a PCI address.
fn pci_bdf_bus(component: &str) -> Option<String> {
    let (domain, rest) = component.split_once(':')?;
    let (bus, devfn) = rest.split_once(':')?;
    let (dev, func) = devfn.split_once('.')?;
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    (hex(domain, 4) && hex(bus, 2) && hex(dev, 2) && hex(func, 1))
        .then(|| format!("{domain}:{bus}"))
}

/// The USB bus number of a USB sysfs path — the leading number of a `N-…` port
/// path or a `usbN` root hub — or `None` when the path is not USB.
fn parse_usb_bus(path: &str) -> Option<String> {
    if !path.contains("/usb") {
        return None;
    }
    let last = path.rsplit('/').next()?;
    if let Some(num) = last.strip_prefix("usb") {
        if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
            return Some(num.to_string());
        }
    }
    let head = last.split('-').next()?;
    (!head.is_empty() && head.bytes().all(|b| b.is_ascii_digit())).then(|| head.to_string())
}

/// The bus **kind** of a generic `/sys/bus/<kind>/…` path (virtio, scsi, i2c, …)
/// for a device on a bus other than PCI/USB, or `None` when the path has no
/// `/bus/<kind>/` segment.
fn parse_bus_kind(path: &str) -> Option<String> {
    let after = path.split("/bus/").nth(1)?;
    let kind = after.split('/').next()?;
    (!kind.is_empty()).then(|| kind.to_string())
}

/// Capitalize the first character of a bus-kind label (`virtio` → `Virtio`).
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// One bus / controller branch of the by-connection tree — a forced-state
/// collapsing header (its open/closed driven by the caller's expand set) whose
/// device rows nest beneath it, mirroring [`category_header`]. The header tints
/// amber with a `⚠ N` count when the bus holds a problem device. Reuses
/// [`device_row`] + [`DeviceSelection`] so a row behaves identically in both views
/// (only the nesting differs, #3).
fn conn_bus_header(
    ui: &mut egui::Ui,
    node: &ConnNode,
    open: bool,
    selected: Option<&DeviceSelection>,
) -> CategoryOutcome {
    let problems = node.problem_count();
    let tone = if problems > 0 {
        Style::WARN
    } else {
        Style::TEXT
    };
    let mut title = node.label.clone();
    if problems > 0 {
        use std::fmt::Write as _;
        let _ = write!(title, "   \u{26A0} {problems}"); // ⚠ N
    }
    let mut clicked: Option<DeviceSelection> = None;
    let resp = egui::CollapsingHeader::new(RichText::new(title).color(tone).size(Style::BODY))
        .id_salt(("dm-conn", node.key.as_str()))
        .open(Some(open))
        .show(ui, |ui| {
            for child in &node.children {
                if let Some(dev) = &child.device {
                    let is_sel = selected.is_some_and(|s| s.matches(&child.category, dev));
                    if device_row(ui, dev, is_sel) {
                        clicked = Some(DeviceSelection::of(&child.category, dev));
                    }
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

// ─────────────────────────────── the host rail (#5) ─────────────────────────

/// One host row in the rail — a freshness/health status dot, the hostname
/// (accent-tinted + strong when it is the selected host), and the ⌂ "you are
/// here" marker on the local node. An absent host dims its name (an honest offline
/// row). The whole strip is one click target (switch to this host) with a hover
/// summary. Returns `true` when the row was clicked this frame.
fn host_row(
    ui: &mut egui::Ui,
    entry: &HostEntry,
    selected: bool,
    is_local: bool,
    now_ms: u64,
) -> bool {
    let fresh = entry.freshness(now_ms);
    let resp = ui
        .horizontal(|ui| {
            status_dot(ui, host_dot_tone(entry, now_ms));
            ui.add_space(Style::SP_XS);
            let name_tone = if selected {
                Style::ACCENT
            } else if fresh == HostFreshness::Absent {
                Style::TEXT_DIM
            } else {
                Style::TEXT
            };
            let mut name = RichText::new(&entry.host)
                .color(name_tone)
                .size(Style::SMALL);
            if selected {
                name = name.strong();
            }
            ui.label(name);
            if is_local {
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new("\u{2302}") // ⌂ — you are here
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
            }
        })
        .response;
    // The row's labels don't sense clicks, so re-interact the whole strip as one
    // selection target (click a host to inspect it), with a hover summary.
    resp.interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(host_hover(entry, now_ms))
        .clicked()
}

/// The status-dot tone for a rail host (design §7): an **absent** host is dim
/// (offline / nothing published), a **stale** snapshot is amber (published but old
/// — likely offline, so its health can't be trusted), and a **fresh** host is
/// green when clean or danger when any device is faulted. Pure, so the honest
/// dim/stale/offline mapping is tested without a render.
fn host_dot_tone(entry: &HostEntry, now_ms: u64) -> egui::Color32 {
    match entry.freshness(now_ms) {
        HostFreshness::Absent => Style::TEXT_DIM,
        HostFreshness::Stale => Style::WARN,
        HostFreshness::Fresh => {
            if entry.problem_count > 0 {
                Style::DANGER
            } else {
                Style::OK
            }
        }
    }
}

/// The rail row's hover summary — device / problem counts + a freshness read-out,
/// or an honest "nothing published" for an absent host (§7). Pure over `now_ms` so
/// it is tested deterministically.
fn host_hover(entry: &HostEntry, now_ms: u64) -> String {
    use std::fmt::Write as _;
    let fresh = entry.freshness(now_ms);
    if fresh == HostFreshness::Absent {
        return "No device inventory published \u{2014} offline or not yet scanned.".to_string();
    }
    let mut s = format!(
        "{} {}",
        entry.device_count,
        plural(entry.device_count, "device", "devices")
    );
    if entry.problem_count > 0 {
        let _ = write!(
            s,
            " \u{00B7} {} {}", // ·
            entry.problem_count,
            plural(entry.problem_count, "problem", "problems")
        );
    }
    s.push('\n');
    if fresh == HostFreshness::Stale {
        s.push_str("Stale \u{2014} "); // —
    }
    s.push_str(&scanned_label(now_ms, entry.published_at_ms.unwrap_or(0)));
    s
}

#[cfg(test)]
mod tests {
    use super::{
        build_connection_tree, build_rail, cpu_line, derive_bus, device_status_display, export_dir,
        format_mem_kb, header_lines, host_dot_tone, host_hover, humanize_ago, humanize_uptime,
        problem_code, render_json, render_report, sanitize, scanned_label, status_tone,
        write_export, DeviceManagerState, DeviceSelection, DrawerTab, HostEntry, HostFreshness,
        MenuAction, ViewMode, STALE_AFTER,
    };
    use mackes_mesh_types::device_inventory::{
        self, category, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
    };
    use mde_egui::menubar::{Entry, Menu};
    use mde_egui::{egui, ChipTone, Style};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// A throwaway substrate root under the system temp dir (this crate does not
    /// vendor `tempfile`), removed on drop. Holds a `device-inventory/` dir the
    /// rail-read tests publish host fixtures into, so `refresh` exercises the real
    /// [`device_inventory::read_all`] path (DEVMGR-4's actual read).
    struct ScratchRoot(PathBuf);

    impl ScratchRoot {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let root = std::env::temp_dir().join(format!("devmgr-{tag}-{nanos}"));
            std::fs::create_dir_all(device_inventory::inventory_dir(&root)).unwrap();
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Publish a host's inventory (a re-hosted fixture at `published_at_ms`).
        fn publish(&self, host: &str, published_at_ms: u64) {
            let mut inv = DeviceInventory::fixture();
            inv.host = host.to_string();
            inv.published_at_ms = published_at_ms;
            let path = device_inventory::inventory_path(&self.0, host);
            std::fs::write(&path, serde_json::to_string(&inv).unwrap()).unwrap();
        }
    }

    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A published fixture inventory re-hosted under `host` (a distinct rail peer).
    fn host_inventory(host: &str) -> DeviceInventory {
        let mut inv = DeviceInventory::fixture();
        inv.host = host.to_string();
        inv
    }

    /// A state carrying a chosen inventory + seen flag, rooted at a non-existent
    /// path so `refresh` reads an honest `None` (no real substrate touched).
    fn state_with(inv: Option<DeviceInventory>, seen: bool) -> DeviceManagerState {
        DeviceManagerState {
            workgroup_root: PathBuf::from("/nonexistent-devmgr-test-root"),
            local_host: "laptop-mm".to_string(),
            selected_host: "laptop-mm".to_string(),
            hosts: Vec::new(),
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
    fn by_type_and_by_connection_are_wired_by_node_stays_a_disabled_seam() {
        // #3 — the View menu offers all three modes; DEVMGR-2 wired By type and
        // DEVMGR-5 wires By connection, while By node (the cross-fleet flatten)
        // stays an honest disabled seam (§7), not a stubbed render.
        assert_eq!(ViewMode::ALL.len(), 3);
        assert!(ViewMode::ByType.is_available());
        assert!(ViewMode::ByConnection.is_available());
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
        // Action → Scan + the DEVMGR-6 export/copy report seams (MDM's Action →
        // generate a report). Separators drop out of `item_ids`.
        assert_eq!(
            item_ids(&menus[0]),
            vec![
                MenuAction::Scan,
                MenuAction::ExportJson,
                MenuAction::ExportMarkdown,
                MenuAction::CopyReport,
            ]
        );
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

    // ── DEVMGR-4: the host rail + mesh-node switching ────────────────────────

    #[test]
    fn the_rail_lists_every_published_host_with_local_pinned_first() {
        // read_all delivers the published peers sorted; build_rail injects the
        // absent local "you are here" row and pins it first, the rest alphabetical.
        let all = vec![
            host_inventory("alpha"),
            host_inventory("mid-node"),
            host_inventory("zulu"),
        ];
        let rail = build_rail(&all, "laptop-mm");
        let names: Vec<&str> = rail.iter().map(|e| e.host.as_str()).collect();
        assert_eq!(names, vec!["laptop-mm", "alpha", "mid-node", "zulu"]);
        // The local node was not among the published set, so it is an honest absent
        // row (§7) — a selectable "you are here" that has published nothing yet.
        assert_eq!(rail[0].published_at_ms, None);
        assert_eq!(rail[0].freshness(0), HostFreshness::Absent);
        // A published peer carries its real counts (the fixture: 2 devices, 1 fault).
        let alpha = rail.iter().find(|e| e.host == "alpha").unwrap();
        assert_eq!(alpha.device_count, 2);
        assert_eq!(alpha.problem_count, 1);
    }

    #[test]
    fn the_local_host_is_pinned_first_even_when_it_published_and_sorts_late() {
        // "zeta" is the local node AND published; alphabetically last, but the rail
        // pins it first (you-are-here) with no duplicate row.
        let all = vec![
            host_inventory("alpha"),
            host_inventory("beta"),
            host_inventory("zeta"),
        ];
        let rail = build_rail(&all, "zeta");
        let names: Vec<&str> = rail.iter().map(|e| e.host.as_str()).collect();
        assert_eq!(names, vec!["zeta", "alpha", "beta"]);
        assert_eq!(
            rail.iter().filter(|e| e.host == "zeta").count(),
            1,
            "the published local host is not duplicated by the injected row"
        );
        assert!(
            rail[0].published_at_ms.is_some(),
            "local published, not absent"
        );
    }

    #[test]
    fn refresh_reads_the_rail_and_switching_loads_the_selected_hosts_tree() {
        // A real multi-host substrate — the DEVMGR-4 read path end to end.
        let scratch = ScratchRoot::new("switch");
        scratch.publish("laptop-mm", 1_000); // the local node
        scratch.publish("edge-1", 2_000);
        scratch.publish("edge-2", 3_000);
        let mut s = state_with(None, false);
        s.workgroup_root = scratch.path().to_path_buf();
        s.refresh();
        // The rail lists every published host from the peer directory, local first.
        let names: Vec<String> = s.hosts.iter().map(|e| e.host.clone()).collect();
        assert_eq!(names, vec!["laptop-mm", "edge-1", "edge-2"]);
        // The default selection loaded the LOCAL host's tree.
        assert_eq!(s.inventory.as_ref().unwrap().host, "laptop-mm");
        // Switching selects the right host's published tree, instantly (#7 hybrid).
        s.select_host("edge-2".to_string());
        assert_eq!(s.selected_host, "edge-2");
        assert_eq!(s.inventory.as_ref().unwrap().host, "edge-2");
        assert_eq!(s.inventory.as_ref().unwrap().device_count(), 2);
        // Switching resets any open device drawer (a selection is per-host).
        assert!(s.selected.is_none());
        // And it still renders headless (a live render of the switched host).
        assert!(drive(&mut s) > 0, "the switched-host surface drew nothing");
    }

    #[test]
    fn an_unpublished_selected_host_reads_an_honest_empty_tree() {
        // Only the local node has published; selecting a never-seen peer reads an
        // honest None (the empty-host state), never a fabricated tree (§7).
        let scratch = ScratchRoot::new("absent");
        scratch.publish("laptop-mm", 5_000);
        let mut s = state_with(None, false);
        s.workgroup_root = scratch.path().to_path_buf();
        s.refresh();
        s.select_host("ghost-node".to_string());
        assert_eq!(s.selected_host, "ghost-node");
        assert!(
            s.inventory.is_none(),
            "an unpublished host reads as None, not a fake tree"
        );
        assert!(s.seen);
        assert!(drive(&mut s) > 0, "the empty-host state drew nothing");
        // The local "you are here" row stays present in the rail regardless.
        assert!(s.hosts.iter().any(|e| e.host == "laptop-mm"));
    }

    #[test]
    fn freshness_maps_to_honest_dim_stale_and_offline_dots() {
        let now = 10_000_000_u64;
        let stale_ms = u64::try_from(STALE_AFTER.as_millis()).unwrap();
        // Absent — nothing published: dim (offline), never green.
        let absent = HostEntry::absent("ghost");
        assert_eq!(absent.freshness(now), HostFreshness::Absent);
        assert_eq!(host_dot_tone(&absent, now), Style::TEXT_DIM);
        // Fresh + clean → OK green; fresh + a fault → danger red.
        let fresh_ok = HostEntry {
            host: "a".into(),
            published_at_ms: Some(now - 1_000),
            device_count: 3,
            problem_count: 0,
        };
        assert_eq!(fresh_ok.freshness(now), HostFreshness::Fresh);
        assert_eq!(host_dot_tone(&fresh_ok, now), Style::OK);
        let fresh_bad = HostEntry {
            problem_count: 2,
            ..fresh_ok
        };
        assert_eq!(host_dot_tone(&fresh_bad, now), Style::DANGER);
        // Stale — published, but older than STALE_AFTER: amber, not green (its
        // health can't be trusted), even with no problems in the stale snapshot.
        let stale = HostEntry {
            host: "b".into(),
            published_at_ms: Some(now - stale_ms - 1),
            device_count: 5,
            problem_count: 0,
        };
        assert_eq!(stale.freshness(now), HostFreshness::Stale);
        assert_eq!(host_dot_tone(&stale, now), Style::WARN);
        // A published-but-unknown-time snapshot (the schema's honest 0) reads stale.
        let unknown = HostEntry {
            host: "c".into(),
            published_at_ms: Some(0),
            device_count: 1,
            problem_count: 0,
        };
        assert_eq!(unknown.freshness(now), HostFreshness::Stale);
    }

    #[test]
    fn the_rail_renders_headless_and_its_hover_stays_honest() {
        let now = 10_000_000_u64;
        // An absent host's hover is the honest offline line — it invents no counts
        // and no freshness read-out (a single line, no "N devices" / "Scanned …").
        let absent = HostEntry::absent("ghost");
        let h = host_hover(&absent, now);
        assert!(
            h.contains("No device inventory published"),
            "absent hover: {h}"
        );
        assert!(
            !h.contains("Scanned"),
            "an absent hover invents no freshness: {h}"
        );
        assert!(
            !h.contains('\n'),
            "an absent hover is a single honest line: {h}"
        );
        // A stale host's hover is flagged honestly, with its real counts.
        let stale = HostEntry {
            host: "b".into(),
            published_at_ms: Some(now - 600_000),
            device_count: 5,
            problem_count: 1,
        };
        let h = host_hover(&stale, now);
        assert!(h.contains("Stale"), "a stale hover flags staleness: {h}");
        assert!(
            h.contains("5 devices") && h.contains("1 problem"),
            "the real counts: {h}"
        );
        // The rail itself renders headless from a populated hosts list (a live
        // render — a fresh local peer + an offline one — proving it isn't dead).
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        s.hosts = vec![
            HostEntry::from_inventory(&DeviceInventory::fixture()),
            HostEntry::absent("edge-offline"),
        ];
        assert!(drive(&mut s) > 0, "the host rail drew nothing");
    }

    // ── DEVMGR-5: the By-connection (bus / controller) view ──────────────────

    #[test]
    fn by_connection_nests_each_device_under_its_parent_bus() {
        // The fixture's two PCI devices sit on distinct bus segments (0000:00 and
        // 0000:02); the by-connection tree re-roots them under those bus branches
        // (host → bus → device) — correct parent→child nesting, no flat degrade.
        let tree = build_connection_tree(&DeviceInventory::fixture());
        assert!(!tree.flat_no_bus, "the fixture carries real bus topology");
        let labels: Vec<&str> = tree.roots.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, vec!["PCI bus 0000:00", "PCI bus 0000:02"]);
        // Every root is a bus branch (no parentless leaves), each holding its one
        // device as a child leaf under the correct parent bus.
        for bus in &tree.roots {
            assert!(
                bus.device.is_none(),
                "a root bus branch is not a device leaf"
            );
            assert_eq!(bus.children.len(), 1, "one device on each fixture bus");
        }
        assert_eq!(
            tree.roots[0].children[0].label, "Intel UHD Graphics 620",
            "the GPU nests under its own bus segment 0000:00"
        );
        // The bus keys (Expand-all fodder) are exactly the two segment branches.
        assert_eq!(tree.bus_keys().len(), 2);
    }

    #[test]
    fn by_connection_puts_a_parentless_device_under_the_host_root() {
        // A device with no sysfs path (nothing to resolve a bus from) is never
        // dropped — it renders as a leaf directly among the roots (§7).
        let mut inv = DeviceInventory::fixture();
        inv.categories.push(device_inventory::DeviceCategory::new(
            category::SENSORS,
            vec![DeviceRecord::new("ACPI thermal zone", DeviceStatus::Ok)],
        ));
        let tree = build_connection_tree(&inv);
        assert!(!tree.flat_no_bus, "some devices still carry a bus");
        // The parentless sensor is a root-level leaf (device Some, no bus branch).
        let leaf = tree
            .roots
            .iter()
            .find(|n| {
                n.device
                    .as_ref()
                    .is_some_and(|d| d.name == "ACPI thermal zone")
            })
            .expect("the parentless device stays under the root, never dropped");
        assert!(leaf.children.is_empty(), "a leaf has no children");
        assert!(leaf.key.is_empty(), "a leaf is not a bus branch");
        // The two PCI bus branches are still present alongside it.
        assert_eq!(
            tree.roots.iter().filter(|n| n.device.is_none()).count(),
            2,
            "the PCI bus branches remain"
        );
    }

    #[test]
    fn by_connection_degrades_honestly_with_no_bus_data() {
        // A host that published no sysfs paths at all (a shallow / non-PC host,
        // #22) has no derivable topology — the tree renders flat under the root
        // with the honest note flag set, never a fabricated hierarchy (§7).
        let inv = DeviceInventory {
            host: "vyos-edge".to_string(),
            published_at_ms: 1,
            summary: HostSummary::default(),
            tools: device_inventory::ToolAvailability::default(),
            categories: vec![device_inventory::DeviceCategory::new(
                category::NETWORK_ADAPTERS,
                vec![
                    DeviceRecord::new("eth0", DeviceStatus::Ok),
                    DeviceRecord::new("eth1", DeviceStatus::Ok),
                ],
            )],
        };
        let tree = build_connection_tree(&inv);
        assert!(tree.flat_no_bus, "no bus data → the honest flat degrade");
        // Both devices are flat leaves under the root (no bus branch invented).
        assert_eq!(tree.roots.len(), 2);
        assert!(
            tree.roots
                .iter()
                .all(|n| n.device.is_some() && n.children.is_empty()),
            "every node is a flat device leaf, no fabricated bus branch"
        );
        assert!(tree.bus_keys().is_empty(), "no bus branches to expand");
    }

    #[test]
    fn switching_to_by_connection_preserves_the_selected_host_and_renders() {
        // The rail selection (DEVMGR-4) governs the host; flipping the view mode
        // (DEVMGR-5) re-groups the SAME host's devices without changing which host
        // is inspected or its loaded inventory.
        let scratch = ScratchRoot::new("view-switch");
        scratch.publish("laptop-mm", 1_000);
        scratch.publish("edge-2", 2_000);
        let mut s = state_with(None, false);
        s.workgroup_root = scratch.path().to_path_buf();
        s.refresh();
        s.select_host("edge-2".to_string());
        assert_eq!(s.selected_host, "edge-2");
        // Flip to By-connection — the seam the View menu drives.
        s.apply(MenuAction::View(ViewMode::ByConnection));
        assert_eq!(s.view, ViewMode::ByConnection);
        // The selected host + its inventory are unchanged by the view switch.
        assert_eq!(s.selected_host, "edge-2", "the host survives a view flip");
        assert_eq!(s.inventory.as_ref().unwrap().host, "edge-2");
        // Expand-all now fills the BUS branches (not the category keys) for this
        // view — the one control tracks whichever tree is showing.
        s.expand_all();
        assert_eq!(
            s.expanded,
            build_connection_tree(s.inventory.as_ref().unwrap()).bus_keys(),
            "Expand-all fills the by-connection bus branches"
        );
        // And the by-connection tree renders headless (a live render, not dead).
        assert!(drive(&mut s) > 0, "the by-connection surface drew nothing");
    }

    #[test]
    fn derive_bus_reads_pci_usb_and_generic_paths_and_honest_none() {
        // PCI: the device's own DDDD:BB bus segment (the flat symlink form).
        assert_eq!(
            derive_bus(Some("/sys/bus/pci/devices/0000:02:00.0")).map(|b| b.label),
            Some("PCI bus 0000:02".to_string())
        );
        // PCI: a real /sys/devices/… path resolves to the device's own bus, not
        // the bridge's (the last address in the path).
        assert_eq!(
            derive_bus(Some("/sys/devices/pci0000:00/0000:00:1c.5/0000:03:00.0")).map(|b| b.label),
            Some("PCI bus 0000:03".to_string())
        );
        // USB: the bus number of a port path (topology from the sysfs name).
        assert_eq!(
            derive_bus(Some("/sys/bus/usb/devices/1-1.2")).map(|b| b.label),
            Some("USB bus 1".to_string())
        );
        // A generic bus kind is title-cased.
        assert_eq!(
            derive_bus(Some("/sys/bus/virtio/devices/virtio0")).map(|b| b.label),
            Some("Virtio bus".to_string())
        );
        // No path, or an unrecognized one, resolves no bus (→ the host root).
        assert_eq!(derive_bus(None).map(|b| b.key), None);
        assert_eq!(derive_bus(Some("/proc/cpuinfo")).map(|b| b.key), None);
    }

    // ── DEVMGR-6: export / print the inventory (#23) ─────────────────────────

    #[test]
    fn export_json_round_trips_the_fixture_inventory() {
        // The machine export serde-serializes the §6 contract and round-trips it
        // byte-for-value — the JSON is the same DeviceInventory back.
        let inv = DeviceInventory::fixture();
        let json = render_json(Some(&inv), &inv.host);
        let back: DeviceInventory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, inv, "the JSON export round-trips the §6 contract");
    }

    #[test]
    fn the_markdown_report_lists_the_host_every_device_and_the_problem_code() {
        let inv = DeviceInventory::fixture();
        let report = render_report(Some(&inv), &inv.host, ViewMode::ByType);
        // The host header + the mirrored header-card summary fields (#20).
        assert!(report.contains("laptop-mm"), "the host header: {report}");
        assert!(report.contains("Fedora"), "the OS summary line: {report}");
        // Every published device is named (the on-screen tree membership).
        assert!(report.contains("Intel UHD Graphics 620"), "the GPU row");
        assert!(report.contains("SD Host Controller"), "the PCI device row");
        // The driverless device carries its MDM problem code + the honest Linux
        // reason, identical to the drawer's General tab (DEVMGR-3 reuse).
        assert!(report.contains("Code 28"), "the MDM problem code: {report}");
        assert!(
            report.contains("no kernel driver bound"),
            "the honest reason"
        );
        // A healthy device reads the working-properly line, never a fake code.
        assert!(report.contains("This device is working properly."));
    }

    #[test]
    fn an_absent_host_exports_an_honest_empty_report_not_a_fabricated_one() {
        // Markdown names the host + an honest "no inventory" note, no device rows.
        let report = render_report(None, "ghost-node", ViewMode::ByType);
        assert!(report.contains("ghost-node"), "the host is named: {report}");
        assert!(
            report.contains("No device inventory has been published"),
            "the honest empty note: {report}"
        );
        assert!(
            !report.contains("Code"),
            "no fabricated device rows: {report}"
        );
        // JSON is an honest published:false object, never a faked inventory tree.
        let json = render_json(None, "ghost-node");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["host"], serde_json::json!("ghost-node"));
        assert_eq!(v["published"], serde_json::json!(false));
        assert!(
            v.get("categories").is_none(),
            "an absent export fabricates no category tree: {json}"
        );
    }

    #[test]
    fn the_report_groups_to_reflect_the_active_view_mode() {
        let inv = DeviceInventory::fixture();
        // By type groups under the category labels (the default tree).
        let by_type = render_report(Some(&inv), &inv.host, ViewMode::ByType);
        assert!(by_type.contains("view: By type"), "the provenance line");
        assert!(
            by_type.contains("## Display adapters"),
            "by-type groups under category headings: {by_type}"
        );
        assert!(
            !by_type.contains("PCI bus 0000:00"),
            "by-type does not bus-group: {by_type}"
        );
        // By connection re-groups the SAME devices under the bus / controller
        // topology instead (DEVMGR-5 parity, reflected in the export).
        let by_conn = render_report(Some(&inv), &inv.host, ViewMode::ByConnection);
        assert!(
            by_conn.contains("view: By connection"),
            "the provenance line"
        );
        assert!(
            by_conn.contains("## PCI bus 0000:00"),
            "by-connection groups under bus headings: {by_conn}"
        );
        assert!(
            !by_conn.contains("## Display adapters"),
            "by-connection regroups off the function category: {by_conn}"
        );
        // The grouping changes, not the membership — every device is still listed.
        for report in [&by_type, &by_conn] {
            assert!(report.contains("Intel UHD Graphics 620"));
            assert!(report.contains("SD Host Controller"));
        }
    }

    #[test]
    fn write_export_writes_a_real_file_that_round_trips() {
        // §7 — a real write, not a stub: the bytes land on disk and read back to
        // the same inventory, and the tmp-then-rename leaves no stray sibling.
        let scratch = ScratchRoot::new("export-write");
        let dir = scratch.path().join("exports");
        let inv = DeviceInventory::fixture();
        let json = render_json(Some(&inv), &inv.host);
        let path = write_export(&dir, "laptop-mm-by-type.json", &json).expect("the export writes");
        assert!(path.exists(), "the export file is on disk");
        let read = std::fs::read_to_string(&path).unwrap();
        let back: DeviceInventory = serde_json::from_str(&read).unwrap();
        assert_eq!(back, inv, "the written file round-trips the inventory");
        assert!(
            !dir.join(".laptop-mm-by-type.json.tmp").exists(),
            "the rename consumed the temp sibling"
        );
    }

    #[test]
    fn a_failed_export_write_is_an_honest_error_not_a_silent_no_op() {
        // A target whose parent component is a regular file cannot be created —
        // even as root — so write_export returns the honest io::Error rather than
        // pretending success (§7 — the shell then raises an error toast).
        let scratch = ScratchRoot::new("export-fail");
        let blocker = scratch.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let result = write_export(&blocker.join("under-a-file"), "x.json", "{}");
        assert!(
            result.is_err(),
            "writing under a file surfaces an error, never a silent no-op"
        );
    }

    #[test]
    fn the_export_dir_is_a_deterministic_user_data_location() {
        // No native save dialog exists on this seat, so the path is deterministic:
        // an absolute mde/device-inventory location under the user data home (or
        // the temp-dir fallback), never the cwd or a fabricated path.
        let dir = export_dir();
        assert!(dir.is_absolute(), "an absolute path: {dir:?}");
        assert!(
            dir.ends_with("mde/device-inventory") || dir.ends_with("mde-device-inventory"),
            "a stable data-home location: {dir:?}"
        );
    }

    #[test]
    fn sanitize_keeps_hostnames_and_neutralizes_path_separators() {
        // A DNS-safe hostname + view slug + extension survives intact.
        assert_eq!(sanitize("laptop-mm-by-type.json"), "laptop-mm-by-type.json");
        // A path separator can never survive to escape the export dir.
        let hostile = sanitize("../../etc/passwd");
        assert!(!hostile.contains('/'), "no separator survives: {hostile}");
        assert_eq!(sanitize("a b/c"), "a_b_c");
    }
}

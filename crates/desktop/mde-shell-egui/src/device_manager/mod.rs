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
//! **Scope now covers DEVMGR-2 through DEVMGR-7** — the by-type tree + header card +
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
//! **DEVMGR-7 adds the honest device actions (#12)** — the MDM per-device action
//! set, offered as a right-click **context menu** on any device row plus a Copy
//! button in the detail drawer: **Properties** (open the device's property sheet —
//! the DEVMGR-3 drawer), **Scan for hardware changes** (re-read the inventory — the
//! honest rescan, the same seam as the Action-menu Scan), and **Copy device
//! details** (the full field dump to the seat clipboard, [`render_device_details`]).
//! **DEVMGR-8 makes the omitted verbs live (#12/#13/#14)** — the node-side
//! privileged-exec seam now exists: a `device_control` mackesd worker executes the
//! real hardware mutation on the target node. So the context menu's previously
//! omitted verbs — **Enable / Disable / Reload driver module / Rescan bus** — are
//! now PRESENT, each behind **typed-arming** ([`DeviceArming`] — echo the device
//! name, #14). Activating an armed op publishes a typed
//! [`DeviceControlRequest`](mackes_mesh_types::device_control::DeviceControlRequest)
//! into the **RAIL-selected** host's replicated `fleet/device-control/<host>/` dir
//! ([`device_control::write_request`], DEVMGR-4 host selection governs → a mesh
//! remote-exec routed to the target node), and a dispatch toast confirms. Honest
//! degrade (§7): a target host that is Absent / never-published raises an error
//! toast and writes nothing (no silent no-op); a network-device op carries a "you
//! may lose reach to this host" warning (#13). The §6 boundary holds — the wire is
//! the shared [`mackes_mesh_types::device_control`] contract, not a `mackesd` dep.
//!
//! **DEVMGR-11 adds the non-PC host types (#6/#22)** — the rail lists more than
//! mesh nodes: **Cloud/Nova instances** + **LAN-discovered hosts** (both read
//! from the EXPLORER `state/units/<node>` Bus mirrors — Nova rides the QC
//! `state/openstack` union the unit aggregator folds, LAN rides the EXPLORER-2
//! scan — via a local wire mirror, never a daemon-crate link, §6), **paired
//! phones** (the SEC-5/KDC `kdc-phones/<host>.json` replicated pairing rosters),
//! and **`VyOS` / router appliances** (the `<host>/router-registry.json` mirror
//! the router-registry worker writes). Each host type maps to a synthesized
//! [`DeviceInventory`] carrying ONLY the categories its source can honestly
//! answer (#22 — router → Network/System/Firmware, phone → Radios (Power +
//! Sensors are explicitly unreported), Nova → virtio devices, LAN → the
//! remotely-detectable NIC), never an empty category and never a fabricated
//! device (§7): a shallow source renders an honest partial tree plus an explicit
//! source note saying what is unreported and why. Privileged device ops
//! (DEVMGR-8) stay mesh-node-only — a non-PC host's context menu honestly omits
//! them (no mackesd on a phone/router/instance to drain the request).
//!
//! **DEVMGR-10 lands the By-node cross-fleet flatten (#3)** — the third
//! [`ViewMode`] is now wired. Where By-type / By-connection re-group ONE host's
//! devices, By-node re-roots the WHOLE fleet: every published host (via
//! [`device_inventory::read_all`], the same read the rail uses) becomes a
//! top-level branch with its devices nested beneath it (sub-grouped by category),
//! so an operator scans every node's hardware in one tree. Hosts with a device in
//! a problem state sort to the top ([`build_node_tree`]) and each carries a
//! per-host `⚠ N` badge. A host that has published nothing renders an honest dim
//! "no inventory" leaf, never a fabricated tree (§7). The device rows, status
//! dots, problem codes, detail drawer + context menu are the DEVMGR-2..8 seams
//! verbatim; only the outer nesting changes. In By-node the DEVMGR-4 rail
//! selection is cross-fleet-wide: the tree shows all hosts with the rail-selected
//! one accented, and clicking a device on another host is an honest cross-fleet
//! **jump** ([`DeviceManagerState::select_node_device`]) — the inspected host
//! follows the click so the drawer + any armed op always resolve against the
//! right node, never a mismatched host.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, the About renderer, …); the shell body in \
              main.rs consumes them"
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::device_control::{
    self, DeviceControlOp, DeviceControlRequest, DeviceTarget,
};
use mackes_mesh_types::device_inventory::{
    self, category, DeviceCategory, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
};
use mackes_mesh_types::peers::default_workgroup_root;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;
use mde_egui::egui::{self, Id, RichText};
use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
use mde_egui::{field, muted_note, status_dot, ChipTone, StatusChip, Style};
use mde_theme::brand;
use serde::Deserialize;

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

/// How the device tree is organised (#3). DEVMGR-2 ships **By type**, DEVMGR-5
/// ships **By connection** (the bus/controller topology), and DEVMGR-10 ships
/// **By node** (the cross-fleet flatten). The faithful MDM View menu offers all
/// three — every mode is now wired (§7 — no honestly-disabled seam remains).
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
    /// The cross-fleet flatten of every host's devices (DEVMGR-10) — every
    /// published host a top-level branch, its devices nested beneath, problem
    /// hosts first ([`build_node_tree`]). Wired.
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

    /// Whether this mode is wired: [`ByType`](Self::ByType) (DEVMGR-2),
    /// [`ByConnection`](Self::ByConnection) (DEVMGR-5) and [`ByNode`](Self::ByNode)
    /// (DEVMGR-10) — all three now render, so the View-menu radio never greys.
    /// Kept as a seam (not folded to a constant) so a future unbuilt mode can
    /// re-introduce an honest disabled control (§7) without touching the menu.
    const fn is_available(self) -> bool {
        matches!(self, Self::ByType | Self::ByConnection | Self::ByNode)
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
/// (§6/§7, one seam per entry). The MENU-5 host-switch + category-jump verbs carry
/// their target (a rail key / a category key), so this is `Clone`, not `Copy` (the
/// iac.rs idiom) — the shared bar returns it by value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MenuAction {
    /// Re-read the published inventory ([`DeviceManagerState::refresh`]).
    Scan,
    /// Switch the tree organisation ([`DeviceManagerState::view`]) — every mode
    /// is now wired (By type / By connection / By node), so all three enable (§7).
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
    /// MENU-5 — switch the inspected host to this rail key (the bar twin of a
    /// host-rail row click, #5/#6; [`DeviceManagerState::select_host`]). Covers
    /// the non-PC host kinds too — the Hosts menu lists every rail row.
    SelectHost(String),
    /// MENU-5 — jump to a published category: switch to By-type and expand it so
    /// the operator lands on it (the bar twin of a category-header click).
    JumpCategory(String),
    /// MENU-5 — copy the SELECTED device's full detail dump to the clipboard (the
    /// DEVMGR-7 per-device Copy, surfaced in the Device menu; needs the render ctx).
    CopyDeviceDetails,
    /// MENU-5 — arm a privileged device op (DEVMGR-8) on the selected device: open
    /// the typed-arming confirm (#14). Mesh-node hosts only (§7 — a non-PC host
    /// omits these; nothing publishes until the operator echoes the device name).
    ArmControl(DeviceControlOp),
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

/// What kind of host a rail row represents (DEVMGR-11, #6): a mesh node with a
/// published mackesd inventory, or one of the four non-PC sources — each mapping
/// to its own inventory source (#22) and rendering only the categories that
/// source can honestly answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostKind {
    /// A mesh peer node (the DEVMGR-1 published `device-inventory/<host>.json`).
    Node,
    /// A Cloud / Nova compute instance (the EXPLORER `state/units` mirror — the
    /// unit aggregator's fold of every node's QC `state/openstack` mirror).
    Nova,
    /// A paired phone (the SEC-5/KDC `kdc-phones/<host>.json` pairing rosters).
    Phone,
    /// A LAN-discovered off-mesh host (the EXPLORER-2 scan via `state/units`).
    Lan,
    /// A discovered `VyOS` / `EdgeOS` router appliance (`<host>/router-registry.json`).
    Router,
}

impl HostKind {
    /// Rail render order: mesh nodes first, then the non-PC sections (#6).
    const ORDER: [Self; 5] = [Self::Node, Self::Nova, Self::Phone, Self::Lan, Self::Router];

    /// The rail section header for this kind.
    const fn section(self) -> &'static str {
        match self {
            Self::Node => "Mesh nodes",
            Self::Nova => "Cloud instances",
            Self::Phone => "Phones",
            Self::Lan => "LAN hosts",
            Self::Router => "Routers",
        }
    }

    /// Whether privileged DEVMGR-8 device ops can route to this host — only a
    /// mesh node runs the mackesd `device_control` worker that drains them (§7:
    /// offering the verbs on a phone / router / instance would be a placebo).
    const fn controllable(self) -> bool {
        matches!(self, Self::Node)
    }
}

/// One row in the host rail (#5) — a peer that may or may not have published an
/// inventory. Carries just what the rail renders (name · freshness · the health
/// badge counts), decoupled from the full [`DeviceInventory`] so an absent host
/// (no published file) is still a first-class, selectable row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostEntry {
    /// The unique rail key. A mesh node's short hostname (the `read_inventory`
    /// stem); a non-PC host's source-namespaced id (`cloud:…` / `phone:…` /
    /// `lan:…` / `router:…`, DEVMGR-11) so kinds can never collide.
    host: String,
    /// The human display name the rail renders (== `host` for a mesh node).
    label: String,
    /// Which host type this row is (DEVMGR-11, #6).
    kind: HostKind,
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
            label: inv.host.clone(),
            kind: HostKind::Node,
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
            label: host.to_string(),
            kind: HostKind::Node,
            published_at_ms: None,
            device_count: 0,
            problem_count: 0,
        }
    }

    /// A rail row for a non-PC host (DEVMGR-11) over its synthesized inventory.
    fn non_pc(host: &NonPcHost) -> Self {
        Self {
            host: host.key.clone(),
            label: host.inventory.host.clone(),
            kind: host.kind,
            published_at_ms: Some(host.inventory.published_at_ms),
            device_count: host.inventory.device_count(),
            problem_count: host.inventory.problem_count(),
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

/// Append the DEVMGR-11 non-PC hosts to the mesh-node rail (#6): the node rows
/// keep their local-first order, then each non-PC kind in [`HostKind::ORDER`]
/// (Cloud → Phones → LAN → Routers), label-sorted within a kind. Pure, so the
/// grouped rail model is tested without a substrate.
fn merge_rail(mut nodes: Vec<HostEntry>, non_pc: &[NonPcHost]) -> Vec<HostEntry> {
    for kind in HostKind::ORDER {
        if kind == HostKind::Node {
            continue;
        }
        let mut rows: Vec<HostEntry> = non_pc
            .iter()
            .filter(|h| h.kind == kind)
            .map(HostEntry::non_pc)
            .collect();
        rows.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.host.cmp(&b.host)));
        nodes.extend(rows);
    }
    nodes
}

// ──────────────── the DEVMGR-11 non-PC host sources (#6/#22) ─────────────────

/// A non-PC rail host (DEVMGR-11): its unique rail key, kind, the synthesized
/// honest-partial [`DeviceInventory`] (only the categories its source can
/// answer, #22), and an explicit source note saying what is unreported (§7).
#[derive(Debug, Clone, PartialEq)]
struct NonPcHost {
    /// The source-namespaced rail key (`cloud:…` / `phone:…` / `lan:…` /
    /// `router:…`) — collision-proof against node hostnames.
    key: String,
    /// The host type.
    kind: HostKind,
    /// The honest partial tree (`inventory.host` carries the display name).
    inventory: DeviceInventory,
    /// The explicit unknowns note rendered under the header card (§7) — what
    /// this source cannot report, never silently absent.
    note: Option<String>,
}

/// The `state/units/<node>` Bus mirror prefix (the EXPLORER unit stream this
/// surface reads Nova instances + LAN hosts from) — a local mirror of
/// `mackesd::workers::unit_aggregator::state_topic` (§6: mirror the wire, never
/// link the daemon crate; the explorer surface pins the same prefix).
const UNITS_STATE_PREFIX: &str = "state/units/";

/// The unit kinds this surface folds — a local mirror of the aggregator's
/// `UnitKind` wire tokens. Unknown future kinds fail only that unit's parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UnitKindMirror {
    /// An in-mesh peer — already on the rail via its published inventory.
    Peer,
    /// An off-mesh LAN host (EXPLORER-2 scan) → a [`HostKind::Lan`] row.
    LanHost,
    /// A Nova compute instance → a [`HostKind::Nova`] row.
    Instance,
    /// Cloud objects that are not hosts — not rail material.
    Volume,
    /// (see [`Self::Volume`])
    Image,
    /// (see [`Self::Volume`])
    Network,
}

/// The Nova/Cinder detail block on an instance unit — a local mirror of the
/// aggregator's `CloudDetail` (only the fields this surface renders). Every
/// field optional: an unprobed fact stays `None` (§7).
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct CloudDetailMirror {
    /// Flavor name (`m1.small`).
    flavor: Option<String>,
    /// vCPU count from the flavor.
    vcpus: Option<u32>,
    /// RAM in MiB from the flavor.
    ram_mb: Option<u64>,
    /// Root-disk size in GiB from the flavor.
    disk_gb: Option<u64>,
    /// Nova status (`ACTIVE` / `SHUTOFF` / `ERROR`).
    status: Option<String>,
    /// Fixed IPs on the instance's ports.
    fixed_ips: Vec<String>,
    /// Floating IPs mapped onto it.
    floating_ips: Vec<String>,
    /// Neutron port ids.
    ports: Vec<String>,
    /// Creation timestamp (ISO), when reported.
    created: Option<String>,
}

/// The enrichment block (EXPLORER-9 E5) — a local mirror of the aggregator's
/// `Extras`; the LAN tree's honestly-detectable facts.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct ExtrasMirror {
    /// Reverse-DNS / mDNS name.
    rdns: Option<String>,
    /// Offline MAC-OUI vendor lookup.
    oui_vendor: Option<String>,
    /// Service/port fingerprint → type guess.
    fingerprint: Option<String>,
}

/// One unit off the `state/units/<node>` mirror — the fields this surface folds
/// (serde ignores the rest of the wire body).
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct UnitMirror {
    /// Stable source-namespaced id (`cloud:instance:<uuid>` / `lan:<ip>` …) —
    /// reused as the rail key.
    id: String,
    /// The unit kind.
    kind: UnitKindMirror,
    /// Display name.
    name: String,
    /// Best-known address, when a source reported one.
    #[serde(default)]
    address: Option<String>,
    /// Coarse health tier token (`healthy` / `degraded` / `critical` /
    /// `unreachable` / `unknown`), when a real source reports one.
    #[serde(default)]
    health: Option<String>,
    /// The Nova detail block on an instance.
    #[serde(default)]
    cloud: Option<CloudDetailMirror>,
    /// The E5 enrichment block.
    #[serde(default)]
    extras: ExtrasMirror,
    /// Most-recent observation, ms since the Unix epoch (the freshness source).
    #[serde(default)]
    last_seen_ms: u64,
}

/// The `state/units/<node>` body — the fields this surface reads.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct UnitsStateMirror {
    /// Every unit that node folded.
    units: Vec<UnitMirror>,
}

/// Read every node's `state/units/<node>` mirror off the Bus spool and dedupe
/// by unit id (latest `last_seen_ms` wins — every node folds the same fleet, so
/// mirrors overlap). `None` / no spool reads empty — the honest solo-host state.
/// The same `list_topics` + latest-body idiom the explorer surface uses.
fn read_units(bus_root: Option<&Path>) -> Vec<UnitMirror> {
    // arch-11: open through the shared BusReader seam.
    let Some(persist) = BusReader::new(bus_root.map(Path::to_path_buf)).open() else {
        return Vec::new();
    };
    let topics = persist.list_topics().unwrap_or_default();
    let mut by_id: std::collections::BTreeMap<String, UnitMirror> =
        std::collections::BTreeMap::new();
    for topic in topics.iter().filter(|t| t.starts_with(UNITS_STATE_PREFIX)) {
        let latest = persist
            .list_since(topic, None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m.body)
            .next_back();
        let Some(body) = latest else { continue };
        let Ok(state) = serde_json::from_str::<UnitsStateMirror>(&body) else {
            continue;
        };
        for unit in state.units {
            match by_id.get(&unit.id) {
                Some(prev) if prev.last_seen_ms >= unit.last_seen_ms => {}
                _ => {
                    by_id.insert(unit.id.clone(), unit);
                }
            }
        }
    }
    by_id.into_values().collect()
}

/// One paired phone off a `kdc-phones/<host>.json` roster — a local mirror of
/// the mesh-shunt's `PublishedDevice` (§6: the wire is the replicated JSON).
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct PhoneMirror {
    /// KDE-Connect device id (stable across renames) — the rail key stem.
    device_id: String,
    /// The phone's human name.
    device_name: String,
    /// The phone's Nebula overlay IP, when the pairing host knows it.
    overlay_ip: Option<String>,
    /// The pinned cert fingerprint (empty ⇒ name-relay only).
    fingerprint: String,
    /// Unix-ms when the phone was paired (0 ⇒ unknown).
    paired_at_ms: i64,
}

/// A `kdc-phones/<host>.json` roster body — the KDC-MESH-2 shape; the legacy
/// pre-roster shape (a bare device array) is handled by [`read_phones`].
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct PhoneRosterMirror {
    /// The phones the publishing host paired (own-row authority).
    devices: Vec<PhoneMirror>,
}

/// Read every host's replicated KDC pairing roster (`<root>/kdc-phones/*.json`),
/// returning each phone with the hostname that paired it. Both file shapes
/// parse (roster + the legacy bare array); junk files are skipped; phones seen
/// from several hosts dedupe by device id (a pin-carrying row wins).
fn read_phones(workgroup_root: &Path) -> Vec<(PhoneMirror, String)> {
    let mut by_id: std::collections::BTreeMap<String, (PhoneMirror, String)> =
        std::collections::BTreeMap::new();
    let dir = workgroup_root.join("kdc-phones");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(host) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Ok(data) = std::fs::read_to_string(&path) else {
            continue;
        };
        let devices = serde_json::from_str::<PhoneRosterMirror>(&data)
            .map(|r| r.devices)
            .or_else(|_| serde_json::from_str::<Vec<PhoneMirror>>(&data))
            .unwrap_or_default();
        for dev in devices {
            if dev.device_id.is_empty() {
                continue;
            }
            match by_id.get(&dev.device_id) {
                // Keep the richer row: a pinned fingerprint beats a name-relay.
                Some((prev, _)) if !prev.fingerprint.is_empty() || dev.fingerprint.is_empty() => {}
                _ => {
                    by_id.insert(dev.device_id.clone(), (dev, host.clone()));
                }
            }
        }
    }
    by_id.into_values().collect()
}

/// One discovered router appliance off a `<host>/router-registry.json` mirror —
/// a local mirror of the router-registry worker's `RouterEntry` (§6).
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct RouterMirror {
    /// Gateway MAC — the stable id.
    id: String,
    /// Management IP.
    ip: String,
    /// The mesh node this appliance sits behind (`peer:<host>`).
    node_id: String,
    /// Fingerprinted vendor token (`edgeos` / `vyos` / `vyatta-unknown` /
    /// `unknown`).
    vendor: String,
    /// First line of `show version` when managed + reachable; else empty.
    version: String,
    /// A sealed credential exists for this appliance.
    managed: bool,
    /// Discovered but no credential sealed yet (read-only surfacing).
    needs_creds: bool,
    /// This is a node's primary default-route appliance.
    is_default: bool,
}

/// Read every node's router-registry mirror (`<root>/<host>/router-registry.json`),
/// deduped by appliance id (several nodes behind one gateway publish the same
/// MAC — the managed / versioned row wins, it carries strictly more facts).
fn read_routers(workgroup_root: &Path) -> Vec<RouterMirror> {
    let mut by_id: std::collections::BTreeMap<String, RouterMirror> =
        std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(workgroup_root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path().join("router-registry.json");
        let Ok(data) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(router) = serde_json::from_str::<RouterMirror>(&data) else {
            continue;
        };
        if router.id.is_empty() {
            continue;
        }
        let richer = |r: &RouterMirror| (r.managed, !r.version.is_empty());
        match by_id.get(&router.id) {
            Some(prev) if richer(prev) >= richer(&router) => {}
            _ => {
                by_id.insert(router.id.clone(), router);
            }
        }
    }
    by_id.into_values().collect()
}

/// Map a unit's health token onto an honest device status: a reported
/// degraded/critical tier is a real problem (Code 10 with the real tier as the
/// reason); `unreachable` is an honest [`Unknown`](DeviceStatus::Unknown); a
/// healthy / absent / unrecognized tier stays plain-present (never a fabricated
/// fault, §7).
fn unit_status(health: Option<&str>) -> (DeviceStatus, Option<String>) {
    match health {
        Some(h @ ("degraded" | "critical")) => (
            DeviceStatus::Degraded,
            Some(format!("unit aggregator reports {h} health")),
        ),
        Some("unreachable") => (
            DeviceStatus::Unknown,
            Some("unreachable per the unit aggregator".to_string()),
        ),
        _ => (DeviceStatus::Ok, None),
    }
}

/// An empty non-PC host summary — every field an honest `None` (§7); the
/// builders fill only what their source actually reports.
fn blank_summary() -> HostSummary {
    HostSummary::default()
}

/// Synthesize the honest virtio tree for a Nova instance (#22 — "Nova →
/// virtio devices"): one virtio network interface per reported fixed/floating
/// IP (a Neutron port IS a virtio-net attachment on the QUASAR-CLOUD
/// libvirt/QEMU plane) — falling back to bare port ids when no IP is mapped —
/// plus the flavor's root disk as a virtio block device. vCPU / RAM flavor
/// facts land in the header summary, not as fabricated devices. No reported
/// detail ⇒ zero categories + an explicit note, never an invented tree (§7).
fn nova_host(u: &UnitMirror) -> NonPcHost {
    let cloud = u.cloud.clone().unwrap_or_default();
    let (status, problem) = unit_status(u.health.as_deref());
    let state_note = cloud
        .status
        .clone()
        .map(|s| format!("instance status: {s}"));
    let mut devices: Vec<DeviceRecord> = Vec::new();
    let mut nic = |detail: String| {
        let mut rec = DeviceRecord::new("virtio network interface", status);
        rec.problem.clone_from(&problem);
        rec.events.push(detail);
        rec.events.extend(state_note.clone());
        devices.push(rec);
    };
    for ip in &cloud.fixed_ips {
        nic(format!("fixed IP {ip}"));
    }
    for ip in &cloud.floating_ips {
        nic(format!("floating IP {ip}"));
    }
    if cloud.fixed_ips.is_empty() && cloud.floating_ips.is_empty() {
        for port in &cloud.ports {
            nic(format!("Neutron port {port}"));
        }
    }
    if let Some(gb) = cloud.disk_gb {
        let mut rec =
            DeviceRecord::new(format!("virtio block device ({gb} GiB root disk)"), status);
        rec.problem.clone_from(&problem);
        if let Some(flavor) = &cloud.flavor {
            rec.events.push(format!("flavor {flavor}"));
        }
        rec.events.extend(state_note.clone());
        devices.push(rec);
    }
    let categories = if devices.is_empty() {
        Vec::new()
    } else {
        vec![DeviceCategory {
            key: "virtio".to_string(),
            label: "Virtio devices".to_string(),
            devices,
        }]
    };
    let note = if categories.is_empty() {
        Some(
            "Nova has reported no attached-device detail for this instance yet \u{2014} \
             no virtio tree is shown rather than an invented one."
                .to_string(),
        )
    } else {
        Some(
            "A Nova instance shows its virtio devices (ports \u{2192} virtio-net, root disk \
             \u{2192} virtio-blk); guest-internal hardware is unreported."
                .to_string(),
        )
    };
    NonPcHost {
        key: u.id.clone(),
        kind: HostKind::Nova,
        inventory: DeviceInventory {
            host: u.name.clone(),
            published_at_ms: u.last_seen_ms,
            summary: HostSummary {
                cpu_count: cloud.vcpus,
                mem_total_kb: cloud.ram_mb.map(|mb| mb.saturating_mul(1024)),
                ..blank_summary()
            },
            tools: mackes_mesh_types::device_inventory::ToolAvailability::default(),
            categories,
        },
        note,
    }
}

/// Synthesize the honest tree for a paired phone (#22 — "phone → Power /
/// Sensors / Radios"): the KDC pairing roster proves a network radio path only
/// when the phone carries an overlay IP (it is dialable there), so **Radios**
/// is the one category pairing state can honestly populate; Power + Sensors are
/// explicitly unreported (the note says so), never fabricated or empty (§7).
fn phone_host(p: &PhoneMirror, paired_on: &str) -> NonPcHost {
    let mut categories = Vec::new();
    if let Some(ip) = &p.overlay_ip {
        let mut rec = DeviceRecord::new("Network radio (mesh overlay link)", DeviceStatus::Ok);
        rec.events.push(format!("overlay IP {ip}"));
        rec.events.push(format!("paired via {paired_on}"));
        if !p.fingerprint.is_empty() {
            rec.events
                .push("certificate fingerprint pinned".to_string());
        }
        rec.events
            .push("transport radio (Wi-Fi / cellular) unreported".to_string());
        categories.push(DeviceCategory {
            key: "radios".to_string(),
            label: "Radios".to_string(),
            devices: vec![rec],
        });
    }
    let dialable = if p.overlay_ip.is_some() {
        ""
    } else {
        " It has no overlay IP yet, so not even its radio link can be shown."
    };
    let note = format!(
        "Paired via {paired_on} (KDC). Pairing state carries no battery or sensor telemetry \
         \u{2014} Power and Sensors are unreported, not empty.{dialable}"
    );
    NonPcHost {
        key: format!("phone:{}", p.device_id),
        kind: HostKind::Phone,
        inventory: DeviceInventory {
            host: p.device_name.clone(),
            published_at_ms: u64::try_from(p.paired_at_ms).unwrap_or(0),
            summary: blank_summary(),
            tools: mackes_mesh_types::device_inventory::ToolAvailability::default(),
            categories,
        },
        note: Some(note),
    }
}

/// Synthesize the honest tree for a LAN-discovered host (#22 — "LAN → what's
/// remotely detectable"): the EXPLORER-2 scan observed exactly one thing — a
/// network interface answering on the LAN — so **Network adapters** carries the
/// observed NIC with its real facts (address, OUI vendor, reverse-DNS name,
/// service fingerprint). Nothing else is detectable remotely, and the note says
/// so (§7).
fn lan_host(u: &UnitMirror) -> NonPcHost {
    let (status, problem) = unit_status(u.health.as_deref());
    let name = u.address.as_ref().map_or_else(
        || "Observed network interface".to_string(),
        |addr| format!("Observed network interface ({addr})"),
    );
    let mut rec = DeviceRecord::new(name, status);
    rec.vendor.clone_from(&u.extras.oui_vendor);
    rec.problem = problem;
    if let Some(rdns) = &u.extras.rdns {
        rec.events.push(format!("reverse-DNS name {rdns}"));
    }
    if let Some(fp) = &u.extras.fingerprint {
        rec.events.push(format!("service fingerprint: {fp}"));
    }
    NonPcHost {
        key: u.id.clone(),
        kind: HostKind::Lan,
        inventory: DeviceInventory {
            host: u.name.clone(),
            published_at_ms: u.last_seen_ms,
            summary: blank_summary(),
            tools: mackes_mesh_types::device_inventory::ToolAvailability::default(),
            categories: vec![DeviceCategory::new(category::NETWORK_ADAPTERS, vec![rec])],
        },
        note: Some(
            "An off-mesh LAN host \u{2014} only what the LAN scan can detect remotely is \
             shown; its internal hardware is unreported."
                .to_string(),
        ),
    }
}

/// The human platform label for a router vendor token.
fn router_platform(vendor: &str) -> Option<&'static str> {
    match vendor {
        "edgeos" => Some("EdgeOS (Ubiquiti EdgeRouter)"),
        "vyos" => Some("VyOS"),
        "vyatta-unknown" => Some("Vyatta-family (unrecognized version)"),
        _ => None,
    }
}

/// Synthesize the honest tree for a discovered router appliance (#22 —
/// "router → Network / System / Firmware"): **Network** carries the real
/// management interface (IP + MAC + default-route fact); **System** the
/// fingerprinted platform when one was recognized; **Firmware** the real
/// `show version` line when the appliance is managed + reachable. An
/// unfingerprinted platform / unreadable firmware is an absent category plus an
/// explicit note (needs-creds), never a guess (§7).
fn router_host(r: &RouterMirror) -> NonPcHost {
    let mut categories = Vec::new();

    let mut nic = DeviceRecord::new(format!("Gateway interface ({})", r.ip), DeviceStatus::Ok);
    nic.events.push(format!("management IP {}", r.ip));
    nic.events.push(format!("MAC {}", r.id));
    if r.is_default {
        nic.events
            .push(format!("primary default route for {}", r.node_id));
    }
    categories.push(DeviceCategory::new(category::NETWORK_ADAPTERS, vec![nic]));

    if let Some(platform) = router_platform(&r.vendor) {
        let mut sys = DeviceRecord::new(format!("Router platform: {platform}"), DeviceStatus::Ok);
        sys.events.push(format!("fingerprinted as {}", r.vendor));
        categories.push(DeviceCategory {
            key: "system".to_string(),
            label: "System".to_string(),
            devices: vec![sys],
        });
    }

    if !r.version.is_empty() {
        let fw = DeviceRecord::new(format!("Firmware: {}", r.version), DeviceStatus::Ok);
        categories.push(DeviceCategory {
            key: "firmware".to_string(),
            label: "Firmware".to_string(),
            devices: vec![fw],
        });
    }

    let note = if r.needs_creds {
        "Discovered, but no router credential is sealed \u{2014} firmware and configuration \
         are unreadable until one is added (read-only surfacing)."
            .to_string()
    } else if r.version.is_empty() {
        "The appliance has not answered `show version` yet \u{2014} Firmware is unreported, \
         not empty."
            .to_string()
    } else {
        "A router shows only what its management plane reports: Network, System and Firmware."
            .to_string()
    };
    NonPcHost {
        key: format!("router:{}", r.id),
        kind: HostKind::Router,
        inventory: DeviceInventory {
            host: r.ip.clone(),
            // RouterEntry carries no publish timestamp — an honest 0 reads as
            // "stale / unknown age" in the rail, never a fabricated freshness.
            published_at_ms: 0,
            summary: blank_summary(),
            tools: mackes_mesh_types::device_inventory::ToolAvailability::default(),
            categories,
        },
        note: Some(note),
    }
}

/// Gather every DEVMGR-11 non-PC host from its real source (#6).
///
/// Nova instances + LAN hosts come off the `state/units` Bus mirrors, phones
/// off the replicated KDC pairing rosters, routers off the router-registry
/// mirrors. Each maps through its pure builder to an honest partial tree
/// (#22/§7).
fn read_non_pc(workgroup_root: &Path, bus_root: Option<&Path>) -> Vec<NonPcHost> {
    let mut out = Vec::new();
    for unit in read_units(bus_root) {
        match unit.kind {
            UnitKindMirror::Instance => out.push(nova_host(&unit)),
            UnitKindMirror::LanHost => out.push(lan_host(&unit)),
            _ => {}
        }
    }
    for (phone, paired_on) in read_phones(workgroup_root) {
        out.push(phone_host(&phone, &paired_on));
    }
    for router in read_routers(workgroup_root) {
        out.push(router_host(&router));
    }
    out
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
    /// The whole fleet's published inventories from the same [`device_inventory::read_all`]
    /// as the rail (DEVMGR-10) — the source the **By-node** cross-fleet tree
    /// ([`build_node_tree`]) flattens. Refreshed on every read; the By-type /
    /// By-connection views read only [`Self::inventory`] and ignore it.
    all_inventories: Vec<DeviceInventory>,
    /// The DEVMGR-11 non-PC hosts (#6) — Nova instances, paired phones, LAN
    /// hosts, routers — each with its synthesized honest-partial tree (#22),
    /// gathered from their real sources on every [`Self::refresh`].
    non_pc: Vec<NonPcHost>,
    /// The Bus spool the `state/units/<node>` mirrors are read from (DEVMGR-11)
    /// — [`mde_bus::client_data_dir`] in production; tests point at a tempdir.
    /// `None` reads no units (the honest no-Bus seat).
    bus_root: Option<PathBuf>,
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
    /// A pending typed-arming confirm for a privileged device op (DEVMGR-8, #14),
    /// if any — the destructive Enable/Disable/Reload/Rescan verbs stage here and
    /// dispatch only once the operator echoes the device name.
    arming: Option<DeviceArming>,
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
            all_inventories: Vec::new(),
            non_pc: Vec::new(),
            bus_root: mde_bus::client_data_dir(),
            seen: false,
            last_poll: None,
            expanded: BTreeSet::new(),
            view: ViewMode::default(),
            selected: None,
            active_tab: DrawerTab::default(),
            show_about: false,
            arming: None,
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
        // One dir read serves the rail (every peer's freshness/health), the
        // selected host's tree (found in the same set — no second file read), AND
        // the By-node cross-fleet flatten (which keeps the whole set, DEVMGR-10).
        let all = device_inventory::read_all(&self.workgroup_root);
        // DEVMGR-11 — the non-PC hosts from their real sources (#6): the
        // `state/units` mirrors (Nova + LAN), the KDC pairing rosters (phones),
        // and the router-registry mirrors.
        let non_pc = read_non_pc(&self.workgroup_root, self.bus_root.as_deref());
        self.hosts = merge_rail(build_rail(&all, &self.local_host), &non_pc);
        self.inventory = all
            .iter()
            .find(|inv| inv.host == self.selected_host)
            .cloned()
            .or_else(|| {
                // A non-PC selection resolves to its synthesized honest-partial
                // tree (DEVMGR-11) — keyed on the namespaced rail key, so a node
                // hostname can never shadow it.
                non_pc
                    .iter()
                    .find(|h| h.key == self.selected_host)
                    .map(|h| h.inventory.clone())
            });
        self.all_inventories = all;
        self.non_pc = non_pc;
        self.seen = true;
    }

    /// The kind of the currently selected host — [`HostKind::Node`] when the
    /// selection is not on the rail (the pre-poll default is the local node).
    fn selected_kind(&self) -> HostKind {
        self.hosts
            .iter()
            .find(|h| h.host == self.selected_host)
            .map_or(HostKind::Node, |h| h.kind)
    }

    /// The selected non-PC host's explicit-unknowns source note (DEVMGR-11, §7),
    /// when one applies — rendered under the header card so a shallow tree says
    /// what its source cannot report rather than looking silently sparse.
    fn selected_note(&self) -> Option<&str> {
        self.non_pc
            .iter()
            .find(|h| h.key == self.selected_host)
            .and_then(|h| h.note.as_deref())
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
    /// category in By-type, every bus / controller branch in By-connection
    /// (DEVMGR-5), or every **host** branch in By-node (DEVMGR-10, the cross-fleet
    /// keys), so the one control fills whichever tree is showing. By-node reads the
    /// whole fleet, so it fills even when the rail-selected host itself is absent.
    fn expand_all(&mut self) {
        if self.view == ViewMode::ByNode {
            self.expanded = build_node_tree(&self.all_inventories, &self.local_host).host_keys();
            return;
        }
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
                        RichText::new("Hosts")
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
                        // DEVMGR-11 (#6): grouped by host kind — a section header
                        // precedes each kind's rows (mesh nodes first, then the
                        // non-PC sources), rendered only when the kind has rows.
                        let mut last_kind: Option<HostKind> = None;
                        for entry in &self.hosts {
                            if last_kind != Some(entry.kind) {
                                if last_kind.is_some() {
                                    ui.add_space(Style::SP_S);
                                }
                                ui.label(
                                    RichText::new(entry.kind.section())
                                        .color(Style::TEXT_DIM)
                                        .size(Style::SMALL)
                                        .strong(),
                                );
                                last_kind = Some(entry.kind);
                            }
                            let is_sel = entry.host == selected;
                            let is_local = entry.kind == HostKind::Node && entry.host == local;
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
        // product mark + the ⓘ button stay always-visible.
        self.title_strip(ui);
        // MENUBAR-ALL: the shared top bar replaces DEVMGR-2's bespoke Action/View/
        // Help chrome (About is the 14th / last surface onto the shared component).
        if let Some(action) = self.chrome_bar(ui) {
            self.dispatch(action, ui.ctx());
        }
        ui.separator();
        ui.add_space(Style::SP_XS);

        // DEVMGR-8 — a pending typed-arming confirm for a privileged device op
        // renders as a prominent full-width banner above the rail/tree (#14),
        // honest feedback before any node-side mutation.
        self.render_arming(ui);

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
                } else if self.view == ViewMode::ByNode {
                    // By-node reads the WHOLE fleet (DEVMGR-10), so it renders even
                    // when the rail-selected host itself has published nothing — its
                    // absent leaf still appears among the fleet, never the single-host
                    // empty state.
                    self.node_tree(ui);
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
                    // DEVMGR-11 (§7): a non-PC host's explicit-unknowns note — what
                    // its source cannot report — so a shallow tree never reads as
                    // silently sparse.
                    if let Some(note) = self.selected_note().map(str::to_string) {
                        muted_note(ui, note);
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
            // MENU-5 — the Device menu's per-device clipboard copy needs the seat.
            MenuAction::CopyDeviceDetails => self.copy_device_details(ctx),
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
            // MENU-5 — switch the inspected host (the bar twin of a rail click, #5).
            MenuAction::SelectHost(host) => {
                if host != self.selected_host {
                    self.select_host(host);
                }
            }
            // MENU-5 — jump to a category: land on it in the By-type tree.
            MenuAction::JumpCategory(key) => {
                self.view = ViewMode::ByType;
                self.expanded.insert(key);
            }
            // MENU-5 — arm a privileged device op on the selected device (#14).
            MenuAction::ArmControl(op) => self.arm_control(op),
            // Handled in `dispatch` (they need the render context) — never reached.
            MenuAction::ExportJson
            | MenuAction::ExportMarkdown
            | MenuAction::CopyReport
            | MenuAction::CopyDeviceDetails => {}
        }
    }

    /// Copy the SELECTED device's full detail dump to the seat clipboard (MENU-5 —
    /// the DEVMGR-7 per-device Copy surfaced in the Device menu) + confirm on the
    /// shared toast lane. A no-selection is a silent no-op (the menu item is disabled
    /// without one) — never a fabricated dump.
    fn copy_device_details(&self, ctx: &egui::Context) {
        if let Some((_, dev)) = self.selected_device() {
            ctx.copy_text(render_device_details(dev));
            raise_toast(
                "info",
                &format!("Copied {} details to the clipboard", dev.name),
            );
        }
    }

    /// Stage the typed-arming confirm for a privileged device op on the selected
    /// device (MENU-5 → DEVMGR-8, #14) — the Device-menu twin of the row
    /// context-menu's Control verb, routing through the very same [`DeviceArming`]
    /// stage + [`Self::dispatch_control`]. Guarded on a mesh-node host + a live
    /// selection (§7 — a non-PC host / no selection never arms), so nothing reaches
    /// a node until the operator echoes the device name.
    fn arm_control(&mut self, op: DeviceControlOp) {
        if !self.selected_kind().controllable() {
            return;
        }
        // Resolve the target from the immutable read, then release that borrow before
        // taking `&mut self` (the toggle/selection idiom used across this surface).
        let Some((category, dev)) = self.selected_device() else {
            return;
        };
        let target = device_target(category, dev);
        self.arming = Some(DeviceArming {
            op,
            target,
            target_host: self.selected_host.clone(),
            typed: String::new(),
        });
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
    /// Action/View/Help chrome. Renders the menus (each item the mouse twin of a real
    /// seam) + a live status cluster over
    /// [`mde_egui::menubar::MenuBar`], tinted with the dock's **System** group accent
    /// ([`Style::ACCENT_SYSTEM`]), and returns the activated [`MenuAction`] (applied
    /// via [`Self::dispatch`]). About is the 14th / last surface onto the component;
    /// MENU-5 grows the spine so the bar fully covers the extended Device Manager —
    /// **Action · View · Hosts · Device · Help** (host-rail node switching, category
    /// jumps, and the armed-action posture all reachable from the bar).
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

    /// Build the menus from live state (#19 → MENUBAR-ALL, extended by MENU-5 so the
    /// bar fully covers the Device Manager as it stands): **Action** (Scan + the
    /// DEVMGR-6 Export/Copy report seams — MDM's `Action → generate a report`),
    /// **View** (the three now-wired modes as radio items, Expand/Collapse-all gated
    /// on a loaded inventory, and a **Jump to category** submenu), **Hosts** (the
    /// host-rail node switch surfaced in the bar — every rail row incl. the non-PC
    /// kinds, #5/#6), **Device** (the DEVMGR-8 armed-action posture on the selected
    /// device, honestly gated §7), and **Help** (the ⓘ dialog). No invented File/Edit
    /// spine — export lives under Action, exactly as Device Manager's does (§7).
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

        // Expand/Collapse gate on there being a tree to fill: the selected host's
        // inventory in By-type/By-connection, or ANY published host in By-node (the
        // cross-fleet flatten renders even when the rail-selected host is absent).
        let has_tree = match self.view {
            ViewMode::ByNode => !self.all_inventories.is_empty(),
            _ => self.inventory.is_some(),
        };
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
        view_entries.push(Entry::Separator);
        view_entries.push(self.jump_submenu());
        let view = Menu::new("View", view_entries);

        let help = Menu::new(
            "Help",
            vec![Entry::Item(Item::new(
                MenuAction::About,
                format!("About {}", brand::logo::PRODUCT_NAME),
            ))],
        );

        vec![action, view, self.hosts_menu(), self.device_menu(), help]
    }

    /// The MENU-5 **Jump to category** submenu (View → …): one item per the selected
    /// host's published category — activating it switches to By-type and expands that
    /// category so the operator lands on it. Honest §7 — a host that published no
    /// category reads a single caption, never a dead entry.
    fn jump_submenu(&self) -> Entry<MenuAction> {
        let mut entries = Vec::new();
        if let Some(inv) = self.inventory.as_ref() {
            for cat in &inv.categories {
                entries.push(Entry::Item(Item::new(
                    MenuAction::JumpCategory(cat.key.clone()),
                    cat.label.clone(),
                )));
            }
        }
        if entries.is_empty() {
            entries.push(Entry::Caption(
                "No categories published for this host yet.".to_string(),
            ));
        }
        Entry::Submenu {
            label: "Jump to category".to_string(),
            mnemonic: None,
            entries,
        }
    }

    /// The MENU-5 **Hosts** menu — the host-rail node switch surfaced in the bar
    /// (#5/#6): a **Refresh this host** seam (the rail's ↻ live-refresh, the same
    /// [`Self::refresh`] as Action → Scan), then every rail host grouped by kind
    /// (Mesh nodes → Cloud instances → Phones → LAN hosts → Routers — the exact rail
    /// grouping), each a radio checked on the selected host. Selecting one switches
    /// the inspected host ([`Self::select_host`]). The non-PC kinds (DEVMGR-11) are
    /// only listed when a real source published them (§7 — the rail is honest); an
    /// empty rail reads a caption, never a dead entry.
    fn hosts_menu(&self) -> Menu<MenuAction> {
        let mut entries = vec![
            Entry::Item(Item::new(MenuAction::Scan, "Refresh this host")),
            Entry::Separator,
        ];
        if self.hosts.is_empty() {
            entries.push(Entry::Caption(
                "No hosts have published an inventory yet.".to_string(),
            ));
            return Menu::new("Hosts", entries);
        }
        let mut last_kind: Option<HostKind> = None;
        for entry in &self.hosts {
            if last_kind != Some(entry.kind) {
                entries.push(Entry::Caption(entry.kind.section().to_string()));
                last_kind = Some(entry.kind);
            }
            // The local "you are here" node reads its identity in the label (the rail
            // marks it with a home glyph; the menu names it in words).
            let is_local = entry.kind == HostKind::Node && entry.host == self.local_host;
            let label = if is_local {
                format!("{} (this node)", entry.label)
            } else {
                entry.label.clone()
            };
            entries.push(Entry::Item(
                Item::new(MenuAction::SelectHost(entry.host.clone()), label)
                    .checked(entry.host == self.selected_host),
            ));
        }
        Menu::new("Hosts", entries)
    }

    /// The MENU-5 **Device** menu — the DEVMGR armed-action posture surfaced in the
    /// bar (#12/#13/#14). Acts on the SELECTED device row: read-only **Copy device
    /// details** (any host kind), then — on a **mesh node**, the only kind that runs
    /// the `device_control` worker (§7) — the armed **Enable / Disable / Reload
    /// driver module / Rescan bus** verbs, each opening the typed-arming confirm
    /// (#14) and context-gated on a live selection. A non-PC host honestly OMITS the
    /// privileged verbs (the exact disclosure the row context-menu shows), never a
    /// greyed placebo. No device selected reads a leading caption so the disabled
    /// items have context.
    fn device_menu(&self) -> Menu<MenuAction> {
        let has_device = self.selected_device().is_some();
        let controllable = self.selected_kind().controllable();
        let mut entries = Vec::new();
        if !has_device {
            entries.push(Entry::Caption(
                "Select a device row to act on it.".to_string(),
            ));
        }
        entries.push(Entry::Item(
            Item::new(MenuAction::CopyDeviceDetails, "Copy device details").enabled(has_device),
        ));
        entries.push(Entry::Separator);
        if controllable {
            for op in DeviceControlOp::ALL {
                entries.push(Entry::Item(
                    Item::new(
                        MenuAction::ArmControl(op),
                        format!("{}\u{2026}", op.label()),
                    )
                    .enabled(has_device),
                ));
            }
            entries.push(Entry::Caption(
                "Enable/Disable, reload + rescan run on the node \u{2014} armed, audited."
                    .to_string(),
            ));
        } else {
            entries.push(Entry::Caption(
                "Enable/Disable + driver ops apply to mesh nodes only \u{2014} this host \
                 runs no mesh device-control worker."
                    .to_string(),
            ));
        }
        Menu::new("Device", entries)
    }

    /// The currently selected device resolved against the live inventory (#9) —
    /// `(category key, record)`, or `None` when nothing is selected / the selection
    /// no longer resolves (a re-read pruned it). Shared by the Device menu (its
    /// gating + the armed target) and the clipboard copy, so the menu and the seam
    /// can never disagree about "is a device selected".
    fn selected_device(&self) -> Option<(&str, &DeviceRecord)> {
        let sel = self.selected.as_ref()?;
        let inv = self.inventory.as_ref()?;
        let dev = find_device(inv, sel)?;
        Some((sel.category.as_str(), dev))
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
        let mut action: Option<RowActionRequest> = None;
        let selected = self.selected.clone();
        // DEVMGR-11 — privileged DEVMGR-8 verbs only reach a mesh node's mackesd;
        // a non-PC host's rows honestly omit them (§7).
        let allow_control = self.selected_kind().controllable();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let Some(inv) = self.inventory.as_ref() else {
                    return;
                };
                if inv.categories.is_empty() {
                    // A source that could answer nothing yet (§7) — the note above
                    // says why; never a fabricated tree.
                    muted_note(ui, "No device detail is reported for this host.");
                    return;
                }
                for cat in &inv.categories {
                    let open = self.expanded.contains(cat.key.as_str());
                    let out = category_header(ui, cat, open, selected.as_ref(), allow_control);
                    if out.header_clicked {
                        toggled = Some(cat.key.clone());
                    }
                    if let Some(sel) = out.selected {
                        clicked = Some(sel);
                    }
                    if let Some(req) = out.action {
                        action = Some(req);
                    }
                }
            });
        if let Some(key) = toggled {
            self.toggle(&key);
        }
        if let Some(sel) = clicked {
            self.toggle_device_selection(sel);
        }
        if let Some(req) = action {
            self.apply_row_action(req, ui.ctx());
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

    /// Dispatch a DEVMGR-7 device-row context-menu action to its real seam (§7 —
    /// the honest, read-only subset of MDM's device verbs): **Properties** opens the
    /// device's property sheet (the DEVMGR-3 drawer, always opening — never toggling
    /// closed like a row click), **Scan** re-reads the inventory (the honest rescan,
    /// the same seam as the Action-menu Scan), and **Copy device details** dumps the
    /// full record to the seat clipboard ([`render_device_details`]) + confirms on
    /// the toast lane. The mutating verbs (Enable/Disable, Reload module) are
    /// honestly omitted upstream (this surface has no privileged-exec seam), so
    /// there is nothing destructive to typed-arm here.
    fn apply_row_action(&mut self, req: RowActionRequest, ctx: &egui::Context) {
        match req {
            RowActionRequest::Properties(sel) => {
                self.selected = Some(sel);
                self.active_tab = DrawerTab::General;
            }
            RowActionRequest::Scan => self.refresh(),
            RowActionRequest::CopyDetails(dev) => {
                ctx.copy_text(render_device_details(&dev));
                raise_toast(
                    "info",
                    &format!("Copied {} details to the clipboard", dev.name),
                );
            }
            // DEVMGR-8 — a privileged verb never fires from the menu: it stages the
            // typed-arming confirm (#14). The echoed confirm (render_arming) then
            // dispatches it to the selected host's mackesd.
            RowActionRequest::Control { op, target } => {
                self.arming = Some(DeviceArming {
                    op,
                    target: *target,
                    target_host: self.selected_host.clone(),
                    typed: String::new(),
                });
            }
        }
    }

    /// The freshness of the currently selected host (DEVMGR-4) — the honest
    /// reachability read used to block a device op against an offline / never-seen
    /// host (§7). A host absent from the rail (nothing published) reads `Absent`.
    fn selected_host_freshness(&self) -> HostFreshness {
        let now = now_ms();
        self.hosts
            .iter()
            .find(|h| h.host == self.selected_host)
            .map_or(HostFreshness::Absent, |h| h.freshness(now))
    }

    /// Render the DEVMGR-8 typed-arming confirm (#14, mirroring the IAC / Console
    /// power-op idiom): a warn-framed group naming the op + device, a
    /// **reach-loss** caption for a network device (#13 — disabling a NIC can strand
    /// the host), and the "type the device name to arm" echo. The DANGER confirm is
    /// enabled ONLY once the echo matches (an unconfirmed op can never dispatch), and
    /// a confirm drives [`Self::dispatch_control`]. Renders above the tree, like the
    /// export toast — honest, never a silent op.
    fn render_arming(&mut self, ui: &mut egui::Ui) {
        // (confirmed) captured so the arming borrow drops before the seam is driven.
        let mut act: Option<bool> = None;
        if let Some(arming) = self.arming.as_mut() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.colored_label(
                    Style::WARN,
                    RichText::new(format!("Confirm: {}", arming.op.label()))
                        .size(Style::BODY)
                        .strong(),
                );
                muted_note(
                    ui,
                    format!(
                        "Type the device name \u{201C}{}\u{201D} to arm this op on {} \u{2014} it \
                         runs on the node's real hardware and is audited.",
                        arming.target.name, arming.target_host,
                    ),
                );
                // #13 — a network device op can drop the operator's own reach.
                if arming.target.category == category::NETWORK_ADAPTERS {
                    ui.colored_label(
                        Style::DANGER,
                        RichText::new(
                            "\u{26A0} You may lose reach to this host if you down its network device.",
                        )
                        .size(Style::SMALL),
                    );
                }
                ui.add(
                    egui::TextEdit::singleline(&mut arming.typed)
                        .hint_text(arming.target.name.as_str()),
                );
                let is_armed = device_armed(&arming.typed, &arming.target.name);
                ui.horizontal(|ui| {
                    let confirm = ui.add_enabled(
                        is_armed,
                        egui::Button::new(
                            RichText::new(arming.op.label()).color(Style::DANGER),
                        ),
                    );
                    if confirm.clicked() && is_armed {
                        act = Some(true);
                    } else if ui.button("Cancel").clicked() {
                        act = Some(false);
                    }
                });
            });
            ui.add_space(Style::SP_S);
        }
        if let Some(confirmed) = act {
            let arming = self.arming.take();
            if confirmed {
                if let Some(a) = arming {
                    self.dispatch_control(a);
                }
            }
        }
    }

    /// Dispatch an armed device op to the RAIL-selected host's mackesd (DEVMGR-8,
    /// #13) — a mesh remote-exec routed to the target node. Honest degrade (§7): an
    /// **absent / never-published** target host raises an error toast and writes
    /// nothing (no silent no-op); otherwise the typed [`DeviceControlRequest`] is
    /// written into the target's replicated `fleet/device-control/<host>/` dir (the
    /// node's `device_control` worker drains + executes + audits it), and a dispatch
    /// toast confirms. A failed write is an honest error toast, never swallowed.
    fn dispatch_control(&self, arming: DeviceArming) {
        // Consume the arming into its typed parts (the echo is spent).
        let DeviceArming {
            op,
            target,
            target_host,
            typed: _,
        } = arming;
        // DEVMGR-11 kind gate (§7): only a mesh node runs the device_control
        // worker that drains these requests — a non-PC target (instance / phone /
        // LAN host / router) is refused honestly, never a request that would sit
        // in a dir nothing reads. (The context menu already omits the verbs for
        // these hosts; this is the seam-level backstop.)
        if !self.selected_kind().controllable() {
            raise_toast(
                "warning",
                &format!(
                    "{target_host} is not a mesh node \u{2014} device ops need the node-side \
                     mesh worker, so {} was not dispatched",
                    op.as_str()
                ),
            );
            return;
        }
        // Reachability gate (§7): a host that has published no inventory is offline /
        // never-seen — we can't route to it, so refuse honestly rather than write a
        // request that will never be drained.
        if self.selected_host_freshness() == HostFreshness::Absent {
            raise_toast(
                "warning",
                &format!(
                    "{target_host} is offline (no published inventory) \u{2014} cannot {} {}",
                    op.as_str(),
                    target.name
                ),
            );
            return;
        }
        // Keep the display fields before `target`/`target_host` move into the request.
        let device_name = target.name.clone();
        let req = DeviceControlRequest {
            id: next_request_id(),
            op,
            target,
            target_host: target_host.clone(),
            from: format!("peer:{}", self.local_host),
        };
        match device_control::write_request(&self.workgroup_root, &req) {
            Ok(_) => raise_toast(
                "info",
                &format!(
                    "Dispatched \u{201C}{}\u{201D} for {device_name} to {target_host}",
                    op.label(),
                ),
            ),
            Err(err) => raise_toast(
                "warning",
                &format!("Could not dispatch {} to {target_host}: {err}", op.as_str()),
            ),
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
        let mut action: Option<RowActionRequest> = None;
        let selected = self.selected.clone();
        // Build an owned tree (clones the records) so the immutable inventory
        // borrow ends before the mutate-after-frame toggle/selection below.
        let tree = self.inventory.as_ref().map(build_connection_tree);
        // DEVMGR-11 — same mesh-node-only gate as the by-type tree.
        let allow_control = self.selected_kind().controllable();
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
                        let out = device_row(ui, dev, &node.category, is_sel, allow_control);
                        if out.clicked {
                            clicked = Some(DeviceSelection::of(&node.category, dev));
                        }
                        if let Some(req) = out.action {
                            action = Some(req);
                        }
                    } else {
                        // A synthetic bus / controller branch — its devices nest
                        // beneath it (host \u{2192} bus \u{2192} device).
                        let open = self.expanded.contains(node.key.as_str());
                        let out = conn_bus_header(ui, node, open, selected.as_ref(), allow_control);
                        if out.header_clicked {
                            toggled = Some(node.key.clone());
                        }
                        if let Some(sel) = out.selected {
                            clicked = Some(sel);
                        }
                        if let Some(req) = out.action {
                            action = Some(req);
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
        if let Some(req) = action {
            self.apply_row_action(req, ui.ctx());
        }
    }

    /// The **By-node** cross-fleet tree (DEVMGR-10, #3): the whole fleet's
    /// published inventories ([`Self::all_inventories`], the same read the rail
    /// uses) flattened into one tree — each host a top-level collapsing branch, its
    /// devices nested beneath (sub-grouped by category), problem hosts sorted first
    /// with a per-host `⚠ N` badge ([`build_node_tree`]). A host that has published
    /// nothing renders an honest dim "no inventory" leaf, never a fabricated tree
    /// (§7). The host branches share [`Self::expanded`] (keyed on the namespaced
    /// host key) and the device rows + selection reuse the By-type render, so only
    /// the outer nesting differs. In this mode the rail selection is
    /// cross-fleet-wide: the rail-selected host is accented and a device click is an
    /// honest jump ([`Self::select_node_device`]).
    fn node_tree(&mut self, ui: &mut egui::Ui) {
        // The host branch a header click toggled + the device a row click selected
        // (carrying its owning HOST so a click can jump the inspected host) + any
        // context-menu action — all applied AFTER the read borrow ends (as in
        // [`Self::tree`] / [`Self::connection_tree`]).
        let mut toggled: Option<String> = None;
        let mut clicked: Option<(String, DeviceSelection)> = None;
        let mut action: Option<(String, RowActionRequest)> = None;
        let selected = self.selected.clone();
        let selected_host = self.selected_host.clone();
        let now = now_ms();
        // Build an owned tree (clones the records) so the immutable fleet borrow
        // ends before the mutate-after-frame toggle/selection below.
        let tree = build_node_tree(&self.all_inventories, &self.local_host);
        fleet_header(ui, &tree);
        ui.add_space(Style::SP_S);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if tree.hosts.is_empty() {
                    muted_note(ui, "No nodes have published a device inventory yet.");
                    return;
                }
                for host in &tree.hosts {
                    let open = self.expanded.contains(node_key(&host.host).as_str());
                    let is_selected_host = host.host == selected_host;
                    let out =
                        node_host_header(ui, host, open, selected.as_ref(), is_selected_host, now);
                    if out.header_clicked {
                        toggled = Some(node_key(&host.host));
                    }
                    if let Some(sel) = out.selected {
                        clicked = Some((host.host.clone(), sel));
                    }
                    if let Some(req) = out.action {
                        action = Some((host.host.clone(), req));
                    }
                }
            });
        if let Some(key) = toggled {
            self.toggle(&key);
        }
        if let Some((host, sel)) = clicked {
            self.select_node_device(host, sel);
        }
        if let Some((host, req)) = action {
            // A context-menu verb on another host's device routes to THAT host: jump
            // the inspection there first so Properties / Scan / an armed Control all
            // resolve + dispatch against the right node (§7 — never a mismatched host).
            if host != self.selected_host {
                self.select_host(host);
            }
            self.apply_row_action(req, ui.ctx());
        }
    }

    /// Handle a device-row click in the By-node cross-fleet view (DEVMGR-10): a
    /// click on a device owned by another host is an honest **jump** — the
    /// inspected host switches to that device's host (the rail follows, DEVMGR-4)
    /// and its detail drawer opens, so the drawer never resolves against the wrong
    /// host. A click on a device already on the selected host toggles the drawer as
    /// usual ([`Self::toggle_device_selection`]).
    fn select_node_device(&mut self, host: String, sel: DeviceSelection) {
        if host == self.selected_host {
            self.toggle_device_selection(sel);
        } else {
            self.select_host(host);
            self.selected = Some(sel);
            self.active_tab = DrawerTab::General;
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
        let mut copy = false;
        egui::TopBottomPanel::bottom(ui.id().with("devmgr-detail-drawer"))
            .resizable(true)
            .min_height(Style::SP_XL * 4.0)
            .default_height(Style::SP_XL * 7.0)
            .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
            .show_inside(ui, |ui| {
                drawer_header(ui, &dev, &mut close, &mut copy);
                drawer_tabs(ui, &mut tab);
                ui.separator();
                ui.add_space(Style::SP_XS);
                drawer_body(ui, &dev, tab);
            });
        self.active_tab = tab;
        if copy {
            // The DEVMGR-7 Copy-info action, reached from the drawer (the
            // non-right-click path) — the same seam the row context menu drives.
            ui.ctx().copy_text(render_device_details(&dev));
            raise_toast(
                "info",
                &format!("Copied {} details to the clipboard", dev.name),
            );
        }
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
    // arch-11: writer (raises a toast) — kept on Persist::open; the shared
    // BusReader seam is read-only.
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
    let product = brand::logo::PRODUCT_NAME;
    let _ = writeln!(out, "_{product} device report \u{00B7} view: {mode}_");
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
/// header was clicked (the caller toggles the expand set), any device row the
/// operator selected (the caller opens the drawer), and any DEVMGR-7 context-menu
/// action the operator chose on a device row (the caller dispatches it).
struct CategoryOutcome {
    /// The collapsing header was clicked (toggle this category's expansion).
    header_clicked: bool,
    /// A device row was clicked (open/toggle its detail drawer).
    selected: Option<DeviceSelection>,
    /// A device-row context-menu action the operator chose (DEVMGR-7).
    action: Option<RowActionRequest>,
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
    allow_control: bool,
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
    let mut action: Option<RowActionRequest> = None;
    let resp = egui::CollapsingHeader::new(RichText::new(title).color(tone).size(Style::BODY))
        .id_salt(("dm-cat", cat.key.as_str()))
        .open(Some(open))
        .show(ui, |ui| {
            for dev in &cat.devices {
                let is_sel = selected.is_some_and(|s| s.matches(&cat.key, dev));
                let out = device_row(ui, dev, &cat.key, is_sel, allow_control);
                if out.clicked {
                    clicked = Some(DeviceSelection::of(&cat.key, dev));
                }
                if let Some(req) = out.action {
                    action = Some(req);
                }
            }
        });
    CategoryOutcome {
        header_clicked: resp.header_response.clicked(),
        selected: clicked,
        action,
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
    allow_control: bool,
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
    let mut action: Option<RowActionRequest> = None;
    let resp = egui::CollapsingHeader::new(RichText::new(title).color(tone).size(Style::BODY))
        .id_salt(("dm-conn", node.key.as_str()))
        .open(Some(open))
        .show(ui, |ui| {
            for child in &node.children {
                if let Some(dev) = &child.device {
                    let is_sel = selected.is_some_and(|s| s.matches(&child.category, dev));
                    let out = device_row(ui, dev, &child.category, is_sel, allow_control);
                    if out.clicked {
                        clicked = Some(DeviceSelection::of(&child.category, dev));
                    }
                    if let Some(req) = out.action {
                        action = Some(req);
                    }
                }
            }
        });
    CategoryOutcome {
        header_clicked: resp.header_response.clicked(),
        selected: clicked,
        action,
    }
}

// ───────────────────── the by-node cross-fleet tree (DEVMGR-10, #3) ──────────

/// The expand-set / id-salt key for a By-node host branch — namespaced so it
/// never collides with a category key (By-type) or a bus key (By-connection) in
/// the shared [`DeviceManagerState::expanded`] set.
fn node_key(host: &str) -> String {
    format!("node:{host}")
}

/// One host branch in the By-node cross-fleet tree ([`build_node_tree`]): a
/// top-level host carrying its own device tree (its categories, cloned so the
/// render borrow ends before the mutate-after-frame toggle), or an honest absent
/// leaf (`published_at_ms == None`, no categories) for a host that has published
/// nothing (§7 — never a fabricated tree).
struct NodeHost {
    /// The host's short name (the expand key + the selection/jump anchor).
    host: String,
    /// When it last published (`None` = absent — an honest dim leaf).
    published_at_ms: Option<u64>,
    /// Total device count in its snapshot (0 for an absent host).
    device_count: usize,
    /// Problem-status device count (its `⚠ N` badge; drives the fleet ranking).
    problem_count: usize,
    /// Its categorized device tree (empty for an absent host).
    categories: Vec<DeviceCategory>,
}

impl NodeHost {
    /// A host branch from a published inventory (its whole device tree cloned in).
    fn from_inventory(inv: &DeviceInventory) -> Self {
        Self {
            host: inv.host.clone(),
            published_at_ms: Some(inv.published_at_ms),
            device_count: inv.device_count(),
            problem_count: inv.problem_count(),
            categories: inv.categories.clone(),
        }
    }

    /// An absent host branch — a known host (e.g. the local "you are here" node)
    /// that has published nothing yet. Rendered as an honest dim leaf (§7).
    fn absent(host: &str) -> Self {
        Self {
            host: host.to_string(),
            published_at_ms: None,
            device_count: 0,
            problem_count: 0,
            categories: Vec::new(),
        }
    }

    /// Whether this host has published an inventory (an expandable branch) — an
    /// absent host is a non-expandable leaf.
    const fn is_published(&self) -> bool {
        self.published_at_ms.is_some()
    }
}

/// The whole By-node cross-fleet tree: every host as a top-level branch, ranked
/// problem-hosts-first (DEVMGR-10, #3).
struct NodeTree {
    /// The host branches — problem hosts first, then clean, then absent, each
    /// tier stable-sorted (see [`node_order`]).
    hosts: Vec<NodeHost>,
}

impl NodeTree {
    /// The published host keys (Expand-all fills these in By-node mode) — an
    /// absent host is a leaf with nothing to expand, so it is skipped.
    fn host_keys(&self) -> BTreeSet<String> {
        self.hosts
            .iter()
            .filter(|h| h.is_published())
            .map(|h| node_key(&h.host))
            .collect()
    }
}

/// The By-node ranking (DEVMGR-10, #3): **problem hosts near the top** so a fleet
/// scan surfaces faults first. Present hosts rank above absent ones (an absent
/// host has no hardware to scan); among present hosts a host with any problem
/// device ranks above a clean one, and more problems rank higher; ties (and the
/// absent tier) break alphabetically for a stable order. Pure, so the ranking is
/// unit-tested without a render.
fn node_order(a: &NodeHost, b: &NodeHost) -> std::cmp::Ordering {
    // Present (false) sorts before absent (true) — nothing to scan on an absent host.
    (!a.is_published())
        .cmp(&!b.is_published())
        // A host with problems (false for problem==0) sorts before a clean one.
        .then_with(|| (a.problem_count == 0).cmp(&(b.problem_count == 0)))
        // Among problem hosts, more problems rank higher.
        .then_with(|| b.problem_count.cmp(&a.problem_count))
        // Stable alphabetical within a tier.
        .then_with(|| a.host.cmp(&b.host))
}

/// Build the By-node cross-fleet tree from every published inventory (DEVMGR-10):
/// each host becomes a top-level [`NodeHost`] carrying its own device tree, with
/// the local "you are here" node always present (even if it has published nothing
/// — an honest absent leaf, mirroring the rail), and the whole set ranked
/// problem-hosts-first ([`node_order`]). `all` arrives already sorted by host
/// ([`device_inventory::read_all`]), so the rank is a stable re-order. Pure over
/// its inputs, so the aggregation + ranking is unit-tested without a substrate.
fn build_node_tree(all: &[DeviceInventory], local: &str) -> NodeTree {
    let mut hosts: Vec<NodeHost> = all.iter().map(NodeHost::from_inventory).collect();
    if !hosts.iter().any(|h| h.host == local) {
        hosts.push(NodeHost::absent(local));
    }
    hosts.sort_by(node_order);
    NodeTree { hosts }
}

/// A compact cross-fleet summary above the By-node tree (DEVMGR-10) — the fleet
/// twin of the By-type/By-connection header card (#20): the host count, how many
/// are faulted, and the aggregate device + problem totals, all off real state
/// (§7 — never a fabricated figure).
fn fleet_header(ui: &mut egui::Ui, tree: &NodeTree) {
    let hosts = tree.hosts.len();
    let published = tree.hosts.iter().filter(|h| h.is_published()).count();
    let problem_hosts = tree.hosts.iter().filter(|h| h.problem_count > 0).count();
    let devices: usize = tree.hosts.iter().map(|h| h.device_count).sum();
    let problems: usize = tree.hosts.iter().map(|h| h.problem_count).sum();
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Cross-fleet")
                    .color(Style::TEXT_STRONG)
                    .size(Style::TITLE)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            muted_note(
                ui,
                format!(
                    "{published} of {hosts} {} \u{00B7} {devices} {}",
                    plural(hosts, "host", "hosts"),
                    plural(devices, "device", "devices"),
                ),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if problems > 0 {
                    ui.colored_label(
                        Style::DANGER,
                        RichText::new(format!(
                            "\u{26A0} {problems} on {problem_hosts} {}", // ⚠
                            plural(problem_hosts, "host", "hosts"),
                        ))
                        .size(Style::SMALL),
                    );
                } else {
                    ui.colored_label(
                        Style::OK,
                        RichText::new("All devices OK across the fleet").size(Style::SMALL),
                    );
                }
            });
        });
    });
}

/// One host branch of the By-node tree — a forced-state collapsing header (its
/// open/closed driven by the caller's expand set) whose device rows nest beneath
/// it, sub-grouped by category. The header names the host, folds in its device
/// count + a `⚠ N` problem badge, tints amber when faulted / accent when it is the
/// rail-selected host, and flags a stale snapshot. An absent host is an honest dim
/// leaf ([`node_absent_row`]) — no header, no device tree (§7). Reuses
/// [`device_row`] + [`DeviceSelection`] verbatim so a device behaves identically
/// across all three views.
fn node_host_header(
    ui: &mut egui::Ui,
    host: &NodeHost,
    open: bool,
    selected: Option<&DeviceSelection>,
    is_selected_host: bool,
    now_ms: u64,
) -> CategoryOutcome {
    // An absent host is a leaf — nothing published, nothing to expand (§7).
    if !host.is_published() {
        node_absent_row(ui, host, is_selected_host);
        return CategoryOutcome {
            header_clicked: false,
            selected: None,
            action: None,
        };
    }
    let problems = host.problem_count;
    let tone = if is_selected_host {
        Style::ACCENT
    } else if problems > 0 {
        Style::WARN
    } else {
        Style::TEXT
    };
    let mut title = host.host.clone();
    {
        use std::fmt::Write as _;
        let _ = write!(
            title,
            "   {} {}",
            host.device_count,
            plural(host.device_count, "device", "devices")
        );
        if problems > 0 {
            let _ = write!(title, "   \u{26A0} {problems}"); // ⚠ N
        }
        if host_freshness(host.published_at_ms, now_ms) == HostFreshness::Stale {
            let _ = write!(title, "   \u{00B7} stale"); // ·
        }
    }
    let mut header = RichText::new(title).color(tone).size(Style::BODY);
    if is_selected_host {
        header = header.strong();
    }
    let mut clicked: Option<DeviceSelection> = None;
    let mut action: Option<RowActionRequest> = None;
    let resp = egui::CollapsingHeader::new(header)
        .id_salt(("dm-node", host.host.as_str()))
        .open(Some(open))
        .show(ui, |ui| {
            for cat in &host.categories {
                node_category_caption(ui, cat);
                for dev in &cat.devices {
                    // Only the rail-selected host's devices highlight (a device with
                    // the same key on a different host must not read as selected).
                    let is_sel =
                        is_selected_host && selected.is_some_and(|s| s.matches(&cat.key, dev));
                    // By-node flattens published MESH-NODE inventories only
                    // (DEVMGR-10), so the DEVMGR-8 verbs are always live here.
                    let out = device_row(ui, dev, &cat.key, is_sel, true);
                    if out.clicked {
                        clicked = Some(DeviceSelection::of(&cat.key, dev));
                    }
                    if let Some(req) = out.action {
                        action = Some(req);
                    }
                }
            }
        });
    CategoryOutcome {
        header_clicked: resp.header_response.clicked(),
        selected: clicked,
        action,
    }
}

/// A non-collapsible category sub-heading within a By-node host branch — the
/// lightweight grouping caption (host → category → device) that keeps the single
/// collapsible tier at the HOST level (so Expand-all is host-keyed). Dim by
/// default, amber with a `⚠ N` count when the category holds a problem device.
fn node_category_caption(ui: &mut egui::Ui, cat: &DeviceCategory) {
    let problems = cat.problem_count();
    let mut label = cat.label.clone();
    if problems > 0 {
        use std::fmt::Write as _;
        let _ = write!(label, "  \u{26A0} {problems}"); // ⚠ N
    }
    let tone = if problems > 0 {
        Style::WARN
    } else {
        Style::TEXT_DIM
    };
    ui.label(RichText::new(label).color(tone).size(Style::SMALL).strong());
}

/// An absent host leaf in the By-node tree — a dim status dot, the hostname, and
/// an honest "no inventory published" note (§7 — never a fabricated device tree).
/// Accent-tinted when it is the rail-selected host, mirroring the header branch.
fn node_absent_row(ui: &mut egui::Ui, host: &NodeHost, is_selected_host: bool) {
    ui.horizontal(|ui| {
        status_dot(ui, Style::TEXT_DIM);
        ui.add_space(Style::SP_XS);
        let tone = if is_selected_host {
            Style::ACCENT
        } else {
            Style::TEXT_DIM
        };
        ui.label(RichText::new(&host.host).color(tone).size(Style::BODY));
        ui.add_space(Style::SP_XS);
        muted_note(ui, "\u{2014} no inventory published"); // — no inventory published
    });
}

// ───────────────────── device actions (DEVMGR-7, #12) ───────────────────────

/// The **honest, read-only** subset of MDM's per-device action verbs this
/// inventory panel can perform (DEVMGR-7, #12) — offered as a right-click context
/// menu on a device row. This surface is a §6 consumer of the published inventory
/// JSON and holds no privileged-exec / worker-request seam, so MDM's
/// hardware-mutating verbs (Enable/Disable, Reload kernel module) are **omitted,
/// not greyed** (§7/§8) — they belong to the mesh-side producer on the node. `Copy`
/// so the static action table can hold it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceAction {
    /// Open the device's property sheet (the DEVMGR-3 detail drawer).
    Properties,
    /// Re-read the inventory (the honest rescan — the same seam as Action → Scan).
    Scan,
    /// Copy the full device detail dump to the seat clipboard.
    CopyDetails,
}

impl DeviceAction {
    /// The honest action set the context menu offers, in MDM order. Deliberately
    /// omits Enable/Disable + Reload module (no honest seam from a read-only
    /// consumer, §7/§8) — asserted by the DEVMGR-7 action-set test.
    const ALL: [Self; 3] = [Self::Properties, Self::Scan, Self::CopyDetails];

    /// The menu-item label (glyph + verb), in the shell's context-menu idiom.
    const fn label(self) -> &'static str {
        match self {
            Self::Properties => "\u{2699}  Properties",           // ⚙
            Self::Scan => "\u{21BB}  Scan for hardware changes",  // ↻
            Self::CopyDetails => "\u{29C9}  Copy device details", // ⧉
        }
    }
}

/// A resolved DEVMGR-7 action request bubbled up from a device-row context menu to
/// [`DeviceManagerState::apply_row_action`], already carrying the payload the seam
/// needs (the selection to open, or the record to copy) so the state applies it
/// after the immutable inventory borrow ends (as with row selection / header
/// toggles).
#[derive(Debug, Clone)]
enum RowActionRequest {
    /// Open the property sheet for this device (its stable selection key).
    Properties(DeviceSelection),
    /// Re-read the inventory (no per-device payload — the honest rescan).
    Scan,
    /// Copy this device's full detail dump to the clipboard. The record is boxed so
    /// this large payload never bloats the small [`Properties`](Self::Properties) /
    /// [`Scan`](Self::Scan) variants (`clippy::large_enum_variant`).
    CopyDetails(Box<DeviceRecord>),
    /// DEVMGR-8 — a **privileged** device op (Enable/Disable/Reload-module/Rescan-bus)
    /// the operator chose. It does NOT fire directly: it opens the typed-arming stage
    /// (#14), and only the echoed confirm dispatches the request to the target node's
    /// mackesd. Boxed for the same [`large_enum_variant`](clippy::large_enum_variant)
    /// reason as `CopyDetails`.
    Control {
        /// The op to arm.
        op: DeviceControlOp,
        /// The typed device target (name/category/sysfs/driver) resolved at menu time.
        target: Box<DeviceTarget>,
    },
}

/// What a device row reports back for one frame (DEVMGR-7): a left-click selection
/// (open/toggle its detail drawer) and/or a context-menu action the operator chose,
/// both applied after the read borrow ends.
struct RowOutcome {
    /// The row was left-clicked (open/toggle the detail drawer).
    clicked: bool,
    /// A context-menu action the operator chose on this device (DEVMGR-7).
    action: Option<RowActionRequest>,
}

/// A pending DEVMGR-8 typed-arming confirm (#14): a privileged device op staged
/// on a device + a target host, awaiting the operator's echo of the device name.
/// Held in [`DeviceManagerState::arming`] and rendered by [`DeviceManagerState::render_arming`].
struct DeviceArming {
    /// The privileged op to run once armed.
    op: DeviceControlOp,
    /// The device the op targets (name/category/sysfs/driver).
    target: DeviceTarget,
    /// The RAIL-selected host the request routes to (DEVMGR-4 governs).
    target_host: String,
    /// The operator's live echo — must equal `target.name` to arm.
    typed: String,
}

/// The typed-arming gate (#14): the trimmed echo must exactly equal the device
/// name before a privileged op may dispatch. The single decision the confirm
/// button + the tests share, so "unconfirmed ⇒ blocked" is proven without a render.
fn device_armed(typed: &str, device_name: &str) -> bool {
    typed.trim() == device_name
}

/// A process-unique request id for a dispatched device op (the correlation key the
/// node writes its result back under). Millis + a monotonic per-process counter, so
/// two rapid dispatches never collide — no ULID dependency needed for a file id.
fn next_request_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{seq}", now_ms())
}

/// The device-row right-click context menu (DEVMGR-7 + DEVMGR-8, #12) — the full
/// MDM action set for one device. The **read-only** verbs fire directly: **Properties**
/// (open the drawer), **Scan for hardware changes** (re-read), **Copy device details**
/// (clipboard). Below a separator, the **privileged** verbs (DEVMGR-8) — **Enable**,
/// **Disable**, **Reload driver module**, **Rescan bus** — are now PRESENT (the
/// node-side exec seam exists): each opens the typed-arming stage (#14) and dispatches
/// to the selected host's mackesd only on the echoed confirm, tinted [`Style::DANGER`]
/// as destructive. Returns the chosen [`RowActionRequest`] (the caller dispatches it).
fn device_context_menu(
    ui: &mut egui::Ui,
    category: &str,
    dev: &DeviceRecord,
    allow_control: bool,
) -> Option<RowActionRequest> {
    let mut chosen: Option<RowActionRequest> = None;
    for action in DeviceAction::ALL {
        if ui
            .button(RichText::new(action.label()).color(Style::TEXT))
            .clicked()
        {
            chosen = Some(match action {
                DeviceAction::Properties => {
                    RowActionRequest::Properties(DeviceSelection::of(category, dev))
                }
                DeviceAction::Scan => RowActionRequest::Scan,
                DeviceAction::CopyDetails => RowActionRequest::CopyDetails(Box::new(dev.clone())),
            });
            ui.close_menu();
        }
    }
    ui.separator();
    if allow_control {
        // DEVMGR-8 — the privileged, node-side verbs (#12/#13/#14). Each arms first
        // (type the device name) and dispatches to the RAIL-selected host's mackesd.
        for op in DeviceControlOp::ALL {
            if ui
                .button(RichText::new(control_label(op)).color(Style::DANGER))
                .clicked()
            {
                chosen = Some(RowActionRequest::Control {
                    op,
                    target: Box::new(device_target(category, dev)),
                });
                ui.close_menu();
            }
        }
        ui.separator();
        // Honest disclosure (§13): these run on the node itself, over the overlay.
        muted_note(
            ui,
            "Enable/Disable, reload + rescan run on the node \u{2014} armed, audited.",
        );
    } else {
        // DEVMGR-11 (§7) — a non-PC host (instance / phone / LAN / router) runs no
        // mackesd device_control worker, so the privileged verbs are honestly
        // ABSENT, not greyed placebos, with the reason disclosed.
        muted_note(
            ui,
            "Enable/Disable + driver ops apply to mesh nodes only \u{2014} this host \
             runs no mesh device-control worker.",
        );
    }
    chosen
}

/// The context-menu glyph + verb for a privileged [`DeviceControlOp`] (DEVMGR-8),
/// in the shell's context-menu idiom (a glyph, two spaces, the verb).
const fn control_label(op: DeviceControlOp) -> &'static str {
    match op {
        DeviceControlOp::Enable => "\u{25B6}  Enable device", // ▶
        DeviceControlOp::Disable => "\u{25A0}  Disable device", // ■
        DeviceControlOp::ReloadModule => "\u{21BB}  Reload driver module", // ↻
        DeviceControlOp::RescanBus => "\u{2921}  Rescan bus", // ⤡
    }
}

/// The typed [`DeviceTarget`] for a device row — the subset of the record the
/// node-side executor needs to resolve the real seam (§9 — typed params, no command).
fn device_target(category: &str, dev: &DeviceRecord) -> DeviceTarget {
    DeviceTarget {
        name: dev.name.clone(),
        category: category.to_string(),
        sysfs_path: dev.sysfs_path.clone(),
        driver: dev.driver.clone(),
    }
}

/// One device row — a clickable selection row (DEVMGR-3) carrying the DEVMGR-7
/// right-click action menu: a status dot in the device's [`status_tone`], the name
/// (accent-tinted when selected), the MDM **problem-code badge** for a faulted
/// device (#11), and the honest Linux reason from the schema, dimmed. Returns a
/// [`RowOutcome`] — a left-click selection (open/toggle the drawer) and any
/// context-menu action the operator chose for this device.
fn device_row(
    ui: &mut egui::Ui,
    dev: &DeviceRecord,
    category: &str,
    selected: bool,
    allow_control: bool,
) -> RowOutcome {
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
    // selection target (the MDM "click a device to inspect it" affordance) that also
    // carries the DEVMGR-7 right-click action menu.
    let resp = inner
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut action: Option<RowActionRequest> = None;
    resp.context_menu(|ui| action = device_context_menu(ui, category, dev, allow_control));
    // a11y-05 — the row's accesskit node (name + the MDM status reading), keyed
    // by the strip response id. Pure metadata over the raw-painted row.
    install_row_accessibility(
        ui.ctx(),
        resp.id,
        device_a11y_label(dev),
        device_a11y_value(dev),
        resp.rect,
    );
    RowOutcome {
        clicked: resp.clicked(),
        action,
    }
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
            let mut name = RichText::new(&entry.label)
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
    let resp = resp
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(host_hover(entry, now_ms));
    // a11y-05 — the rail row's accesskit node (host name + freshness/counts),
    // keyed by the strip response id.
    install_row_accessibility(
        ui.ctx(),
        resp.id,
        &entry.label,
        host_a11y_value(entry, now_ms),
        resp.rect,
    );
    resp.clicked()
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

// ── accesskit (a11y-05 / shell-ux-6) ─────────────────────────────────────────
//
// The Device Manager's two selection rows are hand-rolled: [`device_row`] and
// [`host_row`] each lay out plain `ui.label`s (which don't sense clicks) and
// then `.interact(Sense::click())` the whole strip as one target — so egui
// auto-generates no accesskit node for either (the same raw-cell gap dock.rs /
// console.rs closed under WIN7-5/WIN7-7). A screen reader walking the tree /
// rail heard nothing. This section gives each row a `Role::Button` node keyed
// by the strip response's id, with the device/host name as the accessible label
// and its MDM status / freshness reading as the value — the established
// `install_row_accessibility` idiom (role + label + value + bounds + Click),
// reusing [`device_status_display`] so the a11y value can never drift from the
// painted status.

/// Convert an egui rect to an accesskit one (the `console.rs`/`dock.rs` helper,
/// restated module-locally — the established per-module-copy idiom).
fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The accessible **name** of a device row — the device's display name (the same
/// bold string the row paints).
fn device_a11y_label(dev: &DeviceRecord) -> String {
    dev.name.clone()
}

/// The accessible **state/value** of a device row — the exact MDM status text
/// the drawer shows ([`device_status_display`]): the problem code + honest Linux
/// reason for a faulted device, the "working properly" line for a healthy one,
/// or the honest "could not be determined" otherwise. Reuses the shared display
/// so the spoken status can never drift from the painted one.
fn device_a11y_value(dev: &DeviceRecord) -> String {
    device_status_display(dev).0
}

/// The freshness word a rail row reads out — the spoken counterpart of the
/// host dot's dim/amber/green tone ([`host_dot_tone`]).
const fn freshness_word(fresh: HostFreshness) -> &'static str {
    match fresh {
        HostFreshness::Fresh => "live",
        HostFreshness::Stale => "stale",
        HostFreshness::Absent => "offline",
    }
}

/// The accessible **value** of a rail host row — its freshness plus the device /
/// problem counts, mirroring the [`host_hover`] summary in one spoken line; an
/// absent host reads the honest "nothing published" (§7).
fn host_a11y_value(entry: &HostEntry, now_ms: u64) -> String {
    let fresh = entry.freshness(now_ms);
    if fresh == HostFreshness::Absent {
        return "offline \u{00B7} nothing published".to_string();
    }
    let mut parts = vec![
        freshness_word(fresh).to_owned(),
        format!(
            "{} {}",
            entry.device_count,
            plural(entry.device_count, "device", "devices")
        ),
    ];
    if entry.problem_count > 0 {
        parts.push(format!(
            "{} {}",
            entry.problem_count,
            plural(entry.problem_count, "problem", "problems")
        ));
    }
    parts.join(" \u{00B7} ")
}

/// Install one raw-painted selection row's accesskit `Button` node, keyed by the
/// strip response's own id so egui merges it onto the cell (the dock.rs id-keyed
/// merge). Shared by the device row + the host rail row.
fn install_row_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    label: impl Into<String>,
    value: impl Into<String>,
    rect: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label.into());
        node.set_value(value.into());
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

mod drawer;
use drawer::*;

#[cfg(test)]
mod tests;

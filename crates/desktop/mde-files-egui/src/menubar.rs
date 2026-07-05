//! MENUBAR-ALL (Files) — the **shared top menu bar** across the file-manager
//! surface (design: `docs/design/menubar-all.md`).
//!
//! The Files surface already carries every capability behind the toolbar, the
//! sidebar, right-click menus, and a handful of keyboard chords; this is the
//! *discoverable* face over them — hosted on the shared
//! [`mde_egui::menubar::MenuBar`] (MENUBAR-ALL-1) under one UPPERCASE
//! accent title (`FILES`, the dock's System-group gold accent). Every menu item
//! is the **mouse twin of an existing seam** ([`crate::view::Action`] → the
//! model's shipped methods, §6 one dispatch path), never a new behaviour and
//! never a stub. Per the governing principle (§7 — comprehensive but no dead
//! entries): an item whose feature is genuinely missing is *omitted*, and an
//! item that needs context (Copy with no selection, Unmount with no live mount)
//! renders **disabled**, never a silent no-op.
//!
//! The menus and the seam each drives:
//!
//! * **File** — New Tab / Close Tab ([`FileBrowser::new_tab`]/[`close_tab`]),
//!   Refresh ([`FileBrowser::reload_all`] + roster).
//! * **Edit** — Copy / Cut / Paste (the FILEMGR-12 shell clipboard,
//!   [`clip_copy`]/[`clip_cut`]/[`clip_paste`]), Select All / Clear Selection,
//!   Delete ([`request_delete`], the FILEMGR-11 typed-arming confirm), Properties
//!   ([`open_properties`], the rwx/octal/owner dialog).
//! * **View** — Layout (List/Grid/Details, [`set_view`]), Sort By (the four
//!   [`SortKey`]s + Directories-First, [`sort_by`]/[`toggle_dirs_first`]), Show
//!   Hidden, Dual Pane, Preview Pane, List Thumbnails, Quick Look, Search — the
//!   FILEMGR-4/9/10 toggles, each a live checkmark.
//! * **Go** (Files-specific — places + mesh) — Back / Forward / Up, the fixed
//!   local [`LOCAL_SPOTS`] shortcuts, and one submenu per reachable mesh peer
//!   (Open / Full-Filesystem escalation / Unmount — the FILEMGR-9 mount verbs).
//! * **Share** (Files-specific — the mesh transfer surface) — Send to the chosen
//!   destination ([`send`]), Set Destination / Send to Peer / Send in Chat per
//!   reachable peer (FILEMGR-7/12), and Open in Editor (EDITOR-9).
//! * **Help** — the keyboard-shortcuts reference, the real fixed chords the
//!   surface handles.
//!
//! Each item's shortcut renders beside it **only when a real chord exists** (the
//! surface's fixed widget chords — Ctrl+C/X/V, Ctrl+A, Ctrl+H, Space, Del, Esc);
//! an action with no binding shows none (§7 — never a fabricated chord). The
//! status cluster carries the surface's real live indicators — the current path,
//! the item + selection counts, a live transfer count, and the mesh presence.
//!
//! **Honestly omitted** (no landed seam → no dead entry): **New Folder** and
//! **Rename** (the model has no mkdir/rename op — only copy/move/delete), a
//! **Quit** (Files is an embedded dock surface with no independent window to
//! close — the dock owns its lifecycle, lock 10), and a **free-space** status
//! chip (the workspace bans `statvfs`, and a `df` probe is a blocking subprocess
//! that has no place on the paint path — so the value has no honest non-blocking
//! seam here, and it is omitted rather than faked).
//!
//! §4: the shared [`MenuBar`](mde_egui::menubar::MenuBar) renders through the
//! Carbon [`Style`] install — no forced colours, so egui's disabled dimming reads
//! correctly. The surface builds the menu **model** each frame ([`build_menus`])
//! from a pure [`FilesCtx`] snapshot ([`snapshot`]) and dispatches the activated
//! [`Picked`] id through the existing [`crate::view::Action`] pipeline, so every
//! seam + gate + shortcut is preserved without a new behaviour path.

use mde_egui::egui::{self, Context, RichText};
use mde_egui::menubar::{Entry, Item as BarItem, Menu, StatusChip};
use mde_egui::{ChipTone, Style};

use crate::model::{mount_host_of, FileBrowser, SortKey, SurfaceTab, ViewMode, LOCAL_SPOTS};
use crate::transfers::{LedgerCounts, Method, StateFilter};
use crate::view::Action;

/// The bar's menu titles, left to right — the shared File/Edit/View/Help spine
/// with the two Files-specific menus (Go = places + mesh navigation, Share = the
/// mesh transfer surface) slotted between View and Help. The order authority the
/// menu-order test checks [`build_menus`] against (test-only — the render builds
/// each [`Menu`] with its own title, so nothing outside the test reads this).
#[cfg(test)]
const MENU_TITLES: [&str; 7] = ["File", "Edit", "View", "Go", "Share", "Transfers", "Help"];

/// The egui-memory id the Help → Keyboard-Shortcuts reference window's open flag
/// lives under, so [`files_panel`](crate::view::files_panel) stays stateless (it
/// owns only the `FileBrowser`; the bar keeps its one bit of chrome state here).
const SHORTCUTS_OPEN: &str = "files-menubar-shortcuts-open";

/// The egui-memory id the TRANSFERS-8 Destinations manager window's open flag
/// lives under (the bar owns this chrome bit; the render lives in
/// [`crate::view`], which has the `FileBrowser` to list the live targets).
const DESTINATIONS_OPEN: &str = "files-menubar-destinations-open";

// ─────────────────────────────── the activation id ──────────────────────────

/// One menu activation, mapped to a real seam by [`to_action`] (§7 — no dead
/// entries). Every peer/host-bearing variant carries an owned id so the model is
/// built from a borrow-free snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Picked {
    /// Open a fresh tab in the active pane ([`FileBrowser::new_tab`]).
    NewTab,
    /// Close the active pane's active tab ([`FileBrowser::close_tab`]).
    CloseTab,
    /// Reload every pane + refresh the roster ([`FileBrowser::reload_all`]).
    Refresh,
    /// Copy the selection's paths to the shared clipboard ([`clip_copy`]).
    Copy,
    /// Cut the selection's paths ([`clip_cut`]).
    Cut,
    /// Paste the clipboard set into the active directory ([`clip_paste`]).
    Paste,
    /// Select every row ([`select_all`]).
    SelectAll,
    /// Drop the selection ([`clear_selection`]).
    ClearSelection,
    /// Open the permanent-delete confirm ([`request_delete`]).
    Delete,
    /// Open the Properties dialog ([`open_properties`]).
    Properties,
    /// Set the listing layout ([`set_view`]).
    SetView(ViewMode),
    /// Sort on this column ([`sort_by`]).
    SortBy(SortKey),
    /// Toggle directories-first grouping ([`toggle_dirs_first`]).
    ToggleDirsFirst,
    /// Toggle hidden (dot) entries ([`toggle_hidden`]).
    ToggleHidden,
    /// Toggle the second pane ([`toggle_dual`]).
    ToggleDual,
    /// Toggle the right-hand preview pane ([`toggle_preview_pane`]).
    TogglePreview,
    /// Toggle List-view thumbnails ([`toggle_list_thumbs`]).
    ToggleThumbs,
    /// Toggle the quick-look overlay ([`toggle_quick_look`]).
    QuickLook,
    /// Toggle the recursive-search bar ([`toggle_search_bar`]).
    ToggleSearch,
    /// History back ([`go_back`]).
    Back,
    /// History forward ([`go_forward`]).
    Forward,
    /// Up one level ([`go_up`]).
    Up,
    /// Navigate to a fixed local place ([`LOCAL_SPOTS`] slug).
    Place(&'static str),
    /// Mount + browse a mesh peer ([`open_peer`]), keyed by its mount host.
    MountPeer(String),
    /// Escalate a peer to full-filesystem access ([`escalate_peer`]).
    Escalate(String),
    /// Unmount a peer ([`unmount_peer`]).
    Unmount(String),
    /// Send to the chosen destination ([`send`]).
    Send,
    /// Choose the Send-To destination peer ([`set_destination`]).
    SetDestination(String),
    /// Send the selection to a peer over the mesh ([`send_to_peer`]).
    SendToPeer(String),
    /// Send the selection to a peer *and* drop a chat card ([`send_in_chat`]).
    SendInChat(String),
    /// Open the focused local file in the Editor surface ([`send_to_editor`]).
    OpenInEditor,
    /// TRANSFERS-8 — switch the top-level surface (Files ↔ Transfers, Q1).
    ShowSurface(SurfaceTab),
    /// TRANSFERS-8 — open the New Transfer dialog ([`open_new_transfer`], Q13).
    NewTransfer,
    /// TRANSFERS-8 — pause every pausable job ([`transfer_pause_all`]).
    TransferPauseAll,
    /// TRANSFERS-8 — resume every Paused job ([`transfer_resume_all`]).
    TransferResumeAll,
    /// TRANSFERS-8 — cancel every terminal job ([`transfer_clear_completed`]).
    TransferClearCompleted,
    /// TRANSFERS-8 — filter the Transfers view by state ([`set_transfers_filter`]).
    TransferStateFilter(StateFilter),
    /// TRANSFERS-8 — filter the Transfers view by method (`None` = all lanes).
    TransferMethodFilter(Option<Method>),
    /// Toggle the keyboard-shortcuts reference (owned by the bar, not the model).
    ShowShortcuts,
    /// TRANSFERS-8 — toggle the Destinations manager window (bar-owned chrome, Q16).
    ShowDestinations,
}

/// Map a [`Picked`] to its real [`Action`] seam, or `None` for
/// [`Picked::ShowShortcuts`] (the bar's own chrome, handled in [`crate::view`]).
/// The single 1:1 glue table (§6) — the render host changed, no seam did.
#[must_use]
pub(crate) fn to_action(picked: Picked, cx: &FilesCtx) -> Option<Action> {
    let p = cx.active;
    Some(match picked {
        Picked::NewTab => Action::NewTab(p),
        Picked::CloseTab => Action::CloseTab(p, cx.tab_ix),
        Picked::Refresh => Action::Refresh,
        Picked::Copy => Action::ClipCopy(p),
        Picked::Cut => Action::ClipCut(p),
        Picked::Paste => Action::ClipPaste(p),
        Picked::SelectAll => Action::SelectAll(p),
        Picked::ClearSelection => Action::ClearSelection(p),
        Picked::Delete => Action::RequestDelete(p),
        Picked::Properties => Action::OpenProperties(p),
        Picked::SetView(m) => Action::SetView(p, m),
        Picked::SortBy(k) => Action::SortBy(p, k),
        Picked::ToggleDirsFirst => Action::ToggleDirsFirst(p),
        Picked::ToggleHidden => Action::ToggleHidden(p),
        Picked::ToggleDual => Action::ToggleDual,
        Picked::TogglePreview => Action::TogglePreviewPane,
        Picked::ToggleThumbs => Action::ToggleListThumbs,
        Picked::QuickLook => Action::ToggleQuickLook,
        Picked::ToggleSearch => Action::ToggleSearchBar,
        Picked::Back => Action::Back(p),
        Picked::Forward => Action::Forward(p),
        Picked::Up => Action::Up(p),
        Picked::Place(slug) => Action::Navigate(p, crate::model::Location::Local(slug.to_string())),
        Picked::MountPeer(host) => Action::MountPeer(p, host),
        Picked::Escalate(host) => Action::EscalatePeer(host),
        Picked::Unmount(host) => Action::UnmountPeer(host),
        Picked::Send => Action::Send,
        Picked::SetDestination(id) => Action::SetDestination(id),
        Picked::SendToPeer(id) => Action::SendToPeer(p, id),
        Picked::SendInChat(id) => Action::SendInChat(p, id),
        Picked::OpenInEditor => Action::SendToEditor(p),
        Picked::ShowSurface(tab) => Action::SwitchSurface(tab),
        Picked::NewTransfer => Action::OpenNewTransfer,
        Picked::TransferPauseAll => Action::TransferPauseAll,
        Picked::TransferResumeAll => Action::TransferResumeAll,
        Picked::TransferClearCompleted => Action::TransferClearCompleted,
        Picked::TransferStateFilter(s) => Action::SetTransferStateFilter(s),
        Picked::TransferMethodFilter(m) => Action::SetTransferMethodFilter(m),
        // Bar-owned windows (toggle chrome), not a model seam.
        Picked::ShowShortcuts | Picked::ShowDestinations => return None,
    })
}

// ─────────────────────────────── the frame snapshot ─────────────────────────

/// One reachable mesh peer, flattened for the borrow-free menu model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerLite {
    /// The peer's routing id (Send-To / destination key).
    pub id: String,
    /// The peer's hostname — the menu label + the chat contact.
    pub host: String,
    /// The short mount host (`open`/`escalate`/`unmount` key).
    pub mount_host: String,
    /// Whether a live mount exists (gates Unmount).
    pub mounted: bool,
    /// Whether the live mount is full-filesystem (the Full-FS checkmark).
    pub is_full: bool,
}

/// The per-frame surface-state snapshot the bar builds its model + gating +
/// checkmarks from — read once up front so the render holds no borrow of the
/// model and the whole menu tree is unit-testable without egui.
///
/// The many `bool`s are independent, orthogonal surface toggles + gates (each a
/// distinct menu item's enable/check state); folding them into sub-structs would
/// be artificial ceremony over what is deliberately a flat, one-field-per-menu-
/// item read model — so this ONE snapshot allows `struct_excessive_bools`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilesCtx {
    /// The active pane index (every pane-scoped action targets it).
    pub active: usize,
    /// The active pane's active-tab index (Close Tab's target).
    pub tab_ix: usize,
    /// Tabs open in the active pane (Close Tab's gate).
    pub tab_count: usize,
    /// Rows in the active tab's listing (Select All's gate + the item chip).
    pub rows: usize,
    /// Rows currently selected (the selection chip + Clear-Selection's gate).
    pub selected: usize,
    /// Any row selected (local or virtual).
    pub has_selection: bool,
    /// The selection carries at least one **local** path (Copy/Cut/Delete/
    /// Send-To's gate — a virtual peer row is not a filesystem-op source).
    pub has_local_selection: bool,
    /// The focused row is a local file (Properties / Open-in-Editor's gate).
    pub has_focused_local: bool,
    /// History back is available.
    pub can_back: bool,
    /// History forward is available.
    pub can_forward: bool,
    /// The shared clipboard holds a paste-able set.
    pub can_paste: bool,
    /// A Send-To can fire right now (selection + reachable destination).
    pub can_send: bool,
    /// The active tab's layout (the Layout radio).
    pub view: ViewMode,
    /// The active tab's sort key (the Sort-By radio).
    pub sort_key: SortKey,
    /// The active tab groups directories first (the checkmark).
    pub dirs_first: bool,
    /// Hidden entries shown (the checkmark).
    pub show_hidden: bool,
    /// The second pane is shown (the checkmark).
    pub is_dual: bool,
    /// The preview pane is open (the checkmark).
    pub preview_open: bool,
    /// List-view thumbnails are on (the checkmark).
    pub list_thumbs: bool,
    /// The recursive-search bar is open (the checkmark).
    pub search_open: bool,
    /// This node is on a live mesh (the presence chip).
    pub on_mesh: bool,
    /// The chosen Send-To destination id, if any (the Set-Destination radio).
    pub dest: Option<String>,
    /// The active tab's backend path (the path chip).
    pub path: String,
    /// In-flight (not-yet-finished) op-queue transfers (the local-copy chip).
    pub running: usize,
    /// The reachable mesh peers (the Go + Share menus).
    pub peers: Vec<PeerLite>,
    /// TRANSFERS-8 — the Transfers tab is showing (the tab-switch checkmark).
    pub on_transfers_tab: bool,
    /// TRANSFERS-8 — the worker ledger tallies (the control-item gating + badge).
    pub transfer_counts: LedgerCounts,
    /// TRANSFERS-8 — the current Transfers view state-filter (the radio checkmark).
    pub transfer_state_filter: StateFilter,
    /// TRANSFERS-8 — the current Transfers view method-filter (`None` = all lanes).
    pub transfer_method_filter: Option<Method>,
}

/// Snapshot `browser` into a [`FilesCtx`] — the read half of a render frame.
#[must_use]
pub(crate) fn snapshot(b: &FileBrowser) -> FilesCtx {
    let active = b.active_pane_index();
    let pane = b.pane(active);
    let tab = pane.active_tab();
    let peers = b
        .reachable_destinations()
        .into_iter()
        .map(|peer| {
            let mount = b.peer_mount(peer);
            PeerLite {
                id: peer.id.clone(),
                host: peer.host.clone(),
                mount_host: mount_host_of(peer).to_string(),
                mounted: mount.is_some_and(|m| m.phase.is_mounted()),
                is_full: mount.is_some_and(crate::mesh_mount::MountView::is_full),
            }
        })
        .collect();
    let sort = tab.sort();
    FilesCtx {
        active,
        tab_ix: pane.active_tab_index(),
        tab_count: pane.tabs().len(),
        rows: tab.rows().len(),
        selected: tab.selection().len(),
        has_selection: !tab.selection().is_empty(),
        has_local_selection: !tab.selected_paths().is_empty(),
        has_focused_local: tab.focused_row().and_then(|r| r.path.as_ref()).is_some(),
        can_back: tab.can_back(),
        can_forward: tab.can_forward(),
        can_paste: b.can_paste(),
        can_send: b.can_send(),
        view: tab.view(),
        sort_key: sort.key,
        dirs_first: sort.dirs_first,
        show_hidden: tab.show_hidden(),
        is_dual: b.is_dual(),
        preview_open: b.preview_pane_open(),
        list_thumbs: b.list_thumbs(),
        search_open: b.search_form().open,
        on_mesh: b.mesh_overlay().is_some(),
        dest: b.destination().map(str::to_string),
        path: tab.location().backend_path(),
        running: b.ops().active().iter().filter(|o| !o.is_done()).count(),
        peers,
        on_transfers_tab: b.surface_tab() == SurfaceTab::Transfers,
        transfer_counts: b.transfers_counts(),
        transfer_state_filter: b.transfers_filter().state,
        transfer_method_filter: b.transfers_filter().method,
    }
}

// ─────────────────────────────── the menu model ─────────────────────────────

/// A plain enabled command item.
fn item(id: Picked, label: &str) -> Entry<Picked> {
    Entry::Item(BarItem::new(id, label))
}

/// The File drop-down.
fn file_menu(cx: &FilesCtx) -> Menu<Picked> {
    Menu::new(
        "File",
        vec![
            item(Picked::NewTab, "New Tab"),
            Entry::Item(BarItem::new(Picked::CloseTab, "Close Tab").enabled(cx.tab_count > 1)),
            Entry::Separator,
            item(Picked::Refresh, "Refresh"),
        ],
    )
}

/// The Edit drop-down — the FILEMGR-12 clipboard + FILEMGR-11 delete/properties,
/// each gated on a real precondition (§7 disable, not omit).
fn edit_menu(cx: &FilesCtx) -> Menu<Picked> {
    Menu::new(
        "Edit",
        vec![
            Entry::Item(
                BarItem::new(Picked::Copy, "Copy")
                    .shortcut("Ctrl+C")
                    .enabled(cx.has_local_selection),
            ),
            Entry::Item(
                BarItem::new(Picked::Cut, "Cut")
                    .shortcut("Ctrl+X")
                    .enabled(cx.has_local_selection),
            ),
            Entry::Item(
                BarItem::new(Picked::Paste, "Paste")
                    .shortcut("Ctrl+V")
                    .enabled(cx.can_paste),
            ),
            Entry::Separator,
            Entry::Item(
                BarItem::new(Picked::SelectAll, "Select All")
                    .shortcut("Ctrl+A")
                    .enabled(cx.rows > 0),
            ),
            Entry::Item(
                BarItem::new(Picked::ClearSelection, "Clear Selection")
                    .shortcut("Esc")
                    .enabled(cx.has_selection),
            ),
            Entry::Separator,
            Entry::Item(
                BarItem::new(Picked::Delete, "Delete\u{2026}")
                    .shortcut("Del")
                    .enabled(cx.has_local_selection),
            ),
            Entry::Separator,
            Entry::Item(
                BarItem::new(Picked::Properties, "Properties\u{2026}")
                    .enabled(cx.has_focused_local),
            ),
        ],
    )
}

/// The View drop-down — the Layout + Sort-By radios and the FILEMGR-4/9/10 toggles.
fn view_menu(cx: &FilesCtx) -> Menu<Picked> {
    let layout = Entry::Submenu {
        label: "Layout".to_owned(),
        mnemonic: None,
        entries: ViewMode::ALL
            .iter()
            .map(|&m| {
                Entry::Item(BarItem::new(Picked::SetView(m), m.label()).checked(cx.view == m))
            })
            .collect(),
    };
    let mut sort_entries: Vec<Entry<Picked>> = SortKey::ALL
        .iter()
        .map(|&k| Entry::Item(BarItem::new(Picked::SortBy(k), k.label()).checked(cx.sort_key == k)))
        .collect();
    sort_entries.push(Entry::Separator);
    sort_entries.push(Entry::Item(
        BarItem::new(Picked::ToggleDirsFirst, "Directories First").checked(cx.dirs_first),
    ));
    let sort_by = Entry::Submenu {
        label: "Sort By".to_owned(),
        mnemonic: None,
        entries: sort_entries,
    };
    Menu::new(
        "View",
        vec![
            layout,
            sort_by,
            Entry::Separator,
            Entry::Item(
                BarItem::new(Picked::ToggleHidden, "Show Hidden")
                    .shortcut("Ctrl+H")
                    .checked(cx.show_hidden),
            ),
            Entry::Item(BarItem::new(Picked::ToggleDual, "Dual Pane").checked(cx.is_dual)),
            Entry::Separator,
            Entry::Item(
                BarItem::new(Picked::TogglePreview, "Preview Pane").checked(cx.preview_open),
            ),
            Entry::Item(
                BarItem::new(Picked::ToggleThumbs, "List Thumbnails").checked(cx.list_thumbs),
            ),
            Entry::Item(BarItem::new(Picked::QuickLook, "Quick Look").shortcut("Space")),
            Entry::Separator,
            Entry::Item(BarItem::new(Picked::ToggleSearch, "Search").checked(cx.search_open)),
        ],
    )
}

/// The Go drop-down (Files-specific) — history, the fixed local places, and one
/// submenu per reachable mesh peer carrying the FILEMGR-9 mount verbs.
fn go_menu(cx: &FilesCtx) -> Menu<Picked> {
    let mut entries = vec![
        Entry::Item(BarItem::new(Picked::Back, "Back").enabled(cx.can_back)),
        Entry::Item(BarItem::new(Picked::Forward, "Forward").enabled(cx.can_forward)),
        item(Picked::Up, "Up One Level"),
        Entry::Separator,
        Entry::Caption("Places".to_owned()),
    ];
    for spot in LOCAL_SPOTS {
        entries.push(Entry::Item(
            BarItem::new(Picked::Place(spot.path), spot.label).checked(cx.path == spot.path),
        ));
    }
    entries.push(Entry::Separator);
    if cx.peers.is_empty() {
        entries.push(Entry::Caption("No mesh peers online".to_owned()));
    } else {
        entries.push(Entry::Caption("Mesh peers".to_owned()));
        for peer in &cx.peers {
            entries.push(Entry::Submenu {
                label: peer.host.clone(),
                mnemonic: None,
                entries: vec![
                    item(Picked::MountPeer(peer.mount_host.clone()), "Open"),
                    Entry::Item(
                        BarItem::new(
                            Picked::Escalate(peer.mount_host.clone()),
                            "Full Filesystem Access",
                        )
                        .checked(peer.is_full),
                    ),
                    Entry::Item(
                        BarItem::new(Picked::Unmount(peer.mount_host.clone()), "Unmount")
                            .enabled(peer.mounted),
                    ),
                ],
            });
        }
    }
    Menu::new("Go", entries)
}

/// The Share drop-down (Files-specific) — the mesh transfer surface: the toolbar
/// Send twin, per-peer Set-Destination / Send-to-Peer / Send-in-Chat, and the
/// Editor hand-off. Peerless is an honest caption, not an empty menu.
fn share_menu(cx: &FilesCtx) -> Menu<Picked> {
    let send_label = cx.dest.as_deref().map_or_else(
        || "Send Selection".to_owned(),
        |dest| format!("Send to {dest}"),
    );
    let mut entries = vec![
        Entry::Item(BarItem::new(Picked::Send, send_label).enabled(cx.can_send)),
        Entry::Separator,
    ];
    if cx.peers.is_empty() {
        entries.push(Entry::Caption("No mesh peers online".to_owned()));
    } else {
        entries.push(Entry::Submenu {
            label: "Set Destination".to_owned(),
            mnemonic: None,
            entries: cx
                .peers
                .iter()
                .map(|peer| {
                    Entry::Item(
                        BarItem::new(Picked::SetDestination(peer.id.clone()), peer.host.clone())
                            .checked(cx.dest.as_deref() == Some(peer.id.as_str())),
                    )
                })
                .collect(),
        });
        entries.push(Entry::Submenu {
            label: "Send to Peer".to_owned(),
            mnemonic: None,
            entries: cx
                .peers
                .iter()
                .map(|peer| {
                    Entry::Item(
                        BarItem::new(Picked::SendToPeer(peer.id.clone()), peer.host.clone())
                            .enabled(cx.has_local_selection),
                    )
                })
                .collect(),
        });
        entries.push(Entry::Submenu {
            label: "Send in Chat".to_owned(),
            mnemonic: None,
            entries: cx
                .peers
                .iter()
                .map(|peer| {
                    Entry::Item(
                        BarItem::new(Picked::SendInChat(peer.id.clone()), peer.host.clone())
                            .enabled(cx.has_local_selection),
                    )
                })
                .collect(),
        });
    }
    entries.push(Entry::Separator);
    entries.push(Entry::Item(
        BarItem::new(Picked::OpenInEditor, "Open in Editor").enabled(cx.has_focused_local),
    ));
    Menu::new("Share", entries)
}

/// The Transfers drop-down (Files-specific) — the TRANSFERS-8 tab's control
/// surface reused on the shared bar (Q16): the tab switch, the New Transfer entry
/// point, the batch lifecycle verbs (each gated on a real ledger tally, §7), the
/// View-by-state / -by-method filters, and the Destinations manager. The batch
/// items disable — never vanish — when the ledger has nothing to act on.
fn transfers_menu(cx: &FilesCtx) -> Menu<Picked> {
    let c = cx.transfer_counts;
    // View → state radios + method radios (Q16 — filter the live ledger).
    let mut view_entries: Vec<Entry<Picked>> = StateFilter::ALL
        .iter()
        .map(|&s| {
            Entry::Item(
                BarItem::new(Picked::TransferStateFilter(s), s.label())
                    .checked(cx.transfer_state_filter == s),
            )
        })
        .collect();
    view_entries.push(Entry::Separator);
    view_entries.push(Entry::Caption("Method".to_owned()));
    view_entries.push(Entry::Item(
        BarItem::new(Picked::TransferMethodFilter(None), "All Methods")
            .checked(cx.transfer_method_filter.is_none()),
    ));
    for m in Method::ALL {
        view_entries.push(Entry::Item(
            BarItem::new(Picked::TransferMethodFilter(Some(m)), m.label())
                .checked(cx.transfer_method_filter == Some(m)),
        ));
    }
    Menu::new(
        "Transfers",
        vec![
            Entry::Item(
                BarItem::new(Picked::ShowSurface(SurfaceTab::Transfers), "Transfers Tab")
                    .checked(cx.on_transfers_tab),
            ),
            Entry::Separator,
            item(Picked::NewTransfer, "New Transfer\u{2026}"),
            Entry::Separator,
            Entry::Item(
                BarItem::new(Picked::TransferPauseAll, "Pause All").enabled(c.pausable > 0),
            ),
            Entry::Item(
                BarItem::new(Picked::TransferResumeAll, "Resume All").enabled(c.resumable > 0),
            ),
            Entry::Item(
                BarItem::new(Picked::TransferClearCompleted, "Clear Completed")
                    .enabled(c.terminal > 0),
            ),
            Entry::Separator,
            Entry::Submenu {
                label: "View".to_owned(),
                mnemonic: None,
                entries: view_entries,
            },
            Entry::Separator,
            item(Picked::ShowDestinations, "Destinations\u{2026}"),
        ],
    )
}

/// The Help drop-down — the keyboard-shortcuts reference.
fn help_menu() -> Menu<Picked> {
    Menu::new(
        "Help",
        vec![item(Picked::ShowShortcuts, "Keyboard Shortcuts\u{2026}")],
    )
}

/// Build the full ordered menu tree (File · Edit · View · Go · Share · Help) as
/// the shared model.
#[must_use]
pub(crate) fn build_menus(cx: &FilesCtx) -> Vec<Menu<Picked>> {
    vec![
        file_menu(cx),
        edit_menu(cx),
        view_menu(cx),
        go_menu(cx),
        share_menu(cx),
        transfers_menu(cx),
        help_menu(),
    ]
}

// ─────────────────────────────── the status cluster ─────────────────────────

/// The friendly display of a backend path for the status chip: the `local:`
/// prefix dropped, a `peer:<id>` shown as `mesh:<id>`, and a long path truncated
/// from the head so the leaf stays visible. Pure, so the transform is testable.
#[must_use]
pub(crate) fn short_path(path: &str) -> String {
    const MAX: usize = 34;
    let friendly = path.strip_prefix("local:").map_or_else(
        || {
            path.strip_prefix("peer:")
                .map_or_else(|| path.to_owned(), |id| format!("mesh:{id}"))
        },
        str::to_owned,
    );
    let count = friendly.chars().count();
    if count <= MAX {
        return friendly;
    }
    let tail: String = friendly.chars().skip(count - (MAX - 1)).collect::<String>();
    format!("\u{2026}{tail}")
}

/// The Files live status cluster (lock 6): the current path, the item + selection
/// counts, an in-flight transfer count, and the mesh presence — all real state
/// read from the frame's [`FilesCtx`] (§7 — every chip reflects live state).
#[must_use]
pub(crate) fn build_status(cx: &FilesCtx) -> Vec<StatusChip> {
    let mut chips = vec![
        StatusChip::new(short_path(&cx.path), ChipTone::Neutral),
        StatusChip::new(format!("{} items", cx.rows), ChipTone::Neutral),
    ];
    if cx.selected > 0 {
        chips.push(StatusChip::new(
            format!("{} selected", cx.selected),
            ChipTone::Info,
        ));
    }
    if cx.running > 0 {
        chips.push(StatusChip::new(
            format!("{} transferring", cx.running),
            ChipTone::Info,
        ));
    }
    // TRANSFERS-8 — the worker ledger's in-flight count (distinct from the local
    // op-queue "transferring" chip above; the dock Files cell badges this, Q1).
    if cx.transfer_counts.active > 0 {
        chips.push(StatusChip::new(
            format!("{} in transfers", cx.transfer_counts.active),
            ChipTone::Info,
        ));
    }
    if cx.on_mesh {
        chips.push(StatusChip::with_icon("\u{25CF}", "on mesh", ChipTone::Ok));
    } else {
        chips.push(StatusChip::with_icon(
            "\u{25CF}",
            "standalone",
            ChipTone::Warn,
        ));
    }
    chips
}

// ─────────────────────────────── the Help window ────────────────────────────

/// The surface's fixed keyboard chords — the Help reference's content. Every one
/// is a real handler in [`crate::view`] (the FILEMGR-12 clipboard, the pane's
/// consume-key set, drag-and-drop), so the reference is honest (§7).
const FIXED_SHORTCUTS: [(&str, &str); 9] = [
    ("Copy selection", "Ctrl+C"),
    ("Cut selection", "Ctrl+X"),
    ("Paste", "Ctrl+V"),
    ("Select all", "Ctrl+A"),
    ("Show hidden entries", "Ctrl+H"),
    ("Quick look", "Space"),
    ("Delete (permanent)", "Del"),
    ("Clear selection / close", "Esc"),
    ("Move / copy (drag)", "Drag / Ctrl+drag"),
];

/// Flip the Help → Keyboard-Shortcuts reference window's open flag (egui memory,
/// so the stateless [`files_panel`](crate::view::files_panel) needs no field).
pub(crate) fn toggle_shortcuts(ctx: &Context) {
    let id = egui::Id::new(SHORTCUTS_OPEN);
    let open = ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    ctx.data_mut(|d| d.insert_temp(id, !open));
}

/// Flip the TRANSFERS-8 Destinations manager window's open flag (egui memory).
pub(crate) fn toggle_destinations(ctx: &Context) {
    let id = egui::Id::new(DESTINATIONS_OPEN);
    let open = ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    ctx.data_mut(|d| d.insert_temp(id, !open));
}

/// Read the Destinations manager window's open flag (the [`crate::view`] render).
#[must_use]
pub(crate) fn destinations_open(ctx: &Context) -> bool {
    ctx.data(|d| {
        d.get_temp::<bool>(egui::Id::new(DESTINATIONS_OPEN))
            .unwrap_or(false)
    })
}

/// Write the Destinations manager window's open flag (its own close button).
pub(crate) fn set_destinations_open(ctx: &Context, open: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DESTINATIONS_OPEN), open));
}

/// Render the keyboard-shortcuts reference window when open (the Help seam).
pub(crate) fn shortcuts_window(ctx: &Context) {
    let id = egui::Id::new(SHORTCUTS_OPEN);
    let mut open = ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    if !open {
        return;
    }
    egui::Window::new("Keyboard Shortcuts")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(
                RichText::new("Fixed shortcuts")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            egui::Grid::new("files-shortcuts")
                .num_columns(2)
                .spacing([Style::SP_L, Style::SP_XS])
                .show(ui, |ui| {
                    for (label, chord) in FIXED_SHORTCUTS {
                        ui.label(label);
                        ui.label(RichText::new(chord).color(Style::ACCENT));
                        ui.end_row();
                    }
                });
        });
    ctx.data_mut(|d| d.insert_temp(id, open));
}

#[cfg(test)]
mod tests {
    use super::{
        build_menus, build_status, short_path, to_action, FilesCtx, Menu, PeerLite, Picked,
        MENU_TITLES,
    };
    use crate::model::{SortKey, ViewMode};
    use crate::view::Action;
    use mde_egui::menubar::Entry;

    /// A representative two-pane, one-selection context with a reachable peer —
    /// the fixture the model/gating tests build from.
    fn fixture() -> FilesCtx {
        FilesCtx {
            active: 0,
            tab_ix: 0,
            tab_count: 2,
            rows: 12,
            selected: 1,
            has_selection: true,
            has_local_selection: true,
            has_focused_local: true,
            can_back: true,
            can_forward: false,
            can_paste: true,
            can_send: true,
            view: ViewMode::List,
            sort_key: SortKey::Name,
            dirs_first: true,
            show_hidden: false,
            is_dual: false,
            preview_open: false,
            list_thumbs: false,
            search_open: false,
            on_mesh: true,
            dest: Some("peer-a".to_owned()),
            path: "local:home".to_owned(),
            running: 0,
            peers: vec![PeerLite {
                id: "peer-a".to_owned(),
                host: "nyc3".to_owned(),
                mount_host: "nyc3".to_owned(),
                mounted: true,
                is_full: false,
            }],
            on_transfers_tab: false,
            transfer_counts: crate::transfers::LedgerCounts {
                pausable: 1,
                resumable: 1,
                terminal: 1,
                active: 2,
                total: 3,
            },
            transfer_state_filter: crate::transfers::StateFilter::All,
            transfer_method_filter: None,
        }
    }

    /// Collect every activatable [`Picked`] in a menu tree (recursing submenus).
    fn all_items(menus: &[Menu<Picked>]) -> Vec<(String, Picked, bool)> {
        fn walk(entries: &[Entry<Picked>], out: &mut Vec<(String, Picked, bool)>) {
            for entry in entries {
                match entry {
                    Entry::Item(item) => {
                        out.push((item.label.clone(), item.id.clone(), item.enabled));
                    }
                    Entry::Submenu { entries, .. } => walk(entries, out),
                    Entry::Separator | Entry::Caption(_) => {}
                }
            }
        }
        let mut out = Vec::new();
        for menu in menus {
            walk(&menu.entries, &mut out);
        }
        out
    }

    // ── structure (§7 no dead entries) ───────────────────────────────────────

    #[test]
    fn menu_order_is_the_spine_plus_files_menus() {
        let menus = build_menus(&fixture());
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, MENU_TITLES);
    }

    #[test]
    fn every_item_is_labeled_and_maps_to_a_seam() {
        let cx = fixture();
        let menus = build_menus(&cx);
        for (label, id, _) in all_items(&menus) {
            assert!(!label.is_empty(), "an item shipped unlabeled");
            // Every id resolves to a real Action, or is a bar-owned window toggle.
            let resolved = to_action(id.clone(), &cx);
            assert!(
                resolved.is_some() || id == Picked::ShowShortcuts || id == Picked::ShowDestinations,
                "{label} maps to no seam"
            );
        }
    }

    #[test]
    fn omitted_features_have_no_dead_entry() {
        // No mkdir/rename seam, no window to quit, no honest free-space probe —
        // so New Folder / Rename / Quit never ship as a greyed teaser (§7).
        let labels: Vec<String> = all_items(&build_menus(&fixture()))
            .into_iter()
            .map(|(label, _, _)| label)
            .collect();
        for banned in ["New Folder", "Rename", "Quit"] {
            assert!(
                !labels.iter().any(|l| l == banned),
                "{banned} shipped without a landed seam"
            );
        }
    }

    // ── item → action (the §6 glue) ──────────────────────────────────────────

    #[test]
    fn items_dispatch_their_real_seam() {
        // `Action` carries non-Eq payloads (paths, resolutions), so assert the
        // variant + pane index with `matches!` rather than value equality.
        let cx = fixture();
        assert!(matches!(
            to_action(Picked::NewTab, &cx),
            Some(Action::NewTab(0))
        ));
        assert!(matches!(
            to_action(Picked::CloseTab, &cx),
            Some(Action::CloseTab(0, 0))
        ));
        assert!(matches!(
            to_action(Picked::SetView(ViewMode::Grid), &cx),
            Some(Action::SetView(0, ViewMode::Grid))
        ));
        assert!(matches!(
            to_action(Picked::SortBy(SortKey::Size), &cx),
            Some(Action::SortBy(0, SortKey::Size))
        ));
        assert!(matches!(
            to_action(Picked::Delete, &cx),
            Some(Action::RequestDelete(0))
        ));
        assert!(matches!(
            to_action(Picked::MountPeer("nyc3".to_owned()), &cx),
            Some(Action::MountPeer(0, ref h)) if h == "nyc3"
        ));
        assert!(matches!(
            to_action(Picked::SendToPeer("peer-a".to_owned()), &cx),
            Some(Action::SendToPeer(0, ref id)) if id == "peer-a"
        ));
        // The active pane threads through — a right-pane frame targets pane 1.
        let right = FilesCtx {
            active: 1,
            tab_ix: 3,
            ..fixture()
        };
        assert!(matches!(
            to_action(Picked::Copy, &right),
            Some(Action::ClipCopy(1))
        ));
        assert!(matches!(
            to_action(Picked::CloseTab, &right),
            Some(Action::CloseTab(1, 3))
        ));
        // Help is the bar's own chrome, not a model seam.
        assert!(to_action(Picked::ShowShortcuts, &cx).is_none());
    }

    // ── honest gating (§7 disable, not omit) ─────────────────────────────────

    #[test]
    fn context_gated_items_disable_without_their_precondition() {
        // A fresh, empty, single-tab, peerless, offline pane: every context item
        // greys, none vanish.
        let bare = FilesCtx {
            tab_count: 1,
            rows: 0,
            selected: 0,
            has_selection: false,
            has_local_selection: false,
            has_focused_local: false,
            can_back: false,
            can_forward: false,
            can_paste: false,
            can_send: false,
            on_mesh: false,
            dest: None,
            peers: Vec::new(),
            ..fixture()
        };
        let enabled = |menus: &[Menu<Picked>], want: &Picked| {
            all_items(menus)
                .into_iter()
                .find(|(_, id, _)| id == want)
                .map(|(_, _, en)| en)
        };
        let menus = build_menus(&bare);
        assert_eq!(
            enabled(&menus, &Picked::Copy),
            Some(false),
            "Copy needs a local selection"
        );
        assert_eq!(
            enabled(&menus, &Picked::Paste),
            Some(false),
            "Paste needs a clipboard"
        );
        assert_eq!(
            enabled(&menus, &Picked::SelectAll),
            Some(false),
            "Select All needs rows"
        );
        assert_eq!(
            enabled(&menus, &Picked::Delete),
            Some(false),
            "Delete needs a local selection"
        );
        assert_eq!(
            enabled(&menus, &Picked::Properties),
            Some(false),
            "Properties needs a local row"
        );
        assert_eq!(
            enabled(&menus, &Picked::CloseTab),
            Some(false),
            "Close Tab needs >1 tab"
        );
        assert_eq!(
            enabled(&menus, &Picked::Back),
            Some(false),
            "Back needs history"
        );
        assert_eq!(
            enabled(&menus, &Picked::Send),
            Some(false),
            "Send needs a destination"
        );
        // The always-available commands stay enabled.
        assert_eq!(enabled(&menus, &Picked::NewTab), Some(true));
        assert_eq!(enabled(&menus, &Picked::Up), Some(true));
        assert_eq!(enabled(&menus, &Picked::Refresh), Some(true));

        // The rich fixture opens the same items up.
        let rich = build_menus(&fixture());
        assert_eq!(enabled(&rich, &Picked::Copy), Some(true));
        assert_eq!(enabled(&rich, &Picked::CloseTab), Some(true));
        assert_eq!(enabled(&rich, &Picked::Send), Some(true));
    }

    #[test]
    fn checkmarks_read_back_live_state() {
        let cx = FilesCtx {
            view: ViewMode::Details,
            sort_key: SortKey::Modified,
            dirs_first: true,
            show_hidden: true,
            is_dual: true,
            dest: Some("peer-a".to_owned()),
            ..fixture()
        };
        let menus = build_menus(&cx);
        // The checkmark for `want`, or `None` for an absent-or-plain item — a
        // checked item is always `Some(bool)`, so collapsing the two None cases
        // (not-found / no-checkmark) loses nothing the assertions below rely on.
        let checked = |want: &Picked| {
            fn find(entries: &[Entry<Picked>], want: &Picked) -> Option<bool> {
                for entry in entries {
                    match entry {
                        Entry::Item(item) if &item.id == want => return item.checked,
                        Entry::Submenu { entries, .. } => {
                            if let Some(hit) = find(entries, want) {
                                return Some(hit);
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
            menus.iter().find_map(|m| find(&m.entries, want))
        };
        assert_eq!(checked(&Picked::SetView(ViewMode::Details)), Some(true));
        assert_eq!(checked(&Picked::SetView(ViewMode::List)), Some(false));
        assert_eq!(checked(&Picked::SortBy(SortKey::Modified)), Some(true));
        assert_eq!(checked(&Picked::ToggleDirsFirst), Some(true));
        assert_eq!(checked(&Picked::ToggleHidden), Some(true));
        assert_eq!(checked(&Picked::ToggleDual), Some(true));
        assert_eq!(
            checked(&Picked::SetDestination("peer-a".to_owned())),
            Some(true)
        );
        // A plain command carries no checkmark.
        assert_eq!(checked(&Picked::NewTab), None);
    }

    #[test]
    fn transfers_menu_gates_batch_verbs_on_the_ledger_tally() {
        use crate::transfers::{LedgerCounts, StateFilter};
        let enabled = |menus: &[Menu<Picked>], want: &Picked| {
            all_items(menus)
                .into_iter()
                .find(|(_, id, _)| id == want)
                .map(|(_, _, en)| en)
        };
        // A rich ledger (1 pausable / 1 resumable / 1 terminal) opens all three.
        let rich = build_menus(&fixture());
        assert_eq!(enabled(&rich, &Picked::TransferPauseAll), Some(true));
        assert_eq!(enabled(&rich, &Picked::TransferResumeAll), Some(true));
        assert_eq!(enabled(&rich, &Picked::TransferClearCompleted), Some(true));
        // New Transfer is always available (no precondition).
        assert_eq!(enabled(&rich, &Picked::NewTransfer), Some(true));
        // An empty ledger greys the batch verbs — but never omits them (§7).
        let empty = FilesCtx {
            transfer_counts: LedgerCounts::default(),
            ..fixture()
        };
        let menus = build_menus(&empty);
        assert_eq!(enabled(&menus, &Picked::TransferPauseAll), Some(false));
        assert_eq!(enabled(&menus, &Picked::TransferResumeAll), Some(false));
        assert_eq!(
            enabled(&menus, &Picked::TransferClearCompleted),
            Some(false)
        );
        // The state/method filter radios read back the live filter.
        let filtered = FilesCtx {
            transfer_state_filter: StateFilter::Failed,
            transfer_method_filter: Some(crate::transfers::Method::Rsync),
            ..fixture()
        };
        assert!(matches!(
            to_action(Picked::TransferStateFilter(StateFilter::Failed), &filtered),
            Some(Action::SetTransferStateFilter(StateFilter::Failed))
        ));
        // Destinations is the bar's own window, not a model seam.
        assert!(to_action(Picked::ShowDestinations, &filtered).is_none());
    }

    #[test]
    fn peerless_menus_caption_rather_than_ship_empty() {
        let peerless = FilesCtx {
            peers: Vec::new(),
            ..fixture()
        };
        let menus = build_menus(&peerless);
        // No peer submenu means no per-peer send item exists at all (omitted, not
        // a dead entry); the menus carry an honest caption instead.
        let ids: Vec<Picked> = all_items(&menus).into_iter().map(|(_, id, _)| id).collect();
        assert!(
            !ids.iter()
                .any(|id| matches!(id, Picked::SendToPeer(_) | Picked::MountPeer(_))),
            "peerless build must not ship a peer action"
        );
    }

    // ── the status cluster (real live state) ─────────────────────────────────

    #[test]
    fn status_cluster_reflects_the_frame() {
        let mut cx = fixture();
        cx.rows = 42;
        cx.selected = 3;
        cx.running = 2;
        cx.on_mesh = true;
        cx.path = "local:home".to_owned();
        let chips = build_status(&cx);
        let texts: Vec<&str> = chips.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"home"), "the path chip strips local:");
        assert!(texts.contains(&"42 items"));
        assert!(texts.contains(&"3 selected"));
        assert!(texts.contains(&"2 transferring"));
        assert!(texts.contains(&"on mesh"));
        // Idle, unselected, standalone drops the optional chips + flips presence.
        let idle = FilesCtx {
            selected: 0,
            running: 0,
            on_mesh: false,
            ..cx
        };
        let idle_texts: Vec<String> = build_status(&idle).iter().map(|c| c.text.clone()).collect();
        assert!(!idle_texts.iter().any(|t| t.contains("selected")));
        assert!(!idle_texts.iter().any(|t| t.contains("transferring")));
        assert!(idle_texts.iter().any(|t| t == "standalone"));
    }

    #[test]
    fn short_path_strips_prefixes_and_truncates() {
        assert_eq!(short_path("local:home"), "home");
        assert_eq!(short_path("peer:nyc3"), "mesh:nyc3");
        assert_eq!(short_path("/etc/hosts"), "/etc/hosts");
        let long = "/very/deeply/nested/directory/tree/that/exceeds/the/chip/budget";
        let short = short_path(long);
        assert!(
            short.starts_with('\u{2026}'),
            "a long path truncates from the head"
        );
        assert!(short.chars().count() <= 34);
        assert!(long.ends_with(short.trim_start_matches('\u{2026}')));
    }
}

//! The egui rendering of the Files surface shell (FILEMGR-8).
//!
//! Every widget reads the render-agnostic [`FileBrowser`] and draws through the
//! shared [`Style`] — no raw colours or spacing (governance §4). The view never
//! mutates the model mid-render: a frame collects the user's intents as
//! [`Action`]s while it holds a shared `&FileBrowser`, then applies them once the
//! borrow is released. That keeps the borrow checker happy and the data flow
//! one-directional (render → intents → apply).
//!
//! This is the full desktop shell: a global toolbar (view mode · hidden · dirs-
//! first · dual-pane · Send-To), a Places + Mesh sidebar, and one or two panes.
//! Each pane carries back/forward/up history, clickable breadcrumbs, an editable
//! path box, a tab strip, and a listing rendered as List / Grid / Details (with
//! sortable column headers). Rows support click / Ctrl-click / Shift-range /
//! Ctrl-A / rubber-band selection and full drag-and-drop (move by default, copy
//! with Ctrl) within a pane and between the two panes — every drop is a real
//! transfer run through the FILEMGR-2 op queue, with live progress on the bottom
//! strip.
//!
//! FILEMGR-10 adds the preview surfaces: lazy cached thumbnails in the Grid
//! tiles + an optional List column, a toggleable right-hand preview pane
//! (image / highlighted text / media metadata), and the Space quick-look
//! overlay. Decodes never happen on this paint path — the view *requests*
//! (through [`Action`]s, which doubles as the cache-recency signal), draws an
//! icon placeholder, and the [`crate::preview`] worker delivers off-thread.
//! Every viewer is built in (§9 / lock 23): nothing here spawns a program.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mde_egui::egui::load::SizedTexture;
use mde_egui::egui::{
    self, Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions,
};
use mde_egui::{field, muted_note, status_dot, Style};

use mde_files::model::{Mime, PeerStatus};
use mde_files::opqueue::Progress;

use crate::mesh_mount::{MountPhase, MountView};
use crate::model::{
    mount_host_of, FileBrowser, Location, SendOutcome, SortKey, SortSpec, ViewMode, LOCAL_SPOTS,
};
use crate::preview::{
    Pixels, PreviewData, PreviewKind, PreviewState, ThumbState, TokenKind, TokenSpan,
};

// Details-view column widths (right-hand columns; name takes the rest).
const COL_SIZE_W: f32 = 96.0;
const COL_TYPE_W: f32 = 56.0;
const COL_MOD_W: f32 = 80.0;
// Listing row height (List + Details); grid tile size.
const ROW_H: f32 = 26.0;
const TILE_W: f32 = 132.0;
const TILE_H: f32 = 72.0;

/// The drag-and-drop payload: which pane the drag started in. The selection to
/// transfer is read from that pane's model at drop time (it is already the
/// focused pane, since a press focuses it).
#[derive(Clone)]
struct FilesDrag {
    source_pane: usize,
}

/// A per-frame snapshot of one listing row (so the render holds no `&FileRow`
/// while it pushes selection/DnD [`Action`]s).
struct EntryView {
    idx: usize,
    name: String,
    size: String,
    age: String,
    mime: Mime,
    is_dir: bool,
    selected: bool,
    path: Option<String>,
    /// FILEMGR-10 — what this row can preview as (extension-keyed, honest).
    kind: PreviewKind,
    /// FILEMGR-10 — on a mesh mount: thumbnails only when selected (lock 18).
    remote: bool,
}

/// A user intent captured during a render, applied after the frame releases its
/// shared borrow of the model.
enum Action {
    Focus(usize),
    Navigate(usize, Location),
    Back(usize),
    Forward(usize),
    Up(usize),
    SetPathEdit(usize, String),
    OpenPathEdit(usize),
    OpenRow(usize, usize),
    SetView(usize, ViewMode),
    SortBy(usize, SortKey),
    ToggleHidden(usize),
    ToggleDirsFirst(usize),
    ToggleDual,
    Refresh,
    Click(usize, usize),
    CtrlClick(usize, usize),
    ShiftClick(usize, usize),
    SelectAll(usize),
    ClearSelection(usize),
    Rubber(usize, BTreeSet<usize>),
    NewTab(usize),
    CloseTab(usize, usize),
    SelectTab(usize, usize),
    Drop {
        source_pane: usize,
        dest_dir: PathBuf,
        copy: bool,
    },
    SetDestination(String),
    /// FILEMGR-9 — navigate into a peer: request its mount + browse it.
    MountPeer(usize, String),
    /// FILEMGR-9 — escalate a peer from home to full-filesystem access.
    EscalatePeer(String),
    /// FILEMGR-9 — unmount a peer.
    UnmountPeer(String),
    /// FILEMGR-10 — want a thumbnail for this path (pushed every frame the
    /// cell is visible; the model dedups + uses it as the LRU recency signal).
    NeedThumb(String),
    /// FILEMGR-10 — want a pane/quick-look preview for this path.
    NeedPreview(String),
    /// FILEMGR-10 — toggle the right-hand preview pane.
    TogglePreviewPane,
    /// FILEMGR-10 — toggle the List view's thumbnail column.
    ToggleListThumbs,
    /// FILEMGR-10 — Space: toggle the quick-look overlay.
    ToggleQuickLook,
    /// FILEMGR-10 — Escape / backdrop click: close the quick-look overlay.
    CloseQuickLook,
    Send,
    PauseOp(u64),
    ResumeOp(u64),
    CancelOp(u64),
    DismissOp(u64),
}

/// Render the whole Files surface into `ui`. The one reusable entry point: the
/// standalone binary and the E12 shell both call it.
pub fn files_panel(ui: &mut egui::Ui, browser: &mut FileBrowser) {
    // Fold the op queue's events in first, so a just-finished transfer's results
    // are already reloaded before this frame reads the listings.
    browser.pump_ops();
    if browser.ops().any_running() {
        ui.ctx().request_repaint();
    }
    // FILEMGR-10 — fold finished thumbnail/preview decodes in before the frame
    // reads them, and keep a short heartbeat while any decode is in flight so
    // results appear without input. Decoding itself NEVER runs on this paint
    // path — the worker thread owns it.
    browser.pump_previews();
    if browser.previews_pending() {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    // FILEMGR-9 — refresh the mesh-mount state (a cheap, cadence-gated local Bus
    // read; never a peer probe). While a mount is still coming up, keep a repaint
    // heartbeat alive so the sidebar pip animates mounting → mounted without input.
    browser.pump_mounts();
    if browser.any_mount_transitional() {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }

    let mut actions: Vec<Action> = Vec::new();
    top_bar(ui, browser, &mut actions);
    egui::TopBottomPanel::bottom("files-bottom").show_inside(ui, |ui| {
        op_strip(ui, browser, &mut actions);
        status_line(ui, browser);
    });
    sidebar(ui, browser, &mut actions);
    // FILEMGR-10 — the toggleable preview pane sits between the listing and the
    // window edge, previewing the focused selection with built-in viewers only.
    if browser.preview_pane_open() {
        preview_pane(ui, browser, &mut actions);
    }
    egui::CentralPanel::default().show_inside(ui, |ui| {
        if browser.is_dual() {
            ui.columns(2, |cols| {
                pane_view(&mut cols[0], browser, 0, &mut actions);
                pane_view(&mut cols[1], browser, 1, &mut actions);
            });
        } else {
            pane_view(ui, browser, 0, &mut actions);
        }
    });
    // FILEMGR-10 — the Space quick-look modal, over everything.
    quick_look_overlay(ui, browser, &mut actions);

    for action in actions {
        apply(browser, action);
    }
}

/// Apply a captured intent to the model.
fn apply(browser: &mut FileBrowser, action: Action) {
    match action {
        Action::Focus(p) => browser.set_active_pane(p),
        Action::Navigate(p, loc) => {
            browser.set_active_pane(p);
            browser.navigate(p, loc);
        }
        Action::Back(p) => browser.go_back(p),
        Action::Forward(p) => browser.go_forward(p),
        Action::Up(p) => browser.go_up(p),
        Action::SetPathEdit(p, text) => browser.set_path_edit(p, text),
        Action::OpenPathEdit(p) => browser.open_path_edit(p),
        Action::OpenRow(p, i) => {
            // Focus follows the activation, so a quick-look opened from the
            // other pane targets the row that was actually opened.
            browser.set_active_pane(p);
            browser.open_row(p, i);
        }
        Action::SetView(p, m) => browser.set_view(p, m),
        Action::SortBy(p, k) => browser.sort_by(p, k),
        Action::ToggleHidden(p) => browser.toggle_hidden(p),
        Action::ToggleDirsFirst(p) => browser.toggle_dirs_first(p),
        Action::ToggleDual => browser.toggle_dual(),
        Action::Refresh => {
            browser.refresh_roster();
            browser.reload_all();
            // Lock 18 — a manual refresh busts the thumbnail/preview caches so
            // changed files re-decode.
            browser.clear_previews();
        }
        Action::Click(p, i) => browser.click(p, i),
        Action::CtrlClick(p, i) => browser.ctrl_click(p, i),
        Action::ShiftClick(p, i) => browser.shift_click(p, i),
        Action::SelectAll(p) => browser.select_all(p),
        Action::ClearSelection(p) => browser.clear_selection(p),
        Action::Rubber(p, set) => browser.set_rubber(p, set),
        Action::NewTab(p) => browser.new_tab(p),
        Action::CloseTab(p, t) => browser.close_tab(p, t),
        Action::SelectTab(p, t) => browser.select_tab(p, t),
        Action::Drop {
            source_pane,
            dest_dir,
            copy,
        } => {
            browser.drop_transfer(source_pane, dest_dir, copy);
        }
        Action::SetDestination(id) => browser.set_destination(id),
        Action::MountPeer(p, host) => {
            browser.set_active_pane(p);
            browser.open_peer(p, &host);
        }
        Action::EscalatePeer(host) => browser.escalate_peer(&host),
        Action::UnmountPeer(host) => browser.unmount_peer(&host),
        Action::NeedThumb(path) => browser.request_thumb(&path),
        Action::NeedPreview(path) => browser.request_preview(&path),
        Action::TogglePreviewPane => browser.toggle_preview_pane(),
        Action::ToggleListThumbs => browser.toggle_list_thumbs(),
        Action::ToggleQuickLook => browser.toggle_quick_look(),
        Action::CloseQuickLook => browser.close_quick_look(),
        Action::Send => {
            browser.send();
        }
        Action::PauseOp(id) => browser.pause_op(id),
        Action::ResumeOp(id) => browser.resume_op(id),
        Action::CancelOp(id) => browser.cancel_op(id),
        Action::DismissOp(id) => browser.dismiss_op(id),
    }
}

// ── Top toolbar ───────────────────────────────────────────────────────────────

fn top_bar(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let active = b.active_pane_index();
    egui::TopBottomPanel::top("files-top").show_inside(ui, |ui| {
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("Files")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
            ui.add_space(Style::SP_M);

            // View-mode segmented control.
            let cur_view = b.active_tab().view();
            for mode in ViewMode::ALL {
                if ui
                    .selectable_label(cur_view == mode, mode.label())
                    .clicked()
                {
                    actions.push(Action::SetView(active, mode));
                }
            }
            ui.add_space(Style::SP_M);

            let hidden = b.active_tab().show_hidden();
            if ui
                .selectable_label(hidden, "Hidden")
                .on_hover_text("Show hidden entries (Ctrl+H)")
                .clicked()
            {
                actions.push(Action::ToggleHidden(active));
            }
            let dirs_first = b.active_tab().sort().dirs_first;
            if ui
                .selectable_label(dirs_first, "Dirs first")
                .on_hover_text("Group directories ahead of files")
                .clicked()
            {
                actions.push(Action::ToggleDirsFirst(active));
            }
            if ui
                .selectable_label(b.is_dual(), "Dual pane")
                .on_hover_text("Show a second pane for cross-folder work")
                .clicked()
            {
                actions.push(Action::ToggleDual);
            }
            // FILEMGR-10 — the preview toggles.
            if ui
                .selectable_label(b.list_thumbs(), "Thumbs")
                .on_hover_text("Thumbnails in the List view (the Grid always thumbnails)")
                .clicked()
            {
                actions.push(Action::ToggleListThumbs);
            }
            if ui
                .selectable_label(b.preview_pane_open(), "Preview")
                .on_hover_text("Preview pane \u{2014} Space quick-looks the selection")
                .clicked()
            {
                actions.push(Action::TogglePreviewPane);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = b.can_send();
                let label = b
                    .destination()
                    .map_or_else(|| "Send to…".to_string(), |peer| format!("Send to {peer}"));
                let button = egui::Button::new(RichText::new(label).color(Style::BG).strong())
                    .fill(Style::ACCENT);
                if ui.add_enabled(can, button).clicked() {
                    actions.push(Action::Send);
                }
                ui.add_space(Style::SP_S);
                if ui.button("Refresh").clicked() {
                    actions.push(Action::Refresh);
                }
            });
        });
        ui.add_space(Style::SP_XS);
    });
}

// ── Sidebar ───────────────────────────────────────────────────────────────────

fn sidebar(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let active = b.active_pane_index();
    egui::SidePanel::left("files-side")
        .default_width(Style::SP_XL * 7.0)
        .show_inside(ui, |ui| {
            ui.add_space(Style::SP_S);
            let host = if b.self_node().host.is_empty() {
                "this node"
            } else {
                b.self_node().host.as_str()
            };
            ui.label(RichText::new(host).color(Style::TEXT).strong());
            ui.colored_label(Style::TEXT_DIM, node_role(b));
            mesh_badge(ui, b);
            ui.add_space(Style::SP_M);

            section_header(ui, "PLACES");
            for spot in LOCAL_SPOTS {
                let here = matches!(b.active_tab().location(), Location::Local(p) if p.as_str() == spot.path);
                if ui.selectable_label(here, spot.label).clicked() {
                    actions.push(Action::Navigate(active, Location::Local(spot.path.to_string())));
                }
            }
            ui.add_space(Style::SP_M);

            section_header(ui, "MESH");
            if b.peers().is_empty() {
                muted_note(ui, "No peers connected.");
            } else {
                muted_note(
                    ui,
                    format!(
                        "{} of {} reachable",
                        b.reachable_destinations().len(),
                        b.peers().len()
                    ),
                );
                for peer in b.peers() {
                    peer_row(ui, b, peer, active, actions);
                }
            }
        });
}

/// One Mesh sidebar tree row: a peer with a live presence pip, its worker-
/// published mount phase, and the home↔full-filesystem escalation + eject
/// controls (FILEMGR-9). An offline peer is honestly greyed and inert — its label
/// isn't a button, so there's no dead-end and no blocking probe.
fn peer_row(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    peer: &mde_files::model::Peer,
    active: usize,
    actions: &mut Vec<Action>,
) {
    let host = mount_host_of(peer);
    let reachable = peer.status.is_reachable();
    let mount = b.peer_mount(peer);
    let browsing = matches!(
        b.active_tab().location(),
        Location::Peer(id) if id.as_str() == peer.id.as_str()
    );

    // Line 1 — presence pip + host name (click = request the mount + browse it).
    ui.horizontal(|ui| {
        status_dot(ui, peer_color(peer.status));
        if reachable {
            if ui
                .selectable_label(browsing, peer.host.as_str())
                .on_hover_text("Mount this peer over the mesh and browse it")
                .clicked()
            {
                actions.push(Action::MountPeer(active, host.to_string()));
            }
        } else {
            // Honestly greyed: an offline peer can't be mounted, so its name is
            // rendered inert (a disabled, frameless label) rather than a live link.
            ui.add_enabled(
                false,
                egui::Button::new(RichText::new(peer.host.as_str()).color(Style::TEXT_DIM))
                    .frame(false),
            )
            .on_hover_text("Peer is offline \u{2014} can't be mounted");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if reachable {
                let is_dest = b.destination() == Some(peer.id.as_str());
                if ui
                    .selectable_label(is_dest, "dest")
                    .on_hover_text("Set as the Send-To destination")
                    .clicked()
                {
                    actions.push(Action::SetDestination(peer.id.clone()));
                }
            } else {
                muted_note(ui, "offline");
            }
        });
    });

    // Line 2 — the indented mount state + escalation/eject (reachable peers only).
    if reachable {
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_L);
            peer_mount_chip(ui, mount);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Home ↔ full-filesystem escalation (lock 14 — the `escalate` verb).
                let full = mount.is_some_and(MountView::is_full);
                if ui
                    .selectable_label(full, "Full FS")
                    .on_hover_text(if full {
                        "Mounted with full-filesystem access"
                    } else {
                        "Escalate this mount to full-filesystem (/) access"
                    })
                    .clicked()
                {
                    actions.push(Action::EscalatePeer(host.to_string()));
                }
                // Eject a live mount.
                if mount.is_some_and(|m| m.phase.is_mounted())
                    && ui
                        .small_button("Eject")
                        .on_hover_text("Unmount this peer")
                        .clicked()
                {
                    actions.push(Action::UnmountPeer(host.to_string()));
                }
            });
        });
    }
}

/// The per-peer mount-state chip: the worker's live phase (mounting / mounted /
/// reconnecting / unreachable) in a Carbon-token colour, with the mounted scope
/// folded in and any degrade reason on hover. A peer never navigated to has no
/// published state, shown as an honest "not mounted".
fn peer_mount_chip(ui: &mut egui::Ui, mount: Option<&MountView>) {
    let Some(mount) = mount else {
        muted_note(ui, "not mounted");
        return;
    };
    let mut label = mount.phase.label().to_string();
    if mount.phase.is_mounted() {
        if let Some(scope) = mount.scope {
            label = format!("mounted \u{b7} {}", scope.label());
        }
    }
    let resp = ui.colored_label(
        mount_phase_color(mount.phase),
        RichText::new(label).size(Style::SMALL),
    );
    if let Some(reason) = mount.reason.as_deref() {
        resp.on_hover_text(reason);
    }
}

/// The Carbon token for a mount phase (§4 — no raw hex).
const fn mount_phase_color(phase: MountPhase) -> Color32 {
    match phase {
        MountPhase::Mounted => Style::OK,
        MountPhase::Mounting | MountPhase::Reconnecting => Style::WARN,
        MountPhase::Unreachable => Style::DANGER,
        MountPhase::Unmounted => Style::TEXT_DIM,
    }
}

// ── One pane ──────────────────────────────────────────────────────────────────

fn pane_view(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize, actions: &mut Vec<Action>) {
    // A press anywhere focuses the pane; a focused pane in dual mode gets an
    // accent rule so it's clear which pane the toolbar + keyboard act on.
    if b.is_dual() && b.active_pane_index() == pane_ix {
        let top = ui.max_rect();
        ui.painter()
            .hline(top.x_range(), top.top(), Stroke::new(2.0, Style::ACCENT));
        ui.add_space(Style::SP_XS);
    }
    nav_row(ui, b, pane_ix, actions);
    tab_strip(ui, b, pane_ix, actions);
    ui.separator();
    listing(ui, b, pane_ix, actions);
}

fn nav_row(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize, actions: &mut Vec<Action>) {
    let tab = b.pane(pane_ix).active_tab();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(tab.can_back(), egui::Button::new("\u{2190}"))
            .on_hover_text("Back")
            .clicked()
        {
            actions.push(Action::Back(pane_ix));
        }
        if ui
            .add_enabled(tab.can_forward(), egui::Button::new("\u{2192}"))
            .on_hover_text("Forward")
            .clicked()
        {
            actions.push(Action::Forward(pane_ix));
        }
        let can_up = tab.location().parent().is_some();
        if ui
            .add_enabled(can_up, egui::Button::new("\u{2191}"))
            .on_hover_text("Up")
            .clicked()
        {
            actions.push(Action::Up(pane_ix));
        }
        ui.separator();
        let crumbs = tab.location().crumbs();
        let last = crumbs.len().saturating_sub(1);
        for (i, crumb) in crumbs.into_iter().enumerate() {
            let strong = i == last;
            let color = if strong { Style::TEXT } else { Style::TEXT_DIM };
            if ui
                .add(egui::Button::new(RichText::new(&crumb.label).color(color)).frame(false))
                .clicked()
            {
                actions.push(Action::Navigate(pane_ix, crumb.location));
            }
            if i != last {
                muted_note(ui, "\u{203a}");
            }
        }
    });
    ui.horizontal(|ui| {
        let mut buf = tab.path_edit().to_string();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut buf)
                .desired_width(f32::INFINITY)
                .hint_text("path or peer:<id>"),
        );
        if resp.changed() {
            actions.push(Action::SetPathEdit(pane_ix, buf.clone()));
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            actions.push(Action::SetPathEdit(pane_ix, buf));
            actions.push(Action::OpenPathEdit(pane_ix));
        }
    });
}

fn tab_strip(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize, actions: &mut Vec<Action>) {
    let pane = b.pane(pane_ix);
    ui.horizontal(|ui| {
        let closeable = pane.tabs().len() > 1;
        for (i, tab) in pane.tabs().iter().enumerate() {
            let active = i == pane.active_tab_index();
            if ui.selectable_label(active, tab.title()).clicked() {
                actions.push(Action::SelectTab(pane_ix, i));
            }
            if closeable
                && ui
                    .small_button("\u{00d7}")
                    .on_hover_text("Close tab")
                    .clicked()
            {
                actions.push(Action::CloseTab(pane_ix, i));
            }
        }
        if ui.small_button("+").on_hover_text("New tab").clicked() {
            actions.push(Action::NewTab(pane_ix));
        }
    });
}

// ── The listing (List / Grid / Details) + selection + DnD + rubber-band ───────

fn listing(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize, actions: &mut Vec<Action>) {
    // Keyboard shortcuts act on the focused pane — but never while a text
    // field (the path box) owns the keyboard, so typing a space or Ctrl+A
    // edits the text instead of hijacking the listing.
    if b.active_pane_index() == pane_ix && !ui.ctx().wants_keyboard_input() {
        let quick_look = b.quick_look_open();
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::A) {
                actions.push(Action::SelectAll(pane_ix));
            }
            if i.consume_key(Modifiers::COMMAND, Key::H) {
                actions.push(Action::ToggleHidden(pane_ix));
            }
            // FILEMGR-10 — Space quick-looks the focused selection.
            if i.consume_key(Modifiers::NONE, Key::Space) {
                actions.push(Action::ToggleQuickLook);
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                if quick_look {
                    actions.push(Action::CloseQuickLook);
                } else {
                    actions.push(Action::ClearSelection(pane_ix));
                }
            }
        });
    }

    let tab = b.pane(pane_ix).active_tab();
    let view = tab.view();
    let entries: Vec<EntryView> = tab
        .rows()
        .iter()
        .enumerate()
        .map(|(idx, r)| EntryView {
            idx,
            name: r.name.clone(),
            size: r.size.clone(),
            age: r.age.clone(),
            mime: r.mime,
            is_dir: r.is_dir(),
            selected: tab.is_selected(idx),
            path: r.path.clone(),
            kind: PreviewKind::detect(&r.name, r.is_dir()),
            remote: r.path.as_deref().is_some_and(|p| b.is_remote_path(p)),
        })
        .collect();

    ui.horizontal(|ui| {
        muted_note(ui, format!("{} items", entries.len()));
        if !tab.selection().is_empty() {
            muted_note(ui, format!("\u{b7} {} selected", tab.selection().len()));
        }
        if tab.show_hidden() {
            muted_note(ui, "\u{b7} hidden shown");
        }
    });

    if view == ViewMode::Details {
        details_header(ui, pane_ix, tab.sort(), actions);
    }

    // A background *drag* sensor over the whole listing region: it wins a press
    // that begins on empty space (a rubber-band, or a drop into the current dir)
    // while rows drawn on top of it win presses on themselves. It senses drag
    // only — never click — so it can never race a row's click and clear the
    // selection out from under it (Escape clears; the background never does).
    let region = ui.available_rect_before_wrap();
    let bg = ui.interact(
        region,
        ui.make_persistent_id(("listing-bg", pane_ix)),
        Sense::drag(),
    );
    if bg.dragged() {
        actions.push(Action::Focus(pane_ix));
    }

    let mut rects: Vec<(usize, Rect)> = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt(("listing-scroll", pane_ix))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if entries.is_empty() {
                empty_state(ui, b, pane_ix);
            } else {
                match view {
                    ViewMode::List => list_view(ui, b, pane_ix, &entries, &mut rects, actions),
                    ViewMode::Grid => grid_view(ui, b, pane_ix, &entries, &mut rects, actions),
                    ViewMode::Details => details_view(ui, pane_ix, &entries, &mut rects, actions),
                }
            }
        });

    rubber_band(ui, pane_ix, &bg, &rects, actions);
    pane_drop(ui, b, pane_ix, &bg, actions);
}

fn list_view(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    pane_ix: usize,
    entries: &[EntryView],
    rects: &mut Vec<(usize, Rect)>,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width();
    let thumbs = b.list_thumbs();
    for e in entries {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, ROW_H), Sense::click_and_drag());
        rects.push((e.idx, rect));
        paint_entry_bg(ui, rect, e.selected, resp.hovered());
        let cy = rect.center().y;
        let tag_x = rect.left() + Style::SP_S;
        let name_x = tag_x + Style::SP_L;
        // FILEMGR-10 — the optional thumbnail column: a decoded thumb replaces
        // the type tag; anything not (yet) decoded keeps the honest tag.
        let thumbed = if thumbs {
            want_thumb(ui, e, rect, actions);
            let cell = Rect::from_min_size(
                egui::pos2(rect.left() + Style::SP_XS, rect.top() + 1.0),
                egui::vec2(ROW_H - 2.0, ROW_H - 2.0),
            );
            draw_thumb(ui, b, e, cell)
        } else {
            false
        };
        if !thumbed {
            ui.painter().text(
                egui::pos2(tag_x, cy),
                Align2::LEFT_CENTER,
                mime_tag(e.mime),
                FontId::monospace(Style::SMALL),
                Style::TEXT_DIM,
            );
        }
        let name_clip = Rect::from_min_max(
            egui::pos2(name_x, rect.top()),
            egui::pos2(rect.right() - Style::SP_S, rect.bottom()),
        );
        ui.painter().with_clip_rect(name_clip).text(
            egui::pos2(name_x, cy),
            Align2::LEFT_CENTER,
            &e.name,
            FontId::monospace(Style::BODY),
            name_color(e),
        );
        entry_interactions(ui, pane_ix, e, &resp, actions);
    }
}

fn grid_view(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    pane_ix: usize,
    entries: &[EntryView],
    rects: &mut Vec<(usize, Rect)>,
    actions: &mut Vec<Action>,
) {
    ui.horizontal_wrapped(|ui| {
        for e in entries {
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(TILE_W, TILE_H), Sense::click_and_drag());
            rects.push((e.idx, rect));
            paint_entry_bg(ui, rect, e.selected, resp.hovered());
            // FILEMGR-10 — a decoded thumbnail fills the tile art area; until
            // (or unless) one exists, the honest type tag stays the icon.
            want_thumb(ui, e, rect, actions);
            let art = Rect::from_min_max(
                egui::pos2(rect.left() + Style::SP_XS, rect.top() + Style::SP_XS),
                egui::pos2(rect.right() - Style::SP_XS, rect.bottom() - Style::SP_L),
            );
            if !draw_thumb(ui, b, e, art) {
                ui.painter().text(
                    egui::pos2(rect.center().x, rect.top() + Style::SP_L),
                    Align2::CENTER_CENTER,
                    mime_tag(e.mime),
                    FontId::monospace(Style::HEADING),
                    if e.is_dir {
                        Style::ACCENT
                    } else {
                        Style::TEXT_DIM
                    },
                );
            }
            let name_clip = Rect::from_min_max(
                egui::pos2(rect.left() + Style::SP_XS, rect.bottom() - Style::SP_L),
                egui::pos2(rect.right() - Style::SP_XS, rect.bottom()),
            );
            ui.painter().with_clip_rect(name_clip).text(
                egui::pos2(rect.center().x, rect.bottom() - Style::SP_S),
                Align2::CENTER_CENTER,
                &e.name,
                FontId::monospace(Style::SMALL),
                name_color(e),
            );
            entry_interactions(ui, pane_ix, e, &resp, actions);
        }
    });
}

fn details_view(
    ui: &mut egui::Ui,
    pane_ix: usize,
    entries: &[EntryView],
    rects: &mut Vec<(usize, Rect)>,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width();
    for e in entries {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, ROW_H), Sense::click_and_drag());
        rects.push((e.idx, rect));
        paint_entry_bg(ui, rect, e.selected, resp.hovered());
        let cols = detail_columns(rect);
        let cy = rect.center().y;
        // Name (clipped to its column).
        ui.painter().with_clip_rect(cols.name).text(
            egui::pos2(cols.name.left(), cy),
            Align2::LEFT_CENTER,
            &e.name,
            FontId::monospace(Style::BODY),
            name_color(e),
        );
        let painter = ui.painter();
        painter.text(
            egui::pos2(cols.size_x, cy),
            Align2::LEFT_CENTER,
            &e.size,
            FontId::monospace(Style::SMALL),
            Style::TEXT_DIM,
        );
        painter.text(
            egui::pos2(cols.type_x, cy),
            Align2::LEFT_CENTER,
            mime_tag(e.mime),
            FontId::monospace(Style::SMALL),
            Style::TEXT_DIM,
        );
        painter.text(
            egui::pos2(cols.mod_x, cy),
            Align2::LEFT_CENTER,
            &e.age,
            FontId::monospace(Style::SMALL),
            Style::TEXT_DIM,
        );
        entry_interactions(ui, pane_ix, e, &resp, actions);
    }
}

/// Column geometry shared by the Details header + rows.
struct DetailCols {
    name: Rect,
    size_x: f32,
    type_x: f32,
    mod_x: f32,
}

fn detail_columns(rect: Rect) -> DetailCols {
    let pad = Style::SP_S;
    let mod_x = rect.right() - COL_MOD_W;
    let type_x = mod_x - COL_TYPE_W;
    let size_x = type_x - COL_SIZE_W;
    let name = Rect::from_min_max(
        egui::pos2(rect.left() + pad, rect.top()),
        egui::pos2(size_x - pad, rect.bottom()),
    );
    DetailCols {
        name,
        size_x,
        type_x,
        mod_x,
    }
}

fn details_header(ui: &mut egui::Ui, pane_ix: usize, sort: SortSpec, actions: &mut Vec<Action>) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ROW_H), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, Style::BORDER),
    );
    let cols = detail_columns(rect);
    let cy = rect.center().y;
    let header =
        |ui: &mut egui::Ui, actions: &mut Vec<Action>, key: SortKey, span: Rect, x: f32| {
            let resp = ui.interact(
                span,
                ui.make_persistent_id((pane_ix, "hdr", key.label())),
                Sense::click(),
            );
            let active = sort.key == key;
            let mut label = key.label().to_string();
            if active {
                label.push_str(sort.dir.caret());
            }
            let color = if active { Style::TEXT } else { Style::TEXT_DIM };
            ui.painter().text(
                egui::pos2(x, cy),
                Align2::LEFT_CENTER,
                label,
                FontId::monospace(Style::SMALL),
                color,
            );
            if resp.clicked() {
                actions.push(Action::SortBy(pane_ix, key));
            }
        };
    let size_span = Rect::from_min_max(
        egui::pos2(cols.size_x, rect.top()),
        egui::pos2(cols.type_x, rect.bottom()),
    );
    let type_span = Rect::from_min_max(
        egui::pos2(cols.type_x, rect.top()),
        egui::pos2(cols.mod_x, rect.bottom()),
    );
    let mod_span = Rect::from_min_max(egui::pos2(cols.mod_x, rect.top()), rect.max);
    header(ui, actions, SortKey::Name, cols.name, cols.name.left());
    header(ui, actions, SortKey::Size, size_span, cols.size_x);
    header(ui, actions, SortKey::Kind, type_span, cols.type_x);
    header(ui, actions, SortKey::Modified, mod_span, cols.mod_x);
}

/// Selection + open + drag-source + drop-onto-folder for one entry response.
fn entry_interactions(
    ui: &egui::Ui,
    pane_ix: usize,
    e: &EntryView,
    resp: &egui::Response,
    actions: &mut Vec<Action>,
) {
    let mods = ui.input(|i| i.modifiers);
    if resp.clicked() {
        actions.push(Action::Focus(pane_ix));
        if mods.command {
            actions.push(Action::CtrlClick(pane_ix, e.idx));
        } else if mods.shift {
            actions.push(Action::ShiftClick(pane_ix, e.idx));
        } else {
            actions.push(Action::Click(pane_ix, e.idx));
        }
    }
    if resp.double_clicked() {
        actions.push(Action::OpenRow(pane_ix, e.idx));
    }
    if resp.drag_started() {
        actions.push(Action::Focus(pane_ix));
        if !e.selected {
            actions.push(Action::Click(pane_ix, e.idx));
        }
    }
    resp.dnd_set_drag_payload(FilesDrag {
        source_pane: pane_ix,
    });

    // A directory row is a drop target: drop the drag INTO this folder.
    if e.is_dir {
        if let Some(path) = &e.path {
            if resp.dnd_hover_payload::<FilesDrag>().is_some() {
                ui.painter().rect_stroke(
                    resp.rect,
                    Style::RADIUS,
                    Stroke::new(1.0, Style::ACCENT),
                    StrokeKind::Inside,
                );
            }
            if let Some(payload) = resp.dnd_release_payload::<FilesDrag>() {
                actions.push(Action::Drop {
                    source_pane: payload.source_pane,
                    dest_dir: PathBuf::from(path),
                    copy: mods.command,
                });
            }
        }
    }
}

/// Paint the rubber-band and select the rows it covers (view geometry; the model
/// just stores the resulting set). Driven by the background sensor's drag.
fn rubber_band(
    ui: &egui::Ui,
    pane_ix: usize,
    bg: &egui::Response,
    rects: &[(usize, Rect)],
    actions: &mut Vec<Action>,
) {
    let id = ui.make_persistent_id(("rubber-origin", pane_ix));
    if bg.drag_started() {
        if let Some(p) = bg.interact_pointer_pos() {
            ui.data_mut(|d| d.insert_temp(id, p));
        }
    }
    if bg.dragged() {
        let origin: Option<Pos2> = ui.data(|d| d.get_temp(id));
        if let (Some(origin), Some(cur)) = (origin, bg.interact_pointer_pos()) {
            let band = Rect::from_two_pos(origin, cur);
            ui.painter()
                .rect_filled(band, 0.0, Style::ACCENT.gamma_multiply(0.15));
            ui.painter().rect_stroke(
                band,
                0.0,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
            let covered: BTreeSet<usize> = rects
                .iter()
                .filter(|(_, r)| r.intersects(band))
                .map(|(i, _)| *i)
                .collect();
            actions.push(Action::Rubber(pane_ix, covered));
        }
    }
}

/// A drop onto the pane background = a transfer into the pane's current
/// directory. Runs after the rows, so a folder-row drop (which takes the
/// payload first) wins over a background drop.
fn pane_drop(
    ui: &egui::Ui,
    b: &FileBrowser,
    pane_ix: usize,
    bg: &egui::Response,
    actions: &mut Vec<Action>,
) {
    let tab = b.pane(pane_ix).active_tab();
    if bg.dnd_hover_payload::<FilesDrag>().is_some() {
        let copy = ui.input(|i| i.modifiers.command);
        ui.painter().rect_stroke(
            bg.rect,
            Style::RADIUS,
            Stroke::new(1.0, Style::ACCENT),
            StrokeKind::Inside,
        );
        if let Some(pos) = bg.hover_pos() {
            ui.painter().text(
                egui::pos2(pos.x + Style::SP_M, pos.y),
                Align2::LEFT_CENTER,
                if copy { "Copy here" } else { "Move here" },
                FontId::proportional(Style::SMALL),
                Style::ACCENT,
            );
        }
    }
    if let Some(payload) = bg.dnd_release_payload::<FilesDrag>() {
        if let Some(dest) = tab.current_dir() {
            actions.push(Action::Drop {
                source_pane: payload.source_pane,
                dest_dir: dest,
                copy: ui.input(|i| i.modifiers.command),
            });
        }
    }
}

// ── Previews + thumbnails + quick-look (FILEMGR-10) ───────────────────────────

/// Push the "want a thumbnail" intent for a visible image cell. Pushed every
/// frame the cell is on screen — the model dedups, and the stream of wants is
/// exactly the LRU recency signal, so eviction tracks visibility. A **remote**
/// (mesh-mount) file is only requested while *selected* (lock 18): a remote
/// directory is never bulk-decoded over sshfs just by being scrolled past.
fn want_thumb(ui: &egui::Ui, e: &EntryView, rect: Rect, actions: &mut Vec<Action>) {
    if e.kind != PreviewKind::Image {
        return;
    }
    let Some(path) = &e.path else { return };
    if !ui.is_rect_visible(rect) {
        return;
    }
    if e.remote && !e.selected {
        return;
    }
    actions.push(Action::NeedThumb(path.clone()));
}

/// Paint a decoded thumbnail fitted into `cell`. Returns `false` when there is
/// nothing decoded to draw (pending / failed / not an image) — the caller then
/// paints the honest type tag as the placeholder icon.
fn draw_thumb(ui: &egui::Ui, b: &FileBrowser, e: &EntryView, cell: Rect) -> bool {
    if e.kind != PreviewKind::Image {
        return false;
    }
    let Some(path) = &e.path else { return false };
    let Some(ThumbState::Ready { stamp, pixels }) = b.thumb_state(path) else {
        return false;
    };
    let tex = preview_texture(ui, &format!("t:{path}"), *stamp, pixels);
    let fitted = fit_rect(cell, pixels.size);
    egui::Image::new(SizedTexture::new(tex.id(), fitted.size())).paint_at(ui, fitted);
    true
}

/// The GPU-texture cache over the decoded rasters, held in egui memory so the
/// stateless `files_panel` entry point stays state-free. Keyed by
/// `t:<path>` / `p:<path>` with the delivery stamp — a re-decode (cache bust)
/// re-uploads, and the count-bounded prune drops the oldest stamps so GPU
/// memory stays bounded alongside the pixel LRUs.
fn preview_texture(ui: &egui::Ui, key: &str, stamp: u64, pixels: &Pixels) -> TextureHandle {
    #[derive(Clone, Default)]
    struct TexCache(Arc<Mutex<HashMap<String, (u64, TextureHandle)>>>);
    /// Slightly over the pixel caches' combined cap, so pruning here is rare.
    const TEX_CAP: usize = 600;

    // Fetch the shared map handle only inside `data_mut`; the upload itself
    // runs outside it (a `load_texture` inside `data_mut` re-enters the ctx
    // lock and deadlocks — same caveat as the shell backdrop).
    let cache = ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<TexCache>(egui::Id::new("files-preview-tex"))
            .clone()
    });
    let mut map = cache
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_stamp, tex)) = map.get(key) {
        if *cached_stamp == stamp {
            return tex.clone();
        }
    }
    let image = egui::ColorImage::from_rgba_unmultiplied(pixels.size, &pixels.rgba);
    let tex = ui.ctx().load_texture(
        format!("files-preview:{key}"),
        image,
        TextureOptions::LINEAR,
    );
    map.insert(key.to_string(), (stamp, tex.clone()));
    if map.len() > TEX_CAP {
        // Stamps are monotonic — drop the oldest down to the cap.
        let mut stamps: Vec<u64> = map.values().map(|(s, _)| *s).collect();
        stamps.sort_unstable();
        let cutoff = stamps[map.len() - TEX_CAP];
        map.retain(|_, (s, _)| *s >= cutoff);
    }
    tex
}

/// Center `size` (a decoded raster) inside `cell`, scaled to fit while keeping
/// aspect.
#[allow(clippy::cast_precision_loss)] // raster dims are ≤ PREVIEW_PX — exact in f32.
fn fit_rect(cell: Rect, size: [usize; 2]) -> Rect {
    let (w, h) = (size[0] as f32, size[1] as f32);
    if w <= 0.0 || h <= 0.0 {
        return Rect::from_center_size(cell.center(), egui::Vec2::ZERO);
    }
    let scale = (cell.width() / w).min(cell.height() / h);
    Rect::from_center_size(cell.center(), egui::vec2(w * scale, h * scale))
}

/// A per-frame snapshot of the preview target (the active tab's focused
/// selection), so the pane/overlay hold no `&FileRow` while pushing intents.
struct PreviewTarget {
    name: String,
    size: String,
    age: String,
    mime: Mime,
    path: Option<String>,
    remote: bool,
    kind: PreviewKind,
}

fn preview_target_of(b: &FileBrowser) -> Option<PreviewTarget> {
    let r = b.preview_target()?;
    Some(PreviewTarget {
        name: r.name.clone(),
        size: r.size.clone(),
        age: r.age.clone(),
        mime: r.mime,
        path: r.path.clone(),
        remote: r.path.as_deref().is_some_and(|p| b.is_remote_path(p)),
        kind: PreviewKind::detect(&r.name, r.is_dir()),
    })
}

/// FILEMGR-10 — the toggleable right-hand preview pane.
fn preview_pane(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    egui::SidePanel::right("files-preview")
        .default_width(Style::SP_XL * 9.0)
        .show_inside(ui, |ui| {
            ui.add_space(Style::SP_S);
            section_header(ui, "PREVIEW");
            ui.add_space(Style::SP_XS);
            let Some(target) = preview_target_of(b) else {
                muted_note(
                    ui,
                    "Select a file to preview it \u{2014} Space quick-looks.",
                );
                return;
            };
            preview_body(ui, b, &target, false, actions);
        });
}

/// The shared preview renderer — the pane and the quick-look overlay draw the
/// same content with different size budgets (`big`). Draw-only: every decode
/// request goes out as an [`Action`] and runs on the worker thread.
fn preview_body(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    t: &PreviewTarget,
    big: bool,
    actions: &mut Vec<Action>,
) {
    // Identity + the cheap metadata the listing already carries (no stat here).
    let title_size = if big { Style::HEADING } else { Style::BODY };
    ui.label(
        RichText::new(&t.name)
            .color(Style::TEXT)
            .strong()
            .size(title_size),
    );
    let mut meta = format!("{} \u{b7} {}", mime_tag(t.mime), t.size);
    if !t.age.is_empty() && t.age != "\u{2014}" {
        meta.push_str(" \u{b7} ");
        meta.push_str(&t.age);
    }
    if t.remote {
        meta.push_str(" \u{b7} remote");
    }
    muted_note(ui, meta);
    ui.add_space(Style::SP_S);

    match t.kind {
        PreviewKind::Folder => {
            muted_note(ui, "Folder \u{2014} open it to browse.");
        }
        // Lock 23 — unviewable/unknown types get an honest "no handler",
        // never an external-app spawn.
        PreviewKind::NoViewer(label) => {
            muted_note(ui, format!("No built-in viewer \u{2014} {label}."));
        }
        PreviewKind::ImageNoDecoder => {
            muted_note(ui, "Image \u{2014} no built-in decoder for this format.");
        }
        PreviewKind::VideoNoProbe => {
            muted_note(
                ui,
                "Video \u{2014} no built-in reader for this container: no frame preview or duration.",
            );
        }
        PreviewKind::Image | PreviewKind::Text(_) | PreviewKind::Audio | PreviewKind::Video => {
            preview_decoded_body(ui, b, t, big, actions);
        }
    }
}

/// The worker-decoded part of a preview (image / text / media), by cache state.
fn preview_decoded_body(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    t: &PreviewTarget,
    big: bool,
    actions: &mut Vec<Action>,
) {
    let Some(path) = &t.path else {
        muted_note(
            ui,
            "No local path \u{2014} mount the peer (Mesh sidebar) to preview.",
        );
        return;
    };
    match b.preview_state(path) {
        // Lock 18 — a remote file is never auto-read over sshfs: an honest
        // on-demand affordance instead.
        None if t.remote => {
            muted_note(ui, "Remote file \u{2014} preview on demand.");
            if ui.button("Load preview").clicked() {
                actions.push(Action::NeedPreview(path.clone()));
            }
        }
        // A cold local slot requests the decode; a Pending one just keeps its
        // LRU slot warm — both draw the same honest loading note.
        None | Some(PreviewState::Pending) => {
            actions.push(Action::NeedPreview(path.clone()));
            muted_note(ui, "Loading preview\u{2026}");
        }
        Some(PreviewState::Failed(reason)) => {
            actions.push(Action::NeedPreview(path.clone()));
            ui.colored_label(Style::WARN, format!("No preview \u{2014} {reason}"));
        }
        Some(PreviewState::Ready { stamp, data }) => {
            // The re-push keeps the LRU slot warm while it's on screen.
            actions.push(Action::NeedPreview(path.clone()));
            render_preview_data(ui, path, *stamp, data.as_ref(), t, big);
        }
    }
}

fn render_preview_data(
    ui: &mut egui::Ui,
    path: &str,
    stamp: u64,
    data: &PreviewData,
    t: &PreviewTarget,
    big: bool,
) {
    match data {
        PreviewData::Image { pixels, full } => {
            let budget_h = if big {
                (ui.available_height() - Style::SP_XL).max(Style::SP_XL)
            } else {
                Style::SP_XL * 8.0
            };
            let cell =
                Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), budget_h));
            let fitted = fit_rect(cell, pixels.size);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), fitted.height()),
                Sense::hover(),
            );
            let draw = Rect::from_center_size(rect.center(), fitted.size());
            let tex = preview_texture(ui, &format!("p:{path}"), stamp, pixels);
            egui::Image::new(SizedTexture::new(tex.id(), draw.size())).paint_at(ui, draw);
            muted_note(ui, format!("{} \u{d7} {} px", full[0], full[1]));
        }
        PreviewData::Text { lines, truncated } => {
            let budget_h = if big {
                (ui.available_height() - Style::SP_L).max(Style::SP_XL)
            } else {
                Style::SP_XL * 10.0
            };
            egui::ScrollArea::vertical()
                .id_salt(("files-preview-text", big))
                .max_height(budget_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(text_layout_job(lines));
                });
            if *truncated {
                muted_note(ui, "\u{2026} preview truncated.");
            }
        }
        PreviewData::Media(meta) => {
            match meta.duration_secs {
                Some(secs) => field(ui, "Duration", &fmt_duration(secs), Style::TEXT),
                None => field(ui, "Duration", "unknown", Style::TEXT_DIM),
            }
            if let Some(codec) = &meta.codec {
                field(ui, "Codec", codec, Style::TEXT);
            }
            if let Some(rate) = meta.sample_rate {
                field(ui, "Sample rate", &format!("{rate} Hz"), Style::TEXT);
            }
            if let Some(ch) = meta.channels {
                field(ui, "Channels", &ch.to_string(), Style::TEXT);
            }
            if t.kind == PreviewKind::Video {
                ui.add_space(Style::SP_XS);
                muted_note(
                    ui,
                    "Container metadata only \u{2014} the shell ships no video frame decoder.",
                );
            }
        }
    }
}

/// Lay the worker-tokenized lines out as one monospace galley, each token in
/// its Carbon tone (§4 — tokens only, no raw colours).
fn text_layout_job(lines: &[Vec<TokenSpan>]) -> egui::text::LayoutJob {
    let fmt = |kind: TokenKind| egui::TextFormat {
        font_id: FontId::monospace(Style::SMALL),
        color: token_color(kind),
        ..egui::TextFormat::default()
    };
    let mut job = egui::text::LayoutJob::default();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            job.append("\n", 0.0, fmt(TokenKind::Plain));
        }
        for (span, kind) in line {
            job.append(span, 0.0, fmt(*kind));
        }
    }
    job
}

/// The Carbon tone for a syntax-ish token class (§4 — no raw hex).
const fn token_color(kind: TokenKind) -> Color32 {
    match kind {
        TokenKind::Plain => Style::TEXT,
        TokenKind::Keyword => Style::ACCENT_HI,
        TokenKind::Comment => Style::TEXT_DIM,
        TokenKind::Str => Style::OK,
        TokenKind::Number => Style::WARN,
        TokenKind::Heading => Style::ACCENT,
    }
}

/// `m:ss` / `h:mm:ss` for a probed duration.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // durations are small +ve.
fn fmt_duration(secs: f64) -> String {
    let total = secs.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// FILEMGR-10 — the Space quick-look: a modal overlay dimming the shell and
/// centering the built-in viewer at full size. Space / Escape / a backdrop
/// click closes it. Same `preview_body` the pane draws — one viewer, two
/// surfaces, zero external programs (§9).
fn quick_look_overlay(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    if !b.quick_look_open() {
        return;
    }
    let Some(target) = preview_target_of(b) else {
        // The selection vanished under the overlay — nothing left to look at.
        actions.push(Action::CloseQuickLook);
        return;
    };
    let ctx = ui.ctx().clone();
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("files-quicklook"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(&ctx, |ui| {
            // The dim backdrop swallows pointer input under the modal; a click
            // on it closes.
            let dim = ui.interact(screen, egui::Id::new("files-quicklook-dim"), Sense::click());
            ui.painter()
                .rect_filled(screen, 0.0, Style::BG.gamma_multiply(0.94));
            if dim.clicked() {
                actions.push(Action::CloseQuickLook);
            }
            let card = Rect::from_center_size(
                screen.center(),
                egui::vec2(screen.width() * 0.72, screen.height() * 0.84),
            );
            ui.painter()
                .rect_filled(card, Style::RADIUS, Style::SURFACE);
            ui.painter().rect_stroke(
                card,
                Style::RADIUS,
                Stroke::new(1.0, Style::BORDER),
                StrokeKind::Inside,
            );
            let inner = card.shrink(Style::SP_M);
            let mut body = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            preview_body(&mut body, b, &target, true, actions);
        });
}

// ── Bottom: op-queue progress strip + status line ─────────────────────────────

fn op_strip(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let ops = b.ops();
    if ops.active().is_empty() {
        return;
    }
    for op in ops.active() {
        ui.horizontal(|ui| {
            let frac = op.progress.as_ref().map_or(0.0, Progress::fraction);
            ui.label(
                RichText::new(&op.label)
                    .size(Style::SMALL)
                    .color(Style::TEXT),
            );
            ui.add(
                egui::ProgressBar::new(frac)
                    .desired_width(Style::SP_XL * 5.0)
                    .fill(Style::ACCENT)
                    .text(RichText::new(op_status(op)).size(Style::SMALL)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if op.is_done() {
                    if ui.small_button("Dismiss").clicked() {
                        actions.push(Action::DismissOp(op.op_id));
                    }
                } else {
                    if ui.small_button("Cancel").clicked() {
                        actions.push(Action::CancelOp(op.op_id));
                    }
                    if op.control.is_paused() {
                        if ui.small_button("Resume").clicked() {
                            actions.push(Action::ResumeOp(op.op_id));
                        }
                    } else if ui.small_button("Pause").clicked() {
                        actions.push(Action::PauseOp(op.op_id));
                    }
                }
            });
        });
    }
    ui.separator();
}

fn op_status(op: &crate::ops::ActiveOp) -> String {
    if let Some(out) = &op.outcome {
        if out.cancelled {
            return format!("Cancelled ({} done)", out.items_completed);
        }
        if let Some(err) = &out.error {
            return format!("Failed: {err}");
        }
        return format!("Done ({} items)", out.items_completed);
    }
    op.progress.as_ref().map_or_else(
        || "Starting…".to_string(),
        |p| {
            let pct = p.fraction() * 100.0;
            p.eta().map_or_else(
                || format!("{pct:.0}%  {}/{} files", p.files_done, p.files_total),
                |eta| {
                    format!(
                        "{pct:.0}%  {}/{} files  ~{}s",
                        p.files_done,
                        p.files_total,
                        eta.as_secs()
                    )
                },
            )
        },
    )
}

fn status_line(ui: &mut egui::Ui, b: &FileBrowser) {
    if let Some(note) = b.last_note() {
        ui.colored_label(Style::WARN, note);
    }
    match b.last_send() {
        SendOutcome::Idle => {
            muted_note(
                ui,
                "Select a local file, choose a reachable peer, then Send \u{2014} or drag between panes to move (Ctrl to copy).",
            );
        }
        SendOutcome::Sent { op_id, file, peer } => {
            ui.colored_label(
                Style::OK,
                format!("Sent {file} \u{2192} {peer}  (op #{op_id})"),
            );
        }
        SendOutcome::Failed(err) => {
            ui.colored_label(Style::DANGER, format!("Send failed: {err}"));
        }
    }
}

// ── Empty state + small helpers ───────────────────────────────────────────────

fn empty_state(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize) {
    ui.add_space(Style::SP_XL);
    ui.vertical_centered(|ui| {
        let loc = b.pane(pane_ix).active_tab().location();
        let (title, body) = match loc {
            Location::Local(_) => (
                "Nothing here",
                "This directory is empty, or it doesn't exist on this node.",
            ),
            Location::Peer(_) if b.mesh_overlay().is_none() => (
                "No mesh connection",
                "This node isn't on a live mesh, so no peer files can be listed.",
            ),
            Location::Peer(_) => ("No shared files", "This peer is sharing nothing right now."),
        };
        ui.label(
            RichText::new(title)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        muted_note(ui, body);
    });
}

fn paint_entry_bg(ui: &egui::Ui, rect: Rect, selected: bool, hovered: bool) {
    if selected {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::ACCENT.gamma_multiply(0.30));
    } else if hovered {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
}

fn name_color(e: &EntryView) -> egui::Color32 {
    if e.is_dir {
        Style::ACCENT
    } else {
        Style::TEXT
    }
}

fn node_role(b: &FileBrowser) -> &'static str {
    match b.mesh_overlay() {
        Some(m) if m.is_lighthouse => "this node \u{b7} Lighthouse",
        Some(_) => "this node \u{b7} Workstation",
        None => "this node",
    }
}

fn mesh_badge(ui: &mut egui::Ui, b: &FileBrowser) {
    ui.horizontal(|ui| {
        if let Some(mesh) = b.mesh_overlay() {
            status_dot(ui, Style::OK);
            let mut label = if mesh.mesh_id.is_empty() {
                "on the mesh".to_string()
            } else {
                format!("mesh {}", mesh.mesh_id)
            };
            if !mesh.active_transport.is_empty() {
                label.push_str(" \u{b7} via ");
                label.push_str(&mesh.active_transport);
            }
            muted_note(ui, label);
        } else {
            status_dot(ui, Style::WARN);
            muted_note(ui, "Standalone \u{2014} no mesh");
        }
    });
}

fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
}

const fn mime_tag(mime: Mime) -> &'static str {
    match mime {
        Mime::Folder => "DIR",
        Mime::Doc => "DOC",
        Mime::Image => "IMG",
        Mime::Pdf => "PDF",
        Mime::Archive => "ZIP",
        Mime::Disk => "DSK",
    }
}

const fn peer_color(status: PeerStatus) -> egui::Color32 {
    match status {
        PeerStatus::Online | PeerStatus::Self_ => Style::OK,
        PeerStatus::Idle => Style::WARN,
        PeerStatus::Offline => Style::TEXT_DIM,
    }
}

#[cfg(test)]
mod tests {
    use super::files_panel;
    use crate::model::{FileBrowser, Location, ViewMode};
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;
    use mde_files::backend::{
        AuditEntry, Backend, BackendError, ConflictPolicy, Destination, MeshOverlayBadge, OpId,
        SendMode,
    };
    use mde_files::fileops::{FakeFileOps, FileOps};
    use mde_files::model::{FileRow, Mime, Peer, PeerKind, PeerStatus, SelfNode};
    use std::path::PathBuf;

    /// A backend double for the render tests: a curated local listing (rows with
    /// real paths) + a small peer roster + a per-peer listing. No live Bus.
    struct RenderFixture {
        peers: Vec<Peer>,
        rows: Vec<FileRow>,
    }

    impl RenderFixture {
        fn populated() -> Self {
            let rows = vec![
                FileRow::local(
                    "alpha/",
                    Mime::Folder,
                    "\u{2014} \u{b7} 3 items",
                    "\u{2014}",
                )
                .with_path("/data/alpha"),
                FileRow::local("report.pdf", Mime::Pdf, "2.4 MB", "4 min")
                    .with_path("/data/report.pdf"),
                FileRow::local("photo.png", Mime::Image, "812 KB", "2 h")
                    .with_path("/data/photo.png"),
                FileRow::local(".hidden", Mime::Doc, "1 KB", "1 d").with_path("/data/.hidden"),
            ];
            Self {
                peers: vec![
                    Peer {
                        id: "pine".into(),
                        host: "pine.mesh".into(),
                        label: "workstation".into(),
                        kind: PeerKind::Desktop,
                        addr: "10.0.0.14".into(),
                        status: PeerStatus::Online,
                        latency: Some(14),
                        files: 0,
                        shared: 0,
                        last: "now".into(),
                    },
                    Peer {
                        id: "cedar".into(),
                        host: "cedar.mesh".into(),
                        label: "build runner".into(),
                        kind: PeerKind::Server,
                        addr: String::new(),
                        status: PeerStatus::Offline,
                        latency: None,
                        files: 0,
                        shared: 0,
                        last: "2 h".into(),
                    },
                ],
                rows,
            }
        }
    }

    impl Backend for RenderFixture {
        fn self_node(&self) -> SelfNode {
            SelfNode {
                host: "fixture.mesh".into(),
                ..SelfNode::default()
            }
        }
        fn peers(&self) -> Vec<Peer> {
            self.peers.clone()
        }
        fn list(&self, path: &str) -> Vec<FileRow> {
            if let Some(id) = path.strip_prefix("peer:") {
                if id == "pine" {
                    return self.rows.clone();
                }
                return Vec::new();
            }
            self.rows.clone()
        }
        fn audit_log(&self) -> Vec<AuditEntry> {
            Vec::new()
        }
        fn send_to(
            &mut self,
            _sources: &[PathBuf],
            destination: Destination,
            _mode: SendMode,
            _conflict: ConflictPolicy,
        ) -> Result<OpId, BackendError> {
            Err(BackendError::DestinationUnreachable(destination))
        }
        fn rollback(&mut self, op_id: OpId) -> Result<OpId, BackendError> {
            Err(BackendError::NotFound(op_id))
        }
        fn mesh_overlay(&self) -> Option<MeshOverlayBadge> {
            None
        }
    }

    fn browser() -> FileBrowser {
        FileBrowser::with_file_ops(Box::new(RenderFixture::populated()), FakeFileOps::new())
    }

    /// Drive one headless egui frame that renders [`files_panel`] into a real
    /// `CentralPanel`, then tessellate the result on the CPU so any paint-path
    /// fault surfaces as a test failure. This is the harness's headless "mount"
    /// test: no window, no GPU, no live Bus — the same path the shell mounts.
    fn mount(browser: &mut FileBrowser) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 700.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                files_panel(ui, browser);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "files_panel produced no draw primitives");
    }

    #[test]
    fn mounts_and_renders_the_list_view() {
        let mut b = browser();
        assert_eq!(b.active_tab().view(), ViewMode::List);
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_the_grid_view() {
        let mut b = browser();
        b.set_view(0, ViewMode::Grid);
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_the_details_view_with_headers() {
        let mut b = browser();
        b.set_view(0, ViewMode::Details);
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_dual_pane() {
        let mut b = browser();
        b.toggle_dual();
        b.set_active_pane(1);
        b.navigate(1, Location::Peer("pine".into()));
        assert!(b.is_dual());
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_the_empty_no_mesh_state() {
        let mut b = browser();
        b.navigate(0, Location::Peer("ghost".into()));
        assert!(b.active_tab().rows().is_empty());
        assert!(b.mesh_overlay().is_none());
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_the_op_progress_strip() {
        // A real fake-FS transfer so the strip has a live op to draw.
        let fs = FakeFileOps::new();
        fs.create_dir(std::path::Path::new("/data")).expect("mkdir");
        fs.create_dir(std::path::Path::new("/dst")).expect("mkdir");
        fs.seed_file("/data/report.pdf", b"x").expect("seed");
        let mut b = FileBrowser::with_file_ops(Box::new(RenderFixture::populated()), fs);
        b.set_view(0, ViewMode::Details);
        // Sorted (dirs-first, name asc, hidden filtered): [alpha/, photo.png,
        // report.pdf] → index 2 is the seeded file.
        b.click(0, 2);
        b.drop_transfer(0, PathBuf::from("/dst"), true);
        assert!(!b.ops().active().is_empty(), "an op is on the strip");
        mount(&mut b);
    }

    // ── the Mesh sidebar tree states (FILEMGR-9) ─────────────────────────────

    #[test]
    fn mounts_and_renders_the_mesh_sidebar_states() {
        use crate::mesh_mount::test_support::FakeMeshMount;
        use crate::mesh_mount::{MountPhase, MountScope, MountView};
        // pine (Online) shown mounted + escalated to full FS (its short mount host is
        // its roster label, "workstation"); cedar (Offline) is honestly greyed. The
        // render exercises both the mount chip + the Full FS escalation control.
        let fake = FakeMeshMount::new().with_view(
            "workstation",
            MountView {
                phase: MountPhase::Mounted,
                scope: Some(MountScope::Full),
                path: Some("/run/user/1000/mde-mesh/workstation".into()),
                reason: None,
            },
        );
        let mut b =
            FileBrowser::with_file_ops(Box::new(RenderFixture::populated()), FakeFileOps::new())
                .with_mesh_mount(Box::new(fake));
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_a_transitional_and_unreachable_peer() {
        use crate::mesh_mount::test_support::FakeMeshMount;
        use crate::mesh_mount::{MountPhase, MountView};
        let fake = FakeMeshMount::new()
            .with_view(
                "workstation",
                MountView {
                    phase: MountPhase::Mounting,
                    scope: None,
                    path: None,
                    reason: None,
                },
            )
            .with_view(
                "build runner",
                MountView {
                    phase: MountPhase::Unreachable,
                    scope: None,
                    path: None,
                    reason: Some("unreachable: offline".into()),
                },
            );
        let mut b =
            FileBrowser::with_file_ops(Box::new(RenderFixture::populated()), FakeFileOps::new())
                .with_mesh_mount(Box::new(fake));
        assert!(b.any_mount_transitional());
        mount(&mut b);
    }

    // ── previews + thumbnails + quick-look (FILEMGR-10) ──────────────────────

    use crate::preview::{PreviewState, ThumbState};
    use std::time::{Duration, Instant};

    /// Write a real PNG to a scratch path so the decode worker exercises the
    /// exact production read → decode → deliver → texture-upload pipeline.
    fn scratch_png(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("mde-files-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir scratch");
        let path = dir.join(name);
        let img = image::RgbaImage::from_pixel(48, 24, image::Rgba([40, 160, 220, 255]));
        image::DynamicImage::ImageRgba8(img)
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("write scratch png");
        path.to_string_lossy().into_owned()
    }

    /// A browser whose first row is a real on-disk PNG (name sorts it first).
    fn browser_with_real_image(path: &str) -> FileBrowser {
        let rows = vec![
            FileRow::local("art.png", Mime::Image, "1 KB", "now").with_path(path),
            FileRow::local("report.pdf", Mime::Pdf, "2.4 MB", "4 min")
                .with_path("/data/report.pdf"),
        ];
        let fixture = RenderFixture {
            peers: Vec::new(),
            rows,
        };
        FileBrowser::with_file_ops(Box::new(fixture), FakeFileOps::new())
    }

    /// Pump until both the thumbnail and the preview for `path` are decoded.
    fn wait_decoded(b: &mut FileBrowser, path: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            b.pump_previews();
            let thumb = matches!(b.thumb_state(path), Some(ThumbState::Ready { .. }));
            let preview = matches!(b.preview_state(path), Some(PreviewState::Ready { .. }));
            if thumb && preview {
                return;
            }
            assert!(Instant::now() < deadline, "decode worker never delivered");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn preview_pane_renders_an_honest_no_viewer_for_a_pdf() {
        let mut b = browser();
        assert!(b.preview_pane_open(), "the pane ships open");
        // Sorted display: [alpha/, photo.png, report.pdf] → index 2 = the PDF,
        // which has no built-in viewer (lock 23) → the honest "no handler".
        b.click(0, 2);
        assert_eq!(b.preview_target().expect("target").name, "report.pdf");
        mount(&mut b);
    }

    #[test]
    fn quick_look_renders_over_the_surface_even_while_loading() {
        let mut b = browser();
        // photo.png's fixture path doesn't exist on disk — the decode fails
        // honestly off-thread while the overlay renders its loading state.
        b.click(0, 1);
        b.toggle_quick_look();
        assert!(b.quick_look_open());
        mount(&mut b);
    }

    #[test]
    fn thumbnails_and_previews_decode_and_render_headless() {
        // The full FILEMGR-10 pipeline, no GPU: a real PNG on disk is decoded
        // by the worker thread, delivered over the channel, uploaded as a
        // texture, and drawn in the Grid tile, the List thumbnail column, the
        // preview pane, and the quick-look overlay.
        let path = scratch_png("art.png");
        let mut b = browser_with_real_image(&path);
        b.click(0, 0);
        assert_eq!(b.preview_target().expect("target").name, "art.png");
        b.request_thumb(&path);
        b.request_preview(&path);
        wait_decoded(&mut b, &path);
        let Some(PreviewState::Ready { data, .. }) = b.preview_state(&path) else {
            unreachable!("wait_decoded proved the Ready state");
        };
        assert!(
            matches!(
                data.as_ref(),
                crate::preview::PreviewData::Image { full: [48, 24], .. }
            ),
            "the preview carries the file's real dimensions"
        );
        // Grid tiles + the preview pane.
        b.set_view(0, ViewMode::Grid);
        mount(&mut b);
        // The List thumbnail column.
        b.set_view(0, ViewMode::List);
        assert!(b.list_thumbs());
        mount(&mut b);
        // The quick-look overlay at full size.
        b.toggle_quick_look();
        assert!(b.quick_look_open());
        mount(&mut b);
    }
}

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
use mde_egui::menubar::{MenuBar as SharedMenuBar, MenuBarModel};
use mde_egui::style::Elevation;
use mde_egui::{field, muted_note, status_dot, Motion, Style};
use mde_theme::brand::icons::{icon_image, IconId};

use mde_files::model::{Mime, PeerStatus};
use mde_files::opqueue::{Progress, Resolution};
use mde_files::search::TypeFilter;

use crate::dialogs::{Perm, PermClass};
use crate::mesh_mount::{MountPhase, MountView};
use crate::model::{
    mount_host_of, FileBrowser, Location, SearchForm, SendOutcome, SortKey, SortSpec, SurfaceTab,
    ViewMode, LOCAL_SPOTS,
};
use crate::preview::{
    Pixels, PreviewData, PreviewKind, PreviewState, ThumbState, TokenKind, TokenSpan,
};
use crate::transfers::{
    Method, NewTransferForm, StateFilter, TargetKind, TransferJob, TransferState, TransferTarget,
};

// Details-view column widths (right-hand columns; name takes the rest).
const COL_SIZE_W: f32 = 96.0;
const COL_TYPE_W: f32 = 56.0;
const COL_MOD_W: f32 = 80.0;
// Listing row height (List + Details); grid tile size.
const ROW_H: f32 = 26.0;
const TILE_W: f32 = 132.0;
const TILE_H: f32 = 72.0;
const ACTION_BUTTON_H: f32 = Style::TOOLBAR_CONTROL_H;
const FILES_ICON_BUTTON_ICON: f32 = 14.0;
const FILES_PLACE_ICON: f32 = 14.0;
const FILES_ROW_ICON: f32 = 15.0;
const FILES_GRID_ICON: f32 = 28.0;
const FILES_NAV_ICONS: &[(IconId, &str)] = &[
    (IconId::ArrowLeft, "Back"),
    (IconId::ArrowRight, "Forward"),
    (IconId::ChevronUp, "Up"),
];
const FILES_TAB_ICONS: &[(IconId, &str)] =
    &[(IconId::Close, "Close tab"), (IconId::NewTab, "New tab")];
const FILES_TOOLTIP_MAX_W: f32 = Style::SP_XL * 12.0;

#[derive(Clone, Copy)]
enum FilesActionTone {
    Quiet,
    Primary,
    Danger,
}

fn files_action_text(tone: FilesActionTone) -> Color32 {
    match tone {
        FilesActionTone::Quiet => Style::TEXT,
        FilesActionTone::Primary => Style::ACCENT,
        FilesActionTone::Danger => Style::DANGER,
    }
}

fn files_tooltip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS_S)
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(FILES_TOOLTIP_MAX_W);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(Style::TEXT)).wrap(),
            );
        });
}

fn files_context_menu(response: &egui::Response, add_contents: impl FnOnce(&mut egui::Ui)) {
    let previous_style = response.ctx.style();
    let mut menu_style = (*previous_style).clone();
    apply_files_popup_style(&response.ctx, &mut menu_style);
    response.ctx.set_style(menu_style);
    let _ = response.context_menu(|ui| {
        scope_files_popup_ui(ui);
        add_contents(ui);
    });
    response.ctx.set_style(previous_style);
}

fn files_submenu_button<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ui.menu_button(title, |ui| {
        scope_files_popup_ui(ui);
        add_contents(ui)
    })
}

fn scope_files_popup_ui(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    apply_files_popup_style(&ctx, ui.style_mut());
}

fn apply_files_popup_style(ctx: &egui::Context, style: &mut egui::Style) {
    let palette = Style::current_palette(ctx);
    let border = Stroke::new(1.0, palette.border);
    let text = Stroke::new(1.0, palette.text);
    let text_dim = Stroke::new(1.0, palette.text_dim);
    let accent = Style::resolve_color(ctx, Style::ACCENT);
    let visuals = &mut style.visuals;

    visuals.window_fill = palette.surface;
    visuals.panel_fill = palette.surface;
    visuals.faint_bg_color = palette.surface;
    visuals.extreme_bg_color = palette.bg;
    visuals.window_stroke = border;
    visuals.override_text_color = Some(palette.text);

    visuals.widgets.noninteractive.bg_fill = palette.surface;
    visuals.widgets.noninteractive.weak_bg_fill = palette.surface;
    visuals.widgets.noninteractive.bg_stroke = border;
    visuals.widgets.noninteractive.fg_stroke = text_dim;

    visuals.widgets.inactive.bg_fill = palette.surface;
    visuals.widgets.inactive.weak_bg_fill = palette.surface;
    visuals.widgets.inactive.bg_stroke = border;
    visuals.widgets.inactive.fg_stroke = text;

    visuals.widgets.hovered.bg_fill = palette.surface_hi;
    visuals.widgets.hovered.weak_bg_fill = palette.surface_hi;
    visuals.widgets.hovered.bg_stroke = border;
    visuals.widgets.hovered.fg_stroke = text;

    visuals.widgets.active.bg_fill = palette.surface_hi;
    visuals.widgets.active.weak_bg_fill = palette.surface_hi;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    visuals.widgets.active.fg_stroke = text;

    visuals.widgets.open.bg_fill = palette.surface_hi;
    visuals.widgets.open.weak_bg_fill = palette.surface_hi;
    visuals.widgets.open.bg_stroke = border;
    visuals.widgets.open.fg_stroke = text;

    visuals.selection.bg_fill = accent.gamma_multiply(0.25);
    visuals.selection.stroke = Stroke::new(1.0, accent);
    style.spacing.button_padding = egui::vec2(Style::SP_S, Style::CONTROL_PAD_Y);
    style.spacing.item_spacing = egui::vec2(Style::SP_XS, Style::TOOLBAR_INSET_Y);
}

fn scope_files_toolbar_ui(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.spacing.interact_size.y = Style::TOOLBAR_CONTROL_H;
    style.spacing.button_padding = egui::vec2(Style::SP_S, Style::CONTROL_PAD_Y);
    style.spacing.item_spacing = egui::vec2(Style::SP_XS, Style::TOOLBAR_INSET_Y);
}

trait FilesHoverExt {
    fn files_hover_text(self, text: impl Into<String>) -> Self;
    fn files_disabled_hover_text(self, text: impl Into<String>) -> Self;
}

impl FilesHoverExt for egui::Response {
    fn files_hover_text(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.on_hover_ui(move |ui| files_tooltip(ui, text.as_str()))
    }

    fn files_disabled_hover_text(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.on_disabled_hover_ui(move |ui| files_tooltip(ui, text.as_str()))
    }
}

fn files_action_button(
    ui: &mut egui::Ui,
    label: &str,
    tone: FilesActionTone,
    tip: &str,
) -> egui::Response {
    let text_color = files_action_text(tone);
    let response = ui.add(
        egui::Button::new(
            RichText::new(label)
                .size(Style::SMALL)
                .color(text_color)
                .strong(),
        )
        .fill(Style::LAYER_02)
        .stroke(Stroke::new(1.0, text_color.gamma_multiply(0.72)))
        .corner_radius(Style::RADIUS_S)
        .min_size(egui::vec2(52.0, ACTION_BUTTON_H)),
    );
    mde_egui::focus::paint_focus_ring(ui.painter(), response.rect, response.has_focus());
    response.files_hover_text(tip)
}

fn files_nav_button(ui: &mut egui::Ui, icon: IconId, enabled: bool, tip: &str) -> egui::Response {
    files_icon_button(ui, icon, enabled, tip)
}

fn files_icon_button(
    ui: &mut egui::Ui,
    icon: IconId,
    enabled: bool,
    label: &str,
) -> egui::Response {
    let text_color = if enabled {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .fill(Style::LAYER_02)
            .stroke(Stroke::new(1.0, text_color.gamma_multiply(0.72)))
            .corner_radius(Style::RADIUS_S)
            .min_size(egui::vec2(ACTION_BUTTON_H, ACTION_BUTTON_H)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let rect = egui::Rect::from_center_size(
        response.rect.center(),
        egui::vec2(FILES_ICON_BUTTON_ICON, FILES_ICON_BUTTON_ICON),
    );
    if let Some(tex) = files_icon_texture(ui, icon, FILES_ICON_BUTTON_ICON, text_color) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
    }
    mde_egui::focus::paint_focus_ring(ui.painter(), response.rect, response.has_focus());
    response.files_hover_text(label)
}

#[allow(
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 -> u32
    clippy::cast_sign_loss            // size_px >= 1.0 by the .max(1.0) clamp
)]
fn files_icon_texture(
    ui: &egui::Ui,
    id: IconId,
    logical_px: f32,
    tint: Color32,
) -> Option<TextureHandle> {
    let size_px = (logical_px * ui.ctx().pixels_per_point()).round().max(1.0) as u32;
    let tint = tint.to_array();
    let key = egui::Id::new(("files-yamis-icon", id.name(), size_px, tint));
    if let Some(cached) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<Option<TextureHandle>>(key))
    {
        return cached;
    }
    let texture = icon_image(id, size_px, tint).ok().map(|img| {
        let color = egui::ColorImage::from_rgba_unmultiplied(img.size_usize(), &img.rgba);
        ui.ctx()
            .load_texture(id.name(), color, TextureOptions::LINEAR)
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(key, texture.clone()));
    texture
}

fn paint_files_icon(ui: &egui::Ui, id: IconId, rect: Rect, logical_px: f32, tint: Color32) -> bool {
    let Some(tex) = files_icon_texture(ui, id, logical_px, tint) else {
        return false;
    };
    let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
    true
}

const fn file_type_icon(mime: Mime) -> IconId {
    match mime {
        Mime::Folder => IconId::FileFolder,
        Mime::Doc => IconId::FileDocument,
        Mime::Image => IconId::FileImage,
        Mime::Pdf => IconId::FilePdf,
        Mime::Archive => IconId::FileArchive,
        Mime::Disk => IconId::Storage,
    }
}

fn file_icon_tint(e: &EntryView) -> Color32 {
    if e.is_dir {
        Style::ACCENT
    } else {
        Style::TEXT_DIM
    }
}

fn local_place_icon(path: &str) -> IconId {
    match path {
        "local:home" => IconId::FileHome,
        "local:docs" => IconId::FileDocuments,
        "local:downloads" => IconId::FileDownloads,
        "local:root" => IconId::Storage,
        _ => IconId::FileFolder,
    }
}

fn local_place_row(ui: &mut egui::Ui, icon: IconId, label: &str, selected: bool) -> egui::Response {
    let height = ROW_H;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let fill = if selected {
        Style::LAYER_02
    } else if response.hovered() {
        Style::LAYER_01
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, Style::RADIUS_S, fill);
    }
    let tint = if selected {
        Style::ACCENT
    } else {
        Style::TEXT_DIM
    };
    let icon_rect = Rect::from_center_size(
        egui::pos2(
            rect.left() + Style::SP_S + FILES_PLACE_ICON * 0.5,
            rect.center().y,
        ),
        egui::vec2(FILES_PLACE_ICON, FILES_PLACE_ICON),
    );
    let _ = paint_files_icon(ui, icon, icon_rect, FILES_PLACE_ICON, tint);
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_L, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::monospace(Style::SMALL),
        if selected {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        },
    );
    mde_egui::focus::paint_focus_ring(ui.painter(), rect, response.has_focus());
    response.files_hover_text(label)
}

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
///
/// `pub(crate)` so the MENUBAR-ALL top bar ([`crate::menubar`]) can produce the
/// same intents its toolbar/keyboard/right-click twins already do — the menu bar
/// is a discoverable face over these seams, never a new dispatch path (§6).
pub(crate) enum Action {
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
    /// FILEMGR-11 — open the permanent-delete confirm for the pane's selection.
    RequestDelete(usize),
    /// FILEMGR-11 — a keystroke in the delete confirm's typed-arming echo.
    DeleteEcho(String),
    /// FILEMGR-11 — fire the armed permanent delete.
    ConfirmDelete,
    /// FILEMGR-11 — dismiss the delete confirm.
    CancelDelete,
    /// FILEMGR-11 — answer the collision an op is parked on.
    ResolveConflict {
        op_id: u64,
        resolution: Resolution,
        apply_to_all: bool,
    },
    /// FILEMGR-11 — open the Properties dialog for the pane's focused selection.
    OpenProperties(usize),
    /// FILEMGR-11 — toggle one rwx grid cell in the Properties dialog.
    PropToggle(PermClass, Perm),
    /// FILEMGR-11 — a keystroke in the Properties octal field.
    PropSetOctal(String),
    /// FILEMGR-11 — a keystroke in the Properties owner-uid field.
    PropSetUid(String),
    /// FILEMGR-11 — a keystroke in the Properties owner-gid field.
    PropSetGid(String),
    /// FILEMGR-11 — apply the Properties dialog's chmod / chown (reloads `pane`).
    PropApply(usize),
    /// FILEMGR-11 — close the Properties dialog.
    PropClose,
    Send,
    /// FILEMGR-12 — right-click Send-To: send the selection to this peer.
    SendToPeer(usize, String),
    /// FILEMGR-12 — right-click Send-in-Chat: transfer + drop a chat file card.
    SendInChat(usize, String),
    /// EDITOR-9 — right-click Send-to-Editor: open the focused file in the Editor
    /// surface (posts `action/editor/open`, drained by the shell's editor mount).
    SendToEditor(usize),
    /// FILEMGR-12 — copy the selection's paths to the shared shell clipboard.
    ClipCopy(usize),
    /// FILEMGR-12 — cut the selection's paths (a matching in-app paste moves).
    ClipCut(usize),
    /// FILEMGR-12 — paste the in-app clipboard set into the pane's directory.
    ClipPaste(usize),
    /// FILEMGR-12 — paste from the shared shell clipboard text (Ctrl+V), so a
    /// path copied in another surface pastes into Files.
    ClipPasteText(usize, String),
    PauseOp(u64),
    ResumeOp(u64),
    CancelOp(u64),
    DismissOp(u64),
    /// FILEMGR-4 — toggle the recursive-search bar.
    ToggleSearchBar,
    /// FILEMGR-4 — commit an edit to the search entry/filter form.
    SetSearchForm(SearchForm),
    /// FILEMGR-4 — run the search over `pane`'s current directory.
    RunSearch(usize),
    /// FILEMGR-4 — cancel the running search (results so far stay on screen).
    CancelSearch,
    /// FILEMGR-4 — leave search mode and restore `pane`'s folder listing.
    ClearSearch(usize),
    /// TRANSFERS-8 — switch the top-level surface (Files ↔ Transfers, Q1).
    SwitchSurface(SurfaceTab),
    /// TRANSFERS-8 — open a blank New Transfer dialog (Q13 entry 1).
    OpenNewTransfer,
    /// TRANSFERS-8 — open the New Transfer dialog pre-pointed at a destination.
    OpenNewTransferTo(String, Method),
    /// TRANSFERS-8 — commit an edit to the New Transfer form.
    NewTransferEdit(NewTransferForm),
    /// TRANSFERS-8 — submit the New Transfer dialog's job + close it.
    SubmitNewTransfer,
    /// TRANSFERS-8 — dismiss the New Transfer dialog.
    CancelNewTransfer,
    /// TRANSFERS-8 — right-click "Send to → `<target>`" (Q13 entry 2).
    SendToTarget(usize, TransferTarget),
    /// TRANSFERS-8 — drag-drop onto a destination (Q13 entry 3).
    DropOnTarget(usize, TransferTarget),
    /// TRANSFERS-8 — pause one ledger job.
    TransferPause(String),
    /// TRANSFERS-8 — resume one Paused job.
    TransferResume(String),
    /// TRANSFERS-8 — cancel one job (removes it from the ledger).
    TransferCancel(String),
    /// TRANSFERS-8 — pause every pausable job (Q16 menu).
    TransferPauseAll,
    /// TRANSFERS-8 — resume every Paused job (Q16 menu).
    TransferResumeAll,
    /// TRANSFERS-8 — cancel every terminal job (Clear-completed, Q16 menu).
    TransferClearCompleted,
    /// TRANSFERS-8 — set the Transfers view state-filter (Q16 menu).
    SetTransferStateFilter(StateFilter),
    /// TRANSFERS-8 — set the Transfers view method-filter (`None` = all lanes).
    SetTransferMethodFilter(Option<Method>),
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
    // FILEMGR-4 — fold the recursive-search worker's streamed hits into the
    // results tab before the frame reads the listing, and keep a heartbeat alive
    // while it walks so results appear live without input. The walk itself runs
    // off-thread (never this paint path).
    browser.pump_search();
    if browser.search_running() {
        ui.ctx().request_repaint_after(Duration::from_millis(80));
    }
    // TRANSFERS-8 — refresh the worker's ledger (a cheap, cadence-gated local
    // directory scan; never a peer probe). Keep a repaint heartbeat while any job
    // is in flight so live progress updates without input.
    browser.pump_transfers();
    if browser.transfers_active() {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }

    let mut actions: Vec<Action> = Vec::new();
    // FILEMGR-12 — Ctrl+C / Ctrl+X / Ctrl+V over the active pane, sharing the shell
    // clipboard. Kept off the text-edit path (path-edit / rename) so a paste there
    // lands in the field, never as a file transfer.
    clipboard_keys(ui, browser, &mut actions);
    // MENUBAR-ALL — the shared top bar (FILES title · File/Edit/View/Go/Share/Help
    // spine over real seams · live status cluster), registered before the toolbar
    // so it reads as the surface's chrome header (lock 11), above the quick-access
    // strip below it.
    menu_bar(ui, browser, &mut actions);
    // TRANSFERS-8 — the top-level surface tab strip (Files ↔ Transfers, Q1). The
    // MenuBar above + the sidebar/bottom strip below are shared across both (Q16);
    // only the central content + the file-specific toolbar switch.
    surface_tabs(ui, browser, &mut actions);
    let on_files = browser.surface_tab() == SurfaceTab::Files;
    if on_files {
        top_bar(ui, browser, &mut actions);
    }
    egui::TopBottomPanel::bottom("files-bottom").show_inside(ui, |ui| {
        op_strip(ui, browser, &mut actions);
        status_line(ui, browser);
    });
    sidebar(ui, browser, &mut actions);
    // FILEMGR-10 — the toggleable preview pane sits between the listing and the
    // window edge, previewing the focused selection with built-in viewers only.
    if on_files && browser.preview_pane_open() {
        preview_pane(ui, browser, &mut actions);
    }
    egui::CentralPanel::default().show_inside(ui, |ui| {
        if on_files {
            if browser.is_dual() {
                ui.columns(2, |cols| {
                    pane_view(&mut cols[0], browser, 0, &mut actions);
                    pane_view(&mut cols[1], browser, 1, &mut actions);
                });
            } else {
                pane_view(ui, browser, 0, &mut actions);
            }
        } else {
            // TRANSFERS-8 — the Transfers tab: the worker's live ledger.
            transfers_panel(ui, browser, &mut actions);
        }
    });
    // FILEMGR-10 — the Space quick-look modal, over everything (Files tab only).
    if on_files {
        quick_look_overlay(ui, browser, &mut actions);
    }
    // FILEMGR-11 — the operation dialogs, topmost. At most one shows at a time
    // (a worker parked on a collision is the most urgent, so it wins).
    operation_dialogs(ui, browser, &mut actions);
    // TRANSFERS-8 — the New Transfer dialog (Q13 entry 1) + the Destinations
    // manager window (Q16), both modal over either tab.
    new_transfer_dialog(ui, browser, &mut actions);
    destinations_window(ui, browser, &mut actions);

    let ctx = ui.ctx().clone();
    for action in actions {
        apply(&ctx, browser, action);
    }
}

/// FILEMGR-12 — the clipboard keyboard verbs over the active pane, sharing the
/// one shell clipboard: Ctrl+C copies + Ctrl+X cuts the selection's paths onto
/// it; a Ctrl+V arrives as an [`egui::Event::Paste`] carrying the shell
/// clipboard's text, so a path copied in ANY surface pastes here. Copy/Cut are
/// suppressed while a text field owns focus (the path-edit / rename box) so the
/// keystroke edits text as expected; a paste there is the field's, not ours.
fn clipboard_keys(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let active = b.active_pane_index();
    let editing = ui.memory(egui::Memory::focused).is_some();
    let (copy, cut, pasted) = ui.input(|i| {
        let copy = i.modifiers.command && i.key_pressed(egui::Key::C);
        let cut = i.modifiers.command && i.key_pressed(egui::Key::X);
        let pasted = i.events.iter().find_map(|e| match e {
            egui::Event::Paste(text) => Some(text.clone()),
            _ => None,
        });
        (copy, cut, pasted)
    });
    if !editing && copy {
        actions.push(Action::ClipCopy(active));
    }
    if !editing && cut {
        actions.push(Action::ClipCut(active));
    }
    // A paste with no field focused pastes the shell-clipboard paths into Files.
    if let Some(text) = pasted.filter(|_| !editing) {
        actions.push(Action::ClipPasteText(active, text));
    }
}

/// FILEMGR-11 — render whichever operation dialog is active (conflict / confirm-
/// delete / properties), each as a modal over a dimmed shell. The conflict dialog
/// wins because its op's worker is blocked waiting for the answer.
fn operation_dialogs(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    if b.pending_conflict().is_some() {
        conflict_dialog(ui, b, actions);
    } else if b.pending_delete().is_some() {
        delete_dialog(ui, b, actions);
    } else if b.properties().is_some() {
        properties_dialog(ui, b, actions);
    }
}

/// Apply a captured intent to the model. `ctx` is threaded through so the
/// clipboard writes (Cut/Copy) can put the paths on the shared shell clipboard.
// A flat one-arm-per-`Action` dispatch: it exceeds the line heuristic purely
// because the surface's intent set is large (FILEMGR-4's search verbs are the
// latest to land). Splitting it would only scatter a trivial 1:1 mapping across
// helpers, so the length is allowed here.
#[allow(clippy::too_many_lines)]
fn apply(ctx: &egui::Context, browser: &mut FileBrowser, action: Action) {
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
        Action::RequestDelete(p) => browser.request_delete(p),
        Action::DeleteEcho(text) => browser.set_delete_echo(text),
        Action::ConfirmDelete => browser.confirm_delete(),
        Action::CancelDelete => browser.cancel_delete(),
        Action::ResolveConflict {
            op_id,
            resolution,
            apply_to_all,
        } => browser.resolve_conflict(op_id, resolution, apply_to_all),
        Action::OpenProperties(p) => browser.open_properties(p),
        Action::PropToggle(class, perm) => browser.properties_toggle_perm(class, perm),
        Action::PropSetOctal(text) => browser.properties_set_octal(text),
        Action::PropSetUid(text) => browser.properties_set_uid(text),
        Action::PropSetGid(text) => browser.properties_set_gid(text),
        Action::PropApply(p) => browser.properties_apply(p),
        Action::PropClose => browser.close_properties(),
        Action::Send => {
            browser.send();
        }
        Action::SendToPeer(p, peer) => {
            browser.send_to_peer(p, &peer);
        }
        Action::SendInChat(p, peer) => {
            browser.send_in_chat(p, &peer);
        }
        Action::SendToEditor(p) => {
            browser.send_to_editor(p);
        }
        // Cut/Copy stage the set in the model AND mirror the paths onto the shared
        // shell clipboard (via `ctx`), so a path copied here pastes in any surface.
        Action::ClipCopy(p) => {
            if let Some(text) = browser.clip_copy(p) {
                ctx.copy_text(text);
            }
        }
        Action::ClipCut(p) => {
            if let Some(text) = browser.clip_cut(p) {
                ctx.copy_text(text);
            }
        }
        Action::ClipPaste(p) => {
            browser.clip_paste(p);
        }
        Action::ClipPasteText(p, text) => {
            browser.clip_paste_text(p, &text);
        }
        Action::PauseOp(id) => browser.pause_op(id),
        Action::ResumeOp(id) => browser.resume_op(id),
        Action::CancelOp(id) => browser.cancel_op(id),
        Action::DismissOp(id) => browser.dismiss_op(id),
        Action::ToggleSearchBar => browser.toggle_search_bar(),
        Action::SetSearchForm(form) => browser.set_search_form(form),
        Action::RunSearch(p) => browser.start_search(p),
        Action::CancelSearch => browser.cancel_search(),
        Action::ClearSearch(p) => browser.clear_search(p),
        // ── TRANSFERS-8 ──────────────────────────────────────────────────────
        Action::SwitchSurface(tab) => browser.set_surface_tab(tab),
        Action::OpenNewTransfer => browser.open_new_transfer(),
        Action::OpenNewTransferTo(dest, method) => browser.open_new_transfer_to(dest, method),
        Action::NewTransferEdit(form) => browser.set_new_transfer_form(form),
        Action::SubmitNewTransfer => browser.submit_new_transfer(),
        Action::CancelNewTransfer => browser.cancel_new_transfer(),
        Action::SendToTarget(p, target) => browser.send_to_target(p, &target),
        Action::DropOnTarget(p, target) => browser.drop_on_target(p, &target),
        Action::TransferPause(id) => browser.transfer_pause(&id),
        Action::TransferResume(id) => browser.transfer_resume(&id),
        Action::TransferCancel(id) => browser.transfer_cancel(&id),
        Action::TransferPauseAll => browser.transfer_pause_all(),
        Action::TransferResumeAll => browser.transfer_resume_all(),
        Action::TransferClearCompleted => browser.transfer_clear_completed(),
        Action::SetTransferStateFilter(state) => {
            let mut f = browser.transfers_filter();
            f.state = state;
            browser.set_transfers_filter(f);
        }
        Action::SetTransferMethodFilter(method) => {
            let mut f = browser.transfers_filter();
            f.method = method;
            browser.set_transfers_filter(f);
        }
    }
}

// ── MENUBAR-ALL shared top bar ─────────────────────────────────────────────────

/// Render the shared [`SharedMenuBar`] at the very top of the Files panel
/// (MENUBAR-ALL): the UPPERCASE `FILES` title in the dock's System-group gold
/// accent, the File/Edit/View/Go/Share/Help menu spine, and the live status
/// cluster. The whole tree is built from a borrow-free [`crate::menubar`]
/// snapshot; an activated item dispatches through the existing [`Action`]
/// pipeline (§6 — the menu bar is a discoverable face over the surface's seams,
/// never a new behaviour path). The Help window's open flag lives in egui memory,
/// so the [`files_panel`] entry point stays stateless.
fn menu_bar(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let cx = crate::menubar::snapshot(b);
    let menus = crate::menubar::build_menus(&cx);
    let status = crate::menubar::build_status(&cx);
    let mut picked = None;
    egui::TopBottomPanel::top("files-menubar").show_inside(ui, |ui| {
        let model = MenuBarModel {
            title: "Files",
            accent: Style::ACCENT_SYSTEM,
            menus: &menus,
            status: &status,
        };
        picked = SharedMenuBar::show(ui, &model);
    });
    if let Some(p) = picked {
        match crate::menubar::to_action(p.clone(), &cx) {
            Some(action) => actions.push(action),
            // The two non-seam picks toggle a bar-owned window (Help / Destinations).
            None => {
                if p == crate::menubar::Picked::ShowDestinations {
                    crate::menubar::toggle_destinations(ui.ctx());
                } else {
                    crate::menubar::toggle_shortcuts(ui.ctx());
                }
            }
        }
    }
    crate::menubar::shortcuts_window(ui.ctx());
}

// ── TRANSFERS-8: the surface tab strip + the Transfers tab ─────────────────────

/// The top-level surface tab strip (Files ↔ Transfers, Q1) — a slim row of
/// segmented tabs just under the shared `MenuBar`. The Transfers tab carries a live
/// active-count badge (the same count the dock Files cell badges, Q1). Switching is
/// one [`Action::SwitchSurface`]; the `MenuBar` + sidebar + bottom strip stay put
/// (Q16 — the Transfers tab reuses the File Browser's chrome, not a new spine).
fn surface_tabs(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let current = b.surface_tab();
    let active = b.transfers_counts().active;
    egui::TopBottomPanel::top("files-surface-tabs").show_inside(ui, |ui| {
        scope_files_toolbar_ui(ui);
        ui.add_space(Style::TOOLBAR_INSET_Y);
        ui.horizontal(|ui| {
            for tab in SurfaceTab::ALL {
                let label = if tab == SurfaceTab::Transfers && active > 0 {
                    format!("{}  ({active})", tab.label())
                } else {
                    tab.label().to_string()
                };
                if ui.selectable_label(current == tab, label).clicked() {
                    actions.push(Action::SwitchSurface(tab));
                }
            }
        });
        ui.add_space(Style::TOOLBAR_INSET_Y);
    });
}

/// The Carbon [`Style`] token for a transfer state (§4 — no raw hex).
const fn transfer_state_color(state: TransferState) -> Color32 {
    match state {
        TransferState::Queued => Style::TEXT_DIM,
        TransferState::Running => Style::ACCENT,
        TransferState::Paused => Style::WARN,
        TransferState::Done => Style::OK,
        TransferState::Failed => Style::DANGER,
    }
}

/// The Transfers tab (Q1) — the worker's live ledger in newest-relevant order,
/// with an inline state filter, a New Transfer button, and honest empty states.
/// This is a pure renderer (§9): every control emits a typed verb the daemon owns;
/// the list reflects the ledger the worker publishes, never a fabricated row.
fn transfers_panel(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    ui.add_space(Style::SP_S);
    let counts = b.transfers_counts();
    let filter = b.transfers_filter();

    // Header: title + the New Transfer entry point (Q13).
    ui.horizontal(|ui| {
        ui.label(RichText::new("Transfers").color(Style::TEXT).strong());
        if counts.total > 0 {
            muted_note(ui, format!("\u{b7} {} total", counts.total));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn = egui::Button::new(RichText::new("New Transfer").color(Style::BG).strong())
                .fill(Style::ACCENT);
            if ui
                .add(btn)
                .files_hover_text("Queue a transfer (source \u{2192} destination)")
                .clicked()
            {
                actions.push(Action::OpenNewTransfer);
            }
        });
    });

    // Inline state filter (the MenuBar's View-by-state twin, for quick reach).
    ui.add_space(Style::SP_XS);
    ui.horizontal_wrapped(|ui| {
        for s in StateFilter::ALL {
            if ui.selectable_label(filter.state == s, s.label()).clicked() {
                actions.push(Action::SetTransferStateFilter(s));
            }
        }
    });
    if let Some(m) = filter.method {
        ui.horizontal(|ui| {
            muted_note(ui, format!("method: {}", m.label()));
            if ui.small_button("clear").clicked() {
                actions.push(Action::SetTransferMethodFilter(None));
            }
        });
    }
    ui.separator();

    let jobs = b.transfers_view();
    if jobs.is_empty() {
        transfers_empty_state(ui, b, counts.total);
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for job in &jobs {
                transfer_row(ui, job, actions);
                ui.add_space(Style::SP_XS);
            }
        });
}

/// The honest empty state (§7) — distinguishes "no worker on this node", "no
/// transfers yet", and "nothing matches this filter"; never a fake row.
fn transfers_empty_state(ui: &mut egui::Ui, b: &FileBrowser, total: usize) {
    ui.add_space(Style::SP_L);
    ui.vertical_centered(|ui| {
        if !b.transfers_worker_present() {
            ui.label(RichText::new("No transfers worker on this node yet.").color(Style::TEXT_DIM));
            muted_note(
                ui,
                "The daemon's transfers worker runs on Workstation-tier nodes; \
                 it publishes the ledger this tab reads.",
            );
        } else if total == 0 {
            ui.label(RichText::new("No transfers yet.").color(Style::TEXT_DIM));
            muted_note(
                ui,
                "Start one with New Transfer, drop a file onto a destination, \
                 or right-click a file and Send to \u{2192}.",
            );
        } else {
            ui.label(RichText::new("No transfers match this filter.").color(Style::TEXT_DIM));
            muted_note(ui, "Pick a different state above, or All.");
        }
    });
}

/// One ledger row: the method, the source → dest route, the live state chip, a
/// determinate progress bar ONLY when the lane reported a real percent (§7 — never
/// a fabricated bar), the honest failure reason, and the per-job lifecycle controls
/// gated by the worker's state machine.
fn transfer_row(ui: &mut egui::Ui, job: &TransferJob, actions: &mut Vec<Action>) {
    egui::Frame::NONE
        .fill(Style::LAYER_01)
        .corner_radius(Style::RADIUS)
        .inner_margin(Style::SP_S)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Method chip.
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(job.method.label()).size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                // Route (truncated by the row width; the full route is on hover).
                ui.label(RichText::new(job.route()).color(Style::TEXT))
                    .files_hover_text(job.route());
                // State chip + controls on the right.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    transfer_row_controls(ui, job, actions);
                    ui.colored_label(
                        transfer_state_color(job.state),
                        RichText::new(job.state.label()).size(Style::SMALL).strong(),
                    );
                });
            });
            // Progress: a determinate bar only when a real percent exists.
            if let Some(pct) = job.progress {
                ui.add(
                    egui::ProgressBar::new(f32::from(pct) / 100.0)
                        .desired_height(Style::SP_XS)
                        .fill(transfer_state_color(job.state)),
                );
            } else if job.state == TransferState::Running {
                // Running with no parsed percent yet — honest, not a fake 0%.
                muted_note(ui, "working\u{2026}");
            }
            // The honest failure reason (§7).
            if let Some(err) = &job.error {
                ui.colored_label(
                    Style::DANGER,
                    RichText::new(format!("! {err}")).size(Style::SMALL),
                );
            }
        });
}

/// The per-row lifecycle buttons (right-aligned): Pause / Resume gated by the
/// worker's state machine, Cancel always available (a cancel removes the row).
fn transfer_row_controls(ui: &mut egui::Ui, job: &TransferJob, actions: &mut Vec<Action>) {
    if files_action_button(
        ui,
        "Cancel",
        FilesActionTone::Danger,
        "Remove this transfer from the ledger",
    )
    .clicked()
    {
        actions.push(Action::TransferCancel(job.id.clone()));
    }
    if job.state.can_resume()
        && files_action_button(
            ui,
            "Resume",
            FilesActionTone::Primary,
            "Resume this transfer",
        )
        .clicked()
    {
        actions.push(Action::TransferResume(job.id.clone()));
    }
    if job.state.can_pause()
        && files_action_button(ui, "Pause", FilesActionTone::Quiet, "Pause this transfer").clicked()
    {
        actions.push(Action::TransferPause(job.id.clone()));
    }
}

/// The New Transfer dialog (Q13 entry 1) — source / dest / method + the Q12/Q15
/// policy knobs. Edits a clone of the model's [`NewTransferForm`] and hands it back
/// through one [`Action::NewTransferEdit`] (the surface's render → intents → apply
/// flow); Submit is disabled until both endpoints are non-blank (an honest guard,
/// §7). A modal over a dimmed shell.
fn new_transfer_dialog(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let Some(form) = b.new_transfer() else {
        return;
    };
    if modal_backdrop(ui.ctx(), "files-new-transfer-dim") {
        actions.push(Action::CancelNewTransfer);
    }
    let mut edited = form.clone();
    let before = edited.clone();
    egui::Window::new("New Transfer")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.set_max_width(Style::SP_XL * 14.0);
            egui::Grid::new("new-transfer-grid")
                .num_columns(2)
                .spacing([Style::SP_M, Style::SP_S])
                .show(ui, |ui| {
                    ui.label(RichText::new("Source").color(Style::TEXT_DIM));
                    ui.add(
                        egui::TextEdit::singleline(&mut edited.source)
                            .desired_width(Style::SP_XL * 9.0)
                            .hint_text("/path, https://…, host:/path, peer:<id>"),
                    );
                    ui.end_row();

                    ui.label(RichText::new("Destination").color(Style::TEXT_DIM));
                    ui.add(
                        egui::TextEdit::singleline(&mut edited.dest)
                            .desired_width(Style::SP_XL * 9.0)
                            .hint_text("/path, host:/path, peer:<id>, music:library"),
                    );
                    ui.end_row();

                    ui.label(RichText::new("Method").color(Style::TEXT_DIM));
                    egui::ComboBox::from_id_salt("new-transfer-method")
                        .selected_text(edited.method.label())
                        .show_ui(ui, |ui| {
                            for m in Method::MANUAL {
                                ui.selectable_value(&mut edited.method, m, m.label());
                            }
                        });
                    ui.end_row();

                    ui.label(RichText::new("Bandwidth cap").color(Style::TEXT_DIM));
                    ui.add(
                        egui::TextEdit::singleline(&mut edited.bwlimit)
                            .desired_width(Style::SP_XL * 4.0)
                            .hint_text("e.g. 2m (optional)"),
                    );
                    ui.end_row();

                    ui.label(RichText::new("Verify").color(Style::TEXT_DIM));
                    ui.checkbox(&mut edited.verify, "Checksum on completion");
                    ui.end_row();
                });

            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                let runnable = edited.runnable();
                let submit =
                    egui::Button::new(RichText::new("Queue transfer").color(Style::BG).strong())
                        .fill(Style::ACCENT);
                if ui
                    .add_enabled(runnable, submit)
                    .files_disabled_hover_text("Enter a source and a destination")
                    .clicked()
                {
                    actions.push(Action::SubmitNewTransfer);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CancelNewTransfer);
                }
            });
        });
    // Hand the edited form back once (only when it actually changed).
    if edited != before {
        actions.push(Action::NewTransferEdit(edited));
    }
}

/// The Carbon token for a destination kind (§4).
const fn target_tone(kind: TargetKind) -> Color32 {
    match kind {
        TargetKind::Peer => Style::ACCENT_MESH,
        TargetKind::Music => Style::ACCENT_MEDIA,
        TargetKind::MeshShare => Style::ACCENT,
    }
}

/// The Destinations manager window (Q16) — the auto-only registry (Q10): the two
/// standing node-state targets plus one per reachable peer, each a one-click New
/// Transfer entry point. Honest about the model: arbitrary hosts/URLs aren't pins,
/// they're typed per-job. Bar-owned open flag (egui memory), rendered here because
/// it needs the live roster.
fn destinations_window(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let ctx = ui.ctx();
    if !crate::menubar::destinations_open(ctx) {
        return;
    }
    let mut open = true;
    egui::Window::new("Destinations")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.set_max_width(Style::SP_XL * 12.0);
            muted_note(
                ui,
                "Auto-registered from mesh state. Arbitrary hosts / URLs are typed \
                 per-job in New Transfer.",
            );
            ui.add_space(Style::SP_S);
            for target in b.transfer_targets() {
                ui.horizontal(|ui| {
                    status_dot(ui, target_tone(target.kind));
                    ui.label(RichText::new(&target.label).color(Style::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("New transfer\u{2026}")
                            .files_hover_text("Open the New Transfer dialog pointed here")
                            .clicked()
                        {
                            actions.push(Action::OpenNewTransferTo(
                                target.dest.clone(),
                                target.method,
                            ));
                        }
                    });
                });
                muted_note(
                    ui,
                    format!("{} \u{b7} {}", target.method.label(), target.dest),
                );
                ui.add_space(Style::SP_XS);
            }
        });
    if !open {
        crate::menubar::set_destinations_open(ctx, false);
    }
}

// ── Top toolbar ───────────────────────────────────────────────────────────────

fn top_bar(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let active = b.active_pane_index();
    egui::TopBottomPanel::top("files-top").show_inside(ui, |ui| {
        scope_files_toolbar_ui(ui);
        ui.add_space(Style::TOOLBAR_INSET_Y);
        ui.horizontal(|ui| {
            // View-mode segmented control (the FILES title now lives in the shared
            // MENUBAR-ALL bar above — no duplicate heading here).
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
                .files_hover_text("Show hidden entries (Ctrl+H)")
                .clicked()
            {
                actions.push(Action::ToggleHidden(active));
            }
            let dirs_first = b.active_tab().sort().dirs_first;
            if ui
                .selectable_label(dirs_first, "Dirs first")
                .files_hover_text("Group directories ahead of files")
                .clicked()
            {
                actions.push(Action::ToggleDirsFirst(active));
            }
            if ui
                .selectable_label(b.is_dual(), "Dual pane")
                .files_hover_text("Show a second pane for cross-folder work")
                .clicked()
            {
                actions.push(Action::ToggleDual);
            }
            // FILEMGR-10 — the preview toggles.
            if ui
                .selectable_label(b.list_thumbs(), "Thumbs")
                .files_hover_text("Thumbnails in the List view (the Grid always thumbnails)")
                .clicked()
            {
                actions.push(Action::ToggleListThumbs);
            }
            if ui
                .selectable_label(b.preview_pane_open(), "Preview")
                .files_hover_text("Preview pane \u{2014} Space quick-looks the selection")
                .clicked()
            {
                actions.push(Action::TogglePreviewPane);
            }
            // FILEMGR-4 — the recursive-search bar toggle.
            if ui
                .selectable_label(b.search_form().open, "Search")
                .files_hover_text("Recursive search from here (name + content, with filters)")
                .clicked()
            {
                actions.push(Action::ToggleSearchBar);
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
        // FILEMGR-4 — the expandable recursive-search bar.
        if b.search_form().open {
            ui.separator();
            search_bar(ui, b, actions);
        }
        ui.add_space(Style::TOOLBAR_INSET_Y);
    });
}

// ── Recursive search bar (FILEMGR-4) ──────────────────────────────────────────

/// The recursive-search entry + filter controls, plus the live run status. The
/// results render as an ordinary listing in the active pane's tab (so selection
/// and every op apply to a hit directly) — this bar only drives the query and
/// shows progress. The view edits a clone of the model's [`SearchForm`] and hands
/// it back through one [`Action::SetSearchForm`], matching the surface's
/// render → intents → apply flow.
fn search_bar(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let active = b.active_pane_index();
    let mut form = b.search_form().clone();
    let before = form.clone();

    search_query_row(ui, b, active, &mut form, actions);
    search_filter_row(ui, &mut form);
    search_status_row(ui, b);

    // One intents hand-back for the whole form (only when something changed).
    if form != before {
        actions.push(Action::SetSearchForm(form));
    }
}

/// Row 1 — the name-glob + content-grep entries and the Search / Cancel / Clear
/// controls. Edits land on `form`; the run controls push their own actions.
fn search_query_row(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    active: usize,
    form: &mut SearchForm,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Name")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.name_glob)
                .desired_width(Style::SP_XL * 4.0)
                .hint_text("*.rs, report-*"),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("Contains")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.content)
                .desired_width(Style::SP_XL * 5.0)
                .hint_text("text in file"),
        );
        ui.selectable_value(&mut form.content_regex, true, "re")
            .files_hover_text("Interpret the contents text as a regular expression");
        ui.selectable_value(&mut form.content_regex, false, "abc")
            .files_hover_text("Interpret the contents text as a literal substring");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if b.search_active() {
                if ui
                    .button("Clear")
                    .files_hover_text("Leave search and show the folder again")
                    .clicked()
                {
                    actions.push(Action::ClearSearch(active));
                }
                if b.search_running()
                    && ui
                        .button(RichText::new("Cancel").color(Style::DANGER))
                        .files_hover_text("Stop the search (results so far stay)")
                        .clicked()
                {
                    actions.push(Action::CancelSearch);
                }
            }
            let runnable = form.to_query().is_some();
            let btn = egui::Button::new(RichText::new("Search").color(Style::BG).strong())
                .fill(Style::ACCENT);
            if ui
                .add_enabled(runnable, btn)
                .files_hover_text("Search this folder and everything under it")
                .clicked()
            {
                actions.push(Action::RunSearch(active));
            }
        });
    });
}

/// Row 2 — the type / extension / size / mtime filter controls (all edit `form`).
fn search_filter_row(ui: &mut egui::Ui, form: &mut SearchForm) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("files-search-kind")
            .selected_text(form.kind.label())
            .show_ui(ui, |ui| {
                for kind in TypeFilter::ALL {
                    ui.selectable_value(&mut form.kind, kind, kind.label());
                }
            });
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("ext")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.ext)
                .desired_width(Style::SP_XL * 1.6)
                .hint_text("pdf"),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("size KB")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.min_size_kb)
                .desired_width(Style::SP_XL * 1.6)
                .hint_text("min"),
        );
        ui.label(RichText::new("\u{2013}").color(Style::TEXT_DIM));
        ui.add(
            egui::TextEdit::singleline(&mut form.max_size_kb)
                .desired_width(Style::SP_XL * 1.6)
                .hint_text("max"),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("modified \u{2264}")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.within_days)
                .desired_width(Style::SP_XL * 1.6)
                .hint_text("days"),
        );
    });
}

/// Row 3 — live run status: the streaming hit count, a done / cancelled summary,
/// and the honest "remote mount is slower" note when the root is an sshfs mount.
fn search_status_row(ui: &mut egui::Ui, b: &FileBrowser) {
    let Some(p) = b.search_progress() else {
        return;
    };
    ui.horizontal(|ui| {
        if p.running {
            let verb = if p.cancelled { "Stopping" } else { "Searching" };
            ui.colored_label(
                Style::ACCENT,
                format!("{verb} {} \u{2014} {} found", p.root_label, p.matched),
            );
        } else if p.cancelled {
            ui.colored_label(
                Style::WARN,
                format!(
                    "Cancelled \u{2014} {} found (scanned {})",
                    p.matched, p.scanned
                ),
            );
        } else {
            ui.colored_label(
                Style::OK,
                format!("{} results (scanned {})", p.matched, p.scanned),
            );
        }
        if p.remote {
            muted_note(ui, "\u{b7} remote mount \u{2014} slower");
        }
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
                if local_place_row(ui, local_place_icon(spot.path), spot.label, here).clicked() {
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
            ui.add_space(Style::SP_M);

            // TRANSFERS-8 — the destination drop dock (Q13 entry 3). Drag a file
            // selection onto a target to queue a transfer; a click opens the New
            // Transfer dialog pointed here. Visible on both surface tabs (the sidebar
            // is shared), so a drag from the Files tab reaches a target either way.
            destinations_section(ui, b, actions);
        });
}

/// The sidebar's "SEND TO" section — the auto destination registry (Q10) as live
/// **drop targets** (Q13 entry 3) plus a click-to-open-dialog convenience. A file
/// drag released on a row queues a transfer of the source pane's selection; a plain
/// click opens the New Transfer dialog pre-pointed at the target.
fn destinations_section(ui: &mut egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    section_header(ui, "SEND TO");
    muted_note(ui, "Drop files here, or click to set up a transfer.");
    for target in b.transfer_targets() {
        let resp = ui
            .selectable_label(false, format!("\u{2192} {}", target.label))
            .files_hover_text(format!("{} \u{b7} {}", target.method.label(), target.dest));
        if resp.clicked() {
            actions.push(Action::OpenNewTransferTo(
                target.dest.clone(),
                target.method,
            ));
        }
        // A live drop target: highlight on hover, queue on release.
        if resp.dnd_hover_payload::<FilesDrag>().is_some() {
            ui.painter().rect_stroke(
                resp.rect,
                Style::RADIUS,
                Stroke::new(1.0, target_tone(target.kind)),
                StrokeKind::Inside,
            );
        }
        if let Some(payload) = resp.dnd_release_payload::<FilesDrag>() {
            actions.push(Action::DropOnTarget(payload.source_pane, target.clone()));
        }
    }
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
                .files_hover_text("Mount this peer over the mesh and browse it")
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
            .files_disabled_hover_text("Peer is offline \u{2014} can't be mounted");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if reachable {
                let is_dest = b.destination() == Some(peer.id.as_str());
                if ui
                    .selectable_label(is_dest, "dest")
                    .files_hover_text("Set as the Send-To destination")
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
                    .files_hover_text(if full {
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
                        .files_hover_text("Unmount this peer")
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
        resp.files_hover_text(reason);
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
    // accent rule so it's clear which pane the toolbar + keyboard act on. Both
    // panes reserve the rule's strip so their listings stay top-aligned, and the
    // 2px accent rule cross-fades between panes on the shared BASE tier (a focus
    // shift is a state change, not a snap — CRAFT S4) rather than jumping the
    // solid line from one column to the other.
    if b.is_dual() {
        let focused = b.active_pane_index() == pane_ix;
        let t = Motion::animate(
            ui.ctx(),
            ("files-pane-focus", pane_ix),
            focused,
            Motion::BASE,
        );
        if t > 0.0 {
            let top = ui.max_rect();
            ui.painter().hline(
                top.x_range(),
                top.top(),
                Stroke::new(2.0, Style::ACCENT.gamma_multiply(t)),
            );
        }
        ui.add_space(Style::SP_XS);
    }
    nav_row(ui, b, pane_ix, actions);
    tab_strip(ui, b, pane_ix, actions);
    ui.separator();
    listing(ui, b, pane_ix, actions);
}

fn nav_row(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize, actions: &mut Vec<Action>) {
    let tab = b.pane(pane_ix).active_tab();
    ui.scope(|ui| {
        scope_files_toolbar_ui(ui);
        ui.horizontal(|ui| {
            let (back_icon, back_tip) = FILES_NAV_ICONS[0];
            if files_nav_button(ui, back_icon, tab.can_back(), back_tip).clicked() {
                actions.push(Action::Back(pane_ix));
            }
            let (forward_icon, forward_tip) = FILES_NAV_ICONS[1];
            if files_nav_button(ui, forward_icon, tab.can_forward(), forward_tip).clicked() {
                actions.push(Action::Forward(pane_ix));
            }
            let can_up = tab.location().parent().is_some();
            let (up_icon, up_tip) = FILES_NAV_ICONS[2];
            if files_nav_button(ui, up_icon, can_up, up_tip).clicked() {
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
    });
}

fn tab_strip(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize, actions: &mut Vec<Action>) {
    let pane = b.pane(pane_ix);
    ui.scope(|ui| {
        scope_files_toolbar_ui(ui);
        ui.horizontal(|ui| {
            let closeable = pane.tabs().len() > 1;
            let (close_icon, close_tip) = FILES_TAB_ICONS[0];
            for (i, tab) in pane.tabs().iter().enumerate() {
                let active = i == pane.active_tab_index();
                if ui.selectable_label(active, tab.title()).clicked() {
                    actions.push(Action::SelectTab(pane_ix, i));
                }
                if closeable && files_icon_button(ui, close_icon, true, close_tip).clicked() {
                    actions.push(Action::CloseTab(pane_ix, i));
                }
            }
            let (new_tab_icon, new_tab_tip) = FILES_TAB_ICONS[1];
            if files_icon_button(ui, new_tab_icon, true, new_tab_tip).clicked() {
                actions.push(Action::NewTab(pane_ix));
            }
        });
    });
}

// ── The listing (List / Grid / Details) + selection + DnD + rubber-band ───────

fn listing(ui: &mut egui::Ui, b: &FileBrowser, pane_ix: usize, actions: &mut Vec<Action>) {
    // Keyboard shortcuts act on the focused pane — but never while a text field
    // (the path box, a dialog's arming/octal entry) owns the keyboard, and never
    // while an operation dialog is up (its own buttons/keys drive it).
    let dialog_open =
        b.pending_conflict().is_some() || b.pending_delete().is_some() || b.properties().is_some();
    if b.active_pane_index() == pane_ix && !ui.ctx().wants_keyboard_input() && !dialog_open {
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
            // FILEMGR-11 — Delete opens the permanent-delete confirm.
            if i.consume_key(Modifiers::NONE, Key::Delete) {
                actions.push(Action::RequestDelete(pane_ix));
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
                    ViewMode::Details => {
                        details_view(ui, b, pane_ix, &entries, &mut rects, actions);
                    }
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
        paint_entry_bg(ui, resp.id, rect, e.selected, resp.hovered());
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
            let icon_rect = Rect::from_center_size(
                egui::pos2(tag_x + FILES_ROW_ICON * 0.5, cy),
                egui::vec2(FILES_ROW_ICON, FILES_ROW_ICON),
            );
            if !paint_files_icon(
                ui,
                file_type_icon(e.mime),
                icon_rect,
                FILES_ROW_ICON,
                file_icon_tint(e),
            ) {
                ui.painter().text(
                    egui::pos2(tag_x, cy),
                    Align2::LEFT_CENTER,
                    mime_tag(e.mime),
                    FontId::monospace(Style::SMALL),
                    Style::TEXT_DIM,
                );
            }
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
        entry_interactions(ui, b, pane_ix, e, &resp, actions);
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
            paint_entry_bg(ui, resp.id, rect, e.selected, resp.hovered());
            // FILEMGR-10 — a decoded thumbnail fills the tile art area; until
            // (or unless) one exists, the honest type tag stays the icon.
            want_thumb(ui, e, rect, actions);
            let art = Rect::from_min_max(
                egui::pos2(rect.left() + Style::SP_XS, rect.top() + Style::SP_XS),
                egui::pos2(rect.right() - Style::SP_XS, rect.bottom() - Style::SP_L),
            );
            if !draw_thumb(ui, b, e, art) {
                let icon_rect = Rect::from_center_size(
                    egui::pos2(rect.center().x, rect.top() + Style::SP_L),
                    egui::vec2(FILES_GRID_ICON, FILES_GRID_ICON),
                );
                if !paint_files_icon(
                    ui,
                    file_type_icon(e.mime),
                    icon_rect,
                    FILES_GRID_ICON,
                    file_icon_tint(e),
                ) {
                    ui.painter().text(
                        egui::pos2(rect.center().x, rect.top() + Style::SP_L),
                        Align2::CENTER_CENTER,
                        mime_tag(e.mime),
                        FontId::monospace(Style::HEADING),
                        file_icon_tint(e),
                    );
                }
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
            entry_interactions(ui, b, pane_ix, e, &resp, actions);
        }
    });
}

fn details_view(
    ui: &mut egui::Ui,
    b: &FileBrowser,
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
        paint_entry_bg(ui, resp.id, rect, e.selected, resp.hovered());
        let cols = detail_columns(rect);
        let cy = rect.center().y;
        let icon_rect = Rect::from_center_size(
            egui::pos2(cols.name.left() + FILES_ROW_ICON * 0.5, cy),
            egui::vec2(FILES_ROW_ICON, FILES_ROW_ICON),
        );
        let icon_width = if paint_files_icon(
            ui,
            file_type_icon(e.mime),
            icon_rect,
            FILES_ROW_ICON,
            file_icon_tint(e),
        ) {
            Style::SP_L
        } else {
            0.0
        };
        // Name (clipped to its column).
        ui.painter().with_clip_rect(cols.name).text(
            egui::pos2(cols.name.left() + icon_width, cy),
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
        entry_interactions(ui, b, pane_ix, e, &resp, actions);
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
    b: &FileBrowser,
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
    // FILEMGR-11 — a right-click selects the row (if it wasn't) and opens the
    // op menu: Properties + permanent Delete drive the real dialogs.
    if resp.secondary_clicked() {
        actions.push(Action::Focus(pane_ix));
        if !e.selected {
            actions.push(Action::Click(pane_ix, e.idx));
        }
    }
    files_context_menu(resp, |ui| {
        // FILEMGR-12 — Send-To a peer (reuse the mesh transfer) + Send-in-Chat
        // (reuse the mesh transfer + the NOTIFY-CHAT file message-kind). Both are
        // roster-driven submenus; offline peers are honestly greyed (no probe, no
        // hang — the cached roster is read).
        files_submenu_button(ui, "Send to", |ui| {
            peer_send_submenu(ui, b, pane_ix, false, actions);
        });
        files_submenu_button(ui, "Send in Chat", |ui| {
            peer_send_submenu(ui, b, pane_ix, true, actions);
        });
        // TRANSFERS-8 — right-click "Transfer to →" (Q13 entry 2): queue a real
        // transfer of the selection to an auto destination (peer / Music / mesh-
        // share). Distinct from FILEMGR-7 "Send to" — this rides the daemon-owned
        // transfers queue (survives a shell restart, tracked in the ledger).
        target_transfer_submenu(ui, b, pane_ix, actions);
        // EDITOR-9 — open a file in the Editor surface (files only; a directory has
        // no document to open). Posts `action/editor/open`, drained by the shell.
        if !e.is_dir && ui.button("Open in Editor").clicked() {
            actions.push(Action::SendToEditor(pane_ix));
            ui.close_menu();
        }
        ui.separator();
        // FILEMGR-12 — cut/copy/paste over the shared shell clipboard.
        if ui.button("Cut").clicked() {
            actions.push(Action::ClipCut(pane_ix));
            ui.close_menu();
        }
        if ui.button("Copy").clicked() {
            actions.push(Action::ClipCopy(pane_ix));
            ui.close_menu();
        }
        if ui
            .add_enabled(b.can_paste(), egui::Button::new("Paste"))
            .clicked()
        {
            actions.push(Action::ClipPaste(pane_ix));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Properties").clicked() {
            actions.push(Action::OpenProperties(pane_ix));
            ui.close_menu();
        }
        ui.separator();
        let del = egui::Button::new(RichText::new("Delete\u{2026}").color(Style::DANGER));
        if ui.add(del).clicked() {
            actions.push(Action::RequestDelete(pane_ix));
            ui.close_menu();
        }
    });
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

/// FILEMGR-12 — the roster submenu behind "Send to" / "Send in Chat": one row per
/// mesh peer. A reachable peer is a live button routing to the peer; an offline
/// peer is honestly greyed + inert (no probe, so the menu never hangs). `in_chat`
/// picks the chat hand-off (transfer + a NOTIFY-CHAT file card) over the plain
/// mesh transfer. Reads the cached roster only — never a blocking peer call.
fn peer_send_submenu(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    pane_ix: usize,
    in_chat: bool,
    actions: &mut Vec<Action>,
) {
    if b.peers().is_empty() {
        muted_note(ui, "No peers connected.");
        return;
    }
    for peer in b.peers() {
        if peer.status.is_reachable() {
            if ui.button(peer.host.as_str()).clicked() {
                actions.push(if in_chat {
                    Action::SendInChat(pane_ix, peer.id.clone())
                } else {
                    Action::SendToPeer(pane_ix, peer.id.clone())
                });
                ui.close_menu();
            }
        } else {
            ui.add_enabled(
                false,
                egui::Button::new(RichText::new(peer.host.as_str()).color(Style::TEXT_DIM))
                    .frame(false),
            )
            .files_disabled_hover_text("Peer is offline \u{2014} can't receive a file");
        }
    }
}

/// TRANSFERS-8 — the "Transfer to →" submenu (Q13 entry 2): one row per auto
/// destination (Q10 — the two standing node-state targets + one per reachable
/// peer). Clicking queues a real transfer of the pane's selection to that target
/// through the daemon-owned queue. Reads the cached roster only — never a blocking
/// probe.
fn target_transfer_submenu(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    pane_ix: usize,
    actions: &mut Vec<Action>,
) {
    files_submenu_button(ui, "Transfer to", |ui| {
        for target in b.transfer_targets() {
            if ui
                .button(target.label.as_str())
                .files_hover_text(format!("{} \u{b7} {}", target.method.label(), target.dest))
                .clicked()
            {
                actions.push(Action::SendToTarget(pane_ix, target.clone()));
                ui.close_menu();
            }
        }
    });
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

/// Convert the shared [`Elevation`] ladder's shadow token into egui's
/// `epaint::Shadow`, reusing the token's translucent umbra verbatim — depth
/// stays single-sourced in `mde_egui::style`, no local colour is minted (§4).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // token px values are small +ve.
fn elevation_shadow(elevation: Elevation) -> egui::Shadow {
    let token = elevation.shadow();
    egui::Shadow {
        offset: [token.offset[0] as i8, token.offset[1] as i8],
        blur: token.blur as u8,
        spread: token.spread as u8,
        color: token.umbra,
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
            // The card is the deepest tier of the shared elevation ladder — a
            // soft Modal shadow painted behind the fill lifts it off the dim
            // backdrop (translucent umbra, lock #2; no layout change).
            ui.painter()
                .add(elevation_shadow(Elevation::Modal).as_shape(card, Style::RADIUS));
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

// ── The operation dialogs (FILEMGR-11) ────────────────────────────────────────

/// A dim, full-screen backdrop beneath a modal dialog: it darkens the shell and
/// swallows a click (returned so the caller maps it to its own dismiss). Drawn at
/// `Order::Middle` so the dialog `Window` (shown right after) floats on top.
fn modal_backdrop(ctx: &egui::Context, id: &str) -> bool {
    let screen = ctx.screen_rect();
    let mut clicked = false;
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let hit = ui.interact(screen, egui::Id::new(format!("{id}-hit")), Sense::click());
            ui.painter()
                .rect_filled(screen, 0.0, Style::BG.gamma_multiply(0.82));
            clicked = hit.clicked();
        });
    clicked
}

/// The interactive conflict dialog (FILEMGR-11 / lock 4): the worker is parked on
/// this collision until the user picks Overwrite / Skip / Keep-both. The
/// apply-to-all checkbox rides in the [`Resolution`] answer so the FILEMGR-2
/// engine stops asking for the rest of the op. The backdrop is inert here — the
/// only way out is an answer (or cancelling the op on the strip).
fn conflict_dialog(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let Some((op_id, conflict)) = b.pending_conflict() else {
        return;
    };
    let dst_name = conflict.dst.file_name().map_or_else(
        || conflict.dst.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let both_dirs = conflict.src_is_dir && conflict.dst_is_dir;
    let kind = if conflict.dst_is_dir {
        "folder"
    } else {
        "file"
    };
    let _ = modal_backdrop(ui.ctx(), "files-conflict-dim");

    let all_id = egui::Id::new("files-conflict-apply-all");
    let mut apply_all = ui.data(|d| d.get_temp::<bool>(all_id).unwrap_or(false));
    egui::Window::new("Name collision")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.set_max_width(Style::SP_XL * 12.0);
            ui.colored_label(
                Style::TEXT,
                format!("A {kind} named \u{201c}{dst_name}\u{201d} already exists here."),
            );
            muted_note(
                ui,
                if both_dirs {
                    "Overwrite merges the folders \u{2014} existing files are kept unless a child collides."
                } else {
                    "Overwrite replaces it. Keep both copies in alongside an auto-renamed name."
                },
            );
            ui.add_space(Style::SP_S);
            ui.checkbox(&mut apply_all, "Apply to every remaining collision");
            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                let keep = egui::Button::new(RichText::new("Keep both").color(Style::BG).strong())
                    .fill(Style::ACCENT);
                if ui.add(keep).clicked() {
                    push_resolution(actions, op_id, Resolution::KeepBoth, apply_all);
                }
                if ui.button("Skip").clicked() {
                    push_resolution(actions, op_id, Resolution::Skip, apply_all);
                }
                let overwrite =
                    egui::Button::new(RichText::new("Overwrite").color(Style::BG).strong())
                        .fill(Style::DANGER);
                if ui.add(overwrite).clicked() {
                    push_resolution(actions, op_id, Resolution::Overwrite, apply_all);
                }
            });
        });
    ui.data_mut(|d| d.insert_temp(all_id, apply_all));
}

/// Push a conflict answer and clear the apply-to-all memory so the next op's first
/// collision starts unchecked.
fn push_resolution(
    actions: &mut Vec<Action>,
    op_id: u64,
    resolution: Resolution,
    apply_to_all: bool,
) {
    actions.push(Action::ResolveConflict {
        op_id,
        resolution,
        apply_to_all,
    });
}

/// The permanent-delete confirm (FILEMGR-11 / lock 3/6): names the items, spells
/// out that the delete is final (no trash, no undo), and — when the target is on a
/// remote / escalated mesh mount — layers typed-arming on top (lock 19): the
/// Delete button stays disabled until the node name is typed.
fn delete_dialog(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let Some(cd) = b.pending_delete() else {
        return;
    };
    if modal_backdrop(ui.ctx(), "files-delete-dim") {
        actions.push(Action::CancelDelete);
    }
    let count = cd.count();
    let noun = if count == 1 { "item" } else { "items" };
    egui::Window::new("Permanent delete")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.set_max_width(Style::SP_XL * 13.0);
            ui.colored_label(
                Style::TEXT,
                RichText::new(format!("Permanently delete {count} {noun}?")).strong(),
            );
            ui.colored_label(
                Style::DANGER,
                "This can\u{2019}t be undone \u{2014} there is no trash and no restore.",
            );
            ui.add_space(Style::SP_XS);
            for name in cd.names.iter().take(8) {
                muted_note(ui, format!("\u{2022} {name}"));
            }
            if cd.names.len() > 8 {
                muted_note(ui, format!("\u{2026} and {} more", cd.names.len() - 8));
            }

            // Lock 19 — typed-arming for a remote / escalated target.
            if let Some(arming) = &cd.arming {
                ui.add_space(Style::SP_S);
                ui.separator();
                let scope = if arming.full_fs {
                    " (full-filesystem mount)"
                } else {
                    ""
                };
                ui.colored_label(
                    Style::WARN,
                    format!(
                        "This deletes on the remote node \u{201c}{}\u{201d}{scope}.",
                        arming.node
                    ),
                );
                field(ui, "Target", &arming.path, Style::TEXT_DIM);
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(format!(
                        "Type the node name \u{201c}{}\u{201d} to arm:",
                        arming.node
                    ))
                    .color(Style::TEXT)
                    .size(Style::SMALL),
                );
                let mut echo = cd.echo.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut echo)
                        .hint_text(arming.node.as_str())
                        .desired_width(Style::SP_XL * 6.0),
                );
                if resp.changed() {
                    actions.push(Action::DeleteEcho(echo));
                }
            }

            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                let armed = cd.armed();
                let del = egui::Button::new(
                    RichText::new("Delete permanently")
                        .color(Style::BG)
                        .strong(),
                )
                .fill(Style::DANGER);
                if ui
                    .add_enabled(armed, del)
                    .files_disabled_hover_text("Type the node name to arm this delete")
                    .clicked()
                {
                    actions.push(Action::ConfirmDelete);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CancelDelete);
                }
            });
        });
}

/// The Properties rwx grid + octal field (FILEMGR-11 / lock 8), always in
/// lock-step (a grid toggle rewrites the octal; a valid octal moves the grid).
fn properties_perms(
    ui: &mut egui::Ui,
    d: &crate::dialogs::PropertiesDialog,
    actions: &mut Vec<Action>,
) {
    section_header(ui, "PERMISSIONS");
    egui::Grid::new("files-perm-grid")
        .num_columns(4)
        .spacing(egui::vec2(Style::SP_M, Style::SP_XS))
        .show(ui, |ui| {
            ui.label("");
            for perm in Perm::ALL {
                ui.label(
                    RichText::new(perm.glyph())
                        .color(Style::TEXT_DIM)
                        .monospace(),
                );
            }
            ui.end_row();
            for class in PermClass::ALL {
                ui.label(RichText::new(class.label()).color(Style::TEXT));
                for perm in Perm::ALL {
                    let mut on = d.perms.get(class, perm);
                    if ui.checkbox(&mut on, "").changed() {
                        actions.push(Action::PropToggle(class, perm));
                    }
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Octal")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        let mut octal = d.octal_edit.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut octal)
                .desired_width(Style::SP_XL * 2.0)
                .font(FontId::monospace(Style::BODY)),
        );
        if resp.changed() {
            actions.push(Action::PropSetOctal(octal));
        }
        if d.octal_is_valid() {
            muted_note(ui, d.perms.symbolic());
        } else {
            ui.colored_label(Style::DANGER, "not a valid octal mode");
        }
    });
}

/// The Properties owner/group row (FILEMGR-11 / lock 8): editable ids only when
/// this caller may chown, honestly read-only otherwise.
fn properties_owner(
    ui: &mut egui::Ui,
    d: &crate::dialogs::PropertiesDialog,
    actions: &mut Vec<Action>,
) {
    section_header(ui, "OWNER");
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("User")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        if d.chown_permitted {
            let mut uid = d.uid_edit.clone();
            if ui
                .add(egui::TextEdit::singleline(&mut uid).desired_width(Style::SP_XL * 2.0))
                .changed()
            {
                actions.push(Action::PropSetUid(uid));
            }
        } else {
            ui.colored_label(Style::TEXT, d.uid.to_string());
        }
        ui.add_space(Style::SP_M);
        ui.label(
            RichText::new("Group")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        if d.chown_permitted {
            let mut gid = d.gid_edit.clone();
            if ui
                .add(egui::TextEdit::singleline(&mut gid).desired_width(Style::SP_XL * 2.0))
                .changed()
            {
                actions.push(Action::PropSetGid(gid));
            }
        } else {
            ui.colored_label(Style::TEXT, d.gid.to_string());
        }
    });
    if !d.chown_permitted {
        muted_note(
            ui,
            "Changing the owner needs root / CAP_CHOWN \u{2014} not available here.",
        );
    }
}

/// The Properties / permissions dialog (FILEMGR-11 / lock 8): a live rwx grid in
/// lock-step with the octal field, the owner/group ids, and — only when this
/// caller may chown — editable owner fields. Apply drives a real chmod / chown
/// through the [`FileOps`](mde_files::fileops::FileOps) seam.
fn properties_dialog(ui: &egui::Ui, b: &FileBrowser, actions: &mut Vec<Action>) {
    let Some(d) = b.properties() else {
        return;
    };
    let pane = b.active_pane_index();
    if modal_backdrop(ui.ctx(), "files-props-dim") {
        actions.push(Action::PropClose);
    }
    egui::Window::new("Properties")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.set_max_width(Style::SP_XL * 13.0);
            ui.label(
                RichText::new(&d.name)
                    .color(Style::TEXT)
                    .strong()
                    .size(Style::BODY),
            );
            muted_note(
                ui,
                format!(
                    "{} \u{b7} {} bytes",
                    if d.is_dir { "Folder" } else { "File" },
                    d.size
                ),
            );
            field(ui, "Path", &d.path.display().to_string(), Style::TEXT_DIM);

            ui.add_space(Style::SP_S);
            properties_perms(ui, d, actions);

            ui.add_space(Style::SP_S);
            properties_owner(ui, d, actions);

            if let Some(outcome) = &d.outcome {
                ui.add_space(Style::SP_XS);
                match outcome {
                    Ok(()) => ui.colored_label(Style::OK, "Applied."),
                    Err(reason) => ui.colored_label(Style::DANGER, reason),
                };
            }

            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                let apply = egui::Button::new(RichText::new("Apply").color(Style::BG).strong())
                    .fill(Style::ACCENT);
                if ui.add_enabled(d.can_apply(), apply).clicked() {
                    actions.push(Action::PropApply(pane));
                }
                if ui.button("Close").clicked() {
                    actions.push(Action::PropClose);
                }
            });
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
                    if files_action_button(
                        ui,
                        "Dismiss",
                        FilesActionTone::Quiet,
                        "Dismiss this file operation",
                    )
                    .clicked()
                    {
                        actions.push(Action::DismissOp(op.op_id));
                    }
                } else {
                    if files_action_button(
                        ui,
                        "Cancel",
                        FilesActionTone::Danger,
                        "Cancel this file operation",
                    )
                    .clicked()
                    {
                        actions.push(Action::CancelOp(op.op_id));
                    }
                    if op.control.is_paused() {
                        if files_action_button(
                            ui,
                            "Resume",
                            FilesActionTone::Primary,
                            "Resume this file operation",
                        )
                        .clicked()
                        {
                            actions.push(Action::ResumeOp(op.op_id));
                        }
                    } else if files_action_button(
                        ui,
                        "Pause",
                        FilesActionTone::Quiet,
                        "Pause this file operation",
                    )
                    .clicked()
                    {
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

fn paint_entry_bg(ui: &egui::Ui, id: egui::Id, rect: Rect, selected: bool, hovered: bool) {
    // Selection is a persistent state — its accent wash reads immediately so a
    // selected row is unmistakable the instant selection lands (and instant
    // through arrow-key navigation, where a lagging wash would trail the cursor).
    if selected {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::ACCENT.gamma_multiply(0.30));
        return;
    }
    // Hover is the transient affordance, so it cross-fades on the shared FAST
    // micro-interaction tier (CRAFT §4/§6.1 — a hover must never snap; scanning a
    // long listing should read as calm, not a strobe). `Motion::animate` schedules
    // its own repaints only while the fade is live, so an idle listing stays quiet.
    let t = Motion::animate(ui.ctx(), id, hovered, Motion::FAST);
    if t > 0.0 {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI.gamma_multiply(t));
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
    use super::{
        apply_files_popup_style, file_type_icon, files_panel, files_tooltip, local_place_icon,
        scope_files_popup_ui, scope_files_toolbar_ui, tab_strip, ACTION_BUTTON_H,
        FILES_ICON_BUTTON_ICON, FILES_NAV_ICONS, FILES_TAB_ICONS,
    };
    use crate::model::{FileBrowser, Location, SurfaceTab, ViewMode, LOCAL_SPOTS};
    use crate::transfers::test_support::FakeTransfers;
    use crate::transfers::{Method, TransferJob, TransferPolicy, TransferState};
    use mde_egui::egui::{self, pos2, vec2, Color32, Rect, Stroke};
    use mde_egui::{Density, Style, StyleColorScheme};
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

    fn render_frame(browser: &mut FileBrowser) -> egui::FullOutput {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 700.0))),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                files_panel(ui, browser);
            });
        })
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

    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Color32)> {
        fn text_color(text: &egui::epaint::TextShape) -> Color32 {
            if let Some(color) = text.override_text_color {
                return color;
            }
            text.galley
                .job
                .sections
                .iter()
                .find_map(|section| {
                    (section.format.color != Color32::PLACEHOLDER).then_some(section.format.color)
                })
                .unwrap_or(text.fallback_color)
        }

        fn walk(shape: &egui::Shape, out: &mut Vec<(String, Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text_color(text)));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn rect_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<Color32> {
        fn walk(shape: &egui::Shape, out: &mut Vec<Color32>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill != Color32::TRANSPARENT => {
                    out.push(rect.fill);
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn rect_strokes(shapes: &[egui::epaint::ClippedShape]) -> Vec<Stroke> {
        fn walk(shape: &egui::Shape, out: &mut Vec<Stroke>) {
            match shape {
                egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => {
                    out.push(rect.stroke);
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn image_mesh_count(shapes: &[egui::epaint::ClippedShape]) -> usize {
        fn walk(shape: &egui::Shape) -> usize {
            match shape {
                egui::Shape::Mesh(mesh) if !mesh.vertices.is_empty() => 1,
                egui::Shape::Vec(shapes) => shapes.iter().map(walk).sum(),
                _ => 0,
            }
        }

        shapes.iter().map(|clipped| walk(&clipped.shape)).sum()
    }

    fn assert_painted_text_color(texts: &[(String, Color32)], label: &str, color: Color32) {
        assert!(
            texts
                .iter()
                .any(|(text, painted)| text == label && *painted == color),
            "expected {label:?} to paint with {color:?}, saw {texts:?}"
        );
    }

    #[test]
    fn files_context_menu_visuals_use_themed_text_and_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut style = (*ctx.style()).clone();
        apply_files_popup_style(&ctx, &mut style);

        assert_eq!(style.visuals.window_fill, Style::SURFACE);
        assert_eq!(style.visuals.panel_fill, Style::SURFACE);
        assert_eq!(style.visuals.window_stroke.color, Style::BORDER);
        assert_eq!(style.visuals.override_text_color, Some(Style::TEXT));
        assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, Style::TEXT);
        assert_eq!(style.visuals.widgets.hovered.bg_fill, Style::SURFACE_HI);
        assert_eq!(style.visuals.widgets.hovered.fg_stroke.color, Style::TEXT);
        assert_eq!(style.visuals.widgets.open.bg_fill, Style::SURFACE_HI);
        assert_eq!(style.spacing.button_padding.y, Style::CONTROL_PAD_Y);

        let light = egui::Context::default();
        Style::install_color_scheme_with_density(&light, StyleColorScheme::Light, Density::Mouse);
        let palette = Style::palette_for(StyleColorScheme::Light);
        let mut light_style = (*light.style()).clone();
        apply_files_popup_style(&light, &mut light_style);

        assert_eq!(light_style.visuals.window_fill, palette.surface);
        assert_eq!(light_style.visuals.panel_fill, palette.surface);
        assert_eq!(light_style.visuals.window_stroke.color, palette.border);
        assert_eq!(light_style.visuals.override_text_color, Some(palette.text));
        assert_eq!(
            light_style.visuals.widgets.inactive.fg_stroke.color,
            palette.text
        );
        assert_eq!(
            light_style.visuals.widgets.hovered.bg_fill,
            palette.surface_hi
        );
        assert_eq!(
            light_style.visuals.widgets.hovered.fg_stroke.color,
            palette.text
        );
        assert_ne!(
            light_style.visuals.widgets.inactive.fg_stroke.color,
            Style::TEXT,
            "Files context menus must not leak dark-mode text into light-mode popups"
        );
    }

    #[test]
    fn files_nested_popup_scope_repairs_raw_menu_visuals() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, StyleColorScheme::Light, Density::Mouse);
        let palette = Style::palette_for(StyleColorScheme::Light);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 120.0))),
            ..Default::default()
        };
        let mut scoped_style = None;

        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    ui.style_mut().visuals.override_text_color = None;
                    ui.style_mut().visuals.widgets.inactive.fg_stroke.color = Style::TEXT;
                    ui.style_mut().visuals.widgets.hovered.bg_fill = Color32::TRANSPARENT;
                    ui.style_mut().visuals.widgets.hovered.fg_stroke.color = Color32::BLACK;

                    scope_files_popup_ui(ui);
                    scoped_style = Some((*ui.style()).clone());
                });
        });

        let scoped_style = scoped_style.expect("test frame should capture scoped popup style");
        assert_eq!(scoped_style.visuals.window_fill, palette.surface);
        assert_eq!(
            scoped_style.visuals.override_text_color,
            Some(palette.text),
            "nested Files popups must restore readable light-mode text"
        );
        assert_eq!(
            scoped_style.visuals.widgets.inactive.fg_stroke.color,
            palette.text
        );
        assert_eq!(
            scoped_style.visuals.widgets.hovered.bg_fill,
            palette.surface_hi
        );
        assert_eq!(
            scoped_style.visuals.widgets.hovered.fg_stroke.color,
            palette.text
        );
    }

    #[test]
    fn files_hover_tooltip_uses_themed_text_and_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 120.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    files_tooltip(ui, "Show hidden entries");
                });
        });

        let texts = painted_text(&out.shapes);
        assert_painted_text_color(&texts, "Show hidden entries", Style::TEXT);
        assert!(
            !texts
                .iter()
                .any(|(text, color)| text == "Show hidden entries" && *color == Color32::BLACK),
            "Files tooltip leaked raw black popup text: {texts:?}"
        );

        let fills = rect_fills(&out.shapes);
        assert!(
            fills.contains(&Style::SURFACE),
            "Files tooltip should paint its own themed surface: {fills:?}"
        );
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
    fn mounts_and_renders_the_transfers_tab_with_ledger_fixtures() {
        // A running (with progress) + a failed (with a reason) job — the tab must
        // render both, the state chips, and the progress bar, headless without panic.
        let mut running = TransferJob::new(
            "/src/a",
            "peer:oak",
            Method::Rsync,
            TransferPolicy::default(),
        );
        running.state = TransferState::Running;
        running.progress = Some(42);
        let mut failed = TransferJob::new(
            "https://x/y.iso",
            "/downloads",
            Method::Http,
            TransferPolicy::default(),
        );
        failed.state = TransferState::Failed;
        failed.error = Some("host unreachable".into());
        let fake = FakeTransfers::new().with_jobs(vec![running, failed]);
        let mut b =
            FileBrowser::with_file_ops(Box::new(RenderFixture::populated()), FakeFileOps::new())
                .with_transfers(Box::new(fake));
        b.set_surface_tab(SurfaceTab::Transfers);
        assert_eq!(b.surface_tab(), SurfaceTab::Transfers);
        assert_eq!(b.transfers_view().len(), 2, "both ledger fixtures show");
        mount(&mut b);
    }

    #[test]
    fn transfer_lifecycle_controls_use_files_action_button_tokens() {
        let mut running = TransferJob::new(
            "/src/a",
            "peer:oak",
            Method::Rsync,
            TransferPolicy::default(),
        );
        running.state = TransferState::Running;
        running.progress = Some(42);
        let fake = FakeTransfers::new().with_jobs(vec![running]);
        let mut b =
            FileBrowser::with_file_ops(Box::new(RenderFixture::populated()), FakeFileOps::new())
                .with_transfers(Box::new(fake));
        b.set_surface_tab(SurfaceTab::Transfers);

        let out = render_frame(&mut b);
        let texts = painted_text(&out.shapes);
        assert_painted_text_color(&texts, "Cancel", Style::DANGER);
        assert_painted_text_color(&texts, "Pause", Style::TEXT);

        let fills = rect_fills(&out.shapes);
        assert!(
            fills.iter().any(|fill| *fill == Style::LAYER_02),
            "Files transfer lifecycle controls must paint the shared action layer fill: {fills:?}"
        );
        let strokes = rect_strokes(&out.shapes);
        assert!(
            strokes
                .iter()
                .any(|stroke| stroke.color == Style::DANGER.gamma_multiply(0.72)),
            "Files transfer cancel action must paint a danger outline: {strokes:?}"
        );
        assert!(
            strokes
                .iter()
                .any(|stroke| stroke.color == Style::TEXT.gamma_multiply(0.72)),
            "Files transfer pause action must paint a quiet outline: {strokes:?}"
        );
    }

    #[test]
    fn files_action_buttons_use_refined_shared_chrome_height() {
        assert_eq!(
            ACTION_BUTTON_H,
            Style::TOOLBAR_CONTROL_H,
            "Files action buttons should follow the shared refined toolbar control height"
        );
        assert!(
            ACTION_BUTTON_H < Style::SP_L,
            "Files should not carry the old full-gutter 24pt local action height"
        );
        assert!(
            FILES_ICON_BUTTON_ICON < ACTION_BUTTON_H,
            "navigation icons should fit within the refined action button rect"
        );
    }

    #[test]
    fn files_toolbar_scope_uses_refined_control_metrics() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(240.0, 80.0))),
            ..Default::default()
        };
        let mut metrics = None;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                scope_files_toolbar_ui(ui);
                metrics = Some((
                    ui.style().spacing.interact_size.y,
                    ui.style().spacing.button_padding.y,
                    ui.style().spacing.item_spacing.y,
                ));
            });
        });
        let (interact_h, pad_y, gap_y) = metrics.expect("test should capture Files toolbar style");
        assert_eq!(interact_h, Style::TOOLBAR_CONTROL_H);
        assert_eq!(pad_y, Style::CONTROL_PAD_Y);
        assert_eq!(gap_y, Style::TOOLBAR_INSET_Y);
    }

    #[test]
    fn files_navigation_toolbar_uses_yamis_icons() {
        assert_eq!(FILES_NAV_ICONS.len(), 3);
        for (icon, label) in FILES_NAV_ICONS {
            assert!(
                icon.name().starts_with("yamis-"),
                "{label} navigation icon should resolve through the YAMIS catalog"
            );
            let img = mde_theme::brand::icons::icon_image(*icon, 16, [0xe0, 0xe0, 0xe0, 0xff])
                .unwrap_or_else(|err| panic!("{label} navigation icon failed to rasterize: {err}"));
            assert!(
                img.rgba.chunks_exact(4).any(|px| px[3] > 0),
                "{label} navigation icon rasterized empty"
            );
        }
    }

    #[test]
    fn files_tab_strip_controls_use_yamis_icon_buttons() {
        assert_eq!(
            FILES_TAB_ICONS,
            &[
                (mde_theme::brand::icons::IconId::Close, "Close tab"),
                (mde_theme::brand::icons::IconId::NewTab, "New tab"),
            ]
        );
        for (icon, label) in FILES_TAB_ICONS {
            assert!(
                icon.name().starts_with("yamis-"),
                "{label} tab-strip icon should resolve through the YAMIS catalog"
            );
            let img = mde_theme::brand::icons::icon_image(*icon, 16, [0xe0, 0xe0, 0xe0, 0xff])
                .unwrap_or_else(|err| panic!("{label} tab-strip icon failed to rasterize: {err}"));
            assert!(
                img.rgba.chunks_exact(4).any(|px| px[3] > 0),
                "{label} tab-strip icon rasterized empty"
            );
        }

        let mut b = browser();
        b.new_tab(0);
        assert_eq!(b.pane(0).tabs().len(), 2, "test setup needs close buttons");
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 80.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut actions = Vec::new();
                tab_strip(ui, &b, 0, &mut actions);
                assert!(
                    actions.is_empty(),
                    "render-only tab strip should not emit actions"
                );
            });
        });

        assert!(
            image_mesh_count(&out.shapes) >= 3,
            "two close controls plus the new-tab control should paint image icons"
        );
        let texts = painted_text(&out.shapes);
        assert!(
            texts
                .iter()
                .all(|(text, _)| text != "\u{00d7}" && text != "+"),
            "tab-strip icon buttons must not paint raw text controls: {texts:?}"
        );
    }

    #[test]
    fn files_mime_glyphs_use_yamis_icons() {
        for (mime, label) in [
            (Mime::Folder, "folder"),
            (Mime::Doc, "document"),
            (Mime::Image, "image"),
            (Mime::Pdf, "pdf"),
            (Mime::Archive, "archive"),
            (Mime::Disk, "disk"),
        ] {
            let icon = file_type_icon(mime);
            assert!(
                icon.name().starts_with("yamis-"),
                "{label} MIME glyph should resolve through the YAMIS catalog"
            );
            let img = mde_theme::brand::icons::icon_image(icon, 16, [0xe0, 0xe0, 0xe0, 0xff])
                .unwrap_or_else(|err| panic!("{label} MIME glyph failed to rasterize: {err}"));
            assert!(
                img.rgba.chunks_exact(4).any(|px| px[3] > 0),
                "{label} MIME glyph rasterized empty"
            );
        }
    }

    #[test]
    fn files_local_places_use_yamis_icons() {
        for spot in LOCAL_SPOTS {
            let icon = local_place_icon(spot.path);
            assert!(
                icon.name().starts_with("yamis-"),
                "{} place glyph should resolve through the YAMIS catalog",
                spot.label
            );
            let img = mde_theme::brand::icons::icon_image(icon, 16, [0xe0, 0xe0, 0xe0, 0xff])
                .unwrap_or_else(|err| {
                    panic!("{} place glyph failed to rasterize: {err}", spot.label)
                });
            assert!(
                img.rgba.chunks_exact(4).any(|px| px[3] > 0),
                "{} place glyph rasterized empty",
                spot.label
            );
        }
    }

    #[test]
    fn mounts_and_renders_the_transfers_empty_state_when_worker_absent() {
        // An absent worker is an honest EmptyState (§7), not a fabricated row.
        let fake = FakeTransfers::new().present(false);
        let mut b =
            FileBrowser::with_file_ops(Box::new(RenderFixture::populated()), FakeFileOps::new())
                .with_transfers(Box::new(fake));
        b.set_surface_tab(SurfaceTab::Transfers);
        assert!(!b.transfers_worker_present());
        assert!(b.transfers_view().is_empty());
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_the_new_transfer_dialog() {
        let mut b = browser();
        b.open_new_transfer();
        assert!(b.new_transfer().is_some());
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
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // token px values are small +ve.
    fn quick_look_card_casts_the_shared_modal_shadow() {
        use mde_egui::style::Elevation;

        // The conversion reuses the Modal token verbatim — the depth ladder
        // stays single-sourced in mde_egui::style, no local colour is minted.
        let token = Elevation::Modal.shadow();
        let shadow = super::elevation_shadow(Elevation::Modal);
        assert_eq!(
            shadow.offset,
            [token.offset[0] as i8, token.offset[1] as i8],
            "the shadow offset comes from the Modal token"
        );
        assert_eq!(
            shadow.blur, token.blur as u8,
            "the shadow blur comes from the Modal token"
        );
        assert_eq!(
            shadow.spread, token.spread as u8,
            "the shadow spread comes from the Modal token"
        );
        assert_eq!(
            shadow.color, token.umbra,
            "the shadow umbra is the token's, not a minted colour"
        );
        assert!(
            shadow.color.a() > 0 && shadow.color.a() < 255,
            "the depth is a translucent umbra (lock #2), never an opaque fill"
        );

        // And an open quick-look really paints it: a blurred rect wearing the
        // Modal umbra sits in the frame's shape list behind the card fill.
        fn holds_shadow(shape: &egui::Shape, shadow: egui::epaint::Shadow) -> bool {
            match shape {
                egui::Shape::Rect(r) => {
                    r.fill == shadow.color
                        && (r.blur_width - f32::from(shadow.blur)).abs() < f32::EPSILON
                }
                egui::Shape::Vec(v) => v.iter().any(|s| holds_shadow(s, shadow)),
                _ => false,
            }
        }
        let mut b = browser();
        b.click(0, 1);
        b.toggle_quick_look();
        assert!(b.quick_look_open());
        let ctx = egui::Context::default();
        Style::install(&ctx);
        // Kill egui's Area fade-in so frame 2 paints the umbra at full
        // strength (the fade would otherwise scale its alpha mid-animation).
        ctx.style_mut(|s| s.animation_time = 0.0);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 700.0))),
            ..Default::default()
        };
        // Frame 1 is the overlay Area's sizing pass (egui hides a new Area's
        // first frame); frame 2 paints it for real.
        let _sizing = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::files_panel(ui, &mut b);
            });
        });
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::files_panel(ui, &mut b);
            });
        });
        assert!(
            out.shapes.iter().any(|c| holds_shadow(&c.shape, shadow)),
            "the quick-look card must cast the shared Modal elevation shadow"
        );
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

    // ── the operation dialogs (FILEMGR-11) ───────────────────────────────────

    #[test]
    fn mounts_and_renders_the_local_delete_confirm() {
        let mut b = browser();
        // Sorted display: [alpha/, photo.png, report.pdf] → select a couple.
        b.click(0, 1);
        b.request_delete(0);
        assert!(b.pending_delete().is_some(), "the confirm opened");
        assert!(
            b.pending_delete().and_then(|c| c.arming.as_ref()).is_none(),
            "a local delete has no typed-arming"
        );
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_the_delete_confirm_with_typed_arming() {
        use crate::mesh_mount::test_support::FakeMeshMount;
        use crate::mesh_mount::{MountPhase, MountScope, MountView};
        // A row on peer `oak`'s escalated full-fs mount → the confirm demands the
        // node be typed. The render exercises the arming field + disabled Delete.
        let remote = "/run/user/1000/mde-mesh/oak/report.txt";
        let fixture = RenderFixture {
            peers: Vec::new(),
            rows: vec![FileRow::local("report.txt", Mime::Doc, "1 KB", "now").with_path(remote)],
        };
        let fake = FakeMeshMount::new().with_view(
            "oak",
            MountView {
                phase: MountPhase::Mounted,
                scope: Some(MountScope::Full),
                path: Some("/run/user/1000/mde-mesh/oak".into()),
                reason: None,
            },
        );
        let mut b = FileBrowser::with_file_ops(Box::new(fixture), FakeFileOps::new())
            .with_mesh_mount(Box::new(fake));
        b.click(0, 0);
        b.request_delete(0);
        assert!(
            b.pending_delete().and_then(|c| c.arming.as_ref()).is_some(),
            "a remote delete arms"
        );
        mount(&mut b);
    }

    #[test]
    fn mounts_and_renders_the_conflict_dialog() {
        use std::path::Path;
        use std::time::{Duration, Instant};
        // A real fake-FS collision so an op parks on the conflict prompt, which
        // the dialog then renders.
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/d")).expect("mkdir");
        fs.create_dir(Path::new("/dst")).expect("mkdir");
        fs.seed_file("/d/f.txt", b"new").expect("seed");
        fs.seed_file("/dst/f.txt", b"old").expect("seed collision");
        let fixture = RenderFixture {
            peers: Vec::new(),
            rows: vec![FileRow::local("f.txt", Mime::Doc, "1 KB", "now").with_path("/d/f.txt")],
        };
        let mut b = FileBrowser::with_file_ops(Box::new(fixture), fs);
        b.click(0, 0);
        let id = b
            .drop_transfer(0, PathBuf::from("/dst"), true)
            .expect("queued");
        let deadline = Instant::now() + Duration::from_secs(5);
        while b.pending_conflict().is_none() {
            b.pump_ops();
            assert!(Instant::now() < deadline, "collision never surfaced");
            std::thread::sleep(Duration::from_millis(5));
        }
        mount(&mut b); // renders the conflict dialog over the shell
                       // Answer so the worker unparks cleanly (Drop would also fail-safe it).
        b.resolve_conflict(id, mde_files::opqueue::Resolution::Skip, true);
    }

    #[test]
    fn mounts_and_renders_the_properties_dialog() {
        use std::path::Path;
        // The Properties dialog reads through the injected meta-ops seam, so a
        // seeded fake FS backs it (no real disk touch), and chown is permitted so
        // the owner fields render editable.
        let meta = FakeFileOps::privileged();
        meta.create_dir(Path::new("/d")).expect("mkdir");
        meta.seed_file("/d/report.txt", b"hello").expect("seed");
        let fixture = RenderFixture {
            peers: Vec::new(),
            rows: vec![
                FileRow::local("report.txt", Mime::Doc, "5 B", "now").with_path("/d/report.txt")
            ],
        };
        let mut b = FileBrowser::with_file_ops(Box::new(fixture), FakeFileOps::new())
            .with_meta_ops(meta, true);
        b.click(0, 0);
        b.open_properties(0);
        assert!(b.properties().is_some(), "the dialog opened");
        mount(&mut b);
    }
}

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

use std::collections::BTreeSet;
use std::path::PathBuf;

use mde_egui::egui::{
    self, Align2, FontId, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
};
use mde_egui::{muted_note, status_dot, Style};

use mde_files::model::{Mime, PeerStatus};
use mde_files::opqueue::Progress;

use crate::model::{FileBrowser, Location, SendOutcome, SortKey, SortSpec, ViewMode, LOCAL_SPOTS};

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

    let mut actions: Vec<Action> = Vec::new();
    top_bar(ui, browser, &mut actions);
    egui::TopBottomPanel::bottom("files-bottom").show_inside(ui, |ui| {
        op_strip(ui, browser, &mut actions);
        status_line(ui, browser);
    });
    sidebar(ui, browser, &mut actions);
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
        Action::OpenRow(p, i) => browser.open_row(p, i),
        Action::SetView(p, m) => browser.set_view(p, m),
        Action::SortBy(p, k) => browser.sort_by(p, k),
        Action::ToggleHidden(p) => browser.toggle_hidden(p),
        Action::ToggleDirsFirst(p) => browser.toggle_dirs_first(p),
        Action::ToggleDual => browser.toggle_dual(),
        Action::Refresh => {
            browser.refresh_roster();
            browser.reload_all();
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

fn peer_row(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    peer: &mde_files::model::Peer,
    active: usize,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        status_dot(ui, peer_color(peer.status));
        let browsing = matches!(b.active_tab().location(), Location::Peer(id) if id.as_str() == peer.id.as_str());
        if ui
            .selectable_label(browsing, peer.host.as_str())
            .on_hover_text("Browse this peer's shared folder")
            .clicked()
        {
            actions.push(Action::Navigate(active, Location::Peer(peer.id.clone())));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if peer.status.is_reachable() {
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
    // Keyboard shortcuts act on the focused pane.
    if b.active_pane_index() == pane_ix {
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::A) {
                actions.push(Action::SelectAll(pane_ix));
            }
            if i.consume_key(Modifiers::COMMAND, Key::H) {
                actions.push(Action::ToggleHidden(pane_ix));
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                actions.push(Action::ClearSelection(pane_ix));
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
                    ViewMode::List => list_view(ui, pane_ix, &entries, &mut rects, actions),
                    ViewMode::Grid => grid_view(ui, pane_ix, &entries, &mut rects, actions),
                    ViewMode::Details => details_view(ui, pane_ix, &entries, &mut rects, actions),
                }
            }
        });

    rubber_band(ui, pane_ix, &bg, &rects, actions);
    pane_drop(ui, b, pane_ix, &bg, actions);
}

fn list_view(
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
        let cy = rect.center().y;
        let tag_x = rect.left() + Style::SP_S;
        let name_x = tag_x + Style::SP_L;
        let painter = ui.painter();
        painter.text(
            egui::pos2(tag_x, cy),
            Align2::LEFT_CENTER,
            mime_tag(e.mime),
            FontId::monospace(Style::SMALL),
            Style::TEXT_DIM,
        );
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
            let painter = ui.painter();
            painter.text(
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
}

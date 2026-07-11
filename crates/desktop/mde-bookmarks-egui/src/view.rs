//! The egui rendering of the Bookmarks surface (BOOKMARKS-4).
//!
//! Every widget reads the render-agnostic [`Manager`] and draws through the
//! shared [`Style`] — no raw colours or spacing (governance §4). The view never
//! reaches around the model to mutate the tree: a frame collects the user's
//! intents as [`Action`]s while it renders, then applies them once, at the end
//! (render → intents → apply). Text fields (search, add, rename) bind straight to
//! the model's draft buffers, which hold no egui — so the decision logic stays
//! testable without a display.
//!
//! Layout — the locked three regions plus the addenda's left vertical tab rail:
//! a top header (search · add · sort), a narrow **tab rail**, the **folder tree**,
//! the **detail/browser pane** on the right, and the **list** in the centre. The
//! detail pane carries an honest browser seam: the interactive Servo browser is
//! BOOKMARKS-5/6, so "open in a tab" is surfaced as a labelled intent, never a
//! fake browser (§7).

use mde_egui::egui::{
    self, Align, Align2, CursorIcon, Layout, Response, RichText, ScrollArea, Sense, TextEdit,
};
use mde_egui::{Motion, Style};

use mde_bookmarks::{Bookmark, Folder, Item, Source};

use crate::model::{ActionOutcome, LinkCheckRecord, LinkHealth, Manager, SortBy};

/// The drag-and-drop payload for this surface — a wrapper so egui routes drops by
/// this surface's own type (never colliding with another panel's `Uuid` payload).
#[derive(Clone, Copy)]
struct DragItem(uuid::Uuid);

/// A user intent captured during a render, applied after the frame.
enum Action {
    /// Browse a folder's contents (`None` = root).
    OpenFolder(Option<uuid::Uuid>),
    /// Expand/collapse a tree folder.
    ToggleExpanded(uuid::Uuid),
    /// Plain-click select just this item.
    SelectOnly(uuid::Uuid),
    /// Ctrl-click toggle.
    SelectToggle(uuid::Uuid),
    /// Shift-click range select.
    SelectRange(uuid::Uuid),
    /// Double-click open (folder → navigate; bookmark → browser-tab intent).
    Open(uuid::Uuid),
    /// Open the add-bookmark form.
    OpenAdd,
    /// Open the add form pre-filled from a pasted URL.
    OpenAddWithUrl(String),
    /// Submit the add form.
    CommitAdd,
    /// Dismiss the add form.
    CancelAdd,
    /// Create a folder under the given parent.
    AddFolder(Option<uuid::Uuid>),
    /// Begin an inline rename.
    BeginRename(uuid::Uuid),
    /// Commit the inline rename.
    CommitRename,
    /// Cancel the inline rename.
    CancelRename,
    /// Request a delete (confirms first on a non-empty folder).
    RequestDelete(uuid::Uuid),
    /// Confirm the parked non-empty-folder delete.
    ConfirmDeleteYes,
    /// Dismiss the parked delete.
    ConfirmDeleteNo,
    /// Choose the list sort order.
    SetSort(SortBy),
    /// Clear the live search.
    ClearSearch,
    /// Bulk: open every selected bookmark (browser-tab intent).
    OpenSelection,
    /// Bulk: copy the selected URLs to the clipboard.
    CopyUrls,
    /// Ask the daemon to run a fresh bounded dead-link check.
    CheckLinks,
    /// Copy a single bookmark's URL (the detail pane's "Copy URL").
    CopyOneUrl(uuid::Uuid),
    /// Bulk: move the selection into a folder.
    BulkMove(Option<uuid::Uuid>),
    /// Bulk: delete the selection.
    BulkDelete,
    /// Drag: move a batch into a folder (`None` = root / current).
    MoveInto {
        /// The dragged item ids.
        ids: Vec<uuid::Uuid>,
        /// The destination folder.
        folder: Option<uuid::Uuid>,
    },
    /// Drag: reorder a batch to before a target row.
    MoveBefore {
        /// The dragged item ids.
        ids: Vec<uuid::Uuid>,
        /// The row the batch lands before.
        target: uuid::Uuid,
    },
}

/// Render the whole Bookmarks surface into `ui` — the one reusable entry point.
///
/// (E12-3, EMBED.) The standalone binary calls it inside its window
/// [`egui::CentralPanel`]; the E12 shell mounts the SAME fn as a panel. The
/// internal regions use `show_inside`, so the surface lays out within whatever
/// `ui` it is handed.
pub fn bookmarks_panel(ui: &mut egui::Ui, m: &mut Manager) {
    let mut actions: Vec<Action> = Vec::new();

    handle_keys(ui, m, &mut actions);
    header(ui, m, &mut actions);
    rail(ui, &mut actions);
    tree(ui, m, &mut actions);
    detail_pane(ui, m, &mut actions);
    list(ui, m, &mut actions);
    confirm_dialog(ui, m, &mut actions);

    let ctx = ui.ctx().clone();
    for action in actions {
        apply(&ctx, m, action);
    }
}

/// Global keyboard + paste handling: Ctrl+N opens the add form (lock Q26 —
/// shortcut path); a paste of a URL-shaped string opens the add form pre-filled
/// (lock Q26 — paste path).
fn handle_keys(ui: &egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    let (shortcut, pasted) = ui.input(|i| {
        let shortcut = i.modifiers.command && i.key_pressed(egui::Key::N);
        let pasted = i.events.iter().find_map(|e| match e {
            egui::Event::Paste(text) => Some(text.clone()),
            _ => None,
        });
        (shortcut, pasted)
    });
    if shortcut && !m.add_open() {
        actions.push(Action::OpenAdd);
    }
    // A bare paste (nothing focused) is the quick-add-a-URL path (lock Q26). When
    // a text field HAS focus, let the paste land there — never hijack it.
    let has_focus = ui.memory(egui::Memory::focused).is_some();
    if let Some(text) = pasted {
        let looks_like_url = text.contains("://") || text.contains('.');
        if looks_like_url && !has_focus && !m.add_open() {
            actions.push(Action::OpenAddWithUrl(text.trim().to_string()));
        }
    }
}

// ── Header ───────────────────────────────────────────────────────────────────

fn header(ui: &mut egui::Ui, m: &mut Manager, actions: &mut Vec<Action>) {
    egui::TopBottomPanel::top("bm-header").show_inside(ui, |ui| {
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("Bookmarks")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
            ui.add_space(Style::SP_M);
            ui.colored_label(Style::TEXT_DIM, location_line(m));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                sort_selector(ui, m, actions);
                ui.add_space(Style::SP_S);
                search_field(ui, m);
            });
        });
        ui.add_space(Style::SP_XS);
        toolbar(ui, m, actions);
        if m.add_open() {
            ui.add_space(Style::SP_XS);
            add_form(ui, m, actions);
        }
        ui.add_space(Style::SP_XS);
        status_line(ui, m);
        ui.add_space(Style::SP_XS);
    });
}

fn toolbar(ui: &mut egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        if ui.button("+ Add bookmark").clicked() {
            actions.push(Action::OpenAdd);
        }
        if ui.button("New folder").clicked() {
            actions.push(Action::AddFolder(m.current()));
        }
        if ui.button("Check links").clicked() {
            actions.push(Action::CheckLinks);
        }
        if m.is_searching() {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::TEXT_DIM, format!("Search: {}", m.query()));
            if ui.button("Clear").clicked() {
                actions.push(Action::ClearSearch);
            }
        }
    });
}

fn search_field(ui: &mut egui::Ui, m: &mut Manager) {
    ui.add(
        TextEdit::singleline(m.query_mut())
            .hint_text("Search title or URL")
            .desired_width(Style::SP_XL * 6.0),
    );
}

fn sort_selector(ui: &mut egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    egui::ComboBox::from_id_salt("bm-sort")
        .selected_text(m.sort().label())
        .show_ui(ui, |ui| {
            for option in SortBy::ALL {
                if ui
                    .selectable_label(m.sort() == option, option.label())
                    .clicked()
                {
                    actions.push(Action::SetSort(option));
                }
            }
        });
}

fn add_form(ui: &mut egui::Ui, m: &mut Manager, actions: &mut Vec<Action>) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("URL")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            let url = ui.add(
                TextEdit::singleline(m.add_url_mut())
                    .hint_text("https://…")
                    .desired_width(Style::SP_XL * 7.0),
            );
            ui.label(
                RichText::new("Title")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.add(
                TextEdit::singleline(m.add_title_mut())
                    .hint_text("optional — from the URL when blank")
                    .desired_width(Style::SP_XL * 6.0),
            );
            let submit_on_enter = url.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let add = egui::Button::new(RichText::new("Add").color(Style::BG).strong())
                .fill(Style::ACCENT);
            let clicked = ui.add_enabled(m.can_submit_add(), add).clicked();
            if clicked || (submit_on_enter && m.can_submit_add()) {
                actions.push(Action::CommitAdd);
            }
            if ui.button("Cancel").clicked() {
                actions.push(Action::CancelAdd);
            }
        });
    });
}

fn status_line(ui: &mut egui::Ui, m: &Manager) {
    match m.last_action() {
        ActionOutcome::Idle => {
            mde_egui::muted_note(
                ui,
                "Add a bookmark with +, paste a URL, or drag to organise. Ctrl+N adds.",
            );
        }
        ActionOutcome::Note(note) => {
            ui.colored_label(Style::TEXT_DIM, note);
        }
        ActionOutcome::Done(done) => {
            ui.colored_label(Style::OK, done);
        }
    }
    if let Some(status) = m.latest_link_check() {
        let truncated = if status.truncated {
            " · truncated"
        } else {
            ""
        };
        ui.colored_label(
            Style::TEXT_DIM,
            format!(
                "Link check: {} checked · {} alive · {} dead · {} unsupported · {} error(s){}",
                status.checked,
                status.alive,
                status.dead,
                status.unsupported,
                status.errors,
                truncated
            ),
        );
    }
}

/// The header's location line: the current folder path, or the live-search state.
fn location_line(m: &Manager) -> String {
    if m.is_searching() {
        let hits = m.listing().len();
        return format!("Search · {hits} result(s)");
    }
    let crumbs: Vec<String> = m.breadcrumb().into_iter().map(|f| f.name).collect();
    if crumbs.is_empty() {
        "All bookmarks".to_string()
    } else {
        format!("All bookmarks / {}", crumbs.join(" / "))
    }
}

// ── Left vertical tab rail (enterprise addenda) ──────────────────────────────

fn rail(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    egui::SidePanel::left("bm-rail")
        .resizable(false)
        .exact_width(Style::SP_XL * 3.0)
        .show_inside(ui, |ui| {
            ui.add_space(Style::SP_S);
            ui.vertical_centered_justified(|ui| {
                if ui.button("+ New").clicked() {
                    actions.push(Action::OpenAdd);
                }
                ui.add_space(Style::SP_S);
                // The one live tab today: the manager itself. Opened pages will
                // stack below it as tabs when the browser (BOOKMARKS-5) lands.
                let _ = ui.selectable_label(true, "Manager");
            });
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                ui.add_space(Style::SP_S);
                mde_egui::muted_note(ui, "Browser tabs stack here (BOOKMARKS-5).");
            });
        });
}

// ── Folder tree (region 1) ───────────────────────────────────────────────────

fn tree(ui: &mut egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    egui::SidePanel::left("bm-tree")
        .default_width(Style::SP_XL * 6.0)
        .show_inside(ui, |ui| {
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("FOLDERS")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    root_row(ui, m, actions);
                    for folder in m.child_folders(None) {
                        tree_folder(ui, m, &folder, 0, actions);
                    }
                });
        });
}

/// The "All bookmarks" root row — selectable + a drop target for a move to root.
fn root_row(ui: &mut egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    let selected = m.current().is_none() && !m.is_searching();
    let resp = ui.selectable_label(selected, "All bookmarks");
    if resp.clicked() {
        actions.push(Action::OpenFolder(None));
    }
    if let Some(payload) = resp.dnd_release_payload::<DragItem>() {
        actions.push(Action::MoveInto {
            ids: m.drag_batch(payload.0),
            folder: None,
        });
    }
}

/// One folder row + (when expanded) its children — recursive. Each row is a drop
/// target (drag a bookmark/folder onto it to move it in, lock Q29).
fn tree_folder(
    ui: &mut egui::Ui,
    m: &Manager,
    folder: &Folder,
    depth: usize,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        // Indent one grid step per depth level (no int→float cast).
        for _ in 0..depth {
            ui.add_space(Style::SP_M);
        }
        let has_kids = m.has_child_folders(folder.id);
        let expanded = m.is_expanded(folder.id);
        if has_kids {
            let arrow = if expanded { "v" } else { ">" };
            if ui
                .add(egui::Button::new(RichText::new(arrow).monospace()).frame(false))
                .clicked()
            {
                actions.push(Action::ToggleExpanded(folder.id));
            }
        } else {
            ui.add_space(Style::SP_M);
        }
        mde_egui::status_dot(ui, Style::WARN);
        let selected = m.current() == Some(folder.id);
        let resp = ui.selectable_label(selected, folder.name.as_str());
        if resp.clicked() {
            actions.push(Action::OpenFolder(Some(folder.id)));
        }
        if resp.dnd_hover_payload::<DragItem>().is_some() {
            // A live drop target: outline the destination folder so the move is clear.
            ui.painter().rect_stroke(
                resp.rect,
                Style::RADIUS,
                egui::Stroke::new(1.0, Style::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(payload) = resp.dnd_release_payload::<DragItem>() {
            actions.push(Action::MoveInto {
                ids: m.drag_batch(payload.0),
                folder: Some(folder.id),
            });
        }
    });
    if m.is_expanded(folder.id) {
        for child in m.child_folders(Some(folder.id)) {
            tree_folder(ui, m, &child, depth + 1, actions);
        }
    }
}

// ── Detail / browser pane (region 3) ─────────────────────────────────────────

fn detail_pane(ui: &mut egui::Ui, m: &mut Manager, actions: &mut Vec<Action>) {
    egui::SidePanel::right("bm-detail")
        .default_width(Style::SP_XL * 8.0)
        .show_inside(ui, |ui| {
            ui.add_space(Style::SP_S);
            ui.label(RichText::new("Details").color(Style::TEXT).strong());
            ui.add_space(Style::SP_XS);
            ui.separator();
            ui.add_space(Style::SP_S);
            match m.detail() {
                None => {
                    mde_egui::muted_note(ui, "Select a bookmark or folder to see its details.");
                }
                Some(Item::Folder(folder)) => folder_detail(ui, m, &folder, actions),
                Some(Item::Bookmark(bookmark)) => bookmark_detail(ui, m, &bookmark, actions),
            }
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                browser_seam(ui, m);
            });
        });
}

fn folder_detail(ui: &mut egui::Ui, m: &mut Manager, folder: &Folder, actions: &mut Vec<Action>) {
    if m.rename_target() == Some(folder.id) {
        rename_row(ui, m, actions);
    } else {
        ui.label(
            RichText::new(&folder.name)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
    }
    mde_egui::field(
        ui,
        "Items inside",
        &m.descendant_count(folder.id).to_string(),
        Style::TEXT,
    );
    mde_egui::field(
        ui,
        "Last edited by",
        &author_line(&folder.last_author),
        Style::TEXT_DIM,
    );
    ui.add_space(Style::SP_S);
    detail_actions(ui, m, folder.id, actions);
}

fn bookmark_detail(
    ui: &mut egui::Ui,
    m: &mut Manager,
    bookmark: &Bookmark,
    actions: &mut Vec<Action>,
) {
    if m.rename_target() == Some(bookmark.id) {
        rename_row(ui, m, actions);
    } else {
        let title = if bookmark.title.is_empty() {
            bookmark.url.as_str()
        } else {
            bookmark.title.as_str()
        };
        ui.label(
            RichText::new(title)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
    }
    ui.add_space(Style::SP_XS);
    // The URL is data — render it monospace (mono-first, lock #3) so it reads as
    // an address, not prose.
    ui.label(
        RichText::new(&bookmark.url)
            .monospace()
            .color(Style::ACCENT)
            .size(Style::SMALL),
    );
    ui.add_space(Style::SP_XS);
    mde_egui::field(
        ui,
        "Source",
        source_label(&bookmark.source),
        Style::TEXT_DIM,
    );
    mde_egui::field(
        ui,
        "Last edited by",
        &author_line(&bookmark.last_author),
        Style::TEXT_DIM,
    );
    if let Some(record) = m.link_check_for(bookmark.id) {
        link_check_detail(ui, record);
    } else if m.latest_link_check().is_some() {
        mde_egui::field(
            ui,
            "Link health",
            "not checked in latest pass",
            Style::TEXT_DIM,
        );
    }
    if !bookmark.tags.is_empty() {
        mde_egui::field(ui, "Tags", &bookmark.tags.join(", "), Style::TEXT_DIM);
    }
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        if ui.button("Open").clicked() {
            actions.push(Action::Open(bookmark.id));
        }
        if ui.button("Copy URL").clicked() {
            actions.push(Action::CopyOneUrl(bookmark.id));
        }
    });
    ui.add_space(Style::SP_S);
    detail_actions(ui, m, bookmark.id, actions);
}

fn link_check_detail(ui: &mut egui::Ui, record: &LinkCheckRecord) {
    let color = match record.health {
        LinkHealth::Alive => Style::OK,
        LinkHealth::Dead | LinkHealth::Error => Style::DANGER,
        LinkHealth::Unsupported => Style::TEXT_DIM,
    };
    let status = record.http_status.map_or_else(
        || record.health.label().to_string(),
        |code| format!("{} · HTTP {code}", record.health.label()),
    );
    mde_egui::field(ui, "Link health", &status, color);
    if !record.detail.is_empty() {
        mde_egui::field(ui, "Probe detail", &record.detail, Style::TEXT_DIM);
    }
}

/// The Rename / Delete row shared by folder + bookmark detail.
fn detail_actions(ui: &mut egui::Ui, m: &Manager, id: uuid::Uuid, actions: &mut Vec<Action>) {
    if m.rename_target() == Some(id) {
        return; // the rename row already shows Save/Cancel
    }
    ui.horizontal(|ui| {
        if ui.button("Rename").clicked() {
            actions.push(Action::BeginRename(id));
        }
        let del = egui::Button::new(RichText::new("Delete").color(Style::DANGER));
        if ui.add(del).clicked() {
            actions.push(Action::RequestDelete(id));
        }
    });
}

fn rename_row(ui: &mut egui::Ui, m: &mut Manager, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let field =
            ui.add(TextEdit::singleline(m.rename_buf_mut()).desired_width(Style::SP_XL * 5.0));
        let commit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.button("Save").clicked() || commit {
            actions.push(Action::CommitRename);
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::CancelRename);
        }
    });
}

/// The honest browser seam at the foot of the detail pane. The interactive Servo
/// browser is BOOKMARKS-5/6; until it lands this pane is detail-only, and any
/// "open in a tab" intent is listed here — a clearly-labelled seam, not a stub (§7).
fn browser_seam(ui: &mut egui::Ui, m: &Manager) {
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Browser").color(Style::TEXT_DIM).size(Style::SMALL).strong());
        mde_egui::muted_note(
            ui,
            "The sandboxed Servo browser arrives with BOOKMARKS-5/6. \
             Until then this pane shows details; \u{201c}open\u{201d} and add-from-page light up with it.",
        );
        let intent = m.open_intent();
        if !intent.is_empty() {
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::TEXT_DIM,
                format!("Queued to open ({}):", intent.len()),
            );
            for url in intent.iter().take(8) {
                ui.colored_label(
                    Style::ACCENT,
                    RichText::new(url).monospace().size(Style::SMALL),
                );
            }
        }
    });
}

// ── List (region 2) ──────────────────────────────────────────────────────────

fn list(ui: &mut egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    egui::CentralPanel::default().show_inside(ui, |ui| {
        ui.add_space(Style::SP_S);
        if m.selection_len() > 0 {
            bulk_bar(ui, m, actions);
            ui.add_space(Style::SP_XS);
        }
        let items = m.listing();
        ui.colored_label(Style::TEXT_DIM, format!("{} item(s)", items.len()));
        ui.add_space(Style::SP_XS);
        ui.separator();
        ui.add_space(Style::SP_XS);

        if items.is_empty() {
            empty_state(ui, m);
            return;
        }
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in &items {
                    list_row(ui, m, item, actions);
                }
                // A drop on the empty tail below the rows appends to the current
                // folder — so a drag that misses every row still lands honestly.
                tail_drop(ui, m, actions);
            });
    });
}

fn bulk_bar(ui: &mut egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.colored_label(Style::ACCENT, format!("{} selected", m.selection_len()));
            ui.add_space(Style::SP_S);
            if ui.button("Open all").clicked() {
                actions.push(Action::OpenSelection);
            }
            if ui.button("Copy URLs").clicked() {
                actions.push(Action::CopyUrls);
            }
            if ui.button("Move to current folder").clicked() {
                actions.push(Action::BulkMove(m.current()));
            }
            let del = egui::Button::new(RichText::new("Delete").color(Style::DANGER));
            if ui.add(del).clicked() {
                actions.push(Action::BulkDelete);
            }
        });
    });
}

fn empty_state(ui: &mut egui::Ui, m: &Manager) {
    ui.add_space(Style::SP_XL);
    ui.vertical_centered(|ui| {
        let (title, body) = if m.is_searching() {
            (
                "No matches",
                "No bookmark title or URL matches this search.",
            )
        } else if m.total() == 0 {
            (
                "No bookmarks yet",
                "Add one with +, paste a URL, or press Ctrl+N. They sync across the mesh once the worker is running.",
            )
        } else {
            ("This folder is empty", "Drag bookmarks here, or add one with +.")
        };
        // The empty-state hero title: one type tier up from body, in the honest
        // emphasis tone (Inter has no bold cut, so brightness is the weight cue).
        ui.label(
            RichText::new(title)
                .color(Style::TEXT_STRONG)
                .size(Style::TITLE),
        );
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(ui, body);
    });
}

/// One list row: a drag grip (the `DnD` source), a kind dot, and the clickable /
/// double-clickable / drop-target body.
fn list_row(ui: &mut egui::Ui, m: &Manager, item: &Item, actions: &mut Vec<Action>) {
    let id = item.id();
    let is_folder = matches!(item, Item::Folder(_));
    // Reserve a paint slot so the row's hover / selection wash lands BEHIND the
    // row content (grip · dot · label) and spans the full row width — the shared
    // reserve-then-set idiom the shell uses for row backgrounds.
    let bg_slot = ui.painter().add(egui::Shape::Noop);
    let row = ui.horizontal(|ui| {
        let handle = grip(ui);
        if handle.dragged() {
            egui::DragAndDrop::set_payload(ui.ctx(), DragItem(id));
        }
        mde_egui::status_dot(
            ui,
            if is_folder {
                Style::WARN
            } else {
                Style::ACCENT
            },
        );
        let label = row_label(item);
        // A bookmark row is data — a fixed-width kind tag, a title column, then
        // the URL. Render it monospace (mono-first, lock #3) so the columns line
        // up, and truncate to one line so a long URL degrades gracefully at any
        // width rather than wrapping.
        ui.add_sized(
            [ui.available_width(), Style::SP_L],
            egui::Label::new(
                RichText::new(label)
                    .monospace()
                    .color(Style::TEXT)
                    .size(Style::BODY),
            )
            .truncate()
            .sense(Sense::click()),
        )
    });
    let body = row.inner;
    let row_rect = row.response.rect;
    // A full-row hover wash, eased through the shared Motion table (FAST) so the
    // highlight fades rather than snapping; the selection wash is the steady
    // accent tint, matching the sibling file list's selected row.
    let hover_t = Motion::animate(
        ui.ctx(),
        ("bm-row", id),
        ui.rect_contains_pointer(row_rect),
        Motion::FAST,
    );
    let wash = if m.is_selected(id) {
        Some(Style::ACCENT.gamma_multiply(0.30))
    } else if hover_t > 0.0 {
        Some(Style::SURFACE_HI.gamma_multiply(hover_t))
    } else {
        None
    };
    if let Some(fill) = wash {
        ui.painter().set(
            bg_slot,
            egui::Shape::rect_filled(row_rect, Style::RADIUS, fill),
        );
    }
    if let Some(payload) = body.dnd_release_payload::<DragItem>() {
        actions.push(Action::MoveBefore {
            ids: m.drag_batch(payload.0),
            target: id,
        });
    }
    if body.double_clicked() {
        actions.push(Action::Open(id));
    } else if body.clicked() {
        let mods = ui.input(|i| i.modifiers);
        actions.push(if mods.command {
            Action::SelectToggle(id)
        } else if mods.shift {
            Action::SelectRange(id)
        } else {
            Action::SelectOnly(id)
        });
    }
}

/// The empty area below the last row: a catch-all drop zone that appends a
/// missed drop to the current folder.
fn tail_drop(ui: &mut egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    let (_id, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), Style::SP_XL),
        Sense::hover(),
    );
    if let Some(payload) = resp.dnd_release_payload::<DragItem>() {
        actions.push(Action::MoveInto {
            ids: m.drag_batch(payload.0),
            folder: m.current(),
        });
    }
}

/// The small draggable grip at the left of a list row — a 2×3 dot grid painted
/// from `Style` tokens (font-independent), sensing drag so it is the `DnD` source.
fn grip(ui: &mut egui::Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(Style::SP_M, Style::SP_L), Sense::drag());
    let color = if resp.hovered() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let painter = ui.painter();
    let radius = Style::SP_XS * 0.4;
    let gap = Style::SP_XS;
    let cx = rect.center().x;
    let cy = rect.center().y;
    for col in [-1.0_f32, 1.0] {
        for row in [-1.0_f32, 0.0, 1.0] {
            let center = egui::pos2((col * gap).mul_add(0.5, cx), row.mul_add(gap, cy));
            painter.circle_filled(center, radius, color);
        }
    }
    resp.on_hover_cursor(CursorIcon::Grab)
}

/// One list line's text: a fixed-width kind tag, the display label, and (for
/// bookmarks) the URL — Intel One Mono is monospace, so the tag column lines up.
fn row_label(item: &Item) -> String {
    match item {
        Item::Folder(f) => format!("DIR  {}", f.name),
        Item::Bookmark(b) => {
            let title = if b.title.is_empty() {
                b.url.as_str()
            } else {
                b.title.as_str()
            };
            format!("URL  {title:<28.28}  {}", b.url)
        }
    }
}

// ── Confirm dialog (lock Q30) ────────────────────────────────────────────────

fn confirm_dialog(ui: &egui::Ui, m: &Manager, actions: &mut Vec<Action>) {
    let Some(id) = m.confirm_delete() else {
        return;
    };
    let (name, count) = m
        .folder(id)
        .map_or_else(|| (String::new(), 0), |f| (f.name, m.descendant_count(id)));
    egui::Window::new("Delete folder?")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.colored_label(
                Style::TEXT,
                format!("\u{201c}{name}\u{201d} holds {count} item(s). Delete it and everything inside?"),
            );
            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                let del = egui::Button::new(RichText::new("Delete").color(Style::BG).strong())
                    .fill(Style::DANGER);
                if ui.add(del).clicked() {
                    actions.push(Action::ConfirmDeleteYes);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::ConfirmDeleteNo);
                }
            });
        });
}

// ── Apply ────────────────────────────────────────────────────────────────────

fn apply(ctx: &egui::Context, m: &mut Manager, action: Action) {
    match action {
        Action::OpenFolder(folder) => m.open_folder(folder),
        Action::ToggleExpanded(id) => m.toggle_expanded(id),
        Action::SelectOnly(id) => m.select_only(id),
        Action::SelectToggle(id) => m.select_toggle(id),
        Action::SelectRange(id) => m.select_range_to(id),
        Action::Open(id) => m.open(id),
        Action::OpenAdd => m.open_add(),
        Action::OpenAddWithUrl(url) => m.open_add_with_url(url),
        Action::CommitAdd => m.commit_add(),
        Action::CancelAdd => m.cancel_add(),
        Action::AddFolder(parent) => {
            // Create it, focus it, and drop straight into an inline rename so the
            // user names it (a folder is never left as the placeholder name).
            let id = m.add_folder("New folder", parent);
            m.select_only(id);
            m.begin_rename(id);
        }
        Action::BeginRename(id) => m.begin_rename(id),
        Action::CommitRename => m.commit_rename(),
        Action::CancelRename => m.cancel_rename(),
        Action::RequestDelete(id) => m.request_delete(id),
        Action::ConfirmDeleteYes => m.confirm_delete_yes(),
        Action::ConfirmDeleteNo => m.confirm_delete_no(),
        Action::SetSort(sort) => m.set_sort(sort),
        Action::ClearSearch => m.open_folder(m.current()),
        Action::OpenSelection => m.open_selection(),
        Action::CheckLinks => m.request_link_check(),
        Action::CopyUrls => {
            let text = m.copy_selected_urls();
            if !text.is_empty() {
                ctx.copy_text(text);
            }
        }
        Action::CopyOneUrl(id) => {
            let url = m.copy_url(id);
            if !url.is_empty() {
                ctx.copy_text(url);
            }
        }
        Action::BulkMove(folder) => m.bulk_move(folder),
        Action::BulkDelete => m.bulk_delete(),
        Action::MoveInto { ids, folder } => m.move_into(&ids, folder),
        Action::MoveBefore { ids, target } => m.move_before(&ids, target),
    }
}

// ── Small label helpers ──────────────────────────────────────────────────────

/// A source's short label for the detail pane.
const fn source_label(source: &Source) -> &str {
    match source {
        Source::Manual => "Added here",
        Source::Firefox => "Firefox import",
        Source::Chromium => "Chromium import",
        Source::Safari => "Safari import",
        Source::NetscapeHtml => "HTML import",
        Source::Other(_) => "Imported",
    }
}

/// `user · node` attribution, for the detail pane.
fn author_line(author: &mde_bookmarks::Author) -> String {
    if author.user.is_empty() && author.node.is_empty() {
        "unknown".to_string()
    } else {
        format!("{} \u{00b7} {}", author.user, author.node)
    }
}

#[cfg(test)]
mod tests {
    use super::bookmarks_panel;
    use crate::model::Manager;
    use mde_bookmarks::Author;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    /// A manager under a fixed test author (no env reads).
    fn manager() -> Manager {
        Manager::new(Author::new("tester".into(), "test-node".into()))
    }

    /// Drive one headless egui frame that renders [`bookmarks_panel`] into a real
    /// `CentralPanel`, then tessellate on the CPU so any paint-path fault surfaces
    /// as a test failure. This is the same `Context::run` → `tessellate` path the
    /// DRM runner drives, minus the GPU — no window, no wgpu — and it proves the
    /// panel is embeddable exactly as the E12 shell mounts it (E12-3b).
    fn render(m: &mut Manager) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 700.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| bookmarks_panel(ui, m));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "bookmarks_panel produced no draw primitives"
        );
    }

    #[test]
    fn renders_the_empty_first_run_state() {
        // The honest first-run state: an empty collection paints its "No bookmarks
        // yet" copy and the standalone detail/rail seams — no fabricated data (§7).
        let mut m = manager();
        assert!(m.is_empty());
        render(&mut m);
    }

    #[test]
    fn renders_the_populated_manager() {
        // A populated tree exercises the FULL paint path: the folder tree, the
        // list rows (grip + dot + label), the detail pane for a focused bookmark,
        // the bulk bar for a multi-selection, and the sort combo — all tessellated
        // off-GPU. Every item is built through real mde-bookmarks ops.
        let mut m = manager();
        let work = m.add_folder("Work", None);
        m.add_folder("Personal", None);
        let a = m.add_bookmark("https://rust-lang.org", "Rust", None);
        let b = m.add_bookmark("https://docs.rs", "docs.rs", None);
        m.add_bookmark("https://intranet.local", "Intranet", Some(work));
        m.toggle_expanded(work);
        // A multi-selection lights the bulk bar; the detail pane focuses `b`.
        m.select_only(a);
        m.select_toggle(b);
        render(&mut m);
    }

    #[test]
    fn renders_the_add_form_and_search_and_confirm() {
        // The transient UI states: the open add form, an active search, and the
        // non-empty-folder delete confirmation window — each a real render branch.
        let mut m = manager();
        let f = m.add_folder("Docs", None);
        m.add_bookmark("https://a.example", "Alpha", Some(f));
        // Add form open.
        m.open_add();
        m.add_url_mut().push_str("https://new.example");
        render(&mut m);
        // Active search.
        m.cancel_add();
        m.query_mut().push_str("alpha");
        render(&mut m);
        // Confirm dialog for the non-empty folder.
        m.query_mut().clear();
        m.request_delete(f);
        assert_eq!(m.confirm_delete(), Some(f));
        render(&mut m);
    }

    #[test]
    fn renders_a_focused_folder_and_open_intent() {
        // The detail pane for a folder, plus a recorded open intent surfaced under
        // the honest browser seam (the browser itself is BOOKMARKS-5/6).
        let mut m = manager();
        let f = m.add_folder("Reading", None);
        let bm = m.add_bookmark("https://blog.example", "Blog", None);
        m.select_only(f); // focus the folder in the detail pane
        m.open(bm); // record an open intent (browser seam)
        assert!(!m.open_intent().is_empty());
        render(&mut m);
    }

    #[test]
    fn renders_link_check_summary_and_detail() {
        let mut m = manager();
        let bm = m.add_bookmark("https://broken.example", "Broken", None);
        m.select_only(bm);
        m.apply_link_check_status(crate::model::LinkCheckStatus {
            op: "bookmarks_link_check".to_string(),
            node: "test-node".to_string(),
            checked_at_ms: 42,
            checked: 1,
            alive: 0,
            dead: 1,
            unsupported: 0,
            errors: 0,
            truncated: false,
            records: vec![crate::model::LinkCheckRecord {
                id: bm,
                url: "https://broken.example".to_string(),
                title: "Broken".to_string(),
                health: crate::model::LinkHealth::Dead,
                http_status: Some(404),
                detail: "HTTP 404".to_string(),
            }],
        });
        render(&mut m);
    }
}

//! The editor panel's **free-standing view/render helper functions** — the tab
//! strip (`tab_title` / `tab_chip` / `tab_bar`), the Format strip's markdown
//! heading-level read-back (`heading_level_of`), and the honest empty-state
//! faces (`no_folder_face` / `preview_empty` / `tree_toggle` / `empty_state`).
//! Split out of the `panel` god-module (pure relocation, no behaviour change);
//! each is a leaf the parent [`EditorSurface`] render loop drives, reading the
//! parent's private types + egui/`Style` imports via `use super::*`.

use super::*;

/// The tab's title: its file name, or "scratch" for a pathless buffer — the same
/// naming the old single-doc chrome used.
fn tab_title(doc: &Doc) -> String {
    doc.buffer.path().and_then(Path::file_name).map_or_else(
        || "scratch".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Render one pane's tab bar (EDITOR-6): a chip per open buffer (name + dirty
/// marker, active highlighted), a `×` close on each, a drag-to-reorder gesture,
/// and a trailing `+` new-tab button. Returns at most one [`LeafAction`] for the
/// caller to apply outside the pane borrow. Token-styled (§4).
pub(super) fn tab_bar(ui: &mut Ui, pane: &PaneTabs, pane_focused: bool) -> Option<LeafAction> {
    let mut action: Option<LeafAction> = None;
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_XS);
        let pointer_x = ui.input(|i| i.pointer.interact_pos()).map(|p| p.x);
        let mut rects: Vec<(usize, Rect)> = Vec::with_capacity(pane.tabs.len());
        let mut drag_release: Option<usize> = None;
        for (i, doc) in pane.tabs.iter().enumerate() {
            let title = tab_title(doc);
            let dirty = doc.buffer.is_dirty();
            let active = i == pane.active;
            let (resp, close_clicked) = tab_chip(ui, &title, active, dirty, active && pane_focused);
            rects.push((i, resp.rect));
            if close_clicked {
                action = Some(LeafAction::CloseTab(i));
            } else if resp.clicked() {
                action = Some(LeafAction::SelectTab(i));
            }
            if resp.drag_stopped() {
                drag_release = Some(i);
            }
        }
        // Resolve a drag-reorder against the collected chip rects.
        if let (Some(from), Some(px)) = (drag_release, pointer_x) {
            let target = rects
                .iter()
                .find(|(_, r)| px >= r.min.x && px <= r.max.x)
                .map_or(from, |(j, _)| *j);
            if target != from {
                action = Some(LeafAction::MoveTab { from, to: target });
            } else if action.is_none() {
                action = Some(LeafAction::SelectTab(from));
            }
        }
        ui.add_space(Style::SP_XS);
        if ui
            .selectable_label(false, RichText::new("+").size(Style::BODY))
            .on_hover_text("New tab (Ctrl+T)")
            .clicked()
        {
            action = Some(LeafAction::NewTab);
        }
    });
    action
}

/// One tab chip: a draggable, clickable pill (name + dirty marker) with a `×`
/// close zone on its right edge. Returns the chip's drag/click [`Response`] plus
/// whether the `×` was clicked. Every colour is a `Style` token (§4).
fn tab_chip(
    ui: &mut Ui,
    title: &str,
    active: bool,
    dirty: bool,
    show_active_underline: bool,
) -> (Response, bool) {
    let label = if dirty {
        format!("{title} \u{2022}")
    } else {
        title.to_owned()
    };
    let text_color = if active { Style::TEXT } else { Style::TEXT_DIM };
    let font = FontId::proportional(Style::SMALL);
    let galley = ui.painter().layout_no_wrap(label, font, text_color);
    let close_w = Style::SP_M;
    let pad = Style::SP_S;
    let size = vec2(
        2.0f32.mul_add(pad, galley.size().x) + close_w,
        2.0f32.mul_add(Style::SP_XS, galley.size().y),
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let close_rect = Rect::from_min_max(pos2(rect.max.x - close_w, rect.min.y), rect.max);
    let close_resp = ui.interact(close_rect, resp.id.with("close"), Sense::click());

    let bg = if active {
        Style::SURFACE_HI
    } else if resp.hovered() {
        Style::SURFACE
    } else {
        Style::BG
    };
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS, bg);
    if show_active_underline {
        // A hairline accent along the bottom edge marks the active tab of the
        // focused pane (the "you are here" cue).
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.min.x, rect.max.y - 2.0), rect.max),
            0.0,
            Style::ACCENT,
        );
    }
    painter.galley(
        pos2(rect.min.x + pad, rect.center().y - galley.size().y / 2.0),
        galley,
        text_color,
    );
    let x_color = if close_resp.hovered() {
        Style::WARN
    } else {
        Style::TEXT_DIM
    };
    painter.text(
        close_rect.center(),
        Align2::CENTER_CENTER,
        "\u{00d7}",
        FontId::proportional(Style::SMALL),
        x_color,
    );
    (resp, close_resp.clicked())
}

/// The markdown ATX heading level of `line` (0-based) — the leading `#`-run
/// (1-6) when it is a real heading (followed by a space or the line end), else 0
/// (Normal body text). The Format strip's Style dropdown read-back (EDTB-3); a
/// cheap, allocation-light mirror of the engine's own `#`-run detection.
pub(super) fn heading_level_of(buffer: &Buffer, line: usize) -> u8 {
    if line >= buffer.len_lines() {
        return 0;
    }
    let text = buffer.line(line);
    let hashes = text.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && matches!(text.chars().nth(hashes), Some(' ' | '\n') | None) {
        u8::try_from(hashes).unwrap_or(0)
    } else {
        0
    }
}

/// The project tree's "no folder open" face (§7) — an honest note plus a reachable
/// affordance to root the tree at the current working directory, so the tree is
/// exercisable before a Files send / folder picker lands. Sets `open_cwd` on click.
pub(super) fn no_folder_face(ui: &mut Ui, open_cwd: &mut bool) {
    ui.add_space(Style::SP_M);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("No folder open")
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        *open_cwd = ui
            .button(RichText::new("Open current folder").size(Style::SMALL))
            .clicked();
    });
}

/// The split-preview pane's honest empty face (§7): shown when the previewable
/// buffer has no content yet, so the pane is never a blank void. Token-styled (§4).
pub(super) fn preview_empty(ui: &mut Ui) {
    ui.add_space(Style::SP_M);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("Nothing to preview yet")
                .size(Style::BODY)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new("Type markdown to see it rendered here.")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    });
}

/// The left-edge toggle that shows/hides the project-tree side panel, shared by the
/// open-document and empty-state chromes. A token-styled (§4) `selectable_label`
/// bound to `show_tree`.
pub(super) fn tree_toggle(ui: &mut Ui, show_tree: &mut bool) {
    if ui
        .selectable_label(*show_tree, RichText::new("\u{2630}").size(Style::BODY))
        .on_hover_text("Toggle the project tree")
        .clicked()
    {
        *show_tree = !*show_tree;
    }
}

/// The "no document" face — the honest empty state (§7), plus a temporary button
/// to open a scratch buffer so the surface is exercisable before fuzzy-open
/// lands. Returns `true` when the operator clicked the button (the caller opens
/// the buffer). Every value is a shared [`Style`] token (§4), no raw hex/metric.
pub(super) fn empty_state(ui: &mut Ui) -> bool {
    let mut open = false;
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_XL);
        ui.label(
            RichText::new(NO_FILE_TITLE)
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(NO_FILE_HINT)
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_L);
        open = ui
            .button(RichText::new("Open a scratch buffer").size(Style::BODY))
            .clicked();
    });
    open
}

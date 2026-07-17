use super::{
    editor_panel, EditorSurface, ABOUT_PRODUCT_LINE, NO_FILE_HINT, NO_FILE_TITLE, SCRATCH_SEED,
};
use crate::menu_bar::MenuAction;
use crate::palette::PaletteCommand;
use crate::real_editor;
use mde_egui::egui::{self, pos2, vec2, Event, Key, Modifiers, Rect};
use mde_egui::Style;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Build a key-press event for the headless driver (mirrors `widget.rs`'s test
/// helper): pressed, non-repeat, with the given modifiers.
fn key_press(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Drive one real `editor_panel` frame on a *persistent* `ctx` with injected
/// `events`, so a multi-frame interaction (open an overlay, then act on it)
/// exercises the true render + routing path — not a mocked seam.
fn run_frame(ctx: &egui::Context, surface: &mut EditorSurface, events: Vec<Event>) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        events,
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            editor_panel(ui, surface);
        });
    });
}

/// A unique temp dir for a live editor test, cleaned up on drop (the crate has
/// no `tempfile` dev-dep — the same idiom `project_tree`'s tests use).
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "mde-editor-panel-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("create temp dir");
        Self(base)
    }
    fn join(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// Drive one headless frame through the editor panel, tessellating on the CPU
/// so any paint-path fault surfaces — the same `Context::run` → `tessellate`
/// path the shell's mount test drives, minus the GPU. Returns the primitive
/// count so callers can assert the surface actually paints.
fn tessellate_panel(surface: &mut EditorSurface) -> usize {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            editor_panel(ui, surface);
        });
    });
    ctx.tessellate(out.shapes, out.pixels_per_point).len()
}

/// Like [`tessellate_panel`] but at a caller-chosen panel width, so a test can
/// drive the EDTB-4 wide (full) vs narrow (compact) bar layouts. Returns the
/// primitive count so callers can assert the surface actually paints.
fn tessellate_panel_at(surface: &mut EditorSurface, width: f32) -> usize {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 640.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            editor_panel(ui, surface);
        });
    });
    ctx.tessellate(out.shapes, out.pixels_per_point).len()
}

#[test]
fn empty_state_panel_mounts_and_renders_headless() {
    let mut surface = real_editor();
    assert!(!surface.is_open(), "a fresh surface opens no document");
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the empty-state editor surface produced no draw primitives"
    );
}

#[test]
fn opening_a_document_renders_the_widget() {
    let mut surface = real_editor();
    surface.open_text("fn main() {}\n");
    assert!(surface.is_open(), "open_text opens a document");
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the open-document editor surface produced no draw primitives"
    );
    surface.close();
    assert!(!surface.is_open(), "close returns to the empty state");
}

/// EDITOR-10 — the integrated terminal dock is reachable through the real
/// `editor_panel` path: the Ctrl+Backtick chord toggles it on, it spawns a real PTY
/// in the open project root, the panel mounts + paints with it shown, and the
/// chord toggles it back off — all without losing the session (the acceptance).
#[test]
fn the_terminal_dock_toggles_and_spawns_in_the_project_cwd() {
    let tmp = TempDir::new("term-dock");
    let want = tmp.0.canonicalize().expect("canonical project root");
    let mut surface = real_editor();
    surface.open_folder(want.clone());
    assert!(!surface.terminal.is_shown(), "the dock starts hidden");

    // Frame 1: Ctrl+` opens the dock (the panel-level chord intercept), and
    // the same frame mounts + paints the dock's real terminal (the chord fires
    // at the top of `editor_panel`, before the bottom panel renders). One ctx
    // for the whole interaction so the lazily-installed terminal font persists.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        events: vec![key_press(Key::Backtick, Modifiers::COMMAND)],
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            editor_panel(ui, &mut surface);
        });
    });
    assert!(
        surface.terminal.is_shown(),
        "Ctrl+` shows the terminal dock"
    );
    assert!(
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
        "the editor with the terminal dock produced no draw primitives"
    );
    let got = surface
        .terminal
        .active_cwd()
        .expect("the dock spawned a live local shell")
        .canonicalize()
        .expect("canonical shell cwd");
    assert_eq!(got, want, "the shell spawned in the open project root");

    // Frame 2: Ctrl+` again hides it — the session survives (still spawned).
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::Backtick, Modifiers::COMMAND)],
    );
    assert!(
        !surface.terminal.is_shown(),
        "Ctrl+` hides the terminal dock"
    );
    assert!(
        surface.terminal.active_cwd().is_some(),
        "hiding the dock kept its live session"
    );
}

/// EDITOR-10 — the View → Terminal menu action drives the SAME toggle seam as
/// the chord (§6, one implementation), and the menu context reports the state
/// for its checkmark.
#[test]
fn the_view_menu_terminal_action_toggles_the_dock() {
    let ctx = egui::Context::default();
    let mut surface = real_editor();
    assert!(!surface.menu_context().terminal_shown, "starts hidden");
    surface.run_action(&ctx, MenuAction::ToggleTerminal);
    assert!(
        surface.menu_context().terminal_shown,
        "the View → Terminal action shows the dock"
    );
    surface.run_action(&ctx, MenuAction::ToggleTerminal);
    assert!(
        !surface.menu_context().terminal_shown,
        "toggling again hides it"
    );
}

#[test]
fn open_scratch_seeds_a_real_editable_buffer() {
    let mut surface = EditorSurface::default();
    surface.open_scratch();
    assert!(surface.is_open());
    // The scratch seed is real, reachable text (not empty, not a stub).
    assert!(!SCRATCH_SEED.is_empty());
}

#[test]
fn empty_state_copy_is_honest_and_reachable() {
    assert!(!NO_FILE_TITLE.is_empty(), "empty-state title is blank");
    assert!(!NO_FILE_HINT.is_empty(), "empty-state hint is blank");
    assert!(
        NO_FILE_TITLE.to_lowercase().contains("file"),
        "the headline should name the missing file"
    );
    let hint = NO_FILE_HINT.to_lowercase();
    assert!(
        hint.contains("open") && hint.contains("edit"),
        "the hint should tell the operator to open a file to edit"
    );
}

#[test]
fn about_dialog_uses_canonical_quazar_identity() {
    assert_eq!(ABOUT_PRODUCT_LINE, "Quazar Editor");
    assert!(
        !ABOUT_PRODUCT_LINE.contains(concat!("Qua", "sar")),
        "Editor About copy must not drift back to the superseded spelling"
    );
}

#[test]
fn opening_a_source_file_attaches_its_highlighter() {
    // EDITOR-5 — the open-path seam picks the grammar by extension; a
    // pathless scratch buffer honestly stays plain.
    let d = TempDir::new("hl");
    let file = d.join("lib.rs");
    std::fs::write(&file, b"fn f() {}\n").expect("write");

    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    assert!(
        surface.doc().expect("doc").highlight.is_some(),
        "a .rs file gets the rust highlighter"
    );
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the highlighted document renders"
    );

    surface.open_scratch();
    assert!(
        surface.doc().expect("doc").highlight.is_none(),
        "a pathless scratch buffer renders plain (no guessed grammar)"
    );
}

#[test]
fn the_outline_lists_and_jumps_to_symbols() {
    // EDITOR-12 — the outline is derived from the SAME tree-sitter tree the
    // highlighter parses; a symbol click jumps the caret via `place_cursor`.
    let d = TempDir::new("outline");
    let file = d.join("shapes.rs");
    std::fs::write(
        &file,
        b"struct Point {\n    x: i32,\n}\n\nfn area() -> i32 {\n    0\n}\n",
    )
    .expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    surface.show_outline = true;
    // The first frame syncs the highlighter tree, so the outline populates.
    assert!(tessellate_panel(&mut surface) > 0, "panel + outline render");

    assert!(surface.active_has_grammar(), "the .rs doc has a grammar");
    let symbols = surface.active_symbols();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Point"), "struct listed: {names:?}");
    assert!(names.contains(&"area"), "fn listed: {names:?}");

    // Clicking the fn jumps the caret onto it (the outline row seam).
    let area = symbols.iter().find(|s| s.name == "area").expect("fn area");
    surface.jump_caret(area.char_start);
    let cursor = surface.doc().expect("doc").view.cursor();
    assert_eq!(cursor, area.char_start, "the caret jumped to the symbol");
}

#[test]
fn the_outline_shows_an_honest_empty_state_without_a_grammar() {
    // A pathless scratch buffer has no grammar → the panel shows the honest
    // empty state (no fabricated symbols, §7) and still renders.
    let mut surface = real_editor();
    surface.open_scratch();
    surface.show_outline = true;
    assert!(!surface.active_has_grammar(), "scratch has no grammar");
    assert!(surface.active_symbols().is_empty(), "no symbols to list");
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the empty-state outline still paints"
    );
}

#[test]
fn toggle_outline_command_flips_the_panel() {
    // §7 — the palette command drives the real panel flag (no dead entry).
    let mut surface = real_editor();
    let before = surface.show_outline;
    surface.run_command(PaletteCommand::ToggleOutline);
    assert_ne!(
        surface.show_outline, before,
        "the command toggled the outline"
    );
    surface.run_command(PaletteCommand::ToggleOutline);
    assert_eq!(surface.show_outline, before, "toggling again restores it");
}

#[test]
fn open_folder_sets_the_project_root_and_shows_the_tree() {
    let d = TempDir::new("folder");
    std::fs::write(d.join("a.rs"), b"fn main() {}").expect("write");
    let mut surface = real_editor();
    assert!(!surface.has_project(), "a fresh surface has no project");
    surface.open_folder(d.0.clone());
    assert!(surface.has_project(), "open_folder roots the project tree");
}

#[test]
fn a_tree_file_click_routes_through_open_path() {
    // The exact routing a project-tree file click drives: `show` yields the
    // clicked path, `editor_panel` hands it to `open_selected` → `open_path`.
    let d = TempDir::new("click");
    let file = d.join("hello.rs");
    std::fs::write(&file, b"fn hello() {}\n").expect("write");
    let mut surface = real_editor();
    surface.open_folder(d.0.clone());
    assert!(
        !surface.is_open(),
        "no document open until a file is picked"
    );

    surface.open_selected(&file);
    assert!(surface.is_open(), "the picked file opened a document");
    assert_eq!(
        surface.current_path(),
        Some(file.as_path()),
        "open-on-click opened the exact clicked file"
    );
}

#[test]
fn project_tree_panel_tessellates_over_a_real_dir() {
    // The whole editor body with the tree side panel shown paints real
    // primitives over a real directory listing (§7 — a reachable render path).
    let d = TempDir::new("render");
    std::fs::create_dir(d.join("src")).expect("mkdir src");
    std::fs::write(d.join("Cargo.toml"), b"[package]").expect("write");
    let mut surface = real_editor();
    surface.open_folder(d.0.clone());
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the editor + project tree produced no draw primitives"
    );
}

// ── EDITOR-7: the fuzzy finder + command palette ─────────────────────────

#[test]
fn cmd_p_then_enter_opens_the_selected_file_through_open_path() {
    // The full select→open routing, driven end-to-end through the real panel:
    // Cmd+P opens the finder (rooted at the project), Enter opens the
    // highlighted file via `open_path`. No mocked seam — real frames + events.
    let d = TempDir::new("finder-route");
    let file = d.join("routing_target.rs");
    std::fs::write(&file, b"fn go() {}\n").expect("write");

    let mut surface = real_editor();
    surface.open_folder(d.0.clone());

    let ctx = egui::Context::default();
    Style::install(&ctx);

    // Frame 1: Cmd+P opens the finder over the one seeded file (empty query
    // lists it, selection at row 0).
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::P, Modifiers::COMMAND)],
    );
    assert!(surface.finder.is_open(), "Cmd+P opened the file finder");
    assert!(!surface.is_open(), "no document is open yet");

    // Frame 2: Enter opens the highlighted result, routed to `open_path`.
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::Enter, Modifiers::NONE)],
    );
    assert!(surface.is_open(), "Enter opened a document");
    assert_eq!(
        surface.current_path(),
        Some(file.as_path()),
        "the finder opened the exact file through open_path"
    );
    assert!(
        !surface.finder.is_open(),
        "the finder closed after the pick"
    );
}

#[test]
fn cmd_shift_p_toggles_the_palette_at_the_panel_level() {
    // The palette chord is intercepted at the panel level (not the widget), so
    // pressing it opens the overlay; pressing it again closes it.
    let mut surface = real_editor();
    surface.open_text("fn main() {}\n");

    let ctx = egui::Context::default();
    Style::install(&ctx);

    let chord = || {
        key_press(
            Key::P,
            Modifiers {
                shift: true,
                ..Modifiers::COMMAND
            },
        )
    };
    run_frame(&ctx, &mut surface, vec![chord()]);
    assert!(surface.palette.is_open(), "Cmd+Shift+P opened the palette");
    run_frame(&ctx, &mut surface, vec![chord()]);
    assert!(
        !surface.palette.is_open(),
        "Cmd+Shift+P again closed the palette"
    );
}

#[test]
fn palette_command_save_writes_the_buffer_to_disk() {
    let d = TempDir::new("cmd-save");
    let file = d.join("save.txt");
    std::fs::write(&file, b"abc").expect("seed file");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    // Dirty the buffer, then dispatch Save through the palette seam.
    surface.doc_mut().expect("doc").buffer.insert(3, "DEF");
    assert!(surface.doc().expect("doc").buffer.is_dirty());

    surface.run_command(PaletteCommand::Save);

    assert!(
        !surface.doc().expect("doc").buffer.is_dirty(),
        "Save cleared the dirty flag"
    );
    assert_eq!(
        std::fs::read(&file).expect("read back"),
        b"abcDEF",
        "the Save command wrote the on-disk bytes (buffer.save)"
    );
}

#[test]
fn palette_command_open_scratch_opens_a_document() {
    let mut surface = real_editor();
    assert!(!surface.is_open());
    surface.run_command(PaletteCommand::OpenScratch);
    assert!(surface.is_open(), "Open Scratch opened a document");
}

#[test]
fn palette_command_toggle_tree_flips_the_side_panel() {
    let mut surface = real_editor();
    let before = surface.show_tree;
    surface.run_command(PaletteCommand::ToggleTree);
    assert_ne!(
        surface.show_tree, before,
        "Toggle Project Tree flipped the side panel"
    );
}

#[test]
fn palette_command_toggle_wrap_flips_the_editor_wrap() {
    let mut surface = real_editor();
    surface.open_text("a long line\n");
    let before = surface.doc().expect("doc").view.wrap();
    surface.run_command(PaletteCommand::ToggleWrap);
    assert_ne!(
        surface.doc().expect("doc").view.wrap(),
        before,
        "Toggle Soft-Wrap flipped the view's wrap"
    );
}

#[test]
fn palette_command_close_document_returns_to_empty_state() {
    let mut surface = real_editor();
    surface.open_text("x\n");
    assert!(surface.is_open());
    surface.run_command(PaletteCommand::CloseDoc);
    assert!(
        !surface.is_open(),
        "Close Document returned to the empty state"
    );
}

#[test]
fn palette_command_open_folder_roots_the_tree() {
    let mut surface = real_editor();
    assert!(!surface.has_project());
    surface.run_command(PaletteCommand::OpenFolderCwd);
    assert!(
        surface.has_project(),
        "Open Folder rooted the project tree at the cwd"
    );
}

#[test]
fn the_finder_overlay_paints_when_open() {
    let d = TempDir::new("finder-paint");
    std::fs::write(d.join("a.rs"), b"a").expect("write");
    let mut surface = real_editor();
    surface.open_folder(d.0.clone());
    surface.open_finder();
    assert!(surface.finder.is_open());
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the open finder overlay produced no draw primitives"
    );
}

#[test]
fn the_palette_overlay_paints_when_open() {
    let mut surface = real_editor();
    surface.toggle_palette();
    assert!(surface.palette.is_open());
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the open palette overlay produced no draw primitives"
    );
}

// ── EDITOR-8: project + in-buffer search ─────────────────────────────────

#[test]
fn ctrl_f_and_ctrl_h_open_the_find_bar_at_the_panel_level() {
    // The find chords are intercepted at the panel level (not the widget), so
    // they open the bar instead of typing into the buffer.
    let mut surface = real_editor();
    surface.open_text("fn main() {}\n");
    let ctx = egui::Context::default();
    Style::install(&ctx);

    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::F, Modifiers::COMMAND)],
    );
    assert!(surface.find.is_open(), "Ctrl+F opened the find bar");
    // Esc closes it (consumed by the overlay).
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::Escape, Modifiers::NONE)],
    );
    assert!(!surface.find.is_open(), "Esc closed the find bar");

    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::H, Modifiers::COMMAND)],
    );
    assert!(surface.find.is_open(), "Ctrl+H opened the replace bar");
}

#[test]
fn ctrl_shift_f_opens_the_project_search() {
    let d = TempDir::new("ps-open");
    std::fs::write(d.join("a.rs"), b"fn a() {}\n").expect("write");
    let mut surface = real_editor();
    surface.open_folder(d.0.clone());
    let ctx = egui::Context::default();
    Style::install(&ctx);

    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(
            Key::F,
            Modifiers {
                shift: true,
                ..Modifiers::COMMAND
            },
        )],
    );
    assert!(
        surface.project_search.is_open(),
        "Ctrl+Shift+F opened the project search"
    );
}

#[test]
fn find_recompute_highlights_the_right_ranges_and_cycles() {
    // Open the bar over a real buffer, set a query, recompute against the live
    // rope: the paint bands are the exact match ranges, and next/prev cycle the
    // caret onto each match through the real reveal seam.
    let mut surface = real_editor();
    surface.open_text("find me and find me again\n");
    surface.open_find();
    surface.find.set_query("find");
    surface.refresh_find_matches();

    let (bands, current) = surface.find_paint_bands();
    assert_eq!(bands, vec![0..4, 12..16], "both 'find' runs highlight");
    assert_eq!(current, Some(0), "the first match is current");

    surface.find.cycle(true);
    surface.reveal_current_match();
    assert_eq!(
        surface.doc().expect("doc").view.cursor(),
        12,
        "Next moved the caret to the second match's start"
    );
    // The whole surface still paints with the bar open + matches highlighted.
    assert!(tessellate_panel(&mut surface) > 0, "the find bar paints");
}

#[test]
fn find_replace_current_mutates_the_live_buffer() {
    let mut surface = real_editor();
    surface.open_text("aa bb aa\n");
    surface.open_replace();
    surface.find.set_query("aa");
    surface.find.set_replacement("zz");
    surface.refresh_find_matches();

    surface.replace_current_match();
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "zz bb aa\n",
        "Replace mutated the first match on the real rope"
    );
    // The edit is undoable (a real widget edit step).
    assert!(surface.doc().expect("doc").view.can_undo());
}

#[test]
fn find_replace_all_replaces_every_match() {
    let mut surface = real_editor();
    surface.open_text("aa bb aa\n");
    surface.open_replace();
    surface.find.set_query("aa");
    surface.find.set_replacement("z");
    surface.refresh_find_matches();
    assert_eq!(
        surface.find_paint_bands().0.len(),
        2,
        "two matches to replace"
    );

    surface.replace_all_matches();
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "z bb z\n",
        "Replace All replaced every match"
    );
}

#[test]
fn project_search_opens_a_hit_and_jumps_to_the_line() {
    let d = TempDir::new("ps-jump");
    let file = d.join("code.rs");
    std::fs::write(&file, b"fn a() {}\nfn target() {}\n").expect("write");
    let mut surface = real_editor();
    surface.open_folder(d.0.clone());
    surface.open_project_search();
    surface.project_search.set_query("target");
    surface.project_search.run();

    let hit = surface
        .project_search
        .results()
        .first()
        .expect("a hit for the seeded symbol")
        .clone();
    assert!(hit.path.ends_with("code.rs"), "the hit points at the file");

    surface.open_hit(&hit);
    assert!(surface.is_open(), "opening the hit opened the document");
    assert_eq!(surface.current_path(), Some(file.as_path()));
    let doc = surface.doc().expect("doc");
    let (line, col) = doc.view.line_col(&doc.buffer);
    assert_eq!(line, 2, "jumped to the hit's line (1-based)");
    assert_eq!(col, 4, "landed on the 'target' match column");
}

// ── EDTB-1: the menu bar + Standard toolbar dispatch ────────────────────

/// Run `action` through the real dispatch inside a live frame, returning
/// the frame's [`egui::FullOutput`] so tests can observe the platform
/// effects (clipboard commands, viewport commands).
fn run_action_in_frame(surface: &mut EditorSurface, action: MenuAction) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        surface.run_action(ctx, action);
    })
}

#[test]
fn menu_new_scratch_opens_a_document() {
    let mut surface = real_editor();
    run_action_in_frame(&mut surface, MenuAction::NewScratch);
    assert!(surface.is_open(), "File > New opened a scratch document");
}

#[test]
fn menu_open_routes_to_the_finder() {
    let mut surface = real_editor();
    run_action_in_frame(&mut surface, MenuAction::OpenFinder);
    assert!(
        surface.finder.is_open(),
        "File > Open… opened the Ctrl-P finder"
    );
}

/// EDTB-7 — the View → Preview / toolbar toggle is honest-gated on the buffer
/// type: a markdown / text buffer opens the split preview; a code buffer does
/// not offer it (the control greys out) and toggling it is a no-op (§7).
#[test]
fn preview_toggle_gates_on_the_buffer_type() {
    // A markdown buffer offers + opens the preview…
    let mut surface = real_editor();
    surface.open_text("# Title\n");
    assert!(
        surface.menu_context().preview_available,
        "a markdown buffer offers the Preview toggle"
    );
    run_action_in_frame(&mut surface, MenuAction::TogglePreview);
    assert!(surface.show_preview, "View > Preview opened the split pane");
    assert!(
        surface.menu_context().preview_shown,
        "the context reads the pane back as shown (the checkmark / pressed state)"
    );
    run_action_in_frame(&mut surface, MenuAction::TogglePreview);
    assert!(!surface.show_preview, "toggling again closed the pane");

    // …a code buffer greys the toggle out and toggling is an honest no-op.
    let d = TempDir::new("preview-gate");
    let code = d.join("main.rs");
    std::fs::write(&code, b"fn main() {}\n").expect("seed");
    let mut surface = real_editor();
    surface.open_path(&code).expect("open");
    assert!(
        !surface.menu_context().preview_available,
        "a code buffer does not offer the Preview toggle"
    );
    run_action_in_frame(&mut surface, MenuAction::TogglePreview);
    assert!(
        !surface.show_preview,
        "the preview stays closed for a code buffer (honest no-op)"
    );
}

/// EDTB-7 — the preview tracks the buffer live: opening the pane parses the
/// document, and an edit re-parses (debounced on the revision) so the rendered
/// blocks reflect what was typed — proven through the real `editor_panel`
/// paint path, not a mocked seam.
#[test]
fn the_preview_renders_live_as_the_buffer_changes() {
    let mut surface = real_editor();
    surface.open_text("# One\n");
    run_action_in_frame(&mut surface, MenuAction::TogglePreview);
    assert!(surface.show_preview, "the preview pane is open");

    // The first paint parses the initial buffer (one heading block).
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the split preview pane paints"
    );
    let before = surface.doc_mut().expect("doc").preview_blocks().to_vec();
    assert_eq!(before.len(), 1, "one heading block to start: {before:?}");

    // Type a fresh markdown paragraph, then re-paint.
    {
        let doc = surface.doc_mut().expect("doc");
        let end = doc.buffer.len_chars();
        doc.buffer.insert(end, "\nfresh **para** text\n");
    }
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the preview re-paints after the edit"
    );
    let after = surface.doc_mut().expect("doc").preview_blocks().to_vec();
    assert!(
        after.len() > before.len(),
        "the edit reflects live in the preview blocks: {before:?} -> {after:?}"
    );
}

#[test]
fn menu_open_folder_roots_the_project_tree() {
    let mut surface = real_editor();
    assert!(!surface.has_project());
    run_action_in_frame(&mut surface, MenuAction::OpenFolderCwd);
    assert!(
        surface.has_project(),
        "File > Open Folder rooted the tree at the cwd"
    );
}

#[test]
fn menu_save_writes_the_buffer_through_the_palette_seam() {
    let d = TempDir::new("menu-save");
    let file = d.join("menu.txt");
    std::fs::write(&file, b"abc").expect("seed");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    surface.doc_mut().expect("doc").buffer.insert(3, "!");
    run_action_in_frame(&mut surface, MenuAction::Save);
    assert_eq!(
        std::fs::read(&file).expect("read back"),
        b"abc!",
        "File > Save wrote the bytes (the same run_command(Save) seam)"
    );
}

#[test]
fn menu_save_as_commits_to_a_new_path_and_repicks_the_highlighter() {
    let d = TempDir::new("menu-save-as");
    let mut surface = real_editor();
    surface.open_text("fn f() {}\n");
    run_action_in_frame(&mut surface, MenuAction::SaveAs);
    assert!(surface.save_as.open, "File > Save As… opened the dialog");
    assert!(
        surface.overlay_active(),
        "the open dialog reports as an active overlay"
    );
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the Save As dialog produced no draw primitives"
    );

    let target = d.join("adopted.rs");
    surface.save_as.path = target.display().to_string();
    surface.save_as_commit();

    assert!(!surface.save_as.open, "a successful save closes the dialog");
    assert_eq!(
        std::fs::read(&target).expect("read back"),
        b"fn f() {}\n",
        "Save As wrote the buffer to the new path"
    );
    assert_eq!(
        surface.current_path(),
        Some(target.as_path()),
        "the buffer adopted the new path"
    );
    assert!(
        surface.doc().expect("doc").highlight.is_some(),
        "the .rs path re-picked a highlighter for the renamed document"
    );
}

#[test]
fn save_as_failure_keeps_the_dialog_open_with_the_error() {
    let mut surface = real_editor();
    surface.open_text("x");
    run_action_in_frame(&mut surface, MenuAction::SaveAs);
    surface.save_as.path = "/nonexistent-dir-mde-editor/x.txt".to_owned();
    surface.save_as_commit();
    assert!(surface.save_as.open, "a failed write keeps the dialog open");
    assert!(
        surface.save_as.error.is_some(),
        "the write error is shown, not swallowed"
    );
}

#[test]
fn menu_close_returns_to_the_empty_state() {
    let mut surface = real_editor();
    surface.open_text("x");
    run_action_in_frame(&mut surface, MenuAction::CloseDoc);
    assert!(!surface.is_open(), "File > Close closed the document");
}

#[test]
fn menu_undo_and_redo_unwind_a_typed_edit() {
    // Type through a REAL widget frame (the same path the keyboard takes),
    // then unwind/redo through the menu seams.
    let mut surface = real_editor();
    surface.open_text("abc");
    let ctx = egui::Context::default();
    Style::install(&ctx);
    run_frame(&ctx, &mut surface, vec![Event::Text("X".to_owned())]);
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "Xabc",
        "the frame's Text event inserted at the caret"
    );
    assert!(surface.menu_context().can_undo, "the edit armed Undo");

    surface.run_action(&ctx, MenuAction::Undo);
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "abc",
        "Edit > Undo unwound the typed edit"
    );
    assert!(surface.menu_context().can_redo, "undo armed Redo");

    surface.run_action(&ctx, MenuAction::Redo);
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "Xabc",
        "Edit > Redo re-applied the edit"
    );
}

#[test]
fn menu_select_all_selects_the_whole_buffer() {
    let mut surface = real_editor();
    surface.open_text("hello");
    run_action_in_frame(&mut surface, MenuAction::SelectAll);
    assert_eq!(
        surface.doc().expect("doc").view.selection(),
        Some(0..5),
        "Edit > Select All selected the whole document"
    );
    assert!(surface.menu_context().has_selection);
}

#[test]
fn menu_copy_emits_the_clipboard_command_with_the_selection() {
    let mut surface = real_editor();
    surface.open_text("hello");
    run_action_in_frame(&mut surface, MenuAction::SelectAll);
    let out = run_action_in_frame(&mut surface, MenuAction::Copy);
    let copied = out
        .platform_output
        .commands
        .iter()
        .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "hello"));
    assert!(copied, "Edit > Copy put the selection on the clipboard");
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "hello",
        "Copy leaves the buffer untouched"
    );
}

#[test]
fn menu_cut_copies_then_deletes_the_selection() {
    let mut surface = real_editor();
    surface.open_text("hello");
    run_action_in_frame(&mut surface, MenuAction::SelectAll);
    let out = run_action_in_frame(&mut surface, MenuAction::Cut);
    let copied = out
        .platform_output
        .commands
        .iter()
        .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "hello"));
    assert!(copied, "Edit > Cut copied the selection first");
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "",
        "Edit > Cut deleted the selection"
    );
}

#[test]
fn menu_paste_requests_the_platform_clipboard() {
    // The dispatch asks the backend for its clipboard; the backend answers
    // with the same `Event::Paste` the widget's Ctrl-V inserts (that insert
    // path is covered by the widget's own paste tests).
    let mut surface = real_editor();
    surface.open_text("x");
    let out = run_action_in_frame(&mut surface, MenuAction::Paste);
    let requested = out
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .is_some_and(|v| {
            v.commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::RequestPaste))
        });
    assert!(requested, "Edit > Paste sent ViewportCommand::RequestPaste");
}

#[test]
fn menu_toggles_and_palette_route_to_their_seams() {
    let mut surface = real_editor();
    surface.open_text("a long line\n");
    let tree_before = surface.show_tree;
    run_action_in_frame(&mut surface, MenuAction::ToggleTree);
    assert_ne!(surface.show_tree, tree_before, "View > Project Tree flips");

    let wrap_before = surface.doc().expect("doc").view.wrap();
    run_action_in_frame(&mut surface, MenuAction::ToggleWrap);
    assert_ne!(
        surface.doc().expect("doc").view.wrap(),
        wrap_before,
        "View > Soft-Wrap flips"
    );

    run_action_in_frame(&mut surface, MenuAction::CommandPalette);
    assert!(
        surface.palette.is_open(),
        "Tools > Command Palette… opened the overlay"
    );
}

#[test]
fn menu_about_opens_the_dialog_and_it_paints() {
    let mut surface = real_editor();
    run_action_in_frame(&mut surface, MenuAction::About);
    assert!(surface.about_open, "Help > About opened the dialog");
    assert!(
        surface.overlay_active(),
        "the About dialog reports as an active overlay"
    );
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the About dialog produced no draw primitives"
    );
    assert!(
        !super::ABOUT_VERSION_LINE.is_empty()
            && super::ABOUT_VERSION_LINE.contains("mde-editor-egui"),
        "the About line names the crate + version"
    );
}

#[test]
fn toolbar_zoom_sets_the_editor_view_scale() {
    let mut surface = real_editor();
    surface.open_text("x");
    assert_eq!(
        surface.menu_context().zoom_percent,
        Some(100),
        "a fresh document opens at 100%"
    );
    run_action_in_frame(&mut surface, MenuAction::Zoom(150));
    assert_eq!(
        surface.menu_context().zoom_percent,
        Some(150),
        "the Zoom dropdown set the view's font scale"
    );
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the zoomed editor still paints"
    );
}

#[test]
fn zoom_without_a_document_is_a_genuine_no_op() {
    let mut surface = real_editor();
    assert_eq!(
        surface.menu_context().zoom_percent,
        None,
        "no document, no zoom value (the dropdown is omitted)"
    );
    run_action_in_frame(&mut surface, MenuAction::Zoom(150));
    assert!(!surface.is_open(), "Zoom with no document changed nothing");
}

#[test]
fn doc_gated_actions_are_no_ops_on_the_empty_surface() {
    // Every doc-gated dispatch arm survives the empty state as a genuine
    // no-op (§7 — never a panic); the bars also grey these out.
    let mut surface = real_editor();
    for action in [
        MenuAction::Save,
        MenuAction::CloseDoc,
        MenuAction::Undo,
        MenuAction::Redo,
        MenuAction::Cut,
        MenuAction::Copy,
        MenuAction::SelectAll,
        MenuAction::ToggleWrap,
        MenuAction::Zoom(200),
    ] {
        run_action_in_frame(&mut surface, action);
    }
    assert!(!surface.is_open(), "the surface stayed in the empty state");
}

// ── EDTB-6: the spelling walk (hunspell) ─────────────────────────────────

use crate::spell::{SpellMiss, SpellState};

#[test]
fn spell_walk_replace_edits_the_buffer_through_the_shared_seam() {
    // Injecting a completed check (no live hunspell needed on the farm), the
    // walk's Replace applies a real undoable rope edit — the same
    // EditorView::replace_range the find bar uses (§6/§7).
    let mut surface = real_editor();
    surface.open_text("wrold ok\n"); // a pathless scratch = spell-checkable prose
    let miss = SpellMiss {
        chars: 0..5,
        word: "wrold".to_owned(),
        suggestions: vec!["world".to_owned()],
    };
    surface
        .doc_mut()
        .expect("doc")
        .spell
        .set_misses_for_test(vec![miss.clone()], 1);
    assert_eq!(surface.doc().unwrap().spell.misses().len(), 1);

    surface.spell_replace_current(&miss, "world");
    assert_eq!(
        surface.doc().unwrap().buffer.rope().to_string(),
        "world ok\n",
        "Replace fixed the misspelling in the live buffer"
    );
    assert!(
        surface.menu_context().can_undo,
        "the replace armed a real undo step"
    );
}

#[test]
fn spell_replace_skips_a_stale_span_rather_than_corrupting_text() {
    // §7: if the stored span no longer holds the missed word (an edit shifted
    // it), Replace is a guarded no-op, never a corruption of unrelated text.
    let mut surface = real_editor();
    surface.open_text("hello world\n");
    let stale = SpellMiss {
        chars: 0..5, // "hello", not the word the miss claims
        word: "wrold".to_owned(),
        suggestions: vec!["world".to_owned()],
    };
    surface.spell_replace_current(&stale, "world");
    assert_eq!(
        surface.doc().unwrap().buffer.rope().to_string(),
        "hello world\n",
        "a stale span left the buffer untouched"
    );
}

#[test]
fn spell_walk_is_honest_when_hunspell_is_unavailable() {
    // §7: with hunspell absent (the cfg(test) default — the probe never runs a
    // subprocess), F7 / Tools → Spelling does not open a dead dialog; it sets
    // the truthful "hunspell not installed" note instead.
    let mut surface = real_editor();
    surface.open_text("some prose here\n");
    surface.ensure_spell_probe(); // stays Unavailable under cfg(test)
    assert_eq!(surface.spell_state, SpellState::Unavailable);

    surface.open_spell_walk();
    assert!(
        !surface.spell_walk.open,
        "the walk does not open with no checker"
    );
    assert_eq!(
        surface.spell_notice.as_deref(),
        Some("hunspell not installed"),
        "the honest absent-state note is surfaced (§7)"
    );
}

#[test]
fn spell_gate_is_md_text_first_over_real_files() {
    // md/text first: the Spelling control's gate reflects the open file type —
    // greyed for a code buffer, live for markdown (both real on-disk files).
    let tmp = TempDir::new("spell-gate");
    let code = tmp.join("main.rs");
    let prose = tmp.join("notes.md");
    std::fs::write(&code, "fn main() {}\n").expect("write code");
    std::fs::write(&prose, "helo world\n").expect("write prose");

    let mut surface = real_editor();
    surface.spell_state = SpellState::Ready; // pretend hunspell is installed
    surface.spell_probed = true;

    surface.open_path(&code).expect("open code");
    assert!(
        !surface.menu_context().spellcheckable,
        "a .rs buffer is not spell-checked (md/text first)"
    );

    surface.open_path(&prose).expect("open prose");
    assert!(
        surface.menu_context().spellcheckable,
        "a .md buffer is spell-checked"
    );
    assert!(surface.menu_context().spell_available, "hunspell available");
}

#[test]
fn spell_walk_opens_and_renders_when_ready() {
    // With hunspell "ready" and injected misses, the walk opens and renders in
    // a real frame (§7 — the dialog is reachable, not a mockup). Rendered
    // directly (not via editor_panel) so no background subprocess is spawned.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut surface = real_editor();
    surface.spell_state = SpellState::Ready;
    surface.spell_probed = true;
    surface.open_text("teh quick fox\n");
    surface.doc_mut().unwrap().spell.set_misses_for_test(
        vec![SpellMiss {
            chars: 0..3,
            word: "teh".to_owned(),
            suggestions: vec!["the".to_owned()],
        }],
        1,
    );
    surface.open_spell_walk();
    assert!(surface.spell_walk.open, "the walk opened");

    let input = || egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        ..Default::default()
    };
    // Two frames: an egui Window/Area sizes itself on the first pass and paints
    // on the settled second — tessellate that one.
    let _ = ctx.run(input(), |ctx| surface.render_spell_walk(ctx));
    let out = ctx.run(input(), |ctx| surface.render_spell_walk(ctx));
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(!prims.is_empty(), "the walk dialog painted");
    assert!(surface.spell_walk.open, "still open after a passive render");
}

// ── EDTB-3: the Formatting strip + Insert/Table + Format menus ───────────

use crate::menu_bar::{ListStyle, WrapMarker};

/// The current buffer text of the open document (test helper).
fn text_of(surface: &EditorSurface) -> String {
    surface.doc().expect("doc").buffer.rope().to_string()
}

#[test]
fn format_bold_wraps_the_selection_as_one_undo_step() {
    // The exact seam the strip's B button (and the Format → Bold menu twin)
    // emit — dispatched through the real `run_action`, observed on the bytes.
    let mut surface = real_editor();
    surface.open_text("word");
    run_action_in_frame(&mut surface, MenuAction::SelectAll);
    run_action_in_frame(&mut surface, MenuAction::Wrap(WrapMarker::Bold));
    assert_eq!(text_of(&surface), "**word**", "Bold wrapped the selection");
    assert!(surface.menu_context().can_undo, "the format op armed Undo");

    // ONE operator undo step reverts the whole wrap.
    run_action_in_frame(&mut surface, MenuAction::Undo);
    assert_eq!(text_of(&surface), "word", "one Undo reverts the wrap");
    assert!(
        !surface.menu_context().can_undo,
        "the wrap was exactly one step"
    );
}

#[test]
fn format_italic_underline_and_strike_wrap_with_their_markers() {
    for (marker, wrapped) in [
        (WrapMarker::Italic, "*word*"),
        (WrapMarker::Underline, "<u>word</u>"),
        (WrapMarker::Strike, "~~word~~"),
    ] {
        let mut surface = real_editor();
        surface.open_text("word");
        run_action_in_frame(&mut surface, MenuAction::SelectAll);
        run_action_in_frame(&mut surface, MenuAction::Wrap(marker));
        assert_eq!(text_of(&surface), wrapped, "{marker:?} wraps its markup");
    }
}

#[test]
fn format_heading_sets_the_hash_prefix_at_the_caret_line() {
    let mut surface = real_editor();
    surface.open_text("title\nbody\n");
    // Caret opens at line 0; the Style dropdown → Heading 2 hashes that line.
    run_action_in_frame(&mut surface, MenuAction::Heading(2));
    assert_eq!(text_of(&surface), "## title\nbody\n");
    // Normal Text (level 0) strips it back.
    run_action_in_frame(&mut surface, MenuAction::Heading(0));
    assert_eq!(text_of(&surface), "title\nbody\n");
}

#[test]
fn format_bullet_and_numbered_lists_toggle_the_selected_lines() {
    let mut surface = real_editor();
    surface.open_text("a\nb\n");
    run_action_in_frame(&mut surface, MenuAction::SelectAll);
    run_action_in_frame(&mut surface, MenuAction::List(ListStyle::Bullet));
    assert_eq!(text_of(&surface), "- a\n- b\n", "bullets on both lines");

    run_action_in_frame(&mut surface, MenuAction::SelectAll);
    run_action_in_frame(&mut surface, MenuAction::List(ListStyle::Numbered));
    assert_eq!(text_of(&surface), "1. a\n2. b\n", "converted to numbers");
}

#[test]
fn format_indent_shifts_the_caret_line_and_round_trips() {
    let mut surface = real_editor();
    surface.open_text("x\n");
    run_action_in_frame(&mut surface, MenuAction::Indent(1));
    assert_eq!(
        text_of(&surface),
        "  x\n",
        "increase indent adds two spaces"
    );
    run_action_in_frame(&mut surface, MenuAction::Indent(-1));
    assert_eq!(text_of(&surface), "x\n", "decrease indent removes them");
}

#[test]
fn insert_table_action_drops_a_skeleton_as_one_undo_step() {
    let mut surface = real_editor();
    surface.open_text("");
    run_action_in_frame(&mut surface, MenuAction::InsertTable { rows: 2, cols: 3 });
    assert_eq!(
        text_of(&surface),
        "| Col 1 | Col 2 | Col 3 |\n\
             | ----- | ----- | ----- |\n\
             |       |       |       |\n\
             |       |       |       |\n",
        "the grid-picker inserts a markdown table skeleton"
    );
    assert!(surface.menu_context().can_undo, "the insert armed Undo");
    run_action_in_frame(&mut surface, MenuAction::Undo);
    assert_eq!(text_of(&surface), "", "one Undo removes the whole table");
}

#[test]
fn insert_table_picker_opens_and_paints() {
    let mut surface = real_editor();
    surface.open_text("x");
    run_action_in_frame(&mut surface, MenuAction::InsertTablePicker);
    assert!(
        surface.table_picker.open,
        "Insert → Table… opened the picker"
    );
    assert!(
        surface.overlay_active(),
        "the open picker reports as an active overlay"
    );
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the grid-picker dialog produced no draw primitives"
    );
}

#[test]
fn the_style_dropdown_reads_back_the_caret_heading_level() {
    let mut surface = real_editor();
    surface.open_text("## heading\nbody\n");
    assert_eq!(
        surface.menu_context().heading_level,
        Some(2),
        "the Style box reflects the caret line's `##`"
    );
    // A non-heading line reads back Normal (0). Under EDITOR-6 this opens a
    // second tab, so its "plain" caret line is the active read-back.
    surface.open_text("plain\n");
    assert_eq!(surface.menu_context().heading_level, Some(0));
    // Closing every open tab returns to the empty state → no read-back
    // (the Style box greys out).
    surface.close();
    surface.close();
    assert_eq!(surface.menu_context().heading_level, None);
}

#[test]
fn format_actions_are_no_ops_on_the_empty_surface() {
    // Every EDTB-3 dispatch arm survives the empty state as a genuine no-op
    // (§7); the Formatting strip also greys these out (Gate::Doc).
    let mut surface = real_editor();
    for action in [
        MenuAction::Heading(3),
        MenuAction::Wrap(WrapMarker::Bold),
        MenuAction::List(ListStyle::Numbered),
        MenuAction::Indent(1),
        MenuAction::InsertTable { rows: 2, cols: 2 },
    ] {
        run_action_in_frame(&mut surface, action);
    }
    assert!(!surface.is_open(), "the surface stayed in the empty state");
    // The picker toggle is harmless with no document (grid-picker acts at
    // the caret only once a document is open).
    run_action_in_frame(&mut surface, MenuAction::InsertTablePicker);
    run_action_in_frame(&mut surface, MenuAction::InsertTable { rows: 1, cols: 1 });
    assert!(!surface.is_open());
}

#[test]
fn the_format_strip_paints_over_an_open_document() {
    // The whole three-bar chrome (menu + Standard + Formatting) tessellates
    // real primitives — the Formatting strip is mounted + reachable (§7).
    let mut surface = real_editor();
    surface.open_text("# title\n\n- item\n");
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the editor with the Formatting strip produced no draw primitives"
    );
}

// ── EDITOR-LSP-2: the language-server lifecycle wiring ───────────────────
//
// The suite never spawns a real OS language server (see `build_lsp_client`
// under `cfg(test)`): a serverless language (`.md` → NoServer) yields a
// gated client with no process, so the open/change/close *wiring* is
// exercised — the doc-sync calls are honest no-ops on the gated client. The
// honest gated *statuses* + the diagnostics paint are covered by `lsp_ui`'s
// and `widget`'s own tests.

#[test]
fn opening_a_recognized_file_starts_a_language_client() {
    let d = TempDir::new("lsp-open");
    let file = d.join("notes.md");
    std::fs::write(&file, b"# Title\n\nbody\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    assert!(
        surface.doc().expect("doc").lsp.is_some(),
        "opening a file with a known language attaches a client (didOpen)"
    );
}

#[test]
fn a_scratch_buffer_starts_no_language_client() {
    let mut surface = real_editor();
    surface.open_scratch();
    assert!(
        surface.doc().expect("doc").lsp.is_none(),
        "a pathless scratch buffer has nothing to serve"
    );
}

#[test]
fn a_plain_text_file_starts_no_language_client() {
    let d = TempDir::new("lsp-plain");
    let file = d.join("readme.txt");
    std::fs::write(&file, b"just prose\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    assert!(
        surface.doc().expect("doc").lsp.is_none(),
        "an unknown extension has no language — no server"
    );
}

#[test]
fn editing_an_open_file_pushes_a_didchange_each_frame() {
    // The per-frame `didChange` wiring: a real typed frame bumps the edit
    // generation and the panel's sync point advances `lsp_synced_gen` to
    // match — proof `on_change` fired for the settled buffer.
    let d = TempDir::new("lsp-change");
    let file = d.join("doc.md");
    std::fs::write(&file, b"body\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    assert_eq!(
        surface.doc().expect("doc").lsp_synced_gen,
        0,
        "a freshly opened doc is synced at generation 0"
    );

    let ctx = egui::Context::default();
    Style::install(&ctx);
    run_frame(&ctx, &mut surface, vec![Event::Text("X".to_owned())]);

    let doc = surface.doc().expect("doc");
    assert!(
        doc.view.edit_generation() >= 1,
        "the typed frame recorded a real edit"
    );
    assert_eq!(
        doc.lsp_synced_gen,
        doc.view.edit_generation(),
        "the panel pushed the settled buffer to the server (didChange)"
    );
}

#[test]
fn a_caret_only_frame_sends_no_didchange() {
    // The throttle: an arrow-key frame moves the caret but does not change
    // the buffer, so the sync generation must not advance.
    let d = TempDir::new("lsp-quiet");
    let file = d.join("doc.md");
    std::fs::write(&file, b"abc\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");

    let ctx = egui::Context::default();
    Style::install(&ctx);
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::ArrowRight, Modifiers::NONE)],
    );
    let doc = surface.doc().expect("doc");
    assert_eq!(
        doc.lsp_synced_gen, 0,
        "a caret-only frame is not resent to the server"
    );
}

#[test]
fn closing_a_document_tears_down_the_client_without_panic() {
    // Close fires didClose + shutdown then drops the client — the graceful
    // teardown path, exercised end to end.
    let d = TempDir::new("lsp-close");
    let file = d.join("x.md");
    std::fs::write(&file, b"hi\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    assert!(surface.is_open());
    surface.close();
    assert!(!surface.is_open(), "close returned to the empty state");
}

#[test]
fn switching_documents_opens_a_second_tab_with_its_own_client() {
    // Under EDITOR-6 opening a second file opens a second TAB (both stay
    // open); the newly opened tab becomes the active document and attaches
    // its own language client (didOpen).
    let d = TempDir::new("lsp-switch");
    let first = d.join("a.md");
    let second = d.join("b.md");
    std::fs::write(&first, b"a\n").expect("write");
    std::fs::write(&second, b"b\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&first).expect("open first");
    surface.open_path(&second).expect("open second");
    assert_eq!(
        surface.current_path(),
        Some(second.as_path()),
        "the second file is now the active document"
    );
    assert!(
        surface.doc().expect("doc").lsp.is_some(),
        "the active document has its own client"
    );
}

// ── EDTB-4: the compact-aware bars ───────────────────────────────────────

#[test]
fn the_compact_threshold_switches_at_the_token_width() {
    // The layout decision is a pure fn of the panel's available width: at or
    // above the token-derived threshold the full bars render; just below it
    // the strips lean out (compact).
    assert!(
        !super::is_compact(super::COMPACT_WIDTH),
        "at the threshold the full bars render"
    );
    assert!(!super::is_compact(super::COMPACT_WIDTH + 1.0));
    assert!(
        super::is_compact(super::COMPACT_WIDTH - 1.0),
        "just under the threshold is compact"
    );
    assert!(super::is_compact(320.0), "a phone-narrow panel is compact");
    assert!(!super::is_compact(1280.0), "a desktop-wide panel is full");
}

#[test]
fn the_full_bars_render_at_a_wide_panel() {
    // §7 — at a wide panel the full three-bar chrome (menu + Standard +
    // Formatting, the Style + Zoom dropdowns inline) paints real primitives.
    let mut surface = real_editor();
    surface.open_text("# title\n\nbody\n");
    assert!(!super::is_compact(1200.0), "1200px is the full layout");
    assert!(
        tessellate_panel_at(&mut surface, 1200.0) > 0,
        "the full bars produced no draw primitives"
    );
}

#[test]
fn the_bars_lean_out_and_still_paint_at_a_narrow_panel() {
    // §7 — at a narrow panel the strips go compact (the wide dropdowns fold
    // into the `»` overflow) and still paint real primitives over an open
    // document (Zoom + Style present, so both overflows render).
    let mut surface = real_editor();
    surface.open_text("# title\n\nbody\n");
    assert!(super::is_compact(400.0), "400px is the compact layout");
    assert!(
        tessellate_panel_at(&mut surface, 400.0) > 0,
        "the compact bars produced no draw primitives"
    );
}

#[test]
fn compact_bars_keep_every_command_reachable_and_dispatching() {
    // §7 — the folded controls are relocated into the `»` overflow, never
    // lost: the SAME MenuAction they emit still dispatches. Render the whole
    // panel narrow (compact), then drive the two overflowed controls' actions
    // through the real seam — Zoom (Standard-strip overflow) and Heading
    // (Format-strip overflow) both act on the live document at compact width.
    let mut surface = real_editor();
    surface.open_text("title\n");
    assert!(
        super::is_compact(400.0),
        "the panel is in the compact layout at this width"
    );
    assert!(
        tessellate_panel_at(&mut surface, 400.0) > 0,
        "the compact bars paint"
    );
    // The overflowed Zoom still zooms…
    run_action_in_frame(&mut surface, MenuAction::Zoom(150));
    assert_eq!(
        surface.menu_context().zoom_percent,
        Some(150),
        "the overflow Zoom still sets the view scale"
    );
    // …and the overflowed paragraph Style still sets the heading.
    run_action_in_frame(&mut surface, MenuAction::Heading(2));
    assert_eq!(
        text_of(&surface),
        "## title\n",
        "the overflow Style still sets the caret-line heading"
    );
}

// ── EDITOR-6: tabs + splittable panes ────────────────────────────────────

use super::{Doc, PaneTabs};
use crate::buffer::Buffer;
use crate::panes::{NavDir, SplitDir};

/// The number of open tabs in the focused pane (test helper).
fn focused_tabs(surface: &EditorSurface) -> usize {
    surface
        .panes
        .get(&surface.focus)
        .map_or(0, |pane| pane.tabs.len())
}

#[test]
fn pane_tabs_close_keeps_the_active_index_valid() {
    // The pure tab-strip model: closing tabs never leaves `active` dangling.
    let mut pane = PaneTabs::empty();
    pane.push(Doc::new(Buffer::from_text("a")));
    pane.push(Doc::new(Buffer::from_text("b")));
    pane.push(Doc::new(Buffer::from_text("c")));
    assert_eq!(pane.active, 2, "push activates the new tab");
    // Close the middle tab: active (the last) shifts left to stay in range.
    pane.close(1);
    assert_eq!(pane.tabs.len(), 2);
    assert!(pane.active < pane.tabs.len(), "active stays in range");
    // Close down to empty without panicking.
    pane.close(0);
    pane.close(0);
    assert!(pane.is_empty());
    assert_eq!(pane.active_index(), None, "an empty pane has no active tab");
}

#[test]
fn pane_tabs_move_reorders_and_follows_the_moved_tab() {
    let mut pane = PaneTabs::empty();
    pane.push(Doc::new(Buffer::from_text("a")));
    pane.push(Doc::new(Buffer::from_text("b")));
    pane.push(Doc::new(Buffer::from_text("c")));
    // Move the first tab ("a") to the end.
    pane.move_tab(0, 2);
    let order: Vec<String> = pane
        .tabs
        .iter()
        .map(|d| d.buffer.rope().to_string())
        .collect();
    assert_eq!(order, vec!["b", "c", "a"], "the tab moved to the end");
    assert_eq!(pane.active, 2, "the moved tab stays active");
}

#[test]
fn ctrl_t_opens_a_new_tab_in_the_focused_pane() {
    let mut surface = real_editor();
    surface.open_text("first\n");
    assert_eq!(focused_tabs(&surface), 1);

    let ctx = egui::Context::default();
    Style::install(&ctx);
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::T, Modifiers::COMMAND)],
    );
    assert_eq!(
        focused_tabs(&surface),
        2,
        "Ctrl-T opened a second tab in the focused pane"
    );
}

#[test]
fn ctrl_w_closes_the_active_tab() {
    let mut surface = real_editor();
    surface.open_text("one\n");
    surface.open_text("two\n");
    assert_eq!(focused_tabs(&surface), 2);

    let ctx = egui::Context::default();
    Style::install(&ctx);
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::W, Modifiers::COMMAND)],
    );
    assert_eq!(focused_tabs(&surface), 1, "Ctrl-W closed the active tab");
    assert!(surface.is_open(), "the surviving tab is still open");
}

#[test]
fn splitting_shows_two_live_buffers_at_once() {
    // The acceptance: a split shows two buffers at once — each a real,
    // independently editable rope (§7).
    let mut surface = real_editor();
    surface.open_text("hello\n");
    surface.split_focused(SplitDir::V);
    assert_eq!(surface.pane_count(), 2, "the surface split into two panes");
    let with_docs = surface
        .panes
        .values()
        .filter(|pane| pane.active_doc().is_some())
        .count();
    assert_eq!(with_docs, 2, "both panes show a live buffer");
    // The new pane's buffer is an independent copy of the source text.
    for pane in surface.panes.values() {
        assert_eq!(
            pane.active_doc().expect("doc").buffer.rope().to_string(),
            "hello\n",
            "each pane holds the same text in its own rope"
        );
    }
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the split surface produced no draw primitives"
    );
}

#[test]
fn the_split_chord_splits_the_focused_pane() {
    let mut surface = real_editor();
    surface.open_text("body\n");
    assert_eq!(surface.pane_count(), 1);

    let ctx = egui::Context::default();
    Style::install(&ctx);
    // Ctrl+\ splits vertically (side by side).
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::Backslash, Modifiers::COMMAND)],
    );
    assert_eq!(
        surface.pane_count(),
        2,
        "Ctrl+\\ split the focused pane in two"
    );
}

#[test]
fn closing_the_last_tab_in_a_split_collapses_the_pane() {
    let mut surface = real_editor();
    surface.open_text("a\n");
    surface.split_focused(SplitDir::H);
    assert_eq!(surface.pane_count(), 2);
    // The focus is now the new (second) pane; closing its only tab collapses
    // the split back to the sibling pane.
    surface.close();
    assert_eq!(
        surface.pane_count(),
        1,
        "the emptied pane collapsed to its sibling"
    );
    assert!(surface.is_open(), "the sibling pane kept its buffer");
}

#[test]
fn navigate_focus_moves_between_split_panes() {
    let mut surface = real_editor();
    surface.open_text("left\n");
    surface.split_focused(SplitDir::V); // focus is now the right pane
    let right = surface.focus;
    surface.navigate_focus(NavDir::Left);
    assert_ne!(
        surface.focus, right,
        "Alt+Left moved focus to the left pane"
    );
    surface.navigate_focus(NavDir::Right);
    assert_eq!(
        surface.focus, right,
        "Alt+Right moved focus back to the right"
    );
}

#[test]
fn reopening_an_open_file_focuses_its_existing_tab() {
    let d = TempDir::new("dedup");
    let file = d.join("once.rs");
    std::fs::write(&file, b"fn f() {}\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    surface.open_path(&file).expect("reopen");
    assert_eq!(
        focused_tabs(&surface),
        1,
        "reopening the same file focused its tab instead of stacking a duplicate"
    );
}

#[test]
fn a_fresh_surface_has_one_pane_and_no_tabs() {
    let surface = real_editor();
    assert_eq!(surface.pane_count(), 1, "the surface opens with one pane");
    assert_eq!(focused_tabs(&surface), 0, "and no open tabs (empty state)");
    assert!(!surface.is_open());
}

// ── EDITOR-LSP-3: navigation routing over the surface (no live server) ────
//
// The test build spawns no real language server (see `build_lsp_client`), so
// the async request → reply round-trip is proven in `lsp`'s fake-server
// tests. Here the reply *routing* — the jump / list / cross-file edit / format
// application + the honest server-absent no-op — is driven through the real
// surface seams by handing `route_reply` a synthesized reply.

use crate::lsp::{Location, LspRange, LspReply, TextEdit, WorkspaceEdit};

/// A single-line LSP range at the given UTF-16 columns.
fn lsp_range(line: u32, c0: u32, c1: u32) -> LspRange {
    LspRange {
        start_line: line,
        start_character: c0,
        end_line: line,
        end_character: c1,
    }
}

#[test]
fn definition_reply_opens_the_target_and_jumps() {
    let d = TempDir::new("lsp3-def");
    let src = d.join("src.rs");
    std::fs::write(&src, b"use dep;\n").expect("write src");
    let target = d.join("dep.rs");
    std::fs::write(&target, b"pub fn thing() {}\n").expect("write target");
    let mut surface = real_editor();
    surface.open_path(&src).expect("open src");
    // A definition at dep.rs line 0, col 7 (the `thing` identifier).
    let loc = Location {
        path: target.clone(),
        range: lsp_range(0, 7, 12),
    };
    surface.route_reply(LspReply::Definition(vec![loc]));
    assert_eq!(
        surface.current_path(),
        Some(target.as_path()),
        "the definition target opened"
    );
    assert_eq!(
        surface.doc().expect("a doc").view.cursor(),
        7,
        "the caret jumped onto the definition"
    );
}

#[test]
fn empty_definition_reply_is_an_honest_notice() {
    let d = TempDir::new("lsp3-nodef");
    let src = d.join("src.rs");
    std::fs::write(&src, b"fn f() {}\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&src).expect("open");
    surface.route_reply(LspReply::Definition(Vec::new()));
    assert_eq!(surface.lsp_notice.as_deref(), Some("No definition found"));
}

#[test]
fn references_reply_populates_the_list_and_a_pick_jumps() {
    let d = TempDir::new("lsp3-refs");
    let a = d.join("a.rs");
    std::fs::write(&a, b"let name = 1;\nuse name;\n").expect("write a");
    let b = d.join("b.rs");
    std::fs::write(&b, b"// unrelated\nname();\n").expect("write b");
    let mut surface = real_editor();
    surface.open_path(&a).expect("open a");
    let locs = vec![
        Location {
            path: a.clone(),
            range: lsp_range(0, 4, 8),
        },
        Location {
            path: b.clone(),
            range: lsp_range(1, 0, 4),
        },
    ];
    surface.route_reply(LspReply::References(locs));
    assert!(surface.references.is_open(), "the references list opened");
    assert_eq!(
        surface.references.rows().len(),
        2,
        "both references are listed"
    );
    // Picking the second row jumps to b.rs line 1 (the same seam the overlay
    // pick drives).
    let row = surface.references.rows()[1].clone();
    surface.jump_to_location(&row.path, row.line0, row.char0);
    assert_eq!(
        surface.current_path(),
        Some(b.as_path()),
        "the pick opened b.rs"
    );
    let doc = surface.doc().expect("a doc");
    assert_eq!(
        doc.view.cursor(),
        doc.buffer.line_to_char(1),
        "the caret jumped to the reference's line"
    );
}

#[test]
fn rename_reply_applies_across_an_open_and_a_closed_file() {
    let d = TempDir::new("lsp3-rename");
    let open = d.join("open.rs");
    std::fs::write(&open, b"let foo = 1;\n").expect("write open");
    let closed = d.join("closed.rs");
    std::fs::write(&closed, b"use foo;\n").expect("write closed");
    let mut surface = real_editor();
    surface.open_path(&open).expect("open the open file");
    // Rename `foo` → `bar` in both files (chars 4..7 on line 0 of each).
    let edit = WorkspaceEdit {
        changes: vec![
            (
                open.clone(),
                vec![TextEdit {
                    range: lsp_range(0, 4, 7),
                    new_text: "bar".to_owned(),
                }],
            ),
            (
                closed.clone(),
                vec![TextEdit {
                    range: lsp_range(0, 4, 7),
                    new_text: "bar".to_owned(),
                }],
            ),
        ],
    };
    surface.route_reply(LspReply::Rename(edit));
    // The open file changed in its live buffer (undoable) ...
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "let bar = 1;\n"
    );
    // ... and the closed file was rewritten on disk.
    assert_eq!(
        std::fs::read_to_string(&closed).expect("read closed"),
        "use bar;\n"
    );
    assert_eq!(surface.lsp_notice.as_deref(), Some("Renamed 2 files"));
}

#[test]
fn format_reply_edits_the_focused_buffer() {
    let d = TempDir::new("lsp3-fmt");
    let src = d.join("src.rs");
    std::fs::write(&src, b"fn  f(){}\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&src).expect("open");
    // Collapse the double space (chars 2..4) to one.
    let edits = vec![TextEdit {
        range: lsp_range(0, 2, 4),
        new_text: " ".to_owned(),
    }];
    surface.route_reply(LspReply::Format(edits));
    assert_eq!(
        surface.doc().expect("doc").buffer.rope().to_string(),
        "fn f(){}\n"
    );
    assert_eq!(surface.lsp_notice.as_deref(), Some("Formatted"));
}

#[test]
fn navigation_without_a_server_is_an_honest_no_op() {
    // §7: the test build spawns no server, so the doc's client is absent —
    // every action honestly no-ops with a status, never a fake jump/edit.
    let d = TempDir::new("lsp3-noserver");
    let src = d.join("src.rs");
    std::fs::write(&src, b"fn main() {}\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&src).expect("open");
    surface.lsp_goto_definition();
    assert_eq!(surface.lsp_notice.as_deref(), Some("No language server"));
    surface.lsp_format_document();
    assert_eq!(surface.lsp_notice.as_deref(), Some("No language server"));
    // A rename never even opens its box without a server.
    surface.lsp_start_rename();
    assert!(
        !surface.rename.is_open(),
        "rename is a no-op without a server"
    );
    assert_eq!(surface.lsp_notice.as_deref(), Some("No language server"));
}

#[test]
fn the_references_overlay_and_rename_box_paint() {
    let d = TempDir::new("lsp3-paint");
    let a = d.join("a.rs");
    std::fs::write(&a, b"let x = 1;\n").expect("write");
    let mut surface = real_editor();
    surface.open_path(&a).expect("open");
    surface.route_reply(LspReply::References(vec![Location {
        path: a.clone(),
        range: lsp_range(0, 4, 5),
    }]));
    assert!(surface.references.is_open());
    assert!(
        tessellate_panel(&mut surface) > 0,
        "the references overlay paints"
    );
    surface.references.close();
    surface.rename.open_for(a.clone(), 0, 4, "x");
    assert!(tessellate_panel(&mut surface) > 0, "the rename box paints");
}

// ── EDITOR-11: save / autosave / external-change reload ──────────────────

#[test]
fn ctrl_s_saves_the_focused_buffer_and_clears_dirty() {
    let d = TempDir::new("save-ctrl-s");
    let file = d.join("s.txt");
    std::fs::write(&file, b"abc").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");

    let ctx = egui::Context::default();
    Style::install(&ctx);
    // A real typed frame dirties the buffer …
    run_frame(&ctx, &mut surface, vec![Event::Text("X".to_owned())]);
    assert!(
        surface.doc().expect("doc").buffer.is_dirty(),
        "the edit dirtied the buffer"
    );

    // … and Ctrl-S at the panel level writes it and clears dirty.
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::S, Modifiers::COMMAND)],
    );
    assert!(
        !surface.doc().expect("doc").buffer.is_dirty(),
        "Ctrl-S cleared the dirty state"
    );
    let on_disk = std::fs::read_to_string(&file).expect("read back");
    assert!(on_disk.contains('X'), "the edit reached disk: {on_disk:?}");
}

#[test]
fn ctrl_s_on_a_scratch_buffer_prompts_for_a_path() {
    let mut surface = real_editor();
    surface.open_scratch(); // pathless — nowhere to write yet
    let ctx = egui::Context::default();
    Style::install(&ctx);
    run_frame(
        &ctx,
        &mut surface,
        vec![key_press(Key::S, Modifiers::COMMAND)],
    );
    assert!(
        surface.save_as.open,
        "Ctrl-S on a pathless buffer opens the Save As prompt (honest, §7)"
    );
}

#[test]
fn autosave_writes_a_dirty_buffer_only_after_the_idle_window() {
    let d = TempDir::new("autosave-on");
    let file = d.join("a.txt");
    std::fs::write(&file, b"abc").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    surface.doc_mut().expect("doc").buffer.insert(3, "Z");
    surface.autosave.enabled = true;
    surface.autosave.idle_secs = 5.0;

    // The first tick observes the fresh edit → arms the idle timer, no write.
    surface.tick_autosave(100.0);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "abc",
        "not saved on the same tick the edit landed"
    );
    // Still inside the debounce window.
    surface.tick_autosave(103.0);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "abc",
        "debounced — the buffer is still settling"
    );
    // Past the window → the dirty, path-backed buffer is written.
    surface.tick_autosave(106.0);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "abcZ",
        "autosaved once idle past the window"
    );
    assert!(
        !surface.doc().expect("doc").buffer.is_dirty(),
        "the autosave cleared dirty"
    );
}

#[test]
fn autosave_disabled_never_writes_a_dirty_buffer() {
    let d = TempDir::new("autosave-off");
    let file = d.join("a.txt");
    std::fs::write(&file, b"abc").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    surface.doc_mut().expect("doc").buffer.insert(3, "Z");
    assert!(!surface.autosave.enabled, "autosave is off by default");

    surface.tick_autosave(100.0);
    surface.tick_autosave(500.0); // long past any window
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "abc",
        "autosave off → the file is never written"
    );
    assert!(
        surface.doc().expect("doc").buffer.is_dirty(),
        "the buffer stays dirty"
    );
}

#[test]
fn an_external_change_opens_the_reload_prompt() {
    let d = TempDir::new("reload-detect");
    let file = d.join("a.txt");
    std::fs::write(&file, b"abc").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");

    // A tool rewrites the file (a different length is caught regardless of
    // mtime granularity).
    std::fs::write(&file, b"abcdef").expect("external write");
    surface.poll_external_change(0.0);

    let prompt = surface.reload.prompt.as_ref().expect("a change is pending");
    assert_eq!(prompt.path, file, "the prompt names the changed file");
    assert!(!prompt.conflict, "a clean buffer is not a conflict");
}

#[test]
fn a_dirty_external_change_is_a_conflict_and_reload_takes_disk() {
    let d = TempDir::new("reload-conflict");
    let file = d.join("a.txt");
    std::fs::write(&file, b"abc").expect("write");
    let mut surface = real_editor();
    surface.open_path(&file).expect("open");
    // A local unsaved edit …
    surface.doc_mut().expect("doc").buffer.insert(3, "-mine");
    // … concurrent with an external rewrite.
    std::fs::write(&file, b"theirs\n").expect("external write");
    surface.poll_external_change(0.0);

    let path = {
        let prompt = surface.reload.prompt.as_ref().expect("pending");
        assert!(
            prompt.conflict,
            "unsaved edits + an external change is a conflict"
        );
        prompt.path.clone()
    };

    // Reload-theirs replaces the buffer with the on-disk copy, landing clean.
    surface.reload_focused(&path);
    let doc = surface.doc().expect("doc");
    assert_eq!(
        doc.buffer.rope().to_string(),
        "theirs\n",
        "reload took the disk copy"
    );
    assert!(!doc.buffer.is_dirty(), "the reloaded buffer is clean");
}

#[test]
fn autosave_prefs_round_trip_through_the_config_file() {
    let d = TempDir::new("autosave-cfg");
    let path = d.join("editor-egui.json");
    let prefs = super::AutosavePrefs {
        enabled: true,
        idle_secs: 3.5,
    };
    super::write_autosave_prefs_at(&path, prefs).expect("write prefs");
    let back = super::read_autosave_prefs_at(&path).expect("read prefs");
    assert!(back.enabled, "the enabled toggle persisted");
    assert!(
        (back.idle_secs - 3.5).abs() < f64::EPSILON,
        "the interval persisted"
    );
}

#[test]
fn a_hand_edited_zero_interval_is_clamped_on_load() {
    let d = TempDir::new("autosave-clamp");
    let path = d.join("editor-egui.json");
    std::fs::write(&path, br#"{"enabled":true,"idle_secs":0.0}"#).expect("seed");
    let prefs = super::read_autosave_prefs_at(&path).expect("read");
    assert!(
        prefs.idle_secs >= 0.2,
        "a hand-edited 0 interval is clamped to a sane floor"
    );
}

//! **Follow mode** (EDITOR-COLLAB-3) — the view half of following a
//! collaborator: replay the followed peer's presence onto the local editor
//! view, and the visible "Following …" affordance that ends it.
//!
//! The *protocol* half lives in [`crate::collab_session`]: the session decides
//! **whom** we follow ([`CollabSession::follow`]) and surfaces the followed
//! peer's fresh presence as [`PollOutcome::follow`]; it also owns every break
//! idiom (local edits, [`CollabSession::note_local_input`], the peer leaving).
//! This module is the **glue onto the widget** (§6 — nothing here re-implements
//! session or view logic):
//!
//! * [`apply_follow`] drives the local [`EditorView`] to track the leader: its
//!   cursor/selection lands through the widget's own
//!   [`EditorView::place_selection`] seam (which scroll-reveals exactly like a
//!   finder jump), and a scroll-only report (viewport, no cursor) lands the
//!   caret on the leader's topmost visible line. The view then *tracks* the
//!   peer — every fresh presence re-lands it — until something breaks follow.
//! * [`follow_banner`] is the standard affordance: a small accent pill naming
//!   the followed peer; clicking it stops following (the caller then calls
//!   [`CollabSession::unfollow`]). All colour/space/type comes from the shared
//!   [`Style`] tokens (§4).
//!
//! Like the rest of the COLLAB stack, this ships as a **tested library seam**:
//! the live mount (pumping a session per frame and feeding these two calls) is
//! the `panel.rs` share-session wiring unit COLLAB-2 already gates its live
//! smoke on — documented there, not faked here (§7).
//!
//! [`CollabSession::follow`]: crate::collab_session::CollabSession::follow
//! [`CollabSession::unfollow`]: crate::collab_session::CollabSession::unfollow
//! [`CollabSession::note_local_input`]: crate::collab_session::CollabSession::note_local_input
//! [`PollOutcome::follow`]: crate::collab_session::PollOutcome::follow

use mde_egui::egui::{self, RichText, Stroke};
use mde_egui::Style;

use crate::tooltip::editor_hover_text;

use crate::buffer::Buffer;
use crate::collab_session::FollowUpdate;
use crate::widget::EditorView;

/// Hairline width of the banner pill's accent outline (a drawn-widget
/// dimension, like the mesh-map's `NODE_STROKE_W`; colours are `Style` tokens).
const BANNER_STROKE_W: f32 = 1.0;

/// Replay one [`FollowUpdate`] onto the local view — the follower's screen
/// tracks the leader:
///
/// * a reported **cursor/selection** lands as the local selection (anchor →
///   head, clamped) through [`EditorView::place_selection`], scroll-revealed;
/// * a **viewport-only** report (the leader scrolled without moving its caret)
///   lands the caret at the start of the leader's topmost visible line
///   (clamped to the buffer), scroll-revealed;
/// * an empty report (neither field yet) moves nothing.
///
/// Returns whether the view moved. The caller keeps its own caret state out of
/// the way while following — any local gesture breaks follow at the session
/// (`note_local_input`), after which these calls stop arriving.
pub fn apply_follow(update: &FollowUpdate, view: &mut EditorView, buffer: &Buffer) -> bool {
    if let Some(cursor) = update.cursor {
        view.place_selection(buffer, cursor.anchor, cursor.head);
        return true;
    }
    if let Some(viewport) = update.viewport {
        let line = viewport
            .first_line
            .min(buffer.len_lines().saturating_sub(1));
        view.place_cursor(buffer, buffer.line_to_char(line));
        return true;
    }
    false
}

/// The visible **"Following <peer>"** affordance.
///
/// A small accent-outlined pill the surface shows while follow mode is active.
/// Returns `true` when the user clicks it to stop following (the caller then
/// calls
/// [`CollabSession::unfollow`](crate::collab_session::CollabSession::unfollow)
/// and drops the banner). Editing or any reported local gesture also breaks
/// follow at the session layer — the pill is the *discoverable* exit, not the
/// only one.
pub fn follow_banner(ui: &mut egui::Ui, name: &str) -> bool {
    let text = RichText::new(format!("Following {name} — click to stop"))
        .size(Style::SMALL)
        .color(Style::ACCENT_HI);
    let pill = egui::Button::new(text)
        .fill(Style::SURFACE_HI)
        .stroke(Stroke::new(BANNER_STROKE_W, Style::ACCENT));
    editor_hover_text(ui.add(pill), "Typing or editing also stops following").clicked()
}

#[cfg(test)]
mod tests {
    use super::{apply_follow, follow_banner};
    use crate::buffer::Buffer;
    use crate::collab_session::{CursorPos, FollowUpdate, Viewport};
    use crate::widget::EditorView;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    /// A follow update naming peer `p` with the given cursor/viewport.
    fn update(cursor: Option<CursorPos>, viewport: Option<Viewport>) -> FollowUpdate {
        FollowUpdate {
            peer: "p".to_string(),
            name: "Peer".to_string(),
            cursor,
            viewport,
        }
    }

    #[test]
    fn a_reported_selection_lands_as_the_local_selection() {
        let buffer = Buffer::from_text("hello\nworld\n");
        let mut view = EditorView::new();
        let moved = apply_follow(
            &update(Some(CursorPos { anchor: 1, head: 4 }), None),
            &mut view,
            &buffer,
        );
        assert!(moved);
        assert_eq!(view.cursor(), 4, "the caret lands on the leader's head");
        assert_eq!(view.selection(), Some(1..4), "the selection mirrors");
    }

    #[test]
    fn a_bare_caret_lands_without_a_selection_and_clamps() {
        let buffer = Buffer::from_text("short\n");
        let mut view = EditorView::new();
        assert!(apply_follow(
            &update(Some(CursorPos::caret(3)), None),
            &mut view,
            &buffer,
        ));
        assert_eq!(view.cursor(), 3);
        assert_eq!(view.selection(), None, "a caret carries no selection");

        // A peer position past this replica's end (edits still in flight)
        // clamps rather than panicking.
        assert!(apply_follow(
            &update(
                Some(CursorPos {
                    anchor: 2,
                    head: 999
                }),
                None
            ),
            &mut view,
            &buffer,
        ));
        assert_eq!(view.cursor(), buffer.len_chars(), "clamped to the end");
        assert_eq!(view.selection(), Some(2..buffer.len_chars()));
    }

    #[test]
    fn a_viewport_only_report_lands_on_the_leaders_top_line() {
        // The leader scrolled without moving its caret: the follower's caret
        // lands at the start of the leader's topmost visible line.
        let buffer = Buffer::from_text("one\ntwo\nthree\nfour\n");
        let mut view = EditorView::new();
        assert!(apply_follow(
            &update(
                None,
                Some(Viewport {
                    first_line: 2,
                    last_line: 3,
                })
            ),
            &mut view,
            &buffer,
        ));
        assert_eq!(view.cursor(), buffer.line_to_char(2));
        assert_eq!(view.selection(), None);

        // A viewport past this replica's end clamps to the last line.
        assert!(apply_follow(
            &update(
                None,
                Some(Viewport {
                    first_line: 999,
                    last_line: 1000,
                })
            ),
            &mut view,
            &buffer,
        ));
        assert_eq!(
            view.cursor(),
            buffer.line_to_char(buffer.len_lines() - 1),
            "clamped to the last line"
        );
    }

    #[test]
    fn an_empty_report_moves_nothing() {
        // A followed peer that has reported neither cursor nor viewport yet
        // must not yank the local view anywhere.
        let buffer = Buffer::from_text("stay\n");
        let mut view = EditorView::new();
        view.place_cursor(&buffer, 2);
        assert!(!apply_follow(&update(None, None), &mut view, &buffer));
        assert_eq!(view.cursor(), 2, "view untouched");
    }

    #[test]
    fn the_banner_renders_and_is_quiet_without_a_click() {
        // Headless frame → tessellate (the same CPU-only path the other egui
        // component tests drive): the affordance draws real primitives, and
        // with no input it reports no stop-click.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 120.0))),
            ..Default::default()
        };
        let mut clicked = false;
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                clicked = follow_banner(ui, "Ada");
            });
        });
        assert!(!clicked, "no interaction → still following");
        assert!(
            !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
            "the banner produced no draw primitives"
        );
    }
}

//! Clipboard mode — a space's cross-mesh clipboard lane (WL-FUNC-011).
//!
//! Renders the [`ClipboardLane`](mde_collab_types::ClipboardLane) projection: one
//! row per captured clip with its MIME kind (text vs. a shared URI), a preview
//! (shown where safe), the capturing node's attribution, and the SHA-256 content
//! address that de-duplicates the same clip across nodes. The worker folds the
//! existing cross-mesh clipboard captures (`event/clipboard/clip`) into
//! [`ClipboardPublished`](mde_collab_types::CollabEventKind::ClipboardPublished)
//! events; this mode also lets the seat **publish** a new clip and, per row,
//! **attach** it to the space, **pin/unpin** it, or **delete** it — each a typed
//! [`CollabCommand`].
//!
//! Arbitrary MIME up to 100 MB rides the clipboard lane; anything larger is a
//! Transfer, not a clip (the worker routes it there rather than truncating).

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    ClipItemKind, ClipboardItem, ClipboardView, CollabCommand, EventId, SpaceId,
};

use crate::files::short_hash;
use crate::{icons, relative_age, CommunicationsSurface};

/// The lane row preview cap — a clip's preview is a recognisable head, never the
/// full (possibly large) content pasted into the row.
const PREVIEW_MAX: usize = 160;

impl CommunicationsSurface {
    /// Render Clipboard mode for the selected space: the publish composer, then
    /// the newest-first lane with per-clip pin/attach/delete controls.
    pub(crate) fn clipboard_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        let Some(space) = self.selected_space() else {
            ui.label(
                egui::RichText::new("Select a space to see its clipboard lane.")
                    .color(Style::TEXT_DIM),
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Clipboard")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new("shared across the mesh")
                    .small()
                    .color(Style::TEXT_DIM),
            );
        });
        ui.separator();

        self.clip_publish_composer(ui, sink, space, data.me().as_str());
        ui.separator();

        match data.clipboard_lane(space) {
            Some(lane) if !lane.items.is_empty() => {
                let now = data.now_unix_ms();
                egui::ScrollArea::vertical()
                    .id_salt("collab-clipboard")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for item in &lane.items {
                            self.clip_row(ui, sink, space, item, now);
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
            _ => {
                ui.label(
                    egui::RichText::new("No clips shared in this space yet.")
                        .color(Style::TEXT_DIM),
                );
                ui.label(
                    egui::RichText::new(
                        "Copy on any mesh node and it lands here — or publish one below.",
                    )
                    .small()
                    .color(Style::TEXT_DIM),
                );
            }
        }
    }

    /// The publish composer: type text, press <kbd>Enter</kbd> (or the publish
    /// glyph) to emit [`PublishClipboard`](CollabCommand::PublishClipboard) with a
    /// clip carrying the seat's attribution + the real SHA-256 content address.
    fn clip_publish_composer(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        me: &str,
    ) {
        let mut buf = self.clip_drafts.get(&space).cloned().unwrap_or_default();
        let mut publish = false;
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut buf)
                    .id(egui::Id::new(("mde-collab-clip-composer", space.as_uuid())))
                    .desired_width(f32::INFINITY)
                    .hint_text("Publish a clip  ·  Enter to share"),
            );
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (resp.lost_focus() || resp.has_focus()) && enter {
                publish = true;
            }
            if icons::icon_button(
                ui,
                icons::CLIP_PUBLISH,
                Style::SP_M,
                Style::ACCENT,
                "Publish clip",
            )
            .clicked()
            {
                publish = true;
            }
        });
        let text = buf.trim().to_owned();
        if publish && !text.is_empty() {
            self.publish_clip_text(sink, space, &text, me);
            buf.clear();
        }
        self.clip_drafts.insert(space, buf);
    }

    /// One clipboard-lane row: the kind glyph, the preview, the source + content
    /// address facts, and the pin/attach/delete controls.
    fn clip_row(
        &self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        item: &ClipboardView,
        now_unix_ms: i64,
    ) {
        mde_egui::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(
                    ui,
                    icons::clip_kind_icon(item.kind),
                    Style::SP_M,
                    Style::ACCENT,
                );
                ui.label(egui::RichText::new(clip_preview(&item.preview)).color(Style::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::icon_button(
                        ui,
                        icons::CLIP_DELETE,
                        Style::SP_M,
                        Style::DANGER,
                        "Delete clip",
                    )
                    .clicked()
                    {
                        self.delete_clip(sink, space, item.event_id);
                    }
                    if icons::icon_button(
                        ui,
                        icons::CLIP_ATTACH,
                        Style::SP_M,
                        Style::TEXT_DIM,
                        "Attach to a message in this space",
                    )
                    .clicked()
                    {
                        self.attach_clip(sink, space, item.event_id);
                    }
                    // Pin toggle — a pinned clip survives the cap + clear.
                    let (tint, hint) = if item.pinned {
                        (Style::WARN, "Unpin")
                    } else {
                        (Style::TEXT_DIM, "Pin")
                    };
                    if icons::icon_button(ui, icons::CLIP_PIN, Style::SP_M, tint, hint).clicked() {
                        self.toggle_clip_pin(sink, space, item.event_id, item.pinned);
                    }
                });
            });

            // Honest facts: who captured it, when, and its content address.
            ui.label(
                egui::RichText::new(format!(
                    "{}  ·  {}  ·  content {}",
                    item.source,
                    relative_age(now_unix_ms, item.at_unix_ms),
                    short_hash(&item.sha256_hex),
                ))
                .small()
                .color(Style::TEXT_DIM),
            );
        });
    }

    // ── testable command seams (the UI above drives these same methods) ──────

    /// Build a [`ClipboardItem`] from `text` (detecting a URI, hashing the real
    /// content address, attributing it to `source`) and emit
    /// [`PublishClipboard`](CollabCommand::PublishClipboard) into `space`.
    pub(crate) fn publish_clip_text(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        text: &str,
        source: &str,
    ) {
        let item = ClipboardItem {
            kind: detect_kind(text),
            preview: clip_preview(text),
            sha256_hex: mde_collab_types::value::sha256_hex(text.as_bytes()),
            len: text.len() as u64,
            source: source.to_owned(),
        };
        sink.emit(CollabCommand::PublishClipboard { space, item });
    }

    /// Emit [`AttachClipboard`](CollabCommand::AttachClipboard) — re-share `clip`
    /// as a message in `space`.
    pub(crate) fn attach_clip(&self, sink: &mut crate::CommandSink, space: SpaceId, clip: EventId) {
        sink.emit(CollabCommand::AttachClipboard { space, clip });
    }

    /// Emit [`PinClipboard`](CollabCommand::PinClipboard) or
    /// [`UnpinClipboard`](CollabCommand::UnpinClipboard) depending on the current
    /// pin state.
    pub(crate) fn toggle_clip_pin(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        clip: EventId,
        pinned: bool,
    ) {
        if pinned {
            sink.emit(CollabCommand::UnpinClipboard { space, clip });
        } else {
            sink.emit(CollabCommand::PinClipboard { space, clip });
        }
    }

    /// Emit [`DeleteClipboard`](CollabCommand::DeleteClipboard) — remove a single
    /// clip from the lane.
    pub(crate) fn delete_clip(&self, sink: &mut crate::CommandSink, space: SpaceId, clip: EventId) {
        sink.emit(CollabCommand::DeleteClipboard { space, clip });
    }
}

/// Classify a clip's content: an `http(s)://` head is a shared URI, everything
/// else is text (an honest, conservative guess — never a faked MIME).
fn detect_kind(text: &str) -> ClipItemKind {
    let t = text.trim_start();
    if t.starts_with("http://") || t.starts_with("https://") {
        ClipItemKind::Uri
    } else {
        ClipItemKind::Text
    }
}

/// A single-line, capped preview of clip content (the row shows a recognisable
/// head, never the full possibly-large payload).
fn clip_preview(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > PREVIEW_MAX {
        let head: String = one_line.chars().take(PREVIEW_MAX).collect();
        format!("{head}\u{2026}")
    } else {
        one_line
    }
}

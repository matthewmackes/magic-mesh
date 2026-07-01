//! The Clipboard surface — tails `event/clipboard/clip` and shows recent
//! mesh clipboard entries newest-first.
//!
//! The clipboard_sync mackesd worker publishes each captured clip to
//! `event/clipboard/clip` as a JSON body with `id`, `text`, `source`, `time`.
//! This module reads that topic incrementally via the Bus cursor API — the
//! same pattern the Fleet plane uses for `event/kvm/services` — so it never
//! depends on the mackesd crate (§6 mesh/desktop boundary). The full
//! pin-and-history management stays in the `action/clipboard/*` IPC layer;
//! this panel is read-only.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use mde_bus::persist::Persist;
use mde_egui::egui::{self, Align, Layout, RichText, ScrollArea};
use mde_egui::Style;
use serde::Deserialize;

const REFRESH: Duration = Duration::from_secs(5);

/// The bus topic the clipboard_sync worker publishes to on every captured clip.
const CLIP_TOPIC: &str = "event/clipboard/clip";

/// Local mirror of the clipboard entry shape — a JSON boundary so the shell
/// does not depend on mackesd directly (§6). Field names match the
/// `CLIP-VIEW-1 contract` test in `ipc::clipboard`.
#[derive(Debug, Clone, Deserialize)]
struct ClipEntry {
    text: String,
    source: String,
    time: String,
    #[serde(default)]
    pinned: bool,
}

/// The Clipboard surface state: an incremental cursor into
/// `event/clipboard/clip` and the accumulated entries newest-first.
pub(crate) struct ClipboardState {
    bus_root: Option<PathBuf>,
    /// Newest-first list of entries seen since the shell started.
    entries: Vec<ClipEntry>,
    /// Bus ULID cursor for `list_since` — advances on each poll.
    cursor: Option<String>,
    last_poll: Option<Instant>,
}

impl Default for ClipboardState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            entries: Vec::new(),
            cursor: None,
            last_poll: None,
        }
    }
}

impl ClipboardState {
    /// Poll the bus on the shared cadence and keep the repaint heartbeat alive.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    fn refresh(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let msgs = match persist.list_since(CLIP_TOPIC, self.cursor.as_deref()) {
            Ok(m) => m,
            Err(_) => return,
        };
        for msg in msgs {
            self.cursor = Some(msg.ulid.clone());
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            let Ok(entry) = serde_json::from_str::<ClipEntry>(body) else {
                continue;
            };
            self.entries.insert(0, entry);
        }
    }

    /// Render the clipboard history into `ui`.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        if self.entries.is_empty() {
            let (title, subtitle) = empty_copy(self.bus_root.is_some());
            crate::session::empty_state(ui, title, subtitle);
            return;
        }

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &self.entries {
                    ui.group(|ui| {
                        let preview = truncate_clip(&entry.text);
                        ui.label(RichText::new(preview).color(Style::TEXT).size(Style::BODY));
                        ui.horizontal(|ui| {
                            mde_egui::muted_note(ui, &entry.source);
                            if entry.pinned {
                                ui.label(
                                    RichText::new("pinned")
                                        .color(Style::ACCENT)
                                        .size(Style::SMALL),
                                );
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                mde_egui::muted_note(ui, &entry.time);
                            });
                        });
                    });
                    ui.add_space(Style::SP_S);
                }
            });
    }
}

/// The empty-panel copy — honest about *why* nothing is listed. With no mesh Bus
/// directory the clip topic is unreadable (a gated read), which must not render
/// as a live-looking "empty history" (§7).
const fn empty_copy(has_bus: bool) -> (&'static str, &'static str) {
    if has_bus {
        (
            "Clipboard history is empty",
            "Text you copy appears here once the clipboard_sync worker captures it to the mesh Bus.",
        )
    } else {
        (
            "Clipboard history unavailable",
            "No mesh Bus directory on this node, so event/clipboard/clip can't be read — joining the mesh (the mde-bus spool) unblocks this panel.",
        )
    }
}

/// Return the first line of `text`, capped at 120 chars. Keeps entries legible
/// without the panel scrolling horizontally on multi-line pastes.
fn truncate_clip(text: &str) -> &str {
    let line = text.lines().next().unwrap_or(text);
    if line.len() <= 120 {
        return line;
    }
    // Snap back to the nearest char boundary at or before the cap.
    let mut end = 120;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::{empty_copy, truncate_clip};

    #[test]
    fn empty_copy_distinguishes_a_missing_bus_from_an_empty_history() {
        // An empty history reads as "nothing copied yet"; a missing Bus must NOT
        // (§7 — a gated read never renders as a live-looking empty state).
        let (title, _) = empty_copy(true);
        assert_eq!(title, "Clipboard history is empty");
        let (title, subtitle) = empty_copy(false);
        assert_eq!(title, "Clipboard history unavailable");
        assert!(
            subtitle.contains("Bus") && subtitle.contains("unblocks"),
            "the gated copy names what's missing and what unblocks it: {subtitle}"
        );
    }

    #[test]
    fn truncate_returns_first_line() {
        assert_eq!(truncate_clip("hello\nworld"), "hello");
    }

    #[test]
    fn truncate_caps_at_120_chars_on_char_boundary() {
        let long = "a".repeat(200);
        let got = truncate_clip(&long);
        assert_eq!(got.len(), 120);
    }

    #[test]
    fn truncate_short_text_is_unchanged() {
        assert_eq!(truncate_clip("short"), "short");
    }
}

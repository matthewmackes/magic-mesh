//! TERM-8 — the "new terminal on → <peer>" picker + manual host entry.
//!
//! A small overlay panel over the terminal surface: the mesh roster
//! ([`crate::roster`]) as a list of peers with presence pips (offline greyed +
//! unpickable, no hang), plus a manual host / overlay-address field for a peer
//! not in the roster. Either path yields a [`RemoteTarget`] the surface opens a
//! remote pane on (routing through the TERM-7 broker via [`crate::remote`]).
//!
//! §4: every colour here is a `Style` token (the presence pips map through
//! [`presence_pip`], the same token mapping the Chat roster uses). The pure
//! target-parsing + filtering folds are unit-tested; the interactive panel is the
//! thin egui shell over them.

use std::time::{Duration, Instant};

use mde_egui::egui::{
    self, Align2, Area, Color32, Key, Order, RichText, ScrollArea, Sense, Stroke, StrokeKind, Vec2,
};
use mde_egui::Style;

use crate::roster::{Presence, RosterClient, RosterSnapshot};

/// How often the picker re-reads the roster while open (a cheap local scan, but
/// not every frame).
const REFRESH_INTERVAL: Duration = Duration::from_millis(750);

/// The picker panel's fixed width.
const PANEL_WIDTH: f32 = 320.0;
/// The peer list's max height before it scrolls.
const LIST_MAX_H: f32 = 240.0;
/// The presence pip radius.
const PIP_RADIUS: f32 = 4.0;

/// A chosen remote target: the mesh peer short-name (the `action/pty/<peer>` verb
/// slot) and a display label for the pane's node marker.
///
/// Serde-serializable so a saved layout (TERM-10) records a remote pane's target
/// node and, on launch, feeds this exact struct back into the remote-open path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteTarget {
    /// The mesh peer short-name — the broker verb slot + the ssh `<peer>.mesh` host.
    pub peer: String,
    /// The label shown on the pane's node marker.
    pub label: String,
}

/// Parse a manual host / overlay-address entry into a [`RemoteTarget`].
///
/// Accepts a bare mesh host (`oak`), a `user@host` form (the broker uses its own
/// fixed mesh user, so the user part is dropped), and a trailing `.mesh` (the
/// broker appends the mesh suffix itself). Rejects empties and anything with a
/// topic separator / whitespace so the minted `action/pty/<peer>` topic stays
/// valid. Pure, so the acceptance ("manual host entry routes through TERM-7") is
/// asserted without a UI.
#[must_use]
pub fn manual_target(input: &str) -> Option<RemoteTarget> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    // Drop any `user@` prefix, then a `.mesh` suffix — the broker owns both.
    let host = raw.rsplit('@').next().unwrap_or(raw);
    let host = host.strip_suffix(".mesh").unwrap_or(host).trim();
    if host.is_empty() || host.contains('/') || host.chars().any(char::is_whitespace) {
        return None;
    }
    Some(RemoteTarget {
        peer: host.to_string(),
        label: host.to_string(),
    })
}

/// Map a presence to its pip colour (§4 — no raw hex; the same token mapping the
/// Chat roster uses).
const fn presence_pip(p: Presence) -> Color32 {
    match p {
        Presence::Online | Presence::FreeForChat => Style::OK,
        Presence::Away | Presence::ManualAway => Style::WARN,
        Presence::Dnd => Style::DANGER,
        Presence::Offline | Presence::Invisible => Style::TEXT_DIM,
    }
}

/// The remote-terminal picker: open/closed state, the filter + manual buffers,
/// and the throttled roster snapshot.
#[derive(Default)]
pub struct RemotePicker {
    open: bool,
    filter: String,
    manual: String,
    snapshot: Option<RosterSnapshot>,
    last_refresh: Option<Instant>,
}

impl RemotePicker {
    /// A fresh, closed picker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the picker is currently shown.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the picker (forcing a roster refresh on the next frame).
    pub const fn open(&mut self) {
        self.open = true;
        self.last_refresh = None;
    }

    /// Close the picker.
    pub const fn close(&mut self) {
        self.open = false;
    }

    /// Toggle the picker open/closed (the tab-bar remote button).
    pub const fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Re-read the roster if the throttle has elapsed.
    fn refresh(&mut self, roster: &dyn RosterClient) {
        let now = Instant::now();
        let due = self
            .last_refresh
            .is_none_or(|last| now.duration_since(last) >= REFRESH_INTERVAL);
        if due {
            self.snapshot = roster.snapshot();
            self.last_refresh = Some(now);
        }
    }

    /// Render the picker overlay and return a chosen [`RemoteTarget`], if any. A
    /// no-op returning `None` while closed. Escape (or the Cancel button) closes
    /// it; a pick closes it too.
    pub fn show(&mut self, ctx: &egui::Context, roster: &dyn RosterClient) -> Option<RemoteTarget> {
        if !self.open {
            return None;
        }
        self.refresh(roster);
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.close();
            return None;
        }

        let mut picked = None;
        Area::new(egui::Id::new("term-remote-picker"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, Style::SP_XL))
            .show(ctx, |ui| {
                // The panel background is painted *behind* the content: reserve
                // two shape slots now, lay the content, then set them to the
                // content-sized plate + border (the codebase's immediate-mode
                // idiom — same tokens as the quicklook card). §4.
                let margin = Style::SP_M;
                let bg = ui.painter().add(egui::Shape::Noop);
                let border = ui.painter().add(egui::Shape::Noop);

                let start = ui.min_rect().min + Vec2::splat(margin);
                let mut content = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_min_size(
                            start,
                            Vec2::new(PANEL_WIDTH, ui.available_height()),
                        ))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                content.set_width(PANEL_WIDTH);
                picked = self.panel(&mut content);

                let plate = content.min_rect().expand(margin);
                ui.painter().set(
                    bg,
                    egui::Shape::rect_filled(plate, Style::RADIUS, Style::SURFACE),
                );
                ui.painter().set(
                    border,
                    egui::Shape::rect_stroke(
                        plate,
                        Style::RADIUS,
                        Stroke::new(1.0, Style::BORDER),
                        StrokeKind::Inside,
                    ),
                );
                ui.allocate_rect(plate, Sense::hover());
            });
        if picked.is_some() {
            self.close();
        }
        picked
    }

    /// The panel body: header, filter, peer list, manual entry.
    fn panel(&mut self, ui: &mut egui::Ui) -> Option<RemoteTarget> {
        ui.label(
            RichText::new("New terminal on a mesh node")
                .color(Style::TEXT)
                .strong(),
        );
        ui.add_space(Style::SP_XS);

        // Filter.
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("filter peers\u{2026}")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(Style::SP_XS);

        // The peer list (offline greyed + unpickable).
        let mut picked = self.peer_list(ui);

        ui.add_space(Style::SP_S);
        Self::hairline(ui);
        ui.add_space(Style::SP_S);

        // Manual host / overlay-address entry.
        ui.label(
            RichText::new("Or a host directly")
                .color(Style::TEXT_DIM)
                .small(),
        );
        ui.add_space(Style::SP_XS);
        let manual = ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.manual)
                    .hint_text("host or user@host")
                    .desired_width(PANEL_WIDTH - 96.0),
            );
            let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let clicked = ui.button("Connect").clicked();
            (entered || clicked)
                .then(|| manual_target(&self.manual))
                .flatten()
        });
        if let Some(target) = manual.inner {
            self.manual.clear();
            picked = Some(target);
        }

        ui.add_space(Style::SP_S);
        if ui.button("Cancel").clicked() {
            self.close();
        }
        picked
    }

    /// The scrollable roster list; a reachable row is a button, an offline row a
    /// greyed, unpickable label. Returns the picked target, if any.
    fn peer_list(&self, ui: &mut egui::Ui) -> Option<RemoteTarget> {
        let Some(snapshot) = &self.snapshot else {
            ui.label(
                RichText::new("No mesh roster yet \u{2014} use a host below.")
                    .color(Style::TEXT_DIM)
                    .italics(),
            );
            return None;
        };
        let peers = snapshot.matching(&self.filter);
        if peers.is_empty() {
            let msg = if snapshot.is_solo() {
                "No other mesh peers \u{2014} use a host below."
            } else {
                "No peers match the filter."
            };
            ui.label(RichText::new(msg).color(Style::TEXT_DIM).italics());
            return None;
        }

        let mut picked = None;
        ScrollArea::vertical()
            .max_height(LIST_MAX_H)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for peer in peers {
                    ui.horizontal(|ui| {
                        // Presence pip (§4 token).
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::splat(PIP_RADIUS * 2.5), Sense::hover());
                        ui.painter().circle_filled(
                            rect.center(),
                            PIP_RADIUS,
                            presence_pip(peer.presence),
                        );
                        if peer.is_reachable() {
                            if ui
                                .add(egui::Button::new(
                                    RichText::new(peer.display.as_str()).color(Style::TEXT),
                                ))
                                .clicked()
                            {
                                picked = Some(RemoteTarget {
                                    peer: peer.host.clone(),
                                    label: peer.display.clone(),
                                });
                            }
                        } else {
                            // Offline: greyed + unpickable.
                            ui.add_enabled(
                                false,
                                egui::Button::new(
                                    RichText::new(peer.display.as_str()).color(Style::TEXT_DIM),
                                ),
                            );
                        }
                        ui.label(
                            RichText::new(peer.presence.label())
                                .small()
                                .color(Style::TEXT_DIM),
                        );
                    });
                }
            });
        picked
    }

    /// A one-pixel hairline separator in the border token.
    fn hairline(ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, Style::BORDER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_entry_parses_bare_user_and_mesh_forms() {
        assert_eq!(
            manual_target("oak"),
            Some(RemoteTarget {
                peer: "oak".into(),
                label: "oak".into()
            })
        );
        // A user@ prefix is dropped (the broker uses its fixed mesh user).
        assert_eq!(manual_target("root@cedar").expect("cedar").peer, "cedar");
        // A trailing .mesh is stripped (the broker appends the suffix).
        assert_eq!(manual_target("birch.mesh").expect("birch").peer, "birch");
        assert_eq!(manual_target("  anvil  ").expect("anvil").peer, "anvil");
    }

    #[test]
    fn manual_entry_rejects_empty_and_unsafe_input() {
        assert!(manual_target("").is_none());
        assert!(manual_target("   ").is_none());
        // A topic separator or whitespace would break the action topic.
        assert!(manual_target("a/b").is_none());
        assert!(manual_target("two hosts").is_none());
        assert!(manual_target("root@").is_none());
    }

    #[test]
    fn a_fresh_picker_is_closed_and_toggles() {
        let mut picker = RemotePicker::new();
        assert!(!picker.is_open());
        picker.toggle();
        assert!(picker.is_open());
        picker.toggle();
        assert!(!picker.is_open());
    }
}

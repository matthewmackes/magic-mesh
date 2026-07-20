//! TERM-10 — the saved-layouts overlay: save the current arrangement under a
//! name, and launch any layout the mesh has synced here.
//!
//! A small overlay panel over the terminal surface (the same Area/plate idiom as
//! the TERM-8 remote picker): the mesh-synced [`LayoutStore`] listing as launch
//! buttons — each layout labelled with its tab count + origin node — plus a
//! "save this arrangement as…" field. It is the thin egui shell over the pure
//! [`crate::layout`] model + store; the capture (surface → layout) and launch
//! (layout → surface) live on [`crate::TabbedTerminal`], which owns this manager
//! and feeds an emitted [`LayoutIntent`] back into itself.
//!
//! §4: every colour is a `Style` token — no raw hex — matching the remote picker.

use std::time::{Duration, Instant};

use mde_egui::egui::{
    self, Align2, Area, Key, Order, RichText, ScrollArea, Sense, StrokeKind, Vec2,
};
use mde_egui::Style;

use crate::layout::{LayoutStore, SavedLayout};
use crate::tooltip::terminal_hover_text;

/// How often the overlay re-reads the synced store while open (a cheap local dir
/// scan, but not every frame — the same throttle the remote picker uses).
const REFRESH_INTERVAL: Duration = Duration::from_millis(750);

/// How long an honest save/launch outcome stays on the overlay's status line.
const STATUS_TTL: Duration = Duration::from_secs(5);

/// The overlay panel's fixed width.
const PANEL_WIDTH: f32 = 340.0;
/// The layout list's max height before it scrolls.
const LIST_MAX_H: f32 = 240.0;

/// What the overlay asks the surface to do this frame — handled by the surface
/// (which owns the capture + launch), since only it can read/rebuild its panes.
pub enum LayoutIntent {
    /// Capture the current surface and persist it under this name.
    Save(String),
    /// Launch (rebuild) this saved layout.
    Launch(SavedLayout),
}

/// The saved-layouts overlay: the synced store, open state, the save-as buffer,
/// a throttled listing snapshot, and the honest last-outcome status line.
pub struct LayoutManager {
    store: LayoutStore,
    open: bool,
    name: String,
    snapshot: Vec<SavedLayout>,
    last_refresh: Option<Instant>,
    status: Option<(String, Instant)>,
}

impl LayoutManager {
    /// A fresh, closed overlay over `store`.
    #[must_use]
    pub fn new(store: LayoutStore) -> Self {
        Self {
            store,
            open: false,
            name: String::new(),
            snapshot: Vec::new(),
            last_refresh: None,
            status: None,
        }
    }

    /// The production overlay — the shared workgroup-root store for this node.
    #[must_use]
    pub fn local() -> Self {
        Self::new(LayoutStore::local())
    }

    /// Whether the overlay is currently shown.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the overlay (forcing a store re-read on the next frame).
    pub const fn open(&mut self) {
        self.open = true;
        self.last_refresh = None;
    }

    /// Close the overlay.
    pub const fn close(&mut self) {
        self.open = false;
    }

    /// Toggle the overlay open/closed (the tab-bar button / `Ctrl+Shift+L`).
    pub const fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Persist `layout` through the synced store, refresh the listing, and set the
    /// honest outcome — the surface calls this after capturing a [`LayoutIntent::Save`].
    pub fn persist(&mut self, layout: &SavedLayout) {
        self.status = Some((
            match self.store.save(layout) {
                Ok(_) => format!("Saved \u{201c}{}\u{201d}.", layout.name),
                Err(e) => format!("Could not save \u{201c}{}\u{201d}: {e}", layout.name),
            },
            Instant::now(),
        ));
        self.refresh_now();
    }

    /// Set the honest outcome of launching a layout — the surface calls this after
    /// a [`LayoutIntent::Launch`].
    pub fn note_launch(&mut self, name: &str, added: &std::io::Result<usize>) {
        self.status = Some((
            match added {
                Ok(n) => format!("Launched \u{201c}{name}\u{201d} ({n} tab(s))."),
                Err(e) => format!("Could not launch \u{201c}{name}\u{201d}: {e}"),
            },
            Instant::now(),
        ));
    }

    /// Force a store re-read now (after a save).
    pub fn refresh_now(&mut self) {
        self.snapshot = self.store.list();
        self.last_refresh = Some(Instant::now());
    }

    /// Re-read the store if the throttle has elapsed.
    fn refresh(&mut self) {
        let now = Instant::now();
        let due = self
            .last_refresh
            .is_none_or(|last| now.duration_since(last) >= REFRESH_INTERVAL);
        if due {
            self.snapshot = self.store.list();
            self.last_refresh = Some(now);
        }
    }

    /// Render the overlay and return an intent for the surface to act on, if any.
    /// A no-op returning `None` while closed; Escape (or Cancel) closes it.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<LayoutIntent> {
        if !self.open {
            return None;
        }
        self.refresh();
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.close();
            return None;
        }

        let mut intent = None;
        Area::new(egui::Id::new("term-layout-overlay"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, Style::SP_XL))
            .show(ctx, |ui| {
                // The first reserved slot is the shared Overlay-elevation shadow,
                // painted behind the plate so this floating popover reads as
                // lifted off the grid (same card idiom as the remote picker). §4.
                let margin = Style::SP_M;
                let shadow = ui.painter().add(egui::Shape::Noop);
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
                intent = self.panel(&mut content);

                let plate = content.min_rect().expand(margin);
                ui.painter()
                    .set(shadow, crate::overlay::overlay_shadow(plate));
                ui.painter().set(
                    bg,
                    egui::Shape::rect_filled(plate, Style::RADIUS, Style::SURFACE),
                );
                ui.painter().set(
                    border,
                    egui::Shape::rect_stroke(
                        plate,
                        Style::RADIUS,
                        Style::hairline(),
                        StrokeKind::Inside,
                    ),
                );
                ui.allocate_rect(plate, Sense::hover());
            });
        // Launching swaps the surface out from under the overlay — close it.
        if matches!(intent, Some(LayoutIntent::Launch(_))) {
            self.close();
        }
        intent
    }

    /// The panel body: header, the saved-layout list, the save-as row, status.
    fn panel(&mut self, ui: &mut egui::Ui) -> Option<LayoutIntent> {
        ui.label(RichText::new("Saved layouts").color(Style::TEXT).strong());
        ui.add_space(Style::SP_XS);

        let mut intent = self.layout_list(ui);

        ui.add_space(Style::SP_S);
        Self::hairline(ui);
        ui.add_space(Style::SP_S);

        // Save the current arrangement.
        ui.label(
            RichText::new("Save this arrangement")
                .color(Style::TEXT_DIM)
                .small(),
        );
        ui.add_space(Style::SP_XS);
        let saved = ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .hint_text("layout name\u{2026}")
                    .desired_width(PANEL_WIDTH - 96.0),
            );
            let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let clicked = ui.button("Save").clicked();
            let name = self.name.trim().to_string();
            ((entered || clicked) && !name.is_empty()).then_some(name)
        });
        if let Some(name) = saved.inner {
            self.name.clear();
            intent = Some(LayoutIntent::Save(name));
        }

        if let Some((msg, since)) = &self.status {
            if since.elapsed() < STATUS_TTL {
                ui.add_space(Style::SP_XS);
                ui.label(RichText::new(msg.as_str()).small().color(Style::TEXT_DIM));
            }
        }

        ui.add_space(Style::SP_S);
        if ui.button("Close").clicked() {
            self.close();
        }
        intent
    }

    /// The scrollable list of synced layouts; each row a launch button plus a
    /// dim "N tab(s) · saved on <origin>" subtitle. Returns a launch intent, if any.
    fn layout_list(&self, ui: &mut egui::Ui) -> Option<LayoutIntent> {
        if self.snapshot.is_empty() {
            ui.label(
                RichText::new("No saved layouts yet \u{2014} save one below.")
                    .color(Style::TEXT_DIM)
                    .italics(),
            );
            return None;
        }

        let mut intent = None;
        ScrollArea::vertical()
            .max_height(LIST_MAX_H)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for layout in &self.snapshot {
                    ui.horizontal(|ui| {
                        if terminal_hover_text(
                            ui.add(egui::Button::new(
                                RichText::new(layout.name.as_str()).color(Style::TEXT),
                            )),
                            "Launch this layout",
                        )
                        .clicked()
                        {
                            intent = Some(LayoutIntent::Launch(layout.clone()));
                        }
                        let panes: usize = layout.tabs.iter().map(|t| t.root.pane_count()).sum();
                        ui.label(
                            RichText::new(format!(
                                "{} tab(s), {panes} pane(s) \u{00b7} {}",
                                layout.tabs.len(),
                                layout.origin
                            ))
                            .small()
                            .color(Style::TEXT_DIM),
                        );
                    });
                }
            });
        intent
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
    fn a_fresh_overlay_is_closed_and_toggles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut mgr = LayoutManager::new(LayoutStore::new(dir.path(), "nodeA"));
        assert!(!mgr.is_open());
        mgr.toggle();
        assert!(mgr.is_open());
        mgr.toggle();
        assert!(!mgr.is_open());
    }

    #[test]
    fn refresh_sees_a_layout_saved_through_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LayoutStore::new(dir.path(), "nodeA");
        let mut mgr = LayoutManager::new(LayoutStore::new(dir.path(), "nodeA"));
        // Nothing yet.
        mgr.refresh_now();
        assert!(mgr.snapshot.is_empty());
        // Persist one (through the manager's own store) and it appears.
        let layout = SavedLayout {
            name: "Setup".into(),
            origin: "nodeA".into(),
            tabs: Vec::new(),
            active: 0,
        };
        store.save(&layout).expect("seed a layout on disk");
        mgr.refresh_now();
        assert_eq!(mgr.snapshot.len(), 1);
        assert_eq!(mgr.snapshot[0].name, "Setup");
    }
}

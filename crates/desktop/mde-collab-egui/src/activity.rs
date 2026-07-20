//! Activity mode — the space's action-oriented chronological feed from the
//! [`ActivityFeed`](mde_collab_types::ActivityFeed) projection, with band
//! filters. There is deliberately **no** competing global search box here
//! (spec §2): the rail is the space selector and the chips are the only filter.

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::ActivityEntry;

use crate::{icons, relative_age, ActivityFilter, CommunicationsSurface};

impl CommunicationsSurface {
    /// Render the Activity feed for the selected space: a row of band-filter
    /// chips, then the chronological entries the active filter admits.
    pub(crate) fn activity_body(&mut self, ui: &mut egui::Ui, data: &dyn crate::CollabData) {
        self.activity_filter_chips(ui);
        ui.add_space(Style::SP_S);
        ui.separator();
        ui.add_space(Style::SP_S);

        let feed = data.activity(self.selected_space());
        let entries: &[ActivityEntry] = feed.map_or(&[], |f| f.entries.as_slice());
        let filter = self.activity_filter();
        let now = data.now_unix_ms();

        let admitted = entries
            .iter()
            .filter(|e| filter.matches(&e.kind_tag))
            .count();
        if admitted == 0 {
            ui.label(
                egui::RichText::new("No activity for this filter yet.").color(Style::TEXT_DIM),
            );
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in entries.iter().filter(|e| filter.matches(&e.kind_tag)) {
                    activity_row(ui, entry, now);
                }
            });
    }

    /// The band-filter chip row (`All`, `Messages`, `Alerts`, `Calls`, `Files`,
    /// `People`). A chip carries a Carbon glyph when the band has a faithful one.
    fn activity_filter_chips(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for filter in ActivityFilter::ALL {
                let selected = self.activity_filter() == filter;
                if let Some(glyph) = icons::activity_filter_icon(filter) {
                    let tint = if selected {
                        Style::ACCENT
                    } else {
                        Style::TEXT_DIM
                    };
                    icons::icon(ui, glyph, Style::SP_M, tint);
                }
                if ui.selectable_label(selected, filter.label()).clicked() {
                    self.activity_filter = filter;
                }
                ui.add_space(Style::SP_XS);
            }
        });
    }
}

/// One Activity row: a band glyph, the actor, the projected summary line, and a
/// right-aligned relative age.
fn activity_row(ui: &mut egui::Ui, entry: &ActivityEntry, now_unix_ms: i64) {
    ui.horizontal(|ui| {
        icons::icon(
            ui,
            entry_icon(&entry.kind_tag),
            Style::SP_M,
            Style::TEXT_DIM,
        );
        ui.label(
            egui::RichText::new(entry.actor.as_str())
                .small()
                .strong()
                .color(Style::TEXT),
        );
        ui.label(egui::RichText::new(&entry.summary).color(Style::TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(relative_age(now_unix_ms, entry.created_unix_ms))
                    .small()
                    .color(Style::TEXT_DIM),
            );
        });
    });
}

/// The Carbon glyph for an Activity row, chosen from the event-kind band the
/// same way the filter classifies it (kept within [`ALL_COLLAB_ICONS`]).
///
/// [`ALL_COLLAB_ICONS`]: crate::ALL_COLLAB_ICONS
fn entry_icon(kind_tag: &str) -> &'static str {
    if ActivityFilter::Messages.matches(kind_tag) {
        "share"
    } else if ActivityFilter::Alerts.matches(kind_tag) {
        "notification"
    } else if ActivityFilter::Calls.matches(kind_tag) {
        "audio-volume-high"
    } else if ActivityFilter::Files.matches(kind_tag) {
        "download"
    } else if ActivityFilter::People.matches(kind_tag) {
        "view-grid"
    } else {
        "view"
    }
}

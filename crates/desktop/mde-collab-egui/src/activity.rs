//! Activity mode — the action-oriented chronological feed from the
//! [`ActivityFeed`](mde_collab_types::ActivityFeed) projection, with band
//! filters. In the Activity app it prefers the cross-space feed; when routed as
//! a selected channel body it falls back to that channel's feed. There is
//! deliberately **no** competing global search box here (spec §2): the rail is
//! the space selector and the chips are the only filter.

use mde_egui::egui;
use mde_egui::Style;

use std::ops::Range;

use mde_collab_types::ActivityEntry;

use crate::{icons, relative_age, ActivityFilter, CommunicationsSurface, MeshTeamsApp};

const ACTIVITY_ROW_HEIGHT: f32 = Style::SP_L;

impl CommunicationsSurface {
    /// Render the Activity feed: the global cross-space attention feed for the
    /// Activity app, otherwise the selected channel feed. A row of band-filter
    /// chips sits above the chronological entries the active filter admits.
    pub(crate) fn activity_body(&mut self, ui: &mut egui::Ui, data: &dyn crate::CollabData) {
        self.activity_filter_chips(ui);
        ui.add_space(Style::SP_S);
        ui.separator();
        ui.add_space(Style::SP_S);

        let feed = if self.app() == MeshTeamsApp::Activity {
            data.activity(None)
                .or_else(|| data.activity(self.selected_space()))
        } else {
            data.activity(self.selected_space())
        };
        let entries: &[ActivityEntry] = feed.map_or(&[], |f| f.entries.as_slice());
        let filter = self.activity_filter();
        let now = data.now_unix_ms();

        let admitted = activity_rows(entries, filter);
        if admitted.is_empty() {
            ui.label(
                egui::RichText::new("No activity for this filter yet.").color(Style::TEXT_DIM),
            );
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ACTIVITY_ROW_HEIGHT, admitted.len(), |ui, row_range| {
                for entry in admitted.range(row_range) {
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

/// The rows admitted by the active Activity filter. The common first-open path
/// is [`ActivityFilter::All`], so keep that as a borrowed slice instead of
/// allocating a `Vec<&ActivityEntry>` for every retained row before
/// [`ScrollArea::show_rows`](egui::ScrollArea::show_rows) virtualizes painting.
pub(crate) enum ActivityRows<'a> {
    All(&'a [ActivityEntry]),
    Filtered(Vec<&'a ActivityEntry>),
}

impl<'a> ActivityRows<'a> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::All(entries) => entries.len(),
            Self::Filtered(entries) => entries.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn range(&self, row_range: Range<usize>) -> ActivityRowRange<'_, 'a> {
        ActivityRowRange {
            rows: self,
            next: row_range.start,
            end: row_range.end,
        }
    }

    #[cfg(test)]
    pub(crate) fn uses_unfiltered_source(&self) -> bool {
        matches!(self, Self::All(_))
    }
}

pub(crate) struct ActivityRowRange<'rows, 'entry> {
    rows: &'rows ActivityRows<'entry>,
    next: usize,
    end: usize,
}

impl<'rows, 'entry> Iterator for ActivityRowRange<'rows, 'entry> {
    type Item = &'entry ActivityEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        self.next += 1;
        match self.rows {
            ActivityRows::All(entries) => entries.get(index),
            ActivityRows::Filtered(entries) => entries.get(index).copied(),
        }
    }
}

pub(crate) fn activity_rows(entries: &[ActivityEntry], filter: ActivityFilter) -> ActivityRows<'_> {
    if filter == ActivityFilter::All {
        ActivityRows::All(entries)
    } else {
        ActivityRows::Filtered(filtered_activity_entries(entries, filter))
    }
}

pub(crate) fn filtered_activity_entries(
    entries: &[ActivityEntry],
    filter: ActivityFilter,
) -> Vec<&ActivityEntry> {
    entries
        .iter()
        .filter(|entry| filter.matches(&entry.kind_tag))
        .collect()
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

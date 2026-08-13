//! Activity mode — the action-oriented chronological feed from the
//! [`ActivityFeed`](mde_collab_types::ActivityFeed) projection, with band
//! filters. In the Activity app it prefers the cross-space feed; when routed as
//! a selected channel body it falls back to that channel's feed. There is
//! deliberately **no** competing global search box here (spec §2): the rail is
//! the space selector and the chips are the only filter.

use mde_egui::egui;
use mde_egui::Style;

use std::ops::Range;

use mde_collab_types::{ActivityEntry, AlertInbox, Severity, SpaceId};

use crate::{icons, relative_age, ActivityFilter, CommunicationsSurface, MeshTeamsApp};

fn theme_color(ui: &egui::Ui, color: egui::Color32) -> egui::Color32 {
    Style::resolve_color(ui.ctx(), color)
}

const ACTIVITY_ROW_HEIGHT: f32 = Style::SP_L;
/// Keep a burst of identical Activity notifications readable without merging
/// separate incidents that happen later. This matches the notification lane's
/// bounded five-minute coalescing window.
const ACTIVITY_COALESCE_WINDOW_MS: u64 = 5 * 60 * 1_000;

/// View-local state for one Activity source. The entries are cloned only when
/// the source is live (to establish the pause snapshot) or when Resume is
/// clicked; while paused they remain the exact projection snapshot the user
/// chose to hold.
#[derive(Clone, Default)]
struct ActivityViewState {
    source: Option<ActivitySource>,
    paused: bool,
    entries: Vec<ActivityEntry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ActivitySource {
    app: MeshTeamsApp,
    space: Option<SpaceId>,
}

/// One visible row backed by one real ActivityEntry. `count` is the number of
/// adjacent, identical projection entries represented by the row; no synthetic
/// ActivityEntry is introduced by the UI.
#[derive(Clone, Copy)]
pub(crate) struct ActivityRow<'a> {
    entry: &'a ActivityEntry,
    count: usize,
    severity: Option<Severity>,
}

impl ActivityRow<'_> {
    #[cfg(test)]
    pub(crate) fn entry(&self) -> &ActivityEntry {
        self.entry
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    #[cfg(test)]
    pub(crate) fn severity(&self) -> Option<Severity> {
        self.severity
    }
}

/// The bounded/coalesced rows supplied to the virtualized Activity painter.
pub(crate) struct CoalescedActivityRows<'a> {
    rows: Vec<ActivityRow<'a>>,
}

impl<'a> CoalescedActivityRows<'a> {
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn range(&self, row_range: Range<usize>) -> CoalescedActivityRowRange<'_, 'a> {
        CoalescedActivityRowRange {
            rows: &self.rows,
            next: row_range.start,
            end: row_range.end,
        }
    }
}

struct CoalescedActivityRowRange<'rows, 'entry> {
    rows: &'rows [ActivityRow<'entry>],
    next: usize,
    end: usize,
}

impl<'rows, 'entry> Iterator for CoalescedActivityRowRange<'rows, 'entry> {
    type Item = &'rows ActivityRow<'entry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        self.next += 1;
        self.rows.get(index)
    }
}

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
        let live_entries: &[ActivityEntry] = feed.map_or(&[], |f| f.entries.as_slice());
        let source = ActivitySource {
            app: self.app(),
            space: feed.map(|f| f.space).unwrap_or(self.selected_space()),
        };
        let state_id = ui.id().with("activity-feed-state");
        let mut state = activity_view_state(ui, state_id, source, live_entries);
        activity_pause_resume_control(ui, &mut state, live_entries);
        ui.ctx()
            .data_mut(|data| data.insert_temp(state_id, state.clone()));

        let filter = self.activity_filter();
        let now = data.now_unix_ms();

        let admitted = coalesced_activity_rows(&state.entries, filter, data.alert_inbox());
        if admitted.is_empty() {
            ui.label(
                egui::RichText::new("No activity for this filter yet")
                    .color(theme_color(ui, Style::TEXT_DIM)),
            );
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ACTIVITY_ROW_HEIGHT, admitted.len(), |ui, row_range| {
                for row in admitted.range(row_range) {
                    activity_row(ui, row, now);
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
                        theme_color(ui, Style::TEXT_DIM)
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

/// Load or refresh the source snapshot. egui's temporary data survives frames
/// for the lifetime of this context, which is enough for a seat-local view
/// preference without adding state to the collaboration contract or surface
/// model.
fn activity_view_state(
    ui: &egui::Ui,
    state_id: egui::Id,
    source: ActivitySource,
    live_entries: &[ActivityEntry],
) -> ActivityViewState {
    let mut state = ui
        .ctx()
        .data_mut(|data| data.get_temp::<ActivityViewState>(state_id))
        .unwrap_or_default();

    if state.source != Some(source) {
        state = ActivityViewState {
            source: Some(source),
            paused: false,
            entries: live_entries.to_vec(),
        };
    } else if !state.paused {
        state.entries = live_entries.to_vec();
    }

    state
}

/// A visible, text-labelled control for holding the current feed snapshot.
/// Pausing only affects this view: the real projection keeps updating behind
/// it, and Resume takes a fresh snapshot before painting the next row count.
fn activity_pause_resume_control(
    ui: &mut egui::Ui,
    state: &mut ActivityViewState,
    live_entries: &[ActivityEntry],
) {
    ui.horizontal(|ui| {
        let (status, status_color, action) = if state.paused {
            ("Feed paused", Style::WARN, "Resume feed")
        } else {
            ("Live feed", Style::OK, "Pause feed")
        };
        ui.label(
            egui::RichText::new(status)
                .small()
                .color(theme_color(ui, status_color)),
        );
        if ui.button(action).clicked() {
            state.paused = !state.paused;
            if !state.paused {
                state.entries = live_entries.to_vec();
            }
        }
    });
}

/// Compatibility helpers for the pre-coalescing unit tests. Production
/// rendering uses [`CoalescedActivityRows`] exclusively, so this borrowed
/// source-slice assertion does not leave a second virtualized implementation in
/// the shipped Activity path.
#[cfg(test)]
pub(crate) enum ActivityRows<'a> {
    All(&'a [ActivityEntry]),
    Filtered(Vec<&'a ActivityEntry>),
}

#[cfg(test)]
impl ActivityRows<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::All(entries) => entries.len(),
            Self::Filtered(entries) => entries.len(),
        }
    }

    #[cfg(test)]
    pub(crate) fn uses_unfiltered_source(&self) -> bool {
        matches!(self, Self::All(_))
    }
}

#[cfg(test)]
pub(crate) fn activity_rows(entries: &[ActivityEntry], filter: ActivityFilter) -> ActivityRows<'_> {
    if filter == ActivityFilter::All {
        ActivityRows::All(entries)
    } else {
        ActivityRows::Filtered(filtered_activity_entries(entries, filter))
    }
}

#[cfg(test)]
pub(crate) fn filtered_activity_entries(
    entries: &[ActivityEntry],
    filter: ActivityFilter,
) -> Vec<&ActivityEntry> {
    entries
        .iter()
        .filter(|entry| filter.matches(&entry.kind_tag))
        .collect()
}

/// Admit filtered Activity entries and coalesce only adjacent repeats within
/// the bounded notification window. The severity is joined from the existing
/// AlertInbox by event id; equal summaries at different severity levels remain
/// separate rows so a Critical alert can never disappear into an Info repeat.
pub(crate) fn coalesced_activity_rows<'a>(
    entries: &'a [ActivityEntry],
    filter: ActivityFilter,
    alert_inbox: Option<&AlertInbox>,
) -> CoalescedActivityRows<'a> {
    let mut rows = Vec::new();

    for entry in entries
        .iter()
        .filter(|entry| filter.matches(&entry.kind_tag))
    {
        let severity = activity_entry_severity(entry, alert_inbox);
        let can_coalesce = rows.last().is_some_and(|last: &ActivityRow<'a>| {
            last.entry.space == entry.space
                && last.entry.actor == entry.actor
                && last.entry.kind_tag == entry.kind_tag
                && last.entry.summary == entry.summary
                && last.severity == severity
                && last.entry.created_unix_ms.abs_diff(entry.created_unix_ms)
                    < ACTIVITY_COALESCE_WINDOW_MS
        });

        if can_coalesce {
            if let Some(last) = rows.last_mut() {
                last.count = last.count.saturating_add(1);
            }
        } else {
            rows.push(ActivityRow {
                entry,
                count: 1,
                severity,
            });
        }
    }

    CoalescedActivityRows { rows }
}

fn activity_entry_severity(
    entry: &ActivityEntry,
    alert_inbox: Option<&AlertInbox>,
) -> Option<Severity> {
    alert_inbox.and_then(|inbox| {
        inbox
            .alerts
            .iter()
            .find(|view| view.event_id == entry.event_id)
            .map(|view| view.alert.severity)
    })
}

/// One Activity row: a band glyph, the actor, the projected summary line, and a
/// right-aligned relative age.
fn activity_row(ui: &mut egui::Ui, row: &ActivityRow<'_>, now_unix_ms: i64) {
    let entry = row.entry;
    let icon_color = row
        .severity
        .map(activity_severity_color)
        .unwrap_or(theme_color(ui, Style::TEXT_DIM));
    ui.horizontal(|ui| {
        icons::icon(ui, entry_icon(&entry.kind_tag), Style::SP_M, icon_color);
        ui.label(
            egui::RichText::new(entry.actor.as_str())
                .small()
                .strong()
                .color(theme_color(ui, Style::TEXT)),
        );
        ui.label(egui::RichText::new(&entry.summary).color(theme_color(ui, Style::TEXT)));
        if row.count > 1 {
            ui.label(
                egui::RichText::new(format!("×{}", row.count))
                    .small()
                    .strong()
                    .color(theme_color(ui, icon_color)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(relative_age(now_unix_ms, entry.created_unix_ms))
                    .small()
                    .color(theme_color(ui, Style::TEXT_DIM)),
            );
        });
    });
}

const fn activity_severity_color(severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Info => Style::ACCENT,
        Severity::Warning => Style::WARN,
        Severity::Critical => Style::DANGER,
    }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mde_collab_types::{ActorClock, ActorId, AlertPayload, AlertView, EventId};

    use super::{
        coalesced_activity_rows, ActivityEntry, ActivityFilter, AlertInbox, Severity, SpaceId,
    };

    fn entry(
        event_id: EventId,
        space: SpaceId,
        actor: &ActorId,
        created_unix_ms: i64,
        kind_tag: &str,
        summary: &str,
    ) -> ActivityEntry {
        ActivityEntry {
            event_id,
            space,
            actor: actor.clone(),
            clock: ActorClock::at(created_unix_ms.max(0) as u64, 0),
            created_unix_ms,
            kind_tag: kind_tag.to_owned(),
            summary: summary.to_owned(),
        }
    }

    fn alert(event_id: EventId, space: SpaceId, severity: Severity) -> AlertView {
        AlertView {
            event_id,
            space,
            alert: AlertPayload {
                severity,
                source: "test-source".to_owned(),
                headline: "test alert".to_owned(),
                fields: BTreeMap::new(),
                actions: Vec::new(),
                goto: None,
            },
            acknowledged: false,
            snoozed_until_unix_ms: None,
        }
    }

    #[test]
    fn coalesces_adjacent_repeats_and_keeps_truthful_count() {
        let space = SpaceId::new();
        let actor = ActorId::new("eagle");
        let entries = vec![
            entry(
                EventId::new(),
                space,
                &actor,
                10_000,
                "alert_raised",
                "disk warning",
            ),
            entry(
                EventId::new(),
                space,
                &actor,
                10_001,
                "alert_raised",
                "disk warning",
            ),
            entry(
                EventId::new(),
                space,
                &actor,
                10_002,
                "alert_raised",
                "disk warning",
            ),
        ];

        let rows = coalesced_activity_rows(&entries, ActivityFilter::All, None);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows.rows[0].count(), 3);
        assert_eq!(rows.rows[0].entry(), &entries[0]);
    }

    #[test]
    fn severity_change_breaks_an_alert_repeat_group() {
        let space = SpaceId::new();
        let actor = ActorId::new("eagle");
        let info_id = EventId::new();
        let critical_id = EventId::new();
        let entries = vec![
            entry(
                info_id,
                space,
                &actor,
                10_000,
                "alert_raised",
                "disk warning",
            ),
            entry(
                critical_id,
                space,
                &actor,
                10_001,
                "alert_raised",
                "disk warning",
            ),
        ];
        let inbox = AlertInbox {
            alerts: vec![
                alert(info_id, space, Severity::Info),
                alert(critical_id, space, Severity::Critical),
            ],
        };

        let rows = coalesced_activity_rows(&entries, ActivityFilter::Alerts, Some(&inbox));

        assert_eq!(rows.len(), 2);
        assert_eq!(rows.rows[0].severity(), Some(Severity::Info));
        assert_eq!(rows.rows[1].severity(), Some(Severity::Critical));
    }

    #[test]
    fn repeat_after_the_bounded_window_stays_a_new_row() {
        let space = SpaceId::new();
        let actor = ActorId::new("eagle");
        let entries = vec![
            entry(
                EventId::new(),
                space,
                &actor,
                10_000,
                "message_posted",
                "same",
            ),
            entry(
                EventId::new(),
                space,
                &actor,
                10_000 + 5 * 60 * 1_000,
                "message_posted",
                "same",
            ),
        ];

        let rows = coalesced_activity_rows(&entries, ActivityFilter::Messages, None);

        assert_eq!(rows.len(), 2);
        assert!(rows.rows.iter().all(|row| row.count() == 1));
    }
}

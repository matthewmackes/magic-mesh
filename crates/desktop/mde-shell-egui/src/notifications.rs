//! The Notifications surface — tails the Bus alert lanes and accumulates
//! mesh-wide alerts newest-first (security, presence, firewall, compute).
//!
//! Reuses the `mde-notify` shared model (`AlertTail` + `AlertItem` + severity
//! classification) so the alert rendering is consistent with the standalone
//! notification center. No GUI deps live in `mde-notify`; this module owns
//! the egui paint path.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use mde_bus::persist::Persist;
use mde_egui::egui::{self, Align, Layout, RichText, ScrollArea};
use mde_egui::Style;
use mde_notify::{AlertItem, AlertTail, Severity};

const REFRESH: Duration = Duration::from_secs(5);

/// The Notifications surface state: the alert tail (incremental bus cursor +
/// dedup set) and the accumulated `AlertItem` list newest-first.
pub(crate) struct NotificationsState {
    bus_root: Option<PathBuf>,
    tail: AlertTail,
    /// All alerts accumulated since the shell started, newest first.
    items: Vec<AlertItem>,
    last_poll: Option<Instant>,
}

impl Default for NotificationsState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            tail: AlertTail::default(),
            items: Vec::new(),
            last_poll: None,
        }
    }
}

impl NotificationsState {
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
        let fresh = self.tail.poll(&persist);
        // Prepend newest first: `fresh` arrives oldest-first, so reverse then insert.
        for item in fresh.into_iter().rev() {
            self.items.insert(0, item);
        }
    }

    /// Render the notifications list into `ui`.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        if self.items.is_empty() {
            let (title, subtitle) = empty_copy(self.bus_root.is_some());
            crate::session::empty_state(ui, title, subtitle);
            return;
        }

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in &self.items {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                severity_color(item.severity),
                                RichText::new(&item.title).size(Style::BODY).strong(),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                mde_egui::muted_note(ui, item.source.label());
                            });
                        });
                        if !item.body.is_empty() {
                            mde_egui::muted_note(ui, &item.body);
                        }
                        if let Some(host) = &item.host {
                            mde_egui::muted_note(ui, format!("from {host}"));
                        }
                    });
                    ui.add_space(Style::SP_S);
                }
            });
    }
}

/// The empty-panel copy — honest about *why* nothing is listed. With no mesh Bus
/// directory the alert lanes are unreadable (a gated read), which must not render
/// as a live-looking "no alerts" (§7).
const fn empty_copy(has_bus: bool) -> (&'static str, &'static str) {
    if has_bus {
        (
            "No alerts",
            "Mesh alerts — security, presence, firewall, compute — appear here as the Bus lanes report them.",
        )
    } else {
        (
            "Alerts unavailable",
            "No mesh Bus directory on this node, so the alert lanes can't be read — joining the mesh (the mde-bus spool) unblocks this panel.",
        )
    }
}

/// Map `Severity` to the matching `Style` color constant (§4 — no raw hex).
fn severity_color(s: Severity) -> egui::Color32 {
    match s {
        Severity::Critical => Style::DANGER,
        Severity::Warning => Style::WARN,
        Severity::Info => Style::ACCENT,
        Severity::Success => Style::OK,
    }
}

#[cfg(test)]
mod tests {
    use super::{empty_copy, severity_color, Severity, Style};

    #[test]
    fn severity_color_maps_all_variants_without_raw_hex() {
        // Each severity maps to a named Style token, not a literal color.
        assert_eq!(severity_color(Severity::Critical), Style::DANGER);
        assert_eq!(severity_color(Severity::Warning), Style::WARN);
        assert_eq!(severity_color(Severity::Info), Style::ACCENT);
        assert_eq!(severity_color(Severity::Success), Style::OK);
    }

    #[test]
    fn empty_copy_distinguishes_a_missing_bus_from_a_quiet_mesh() {
        // A quiet mesh reads as "no alerts"; a missing Bus must NOT (§7 — a gated
        // read never renders as a live-looking empty state).
        let (title, _) = empty_copy(true);
        assert_eq!(title, "No alerts");
        let (title, subtitle) = empty_copy(false);
        assert_eq!(title, "Alerts unavailable");
        assert!(
            subtitle.contains("Bus") && subtitle.contains("unblocks"),
            "the gated copy names what's missing and what unblocks it: {subtitle}"
        );
    }
}

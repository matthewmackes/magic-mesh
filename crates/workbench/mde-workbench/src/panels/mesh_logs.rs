//! PLANES-8 — the **Mesh Logs / metrics** panel (Node plane), absorbing
//! ENT-9.
//!
//! A journald view of the mesh daemon unit (`journalctl -u mackesd`) with
//! a `--since` window selector — the ENT-9 "mesh logs" surface rendered —
//! plus a deep-link to the local Netdata dashboard for the metrics strip
//! (W23). Distinct from the legacy Maintain → Logs panel (which tailed the
//! retired shell log + sway journal).
//!
//! Build-now-defer-visual: the journalctl argv builder + window model are
//! pure and unit-tested; the on-Cosmic `/preview` pass (and embedding the
//! live Netdata strip vs. the deep-link) is the deferred tail.

use iced::widget::{column, row, scrollable, text};
use iced::{Element, Length, Task};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

/// The mesh daemon systemd unit whose journal this panel renders.
pub const MESH_UNIT: &str = "mackesd";
/// Local Netdata dashboard (the W23 metrics deep-link target).
pub const NETDATA_URL: &str = "http://localhost:19999";
/// Max journal lines fetched per window.
pub const MAX_LINES: u32 = 500;

/// A `--since` window option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub label: &'static str,
    /// The journalctl `--since` argument (e.g. `1 hour ago`, `today`).
    pub since: &'static str,
}

/// The selectable windows (W23 `--since`).
pub const WINDOWS: &[Window] = &[
    Window {
        label: "15m",
        since: "15 min ago",
    },
    Window {
        label: "1h",
        since: "1 hour ago",
    },
    Window {
        label: "24h",
        since: "1 day ago",
    },
    Window {
        label: "Today",
        since: "today",
    },
];

/// Build the `journalctl` argv for the mesh unit + window. Pure so the
/// invocation shape is unit-tested without shelling.
#[must_use]
pub fn journalctl_argv(unit: &str, since: &str, lines: u32) -> Vec<String> {
    vec![
        "-u".into(),
        unit.into(),
        "--since".into(),
        since.into(),
        "-n".into(),
        lines.to_string(),
        "--no-pager".into(),
    ]
}

/// The Mesh Logs panel state.
#[derive(Debug, Clone)]
pub struct MeshLogsPanel {
    pub log: String,
    pub since: &'static str,
    pub status: String,
    pub busy: bool,
}

impl Default for MeshLogsPanel {
    fn default() -> Self {
        Self {
            log: String::new(),
            since: WINDOWS[1].since, // default 1h
            status: String::new(),
            busy: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(String),
    Error(String),
    SetWindow(&'static str),
    RefreshClicked,
}

impl MeshLogsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Self::load_since(WINDOWS[1].since)
    }

    fn load_since(since: &'static str) -> Task<crate::Message> {
        let argv = journalctl_argv(MESH_UNIT, since, MAX_LINES);
        Task::perform(
            async move {
                match Command::new("journalctl").args(&argv).output().await {
                    Ok(o) if o.status.success() => {
                        Message::Loaded(String::from_utf8_lossy(&o.stdout).into_owned())
                    }
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr);
                        // No journal access / not systemd → honest message.
                        Message::Error(if err.trim().is_empty() {
                            "journalctl returned no output (is mackesd a systemd unit here?)".into()
                        } else {
                            err.trim().to_string()
                        })
                    }
                    Err(e) => Message::Error(format!("journalctl unavailable: {e}")),
                }
            },
            crate::Message::MeshLogs,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(log) => {
                self.log = log;
                self.busy = false;
                self.status.clear();
                Task::none()
            }
            Message::Error(e) => {
                self.status = e;
                self.busy = false;
                Task::none()
            }
            Message::SetWindow(since) => {
                self.since = since;
                self.busy = true;
                self.status = "Loading…".into();
                Self::load_since(since)
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load_since(self.since)
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;

        // Window selector buttons (the active one is non-pressable).
        let mut controls = row![].spacing(8).align_y(iced::Alignment::Center);
        for w in WINDOWS {
            let active = w.since == self.since;
            controls = controls.push(variant_button(
                w.label,
                if active {
                    ButtonVariant::Secondary
                } else {
                    ButtonVariant::Ghost
                },
                (!active).then_some(crate::Message::MeshLogs(Message::SetWindow(w.since))),
                palette,
            ));
        }
        controls = controls.push(variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then_some(crate::Message::MeshLogs(Message::RefreshClicked)),
            palette,
        ));
        // W23 — Netdata metrics deep-link (the strip embeds on Cosmic; the
        // deep-link is the always-available fallback).
        controls = controls.push(text(format!("metrics: {NETDATA_URL}")).size(12));

        let body: Element<'_, crate::Message> = if self.log.trim().is_empty() {
            let msg = if self.status.is_empty() {
                format!("No journal entries for {MESH_UNIT} in this window.")
            } else {
                self.status.clone()
            };
            text(msg).size(13).into()
        } else {
            scrollable(text(self.log.clone()).size(12).font(iced::Font::MONOSPACE))
                .height(Length::Fill)
                .into()
        };

        // PLANES-2 — the journal/metrics panel is the Netdata surface.
        let netdata = crate::panel_chrome::hero_band(
            mde_theme::hero::Hero::Netdata,
            crate::panel_chrome::pkg_version_cached("netdata").as_deref(),
            palette,
        );
        crate::panel_chrome::panel_container(
            column![
                row![
                    text(format!("Mesh daemon journal — {MESH_UNIT}")).size(20),
                    iced::widget::Space::new().width(Length::Fill),
                    netdata,
                ]
                .align_y(iced::Alignment::Center),
                controls,
                body,
            ]
            .spacing(12)
            .width(Length::Fill)
            .into(),
            density,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journalctl_argv_targets_the_mesh_unit_and_window() {
        let argv = journalctl_argv("mackesd", "1 hour ago", 500);
        assert_eq!(argv[0], "-u");
        assert_eq!(argv[1], "mackesd");
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "--since" && w[1] == "1 hour ago"));
        assert!(argv.windows(2).any(|w| w[0] == "-n" && w[1] == "500"));
        assert!(argv.contains(&"--no-pager".to_string()));
    }

    #[test]
    fn default_window_is_one_hour() {
        let p = MeshLogsPanel::new();
        assert_eq!(p.since, "1 hour ago");
    }

    #[test]
    fn set_window_changes_the_since_and_marks_busy() {
        let mut p = MeshLogsPanel::new();
        let _ = p.update(Message::SetWindow("today"));
        assert_eq!(p.since, "today");
        assert!(p.busy);
        let _ = p.update(Message::Loaded("some log\n".into()));
        assert!(!p.busy);
        assert!(p.log.contains("some log"));
    }

    #[test]
    fn windows_cover_the_documented_spans() {
        let labels: Vec<&str> = WINDOWS.iter().map(|w| w.label).collect();
        assert_eq!(labels, vec!["15m", "1h", "24h", "Today"]);
    }
}

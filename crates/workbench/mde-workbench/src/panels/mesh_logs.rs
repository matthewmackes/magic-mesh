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

// CUT-1: cosmic::Element bakes in cosmic::Theme, matching panel_chrome's
// container helpers; the local view/body elements must thread the same theme.
use cosmic::iced::font::Weight;
use cosmic::iced::widget::{column, container, row, scrollable, text, Space};
use cosmic::iced::{alignment, Background, Border, Color, Font, Length, Padding, Task};
use cosmic::Element;
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;
use crate::status_strip::mono_text;
use mde_theme::{carbon, FontSize, FontWeight, Palette, TypeRole};

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
        let sizes = FontSize::defaults();
        let weights = FontWeight::defaults();
        let density = crate::live_theme::tokens().density;

        // ---- header: title · Netdata hero ----
        let title = text("Monitoring · Logs & Metrics")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        // PLANES-2 — the journal/metrics panel is the Netdata surface.
        let netdata = crate::panel_chrome::hero_band(
            mde_theme::hero::Hero::Netdata,
            crate::panel_chrome::pkg_version_cached("netdata").as_deref(),
            palette,
        );
        let header = row![title, Space::new().width(Length::Fill), netdata]
            .spacing(12)
            .align_y(alignment::Vertical::Center);

        // ---- metrics band: the REAL Netdata deep-link (W23). No fabricated
        // gauge values (§7) — the live mackesd_* gauges live in the Netdata
        // dashboard, surfaced here as the always-available deep-link. ----
        let metrics_body = container(
            column![
                mono_text(
                    format!("netdata · {NETDATA_URL}"),
                    TypeRole::Body,
                    &sizes,
                    &weights,
                )
                .colr(palette.accent.into_cosmic_color()),
                text("live mackesd_* gauges in the Netdata dashboard")
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(2),
        )
        .padding(Padding::from([10u16, 13u16]))
        .width(Length::Fill)
        .into();
        let metrics_card = dense_card("mackesd metrics", metrics_body, palette, &sizes);

        // ---- window selector + refresh (real controls) ----
        let mut controls = row![].spacing(8).align_y(alignment::Vertical::Center);
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

        // ---- journald log stream (the real `journalctl -u mackesd` output,
        // split into time/host/unit/msg columns for the design's stream) ----
        let window_label = WINDOWS
            .iter()
            .find(|w| w.since == self.since)
            .map_or("", |w| w.label);
        let log_header = container(
            row![
                text(format!("journald · {MESH_UNIT}").to_uppercase())
                    .size(TypeRole::Caption.size_in(sizes))
                    .font(Font {
                        weight: Weight::Medium,
                        ..Font::DEFAULT
                    })
                    .colr(palette.text_muted.into_cosmic_color()),
                Space::new().width(Length::Fill),
                mono_text(
                    format!("follow · {window_label}"),
                    TypeRole::Caption,
                    &sizes,
                    &weights,
                )
                .colr(palette.text_muted.into_cosmic_color()),
            ]
            .align_y(alignment::Vertical::Center),
        )
        .padding(Padding::from([9u16, 15u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: Some(palette.text.into_cosmic_color()),
        });

        let log_body: Element<'_, crate::Message> = if self.log.trim().is_empty() {
            let msg = if self.status.is_empty() {
                format!("No journal entries for {MESH_UNIT} in this window.")
            } else {
                self.status.clone()
            };
            container(
                text(msg)
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            )
            .padding(Padding::from([10u16, 15u16]))
            .into()
        } else {
            let mut col = column![].width(Length::Fill);
            for line in self.log.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                col = col.push(log_row(line, palette, &sizes, &weights));
            }
            scrollable(col).height(Length::Fill).into()
        };

        let log_card = container(
            column![log_header, hairline(palette), log_body]
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(palette.background.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: Some(palette.text.into_cosmic_color()),
        });

        crate::panel_chrome::panel_container(
            column![header, metrics_card, controls, log_card]
                .spacing(12)
                .width(Length::Fill)
                .into(),
            density,
        )
    }
}

/// A 1 px full-width hairline divider in the border token.
fn hairline<'a>(palette: Palette) -> Element<'a, crate::Message> {
    let color = palette.border.into_cosmic_color();
    container(Space::new().height(Length::Fixed(1.0)).width(Length::Fill))
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: None,
        })
        .into()
}

/// A dense Carbon card: a surface-tinted box with a sharp 1 px hairline
/// border, an uppercase section-label header bar, an internal divider, and
/// the caller's body.
fn dense_card<'a>(
    title: &str,
    body: Element<'a, crate::Message>,
    palette: Palette,
    sizes: &FontSize,
) -> Element<'a, crate::Message> {
    let header = container(
        text(title.to_uppercase())
            .size(TypeRole::Caption.size_in(*sizes))
            .font(Font {
                weight: Weight::Medium,
                ..Font::DEFAULT
            })
            .colr(palette.text_muted.into_cosmic_color()),
    )
    .padding(Padding::from([8u16, 13u16]))
    .width(Length::Fill);

    let inner = column![header, hairline(palette), body].width(Length::Fill);

    container(inner)
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: Some(palette.text.into_cosmic_color()),
        })
        .into()
}

/// One journald log row, rendered as the design's mono columns: time (muted)
/// · host (teal) · unit (blue) · message (primary). Built from the parsed
/// real log line.
fn log_row<'a>(
    line: &str,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let (t, h, u, m) = parse_journal_line(line);
    container(
        row![
            mono_text(t, TypeRole::Caption, sizes, weights)
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::Fixed(64.0)),
            mono_text(h, TypeRole::Caption, sizes, weights)
                .colr(carbon::TEAL_30.into_cosmic_color())
                .width(Length::Fixed(96.0)),
            mono_text(u, TypeRole::Caption, sizes, weights)
                .colr(carbon::BLUE_50.into_cosmic_color())
                .width(Length::Fixed(110.0)),
            mono_text(m, TypeRole::Caption, sizes, weights)
                .colr(palette.text.into_cosmic_color())
                .width(Length::Fill),
        ]
        .spacing(12),
    )
    .padding(Padding::from([3u16, 15u16]))
    .width(Length::Fill)
    .into()
}

/// PLANES-8 — best-effort split of a `journalctl` default-format line into
/// `(time, host, unit, message)` for the design's columnar log stream. This
/// is presentation only: the real log text is preserved; an unparseable line
/// falls back to the whole line in the message column (§7 — no fabrication).
fn parse_journal_line(line: &str) -> (String, String, String, String) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    // Default format: "<Mon> <day> <HH:MM:SS> <host> <unit>[pid]: <msg>".
    if parts.len() >= 6 && is_clock(parts[2]) {
        let time = parts[2].to_string();
        let host = parts[3].to_string();
        let unit_raw = parts[4];
        let unit = unit_raw
            .split('[')
            .next()
            .unwrap_or(unit_raw)
            .trim_end_matches(':')
            .to_string();
        let msg = parts[5..].join(" ");
        (time, host, unit, msg)
    } else {
        (
            String::new(),
            String::new(),
            String::new(),
            line.trim().to_string(),
        )
    }
}

/// True when `s` looks like an `HH:MM:SS` clock token.
fn is_clock(s: &str) -> bool {
    s.len() == 8
        && s.as_bytes().iter().enumerate().all(|(i, c)| {
            if i == 2 || i == 5 {
                *c == b':'
            } else {
                c.is_ascii_digit()
            }
        })
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

    #[test]
    fn parse_journal_line_splits_the_default_format_into_columns() {
        // UNIFY-11 — the design's time/host/unit/msg columns, from a real
        // journalctl default-format line (unit's `[pid]` + trailing `:` cut).
        let (t, h, u, m) =
            parse_journal_line("Jun 28 14:31:08 oak mackesd[1234]: reconcile r-2291 ok");
        assert_eq!(t, "14:31:08");
        assert_eq!(h, "oak");
        assert_eq!(u, "mackesd");
        assert_eq!(m, "reconcile r-2291 ok");
    }

    #[test]
    fn parse_journal_line_falls_back_to_the_message_column() {
        // A non-journal line (e.g. journalctl's "-- No entries --") is kept
        // verbatim in the message column rather than mis-split (§7).
        let (t, h, u, m) = parse_journal_line("-- No entries --");
        assert!(t.is_empty() && h.is_empty() && u.is_empty());
        assert_eq!(m, "-- No entries --");
    }
}

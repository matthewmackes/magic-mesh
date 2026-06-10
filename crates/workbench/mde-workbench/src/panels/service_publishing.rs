//! NF-13.8 (v2.5) — Network → Service Publishing panel.
//!
//! Surfaces every canonical Nebula-published service (SSH, NATS,
//! Mesh FS, Media, rsync, WoL, AV) with: status pill (publishable
//! when an overlay IP exists, otherwise "not yet enrolled"), port
//! + protocol, and a per-row hint for the service binary.
//!
//! Reads the live snapshot over the mesh Bus from
//! `action/nebula/published-services` (RETIRE-PY.7 — replaced the v1.x
//! `python3 -c mackes.mesh_nebula` shell-out). `mackesd` builds the summary
//! (the 7 canonical services × this peer's overlay IP) and answers the Bus
//! query; the panel's `parse_summary` decodes the same JSON list-of-rows shape.
//!
//! Chrome influence (per iteration skill Phase 0.8): Ableton
//! parameter table — dense rows, single indigo accent for the
//! status pill, IBM Plex Mono for the numeric port column, 1 px
//! border between rows.

use std::time::SystemTime;

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{FontSize, Palette, TypeRole};
use serde::{Deserialize, Serialize};

/// JSON wire shape published by
/// `mackes.mesh_nebula.published_services_summary()`. The
/// Python helper emits a `list[dict]`; each row deserializes
/// into this struct.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServiceRow {
    /// Stable service id — matches one of the 7 canonical
    /// entries in mackes.mesh_nebula.CANONICAL_SERVICES.
    pub id: String,
    /// Display name (e.g. "SSH" / "NATS broker").
    pub name: String,
    /// Default port the service would bind to.
    pub port: u16,
    /// "tcp" or "udp".
    pub proto: String,
    /// Overlay IP this peer binds to — `None` until the peer
    /// completes enrollment.
    pub overlay_ip: Option<String>,
    /// True when an overlay IP is allocated (the service can
    /// publish). Mirrors the Python helper's `is_publishable`
    /// flag — kept here so the UI doesn't re-derive.
    pub is_publishable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ServicePublishingPanel {
    pub rows: Vec<ServiceRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    /// Last operator-facing message — either "loaded 7
    /// services in HH:MM" or the failure mode.
    pub last_op: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        rows: Vec<ServiceRow>,
        error: Option<String>,
    },
    RefreshClicked,
}

impl ServicePublishingPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_summary() }, |(rows, error)| {
            crate::Message::ServicePublishing(Message::Loaded { rows, error })
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded { rows, error } => {
                self.rows = rows;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                self.last_op = error
                    .unwrap_or_else(|| format!("{} canonical services loaded", self.rows.len()));
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                self.last_op = "refreshing…".into();
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Service Publishing")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());

        let subtitle_text = if !self.last_op.is_empty() {
            self.last_op.clone()
        } else if let Some(t) = self.last_run_at {
            format!("last refresh {}", fmt_age(t))
        } else {
            "click Refresh to probe the Nebula overlay".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let refresh_btn = button(
            text(if self.busy { "Working…" } else { "Refresh" })
                .size(13)
                .color(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .style({
            let accent = palette.accent.into_iced_color();
            move |_t: &Theme, status: iced::widget::button::Status| {
                let bg = match status {
                    iced::widget::button::Status::Hovered => Color {
                        r: accent.r * 1.10,
                        g: accent.g * 1.10,
                        b: accent.b * 1.10,
                        a: accent.a,
                    },
                    _ => accent,
                };
                iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: Color::WHITE,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: iced::Shadow::default(),
                }
            }
        })
        .on_press(crate::Message::ServicePublishing(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let rows_widget: Element<'_, crate::Message> = if self.rows.is_empty() {
            empty_state(palette)
        } else {
            let mut col = column![].spacing(6);
            for r in &self.rows {
                col = col.push(service_row_view(r, palette));
            }
            scrollable(col).height(Length::FillPortion(1)).into()
        };

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                rows_widget,
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn empty_state<'a>(palette: Palette) -> Element<'a, crate::Message> {
    container(
        column![
            text("No service rows available")
                .size(13)
                .color(palette.text.into_iced_color()),
            Space::new().height(Length::Fixed(6.0)),
            text(
                "Run Refresh after mackesd starts (it answers the \
                 published-services query over the mesh Bus). The 7 \
                 canonical services (SSH / NATS / Mesh FS / Media / \
                 rsync / WoL / AV) will populate from the overlay state."
            )
            .size(12)
            .color(palette.text_muted.into_iced_color()),
        ]
        .spacing(2),
    )
    .padding(Padding::from([18u16, 22u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(palette.raised.into_iced_color())),
        border: Border {
            color: palette.border.into_iced_color(),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn service_row_view<'a>(r: &ServiceRow, palette: Palette) -> Element<'a, crate::Message> {
    let (pill_label, pill_color) = if r.is_publishable {
        ("Published", palette.accent.into_iced_color())
    } else {
        ("Not enrolled", palette.warning.into_iced_color())
    };
    let pill = container(text(pill_label).size(10).color(Color::WHITE))
        .padding(Padding::from([2u16, 8u16]))
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(pill_color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        });

    let overlay_text = r.overlay_ip.clone().unwrap_or_else(|| "—".to_string());
    let port_proto = format!("{}/{}", r.port, r.proto);

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(
        row![
            column![
                text(r.name.clone())
                    .size(13)
                    .color(palette.text.into_iced_color()),
                text(format!("id: {}", r.id))
                    .size(10)
                    .color(palette.text_muted.into_iced_color()),
            ]
            .spacing(2)
            .width(Length::FillPortion(3)),
            // Monospace-ish numeric column for port/protocol per
            // the Ableton content-zone influence.
            text(port_proto)
                .size(12)
                .color(palette.text.into_iced_color())
                .width(Length::FillPortion(1)),
            text(overlay_text)
                .size(12)
                .color(palette.text_muted.into_iced_color())
                .width(Length::FillPortion(2)),
            pill,
        ]
        .spacing(12)
        .align_y(iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([10u16, 16u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 5.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

// ---- I/O ------------------------------------------------------

/// Read the published-services summary over the mesh Bus (RETIRE-PY.7 — was a
/// `python3 -c mackes.mesh_nebula` shell-out). Queries
/// `action/nebula/published-services`, which `mackesd` answers with the same
/// JSON list-of-rows shape [`parse_summary`] expects. Returns `(rows, error)` —
/// on any failure the rows are empty and the error carries the operator hint.
#[must_use]
pub fn fetch_summary() -> (Vec<ServiceRow>, Option<String>) {
    match crate::dbus::nebula_request("published-services") {
        Some(json) => parse_summary(&json),
        None => (
            Vec::new(),
            Some("mackesd not reachable over the Bus — service summary unavailable".into()),
        ),
    }
}

/// Pure parser — accepts the JSON string the Python helper
/// emits and produces `(rows, optional_error)`. Pulled out for
/// direct testing without spinning up Python.
#[must_use]
pub fn parse_summary(raw: &str) -> (Vec<ServiceRow>, Option<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (
            Vec::new(),
            Some("empty reply from published_services_summary".into()),
        );
    }
    match serde_json::from_str::<Vec<ServiceRow>>(trimmed) {
        Ok(rows) => (rows, None),
        Err(e) => (Vec::new(), Some(format!("invalid JSON: {e}"))),
    }
}

fn fmt_age(t: SystemTime) -> String {
    use std::time::Duration;
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    let d = elapsed;
    let secs = d.as_secs();
    let dur = Duration::from_secs(secs);
    if dur < Duration::from_secs(60) {
        format!("{secs} s ago")
    } else if dur < Duration::from_secs(3600) {
        format!("{} min ago", secs / 60)
    } else if dur < Duration::from_secs(86_400) {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_summary_returns_empty_with_error_for_empty_input() {
        let (rows, err) = parse_summary("");
        assert!(rows.is_empty());
        assert!(err.is_some());
        assert!(err.unwrap().contains("empty reply"));
    }

    #[test]
    fn parse_summary_decodes_published_services_json() {
        // The exact JSON list-of-rows shape mackesd's
        // `action/nebula/published-services` responder emits.
        let raw = r#"[
            {"id":"ssh","name":"SSH","port":22,"proto":"tcp",
             "overlay_ip":"10.42.0.5","is_publishable":true},
            {"id":"nats","name":"NATS broker","port":4222,"proto":"tcp",
             "overlay_ip":"10.42.0.5","is_publishable":true},
            {"id":"wol","name":"Wake-on-LAN relay","port":9,"proto":"udp",
             "overlay_ip":null,"is_publishable":false}
        ]"#;
        let (rows, err) = parse_summary(raw);
        assert!(err.is_none(), "expected no error, got {err:?}");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "ssh");
        assert_eq!(rows[0].port, 22);
        assert_eq!(rows[0].proto, "tcp");
        assert_eq!(rows[0].overlay_ip.as_deref(), Some("10.42.0.5"));
        assert!(rows[0].is_publishable);
        assert!(!rows[2].is_publishable);
        assert!(rows[2].overlay_ip.is_none());
    }

    #[test]
    fn parse_summary_returns_error_for_garbage() {
        let (rows, err) = parse_summary("{not valid");
        assert!(rows.is_empty());
        assert!(err.is_some());
        assert!(err.unwrap().contains("invalid JSON"));
    }

    #[test]
    fn parse_summary_returns_empty_for_empty_array() {
        let (rows, err) = parse_summary("[]");
        assert!(rows.is_empty());
        assert!(err.is_none());
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = ServicePublishingPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_rows_without_panic() {
        let mut p = ServicePublishingPanel::new();
        p.rows = vec![
            ServiceRow {
                id: "ssh".into(),
                name: "SSH".into(),
                port: 22,
                proto: "tcp".into(),
                overlay_ip: Some("10.42.0.5".into()),
                is_publishable: true,
            },
            ServiceRow {
                id: "wol".into(),
                name: "Wake-on-LAN relay".into(),
                port: 9,
                proto: "udp".into(),
                overlay_ip: None,
                is_publishable: false,
            },
        ];
        let _ = p.view();
    }

    #[test]
    fn update_loaded_clears_busy_and_sets_summary() {
        let mut p = ServicePublishingPanel::new();
        p.busy = true;
        let _ = p.update(Message::Loaded {
            rows: vec![ServiceRow {
                id: "ssh".into(),
                name: "SSH".into(),
                port: 22,
                proto: "tcp".into(),
                overlay_ip: Some("10.42.0.5".into()),
                is_publishable: true,
            }],
            error: None,
        });
        assert!(!p.busy);
        assert!(p.last_op.contains("1 canonical"));
        assert!(p.last_run_at.is_some());
    }

    #[test]
    fn update_loaded_with_error_surfaces_message() {
        let mut p = ServicePublishingPanel::new();
        let _ = p.update(Message::Loaded {
            rows: Vec::new(),
            error: Some("mackesd not reachable over the Bus".into()),
        });
        assert_eq!(p.last_op, "mackesd not reachable over the Bus");
        assert!(p.rows.is_empty());
    }
}

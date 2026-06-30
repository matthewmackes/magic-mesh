//! Network → Mesh Join Status panel.
//!
//! Token-join (the v2.5 Nebula flow) has **no pending-approval step** — a valid
//! single-use join token IS the authorization, so there is no inbox of requests
//! to accept or deny. This panel therefore shows the live **join status**: every
//! node in the replicated peer directory, most recently-seen first, with its
//! overlay IP, role, and health. (It replaces the v1.x "pending pair requests"
//! approval queue, whose probe-cache source — `~/.cache/mde/peers/*/probe.json`
//! — was removed and whose Accept/Reject targeted a leader-ingest flow that was
//! never built.)

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::Task;
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::Element;
use cosmic::Theme;
use mackes_mesh_types::peers::PeerRecord;
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// One enrolled peer as shown in the join-status roster.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JoinedPeer {
    pub hostname: String,
    pub overlay_ip: String,
    pub role: String,
    pub health: String,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MeshPendingPanel {
    pub peers: Vec<JoinedPeer>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<JoinedPeer>),
    RefreshClicked,
}

impl MeshPendingPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                // fetch_peers does a blocking Bus round-trip — keep it off the
                // iced executor thread.
                let peers = tokio::task::spawn_blocking(crate::mesh_directory::fetch_peers)
                    .await
                    .unwrap_or_default();
                roster_from_peers(peers)
            },
            |roster| crate::Message::MeshPending(Message::Loaded(roster)),
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(peers) => {
                self.peers = peers;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Join Status")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if let Some(t) = self.last_run_at {
            format!(
                "{} peer{} · last refresh {}",
                self.peers.len(),
                if self.peers.len() == 1 { "" } else { "s" },
                fmt_age(t),
            )
        } else {
            "click Refresh to load the roster".to_string()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let refresh_btn = button(
            text(if self.busy { "Working…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty({
            let accent = palette.accent.into_cosmic_color();
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                let bg = match status {
                    cosmic::iced::widget::button::Status::Hovered => Color {
                        r: accent.r * 1.10,
                        g: accent.g * 1.10,
                        b: accent.b * 1.10,
                        a: accent.a,
                    },
                    _ => accent,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: Color::WHITE,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            }
        })
        .on_press(crate::Message::MeshPending(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut peers_col = column![].spacing(10);
        for p in &self.peers {
            peers_col = peers_col.push(peer_row(p, palette));
        }
        if self.peers.is_empty() && !self.busy {
            peers_col = peers_col.push(empty_state_card(palette));
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(peers_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// Project the replicated peer directory into the join-status roster, most
/// recently-seen first. Pure + testable (no Bus).
#[must_use]
pub fn roster_from_peers(peers: Vec<PeerRecord>) -> Vec<JoinedPeer> {
    let mut out: Vec<JoinedPeer> = peers
        .into_iter()
        .map(|p| JoinedPeer {
            hostname: p.hostname,
            overlay_ip: p.overlay_ip.unwrap_or_default(),
            role: p.role.unwrap_or_else(|| "peer".into()),
            health: p.health,
            last_seen_ms: p.last_seen_ms,
        })
        .collect();
    out.sort_by(|a, b| b.last_seen_ms.cmp(&a.last_seen_ms));
    out
}

fn peer_row<'a>(p: &'a JoinedPeer, palette: Palette) -> Element<'a, crate::Message> {
    let resolved = mde_icon(Icon::Peer, IconSize::PanelHeader);
    let icon_color = palette.accent.into_cosmic_color();
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(28.0))
            .height(Length::Fixed(28.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(28.0)
            .colr(icon_color)
            .into()
    };

    let hostname_text = text(p.hostname.clone())
        .size(14)
        .colr(palette.text.into_cosmic_color());
    let where_text = text(format!(
        "{} · {}",
        if p.overlay_ip.is_empty() {
            "(no overlay ip)"
        } else {
            p.overlay_ip.as_str()
        },
        p.role,
    ))
    .size(11)
    .colr(palette.text_muted.into_cosmic_color());

    let health_color = match p.health.as_str() {
        "healthy" => palette.success,
        "degraded" => palette.warning,
        "unreachable" => palette.danger,
        _ => palette.text_muted,
    }
    .into_cosmic_color();
    let status_text = text(format!("{} · seen {}", p.health, fmt_seen(p.last_seen_ms)))
        .size(11)
        .colr(health_color);

    let body = row![
        icon_widget,
        column![hostname_text, where_text, status_text].spacing(2),
        Space::new().width(Length::Fill),
    ]
    .spacing(12)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(body)
        .padding(Padding::from([12u16, 16u16]))
        .width(Length::Fill)
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn empty_state_card<'a>(palette: Palette) -> Element<'a, crate::Message> {
    let resolved = mde_icon(Icon::StatusOk, IconSize::PanelHeader);
    let icon_color = palette.success.into_cosmic_color();
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(32.0)
            .colr(icon_color)
            .into()
    };
    container(
        column![
            icon_widget,
            Space::new().height(Length::Fixed(8.0)),
            text("No peers in the directory yet")
                .size(14)
                .colr(palette.text.into_cosmic_color()),
            text(
                "Peers appear here as they enroll (Mesh Join → paste a join token). \
                 Token-join needs no approval — a valid single-use token is the \
                 authorization.",
            )
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2)
        .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

// ---- helpers --------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Coarse "… ago" string for a unix-ms last-seen timestamp.
fn fmt_seen(last_seen_ms: u64) -> String {
    let secs = now_ms().saturating_sub(last_seen_ms) / 1000;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

fn fmt_age(t: SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} s ago")
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(host: &str, health: &str, last_seen_ms: u64) -> PeerRecord {
        let mut r = PeerRecord::now(host, None, health);
        r.last_seen_ms = last_seen_ms;
        r
    }

    #[test]
    fn roster_sorts_most_recently_seen_first() {
        let roster = roster_from_peers(vec![
            peer("old", "healthy", 1_000),
            peer("new", "degraded", 9_000),
        ]);
        assert_eq!(roster[0].hostname, "new");
        assert_eq!(roster[0].health, "degraded");
        assert_eq!(roster[1].hostname, "old");
    }

    #[test]
    fn roster_defaults_role_and_overlay_when_absent() {
        let roster = roster_from_peers(vec![peer("anvil", "healthy", 5_000)]);
        assert_eq!(roster[0].role, "peer");
        assert_eq!(roster[0].overlay_ip, "");
    }

    #[test]
    fn roster_carries_overlay_and_role_when_present() {
        let mut r = peer("lh", "healthy", 5_000);
        r.overlay_ip = Some("10.42.0.1".into());
        r.role = Some("lighthouse".into());
        let roster = roster_from_peers(vec![r]);
        assert_eq!(roster[0].overlay_ip, "10.42.0.1");
        assert_eq!(roster[0].role, "lighthouse");
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = MeshPendingPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_peers_without_panic() {
        let mut p = MeshPendingPanel::new();
        p.peers = roster_from_peers(vec![peer("anvil", "healthy", 5_000)]);
        let _ = p.view();
    }
}

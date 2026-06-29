//! v4.0.1 WB-2.i — Network → Mesh Pending panel.
//!
//! Lists peer-probe JSON entries that `mackesd` has cached at
//! `$XDG_CACHE_HOME/mde/peers/<peer-id>/probe.json` (the
//! `peer_join::write_probe` landing spot). Each row is treated
//! as a pending pair-request: the operator clicks Accept to
//! shell `mackesd enroll <peer-id>`, or Reject to delete the
//! probe file. When the daemon ships a real "pair-request
//! queue" abstraction later, this panel switches its source
//! over without touching the UI shape.
//!
//! Chrome influence: Win11 Settings → Bluetooth & devices →
//! Add device flow (cards-with-accept-reject pattern).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::Task;
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::Element;
use cosmic::Theme;
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingPeer {
    pub peer_id: String,
    pub hostname: String,
    pub distro: String,
    pub mded_version: String,
    pub rtt_ms: u32,
    /// Path to the cached probe.json — used by the reject
    /// button to delete the file.
    pub probe_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct MeshPendingPanel {
    pub peers: Vec<PendingPeer>,
    pub busy: bool,
    pub last_op: String,
    pub last_run_at: Option<SystemTime>,
    /// MOTION-TRANS-3 — the keyed-diff reveal: a pending peer that just appeared
    /// on a refresh slides up + fades into the list, while rows already on screen
    /// stay put (the roster doesn't restroke). Keyed by `peer_id`.
    reveal: mde_theme::animation::KeyedListReveal,
}

/// MOTION-TRANS-3 — stable scrollable id so the pending-peer list keeps its scroll
/// position across a refresh (the roster doesn't jump when a row is added/removed).
const LIST_ID: &str = "mesh-pending-list";

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<PendingPeer>),
    /// MOTION-TRANS-3 — advance the row-insert reveal one frame (in-flight-only
    /// tick; GC's settled reveals so it self-stops at rest).
    AnimTick,
    RefreshClicked,
    AcceptClicked(String),
    RejectClicked {
        peer_id: String,
        probe_path: PathBuf,
    },
    OpFinished {
        peer_id: String,
        op: String,
        success: bool,
    },
}

impl MeshPendingPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            reveal: mde_theme::animation::KeyedListReveal::new(crate::live_theme::reduce_motion()),
            ..Self::default()
        }
    }

    /// MOTION-TRANS-3 — does the row-insert reveal still have a frame to draw? The
    /// app gates the panel's per-frame tick subscription on this (idle ⇒ no tick).
    #[must_use]
    pub fn needs_tick(&self, now: std::time::Instant) -> bool {
        !self.reveal.is_idle(now)
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { scan_pending_probes() }, |peers| {
            crate::Message::MeshPending(Message::Loaded(peers))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(peers) => {
                // MOTION-TRANS-3 — diff the freshly-loaded roster against the last
                // frame so any newly-appeared pending peer reveals in (the first
                // load is treated as the list appearing — no mass reveal).
                self.reveal.sync(
                    peers.iter().map(|p| p.peer_id.clone()),
                    std::time::Instant::now(),
                );
                self.peers = peers;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            // MOTION-TRANS-3 — drop settled reveals so the tick subscription stops.
            Message::AnimTick => {
                self.reveal.gc(std::time::Instant::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
            Message::AcceptClicked(peer_id) => {
                self.busy = true;
                self.last_op = format!("enrolling {peer_id}…");
                let id = peer_id.clone();
                Task::perform(
                    async move {
                        let ok = run_mackesd_enroll(&id).await;
                        (id, "enroll".to_string(), ok)
                    },
                    |(peer_id, op, success)| {
                        crate::Message::MeshPending(Message::OpFinished {
                            peer_id,
                            op,
                            success,
                        })
                    },
                )
            }
            Message::RejectClicked {
                peer_id,
                probe_path,
            } => {
                self.busy = true;
                self.last_op = format!("rejecting {peer_id}…");
                let id = peer_id.clone();
                Task::perform(
                    async move {
                        let ok = std::fs::remove_file(&probe_path).is_ok();
                        (id, "reject".to_string(), ok)
                    },
                    |(peer_id, op, success)| {
                        crate::Message::MeshPending(Message::OpFinished {
                            peer_id,
                            op,
                            success,
                        })
                    },
                )
            }
            Message::OpFinished {
                peer_id,
                op,
                success,
            } => {
                self.last_op = if success {
                    format!("{op} {peer_id}: ok")
                } else {
                    format!("{op} {peer_id}: FAILED")
                };
                self.busy = false;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Mesh Pending")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if !self.last_op.is_empty() {
            self.last_op.clone()
        } else if let Some(t) = self.last_run_at {
            format!("last refresh {}", fmt_age(t))
        } else {
            format!(
                "{} pending request{}",
                self.peers.len(),
                if self.peers.len() == 1 { "" } else { "s" }
            )
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

        // MOTION-TRANS-3 — a single clock read for this frame's reveal sampling.
        let now = std::time::Instant::now();
        let mut peers_col = column![].spacing(10);
        for p in &self.peers {
            // A freshly-inserted row starts a few px low and rises to rest, applied
            // as decaying top padding (iced 0.13 has no transform widget — the
            // translate-as-padding idiom the launcher/Hub/sidebar share). `0` once
            // settled, so the resting layout is unchanged.
            let slide = self.reveal.row_params(&p.peer_id, now).translate_y.max(0.0);
            peers_col = peers_col.push(container(peer_row(p, palette)).padding(Padding {
                top: slide,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            }));
        }
        if self.peers.is_empty() && !self.busy {
            peers_col = peers_col.push(empty_state_card(palette));
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                // MOTION-TRANS-3 — a stable id so a refresh keeps the scroll
                // position (the roster doesn't jump when a row is added/removed).
                scrollable(peers_col)
                    .id(cosmic::iced::widget::Id::new(LIST_ID))
                    .height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn peer_row<'a>(p: &'a PendingPeer, palette: Palette) -> Element<'a, crate::Message> {
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
    let id_text = text(p.peer_id.clone())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());
    let distro_text = text(format!(
        "{} · mded {} · {} ms",
        p.distro, p.mded_version, p.rtt_ms
    ))
    .size(11)
    .colr(palette.text_muted.into_cosmic_color());

    let accept_btn = action_btn("Accept", palette, false).on_press(crate::Message::MeshPending(
        Message::AcceptClicked(p.peer_id.clone()),
    ));
    let reject_btn = action_btn("Reject", palette, true).on_press(crate::Message::MeshPending(
        Message::RejectClicked {
            peer_id: p.peer_id.clone(),
            probe_path: p.probe_path.clone(),
        },
    ));

    let body = row![
        icon_widget,
        column![hostname_text, id_text, distro_text].spacing(2),
        Space::new().width(Length::Fill),
        accept_btn,
        reject_btn,
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
            text("No pending pair requests")
                .size(14)
                .colr(palette.text.into_cosmic_color()),
            text(
                "When a peer initiates a pair request mackesd caches its probe under \
                 ~/.cache/mde/peers/<peer-id>/probe.json; rows appear here.",
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

fn action_btn<'a>(
    label: &'a str,
    palette: Palette,
    ghost: bool,
) -> cosmic::iced::widget::Button<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let danger = palette.danger.into_cosmic_color();
    button(
        text(label)
            .size(11)
            .colr(if ghost { danger } else { Color::WHITE }),
    )
    .padding(Padding::from([4u16, 14u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            let (bg, fg) = if ghost {
                // Faint danger tint on hover for the destructive ghost button.
                let hover_bg = Color { a: 0.12, ..danger };
                match status {
                    cosmic::iced::widget::button::Status::Hovered => (hover_bg, danger),
                    _ => (Color::TRANSPARENT, danger),
                }
            } else {
                let bg = match status {
                    cosmic::iced::widget::button::Status::Hovered => Color {
                        r: accent.r * 1.10,
                        g: accent.g * 1.10,
                        b: accent.b * 1.10,
                        a: accent.a,
                    },
                    _ => accent,
                };
                (bg, Color::WHITE)
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: fg,
                border: Border {
                    color: if ghost { danger } else { Color::TRANSPARENT },
                    width: if ghost { 1.0 } else { 0.0 },
                    radius: 4.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..cosmic::iced::widget::button::Style::default()
            }
        },
    )
}

// ---- I/O ------------------------------------------------------

#[must_use]
pub fn scan_pending_probes() -> Vec<PendingPeer> {
    let Some(root) = pending_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let peer_dir = entry.path();
        let probe_path = peer_dir.join("probe.json");
        if let Some(p) = read_probe(&probe_path) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    out
}

fn pending_root() -> Option<PathBuf> {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    Some(base.join("mde").join("peers"))
}

fn read_probe(path: &Path) -> Option<PendingPeer> {
    let raw = std::fs::read_to_string(path).ok()?;
    parse_probe(&raw, path)
}

/// Pure parser — exposed for tests + to keep the I/O wrapper
/// thin. Extracts the subset of `PeerProbe` fields the panel
/// displays; ignores everything else.
#[must_use]
pub fn parse_probe(raw: &str, probe_path: &Path) -> Option<PendingPeer> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let peer_id = v.get("peer_id")?.as_str()?.to_string();
    let hostname = v
        .get("hostname")
        .and_then(|x| x.as_str())
        .unwrap_or(&peer_id)
        .to_string();
    let distro = v
        .get("distro")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let mded_version = v
        .pointer("/kernel_driver/mded_version")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let rtt_ms = v
        .pointer("/bus_topology/rtt_ms")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    Some(PendingPeer {
        peer_id,
        hostname,
        distro,
        mded_version,
        rtt_ms,
        probe_path: probe_path.to_path_buf(),
    })
}

pub async fn run_mackesd_enroll(peer_id: &str) -> bool {
    use tokio::process::Command;
    let status = Command::new("mackesd")
        .args(["enroll", peer_id])
        .status()
        .await;
    status.map(|s| s.success()).unwrap_or(false)
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

    #[test]
    fn parse_probe_decodes_minimum_required_fields() {
        let raw = r#"{
            "peer_id": "peer:abcd1234",
            "hostname": "anvil",
            "distro": "Fedora 44",
            "vendor_id": "8086",
            "product_id": "5916",
            "kernel_driver": {
                "uname": "6.7.0",
                "transport_module": "tcp",
                "mded_version": "4.0.0",
                "dmesg_tail": []
            },
            "bus_topology": {
                "mesh_path": [],
                "rtt_ms": 42,
                "nat_class": "Open",
                "ice_candidate": "",
                "pci_tree": [],
                "usb_tree": []
            }
        }"#;
        let probe_path = Path::new("/tmp/peer/probe.json");
        let p = parse_probe(raw, probe_path).expect("decoded");
        assert_eq!(p.peer_id, "peer:abcd1234");
        assert_eq!(p.hostname, "anvil");
        assert_eq!(p.distro, "Fedora 44");
        assert_eq!(p.mded_version, "4.0.0");
        assert_eq!(p.rtt_ms, 42);
        assert_eq!(p.probe_path, probe_path);
    }

    #[test]
    fn parse_probe_returns_none_without_peer_id() {
        let raw = r#"{"hostname": "anvil"}"#;
        assert!(parse_probe(raw, Path::new("/tmp/probe.json")).is_none());
    }

    #[test]
    fn parse_probe_uses_peer_id_as_fallback_hostname() {
        let raw = r#"{"peer_id": "peer:only-id"}"#;
        let p = parse_probe(raw, Path::new("/tmp/p.json")).expect("decoded");
        assert_eq!(p.hostname, "peer:only-id");
    }

    #[test]
    fn parse_probe_returns_none_for_garbage() {
        assert!(parse_probe("not json", Path::new("/x")).is_none());
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = MeshPendingPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_pending_peer_without_panic() {
        let mut p = MeshPendingPanel::new();
        p.peers = vec![PendingPeer {
            peer_id: "peer:abc".into(),
            hostname: "anvil".into(),
            distro: "Fedora 44".into(),
            mded_version: "4.0.0".into(),
            rtt_ms: 42,
            probe_path: PathBuf::from("/tmp/probe.json"),
        }];
        let _ = p.view();
    }

    fn peer(id: &str) -> PendingPeer {
        PendingPeer {
            peer_id: id.into(),
            hostname: id.into(),
            ..PendingPeer::default()
        }
    }

    #[test]
    fn first_load_seeds_without_revealing_then_an_insert_reveals() {
        // MOTION-TRANS-3 — opening the panel (first Loaded) seeds the roster with
        // no mass reveal; a later refresh that adds a peer reveals it, so the
        // panel reports it needs a per-frame tick.
        let mut p = MeshPendingPanel::new();
        let now = std::time::Instant::now();
        let _ = p.update(Message::Loaded(vec![peer("a"), peer("b")]));
        assert!(
            !p.needs_tick(now),
            "the initial roster must not mass-reveal (no tick needed)"
        );
        // A refresh that adds "c" arms its reveal → a tick is needed.
        let _ = p.update(Message::Loaded(vec![peer("a"), peer("b"), peer("c")]));
        assert!(
            p.needs_tick(std::time::Instant::now()),
            "an inserted pending peer reveals → the panel needs a tick"
        );
    }

    #[test]
    fn anim_tick_settles_the_reveal_and_stops_the_clock() {
        // MOTION-TRANS-3 — once the reveal's panel-mount duration has elapsed,
        // an AnimTick GC's it and the panel goes idle (the subscription stops).
        let mut p = MeshPendingPanel::new();
        let _ = p.update(Message::Loaded(vec![peer("a")]));
        let _ = p.update(Message::Loaded(vec![peer("a"), peer("b")]));
        // Far enough in the future that the reveal has settled.
        let _ = p.update(Message::AnimTick);
        let later = std::time::Instant::now() + mde_theme::motion::Motion::panel_mount().duration;
        assert!(
            !p.needs_tick(later),
            "a settled reveal needs no further ticks"
        );
    }
}

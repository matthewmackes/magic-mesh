//! v4.0.1 WB-2.k — Network → Mesh Topology panel.
//!
//! Tabular alternative to the canvas-graph version the original
//! worklist spec described. The canvas widget chains on either
//! a substantial iced::Canvas integration or a cairo bridge;
//! the operator's "what peers does this machine know about, and
//! how reachable are they?" question is fully answered by a
//! sortable table. Shipping the table now closes WB-2.k as
//! useful work; the canvas variant remains a v4.1+ polish task
//! (captured below as WB-2.k.a).
//!
//! Data source: `mackesd Fleet.Files.Peers` via the same
//! shell-out path the workbench already uses for Mesh Pending
//! (avoids a fresh settings-backend dep in mde-workbench). Empty
//! when mackesd isn't on the bus or no peers are enrolled —
//! that's the honest state; the panel says so.
//!
//! Chrome influence (Phase 0.8): Win11 Settings → Bluetooth &
//! devices "All devices" tabular view.

use std::f32::consts::TAU;
use std::time::SystemTime;

use iced::widget::canvas::{self, Canvas, Frame, Path, Stroke, Text};
use iced::widget::{button, column, container, row, scrollable, stack, text, Space};
use iced::{
    Background, Border, Color, Element, Length, Padding, Point, Rectangle, Renderer, Task, Theme,
};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, ObjectCard, Palette, TypeRole, CARD_GRID_GAP};

use crate::panel_chrome::object_card;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerStatus {
    Online,
    Idle,
    Offline,
    Unknown,
}

impl PeerStatus {
    fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "online" | "healthy" => Self::Online,
            "idle" | "degraded" => Self::Idle,
            "offline" | "unreachable" => Self::Offline,
            _ => Self::Unknown,
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Online => Icon::StatusOk,
            Self::Idle => Icon::StatusWarning,
            Self::Offline => Icon::StatusError,
            Self::Unknown => Icon::StatusUnknown,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Online => "ONLINE",
            Self::Idle => "IDLE",
            Self::Offline => "OFFLINE",
            Self::Unknown => "UNKNOWN",
        }
    }
    // CR-6.b — re-introduced (dropped by CR-6 Table-only refactor).
    // Graph canvas uses this directly; Table uses icon() instead.
    pub fn color(self) -> Color {
        match self {
            Self::Online => Color::from_rgb(0.20, 0.80, 0.40),
            Self::Idle => Color::from_rgb(0.95, 0.70, 0.20),
            Self::Offline => Color::from_rgb(0.92, 0.32, 0.30),
            Self::Unknown => Color::from_rgba(0.60, 0.60, 0.60, 0.80),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRow {
    pub name: String,
    pub addr: String,
    pub kind: String,
    pub status: PeerStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Table,
    Graph,
}

#[derive(Debug, Clone, Default)]
pub struct MeshTopologyPanel {
    pub peers: Vec<PeerRow>,
    pub error: Option<String>,
    pub last_run_at: Option<SystemTime>,
    pub busy: bool,
    pub layout: Layout,
    /// CR-6.c — peer name of the currently-open Peer Connection Card modal.
    pub peer_modal: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<PeerRow>, String>),
    RefreshClicked,
    SetLayout(Layout),
    /// CR-6.c — open the Peer Connection Card modal for `peer_name`.
    OpenPeerModal(String),
    /// CR-6.c — close the Peer Connection Card modal.
    CloseModal,
}

impl MeshTopologyPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_peers() }, |result| {
            crate::Message::MeshTopology(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(peers)) => {
                self.peers = peers;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.peers = Vec::new();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
            Message::SetLayout(l) => {
                self.layout = l;
                Task::none()
            }
            Message::OpenPeerModal(name) => {
                self.peer_modal = Some(name);
                Task::none()
            }
            Message::CloseModal => {
                self.peer_modal = None;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = Palette::dark();
        let sizes = FontSize::defaults();

        let title = text("Mesh Topology")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        let subtitle_text = if let Some(t) = self.last_run_at {
            format!(
                "{} peer{} · last refresh {}",
                self.peers.len(),
                if self.peers.len() == 1 { "" } else { "s" },
                fmt_age(t)
            )
        } else {
            "click Refresh to probe".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
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
        .on_press(crate::Message::MeshTopology(Message::RefreshClicked));

        let table_btn = layout_toggle_btn("Table", self.layout == Layout::Table, palette).on_press(
            crate::Message::MeshTopology(Message::SetLayout(Layout::Table)),
        );
        let graph_btn = layout_toggle_btn("Graph", self.layout == Layout::Graph, palette).on_press(
            crate::Message::MeshTopology(Message::SetLayout(Layout::Graph)),
        );

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            table_btn,
            Space::new().width(Length::Fixed(4.0)),
            graph_btn,
            Space::new().width(Length::Fixed(8.0)),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let body_element: Element<'_, crate::Message> = match self.layout {
            Layout::Table => {
                // CR-6 (2026-05-25): peers render as Object Cards
                // (CardSize::Medium) per the Classic ChromeOS
                // visual lock. The canvas-graph customization
                // (peer nodes drawn as cards inside the graph)
                // is tracked as CR-6.b.
                let mut rows_col = column![].spacing(CARD_GRID_GAP as f32);
                for p in &self.peers {
                    rows_col = rows_col.push(peer_object_card(p, palette));
                }
                if self.peers.is_empty() && self.last_run_at.is_some() {
                    rows_col = rows_col.push(empty_state_card(palette, self.error.as_deref()));
                }
                scrollable(rows_col).height(Length::Fill).into()
            }
            Layout::Graph => {
                if self.peers.is_empty() {
                    empty_state_card(palette, self.error.as_deref())
                } else {
                    Canvas::new(GraphProgram {
                        peers: self.peers.clone(),
                        palette,
                    })
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
                }
            }
        };

        let footer_text = match self.layout {
            Layout::Table => "Inter-peer latency matrix is not yet collected. Mackesd needs a peer-mesh sniffer to populate the missing edges; tracked as AF-NET-2 follow-up.",
            Layout::Graph => "Graph shows the local node at center + each enrolled peer arrayed around it. Edges + thickness will reflect inter-peer latency when AF-NET-2 ships.",
        };
        let footer = text(footer_text)
            .size(10)
            .color(palette.text_muted.into_iced_color());

        let base = container(
            column![
                header,
                Space::new().height(Length::Fixed(16.0)),
                body_element,
                Space::new().height(Length::Fixed(8.0)),
                footer,
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill);

        // CR-6.c — layer the Peer Connection Card modal on top when open.
        if let Some(ref peer_name) = self.peer_modal {
            if let Some(p) = self.peers.iter().find(|r| r.name == *peer_name) {
                return stack![base, peer_modal_overlay(p, palette)].into();
            }
        }

        base.into()
    }
}

fn layout_toggle_btn<'a>(
    label: &'a str,
    selected: bool,
    palette: Palette,
) -> iced::widget::Button<'a, crate::Message> {
    let accent = palette.accent.into_iced_color();
    let text_main = palette.text.into_iced_color();
    let text_muted = palette.text_muted.into_iced_color();
    iced::widget::button(text(label).size(11).color(if selected {
        Color::WHITE
    } else {
        text_muted
    }))
    .padding(Padding::from([3u16, 10u16]))
    .style(move |_t: &Theme, status: iced::widget::button::Status| {
        let bg = if selected {
            accent
        } else {
            match status {
                iced::widget::button::Status::Hovered => Color {
                    r: 0.15,
                    g: 0.15,
                    b: 0.17,
                    a: 1.0,
                },
                _ => Color::TRANSPARENT,
            }
        };
        iced::widget::button::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            text_color: if selected { Color::WHITE } else { text_main },
            border: Border {
                color: if selected {
                    Color::TRANSPARENT
                } else {
                    Color {
                        a: 0.20,
                        ..Color::WHITE
                    }
                },
                width: if selected { 0.0 } else { 1.0 },
                radius: 4.0.into(),
            },
            shadow: iced::Shadow::default(),
        }
    })
}

/// CR-6.b — canvas rounded-rect path centered at (cx, cy).
/// Uses cubic-bezier arc approximation (k ≈ 0.5523) so only
/// `move_to` / `line_to` / `bezier_curve_to` / `close` are needed.
fn card_path(cx: f32, cy: f32, w: f32, h: f32, r: f32) -> Path {
    let x = cx - w * 0.5;
    let y = cy - h * 0.5;
    let k = r * 0.5523; // bezier magic number for a 90° arc
    Path::new(|b| {
        b.move_to(Point::new(x + r, y));
        b.line_to(Point::new(x + w - r, y));
        b.bezier_curve_to(
            Point::new(x + w - r + k, y),
            Point::new(x + w, y + r - k),
            Point::new(x + w, y + r),
        );
        b.line_to(Point::new(x + w, y + h - r));
        b.bezier_curve_to(
            Point::new(x + w, y + h - r + k),
            Point::new(x + w - r + k, y + h),
            Point::new(x + w - r, y + h),
        );
        b.line_to(Point::new(x + r, y + h));
        b.bezier_curve_to(
            Point::new(x + r - k, y + h),
            Point::new(x, y + h - r + k),
            Point::new(x, y + h - r),
        );
        b.line_to(Point::new(x, y + r));
        b.bezier_curve_to(
            Point::new(x, y + r - k),
            Point::new(x + r - k, y),
            Point::new(x + r, y),
        );
        b.close();
    })
}

/// WB-2.k.a (2026-05-23) — canvas program that draws the mesh
/// graph: local node at center as a filled circle, each peer
/// arrayed around it in a ring with a connecting edge.
/// CR-6.b (2026-05-30) — peers now rendered as 12 px card-shaped
/// nodes (shadow + surface fill + status dot + name + label) instead
/// of circles/diamonds. Edge thickness is uniform for now.
pub struct GraphProgram {
    pub peers: Vec<PeerRow>,
    pub palette: Palette,
}

impl<Message> canvas::Program<Message> for GraphProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let center = Point::new(size.width / 2.0, size.height / 2.0);

        // Empty state guard (view() already short-circuits but
        // be defensive).
        if self.peers.is_empty() {
            return vec![frame.into_geometry()];
        }

        // CR-6.b: card nodes are 108×48 px — ring radius must be large
        // enough that cards don't overlap at the max 8-peer cap.
        // 0.38 of the shorter canvas dimension, floor 160 px.
        let ring_radius = (size.width.min(size.height) * 0.38).max(160.0);
        let n = self.peers.len() as f32;
        let center_radius = 28.0;
        const CARD_W: f32 = 108.0;
        const CARD_H: f32 = 48.0;
        const CARD_R: f32 = 12.0;
        let edge_color = self.palette.border.into_iced_color();
        let text_color = self.palette.text.into_iced_color();
        let muted = self.palette.text_muted.into_iced_color();
        let accent = self.palette.accent.into_iced_color();
        // Card body fill — Classic ChromeOS surface-2 token via palette.
        let card_surface = self.palette.surface.into_iced_color();

        // Draw edges first (so cards render on top).
        for i in 0..self.peers.len() {
            let angle = (i as f32 / n) * TAU - std::f32::consts::FRAC_PI_2;
            let px = center.x + angle.cos() * ring_radius;
            let py = center.y + angle.sin() * ring_radius;
            let edge = Path::line(center, Point::new(px, py));
            frame.stroke(
                &edge,
                Stroke {
                    style: canvas::Style::Solid(edge_color),
                    width: 1.5,
                    ..Stroke::default()
                },
            );
        }

        // Draw center (local) node — filled accent circle; stays circular
        // to visually distinguish the local machine from the peer cards.
        let center_circle = Path::circle(center, center_radius);
        frame.fill(&center_circle, accent);
        let center_label = Text {
            content: "self".to_string(),
            position: center,
            color: Color::WHITE,
            size: 12.0.into(),
            font: iced::Font::DEFAULT,
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..Text::default()
        };
        frame.fill_text(center_label);

        // CR-6.b — Draw peers as card-shaped nodes (12 px corners,
        // shadow, status dot + name + label inside).
        // NF-11.2 lighthouse distinction migrated from diamond to
        // accent border stroke on the card.
        for (i, p) in self.peers.iter().enumerate() {
            let angle = (i as f32 / n) * TAU - std::f32::consts::FRAC_PI_2;
            let pos = Point::new(
                center.x + angle.cos() * ring_radius,
                center.y + angle.sin() * ring_radius,
            );

            // Shadow — slightly larger card, offset 2×3 px, 22% alpha.
            let shadow = card_path(pos.x + 2.0, pos.y + 3.0, CARD_W + 3.0, CARD_H + 3.0, CARD_R);
            frame.fill(&shadow, Color::from_rgba(0.0, 0.0, 0.0, 0.22));

            // Card body.
            let card = card_path(pos.x, pos.y, CARD_W, CARD_H, CARD_R);
            frame.fill(&card, card_surface);

            // NF-11.2 — lighthouse (host-kind) gets accent border.
            if p.kind == "host" {
                frame.stroke(
                    &card,
                    Stroke {
                        style: canvas::Style::Solid(accent),
                        width: 2.0,
                        ..Stroke::default()
                    },
                );
            }

            // Status dot — 5 px circle, top-left inside card.
            let dot = Path::circle(
                Point::new(pos.x - CARD_W * 0.5 + 14.0, pos.y - CARD_H * 0.5 + 14.0),
                5.0,
            );
            frame.fill(&dot, p.status.color());

            // Peer name (13 px, leading edge offset right of dot).
            let name_label = Text {
                content: p.name.clone(),
                position: Point::new(pos.x + 6.0, pos.y - 7.0),
                color: text_color,
                size: 12.0.into(),
                font: iced::Font::DEFAULT,
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Center,
                ..Text::default()
            };
            frame.fill_text(name_label);

            // Status label (10 px, muted, below name).
            let status_label = Text {
                content: p.status.label().to_string(),
                position: Point::new(pos.x + 6.0, pos.y + 9.0),
                color: muted,
                size: 10.0.into(),
                font: iced::Font::DEFAULT,
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Center,
                ..Text::default()
            };
            frame.fill_text(status_label);
        }

        vec![frame.into_geometry()]
    }
}

/// CR-6 — render a peer as a Material Object Card at
/// `CardSize::Medium`. Status icon drives the leading glyph;
/// title is the peer name; subtitle is the peer reachability
/// label (`ONLINE` / `IDLE` / `OFFLINE` / `UNKNOWN`).
///
/// CR-6.c — card wrapped in a transparent button; clicking opens
/// the Peer Connection Card modal (addr + kind detail surface).
/// Addr + kind metadata stays accessible via that modal per the
/// `chromeos-classic-spec.md` §Object Cards "compact content
/// shape" lock (round-4 re-ask 2026-05-24).
fn peer_object_card<'a>(p: &'a PeerRow, palette: Palette) -> Element<'a, crate::Message> {
    let card = ObjectCard::medium(p.status.icon(), p.name.clone(), p.status.label());
    let card_el = object_card(card, palette);
    button(card_el)
        .on_press(crate::Message::MeshTopology(Message::OpenPeerModal(
            p.name.clone(),
        )))
        .padding(Padding::from(0u16))
        .style(
            |_t: &Theme, _s: iced::widget::button::Status| iced::widget::button::Style {
                snap: false,
                background: None,
                text_color: Color::TRANSPARENT,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                shadow: iced::Shadow::default(),
            },
        )
        .into()
}

/// CR-6.c — overlay rendered on top of the panel (via `stack!`)
/// when a peer card is clicked. Shows addr + kind demoted from
/// the compact card front. Close button dismisses; Esc key wiring
/// is a release-bench item (requires top-level keyboard subscription).
fn peer_modal_overlay<'a>(p: &'a PeerRow, palette: Palette) -> Element<'a, crate::Message> {
    let close_btn = button(text("✕").size(13).color(palette.text.into_iced_color()))
        .on_press(crate::Message::MeshTopology(Message::CloseModal))
        .padding(Padding::from([2u16, 8u16]))
        .style(|_t: &Theme, status: iced::widget::button::Status| {
            let bg = match status {
                iced::widget::button::Status::Hovered => Some(Background::Color(Color {
                    a: 0.10,
                    ..Color::WHITE
                })),
                _ => None,
            };
            iced::widget::button::Style {
                snap: false,
                background: bg,
                text_color: Color::TRANSPARENT,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 4.0.into(),
                },
                shadow: iced::Shadow::default(),
            }
        });

    let modal_card = container(
        column![
            row![
                text(p.name.clone())
                    .size(16)
                    .color(palette.text.into_iced_color()),
                Space::new().width(Length::Fill),
                close_btn,
            ]
            .align_y(iced::alignment::Vertical::Center),
            Space::new().height(Length::Fixed(16.0)),
            peer_detail_row("Address", &p.addr, palette),
            Space::new().height(Length::Fixed(8.0)),
            peer_detail_row("Kind", &p.kind, palette),
            Space::new().height(Length::Fixed(8.0)),
            peer_detail_row("Status", p.status.label(), palette),
        ]
        .spacing(0),
    )
    .padding(Padding::from([24u16, 28u16]))
    .width(Length::Fixed(320.0))
    .style(move |_t: &Theme| container::Style {
        snap: false,
        background: Some(Background::Color(palette.surface.into_iced_color())),
        border: Border {
            color: palette.border.into_iced_color(),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    });

    container(
        container(modal_card)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_t: &Theme| container::Style {
        snap: false,
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.45))),
        ..container::Style::default()
    })
    .into()
}

fn peer_detail_row<'a>(
    label: &'a str,
    value: &'a str,
    palette: Palette,
) -> Element<'a, crate::Message> {
    row![
        text(label)
            .size(12)
            .color(palette.text_muted.into_iced_color())
            .width(Length::Fixed(80.0)),
        text(value).size(12).color(palette.text.into_iced_color()),
    ]
    .spacing(8)
    .into()
}

fn empty_state_card<'a>(palette: Palette, error: Option<&'a str>) -> Element<'a, crate::Message> {
    let (icon_kind, icon_color, heading, body): (Icon, Color, String, String) = if let Some(err) =
        error
    {
        (
            Icon::StatusError,
            Color::from_rgb(0.92, 0.32, 0.30),
            "Couldn't load peers".to_string(),
            err.to_string(),
        )
    } else {
        (
                Icon::Fleet,
                palette.accent.into_iced_color(),
                "No peers enrolled".to_string(),
                "Enroll peers via mackes/birthright or mackesd's pair-request flow; rows appear here as mackesd's nodes table grows.".to_string(),
            )
    };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(icon_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(32.0)
            .color(icon_color)
            .into()
    };
    container(
        column![
            icon_widget,
            Space::new().height(Length::Fixed(8.0)),
            text(heading).size(14).color(palette.text.into_iced_color()),
            text(body)
                .size(11)
                .color(palette.text_muted.into_iced_color()),
        ]
        .spacing(2)
        .align_x(iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

// ---- I/O ------------------------------------------------------

/// Shell out to `mackesd nodes list --json` (or
/// fall back to other CLI paths if that one isn't present).
/// Returns Err with the spawn error message on failure.
pub fn fetch_peers() -> Result<Vec<PeerRow>, String> {
    // mackesd ships `nodes list --json`. Older builds may
    // expose it differently; the JSON shape is what matters.
    let out = std::process::Command::new("mackesd")
        .args(["nodes", "list", "--json"])
        .output()
        .map_err(|e| format!("mackesd nodes list failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd nodes list exited non-zero: {stderr}"));
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    Ok(parse_nodes(&raw))
}

/// Pure parser for `mackesd nodes list --json`'s JSON-array
/// output. Each entry has `{node_id, name, public_key, role,
/// health, region}` per `mackesd_core::store::NodeRow`.
#[must_use]
pub fn parse_nodes(raw: &str) -> Vec<PeerRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in top {
        let node_id = entry.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(node_id);
        let region = entry.get("region").and_then(|v| v.as_str()).unwrap_or("—");
        let role = entry.get("role").and_then(|v| v.as_str()).unwrap_or("peer");
        let health = entry
            .get("health")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if node_id.is_empty() {
            continue;
        }
        out.push(PeerRow {
            name: name.to_string(),
            addr: region.to_string(),
            kind: role.to_string(),
            status: PeerStatus::from_str(health),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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
    } else {
        format!("{} h ago", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_status_color_is_status_distinct() {
        // CR-6.b — PeerStatus::color() re-introduced; verify
        // each variant produces a visually distinct dominant channel.
        let online = PeerStatus::Online.color();
        let idle = PeerStatus::Idle.color();
        let offline = PeerStatus::Offline.color();
        let unknown = PeerStatus::Unknown.color();
        // Online is greenish.
        assert!(
            online.g > online.r && online.g > online.b,
            "online should be green-dominant"
        );
        // Idle is yellowish (high R + G, low B).
        assert!(
            idle.r > idle.b && idle.g > idle.b,
            "idle should be yellow-dominant"
        );
        // Offline is reddish.
        assert!(
            offline.r > offline.g && offline.r > offline.b,
            "offline should be red-dominant"
        );
        // Unknown is grey (equal-ish channels).
        assert!(
            (unknown.r - unknown.g).abs() < 0.05,
            "unknown should be grey"
        );
        // All four are distinct from each other.
        assert_ne!(online, idle);
        assert_ne!(online, offline);
        assert_ne!(idle, offline);
        assert_ne!(offline, unknown);
    }

    #[test]
    fn peer_status_from_str_known_values() {
        assert_eq!(PeerStatus::from_str("online"), PeerStatus::Online);
        assert_eq!(PeerStatus::from_str("HEALTHY"), PeerStatus::Online);
        assert_eq!(PeerStatus::from_str("idle"), PeerStatus::Idle);
        assert_eq!(PeerStatus::from_str("degraded"), PeerStatus::Idle);
        assert_eq!(PeerStatus::from_str("offline"), PeerStatus::Offline);
        assert_eq!(PeerStatus::from_str("unreachable"), PeerStatus::Offline);
        assert_eq!(PeerStatus::from_str("???"), PeerStatus::Unknown);
    }

    #[test]
    fn parse_nodes_decodes_array() {
        let raw = r#"[
            {"node_id": "peer:pine", "name": "pine", "public_key": "k1",
             "role": "peer", "health": "healthy", "region": "us-west"},
            {"node_id": "peer:birch", "name": "birch", "public_key": "k2",
             "role": "host", "health": "degraded", "region": null}
        ]"#;
        let rows = parse_nodes(raw);
        assert_eq!(rows.len(), 2);
        // Sorted lexicographically by name.
        assert_eq!(rows[0].name, "birch");
        assert_eq!(rows[0].status, PeerStatus::Idle);
        assert_eq!(rows[0].addr, "—");
        assert_eq!(rows[1].name, "pine");
        assert_eq!(rows[1].status, PeerStatus::Online);
        assert_eq!(rows[1].addr, "us-west");
    }

    #[test]
    fn parse_nodes_returns_empty_for_garbage() {
        assert!(parse_nodes("not json").is_empty());
        assert!(parse_nodes("").is_empty());
    }

    #[test]
    fn parse_nodes_skips_entries_without_node_id() {
        let raw = r#"[{"name": "no-id-here"}]"#;
        assert!(parse_nodes(raw).is_empty());
    }

    #[test]
    fn view_renders_empty_without_panic() {
        let p = MeshTopologyPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_rows_without_panic() {
        let mut p = MeshTopologyPanel::new();
        p.peers = vec![PeerRow {
            name: "pine".into(),
            addr: "us-west".into(),
            kind: "peer".into(),
            status: PeerStatus::Online,
        }];
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
    }

    #[test]
    fn view_renders_error_state_without_panic() {
        let mut p = MeshTopologyPanel::new();
        p.error = Some("mackesd not installed".into());
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
    }

    // CR-6.c tests
    #[test]
    fn open_peer_modal_message_sets_peer_modal() {
        let mut panel = MeshTopologyPanel::new();
        panel.peers = vec![PeerRow {
            name: "pine".into(),
            addr: "us-west".into(),
            kind: "peer".into(),
            status: PeerStatus::Online,
        }];
        assert!(panel.peer_modal.is_none());
        let _ = panel.update(Message::OpenPeerModal("pine".into()));
        assert_eq!(panel.peer_modal.as_deref(), Some("pine"));
    }

    #[test]
    fn close_modal_message_clears_peer_modal() {
        let mut panel = MeshTopologyPanel::new();
        panel.peer_modal = Some("pine".into());
        let _ = panel.update(Message::CloseModal);
        assert!(panel.peer_modal.is_none());
    }

    #[test]
    fn view_with_modal_open_renders_without_panic() {
        let mut panel = MeshTopologyPanel::new();
        panel.peers = vec![PeerRow {
            name: "birch".into(),
            addr: "eu-west".into(),
            kind: "host".into(),
            status: PeerStatus::Idle,
        }];
        panel.last_run_at = Some(SystemTime::now());
        panel.peer_modal = Some("birch".into());
        let _ = panel.view();
    }
}

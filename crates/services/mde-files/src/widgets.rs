//! Reusable building blocks for the Artifact Manager UI — pills, banners, peer
//! cards, file rows, section headers, mime icons. Each function takes the data
//! it needs and returns an `Element<'_, Message>`.

use crate::cosmic_compat::{ButtonSty, ContainerSty, SvgSty, TextSty};
use cosmic::iced::widget::{button, column, container, row, svg, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::{Element, Theme};

use crate::app::Message;
use crate::density::FileListMetrics;
use crate::icons;
use crate::model::{
    fmt_count, latency_bucket, FileRow, LatencyBucket, Mime, Peer, PeerKind, PeerStatus, Transfer,
    TxDir,
};
use crate::theme as t;

/// POLISH-files-focusring — the width (px) of the keyboard-focus ring to paint on
/// a file row/tile this frame, or `None` for no ring.
///
/// Returns the shared 2px Carbon ring ([`mde_theme::feedback::FOCUS_RING_WIDTH_PX`]
/// — the Object Card focus-outline weight, single-sourced) **only** when the item
/// both holds the focus cursor (`focused`) AND that focus is keyboard-visible
/// (`focus_visible` = the resolved [`crate::prefs::FocusVisibility::should_render`]
/// gate on `keyboard_active`). A mouse-driven focus or a plain pointer hover yields
/// `None`, so the ring is the keyboard cue alone — never drawn on every hover, and
/// visually distinct from the `ROW_HOVER` wash (§4/§7). Reading the width off the
/// `feedback` token keeps a `2.0` literal out of the surface crate.
#[must_use]
fn focus_ring_width(focused: bool, focus_visible: bool) -> Option<f32> {
    (focused && focus_visible).then_some(mde_theme::feedback::FOCUS_RING_WIDTH_PX)
}

/// GUI-7 — local cosmic renderer for an `mde_theme::ObjectCard` (replaces the
/// dropped iced-0.14 `mde_iced_components::object_card` in the Cosmic cutover).
/// Faithful to the small-card spec: a leading Material icon + title (+
/// subtitle) on a state-tinted Carbon surface.
pub fn object_card(
    card: mde_theme::ObjectCard,
    palette: mde_theme::Palette,
    focus_visible: bool,
) -> Element<'static, Message> {
    use mde_theme::{CardState, IconSize, IconState};
    let to_color = |c: mde_theme::Rgba| Color {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: c.a,
    };
    let title_color = to_color(card.title_color_override.unwrap_or(palette.text));
    let subtitle_color = to_color(card.subtitle_color_override.unwrap_or(palette.text_muted));
    let icon_px = card.size.icon_size();
    let icon_slot: Element<'static, Message> = if let Some(ic) = card.icon {
        let state = match card.state {
            CardState::Selected => IconState::Active,
            _ => IconState::Idle,
        };
        let bytes = mde_theme::mde_icon(ic, IconSize::Nav).svg_bytes_for_state(state);
        svg(svg::Handle::from_memory(bytes))
            .width(Length::Fixed(icon_px))
            .height(Length::Fixed(icon_px))
            .sty(move |_t: &Theme| svg::Style {
                color: Some(title_color),
            })
            .into()
    } else {
        Space::new()
            .width(Length::Fixed(icon_px))
            .height(Length::Fixed(icon_px))
            .into()
    };
    let title_w = text(card.title).size(13).colr(title_color);
    let text_col = if let Some(sub) = card.subtitle {
        column![title_w, text(sub).size(11).colr(subtitle_color)].spacing(2)
    } else {
        column![title_w]
    };
    let (bg, border) = match card.state {
        CardState::Selected => (t::ACTIVE_RUST_BG, t::ACTIVE_RUST_BORDER),
        CardState::Focused => (t::ROW_HOVER, t::PF_BORDER),
        CardState::Hover => (t::ROW_HOVER_FAINT, t::DIVIDER),
        _ => (t::PF_BG_300, t::PF_BORDER),
    };
    // POLISH-files-focusring — a keyboard-focused card wears the shared 2px Carbon
    // focus ring (`mde_theme::feedback`) in the interactive accent, gated on
    // keyboard-visible focus so a pointer hover keeps the faint 1px resting border.
    // The accent ring is the distinct keyboard cue; the wash + hairline stay the
    // mouse path (§4/§7).
    let (border_color, border_width) =
        match focus_ring_width(card.state == CardState::Focused, focus_visible) {
            Some(w) => (to_color(palette.accent), w),
            None => (border, 1.0),
        };
    container(
        row![icon_slot, text_col]
            .spacing(12)
            .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8.0, 10.0]))
    .width(Length::Fill)
    .sty(move |_t: &Theme| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border_color,
            width: border_width,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// Phase 1.4 — details-panel header for the focused object: its mime icon +
/// name + kind, rendered through the shared [`object_card`] so the panel header
/// matches the file rows exactly (one card idiom, not a bespoke header).
pub fn detail_card(name: &str, mime: Mime) -> Element<'static, Message> {
    let card = mde_theme::ObjectCard::small(mime_to_icon(mime), name.to_string())
        .with_subtitle(mime_label(mime).to_string());
    // The details-panel header is a static, non-focusable card (state stays
    // `Default`), so no keyboard-focus ring ever applies.
    object_card(card, t::mde_files_palette(), false)
}

/// POLISH-files-emptystates — local cosmic renderer for the shared
/// [`mde_theme::EmptyState`] data shape, the zero-data twin of [`object_card`]:
/// the data form lives in `mde-theme` (so non-iced consumers can describe a
/// panel) and the toolkit widget is built here (so iced doesn't leak into the
/// theme crate). It paints a reserved 32 px hero-icon slot, a heading, a muted
/// body line, and — when the state is actionable — a primary-fill CTA whose
/// click handler the call site supplies. Every zero-data view in this crate
/// routes through this ONE shape so empty panels read consistently (UX-6)
/// instead of each rolling its own bare faint line.
///
/// Spacing is the shared empty-state vocabulary verbatim (the `EMPTY_ICON_SIZE`
/// slot, the `HEADING_BODY_GAP`/`BODY_CTA_GAP` gaps, the `VERTICAL_PADDING`
/// centring band) — no re-derived metric (§4/§6). Generic over the message type
/// so the file-manager views AND the standalone file picker (which carries its
/// own `Message`) share the single renderer.
pub fn empty_state<M: Clone + 'static>(
    state: mde_theme::EmptyState,
    on_cta: Option<M>,
) -> Element<'static, M> {
    use mde_theme::components::{
        BODY_CTA_GAP, EMPTY_ICON_SIZE, HEADING_BODY_GAP, VERTICAL_PADDING,
    };
    use mde_theme::{IconSize, IconState};

    let mde_theme::EmptyState {
        icon,
        heading,
        body,
        cta_label,
        body_color_override,
    } = state;

    // Body colour: the shape's override (reserved for UX-22 high-contrast
    // variants) or the muted `text_muted` token the panels use for secondary
    // copy — a token, never a raw colour (§4).
    let body_color = body_color_override.map_or(t::FG_FAINT, |c| Color {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: c.a,
    });

    // Hero icon slot — reserved at the 32 px empty-state tier whether or not an
    // icon is set, so the block geometry is stable (UX-6/UX-8 swap-in target).
    let icon_slot: Element<'static, M> = if let Some(ic) = icon {
        let bytes =
            mde_theme::mde_icon(ic, IconSize::EmptyState).svg_bytes_for_state(IconState::Idle);
        svg(svg::Handle::from_memory(bytes))
            .width(Length::Fixed(EMPTY_ICON_SIZE))
            .height(Length::Fixed(EMPTY_ICON_SIZE))
            .sty(move |_t: &Theme| svg::Style {
                color: Some(t::FG_DIM),
            })
            .into()
    } else {
        Space::new()
            .width(Length::Fixed(EMPTY_ICON_SIZE))
            .height(Length::Fixed(EMPTY_ICON_SIZE))
            .into()
    };

    let mut col = column![
        icon_slot,
        Space::new().height(Length::Fixed(HEADING_BODY_GAP)),
        text(heading).size(15).colr(t::FG),
        Space::new().height(Length::Fixed(HEADING_BODY_GAP)),
        text(body).size(12).colr(body_color),
    ]
    .align_x(cosmic::iced::alignment::Horizontal::Center);

    // CTA — a primary-fill button beneath the body, painted only when the state
    // is actionable: it carries BOTH a label (the data) AND a handler (supplied
    // by the call site). An empty state with nothing to act on stays info-only.
    if let (Some(label), Some(msg)) = (cta_label, on_cta) {
        col = col.push(Space::new().height(Length::Fixed(BODY_CTA_GAP)));
        col = col.push(
            button(text(label).size(12).colr(t::FG))
                .padding(Padding::from([6.0, 14.0]))
                .sty(|_t: &Theme, status: button::Status| {
                    let bg = match status {
                        button::Status::Hovered => t::PRIMARY_AMBER_BG_HOVER,
                        _ => t::PRIMARY_AMBER_BG,
                    };
                    button::Style {
                        snap: false,
                        background: Some(Background::Color(bg)),
                        text_color: t::FG,
                        border: Border {
                            color: t::PRIMARY_AMBER_BORDER,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        ..button::Style::default()
                    }
                })
                .on_press(msg),
        );
    }

    // The shared `VERTICAL_PADDING` band centres the block inside the panel
    // body; `BODY_CTA_GAP` gives the prose breathing room from the edges.
    container(col)
        .width(Length::Fill)
        .padding(Padding::from([VERTICAL_PADDING, BODY_CTA_GAP]))
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .into()
}

#[cfg(test)]
mod empty_state_tests {
    //! POLISH-files-emptystates — the one shared empty-state renderer builds for
    //! every shape the 7 zero-data call sites construct, and the CTA affordance
    //! appears iff the state is actionable (label + handler both present).
    use super::*;
    use crate::app::Message;
    use crate::model::View;

    #[test]
    fn builds_for_info_cta_icon_and_override_variants() {
        // §7 runtime-reachable — info-only, with a CTA, with/without a hero icon,
        // and with a body-colour override all produce a valid element.
        let _: Element<'static, Message> =
            empty_state(mde_theme::EmptyState::info("Empty", "Nothing here."), None);
        let _: Element<'static, Message> = empty_state(
            mde_theme::EmptyState::info("Empty", "Nothing here.")
                .with_icon(mde_theme::Icon::Folder),
            None,
        );
        let _: Element<'static, Message> = empty_state(
            mde_theme::EmptyState::with_cta("Empty", "Add one.", "Get started")
                .with_icon(mde_theme::Icon::Peer),
            Some(Message::SelectView(View::MeshOverview)),
        );
        let mut overridden = mde_theme::EmptyState::info("Failed", "Boom.");
        overridden.body_color_override = Some(mde_theme::Rgba::rgba(255, 0, 0, 1.0));
        let _: Element<'static, Message> = empty_state(overridden, None);
    }

    #[test]
    fn cta_needs_both_a_label_and_a_handler() {
        // Route logic — a label with no handler, or a handler with no label,
        // both stay info-only (no CTA button). Each combination still builds.
        let _: Element<'static, Message> = empty_state(
            mde_theme::EmptyState::with_cta("h", "b", "go"),
            None::<Message>,
        );
        let _: Element<'static, Message> = empty_state(
            mde_theme::EmptyState::info("h", "b"),
            Some(Message::Refresh),
        );
    }

    #[test]
    fn renderer_is_generic_over_the_message_type() {
        // The standalone file picker carries its own `Message`; the single
        // renderer serves it too (compile-asserts the generic bound holds for a
        // second, unrelated message type).
        #[derive(Clone)]
        enum OtherMsg {
            Go,
        }
        let _: Element<'static, OtherMsg> = empty_state(
            mde_theme::EmptyState::with_cta("h", "b", "go"),
            Some(OtherMsg::Go),
        );
    }
}

// ─── Generic helpers ───────────────────────────────────────────────────────

/// A coloured square dot — used for status indicators (`.peer-status` in CSS).
pub fn status_dot(color: Color, size: f32) -> Element<'static, Message> {
    container(
        Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
    )
    .sty(move |_theme: &Theme| container::Style {
        snap: false,
        background: Some(Background::Color(color)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: (size / 2.0).into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// Wrap an SVG icon (one of `icons::*`) in a fixed-size box, tinted with `color`.
pub fn icon(svg_bytes: &'static [u8], size: f32, color: Color) -> Element<'static, Message> {
    svg(icons::handle(svg_bytes))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .sty(move |_theme: &Theme| svg::Style { color: Some(color) })
        .into()
}

/// 1-px horizontal divider line (`var(--divider)`).
pub fn hdivider() -> Element<'static, Message> {
    container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(t::DIVIDER)),
            ..container::Style::default()
        })
        .into()
}

/// Standard background container that paints `bg` and draws a 1-px border in `border`.
pub fn surface<'a, M: 'a>(bg: Color, border: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// Caps section header (`.fm-section-h h3`) — 11 px, letter-spaced.
pub fn section_h(label: &str, right: Option<&str>) -> Element<'static, Message> {
    let mut r = row![text(label.to_uppercase()).size(11).colr(t::FG_DIM),]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);

    if let Some(rt) = right {
        r = r.push(Space::new().width(Length::Fill));
        r = r.push(text(rt.to_string()).size(10).colr(t::FG_FAINT));
    } else {
        r = r.push(Space::new().width(Length::Fill));
    }

    container(r)
        .padding(Padding {
            top: 22.0,
            right: 0.0,
            bottom: 10.0,
            left: 0.0,
        })
        .into()
}

// ─── Pills ─────────────────────────────────────────────────────────────────

/// Amber mesh-origin pill — `↘ peer.mesh`.
pub fn mesh_pill(peer_host: &str) -> Element<'static, Message> {
    container(
        row![
            text("↘").size(10).colr(t::RUST),
            text(peer_host.to_string()).size(10).colr(t::ACCENT_HI),
        ]
        .spacing(4),
    )
    .padding(Padding::from([1.0, 6.0]))
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(t::MESH_PILL_BG)),
        border: Border {
            color: t::MESH_PILL_BORDER,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// Neutral local-origin pill — `local`.
pub fn local_pill() -> Element<'static, Message> {
    container(text("local").size(10).colr(t::FG_FAINT))
        .padding(Padding::from([1.0, 6.0]))
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(t::LOCAL_PILL_BG)),
            border: Border {
                color: t::LOCAL_PILL_BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// Tag chip used in the breadcrumb (`MESH` / `LOCAL`).
pub fn breadcrumb_tag(text_label: &str, is_mesh: bool) -> Element<'static, Message> {
    let (bg, fg, bd) = if is_mesh {
        (t::MESH_PILL_BG, t::ACCENT, t::MESH_PILL_BORDER)
    } else {
        (t::LOCAL_PILL_BG, t::FG_FAINT, t::LOCAL_PILL_BORDER)
    };
    container(text(text_label.to_string()).size(10).colr(fg))
        .padding(Padding::from([1.0, 6.0]))
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: bd,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ─── Banner (`.fm-banner`) ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct BannerStat {
    pub n: String,
    pub k: String,
}

impl BannerStat {
    #[must_use]
    pub fn new(n: impl Into<String>, k: impl Into<String>) -> Self {
        Self {
            n: n.into(),
            k: k.into(),
        }
    }
}

/// Hero banner with amber gradient + 3 px left border. Used at the top of every view
/// except the local veil.
pub fn banner(
    icon_svg: &'static [u8],
    title: String,
    subtitle: String,
    stats: Vec<BannerStat>,
) -> Element<'static, Message> {
    // CV-2 — on the §4 12-step scale: space.lg (20). Was an off-scale 22.
    let mut stats_row = row![].spacing(20);
    for st in stats {
        stats_row = stats_row.push(
            column![
                text(st.n).size(20).colr(t::ACCENT_HI),
                text(st.k.to_uppercase()).size(9).colr(t::FG_FAINT),
            ]
            .spacing(4)
            .align_x(cosmic::iced::alignment::Horizontal::Right),
        );
    }

    let layout = row![
        container(icon(icon_svg, 22.0, t::ACCENT_HI))
            .padding(Padding::new(9.0))
            .sty(|_| container::Style {
                snap: false,
                background: Some(Background::Color(t::MESH_PILL_BG)),
                border: Border {
                    color: t::MESH_PILL_BORDER,
                    width: 1.0,
                    radius: 0.0.into()
                },
                ..container::Style::default()
            }),
        column![
            text(title).size(15).colr(t::FG),
            text(subtitle).size(11).colr(t::FG_DIM),
        ]
        .spacing(2)
        .width(Length::Fill),
        stats_row,
    ]
    .spacing(16)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    container(layout)
        .padding(Padding::from([14.0, 18.0]))
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(t::PF_BG_200)),
            border: Border {
                color: t::BANNER_BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ─── Peer card (`.fm-peer-card`) ───────────────────────────────────────────

pub fn peer_card(peer: Peer) -> Element<'static, Message> {
    let kind_icon = icons::svg_for_peer_kind(peer.kind);

    let avatar_color = match peer.status {
        PeerStatus::Online => t::ACCENT_HI,
        PeerStatus::Self_ => t::RUST,
        _ => t::FG_DIM,
    };

    let stripe_color = match peer.status {
        PeerStatus::Online => t::PF_SUCCESS,
        PeerStatus::Idle => t::ACCENT,
        PeerStatus::Offline => t::PF_BORDER,
        PeerStatus::Self_ => t::RUST,
    };

    let head = row![
        container(icon(kind_icon, 20.0, avatar_color))
            .padding(Padding::new(8.0))
            .sty(|_| container::Style {
                snap: false,
                background: Some(Background::Color(Color {
                    a: 0.04,
                    ..Color::WHITE
                })),
                border: Border {
                    color: Color {
                        a: 0.06,
                        ..Color::WHITE
                    },
                    width: 1.0,
                    radius: 0.0.into()
                },
                ..container::Style::default()
            }),
        column![
            row![
                status_dot(t::peer_status_dot(peer.status), 6.0),
                text(peer.host.to_string()).size(13).colr(t::FG),
            ]
            .spacing(6)
            .align_y(cosmic::iced::alignment::Vertical::Center),
            text(peer.label.to_string()).size(11).colr(t::FG_FAINT),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(10)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let num_row = row![
        row![
            text(fmt_count(peer.files)).size(16).colr(t::FG),
            text("FILES").size(9).colr(t::FG_FAINT),
        ]
        .spacing(4)
        .align_y(cosmic::iced::alignment::Vertical::Center),
        row![
            text(fmt_count(peer.shared)).size(16).colr(t::FG),
            text("SHARED").size(9).colr(t::FG_FAINT),
        ]
        .spacing(4)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    ]
    .spacing(16);

    let meta_row = row![
        text(peer.addr.to_string()).size(10).colr(t::FG_FAINT),
        Space::new().width(Length::Fill),
        match peer.latency {
            Some(ms) => {
                let c = match latency_bucket(ms) {
                    LatencyBucket::Good => t::PF_SUCCESS,
                    LatencyBucket::Ok => t::ACCENT,
                    LatencyBucket::Slow => t::FG_FAINT,
                };
                Element::from(text(format!("{ms} ms")).size(10).colr(c))
            }
            None => Element::from(
                text(format!("last seen {}", peer.last))
                    .size(10)
                    .colr(t::FG_FAINT)
            ),
        },
    ];

    let id = peer.id.clone();
    let actions = row![
        button(text("Browse →").size(11).colr(t::ACCENT_HI))
            .padding(Padding::from([4.0, 8.0]))
            .sty(|_, _| button::Style {
                snap: false,
                background: Some(Background::Color(t::MESH_PILL_BG)),
                text_color: t::ACCENT_HI,
                border: Border {
                    color: t::MESH_PILL_BORDER,
                    width: 1.0,
                    radius: 0.0.into()
                },
                ..button::Style::default()
            })
            .on_press(Message::PeerCardBrowse(id.clone())),
        button(text("Send file").size(11).colr(t::FG_DIM))
            .padding(Padding::from([4.0, 8.0]))
            .sty(|_, _| ghost_button_style())
            .on_press(Message::PeerCardSend(id)),
        button(icon(icons::MORE, 14.0, t::FG_DIM))
            .padding(Padding::from([4.0, 8.0]))
            .sty(|_, _| ghost_button_style())
            .on_press(Message::Noop),
    ]
    .spacing(4);

    let stripe =
        container(Space::new().width(Length::Fill).height(Length::Fixed(2.0))).sty(move |_| {
            container::Style {
                snap: false,
                background: Some(Background::Color(stripe_color)),
                ..container::Style::default()
            }
        });

    let body = column![
        stripe,
        column![
            head,
            num_row,
            meta_row,
            container(hdivider()).padding(Padding {
                top: 8.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0
            }),
            actions,
        ]
        .padding(Padding {
            top: 14.0,
            right: 14.0,
            bottom: 10.0,
            left: 14.0
        })
        .spacing(12),
    ];

    let card_bg = if matches!(peer.status, PeerStatus::Offline) {
        Color {
            a: 0.55,
            ..t::PF_BG_200
        }
    } else {
        t::PF_BG_200
    };

    container(body)
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(card_bg)),
            border: Border {
                color: t::DIVIDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .width(Length::Fixed(244.0))
        .into()
}

/// Ghost-button style (transparent fill, picks up text colour from caller).
pub fn ghost_button_style() -> button::Style {
    button::Style {
        snap: false,
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: t::FG_DIM,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..button::Style::default()
    }
}

// ─── File row (`.fm-row`) — CR-4.b: Object Card retrofit ──────────────────

/// Map the `Mime` enum to the Material Symbols-backed `mde_theme::Icon`
/// variant (CR-3.c). Single canonical mapping; every file-row consumer
/// goes through this instead of calling `icons::svg_for_mime`.
fn mime_to_icon(mime: Mime) -> mde_theme::Icon {
    match mime {
        Mime::Folder => mde_theme::Icon::Folder,
        Mime::Doc => mde_theme::Icon::Document,
        Mime::Image => mde_theme::Icon::Image,
        Mime::Pdf => mde_theme::Icon::Pdf,
        Mime::Archive => mde_theme::Icon::Archive,
        Mime::Disk => mde_theme::Icon::Document,
    }
}

// ─── MOTION-FEEDBACK — file-row/tile motion (shared mde_theme vocabulary) ────

/// MOTION-FEEDBACK — the `mde_theme::animation` key prefix for a hovered file
/// row/tile, namespaced by the row's stable name so each row eases independently.
pub const HOVER_KEY_PREFIX: &str = "fm-hover:";

/// MOTION-FEEDBACK — per-row hover-lift rise (px). A small Carbon-scale lift so
/// a row/tile reads as interactive without the list jumping around. Mirrors the
/// applet's tile lift; rendered as compensating padding so neighbours never
/// reflow.
pub const ROW_HOVER_RISE_PX: f32 = 2.0;

/// MOTION-FEEDBACK — the per-item stagger slide distance (px). Each revealed row
/// slides up this far → 0 as its reveal tween completes (iced 0.13 has no opacity
/// widget, so a short slide reads as the fade-in — same idea the applet uses).
pub const ROW_REVEAL_SLIDE_PX: f32 = 6.0;

/// MOTION-FEEDBACK — the `mde_theme::animation` hover key for a row by name.
#[must_use]
pub fn row_hover_key(name: &str) -> String {
    format!("{HOVER_KEY_PREFIX}{name}")
}

/// MOTION-FEEDBACK — the resolved motion state for ONE file row/tile at the
/// current frame, derived from the shared `mde_theme::animation` helpers by
/// [`RowMotionCtx::for_row`]. All fields collapse to "rest" under reduce-motion
/// (no movement; the state change is instant — the selection accent is still
/// shown, just not animated).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowMotion {
    /// Hover-lift offset (px, ≤ 0 = up); 0 at rest.
    pub lift_px: f32,
    /// Selection-accent strength `0.0..=1.0`: 0 = no accent, 1 = full selection
    /// tint. Eases in on select; instant (0 or 1) under reduce-motion.
    pub accent_t: f32,
    /// Staggered-reveal slide offset (px, ≥ 0 = starts below, settles to 0).
    pub reveal_y: f32,
    /// MOTION-TRANS-3 — presence factor `0.0..=1.0` for the insert/remove
    /// transition: `1.0` = fully present (a normal/inserted row that has settled),
    /// easing toward `0.0` as a *removed* row collapses away (its height + opacity
    /// shrink to nothing). A present row sits at `1.0`; only a row leaving the
    /// listing dips below it. Drives [`RowMotion::collapse_height_factor`] +
    /// [`RowMotion::collapse_alpha`].
    pub presence_t: f32,
}

impl Default for RowMotion {
    fn default() -> Self {
        // A row at rest is fully present (`presence_t == 1.0`) — collapse only
        // applies to a row that is actively being removed.
        Self {
            lift_px: 0.0,
            accent_t: 0.0,
            reveal_y: 0.0,
            presence_t: 1.0,
        }
    }
}

impl RowMotion {
    /// MOTION-FEEDBACK — the eased selection background for this row: the
    /// `LIST_SELECTION_BG` token scaled by [`Self::accent_t`] so the accent
    /// animates in (`hovered`/`focused` fallback handled by the caller). Returns
    /// the resting `base` when there's no accent.
    /// MOTION-FEEDBACK — the `(top, bottom)` compensating padding (px) that
    /// renders this row's hover-lift + reveal-slide WITHOUT reflowing neighbours.
    /// `lift_px` is ≤ 0 (up), `reveal_y` is ≥ 0 (starts below), so the net offset
    /// spans `[-ROW_HOVER_RISE_PX, +ROW_REVEAL_SLIDE_PX]`. The full span
    /// (`RESERVE`) is reserved and split top/bottom so `top + bottom == RESERVE`
    /// at EVERY frame — the cell height is constant. Pure; the renderer just
    /// applies it as `Padding`.
    #[must_use]
    fn vertical_padding(self) -> (f32, f32) {
        const RESERVE: f32 = ROW_HOVER_RISE_PX + ROW_REVEAL_SLIDE_PX;
        let offset = self.lift_px + self.reveal_y;
        let top = (ROW_HOVER_RISE_PX + offset).clamp(0.0, RESERVE);
        (top, RESERVE - top)
    }

    /// MOTION-TRANS-3 — the height multiplier `0.0..=1.0` for a row mid-collapse:
    /// `1.0` while present, shrinking to `0.0` as a removed row eases out, so the
    /// row's reserved vertical space closes smoothly and the rows below slide up
    /// to fill the gap instead of the whole table jumping. A present row
    /// (`presence_t == 1.0`) returns `1.0` — a no-op the renderer can skip. Pure;
    /// the renderer multiplies the row height by this.
    #[must_use]
    pub fn collapse_height_factor(self) -> f32 {
        self.presence_t.clamp(0.0, 1.0)
    }

    /// MOTION-TRANS-3 — the opacity `0.0..=1.0` a collapsing row renders at,
    /// derived from the shared [`mde_theme::animation::Transition::FadeOut`] so a
    /// removed row fades as it shrinks (the iced-0.13-fork reads this as a
    /// color-alpha multiplier — no opacity widget needed). A fully-present row
    /// returns `1.0`. The fade runs slightly ahead of the height collapse (the
    /// `FadeOut` is keyed on `1 - presence_t`) so the row reads as *leaving*
    /// before its space fully closes.
    #[must_use]
    pub fn collapse_alpha(self) -> f32 {
        use mde_theme::animation::Transition;
        // `presence_t` 1→0 as the row leaves; FadeOut wants progress 0→1.
        Transition::FadeOut
            .params(1.0 - self.presence_t.clamp(0.0, 1.0))
            .alpha
    }

    #[must_use]
    fn selection_bg(self, base: Color) -> Color {
        use mde_theme::animation::lerp_f32;
        if self.accent_t <= f32::EPSILON {
            return base;
        }
        // Crossfade every channel from the resting bg (transparent, or the
        // focused `ROW_HOVER` wash) toward the full selection tint, so a focused
        // row that becomes selected blends — rather than hue-snapping — into the
        // indigo accent. At `accent_t == 1.0` this is exactly `LIST_SELECTION_BG`.
        let t = self.accent_t.clamp(0.0, 1.0);
        let to = t::LIST_SELECTION_BG;
        Color {
            r: lerp_f32(base.r, to.r, t),
            g: lerp_f32(base.g, to.g, t),
            b: lerp_f32(base.b, to.b, t),
            a: lerp_f32(base.a, to.a, t),
        }
    }
}

/// MOTION-FEEDBACK — the shared motion context a file-view passes down so every
/// row/tile derives its [`RowMotion`] from ONE `mde_theme::animation::Animator`
/// off a single subscription tick. Borrows the animator + the live hover keys;
/// pure read-only (the view never mutates animation state).
#[derive(Clone, Copy)]
pub struct RowMotionCtx<'a> {
    /// The shared animator (hover + selection-accent tweens live here).
    pub anim: &'a mde_theme::animation::Animator,
    /// The currently-hovered row's hover key, if any.
    pub hovered: Option<&'a str>,
    /// The releasing (hover-exit, settling-back) row's hover key, if any.
    pub releasing: Option<&'a str>,
    /// When the active listing was (re)loaded — the stagger reveal origin. `None`
    /// once the reveal has fully settled (no per-row work at rest).
    pub reveal_origin: Option<std::time::Instant>,
    /// The frame instant every tween is sampled at.
    pub now: std::time::Instant,
    /// Reduce-motion: instant state changes, no movement.
    pub reduce_motion: bool,
    /// POLISH-files-focusring — whether a keyboard-focus ring should be painted
    /// this frame: the resolved [`crate::prefs::FocusVisibility::should_render`]
    /// gate on `keyboard_active`. Carried beside `reduce_motion` (the sibling
    /// per-frame a11y signal) so every file view draws the 2px Carbon focus ring
    /// only when focus is keyboard-driven, never on a pointer hover.
    pub focus_visible: bool,
    /// BEAUT-FILES — the perceived-performance load state of this listing. Drives
    /// the skeleton-first paint + the stale-while-refreshing dim/crossfade the
    /// file views apply around their row list.
    pub load: crate::loading::ListingLoad,
    /// MOTION-TRANS-3 — rows removed from this listing since the prior render,
    /// each collapsing away at the index it last held. Borrowed read-only; the
    /// view splices them back in via [`RowMotionCtx::collapse_at`] so the table
    /// closes their gap smoothly instead of jumping. Empty at rest.
    pub removed: &'a [crate::app::RemovedRow],
}

impl RowMotionCtx<'_> {
    /// BEAUT-FILES — render the skeleton placeholder instead of the row list?
    /// (First load with no prior content.)
    #[must_use]
    pub fn show_skeleton(&self) -> bool {
        self.load.show_skeleton()
    }

    /// BEAUT-FILES — the skeleton placeholder element for a loading listing,
    /// shimmering at the current frame's phase (static under reduce-motion).
    #[must_use]
    pub fn skeleton(&self) -> Element<'static, Message> {
        crate::loading::skeleton_rows(
            crate::loading::SKELETON_ROW_COUNT,
            self.load.skeleton_shimmer(self.reduce_motion),
            self.now,
            &t::mde_files_palette(),
        )
    }

    /// BEAUT-FILES — the opacity to render kept-on-screen rows at: full normally,
    /// dimmed during a stale refresh (stale-while-revalidate). The view multiplies
    /// its row text/background tints by this so a refresh dims rather than blanks.
    #[must_use]
    pub fn content_alpha(&self) -> f32 {
        self.load.content_alpha()
    }
}

impl RowMotionCtx<'_> {
    /// MOTION-FEEDBACK — resolve [`RowMotion`] for the row `name` at visible
    /// `index`. Hover-lift + selection accent come from the [`Animator`]; the
    /// reveal comes from `reveal_origin` + the row's capped (≤8) stagger delay.
    /// Under reduce-motion everything collapses to rest (accent snaps to
    /// `selected`, no movement).
    #[must_use]
    pub fn for_row(self, name: &str, index: usize, selected: bool) -> RowMotion {
        use mde_theme::animation::{ease, Transition};
        use mde_theme::motion::{list, Easing};

        // Selection accent: under reduce-motion it's an instant 0/1; otherwise it
        // eases in as a tween keyed on the row name lives in the animator.
        let accent_t = if !selected {
            0.0
        } else if self.reduce_motion {
            1.0
        } else {
            // The accent shares the row's hover key namespace but its own suffix
            // so a hover doesn't disturb the selection tween and vice-versa.
            self.anim
                .value(&accent_key(name), self.now, Easing::EaseOut)
        };

        if self.reduce_motion {
            // No movement at all — instant state change (a11y contract). A present
            // row is fully present (presence_t == 1.0 via Default).
            return RowMotion {
                lift_px: 0.0,
                accent_t,
                reveal_y: 0.0,
                ..RowMotion::default()
            };
        }

        // Hover-lift: eased in while hovered, out while releasing, 0 at rest.
        let key = row_hover_key(name);
        let hover_amt = if self.hovered == Some(key.as_str()) {
            self.anim.value(&key, self.now, Easing::EaseOut)
        } else if self.releasing == Some(key.as_str()) {
            1.0 - self.anim.value(&key, self.now, Easing::EaseOut)
        } else {
            0.0
        };
        let lift_px = Transition::Lift(ROW_HOVER_RISE_PX)
            .params(hover_amt)
            .translate_y;

        // Staggered reveal: row `index` (capped at STAGGER_CAP-1) gets a delayed
        // SlideUp that settles to 0. Past the cap every row shares the cap delay
        // so a long listing doesn't crawl.
        let reveal_y = match self.reveal_origin {
            Some(origin) => {
                let step = u64::from(list::STAGGER_STEP_MS);
                let capped = index.min(list::STAGGER_CAP - 1) as u64;
                let delay = std::time::Duration::from_millis(capped * step);
                let reveal_start = origin + delay;
                if self.now < reveal_start {
                    // Not yet begun — start fully slid-down (hidden-ish).
                    ROW_REVEAL_SLIDE_PX
                } else {
                    let reveal_dur =
                        std::time::Duration::from_millis(u64::from(list::STAGGER_REVEAL_MS));
                    let tw = mde_theme::animation::Tween::starting_at(reveal_start, reveal_dur);
                    let t = ease(tw.progress(self.now), Easing::EaseOut);
                    Transition::SlideUp(ROW_REVEAL_SLIDE_PX)
                        .params(t)
                        .translate_y
                }
            }
            None => 0.0,
        };

        RowMotion {
            lift_px,
            accent_t,
            reveal_y,
            // A present row is fully present; only `for_removed_row` dips below 1.0.
            ..RowMotion::default()
        }
    }

    /// MOTION-TRANS-3 — resolve the collapse [`RowMotion`] for a row that has been
    /// *removed* from the listing since the prior render. `collapse_origin` is when
    /// the removal was observed; the row's `presence_t` eases `1.0 → 0.0` over the
    /// shared exit window so its height + opacity shrink to nothing (the renderer
    /// closes the gap, so the rows below slide up to fill it instead of the table
    /// jumping). Under reduce-motion the row is removed instantly (`presence_t == 0`
    /// at once — no movement, the a11y contract). Returns `None` once the collapse
    /// has fully elapsed, so a settled listing keeps no leaving rows around (no
    /// per-row work, no neighbour reflow) — the caller drops the row entirely.
    #[must_use]
    pub fn for_removed_row(self, collapse_origin: std::time::Instant) -> Option<RowMotion> {
        use mde_theme::animation::{ease, Tween};
        use mde_theme::motion::{Easing, Motion};

        if self.reduce_motion {
            // No collapse animation — the row is simply gone (instant removal).
            return None;
        }

        // The exit shares the Carbon `dialog_mount` *crossfade* family duration
        // (the same preset the panel/drawer exits use) resolved against
        // reduce-motion, so removal feels consistent with every other leave.
        let dur = Motion::dialog_mount().resolved(self.reduce_motion).duration;
        let tw = Tween::starting_at(collapse_origin, dur);
        if tw.is_complete(self.now) {
            // Fully collapsed — the row no longer occupies any space; drop it.
            return None;
        }
        // `t` 0→1 as the collapse progresses; presence is its inverse (1→0). The
        // collapse is a direct time-based tween from `collapse_origin` (tracked in
        // the app's `removed_rows` queue) — a leaving row is no longer in the live
        // list the hover/accent tweens key off, so it needs no animator key.
        let t = ease(tw.progress(self.now), Easing::EaseOut);
        Some(RowMotion {
            presence_t: 1.0 - t,
            ..RowMotion::default()
        })
    }

    /// MOTION-TRANS-3 — render every removed row that last occupied visible index
    /// `slot`, each collapsing away in place. The view splices these into its row
    /// column at `slot` so a deleted/moved row closes its gap smoothly (the rows
    /// below slide up) rather than the table jumping.
    /// `show_src`/`m` mirror what the live rows render with so the leaving row
    /// matches the listing rhythm. Yields nothing for rows whose collapse has
    /// elapsed or under reduce-motion (the row is simply gone). Pure read-only.
    #[must_use]
    pub fn collapse_at(
        &self,
        slot: usize,
        show_src: bool,
        m: crate::density::FileListMetrics,
    ) -> Vec<Element<'static, Message>> {
        self.removed
            .iter()
            .filter(|r| r.index == slot)
            .filter_map(|r| {
                let motion = self.for_removed_row(r.at)?;
                Some(collapsing_row(r.row.clone(), show_src, m, motion))
            })
            .collect()
    }

    /// MOTION-TRANS-3 — render every removed row whose old visible index is at or
    /// beyond `len` (the new listing's row count), collapsing away at the tail.
    /// Catches rows that were removed from the END of the listing (their old slot
    /// no longer has a live row to splice before). Same contract as
    /// [`RowMotionCtx::collapse_at`].
    #[must_use]
    pub fn collapse_tail(
        &self,
        len: usize,
        show_src: bool,
        m: crate::density::FileListMetrics,
    ) -> Vec<Element<'static, Message>> {
        self.removed
            .iter()
            .filter(|r| r.index >= len)
            .filter_map(|r| {
                let motion = self.for_removed_row(r.at)?;
                Some(collapsing_row(r.row.clone(), show_src, m, motion))
            })
            .collect()
    }
}

/// MOTION-FEEDBACK — the selection-accent tween key for a row (sibling to its
/// hover key so the two tweens never collide).
#[must_use]
pub fn accent_key(name: &str) -> String {
    format!("fm-accent:{name}")
}

/// File-row Object Card (CR-4.b). Renders each file entry as a
/// `CardSize::Small` Object Card so it shares the same grid grammar
/// as folder rows (CR-4.a). Selection and focus state are reflected
/// via `CardState`; conflict chips + sync badges still render inline
/// below the card when set.
///
/// `show_src` folds the origin host into the subtitle when present.
pub fn file_row(
    row_data: FileRow,
    show_src: bool,
    selected: bool,
    focused: bool,
    focus_visible: bool,
    motion: RowMotion,
) -> Element<'static, Message> {
    let has_conflict = row_data.has_conflict;
    let syncing = row_data.syncing;
    let origin_host: Option<String> = row_data.origin().map(str::to_owned);

    let FileRow {
        name,
        conflict_sibling,
        mime,
        size,
        age,
        ..
    } = row_data;
    let sibling = conflict_sibling.unwrap_or_default();

    // Build subtitle: `{size} · {age}` with optional origin suffix.
    let size_age = match (size.is_empty(), age.is_empty()) {
        (true, true) => String::new(),
        (true, false) => age.clone(),
        (false, true) => size.clone(),
        (false, false) => format!("{size} · {age}"),
    };
    let subtitle = if show_src {
        match origin_host.as_deref() {
            Some(host) if !size_age.is_empty() => format!("{size_age} · from {host}"),
            Some(host) => format!("from {host}"),
            None if !size_age.is_empty() => format!("{size_age} · local"),
            None => "local".to_string(),
        }
    } else {
        size_age
    };

    let card_state = if selected {
        mde_theme::CardState::Selected
    } else if focused {
        mde_theme::CardState::Focused
    } else {
        mde_theme::CardState::Default
    };

    let palette = t::mde_files_palette();
    let mut card =
        mde_theme::ObjectCard::small(mime_to_icon(mime), name.clone()).with_state(card_state);
    if !subtitle.is_empty() {
        card = card.with_subtitle(subtitle);
    }
    let card_el = object_card(card, palette, focus_visible);

    // MESHFS-11.1: conflict chip — rendered below the card when present.
    let conflict_chip: Option<Element<'static, Message>> = if has_conflict {
        let orig_name = name.clone();
        Some(
            button(
                container(
                    row![
                        text("⚠").size(10).colr(t::ACCENT_HI),
                        text("conflict").size(10).colr(t::ACCENT_HI),
                    ]
                    .spacing(3)
                    .align_y(cosmic::iced::alignment::Vertical::Center),
                )
                .padding(Padding::from([1.0, 6.0])),
            )
            .padding(0)
            .sty(|_, status| {
                let bg = match status {
                    button::Status::Hovered => t::PRIMARY_AMBER_BG_HOVER,
                    _ => t::PRIMARY_AMBER_BG,
                };
                button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: t::ACCENT_HI,
                    border: Border {
                        color: t::PRIMARY_AMBER_BORDER,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::ConflictResolve(orig_name, sibling))
            .into(),
        )
    } else {
        None
    };

    // MESHFS-11.1: sync badge — renders below the card while healing.
    let sync_badge: Option<Element<'static, Message>> = if syncing {
        Some(
            container(text("⟳").size(10).colr(t::FG_FAINT))
                .padding(Padding::from([1.0, 4.0]))
                .into(),
        )
    } else {
        None
    };

    let body: Element<'static, Message> = if conflict_chip.is_none() && sync_badge.is_none() {
        card_el
    } else {
        let mut col = column![card_el].spacing(2);
        if let Some(chip) = conflict_chip {
            col = col.push(chip);
        }
        if let Some(badge) = sync_badge {
            col = col.push(badge);
        }
        col.into()
    };
    // MOTION-FEEDBACK — hover-lift + staggered reveal as compensating padding
    // (no neighbour reflow), wrapped in a `mouse_area` so enter/exit drive the
    // hover tween. The selection accent rides the card's `CardState` tint above.
    // MOTION-TRANS-3 — the grid tile's collapse footprint is the small object
    // card's height (icon row + title/subtitle); a stable estimate is fine since
    // the collapse only scales it toward 0.
    with_row_motion(body, &name, motion, GRID_TILE_COLLAPSE_PX)
}

/// MOTION-TRANS-3 — the height (px) a grid tile collapses from when removed. The
/// small object card is a leading icon beside a 2-line title/subtitle on ~8px
/// vertical padding; this matches that footprint closely enough to close the gap
/// smoothly (the collapse only scales it toward 0).
const GRID_TILE_COLLAPSE_PX: f32 = 56.0;

/// MOTION-FEEDBACK / MOTION-TRANS-3 — wrap a row/tile body with its [`RowMotion`]:
/// render the hover-lift + reveal-slide as compensating top/bottom padding so the
/// grid never reflows, and attach the `mouse_area` enter/exit that arm the hover
/// tween. At rest (no offset) this is a no-op padding of zero. Shared by both the
/// grid tile ([`file_row`]) and the list row ([`list_row`]).
///
/// MOTION-TRANS-3 — when the row is mid-**collapse** (`presence_t < 1.0`, a row
/// removed since the prior render), the wrapper instead shrinks the row's height
/// by [`RowMotion::collapse_height_factor`] and fades it via
/// [`RowMotion::collapse_alpha`], so a removed row closes its gap smoothly and the
/// rows below slide up to fill it — the table never jumps. A fully-present row
/// (`presence_t == 1.0`, the common path) skips the collapse entirely.
fn with_row_motion(
    body: Element<'static, Message>,
    name: &str,
    motion: RowMotion,
    full_height: f32,
) -> Element<'static, Message> {
    use cosmic::iced::widget::mouse_area;
    let (top, bottom) = motion.vertical_padding();
    let padded = container(body).padding(Padding {
        top,
        right: 0.0,
        bottom,
        left: 0.0,
    });
    // MOTION-TRANS-3 — a row leaving the listing collapses height-stably: its
    // reserved height is scaled toward 0 (closing the gap) while it fades out.
    // Clip so the still-full-height body inside doesn't spill past the shrinking
    // window. Present rows (`collapse_height_factor() == 1.0`) take the cheap
    // identity path with no extra wrappers.
    let factor = motion.collapse_height_factor();
    let key = row_hover_key(name);
    if factor < 1.0 {
        // Reserve the full row span (body + the compensating padding) so the
        // collapse closes ALL of the row's vertical footprint, not just the body.
        let reserved = full_height + ROW_HOVER_RISE_PX + ROW_REVEAL_SLIDE_PX;
        let h = (reserved * factor).max(0.0);
        let collapsing = container(padded)
            .height(Length::Fixed(h))
            .clip(true)
            .sty(move |_| container::Style {
                snap: false,
                ..container::Style::default()
            });
        // No hover handlers on a leaving row — it is on its way out.
        return collapsing.into();
    }
    mouse_area(padded)
        .on_enter(Message::RowHoverEnter(key.clone()))
        .on_exit(Message::RowHoverExit(key))
        .into()
}

/// MOTION-TRANS-3 — render a file row that has been **removed** from the listing
/// since the prior render, collapsing it away. The row keeps its real
/// [`list_row`] / [`file_row`] body (so it reads as the same row shrinking, not a
/// blank bar) wrapped in the collapse-aware [`with_row_motion`]: its height eases
/// to 0 and it fades out, so the rows below slide up to fill the gap with no
/// scroll jump. The caller resolves `motion` via
/// [`RowMotionCtx::for_removed_row`] (which yields `None`, ⇒ drop the row, once
/// the collapse has fully elapsed or under reduce-motion). `m` carries the
/// density-resolved metrics so the leaving row matches the live row rhythm.
pub fn collapsing_row(
    row_data: FileRow,
    show_src: bool,
    m: FileListMetrics,
    motion: RowMotion,
) -> Element<'static, Message> {
    // A leaving row is never selected/focused — it is on its way out (so no
    // focus ring either). `list_row` routes through the collapse-aware
    // `with_row_motion` (height → 0); fade the whole row by `collapse_alpha` so it
    // dims as it shrinks.
    let body = list_row(row_data, show_src, false, false, false, m, motion);
    crate::loading::dim(body, motion.collapse_alpha())
}

/// File-list head row (caps, dim). DENSITY-SYMMETRY — every metric (column
/// widths, inter-cell gap, header padding, caption size) traces to the
/// density-resolved [`FileListMetrics`] tokens, so the header re-rhythms with
/// the listing when the user's Density changes.
pub fn file_row_head(src_label: &str, m: FileListMetrics) -> Element<'static, Message> {
    let layout = row![
        Space::new().width(Length::Fixed(m.icon_col)),
        container(text("Name".to_string()).size(m.caption).colr(t::FG_FAINT)).width(Length::Fill),
        container(
            text(src_label.to_string())
                .size(m.caption)
                .colr(t::FG_FAINT)
        )
        .width(Length::Shrink),
        container(text("Size".to_string()).size(m.caption).colr(t::FG_FAINT))
            .width(Length::Fixed(m.size_col))
            .align_x(cosmic::iced::alignment::Horizontal::Right),
        container(
            text("Modified".to_string())
                .size(m.caption)
                .colr(t::FG_FAINT)
        )
        .width(Length::Fixed(m.modified_col))
        .align_x(cosmic::iced::alignment::Horizontal::Right),
    ]
    .spacing(m.col_gap);

    container(layout)
        .padding(Padding::from([f32::from(m.pad_y), f32::from(m.pad_x)]))
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(Color {
                a: 0.02,
                ..Color::WHITE
            })),
            border: Border {
                color: t::DIVIDER,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ─── List-view file row (`.fm-list-row`) — CR-4.d ─────────────────────────

/// List-view file row — CR-4.d. Classic ChromeOS density. DENSITY-SYMMETRY —
/// the row height, column widths, inter-cell gap, row padding, and the body /
/// caption text sizes all trace to the density-resolved [`FileListMetrics`]
/// tokens (passed in `m`), so the row re-rhythms with the listing when the
/// user's Density changes. 1 px `LIST_ROW_DIVIDER` bottom divider, indigo 15 %
/// selection overlay. Column layout mirrors `file_row_head`: icon · name (fill)
/// · origin (shrink, when `show_src`) · size · modified.
pub fn list_row(
    row_data: FileRow,
    show_src: bool,
    selected: bool,
    focused: bool,
    focus_visible: bool,
    m: FileListMetrics,
    motion: RowMotion,
) -> Element<'static, Message> {
    let has_conflict = row_data.has_conflict;
    let syncing = row_data.syncing;
    let origin_host: Option<String> = row_data.origin().map(str::to_owned);
    let FileRow {
        name,
        conflict_sibling,
        mime,
        size,
        age,
        ..
    } = row_data;
    let sibling = conflict_sibling.unwrap_or_default();

    // MOTION-FEEDBACK — the selection accent animates in: the resting bg is the
    // focus/transparent layer; when selected, `RowMotion::selection_bg` eases the
    // `LIST_SELECTION_BG` tint over it (instant under reduce-motion).
    let rest_bg = if focused {
        t::ROW_HOVER
    } else {
        Color::TRANSPARENT
    };
    let bg = if selected {
        motion.selection_bg(rest_bg)
    } else {
        rest_bg
    };

    let resolved = mde_theme::mde_icon(mime_to_icon(mime), mde_theme::IconSize::Nav);
    let icon_bytes = resolved.svg_bytes_for_state(mde_theme::IconState::Idle);
    let icon_el = icon(icon_bytes, mde_theme::IconSize::Nav.px(), t::FG_DIM);

    let roboto = cosmic::iced::Font::with_name("Roboto");

    let mut inner = row![
        container(icon_el)
            .width(Length::Fixed(m.icon_col))
            .align_x(cosmic::iced::alignment::Horizontal::Center),
        container(text(name.clone()).size(m.body).font(roboto).colr(t::FG)).width(Length::Fill),
    ]
    .spacing(m.col_gap)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    if show_src {
        let origin_str = origin_host.as_deref().unwrap_or("local").to_owned();
        inner = inner.push(
            container(
                text(origin_str)
                    .size(m.caption)
                    .font(roboto)
                    .colr(t::FG_DIM),
            )
            .width(Length::Shrink),
        );
    }

    inner = inner
        .push(
            container(text(size).size(m.caption).font(roboto).colr(t::FG_DIM))
                .width(Length::Fixed(m.size_col))
                .align_x(cosmic::iced::alignment::Horizontal::Right),
        )
        .push(
            container(text(age).size(m.caption).font(roboto).colr(t::FG_DIM))
                .width(Length::Fixed(m.modified_col))
                .align_x(cosmic::iced::alignment::Horizontal::Right),
        );

    // POLISH-files-focusring — paint the shared 2px Carbon focus ring
    // (`mde_theme::feedback`) in the interactive accent around a keyboard-focused
    // row, gated on keyboard-visible focus so a pointer hover/mouse cursor keeps
    // only the bare `ROW_HOVER` wash. The accent ring is the distinct keyboard cue
    // (§4/§7); a mouse-focused row stays the wash it always was.
    let focus_border = match focus_ring_width(focused, focus_visible) {
        Some(w) => Border {
            color: t::ACCENT,
            width: w,
            radius: 0.0.into(),
        },
        None => Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
    };
    let row_el: Element<'static, Message> = container(inner)
        .padding(Padding::from([0.0, f32::from(m.pad_x)]))
        .height(Length::Fixed(m.row_h))
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: focus_border,
            ..container::Style::default()
        })
        .into();

    let divider_el: Element<'static, Message> =
        container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
            .sty(|_| container::Style {
                snap: false,
                background: Some(Background::Color(t::LIST_ROW_DIVIDER)),
                ..container::Style::default()
            })
            .into();

    let row_with_divider: Element<'static, Message> = column![row_el, divider_el].spacing(0).into();

    let conflict_chip: Option<Element<'static, Message>> = if has_conflict {
        let orig_name = name.clone();
        Some(
            button(
                container(
                    row![
                        text("⚠").size(10).colr(t::ACCENT_HI),
                        text("conflict").size(10).colr(t::ACCENT_HI),
                    ]
                    .spacing(3)
                    .align_y(cosmic::iced::alignment::Vertical::Center),
                )
                .padding(Padding::from([1.0, 6.0])),
            )
            .padding(0)
            .sty(|_, status| {
                let bg_chip = match status {
                    button::Status::Hovered => t::PRIMARY_AMBER_BG_HOVER,
                    _ => t::PRIMARY_AMBER_BG,
                };
                button::Style {
                    snap: false,
                    background: Some(Background::Color(bg_chip)),
                    text_color: t::ACCENT_HI,
                    border: Border {
                        color: t::PRIMARY_AMBER_BORDER,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::ConflictResolve(orig_name, sibling))
            .into(),
        )
    } else {
        None
    };

    let sync_badge: Option<Element<'static, Message>> = if syncing {
        Some(
            container(text("⟳").size(10).colr(t::FG_FAINT))
                .padding(Padding::from([1.0, 4.0]))
                .into(),
        )
    } else {
        None
    };

    let body: Element<'static, Message> = if conflict_chip.is_none() && sync_badge.is_none() {
        row_with_divider
    } else {
        let mut col = column![row_with_divider].spacing(2);
        if let Some(chip) = conflict_chip {
            col = col.push(chip);
        }
        if let Some(badge) = sync_badge {
            col = col.push(badge);
        }
        col.into()
    };
    with_row_motion(body, &name, motion, m.row_h)
}

// ─── Transfer-log row (`.fm-tx`) ───────────────────────────────────────────

pub fn tx_row(tx: Transfer) -> Element<'static, Message> {
    let (dir_label, dir_fg, dir_bg, dir_bd) = match tx.dir {
        TxDir::In => (
            "↘ IN",
            t::ACCENT_HI,
            Color {
                a: 0.08,
                ..t::PF_INFO
            },
            Color {
                a: 0.40,
                ..t::PF_INFO
            },
        ),
        TxDir::Out => ("↗ OUT", t::ACCENT_HI, t::MESH_PILL_BG, t::MESH_PILL_BORDER),
    };

    let layout = row![
        container(text(dir_label.to_string()).size(10).colr(dir_fg))
            .padding(Padding::from([1.0, 6.0]))
            .sty(move |_| container::Style {
                snap: false,
                background: Some(Background::Color(dir_bg)),
                border: Border {
                    color: dir_bd,
                    width: 1.0,
                    radius: 0.0.into()
                },
                ..container::Style::default()
            }),
        container(text(tx.name.to_string()).size(12).colr(t::FG)).width(Length::Fill),
        text(tx.peer.to_string()).size(11).colr(t::ACCENT),
        text(format!("{} · {}", tx.size, tx.age))
            .size(11)
            .colr(t::FG_FAINT),
    ]
    .spacing(10)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    container(layout).padding(Padding::from([8.0, 10.0])).into()
}

// ─── Sidebar row ───────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum SideRowVariant {
    Default,
    Active,
    Primary,
    PrimaryActive,
    Peer { status: PeerStatus, active: bool },
    Dim,
}

pub fn side_row(
    icon_svg: &'static [u8],
    label: &str,
    secondary: Option<&str>,
    meta: Option<String>,
    variant: SideRowVariant,
    msg: Message,
) -> Element<'static, Message> {
    let (bg, fg, border_color) = match variant {
        SideRowVariant::Default => (Color::TRANSPARENT, t::FG_DIM, Color::TRANSPARENT),
        SideRowVariant::Active => (t::ACTIVE_RUST_BG, Color::WHITE, t::ACTIVE_RUST_BORDER),
        SideRowVariant::Primary => (t::PRIMARY_AMBER_BG, t::FG, t::PRIMARY_AMBER_BORDER),
        SideRowVariant::PrimaryActive => (t::PRIMARY_AMBER_BG_ACTIVE, t::FG, t::ACCENT_HI),
        SideRowVariant::Peer { status, active } => {
            if active {
                (t::ACTIVE_RUST_BG, Color::WHITE, t::ACTIVE_RUST_BORDER)
            } else if matches!(status, PeerStatus::Offline) {
                (Color::TRANSPARENT, t::FG_FAINT, Color::TRANSPARENT)
            } else {
                (Color::TRANSPARENT, t::FG_DIM, Color::TRANSPARENT)
            }
        }
        SideRowVariant::Dim => (Color::TRANSPARENT, t::FG_FAINT, Color::TRANSPARENT),
    };

    let icon_color = match variant {
        SideRowVariant::Active | SideRowVariant::PrimaryActive => t::ACCENT_HI,
        SideRowVariant::Primary => t::ACCENT_HI,
        _ => fg,
    };

    let mut name_row = row![text(label.to_string()).size(13).colr(fg)].spacing(6);
    if let Some(s) = secondary {
        name_row = name_row.push(text(s.to_string()).size(10).colr(t::FG_FAINT));
    }

    let mut grid = row![
        container(icon(icon_svg, 16.0, icon_color))
            .width(Length::Fixed(18.0))
            .align_x(cosmic::iced::alignment::Horizontal::Center),
        container(name_row).width(Length::Fill),
    ]
    .spacing(t::SIDE_ROW_GAP)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    if let Some(m) = meta {
        grid = grid.push(text(m).size(10).colr(t::FG_FAINT));
    }

    let inner = container(grid)
        .padding(Padding {
            top: t::SIDE_ROW_PAD_Y,
            right: t::SIDE_ROW_PAD_X,
            bottom: t::SIDE_ROW_PAD_Y,
            // leave room for the 2-px active border
            left: t::SIDE_ROW_PAD_X - 2.0,
        })
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border_color,
                width: 2.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

    button(inner)
        .padding(0)
        .sty(|_, _| button::Style {
            snap: false,
            background: Some(Background::Color(Color::TRANSPARENT)),
            text_color: t::FG_DIM,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..button::Style::default()
        })
        .on_press(msg)
        .into()
}

/// Sidebar caps section header (`◆ Mesh` or `Local`).
pub fn side_section_header(
    label: &str,
    meta: &str,
    mesh_tinted: bool,
) -> Element<'static, Message> {
    let fg = if mesh_tinted {
        t::ACCENT_HI
    } else {
        t::FG_FAINT
    };
    container(
        row![
            text(label.to_uppercase()).size(10).colr(fg),
            Space::new().width(Length::Fill),
            text(meta.to_string()).size(10).colr(t::FG_FAINT),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding {
        top: 10.0,
        right: 14.0,
        bottom: 4.0,
        left: 14.0,
    })
    .into()
}

/// "Browse filesystem…" dashed disclosure row at the bottom of the sidebar.
pub fn disclosure_row(open: bool, msg: Message) -> Element<'static, Message> {
    let chevron = if open {
        icons::CHEVRON_DOWN
    } else {
        icons::CHEVRON_RIGHT
    };
    let inner = container(
        row![
            icon(chevron, 14.0, t::FG_FAINT),
            text("Browse filesystem…").size(12).colr(t::FG_DIM),
            Space::new().width(Length::Fill),
            text("/").size(10).colr(t::FG_FAINT),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8.0, 12.0]))
    .sty(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(if open {
            Color {
                a: 0.04,
                ..Color::WHITE
            }
        } else {
            Color {
                a: 0.02,
                ..Color::WHITE
            }
        })),
        border: Border {
            color: Color {
                a: if open { 0.18 } else { 0.10 },
                ..Color::WHITE
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    container(
        button(inner)
            .padding(0)
            .sty(|_, _| ghost_button_style())
            .on_press(msg),
    )
    .padding(Padding {
        top: 8.0,
        right: 12.0,
        bottom: 4.0,
        left: 12.0,
    })
    .into()
}

// ─── Mime helper ───────────────────────────────────────────────────────────

#[must_use]
pub const fn mime_label(mime: Mime) -> &'static str {
    match mime {
        Mime::Folder => "folder",
        Mime::Doc => "doc",
        Mime::Image => "image",
        Mime::Pdf => "pdf",
        Mime::Archive => "archive",
        Mime::Disk => "disk",
    }
}

#[must_use]
pub const fn peer_kind_label(kind: PeerKind) -> &'static str {
    match kind {
        PeerKind::Desktop => "desktop",
        PeerKind::Server => "server",
        PeerKind::Phone => "phone",
        PeerKind::Ci => "ci",
    }
}

#[cfg(test)]
mod motion_tests {
    //! MOTION-FEEDBACK — the file-row/tile motion math (hover-lift + selection
    //! accent + capped staggered reveal) derived from the shared
    //! `mde_theme::animation` helpers.
    use super::*;
    use mde_theme::animation::Animator;
    use mde_theme::motion::{list, Motion};
    use std::time::{Duration, Instant};

    fn ctx<'a>(
        anim: &'a Animator,
        hovered: Option<&'a str>,
        releasing: Option<&'a str>,
        reveal_origin: Option<Instant>,
        now: Instant,
        reduce_motion: bool,
    ) -> RowMotionCtx<'a> {
        RowMotionCtx {
            anim,
            hovered,
            releasing,
            reveal_origin,
            now,
            reduce_motion,
            focus_visible: false,
            load: crate::loading::ListingLoad::default(),
            removed: &[],
        }
    }

    #[test]
    fn vertical_padding_is_height_constant_across_the_whole_offset_range() {
        // The reserved span is split top/bottom so the cell height never changes
        // — no neighbour reflow on hover or reveal. Sweep the full offset range.
        let reserve = ROW_HOVER_RISE_PX + ROW_REVEAL_SLIDE_PX;
        for lift in [0.0, -ROW_HOVER_RISE_PX * 0.5, -ROW_HOVER_RISE_PX] {
            for reveal in [0.0, ROW_REVEAL_SLIDE_PX * 0.5, ROW_REVEAL_SLIDE_PX] {
                let m = RowMotion {
                    lift_px: lift,
                    accent_t: 0.0,
                    reveal_y: reveal,
                    ..RowMotion::default()
                };
                let (top, bottom) = m.vertical_padding();
                assert!(top >= 0.0 && bottom >= 0.0, "padding never negative");
                assert!(
                    (top + bottom - reserve).abs() < 1e-4,
                    "top+bottom must equal RESERVE at lift={lift} reveal={reveal}"
                );
            }
        }
    }

    #[test]
    fn vertical_padding_lifts_up_on_hover_and_drops_down_on_reveal() {
        // Full hover (lift = -rise) sits higher (smaller top) than rest; the
        // reveal start (reveal = +slide) sits lower (larger top) than rest.
        let rest = RowMotion::default().vertical_padding().0;
        let hovered = RowMotion {
            lift_px: -ROW_HOVER_RISE_PX,
            ..RowMotion::default()
        }
        .vertical_padding()
        .0;
        let revealing = RowMotion {
            reveal_y: ROW_REVEAL_SLIDE_PX,
            ..RowMotion::default()
        }
        .vertical_padding()
        .0;
        assert!(hovered < rest, "hover raises the row (less top padding)");
        assert!(
            revealing > rest,
            "reveal starts the row lower (more top padding)"
        );
    }

    #[test]
    fn reduce_motion_collapses_to_rest_but_keeps_selection_accent() {
        // Q32 / a11y: under reduce-motion there is NO movement (lift + reveal are
        // 0), but a selected row still shows its full accent instantly.
        let anim = Animator::new();
        let now = Instant::now();
        let c = ctx(&anim, None, None, Some(now), now, true);
        let m = c.for_row("a.txt", 3, true);
        assert_eq!(m.lift_px, 0.0, "no hover lift under reduce-motion");
        assert_eq!(m.reveal_y, 0.0, "no reveal slide under reduce-motion");
        assert!(
            (m.accent_t - 1.0).abs() < 1e-6,
            "selected ⇒ full accent instantly"
        );
        // Unselected row: no accent.
        assert_eq!(c.for_row("b.txt", 0, false).accent_t, 0.0);
    }

    #[test]
    fn reveal_stagger_is_capped_at_eight_items() {
        // Items past STAGGER_CAP share the cap delay, so a long listing doesn't
        // crawl: the row AT the cap and the row well past it begin together.
        let anim = Animator::new();
        let origin = Instant::now();
        // Sample just after the cap delay so the capped rows have begun revealing
        // (reveal_y < full slide) while earlier rows have already settled.
        let cap_delay_ms = (list::STAGGER_CAP as u64 - 1) * u64::from(list::STAGGER_STEP_MS);
        let now = origin + Duration::from_millis(cap_delay_ms);
        let c = ctx(&anim, None, None, Some(origin), now, false);
        let at_cap = c.for_row("x", list::STAGGER_CAP - 1, false).reveal_y;
        let past_cap = c.for_row("y", list::STAGGER_CAP + 50, false).reveal_y;
        assert!(
            (at_cap - past_cap).abs() < 1e-4,
            "rows at/past the cap reveal together (capped delay)"
        );
        // The very first row has already fully settled (reveal_y == 0) by now.
        assert!(c.for_row("first", 0, false).reveal_y.abs() < 1e-4);
    }

    #[test]
    fn reveal_slides_up_to_rest_over_its_window() {
        // A row's reveal starts at the full slide distance and settles to 0.
        let anim = Animator::new();
        let origin = Instant::now();
        let c0 = ctx(&anim, None, None, Some(origin), origin, false);
        assert!(
            (c0.for_row("r", 0, false).reveal_y - ROW_REVEAL_SLIDE_PX).abs() < 1e-3,
            "row 0 starts at the full slide distance"
        );
        let done = origin + Duration::from_millis(u64::from(list::STAGGER_REVEAL_MS) + 5);
        let c1 = ctx(&anim, None, None, Some(origin), done, false);
        assert!(
            c1.for_row("r", 0, false).reveal_y.abs() < 1e-3,
            "row 0 has settled to rest after its reveal window"
        );
        // No reveal origin ⇒ no slide at all.
        let c2 = ctx(&anim, None, None, None, done, false);
        assert_eq!(c2.for_row("r", 0, false).reveal_y, 0.0);
    }

    #[test]
    fn hover_lift_eases_in_while_hovered_and_out_while_releasing() {
        let now = Instant::now();
        let key = row_hover_key("file");
        let mut anim = Animator::new();
        anim.start(key.clone(), now, Motion::hover(), false);
        let dur = Motion::hover().duration;
        // Mid-hover: lifted (negative translate), magnitude below the full rise.
        let mid = now + dur / 2;
        let c = ctx(&anim, Some(key.as_str()), None, None, mid, false);
        let lift = c.for_row("file", 0, false).lift_px;
        assert!(
            lift < 0.0 && lift > -ROW_HOVER_RISE_PX,
            "easing up, got {lift}"
        );
        // Releasing at the same instant: settling back, so a smaller magnitude.
        let cr = ctx(&anim, None, Some(key.as_str()), None, mid, false);
        let rel = cr.for_row("file", 0, false).lift_px;
        assert!(
            rel < 0.0 && rel > lift,
            "release settles toward rest, got {rel}"
        );
        // A row that is neither hovered nor releasing sits at rest.
        let cn = ctx(&anim, None, None, None, mid, false);
        assert_eq!(cn.for_row("file", 0, false).lift_px, 0.0);
    }

    #[test]
    fn selection_bg_crossfades_from_base_to_full_tint() {
        // accent_t 0 ⇒ base unchanged; accent_t 1 ⇒ exactly LIST_SELECTION_BG.
        let base = Color::TRANSPARENT;
        let none = RowMotion {
            accent_t: 0.0,
            ..RowMotion::default()
        };
        assert_eq!(none.selection_bg(base), base);
        let full = RowMotion {
            accent_t: 1.0,
            ..RowMotion::default()
        };
        let got = full.selection_bg(base);
        let want = t::LIST_SELECTION_BG;
        assert!((got.r - want.r).abs() < 1e-4);
        assert!((got.g - want.g).abs() < 1e-4);
        assert!((got.b - want.b).abs() < 1e-4);
        assert!((got.a - want.a).abs() < 1e-4);
        // Midway: alpha strictly between base and full (the accent is fading in).
        let mid = RowMotion {
            accent_t: 0.5,
            ..RowMotion::default()
        }
        .selection_bg(base);
        assert!(mid.a > base.a && mid.a < want.a);
    }

    #[test]
    fn hover_and_accent_keys_are_distinct_namespaces() {
        // A hover tween and a selection-accent tween for the same row must not
        // collide (different keys) so one never disturbs the other.
        assert_ne!(row_hover_key("a"), accent_key("a"));
        assert!(row_hover_key("a").starts_with(HOVER_KEY_PREFIX));
    }

    // ── MOTION-TRANS-3 — list insert/remove + table-refresh transitions ──────

    #[test]
    fn present_row_is_fully_present_and_collapse_is_a_noop() {
        // A normal (present) row sits at presence_t == 1.0 — full height, full
        // opacity — so the collapse path is a cheap identity for every live row.
        let rm = RowMotion::default();
        assert_eq!(rm.presence_t, 1.0, "default row is fully present");
        assert_eq!(rm.collapse_height_factor(), 1.0, "no height shrink");
        assert_eq!(rm.collapse_alpha(), 1.0, "no fade");
        // for_row never dips a present row below full presence.
        let anim = Animator::new();
        let now = Instant::now();
        let c = ctx(&anim, None, None, None, now, false);
        assert_eq!(c.for_row("live.txt", 0, false).presence_t, 1.0);
    }

    #[test]
    fn removed_row_collapses_height_and_fades_then_drops() {
        // A removed row eases presence 1→0 over the collapse window: height factor
        // + alpha both shrink monotonically, and once the window elapses
        // for_removed_row yields None (the caller drops the row entirely).
        let anim = Animator::new();
        let origin = Instant::now();
        let dur = Motion::dialog_mount().duration;

        // At the start: nearly fully present (height ~1, alpha ~1).
        let c0 = ctx(&anim, None, None, None, origin, false);
        let m0 = c0
            .for_removed_row(origin)
            .expect("collapse in flight at start");
        assert!(m0.collapse_height_factor() > 0.9, "starts near full height");

        // Midway: strictly between — shrinking + fading.
        let mid = origin + dur / 2;
        let cmid = ctx(&anim, None, None, None, mid, false);
        let mmid = cmid
            .for_removed_row(origin)
            .expect("collapse in flight mid");
        let hmid = mmid.collapse_height_factor();
        let amid = mmid.collapse_alpha();
        assert!(
            hmid > 0.0 && hmid < m0.collapse_height_factor(),
            "height shrinks"
        );
        assert!((0.0..1.0).contains(&amid), "row is fading, got {amid}");

        // After the window: gone — None, so the row is dropped (no leftover).
        let done = origin + dur + Duration::from_millis(5);
        let cdone = ctx(&anim, None, None, None, done, false);
        assert!(
            cdone.for_removed_row(origin).is_none(),
            "collapse settled ⇒ row dropped"
        );
    }

    #[test]
    fn reduce_motion_removes_rows_instantly_with_no_collapse() {
        // a11y: under reduce-motion a removed row simply disappears — no collapse
        // animation is produced (for_removed_row is None at once).
        let anim = Animator::new();
        let now = Instant::now();
        let c = ctx(&anim, None, None, None, now, true);
        assert!(
            c.for_removed_row(now).is_none(),
            "reduce-motion ⇒ instant removal, no collapse"
        );
    }

    #[test]
    fn collapse_alpha_runs_from_opaque_to_transparent() {
        // The fade-out maps presence 1→0 onto alpha 1→0 (FadeOut primitive).
        let opaque = RowMotion {
            presence_t: 1.0,
            ..RowMotion::default()
        };
        assert!((opaque.collapse_alpha() - 1.0).abs() < 1e-6);
        let gone = RowMotion {
            presence_t: 0.0,
            ..RowMotion::default()
        };
        assert!(
            gone.collapse_alpha().abs() < 1e-6,
            "fully collapsed ⇒ transparent"
        );
        let half = RowMotion {
            presence_t: 0.5,
            ..RowMotion::default()
        };
        let a = half.collapse_alpha();
        assert!(a > 0.0 && a < 1.0, "mid-collapse partly faded, got {a}");
    }
}

#[cfg(test)]
mod focus_tests {
    //! POLISH-files-focusring — the keyboard-focus-ring gate the file rows/tiles
    //! consume so the 2px Carbon ring tracks real keyboard focus and never paints
    //! on a pointer hover.
    use super::focus_ring_width;
    use crate::prefs::FocusVisibility;

    #[test]
    fn ring_shows_only_for_visible_keyboard_focus() {
        // The ring is drawn only when the row holds the focus cursor AND that
        // focus is keyboard-visible — never on a focused-but-mouse row, and never
        // on an unfocused (e.g. merely hovered) row.
        assert!(
            focus_ring_width(true, true).is_some(),
            "keyboard-focused row draws the ring",
        );
        assert!(
            focus_ring_width(true, false).is_none(),
            "mouse-driven focus draws no ring (the wash stays the mouse cue)",
        );
        assert!(
            focus_ring_width(false, true).is_none(),
            "an unfocused row draws no ring even while keyboard is active",
        );
        assert!(focus_ring_width(false, false).is_none());
    }

    #[test]
    fn ring_width_is_the_2px_carbon_token() {
        // The drawn width is the shared `mde_theme::feedback` ring token (= the
        // Object Card focus-outline weight, 2px) — distinct from the 1px resting
        // border, with no scattered literal in the surface crate (§4).
        let w = focus_ring_width(true, true).expect("ring present");
        assert!((w - mde_theme::feedback::FOCUS_RING_WIDTH_PX).abs() < f32::EPSILON);
        assert!(
            (w - 2.0).abs() < f32::EPSILON,
            "the Carbon focus ring is 2px, got {w}",
        );
    }

    #[test]
    fn gate_matches_the_focus_visibility_policy() {
        // The `focus_visible` bit the rows receive is exactly the resolved
        // FocusVisibility gate, so the ring follows the policy: Auto honors the
        // keyboard-active signal; AlwaysVisible forces it on. This is the consumer
        // of the previously-dropped `FocusVisibility::Auto` signal.
        for keyboard_active in [false, true] {
            let auto = FocusVisibility::Auto.should_render(keyboard_active);
            assert_eq!(
                focus_ring_width(true, auto).is_some(),
                keyboard_active,
                "Auto ring tracks keyboard_active",
            );
            let always = FocusVisibility::AlwaysVisible.should_render(keyboard_active);
            assert!(
                focus_ring_width(true, always).is_some(),
                "AlwaysVisible always draws the ring on a focused row",
            );
        }
    }
}

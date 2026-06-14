//! The five primary views — Mesh Overview, Peer Folder, Inbox, Downloads, Local
//! Veil — plus the persistent sidebar / toolbar / titlebar chrome around them.

use crate::cosmic_compat::{ButtonSty, ContainerSty, TextSty};
use cosmic::iced::widget::{
    button, column, container, row, scrollable, text, text_input, tooltip, Space,
};
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::{Element, Theme};

use crate::a11y_labels::{self, A11yAction};
use crate::app::{Crumb, Message, TrashItem};
use crate::backend::BackendSnapshot;
use crate::grid;
use crate::icons;
use crate::model::{fmt_count, FileRow, Layout, Peer, PeerStatus, SelfNode, Tab, View};
use crate::search;
use crate::selection::Selection;
use crate::theme as t;
use crate::widgets::{
    banner, breadcrumb_tag, disclosure_row, file_row, file_row_head, ghost_button_style, icon,
    list_row, peer_card, section_h, side_row, side_section_header, tx_row, BannerStat,
    SideRowVariant,
};

// ─── Titlebar ──────────────────────────────────────────────────────────────

/// Titlebar with live peer-count status pill.
pub fn titlebar(online: usize, total: usize) -> Element<'static, Message> {
    titlebar_inner(online, total)
}

/// Titlebar carrying live peer-count status.
pub fn titlebar_with_status(online: usize, total: usize) -> Element<'static, Message> {
    titlebar_inner(online, total)
}

fn titlebar_inner(online: usize, total: usize) -> Element<'static, Message> {
    let mesh_text = format!("mesh up · {online}/{total} peers");

    let title = row![
        text("Artifact Manager").size(12).colr(t::FG),
        Space::new().width(Length::Fixed(6.0)),
        text(mesh_text).size(11).colr(t::FG_FAINT),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let app_icon = container(icon(icons::MESH_HUB, 14.0, t::ACCENT))
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(t::TITLEBAR_H))
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .align_y(cosmic::iced::alignment::Vertical::Center);

    let title_cell = container(title)
        .width(Length::Fill)
        .height(Length::Fixed(t::TITLEBAR_H))
        .padding(Padding::from([0.0, 6.0]))
        .align_y(cosmic::iced::alignment::Vertical::Center);

    let make_btn = |svg_bytes: &'static [u8], msg: Message, is_close: bool| {
        let style_fn = move |_theme: &Theme, status: button::Status| {
            let bg = match status {
                button::Status::Hovered if is_close => Color {
                    r: 0.91,
                    g: 0.07,
                    b: 0.14,
                    a: 1.0,
                },
                button::Status::Hovered => Color {
                    a: 0.08,
                    ..Color::WHITE
                },
                _ => Color::TRANSPARENT,
            };
            let fg = match status {
                button::Status::Hovered if is_close => Color::WHITE,
                button::Status::Hovered => t::FG,
                _ => t::FG_DIM,
            };
            button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: fg,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..button::Style::default()
            }
        };
        button(
            container(icon(svg_bytes, 12.0, t::FG_DIM))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center)
                .align_y(cosmic::iced::alignment::Vertical::Center),
        )
        .padding(0)
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(t::TITLEBAR_H))
        .sty(style_fn)
        .on_press(msg)
    };

    let controls = row![
        make_btn(icons::MINUS, Message::TitlebarMinimize, false),
        make_btn(icons::MAXIMIZE, Message::TitlebarMaximize, false),
        make_btn(icons::CLOSE, Message::TitlebarClose, true),
    ];

    container(
        row![app_icon, title_cell, controls].align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(t::TITLEBAR_H))
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(t::WINDOW_TITLEBAR)),
        border: Border {
            color: t::DIVIDER,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

// ─── Sidebar ───────────────────────────────────────────────────────────────

pub fn sidebar<'a>(
    view: &'a View,
    local_open: bool,
    snap: &'a BackendSnapshot,
) -> Element<'a, Message> {
    let self_node = &snap.self_node;
    let online = snap
        .peers
        .iter()
        .filter(|p| matches!(p.status, PeerStatus::Online))
        .count();
    let total = snap.peers.len();

    // Top toolbar
    let top_btn = |svg_bytes: &'static [u8], msg: Message| {
        button(icon(svg_bytes, 16.0, t::FG_DIM))
            .padding(Padding::from([4.0, 6.0]))
            .sty(|_, _| ghost_button_style())
            .on_press(msg)
    };
    let top = container(
        row![
            top_btn(icons::PANEL_RIGHT, Message::Noop),
            top_btn(icons::ARROW_LEFT, Message::SelectView(View::MeshOverview)),
            Space::new().width(Length::Fill),
            top_btn(icons::REFRESH, Message::Refresh),
        ]
        .spacing(4)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::new(6.0))
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(t::WINDOW_SIDE)),
        border: Border {
            color: t::DIVIDER,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    // MESH list
    let mut mesh_col = column![side_section_header(
        "◆ Mesh",
        &format!("{online}/{total} peers"),
        true,
    )];

    mesh_col = mesh_col.push(side_row(
        icons::MESH_HUB,
        "Network overview",
        None,
        Some((total + 1).to_string()),
        if matches!(view, View::MeshOverview) {
            SideRowVariant::Active
        } else {
            SideRowVariant::Default
        },
        Message::SelectView(View::MeshOverview),
    ));

    // AF-mesh.2 — Mesh Home entry. Routes to the XDG-dir card
    // grid; the shared XDG dirs are first-class mesh resources
    // on the LizardFS mesh store, not local.
    mesh_col = mesh_col.push(side_row(
        icons::FOLDER,
        "Mesh Home",
        None,
        Some(MESH_HOME_DIRS.len().to_string()),
        if matches!(view, View::MeshHome | View::MeshHomeChild(_)) {
            SideRowVariant::Active
        } else {
            SideRowVariant::Default
        },
        Message::SelectView(View::MeshHome),
    ));

    // MESHFS-8.1 — Recycle Bin entry (LizardFS `.trash` virtual directory).
    mesh_col = mesh_col.push(side_row(
        icons::TRASH2,
        "Recycle Bin",
        None,
        None,
        if matches!(view, View::MeshUndelete) {
            SideRowVariant::Active
        } else {
            SideRowVariant::Default
        },
        Message::SelectView(View::MeshUndelete),
    ));

    // Self row (rust-coloured "you" label).
    let self_label = format!("{}  · you", self_node.host);
    mesh_col = mesh_col.push(side_row(
        icons::MESH_HUB,
        &self_label,
        None,
        Some(self_node.shared.to_string()),
        SideRowVariant::Peer {
            status: PeerStatus::Self_,
            active: false,
        },
        Message::Noop,
    ));

    for p in &snap.peers {
        let label_with_lat = match p.latency {
            Some(ms) => format!("{}  · {}ms", p.host, ms),
            None => p.host.to_string(),
        };
        let active = matches!(view, View::Peer(id) if id == &p.id);
        mesh_col = mesh_col.push(side_row(
            icons::MESH_HUB,
            &label_with_lat,
            None,
            Some(if p.shared > 0 {
                fmt_count(p.shared)
            } else {
                "—".into()
            }),
            SideRowVariant::Peer {
                status: p.status,
                active,
            },
            Message::SelectView(View::Peer(p.id.clone())),
        ));
    }

    mesh_col = mesh_col.push(Space::new().height(Length::Fixed(4.0)));
    mesh_col = mesh_col.push(side_row(
        icons::INBOX,
        "Inbox",
        None,
        Some(snap.inbox.len().to_string()),
        if matches!(view, View::Inbox) {
            SideRowVariant::Active
        } else {
            SideRowVariant::Default
        },
        Message::SelectView(View::Inbox),
    ));
    mesh_col = mesh_col.push(side_row(
        icons::SEND,
        "Outbox",
        None,
        Some("0".to_string()),
        SideRowVariant::Default,
        Message::Noop,
    ));

    let mesh_scroll = scrollable(mesh_col.spacing(0)).height(Length::Fill);

    // LOCAL (pinned)
    let mut local_col = column![side_section_header("Local", "this device", false)];

    let downloads_variant = if matches!(view, View::Downloads) {
        SideRowVariant::PrimaryActive
    } else {
        SideRowVariant::Primary
    };
    local_col = local_col.push(side_row(
        icons::DOWNLOAD,
        "Downloads",
        None,
        Some(snap.downloads.len().to_string()),
        downloads_variant,
        Message::SelectView(View::Downloads),
    ));

    local_col = local_col.push(disclosure_row(local_open, Message::ToggleLocal));

    if local_open {
        for pin in &snap.local_pins {
            local_col = local_col.push(side_row(
                icons::svg_for_pin(pin.icon),
                &pin.name,
                None,
                None,
                SideRowVariant::Dim,
                // E10 — navigate the Local browser to the pin's real path
                // (was a dead-end to View::Local / $HOME for every pin).
                Message::LocalGoto(pin.path.clone()),
            ));
        }
    }

    // E10 — Network: interactive SMB host-browse (type host → shares → mount).
    local_col = local_col.push(side_row(
        icons::MESH_HUB,
        "Network",
        None,
        None,
        if matches!(view, View::Network) {
            SideRowVariant::Active
        } else {
            SideRowVariant::Default
        },
        Message::SelectView(View::Network),
    ));

    // E10 — Cloud Files: paired KDE-Connect devices.
    local_col = local_col.push(side_row(
        icons::HDD,
        "Cloud Files",
        None,
        None,
        if matches!(view, View::CloudDevices) {
            SideRowVariant::Active
        } else {
            SideRowVariant::Default
        },
        Message::SelectView(View::CloudDevices),
    ));

    let local_pane = container(local_col.spacing(0))
        .padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 4.0,
            left: 0.0,
        })
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(Color {
                a: 0.18,
                ..Color::BLACK
            })),
            border: Border {
                color: t::DIVIDER,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

    let foot_text = match snap.mesh_overlay.as_ref() {
        Some(o) if !o.mesh_id.is_empty() => {
            let role = if o.is_lighthouse {
                "lighthouse"
            } else {
                "peer"
            };
            format!("{} · {} · CA #{}", o.mesh_id, role, o.ca_epoch)
        }
        Some(_) => "nebula · enrolled".into(),
        None => "nebula offline".into(),
    };
    let foot = container(
        row![
            text(foot_text).size(11).colr(t::FG_FAINT),
            Space::new().width(Length::Fill),
            button(
                row![
                    icon(icons::PLUS, 12.0, t::ACCENT_HI),
                    text("Peer").size(11).colr(t::ACCENT_HI),
                ]
                .spacing(6),
            )
            .padding(Padding::from([4.0, 8.0]))
            .sty(|_, _| button::Style {
                snap: false,
                background: Some(Background::Color(Color {
                    a: 0.10,
                    ..t::ACCENT
                })),
                text_color: t::ACCENT_HI,
                border: Border {
                    color: Color {
                        a: 0.30,
                        ..t::ACCENT
                    },
                    width: 1.0,
                    radius: 0.0.into()
                },
                ..button::Style::default()
            })
            .on_press(Message::Noop),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([10.0, 14.0]))
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(t::WINDOW_SIDE)),
        border: Border {
            color: t::DIVIDER,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    let col = column![top, mesh_scroll, local_pane, foot]
        .spacing(0)
        .height(Length::Fill);

    container(col)
        .width(Length::Fixed(t::SIDEBAR_W))
        .height(Length::Fill)
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(t::WINDOW_SIDE)),
            border: Border {
                color: t::DIVIDER,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ─── Tab strip (E10.5) ─────────────────────────────────────────────────────

/// The browser-tab strip: one chip per open tab (click to switch, × to close)
/// plus a trailing `+` new-tab button. Rendered above the toolbar.
pub fn tab_strip(tabs: &[Tab], active: usize) -> Element<'static, Message> {
    let mut strip = row![]
        .spacing(2.0)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let show_close = tabs.len() > 1;
    for (i, tab) in tabs.iter().enumerate() {
        let is_active = i == active;
        let mut chip =
            row![text(tab.title())
                .size(12)
                .colr(if is_active { t::FG } else { t::FG_DIM })]
            .spacing(6.0)
            .align_y(cosmic::iced::alignment::Vertical::Center);
        if show_close {
            chip = chip.push(
                button(icon(icons::CLOSE, 11.0, t::FG_FAINT))
                    .on_press(Message::CloseTab(i))
                    .padding(2.0)
                    .sty(|_t: &Theme, status: button::Status| button::Style {
                        snap: false,
                        background: matches!(status, button::Status::Hovered).then_some(
                            Background::Color(Color {
                                a: 0.10,
                                ..Color::WHITE
                            }),
                        ),
                        text_color: t::FG,
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: 3.0.into(),
                        },
                        ..button::Style::default()
                    }),
            );
        }
        strip = strip.push(
            button(chip)
                .on_press(Message::SwitchTab(i))
                .padding(Padding::from([5.0, 10.0]))
                .sty(move |_t: &Theme, status: button::Status| {
                    let hot = matches!(status, button::Status::Hovered);
                    button::Style {
                        snap: false,
                        background: Some(Background::Color(if is_active {
                            t::PF_BG_300
                        } else if hot {
                            t::ROW_HOVER
                        } else {
                            Color::TRANSPARENT
                        })),
                        text_color: t::FG,
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: 0.0.into(),
                        },
                        ..button::Style::default()
                    }
                }),
        );
    }
    strip = strip.push(
        button(icon(icons::PLUS, 13.0, t::FG_DIM))
            .on_press(Message::NewTab)
            .padding(Padding::from([5.0, 8.0]))
            .sty(|_t: &Theme, status: button::Status| button::Style {
                snap: false,
                background: matches!(status, button::Status::Hovered)
                    .then_some(Background::Color(t::ROW_HOVER)),
                text_color: t::FG,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..button::Style::default()
            }),
    );
    container(strip)
        .width(Length::Fill)
        .padding(Padding::from([3.0, 8.0]))
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(t::WINDOW_TITLEBAR)),
            border: Border {
                color: Color {
                    a: 0.08,
                    ..Color::WHITE
                },
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ─── Toolbar (`.fm-toolbar`) ───────────────────────────────────────────────

pub fn toolbar<'a>(
    view: &'a View,
    layout: Layout,
    search: &'a str,
    crumbs: Vec<Crumb>,
) -> Element<'a, Message> {
    let mut crumb_row = row![]
        .spacing(6)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    for (i, c) in crumbs.iter().enumerate() {
        if i > 0 {
            crumb_row = crumb_row.push(text("/").size(12).colr(t::FG_FAINT));
        }
        let is_last = i == crumbs.len() - 1;
        let fg = if c.mesh {
            t::ACCENT_HI
        } else if is_last {
            t::FG
        } else {
            t::FG_DIM
        };
        crumb_row = crumb_row.push(text(c.label.clone()).size(12).colr(fg));
    }
    let is_mesh = crumbs.iter().any(|c| c.mesh);
    crumb_row = crumb_row.push(breadcrumb_tag(
        if is_mesh { "MESH" } else { "LOCAL" },
        is_mesh,
    ));

    let placeholder = if view.is_mesh() {
        "Search mesh…"
    } else {
        "Search…"
    };
    let search_widget = container(
        row![
            icon(icons::SEARCH, 14.0, t::FG_DIM),
            text_input(placeholder, search)
                .on_input(Message::SearchChanged)
                .size(12)
                .padding(0)
                .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([4.0, 8.0]))
    .width(Length::Fixed(220.0))
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(Color {
            a: 0.05,
            ..Color::WHITE
        })),
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    let list_active = matches!(layout, Layout::List);
    let grid_active = matches!(layout, Layout::Grid);
    // v3.0.3 — every icon-only button gets a tooltip via
    // `a11y_labels::label_for`. The tooltip is both a hover
    // affordance + the accessibility label screen readers pick
    // up (Iced's tooltip widget is the closest standard
    // mechanism in 0.13 for "this button means X").
    let view_toggle = container(
        row![
            tooltip(
                view_toggle_btn(
                    icons::LIST_VIEW,
                    list_active,
                    Message::SetLayout(Layout::List)
                ),
                text(a11y_labels::label_for(A11yAction::ToolbarSetLayoutList)).size(11),
                tooltip::Position::Bottom,
            ),
            tooltip(
                view_toggle_btn(
                    icons::GRID_VIEW,
                    grid_active,
                    Message::SetLayout(Layout::Grid)
                ),
                text(a11y_labels::label_for(A11yAction::ToolbarSetLayoutGrid)).size(11),
                tooltip::Position::Bottom,
            ),
        ]
        .spacing(0),
    )
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(Color::TRANSPARENT)),
        border: Border {
            color: t::DIVIDER,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    let primary = primary_action(view);

    container(
        row![
            crumb_row,
            Space::new().width(Length::Fill),
            search_widget,
            view_toggle,
            primary,
        ]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8.0, 16.0]))
    .width(Length::Fill)
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(t::PF_BG_200)),
        border: Border {
            color: t::DIVIDER,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn view_toggle_btn(
    svg_bytes: &'static [u8],
    active: bool,
    msg: Message,
) -> Element<'static, Message> {
    let bg = if active {
        Color {
            a: 0.14,
            ..t::BUTTON_ACCENT
        }
    } else {
        Color::TRANSPARENT
    };
    let fg = if active { t::BUTTON_ACCENT } else { t::FG_DIM };
    button(
        container(icon(svg_bytes, 14.0, fg))
            .width(Length::Fixed(28.0))
            .height(Length::Fixed(24.0))
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(0)
    .sty(move |_, _| button::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 4.0.into(),
        },
        ..button::Style::default()
    })
    .on_press(msg)
    .into()
}

fn primary_action(view: &View) -> Element<'static, Message> {
    let (label, icon_svg, ghost) = if view.is_mesh() {
        ("Send", icons::SEND, false)
    } else if matches!(view, View::Downloads) {
        ("Share", icons::UPLOAD, false)
    } else {
        ("New", icons::FOLDER, true) // voice-allow:idiom-file-new (file-manager idiom predates lock)
    };

    let inner = row![
        icon(
            icon_svg,
            13.0,
            if ghost {
                t::BUTTON_ACCENT
            } else {
                Color::WHITE
            },
        ),
        text(label.to_string()).size(12).colr(if ghost {
            t::BUTTON_ACCENT
        } else {
            Color::WHITE
        }),
    ]
    .spacing(6)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let btn = button(inner)
        .padding(Padding {
            top: 0.0,
            right: 12.0,
            bottom: 0.0,
            left: 12.0,
        })
        .height(Length::Fixed(32.0))
        .on_press(Message::PrimaryAction);

    if ghost {
        btn.sty(|_, _| button::Style {
            snap: false,
            background: Some(Background::Color(Color::TRANSPARENT)),
            text_color: t::BUTTON_ACCENT,
            border: Border {
                color: t::BUTTON_ACCENT,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..button::Style::default()
        })
        .into()
    } else {
        btn.sty(|_, status| {
            let bg = if matches!(status, button::Status::Hovered) {
                t::BUTTON_ACCENT_HI
            } else {
                t::BUTTON_ACCENT
            };
            button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 4.0.into(),
                },
                ..button::Style::default()
            }
        })
        .into()
    }
}

// ─── Mesh overview ─────────────────────────────────────────────────────────

pub fn mesh_overview<'a>(snap: &'a BackendSnapshot) -> Element<'a, Message> {
    let self_node = &snap.self_node;
    let online = snap
        .peers
        .iter()
        .filter(|p| matches!(p.status, PeerStatus::Online))
        .count();
    let total = snap.peers.len();
    let total_shared: u64 =
        u64::from(self_node.shared) + snap.peers.iter().map(|p| u64::from(p.shared)).sum::<u64>();

    let banner_widget = banner(
        icons::MESH_HUB,
        format!("Mesh is up · {online} of {total} peers reachable"),
        format!(
            "overlay · {host} ({addr}) · {shared} of {files} files shared by this node",
            host = self_node.host,
            addr = self_node.addr,
            shared = self_node.shared,
            files = self_node.files,
        ),
        vec![
            BannerStat::new(online.to_string(), "Online"),
            BannerStat::new(total_shared.to_string(), "Shared"),
        ],
    );

    let card_children: Vec<Element<'_, Message>> =
        snap.peers.iter().cloned().map(peer_card).collect();
    let cards = cosmic::iced::widget::Row::with_children(card_children)
        .spacing(10)
        .wrap();

    let mut tx = column![].spacing(0);
    for transfer in &snap.recent_transfers {
        tx = tx.push(tx_row(transfer.clone()));
    }

    column![
        banner_widget,
        Space::new().height(Length::Fixed(22.0)),
        section_h(
            &format!("Peers · {total}"),
            Some("tailnet · sorted by latency")
        ),
        cards,
        section_h("Recent mesh transfers", Some("last 24 h")),
        tx,
    ]
    .spacing(0)
    .into()
}

// ─── Peer folder ───────────────────────────────────────────────────────────

pub fn peer_folder<'a>(
    peer: &'a Peer,
    self_node: &'a SelfNode,
    files: Vec<FileRow>,
    search_query: &'a str,
    layout: Layout,
    selection: &'a Selection,
) -> Element<'a, Message> {
    let kind_icon = icons::svg_for_peer_kind(peer.kind);
    let lat_or_last = match peer.latency {
        Some(ms) => format!("{ms} ms"),
        None => format!("last seen {}", peer.last),
    };

    let banner_widget = banner(
        kind_icon,
        format!("{}  · {}", peer.host, peer.label),
        format!(
            "{addr} · {lat} · {shared} files shared with this node",
            addr = peer.addr,
            lat = lat_or_last,
            shared = peer.shared,
        ),
        vec![
            BannerStat::new(fmt_count(peer.files), "Total files"),
            BannerStat::new(fmt_count(peer.shared), "Shared"),
        ],
    );

    // v3.0.3 Phase 1.8 wiring — when the toolbar's search input has
    // text, filter the visible rows via `search::filter_rows`.
    // `search::is_active` is the same emptiness check; using both
    // keeps the helpers reachable per §0.8 gate 7.
    let rows_with_origin: Vec<FileRow> = files
        .iter()
        .map(|f| {
            let mut r = f.clone();
            if r.from.is_none() {
                r.from = Some(peer.host.clone());
            }
            r
        })
        .collect();
    let filtered_rows: Vec<FileRow> = if search::is_active(search_query) {
        search::filter_rows(&rows_with_origin, search_query)
    } else {
        rows_with_origin.clone()
    };

    let _tile = grid::tile_layout(800, filtered_rows.len());
    let _tile_meta = grid::tile_metadata_for(&filtered_rows);

    // CR-4.d: List → 28 px tabular rows; Grid → Object Cards (CR-4.b).
    let mut list = match layout {
        Layout::List => column![file_row_head("Origin")],
        Layout::Grid => column![],
    };
    for f in &filtered_rows {
        let sel = selection.is_selected(&f.name);
        let foc = selection.is_focused(&f.name);
        let row_el = match layout {
            Layout::List => list_row(f.clone(), true, sel, foc),
            Layout::Grid => file_row(f.clone(), true, sel, foc),
        };
        list = list.push(row_el);
    }

    let count_label = if search::is_active(search_query) {
        format!(
            "{} of {} items match \"{}\"",
            filtered_rows.len(),
            files.len(),
            search_query
        )
    } else {
        format!("{} items", filtered_rows.len())
    };

    let _ = self_node;
    column![
        banner_widget,
        Space::new().height(Length::Fixed(22.0)),
        section_h("Shared with this node", Some(&count_label)),
        list,
    ]
    .spacing(0)
    .into()
}

// ─── Inbox ─────────────────────────────────────────────────────────────────

pub fn inbox<'a>(snap: &'a BackendSnapshot, selection: &'a Selection) -> Element<'a, Message> {
    let self_node = &snap.self_node;
    let unique_senders = {
        let mut hosts: Vec<&str> = snap
            .inbox
            .iter()
            .filter_map(|f| f.from.as_deref())
            .collect();
        hosts.sort_unstable();
        hosts.dedup();
        hosts.len()
    };

    let banner_widget = banner(
        icons::INBOX,
        "Mesh inbox".to_string(),
        format!(
            "files peers sent to {} · auto-routed to ~/mesh/inbox/",
            self_node.host
        ),
        vec![
            BannerStat::new(snap.inbox.len().to_string(), "Items"),
            BannerStat::new(unique_senders.to_string(), "From peers"),
        ],
    );

    let mut list = column![file_row_head("From")];
    for f in &snap.inbox {
        let sel = selection.is_selected(&f.name);
        let foc = selection.is_focused(&f.name);
        list = list.push(file_row(f.clone(), true, sel, foc));
    }

    column![
        banner_widget,
        Space::new().height(Length::Fixed(22.0)),
        list,
    ]
    .spacing(0)
    .into()
}

// ─── Downloads ─────────────────────────────────────────────────────────────

pub fn downloads<'a>(snap: &'a BackendSnapshot, selection: &'a Selection) -> Element<'a, Message> {
    let mesh_count = snap.downloads.iter().filter(|d| d.mesh.is_some()).count();

    let banner_widget = banner(
        icons::DOWNLOAD,
        "Downloads  · ~/Downloads".to_string(),
        format!(
            "local downloads · {mesh_count} item{plural} arrived via mesh transfer",
            plural = if mesh_count == 1 { "" } else { "s" }
        ),
        vec![
            BannerStat::new(snap.downloads.len().to_string(), "Items"),
            BannerStat::new(mesh_count.to_string(), "From mesh"),
        ],
    );

    let mut list = column![file_row_head("Origin")];
    for f in &snap.downloads {
        let sel = selection.is_selected(&f.name);
        let foc = selection.is_focused(&f.name);
        list = list.push(file_row(f.clone(), true, sel, foc));
    }

    column![
        banner_widget,
        Space::new().height(Length::Fixed(22.0)),
        list,
    ]
    .spacing(0)
    .into()
}

// ─── Local veil ────────────────────────────────────────────────────────────

/// E10 — the real local-filesystem browser (replaces the old privacy veil).
/// Renders the files in `path`; right-click → Open descends a folder / launches
/// a file (handled in the reducer), and the back button ascends to the parent.
pub fn local_browser<'a>(
    files: &'a [crate::model::FileRow],
    path: &'a str,
    selection: &'a Selection,
) -> Element<'a, Message> {
    let up = button(icon(icons::ARROW_LEFT, 16.0, t::FG))
        .on_press(Message::LocalUp)
        .sty(|_, _| ghost_button_style());
    let header = row![
        up,
        icon(icons::HDD, 18.0, t::FG),
        text(path.to_string()).size(13).colr(t::FG),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut list = column![file_row_head("Name")];
    if files.is_empty() {
        list = list.push(
            container(text("Empty folder").size(12).colr(t::FG_DIM))
                .padding(Padding::from([8.0, 4.0])),
        );
    } else {
        for f in files {
            let sel = selection.is_selected(&f.name);
            let foc = selection.is_focused(&f.name);
            list = list.push(file_row(f.clone(), true, sel, foc));
        }
    }

    column![header, Space::new().height(Length::Fixed(12.0)), list,]
        .spacing(0)
        .padding(Padding::from([20.0, 22.0]))
        .into()
}

/// E10 — Cloud Files: the paired KDE-Connect device roster (over the Bus) as
/// device rows. Honest empty state when nothing is paired / mackesd is down.
pub fn cloud_devices<'a>(
    files: &'a [crate::model::FileRow],
    selection: &'a Selection,
) -> Element<'a, Message> {
    let header = row![
        icon(icons::HDD, 18.0, t::FG),
        text("Cloud Files · paired devices").size(13).colr(t::FG),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut list = column![file_row_head("Device")];
    if files.is_empty() {
        list = list.push(
            container(
                text("No paired devices — pair one from Settings ▸ Mobile Devices.")
                    .size(12)
                    .colr(t::FG_DIM),
            )
            .padding(Padding::from([8.0, 4.0])),
        );
    } else {
        for f in files {
            let sel = selection.is_selected(&f.name);
            let foc = selection.is_focused(&f.name);
            list = list.push(file_row(f.clone(), true, sel, foc));
        }
    }

    column![header, Space::new().height(Length::Fixed(12.0)), list,]
        .spacing(0)
        .padding(Padding::from([20.0, 22.0]))
        .into()
}

/// E10 — interactive SMB Network browse: a host box + Browse, then the host's
/// Disk shares; clicking a share mounts it over GVfs and opens it.
pub fn network<'a>(
    host: &'a str,
    shares: &'a [String],
    status: Option<&'a str>,
) -> Element<'a, Message> {
    let header = row![
        icon(icons::MESH_HUB, 18.0, t::FG),
        text("Network · SMB shares").size(13).colr(t::FG),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let host_box = text_input("SMB host (e.g. nas.local or 10.0.0.4)", host)
        .on_input(Message::NetHostChanged)
        .on_submit(Message::NetBrowse)
        .size(13)
        .padding(Padding::from([6.0, 8.0]));
    let browse = button(text("Browse").size(13).colr(t::FG))
        .on_press(Message::NetBrowse)
        .padding(Padding::from([6.0, 12.0]))
        .sty(|_, _| ghost_button_style());
    let input_row = row![host_box, browse]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut list = column![];
    if let Some(s) = status {
        list = list.push(text(s.to_string()).size(12).colr(t::FG_DIM));
    }
    if !shares.is_empty() {
        list = list.push(file_row_head("Share"));
        for sh in shares {
            let r = button(
                row![
                    icon(icons::HDD, 16.0, t::FG_DIM),
                    text(sh.to_string()).size(13).colr(t::FG),
                ]
                .spacing(8)
                .align_y(cosmic::iced::alignment::Vertical::Center),
            )
            .on_press(Message::NetMount(sh.clone()))
            .padding(Padding::from([6.0, 8.0]))
            .width(Length::Fill)
            .sty(|_, _| ghost_button_style());
            list = list.push(r);
        }
    }

    column![
        header,
        Space::new().height(Length::Fixed(10.0)),
        input_row,
        Space::new().height(Length::Fixed(12.0)),
        list.spacing(2),
    ]
    .spacing(0)
    .padding(Padding::from([20.0, 22.0]))
    .into()
}

// ─── Mesh Home (AF-mesh.2) ────────────────────────────────────────────────

/// Landing card grid for the five shared XDG dirs. These dirs
/// live on the LizardFS mesh store, replicated across the fleet
/// over Nebula, so they're first-class mesh resources — not
/// local files. The page is the operator's primary entry into
/// the shared file plane.
pub fn mesh_home<'a>(snap: &'a BackendSnapshot) -> Element<'a, Message> {
    let peer_count = snap.peers.len();
    let vol_summary = vec![BannerStat::new(peer_count.to_string(), "Peers")];
    let mount_subtitle = if peer_count > 0 {
        format!(
            "mesh-storage active · {}",
            mackes_mesh_types::peers::default_workgroup_root().display()
        )
    } else {
        "mesh-storage pending · no peers enrolled yet".into()
    };

    let banner_widget = banner(
        icons::MESH_HUB,
        "Mesh Storage".to_string(),
        mount_subtitle,
        vol_summary,
    );

    // Five cards — Documents · Pictures · Music · Videos · Downloads.
    // Each card routes to MeshHomeChild(slug).
    let cards: Vec<Element<'_, Message>> = MESH_HOME_DIRS
        .iter()
        .map(|(slug, label, pin_icon)| mesh_home_card(slug, label, *pin_icon))
        .collect();
    let card_grid = cosmic::iced::widget::Row::with_children(cards)
        .spacing(10)
        .wrap();

    column![
        banner_widget,
        Space::new().height(Length::Fixed(22.0)),
        section_h(
            "Shared directories",
            Some("auto-synced across every peer in the mesh")
        ),
        card_grid,
    ]
    .spacing(0)
    .into()
}

/// File listing inside one of the shared XDG dirs. Reads
/// from `local:<slug>` via the backend (which today is the
/// `LocalFsBackend` path) — once the LizardFS mount backs the
/// XDG dirs the listing is the same disk read but the content
/// reflects mesh-replicated state.
///
/// AF-mesh.3 — subdirectory navigation. When `path` is non-
/// empty the page shows a parent-link affordance ("↑ <prev>")
/// and folder rows render as clickable buttons that dispatch
/// `Message::MeshFolderEnter`. File rows stay non-clickable;
/// future commits add per-file actions.
pub fn mesh_home_child<'a>(
    slug: &'a str,
    files: Vec<FileRow>,
    search: &'a str,
    _layout: Layout,
    path: &'a [String],
    selection: &'a Selection,
) -> Element<'a, Message> {
    let label = crate::app::mesh_home_label(slug);
    let filtered: Vec<FileRow> = if search::is_active(search) {
        search::filter_rows(&files, search)
    } else {
        files
    };
    let count = filtered.len();
    let sub = path.join("/");
    let banner_subtitle = if path.is_empty() {
        format!(
            "{count} item{plural} · mesh-replicated via mesh-storage",
            plural = if count == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{count} item{plural} · ~/{label}/{sub}",
            plural = if count == 1 { "" } else { "s" }
        )
    };
    let banner_title = if path.is_empty() {
        format!("Mesh Home · {label}")
    } else {
        format!("Mesh Home · {label}/{sub}")
    };
    let banner_widget = banner(
        icons::FOLDER,
        banner_title,
        banner_subtitle,
        vec![BannerStat::new(count.to_string(), "Items")],
    );

    let mut list = column![file_row_head("Modified")];
    // Parent-link row when descended at least one level.
    if !path.is_empty() {
        list = list.push(parent_link_row());
    }
    for f in filtered {
        let is_folder = f.name.ends_with('/') || matches!(f.mime, crate::model::Mime::Folder);
        if is_folder {
            // Clickable folder row. Strip trailing `/` for the
            // message payload so the reducer compares clean
            // names against the path stack.
            list = list.push(folder_row_button(f));
        } else {
            let sel = selection.is_selected(&f.name);
            let foc = selection.is_focused(&f.name);
            list = list.push(file_row(f, false, sel, foc));
        }
    }

    column![
        banner_widget,
        Space::new().height(Length::Fixed(22.0)),
        list.spacing(0),
    ]
    .spacing(0)
    .into()
}

/// Parent-link row for nested Mesh Home navigation. Mirrors the
/// shape of `file_row` so the list looks continuous; clicking
/// dispatches `Message::MeshFolderUp`.
fn parent_link_row() -> Element<'static, Message> {
    button(
        container(
            row![
                icon(icons::ARROW_LEFT, 14.0, t::FG_DIM),
                Space::new().width(Length::Fixed(8.0)),
                text("..").size(12).colr(t::FG_DIM),
                Space::new().width(Length::Fill),
                text("parent folder").size(10).colr(t::FG_FAINT),
            ]
            .align_y(cosmic::iced::alignment::Vertical::Center),
        )
        .padding(Padding::from([6.0, 12.0]))
        .width(Length::Fill),
    )
    .padding(0)
    .sty(|_, _| ghost_button_style())
    .on_press(Message::MeshFolderUp)
    .into()
}

/// Clickable folder row used inside `mesh_home_child`. Renders
/// the same shape as `file_row` but the whole row is a button
/// that dispatches `Message::MeshFolderEnter(name)`.
/// CR-4 — folder navigation row renders as a CardSize::Small
/// Object Card per docs/design/chromeos-classic-spec.md §Object
/// Cards. Title: folder name; subtitle: `<size> · <age>` (size +
/// last-modified condensed into the one-line compact-shape slot
/// per the round-4 re-ask 2026-05-24). Wrapped in a button so the
/// card is the click target for `MeshFolderEnter`.
///
/// File-row retrofit (per-view file enumeration through
/// `widgets::file_row`) tracked as CR-4.b — share this same
/// `mde_iced_components::object_card` call once the file-row
/// data shape (name + size + mtime + selection state) maps
/// cleanly onto the Card schema.
fn folder_row_button(f: FileRow) -> Element<'static, Message> {
    let name_payload = f.name.clone();
    let display = f.name.trim_end_matches('/').to_owned();
    let subtitle = match (f.size.is_empty(), f.age.is_empty()) {
        (true, true) => String::new(),
        (true, false) => f.age.clone(),
        (false, true) => f.size.clone(),
        (false, false) => format!("{} · {}", f.size, f.age),
    };
    let palette = t::mde_files_palette();
    let mut card = mde_theme::ObjectCard::small(mde_theme::Icon::Fleet, format!("{display}/"));
    if !subtitle.is_empty() {
        card = card.with_subtitle(subtitle);
    }
    button(crate::widgets::object_card(card, palette))
        .padding(0)
        .sty(|_, _| ghost_button_style())
        .on_press(Message::MeshFolderEnter(name_payload))
        .into()
}

/// The five mesh-home shortcut slugs the sidebar + the
/// MeshHome card grid both consume. Stays a single source of
/// truth so adding a sixth directory means changing one
/// constant.
pub const MESH_HOME_DIRS: &[(&str, &str, crate::model::PinIcon)] = &[
    ("docs", "Documents", crate::model::PinIcon::Doc2),
    ("pics", "Pictures", crate::model::PinIcon::Image),
    ("music", "Music", crate::model::PinIcon::Doc),
    ("videos", "Videos", crate::model::PinIcon::Player),
    ("downloads", "Downloads", crate::model::PinIcon::Home),
];

fn mesh_home_card(
    slug: &'static str,
    label: &'static str,
    pin_icon: crate::model::PinIcon,
) -> Element<'static, Message> {
    let inner = container(
        column![
            row![
                icon(icons::svg_for_pin(pin_icon), 20.0, t::ACCENT),
                Space::new().width(Length::Fill),
                text("shared").size(9).colr(t::ACCENT),
            ]
            .align_y(cosmic::iced::alignment::Vertical::Center),
            Space::new().height(Length::Fixed(12.0)),
            text(label).size(14).colr(t::FG),
            text(format!("~/{label}")).size(10).colr(t::FG_FAINT),
        ]
        .spacing(2),
    )
    .padding(Padding::from([14.0, 16.0]))
    .width(Length::Fixed(180.0))
    .height(Length::Fixed(110.0))
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(Color {
            a: 0.04,
            ..Color::WHITE
        })),
        border: Border {
            color: Color {
                a: 0.12,
                ..Color::WHITE
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    button(inner)
        .padding(0)
        .on_press(Message::SelectView(crate::model::View::MeshHomeChild(
            slug.into(),
        )))
        .sty(|_, _| button::Style {
            snap: false,
            background: Some(Background::Color(Color::TRANSPARENT)),
            text_color: t::FG,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..button::Style::default()
        })
        .into()
}

// ── MESHFS-11.1: Conflict resolve dialog ────────────────────────────────────

/// Modal overlay for resolving a `.conflict-*` file pair. Renders as a
/// semi-transparent full-screen backdrop (click to dismiss) with a
/// centered card showing the two resolution options. Compose via
/// `cosmic::iced::widget::Stack::with_children(vec![base_view, resolve_conflict_dialog(...)])`.
pub fn resolve_conflict_dialog<'a>(original: &'a str, sibling: &'a str) -> Element<'a, Message> {
    let orig = original.to_owned();
    let sib = sibling.to_owned();

    let dismiss_backdrop = button(Space::new().width(Length::Fill).height(Length::Fill))
        .padding(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .sty(|_, _| button::Style {
            snap: false,
            background: Some(Background::Color(Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.55,
            })),
            text_color: Color::TRANSPARENT,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..button::Style::default()
        })
        .on_press(Message::DismissConflictDialog);

    let dialog_card = container(
        column![
            text("Merge conflict").size(14).colr(t::FG),
            Space::new().height(Length::Fixed(4.0)),
            text(format!("File: {original}")).size(12).colr(t::FG_DIM),
            Space::new().height(Length::Fixed(16.0)),
            button(
                column![
                    text("Keep original").size(12).colr(t::FG),
                    text(format!("Archive: {sibling}"))
                        .size(10)
                        .colr(t::FG_FAINT),
                ]
                .spacing(2),
            )
            .padding(Padding::from([10.0, 14.0]))
            .width(Length::Fill)
            .sty(|_, status| {
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
            .on_press(Message::ArchiveConflictFile(sib)),
            Space::new().height(Length::Fixed(8.0)),
            button(
                column![
                    text("Keep conflict copy").size(12).colr(t::FG),
                    text(format!("Archive: {original}"))
                        .size(10)
                        .colr(t::FG_FAINT),
                ]
                .spacing(2),
            )
            .padding(Padding::from([10.0, 14.0]))
            .width(Length::Fill)
            .sty(|_, status| {
                let bg = match status {
                    button::Status::Hovered => Color {
                        a: 0.08,
                        ..Color::WHITE
                    },
                    _ => Color {
                        a: 0.04,
                        ..Color::WHITE
                    },
                };
                button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: t::FG,
                    border: Border {
                        color: t::DIVIDER,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::ArchiveConflictFile(orig)),
            Space::new().height(Length::Fixed(12.0)),
            button(text("Dismiss").size(11).colr(t::FG_FAINT))
                .padding(Padding::from([4.0, 10.0]))
                .sty(|_, _| button::Style {
                    snap: false,
                    background: Some(Background::Color(Color::TRANSPARENT)),
                    text_color: t::FG_FAINT,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 0.0.into(),
                    },
                    ..button::Style::default()
                })
                .on_press(Message::DismissConflictDialog),
        ]
        .spacing(0),
    )
    .width(Length::Fixed(420.0))
    .padding(Padding::from([24.0, 28.0]))
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(t::PF_BG_200)),
        border: Border {
            color: t::PRIMARY_AMBER_BORDER,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    let centered_dialog = container(dialog_card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .align_y(cosmic::iced::alignment::Vertical::Center);

    cosmic::iced::widget::Stack::with_children(vec![
        dismiss_backdrop.into(),
        centered_dialog.into(),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

// ── MESHFS-8.1: Recycle Bin view ────────────────────────────────────────────

/// Render the LizardFS trash listing. Shows items recoverable within the
/// configured retention window (default 48 h) with a "Restore" button per
/// row. Displays a loading/error state when busy or on error.
pub fn mesh_undelete<'a>(
    items: &'a [TrashItem],
    busy: bool,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    let header = row![
        text("Recycle Bin").size(13).colr(t::FG),
        Space::new().width(Length::Fill),
        text(if busy { "Loading…" } else { "" })
            .size(11)
            .colr(t::FG_FAINT),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let body: Element<'a, Message> = if let Some(err) = error {
        text(format!("Error: {err}"))
            .size(12)
            .colr(Color {
                r: 1.0,
                g: 0.35,
                b: 0.35,
                a: 1.0,
            })
            .into()
    } else if items.is_empty() && !busy {
        text("Recycle Bin is empty — no files recoverable.")
            .size(12)
            .colr(t::FG_FAINT)
            .into()
    } else {
        let rows: Vec<Element<'a, Message>> = items.iter().map(|item| trash_row(item)).collect();
        scrollable(column(rows).spacing(2)).into()
    };

    column![header, Space::new().height(12), body]
        .spacing(4)
        .into()
}

fn trash_row(item: &TrashItem) -> Element<'_, Message> {
    let path = item.trash_path.clone();
    let restore_btn = button(text("Restore").size(11).colr(t::FG))
        .padding(Padding::from([4.0, 10.0]))
        .sty(|_, status: cosmic::iced::widget::button::Status| {
            let bg = match status {
                cosmic::iced::widget::button::Status::Hovered => Color {
                    a: 0.12,
                    ..Color::WHITE
                },
                _ => Color {
                    a: 0.07,
                    ..Color::WHITE
                },
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: t::FG,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 4.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..cosmic::iced::widget::button::Style::default()
            }
        })
        .on_press(Message::RestoreTrashItem(path));

    row![
        icon(icons::TRASH2, 14.0, t::FG_FAINT),
        text(item.name.clone())
            .size(12)
            .colr(t::FG)
            .width(Length::Fill),
        restore_btn,
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .padding(Padding::from([4.0, 0.0]))
    .into()
}

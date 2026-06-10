//! v4.0.1 WB-2.l — Network → Remote Desktop panel.
//!
//! Per-peer RDP / VNC launch surface. Reads the operator's
//! known-host list from `~/.config/mde/peer-macs.json` (cached
//! by `mackes/mesh_wol.py::cache_peer_macs`), surfaces each as
//! a row with "Connect (RDP)" / "Connect (VNC)" buttons, and
//! shells the click out to `remmina -c <proto>://<host>:<port>`.
//!
//! A manual-entry box at the top of the panel lets the operator
//! type a hostname/IP that isn't in the cached list — useful for
//! one-off connections + first-time connections before the ARP
//! cache fills.
//!
//! Chrome influence (per iteration skill Phase 0.8): Win11
//! Remote Desktop Connection legacy app — single text field at
//! the top + a "known hosts" list below + per-row Connect.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Rdp,
    Vnc,
}

impl Protocol {
    fn scheme(self) -> &'static str {
        match self {
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
        }
    }
    fn default_port(self) -> u16 {
        match self {
            Self::Rdp => 3389,
            Self::Vnc => 5900,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnownHost {
    pub ip: String,
    pub mac: String,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteDesktopPanel {
    pub hosts: Vec<KnownHost>,
    pub manual_input: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<KnownHost>),
    ReloadClicked,
    ManualInputChanged(String),
    ConnectManualClicked(Protocol),
    ConnectKnownClicked {
        host: String,
        protocol: Protocol,
    },
    LaunchFinished {
        host: String,
        protocol: Protocol,
        success: bool,
    },
}

impl RemoteDesktopPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { load_known_hosts() }, |hosts| {
            crate::Message::RemoteDesktop(Message::Loaded(hosts))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(hosts) => {
                self.hosts = hosts;
                self.status = if self.hosts.is_empty() {
                    "no cached hosts (run mackes mesh-wol cache or enter manually below)".into()
                } else {
                    format!(
                        "{} known host{}",
                        self.hosts.len(),
                        if self.hosts.len() == 1 { "" } else { "s" }
                    )
                };
                Task::none()
            }
            Message::ReloadClicked => {
                self.busy = true;
                Self::load()
            }
            Message::ManualInputChanged(s) => {
                self.manual_input = s;
                Task::none()
            }
            Message::ConnectManualClicked(proto) => {
                let host = self.manual_input.trim().to_string();
                if host.is_empty() {
                    self.status = "type a host before clicking Connect".into();
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("launching {} {host}…", proto.label());
                let h = host.clone();
                Task::perform(
                    async move {
                        let ok = launch_remmina(&h, proto).await;
                        (h, proto, ok)
                    },
                    |(host, protocol, success)| {
                        crate::Message::RemoteDesktop(Message::LaunchFinished {
                            host,
                            protocol,
                            success,
                        })
                    },
                )
            }
            Message::ConnectKnownClicked { host, protocol } => {
                self.busy = true;
                self.status = format!("launching {} {host}…", protocol.label());
                let h = host.clone();
                Task::perform(
                    async move {
                        let ok = launch_remmina(&h, protocol).await;
                        (h, protocol, ok)
                    },
                    |(host, protocol, success)| {
                        crate::Message::RemoteDesktop(Message::LaunchFinished {
                            host,
                            protocol,
                            success,
                        })
                    },
                )
            }
            Message::LaunchFinished {
                host,
                protocol,
                success,
            } => {
                self.busy = false;
                self.status = if success {
                    format!("{} {host} launched", protocol.label())
                } else {
                    format!(
                        "{} {host} failed to launch — is remmina/xfreerdp installed?",
                        protocol.label()
                    )
                };
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Remote Desktop")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        let subtitle = text(self.status.clone())
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let manual_field = row![
            text_input("hostname or IP (e.g. 172.20.146.245)", &self.manual_input)
                .on_input(|s| crate::Message::RemoteDesktop(Message::ManualInputChanged(s)))
                .padding(Padding::from([6u16, 10u16])),
            connect_btn("Connect RDP", palette, false).on_press(crate::Message::RemoteDesktop(
                Message::ConnectManualClicked(Protocol::Rdp)
            ),),
            connect_btn("Connect VNC", palette, true).on_press(crate::Message::RemoteDesktop(
                Message::ConnectManualClicked(Protocol::Vnc)
            ),),
        ]
        .spacing(6)
        .align_y(iced::alignment::Vertical::Center);

        let manual_block = container(
            column![
                text("Manual connect")
                    .size(12)
                    .color(palette.text_muted.into_iced_color()),
                manual_field,
            ]
            .spacing(6),
        )
        .padding(Padding::from([12u16, 16u16]))
        .width(Length::Fill)
        .style({
            let bg = palette.raised.into_iced_color();
            let border = palette.border.into_iced_color();
            move |_| container::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..container::Style::default()
            }
        });

        let reload_btn = button(text("Reload ARP cache").size(11).color(Color::WHITE))
            .padding(Padding::from([4u16, 10u16]))
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
                            radius: 4.0.into(),
                        },
                        shadow: iced::Shadow::default(),
                    }
                }
            })
            .on_press(crate::Message::RemoteDesktop(Message::ReloadClicked));

        let known_header = row![
            text(format!("Known hosts ({})", self.hosts.len()))
                .size(13)
                .color(palette.text.into_iced_color()),
            Space::new().width(Length::Fill),
            reload_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let mut hosts_col = column![].spacing(6);
        for h in &self.hosts {
            hosts_col = hosts_col.push(host_row(h, palette));
        }
        if self.hosts.is_empty() {
            hosts_col = hosts_col.push(
                container(
                    text("No cached hosts. Populate the ARP cache by running mackes mesh wake from a terminal, or use the manual field above.")
                        .size(12)
                        .color(palette.text_muted.into_iced_color()),
                )
                .padding(Padding::from([12u16, 0u16])),
            );
        }

        container(
            column![
                row![
                    column![title, subtitle].spacing(2),
                    Space::new().width(Length::Fill),
                ],
                Space::new().height(Length::Fixed(20.0)),
                manual_block,
                Space::new().height(Length::Fixed(16.0)),
                known_header,
                scrollable(hosts_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn host_row<'a>(h: &'a KnownHost, palette: Palette) -> Element<'a, crate::Message> {
    let resolved = mde_icon(Icon::Network, IconSize::Inline);
    let icon_color = palette.text_muted.into_iced_color();
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(icon_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .color(icon_color)
            .into()
    };
    let ip_text = text(h.ip.clone())
        .size(13)
        .color(palette.text.into_iced_color());
    let mac_text = text(h.mac.clone())
        .size(11)
        .color(palette.text_muted.into_iced_color());

    let ip = h.ip.clone();
    let rdp_btn = connect_btn("RDP", palette, false).on_press(crate::Message::RemoteDesktop(
        Message::ConnectKnownClicked {
            host: ip.clone(),
            protocol: Protocol::Rdp,
        },
    ));
    let vnc_btn = connect_btn("VNC", palette, true).on_press(crate::Message::RemoteDesktop(
        Message::ConnectKnownClicked {
            host: ip,
            protocol: Protocol::Vnc,
        },
    ));

    let body = row![
        icon_widget,
        column![ip_text, mac_text].spacing(2),
        Space::new().width(Length::Fill),
        rdp_btn,
        vnc_btn,
    ]
    .spacing(10)
    .align_y(iced::alignment::Vertical::Center);

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(body)
        .padding(Padding::from([10u16, 14u16]))
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

fn connect_btn<'a>(
    label: &'a str,
    palette: Palette,
    ghost: bool,
) -> iced::widget::Button<'a, crate::Message> {
    let accent = palette.accent.into_iced_color();
    let border = palette.border.into_iced_color();
    let text_main = palette.text.into_iced_color();
    button(
        text(label)
            .size(11)
            .color(if ghost { text_main } else { Color::WHITE }),
    )
    .padding(Padding::from([4u16, 12u16]))
    .style(move |_t: &Theme, status: iced::widget::button::Status| {
        let (bg, fg) = if ghost {
            let hover_bg = Color {
                r: 0.20,
                g: 0.20,
                b: 0.22,
                a: 1.0,
            };
            match status {
                iced::widget::button::Status::Hovered => (hover_bg, text_main),
                _ => (Color::TRANSPARENT, text_main),
            }
        } else {
            let bg = match status {
                iced::widget::button::Status::Hovered => Color {
                    r: accent.r * 1.10,
                    g: accent.g * 1.10,
                    b: accent.b * 1.10,
                    a: accent.a,
                },
                _ => accent,
            };
            (bg, Color::WHITE)
        };
        iced::widget::button::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: if ghost { border } else { Color::TRANSPARENT },
                width: if ghost { 1.0 } else { 0.0 },
                radius: 4.0.into(),
            },
            shadow: iced::Shadow::default(),
        }
    })
}

// ---- I/O ------------------------------------------------------

/// Best-effort `~/.config/mde/peer-macs.json` read. Falls back to
/// the legacy `~/.config/mackes-shell/peer-macs.json` if the new
/// path is absent (the v2.0.0 rebrand renamed the dir, see
/// state.py BUG-1 fix). Returns an empty Vec when neither file
/// exists or the JSON fails to parse.
pub fn load_known_hosts() -> Vec<KnownHost> {
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return Vec::new(),
    };
    let candidates = [
        home.join(".config/mde/peer-macs.json"),
        home.join(".config/mackes-shell/peer-macs.json"),
    ];
    for path in &candidates {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Some(hosts) = parse_peer_macs(&raw) {
                return hosts;
            }
        }
    }
    Vec::new()
}

/// Pure parser for the `peer-macs.json` shape:
/// `{ "<ip>": "<mac>", ... }`. Returns `None` on a JSON decode
/// failure (keeps the caller free to fall through to the legacy
/// path or surface an empty list).
#[must_use]
pub fn parse_peer_macs(raw: &str) -> Option<Vec<KnownHost>> {
    let map: BTreeMap<String, String> = serde_json::from_str(raw).ok()?;
    Some(
        map.into_iter()
            .map(|(ip, mac)| KnownHost { ip, mac })
            .collect(),
    )
}

/// Shell out to `remmina -c <proto>://<host>:<port>`. Returns
/// `true` when `remmina` exits 0 within the spawn window —
/// remmina detaches into its own window so this is really just
/// "did the binary launch."
pub async fn launch_remmina(host: &str, protocol: Protocol) -> bool {
    use tokio::process::Command;
    let url = format!(
        "{}://{}:{}",
        protocol.scheme(),
        host,
        protocol.default_port()
    );
    let status = Command::new("remmina").args(["-c", &url]).spawn();
    match status {
        Ok(mut child) => {
            // Don't wait — remmina opens a window + we want the
            // workbench to stay responsive. The spawn itself is
            // the success signal.
            let _ = child.try_wait();
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_scheme_round_trip() {
        assert_eq!(Protocol::Rdp.scheme(), "rdp");
        assert_eq!(Protocol::Vnc.scheme(), "vnc");
    }

    #[test]
    fn protocol_default_ports() {
        assert_eq!(Protocol::Rdp.default_port(), 3389);
        assert_eq!(Protocol::Vnc.default_port(), 5900);
    }

    #[test]
    fn parse_peer_macs_decodes_known_shape() {
        let raw = r#"{
            "10.0.0.1": "aa:bb:cc:dd:ee:01",
            "10.0.0.2": "aa:bb:cc:dd:ee:02"
        }"#;
        let hosts = parse_peer_macs(raw).expect("decoded");
        assert_eq!(hosts.len(), 2);
        // BTreeMap sorts lexicographically so 10.0.0.1 sorts
        // before 10.0.0.2.
        assert_eq!(hosts[0].ip, "10.0.0.1");
        assert_eq!(hosts[0].mac, "aa:bb:cc:dd:ee:01");
    }

    #[test]
    fn parse_peer_macs_returns_none_for_garbage() {
        assert!(parse_peer_macs("not json").is_none());
    }

    #[test]
    fn parse_peer_macs_returns_empty_for_empty_object() {
        let hosts = parse_peer_macs("{}").expect("decoded");
        assert!(hosts.is_empty());
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = RemoteDesktopPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_hosts_without_panic() {
        let mut p = RemoteDesktopPanel::new();
        p.hosts = vec![
            KnownHost {
                ip: "10.0.0.1".into(),
                mac: "aa:bb:cc:dd:ee:01".into(),
            },
            KnownHost {
                ip: "10.0.0.2".into(),
                mac: "aa:bb:cc:dd:ee:02".into(),
            },
        ];
        let _ = p.view();
    }

    #[test]
    fn manual_input_with_empty_value_does_not_launch() {
        let mut p = RemoteDesktopPanel::new();
        p.manual_input = "   ".into();
        let _ = p.update(Message::ConnectManualClicked(Protocol::Rdp));
        assert!(p.status.contains("type a host"));
    }
}

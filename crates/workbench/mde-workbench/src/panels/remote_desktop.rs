//! Network → Remote Access panel (SVC-1: SSH + RDP + VNC).
//!
//! Per-peer SSH / RDP / VNC launch surface (the v4.0.1 WB-2.l
//! Remote Desktop panel grown to absorb the retired `mesh_ssh`
//! nav stub, B1). SSH opens cosmic-term running `ssh $USER@host`
//! (the PEERS L7 lock); RDP/VNC shell to remmina. Each known
//! host is TCP-probed on port 22 so the SSH button reflects the
//! remote sshd state; the local sshd state shows in the header
//! (Q56/Q58 — local + remote visibility, no ACL).
//!
//! Reads the operator's
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

use cosmic::iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

// PD-5 / Q8 — the protocol + launch engine moved to the shared
// crate::launcher module (one engine for this panel + the Peers
// directory). Re-exported so existing call sites/tests keep working.
pub use crate::launcher::Protocol;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnownHost {
    pub ip: String,
    pub mac: String,
    /// Remote sshd state — `None` until the port-22 probe lands.
    pub sshd: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteDesktopPanel {
    pub hosts: Vec<KnownHost>,
    pub manual_input: String,
    pub status: String,
    pub busy: bool,
    /// This box's sshd state (`systemctl is-active sshd`); `None`
    /// until probed.
    pub local_sshd: Option<bool>,
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
    /// The port-22 probe for one known host resolved (SVC-1).
    SshProbed {
        host: String,
        up: bool,
    },
    /// `systemctl is-active sshd` on this box resolved (SVC-1).
    LocalSshdProbed(bool),
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
                // SVC-1 — fan out the sshd visibility probes: one local
                // systemd query + one port-22 TCP probe per known host.
                let mut probes = vec![Task::perform(probe_local_sshd(), |up| {
                    crate::Message::RemoteDesktop(Message::LocalSshdProbed(up))
                })];
                for h in &self.hosts {
                    let ip = h.ip.clone();
                    probes.push(Task::perform(
                        async move {
                            let up = probe_ssh_port(&ip).await;
                            (ip, up)
                        },
                        |(host, up)| crate::Message::RemoteDesktop(Message::SshProbed { host, up }),
                    ));
                }
                Task::batch(probes)
            }
            Message::SshProbed { host, up } => {
                if let Some(h) = self.hosts.iter_mut().find(|h| h.ip == host) {
                    h.sshd = Some(up);
                }
                Task::none()
            }
            Message::LocalSshdProbed(up) => {
                self.local_sshd = Some(up);
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

        let local_sshd_line = match self.local_sshd {
            Some(true) => "local sshd: active",
            Some(false) => "local sshd: inactive",
            None => "local sshd: checking…",
        };
        let title = text("Remote Access")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text(format!("{} · {}", self.status, local_sshd_line))
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let manual_field = row![
            text_input("hostname or IP (e.g. 172.20.146.245)", &self.manual_input)
                .on_input(|s| crate::Message::RemoteDesktop(Message::ManualInputChanged(s)))
                .padding(Padding::from([6u16, 10u16])),
            connect_btn("Connect SSH", palette, false).on_press(crate::Message::RemoteDesktop(
                Message::ConnectManualClicked(Protocol::Ssh)
            ),),
            connect_btn("Connect RDP", palette, false).on_press(crate::Message::RemoteDesktop(
                Message::ConnectManualClicked(Protocol::Rdp)
            ),),
            connect_btn("Connect VNC", palette, true).on_press(crate::Message::RemoteDesktop(
                Message::ConnectManualClicked(Protocol::Vnc)
            ),),
        ]
        .spacing(6)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let manual_block = container(
            column![
                text("Manual connect")
                    .size(12)
                    .colr(palette.text_muted.into_cosmic_color()),
                manual_field,
            ]
            .spacing(6),
        )
        .padding(Padding::from([12u16, 16u16]))
        .width(Length::Fill)
        .sty({
            let bg = palette.raised.into_cosmic_color();
            let border = palette.border.into_cosmic_color();
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

        let reload_btn = button(text("Reload ARP cache").size(11).colr(Color::WHITE))
            .padding(Padding::from([4u16, 10u16]))
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
                        icon_color: None,
                        border_radius: 4.0.into(),
                        border_width: 0.0,
                        border_color: Color::TRANSPARENT,
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: 4.0.into(),
                        },
                        shadow: cosmic::iced::Shadow::default(),
                    }
                }
            })
            .on_press(crate::Message::RemoteDesktop(Message::ReloadClicked));

        let known_header = row![
            text(format!("Known hosts ({})", self.hosts.len()))
                .size(13)
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fill),
            reload_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut hosts_col = column![].spacing(6);
        for h in &self.hosts {
            hosts_col = hosts_col.push(host_row(h, palette));
        }
        if self.hosts.is_empty() {
            hosts_col = hosts_col.push(
                container(
                    text("No cached hosts. Populate the ARP cache by running mackes mesh wake from a terminal, or use the manual field above.")
                        .size(12)
                        .colr(palette.text_muted.into_cosmic_color()),
                )
                .padding(Padding::from([12u16, 0u16])),
            );
        }

        // PLANES-2 — Remote Access is the Remmina surface (RDP/VNC client).
        let remmina = crate::panel_chrome::hero_band(
            mde_theme::hero::Hero::Remmina,
            crate::panel_chrome::pkg_version_cached("remmina").as_deref(),
            palette,
        );
        container(
            column![
                row![
                    column![title, subtitle].spacing(2),
                    Space::new().width(Length::Fill),
                    remmina,
                ]
                .align_y(cosmic::iced::Alignment::Center),
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
    let icon_color = palette.text_muted.into_cosmic_color();
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .colr(icon_color)
            .into()
    };
    let ip_text = text(h.ip.clone())
        .size(13)
        .colr(palette.text.into_cosmic_color());
    let mac_text = text(h.mac.clone())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());

    let ip = h.ip.clone();
    // SVC-1 — SSH button gates on the probed remote sshd state:
    // enabled while unknown (optimistic) or up; disabled when the
    // probe said nothing listens on 22.
    let ssh_btn = {
        let b = connect_btn(
            match h.sshd {
                Some(true) => "SSH ✓",
                Some(false) => "SSH ✗",
                None => "SSH",
            },
            palette,
            false,
        );
        if h.sshd == Some(false) {
            b
        } else {
            b.on_press(crate::Message::RemoteDesktop(
                Message::ConnectKnownClicked {
                    host: ip.clone(),
                    protocol: Protocol::Ssh,
                },
            ))
        }
    };
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
        ssh_btn,
        rdp_btn,
        vnc_btn,
    ]
    .spacing(10)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(body)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .sty(move |_| container::Style {
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
) -> cosmic::iced::widget::Button<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let text_main = palette.text.into_cosmic_color();
    let hover = palette.overlay.into_cosmic_color();
    button(
        text(label)
            .size(11)
            .colr(if ghost { text_main } else { Color::WHITE }),
    )
    .padding(Padding::from([4u16, 12u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            let (bg, fg) = if ghost {
                match status {
                    cosmic::iced::widget::button::Status::Hovered => (hover, text_main),
                    _ => (Color::TRANSPARENT, text_main),
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
                icon_color: None,
                border_radius: 4.0.into(),
                border_width: if ghost { 1.0 } else { 0.0 },
                border_color: if ghost { border } else { Color::TRANSPARENT },
                border: Border {
                    color: if ghost { border } else { Color::TRANSPARENT },
                    width: if ghost { 1.0 } else { 0.0 },
                    radius: 4.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
            }
        },
    )
}

// ---- I/O ------------------------------------------------------

/// Best-effort `~/.config/mde/peer-macs.json` read. Falls back to
/// the legacy `~/.config/mackes-shell/peer-macs.json` if the new
/// path is absent (the v2.0.0 rebrand renamed the dir, see
/// state.py BUG-1 fix). Returns an empty Vec when neither file
/// exists or the JSON fails to parse.
pub fn load_known_hosts() -> Vec<KnownHost> {
    // SUBAUDIT-A2 — start from the replicated mesh directory so every
    // enrolled peer (with a known overlay IP) is connectable, not just
    // ARP-discovered LAN hosts. The ARP/peer-macs cache is merged on top
    // for one-off LAN targets. Was: ARP cache only → "Known hosts (0)" on
    // a fresh node even with a healthy 4-node mesh.
    let mut hosts = directory_known_hosts();
    let mut seen: std::collections::BTreeSet<String> = hosts.iter().map(|h| h.ip.clone()).collect();

    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        let candidates = [
            home.join(".config/mde/peer-macs.json"),
            home.join(".config/mackes-shell/peer-macs.json"),
        ];
        for path in &candidates {
            if let Ok(raw) = std::fs::read_to_string(path) {
                if let Some(arp) = parse_peer_macs(&raw) {
                    for h in arp {
                        if seen.insert(h.ip.clone()) {
                            hosts.push(h);
                        }
                    }
                    break;
                }
            }
        }
    }
    hosts
}

/// The mesh peers from the replicated directory, as connectable known
/// hosts: `ip` = the resolvable `<host>.mesh` name, `mac` = the overlay
/// IP (shown as the secondary label). Empty when the directory is absent.
#[must_use]
pub fn directory_known_hosts() -> Vec<KnownHost> {
    let root = mackes_mesh_types::peers::default_workgroup_root();
    let mut out: Vec<KnownHost> =
        mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(&root))
            .into_iter()
            .filter_map(|rec| {
                let ip = rec.overlay_ip?;
                Some(KnownHost {
                    ip: format!("{}.mesh", rec.hostname),
                    mac: ip,
                    sshd: None,
                })
            })
            .collect();
    out.sort_by(|a, b| a.ip.cmp(&b.ip));
    out
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
            .map(|(ip, mac)| KnownHost {
                ip,
                mac,
                sshd: None,
            })
            .collect(),
    )
}

/// Shell out to `remmina -c <proto>://<host>:<port>`. Returns
/// Shared-launcher delegate (PD-5/Q8) — kept for call-site stability.
pub async fn launch_remmina(host: &str, protocol: Protocol) -> bool {
    crate::launcher::launch(host, protocol).await
}

/// SVC-1 — TCP probe of port 22 on `host` (800 ms budget). `true`
/// means something is listening — the remote sshd visibility the
/// panel renders per row.
pub async fn probe_ssh_port(host: &str) -> bool {
    use tokio::net::TcpStream;
    use tokio::time::{timeout, Duration};
    matches!(
        timeout(
            Duration::from_millis(800),
            TcpStream::connect((host, 22u16)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// SVC-1 — this box's sshd state via systemd.
pub async fn probe_local_sshd() -> bool {
    use tokio::process::Command;
    matches!(
        Command::new("systemctl")
            .args(["is-active", "--quiet", "sshd"])
            .status()
            .await,
        Ok(st) if st.success()
    )
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
                sshd: Some(true),
            },
            KnownHost {
                ip: "10.0.0.2".into(),
                mac: "aa:bb:cc:dd:ee:02".into(),
                sshd: Some(false),
            },
        ];
        let _ = p.view();
    }

    #[test]
    fn ssh_probe_result_lands_on_the_right_host() {
        let mut p = RemoteDesktopPanel::new();
        p.hosts = vec![KnownHost {
            ip: "10.0.0.9".into(),
            mac: "aa:bb:cc:dd:ee:09".into(),
            sshd: None,
        }];
        let _ = p.update(Message::SshProbed {
            host: "10.0.0.9".into(),
            up: true,
        });
        assert_eq!(p.hosts[0].sshd, Some(true));
    }

    #[test]
    fn local_sshd_probe_lands_in_state() {
        let mut p = RemoteDesktopPanel::new();
        let _ = p.update(Message::LocalSshdProbed(false));
        assert_eq!(p.local_sshd, Some(false));
    }

    #[test]
    fn ssh_protocol_has_port_22_and_scheme() {
        assert_eq!(Protocol::Ssh.default_port(), 22);
        assert_eq!(Protocol::Ssh.scheme(), "ssh");
        assert_eq!(Protocol::Ssh.label(), "SSH");
    }

    #[test]
    fn manual_input_with_empty_value_does_not_launch() {
        let mut p = RemoteDesktopPanel::new();
        p.manual_input = "   ".into();
        let _ = p.update(Message::ConnectManualClicked(Protocol::Rdp));
        assert!(p.status.contains("type a host"));
    }
}

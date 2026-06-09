//! Network → Wi-Fi & Ethernet panel — NetworkManager via
//! `nmcli`.
//!
//! CB-1.8 partial: replaces the v1.x
//! `mackes/workbench/network/wifi.py`. Two sections:
//! the list of NM connections (active state per row), and a
//! WiFi scan with a Connect button on each row.
//!
//! Password-prompted joins are intentionally out of scope —
//! they need an Iced text-input modal + secret-leak guards
//! that the v1.x panel didn't ship either. Open networks +
//! re-joining a known SSID (where NM already has the
//! credentials) work end-to-end through `nmcli connection up`.

use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Task};
use mde_theme::Palette;
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionRow {
    pub name: String,
    pub kind: String,
    pub device: String,
    pub state: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WifiNetwork {
    pub in_use: bool,
    pub ssid: String,
    pub signal: String,
    pub security: String,
}

#[derive(Debug, Clone, Default)]
pub struct WifiPanel {
    pub nm_available: bool,
    pub connections: Vec<ConnectionRow>,
    pub networks: Vec<WifiNetwork>,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        nm_available: bool,
        connections: Vec<ConnectionRow>,
        networks: Vec<WifiNetwork>,
    },
    Error(String),
    ConnectClicked(String),
    OperationFinished(Result<String, String>),
    RefreshClicked,
}

impl WifiPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let version = run_nmcli(&["--version"]).await;
                let nm_available = !version.is_empty();
                if !nm_available {
                    return Message::Loaded {
                        nm_available,
                        connections: Vec::new(),
                        networks: Vec::new(),
                    };
                }
                let conn_raw =
                    run_nmcli(&["-t", "-f", "NAME,TYPE,DEVICE,STATE", "connection", "show"]).await;
                let scan_raw = run_nmcli(&[
                    "-t",
                    "-f",
                    "IN-USE,SSID,SIGNAL,SECURITY",
                    "device",
                    "wifi",
                    "list",
                ])
                .await;
                Message::Loaded {
                    nm_available,
                    connections: parse_connections(&conn_raw),
                    networks: parse_wifi_scan(&scan_raw),
                }
            },
            crate::Message::Wifi,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                nm_available,
                connections,
                networks,
            } => {
                self.nm_available = nm_available;
                self.connections = connections;
                self.networks = networks;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::ConnectClicked(ssid) => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Connecting to {ssid}…");
                Task::perform(
                    async move { Message::OperationFinished(connect_to_ssid(&ssid).await) },
                    crate::Message::Wifi,
                )
            }
            Message::OperationFinished(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(msg) => msg,
                    Err(msg) => msg,
                };
                Self::load()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        if !self.nm_available {
            return column![
                text("NetworkManager unavailable").size(18),
                text(
                    "MDE talks to the network stack through `nmcli`. Install \
                     NetworkManager and reopen this panel.",
                )
                .size(13),
            ]
            .spacing(8)
            .width(Length::Fill)
            .into();
        }

        // UX-7.a — refresh routed through the shared button variant.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::Wifi(Message::RefreshClicked)),
            Palette::dark(),
        );

        let conn_view = self.connections.iter().fold(column![], |col, c| {
            col.push(
                row![
                    text(&c.name).width(Length::Fixed(180.0)),
                    text(&c.kind).width(Length::Fixed(120.0)),
                    text(&c.device).width(Length::Fixed(120.0)),
                    text(&c.state).width(Length::Fixed(120.0)),
                ]
                .spacing(12),
            )
        });

        let scan_view = self.networks.iter().fold(column![], |col, n| {
            let ssid = n.ssid.clone();
            let in_use_mark = if n.in_use { "✓" } else { "" };
            // UX-7.a — per-row Connect routed through Secondary.
            let connect_btn = variant_button(
                "Connect",
                ButtonVariant::Secondary,
                (!self.busy && !n.in_use)
                    .then(|| crate::Message::Wifi(Message::ConnectClicked(ssid))),
                Palette::dark(),
            );
            col.push(
                row![
                    text(in_use_mark).width(Length::Fixed(20.0)),
                    text(&n.ssid).width(Length::Fixed(220.0)),
                    text(&n.signal).width(Length::Fixed(60.0)),
                    text(&n.security).width(Length::Fixed(120.0)),
                    connect_btn,
                ]
                .spacing(12),
            )
        });

        column![
            row![refresh_btn, text(&self.status).size(13),].spacing(12),
            text("Connections").size(16),
            scrollable(container(conn_view.spacing(4))).height(Length::Fixed(180.0)),
            text("Wi-Fi networks").size(16),
            scrollable(container(scan_view.spacing(4))).height(Length::Fill),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Pure parser for `nmcli -t -f NAME,TYPE,DEVICE,STATE
/// connection show` output. One row per line, colon-
/// separated. Lines with fewer than 4 columns are skipped.
#[must_use]
pub fn parse_connections(raw: &str) -> Vec<ConnectionRow> {
    raw.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, ':').collect();
            if parts.len() < 4 {
                return None;
            }
            Some(ConnectionRow {
                name: parts[0].to_string(),
                kind: parts[1].to_string(),
                device: parts[2].to_string(),
                state: parts[3].to_string(),
            })
        })
        .collect()
}

/// Pure parser for `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY
/// device wifi list`. Lines with an empty SSID column are
/// skipped (the scan emits one row per BSSID — hidden networks
/// produce an empty SSID we don't want to surface).
#[must_use]
pub fn parse_wifi_scan(raw: &str) -> Vec<WifiNetwork> {
    raw.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, ':').collect();
            if parts.len() < 4 {
                return None;
            }
            let ssid = parts[1].trim();
            if ssid.is_empty() {
                return None;
            }
            Some(WifiNetwork {
                in_use: parts[0] == "*",
                ssid: ssid.to_string(),
                signal: parts[2].to_string(),
                security: parts[3].to_string(),
            })
        })
        .collect()
}

/// Shell out to `nmcli`. Empty string on any failure.
pub async fn run_nmcli(args: &[&str]) -> String {
    let Ok(output) = Command::new("nmcli").args(args).output().await else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8(output.stdout).unwrap_or_default()
}

/// Try to re-join a known SSID via `nmcli connection up id
/// <ssid>`. Joining an unknown network needs a password modal
/// that the panel doesn't implement (see CB-1.8 follow-up).
pub async fn connect_to_ssid(ssid: &str) -> Result<String, String> {
    let Ok(output) = Command::new("nmcli")
        .args(["connection", "up", "id", ssid])
        .output()
        .await
    else {
        return Err("nmcli not on PATH".into());
    };
    if output.status.success() {
        Ok(format!("Connected to {ssid}."))
    } else {
        let stderr = String::from_utf8(output.stderr).unwrap_or_default();
        Err(format!(
            "Connecting to {ssid} failed (try the password prompt in a terminal: \
             `nmcli device wifi connect {ssid}`).\n{}",
            stderr.trim(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connections_extracts_four_columns() {
        let raw = "home-wifi:wifi:wlan0:activated\n\
                   wired:ethernet:eno1:activated\n\
                   vpn-work:vpn::deactivated\n";
        let conns = parse_connections(raw);
        assert_eq!(conns.len(), 3);
        assert_eq!(conns[0].name, "home-wifi");
        assert_eq!(conns[0].state, "activated");
        assert_eq!(conns[2].device, "");
    }

    #[test]
    fn parse_connections_skips_malformed_lines() {
        let raw = "ok:wifi:wlan0:activated\nshort:col:count\nalso-ok:vpn:cnx0:disconnected\n";
        let conns = parse_connections(raw);
        assert_eq!(conns.len(), 2);
        assert_eq!(conns[0].name, "ok");
        assert_eq!(conns[1].name, "also-ok");
    }

    #[test]
    fn parse_wifi_scan_extracts_in_use_and_security() {
        let raw = "*:home-wifi:80:WPA2\n :public:30:--\n :neighbor:50:WPA2\n";
        let nets = parse_wifi_scan(raw);
        assert_eq!(nets.len(), 3);
        assert!(nets[0].in_use);
        assert_eq!(nets[0].ssid, "home-wifi");
        assert_eq!(nets[0].security, "WPA2");
        assert!(!nets[1].in_use);
        assert_eq!(nets[2].ssid, "neighbor");
    }

    #[test]
    fn parse_wifi_scan_skips_hidden_ssids() {
        let raw = "*::70:WPA2\n :coffee:50:WPA2\n";
        let nets = parse_wifi_scan(raw);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ssid, "coffee");
    }

    #[test]
    fn parse_wifi_scan_empty_on_empty_input() {
        assert!(parse_wifi_scan("").is_empty());
    }

    #[test]
    fn loaded_records_state_and_clears_busy() {
        let mut panel = WifiPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Loaded {
            nm_available: true,
            connections: parse_connections("home:wifi:wlan0:activated"),
            networks: parse_wifi_scan(" :coffee:50:WPA2"),
        });
        assert!(!panel.busy);
        assert_eq!(panel.connections.len(), 1);
        assert_eq!(panel.networks.len(), 1);
    }

    #[test]
    fn loaded_nm_unavailable_clears_state() {
        let mut panel = WifiPanel::new();
        let _ = panel.update(Message::Loaded {
            nm_available: false,
            connections: Vec::new(),
            networks: Vec::new(),
        });
        assert!(!panel.nm_available);
    }

    #[test]
    fn connect_clicked_while_busy_is_noop() {
        let mut panel = WifiPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::ConnectClicked("home".into()));
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn operation_finished_ok_clears_busy_and_records_status() {
        let mut panel = WifiPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Ok("Connected to home.".into())));
        assert!(!panel.busy);
        assert!(panel.status.contains("Connected"));
    }

    #[test]
    fn operation_finished_err_records_error() {
        let mut panel = WifiPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Err("needs password".into())));
        assert_eq!(panel.status, "needs password");
        assert!(!panel.busy);
    }

    #[test]
    fn refresh_clicked_while_busy_is_noop() {
        let mut panel = WifiPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = WifiPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("nmcli not on PATH".into()));
        assert_eq!(panel.status, "nmcli not on PATH");
        assert!(!panel.busy);
    }
}

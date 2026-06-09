//! Network → VPN panel — NetworkManager VPN connection
//! manager.
//!
//! CB-1.8 partial: replaces the v1.x
//! `mackes/workbench/network/vpn.py`. Lists every VPN
//! connection (filter `connection show` rows to type=vpn /
//! wireguard); each row has a Connect/Disconnect button.
//! VPN-config import is out of scope here — captured as a
//! CB-1.8 follow-up. Users can still `nmcli connection
//! import type wireguard file <path>` directly.

use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Task};
use mde_theme::Palette;
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VpnRow {
    pub name: String,
    pub kind: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct VpnPanel {
    pub nm_available: bool,
    pub vpns: Vec<VpnRow>,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        nm_available: bool,
        vpns: Vec<VpnRow>,
    },
    Error(String),
    ToggleClicked {
        name: String,
        activate: bool,
    },
    OperationFinished(Result<String, String>),
    RefreshClicked,
}

impl VpnPanel {
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
                        vpns: Vec::new(),
                    };
                }
                let raw =
                    run_nmcli(&["-t", "-f", "NAME,TYPE,DEVICE,STATE", "connection", "show"]).await;
                Message::Loaded {
                    nm_available,
                    vpns: parse_vpns(&raw),
                }
            },
            crate::Message::Vpn,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded { nm_available, vpns } => {
                self.nm_available = nm_available;
                self.vpns = vpns;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::ToggleClicked { name, activate } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!(
                    "{} {name}…",
                    if activate {
                        "Connecting to"
                    } else {
                        "Disconnecting"
                    },
                );
                Task::perform(
                    async move { Message::OperationFinished(toggle_vpn(&name, activate).await) },
                    crate::Message::Vpn,
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
                    "MDE talks to VPNs through `nmcli`. Install \
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
            (!self.busy).then(|| crate::Message::Vpn(Message::RefreshClicked)),
            Palette::dark(),
        );

        if self.vpns.is_empty() {
            return column![
                row![refresh_btn, text(&self.status).size(13)].spacing(12),
                text("No VPN connections configured").size(18),
                text(
                    "Import one via `nmcli connection import type \
                     wireguard file <path>` or `nmcli connection \
                     import type openvpn file <path>`, then refresh.",
                )
                .size(13),
            ]
            .spacing(8)
            .width(Length::Fill)
            .into();
        }

        let rows = self.vpns.iter().fold(column![], |col, v| {
            let name = v.name.clone();
            let next_activate = !v.active;
            let btn_label = if v.active { "Disconnect" } else { "Connect" };
            // UX-7.a — per-row toggle routed through the shared
            // button variant. Secondary fits beside the row's
            // status text without dominating.
            let btn = variant_button(
                btn_label,
                ButtonVariant::Secondary,
                (!self.busy).then(|| {
                    crate::Message::Vpn(Message::ToggleClicked {
                        name,
                        activate: next_activate,
                    })
                }),
                Palette::dark(),
            );
            let state = if v.active { "active" } else { "inactive" };
            col.push(
                row![
                    text(&v.name).width(Length::Fixed(220.0)),
                    text(&v.kind).width(Length::Fixed(160.0)),
                    text(state).width(Length::Fixed(80.0)),
                    btn,
                ]
                .spacing(12),
            )
        });

        column![
            row![refresh_btn, text(&self.status).size(13)].spacing(12),
            scrollable(container(rows.spacing(4))).height(Length::Fill),
            text(format!(
                "{} VPN(s) · {} active",
                self.vpns.len(),
                self.vpns.iter().filter(|v| v.active).count(),
            ))
            .size(13),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Pure parser for `nmcli -t -f NAME,TYPE,DEVICE,STATE
/// connection show` — keeps only rows whose TYPE is `vpn` or
/// `wireguard`. STATE columns of `activated` are considered
/// active; any other value (`deactivated`, empty when not
/// running) is inactive.
#[must_use]
pub fn parse_vpns(raw: &str) -> Vec<VpnRow> {
    raw.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, ':').collect();
            if parts.len() < 4 {
                return None;
            }
            let kind = parts[1];
            if kind != "vpn" && kind != "wireguard" {
                return None;
            }
            Some(VpnRow {
                name: parts[0].to_string(),
                kind: kind.to_string(),
                active: parts[3] == "activated",
            })
        })
        .collect()
}

pub async fn run_nmcli(args: &[&str]) -> String {
    let Ok(output) = Command::new("nmcli").args(args).output().await else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8(output.stdout).unwrap_or_default()
}

/// Bring a VPN connection up or down by name. Returns a
/// human-readable status message.
pub async fn toggle_vpn(name: &str, activate: bool) -> Result<String, String> {
    let action = if activate { "up" } else { "down" };
    let Ok(output) = Command::new("nmcli")
        .args(["connection", action, "id", name])
        .output()
        .await
    else {
        return Err("nmcli not on PATH".into());
    };
    if output.status.success() {
        Ok(format!(
            "{} {name}.",
            if activate {
                "Connected to"
            } else {
                "Disconnected"
            },
        ))
    } else {
        let stderr = String::from_utf8(output.stderr).unwrap_or_default();
        Err(format!("nmcli {action} {name} failed: {}", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vpns_filters_to_vpn_and_wireguard_types() {
        let raw = "home:wifi:wlan0:activated\n\
                   work-vpn:vpn:tun0:activated\n\
                   wg-home:wireguard:wg0:deactivated\n\
                   wired:ethernet:eno1:activated\n";
        let vpns = parse_vpns(raw);
        assert_eq!(vpns.len(), 2);
        assert_eq!(vpns[0].name, "work-vpn");
        assert!(vpns[0].active);
        assert_eq!(vpns[1].name, "wg-home");
        assert!(!vpns[1].active);
    }

    #[test]
    fn parse_vpns_empty_when_no_vpn_rows() {
        let raw = "home:wifi:wlan0:activated\nwired:ethernet:eno1:activated\n";
        assert!(parse_vpns(raw).is_empty());
    }

    #[test]
    fn parse_vpns_skips_malformed_lines() {
        let raw = "ok:vpn:tun0:activated\nshort:line\nbad-with-3:cols:only\n";
        let vpns = parse_vpns(raw);
        assert_eq!(vpns.len(), 1);
        assert_eq!(vpns[0].name, "ok");
    }

    #[test]
    fn parse_vpns_empty_on_empty_input() {
        assert!(parse_vpns("").is_empty());
    }

    #[test]
    fn loaded_records_vpns_and_clears_busy() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Loaded {
            nm_available: true,
            vpns: parse_vpns("work-vpn:vpn:tun0:activated"),
        });
        assert!(!panel.busy);
        assert_eq!(panel.vpns.len(), 1);
        assert!(panel.vpns[0].active);
    }

    #[test]
    fn loaded_nm_unavailable_clears_state() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::Loaded {
            nm_available: false,
            vpns: Vec::new(),
        });
        assert!(!panel.nm_available);
    }

    #[test]
    fn toggle_clicked_while_busy_is_noop() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::ToggleClicked {
            name: "work-vpn".into(),
            activate: true,
        });
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn operation_finished_ok_clears_busy_and_records_status() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Ok(
            "Connected to work-vpn.".into()
        )));
        assert!(!panel.busy);
        assert!(panel.status.contains("Connected"));
    }

    #[test]
    fn operation_finished_err_records_error() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Err("auth failed".into())));
        assert_eq!(panel.status, "auth failed");
        assert!(!panel.busy);
    }

    #[test]
    fn refresh_clicked_while_busy_is_noop() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("nmcli not on PATH".into()));
        assert_eq!(panel.status, "nmcli not on PATH");
        assert!(!panel.busy);
    }
}

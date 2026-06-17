//! VOIP-GW-1 — Mesh → SIP Gateway panel.
//!
//! Sets ONE outbound SIP/PSTN gateway for the whole mesh. Apply sends the config
//! to `mackesd` (`action/voip/set-gateway`), which writes it to QNM-Shared
//! (`<workgroup_root>/voip/gateway.toml`, the voice agent's `account.toml` shape);
//! it replicates to every node, whose `mde-voice-hud` agent then registers to the
//! gateway (bare numbers route out via it; intra-mesh peer calls stay P2P,
//! VOIP-P2P-4). Test probes TCP reachability of the registrar. Clear reverts the
//! whole mesh to P2P-only.

use std::time::Duration;

use cosmic::iced::widget::{column, container, row, text, text_input, Space};
use cosmic::iced::{Element, Length, Padding, Task};

use crate::controls::{styled_text_input, variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct SipGatewayPanel {
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub display_name: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Option<GatewayInfo>),
    HostChanged(String),
    PortChanged(String),
    UserChanged(String),
    PassChanged(String),
    DisplayChanged(String),
    Apply,
    Applied(String),
    Test,
    TestResult(String),
    Clear,
}

/// The gateway the daemon currently has stored (for pre-filling the form).
#[derive(Debug, Clone)]
pub struct GatewayInfo {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub display_name: String,
}

impl SipGatewayPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the current mesh gateway from mackesd (`action/voip/get-gateway`).
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let info = tokio::task::spawn_blocking(|| {
                    let raw = crate::dbus::action_request(
                        "action/voip/get-gateway",
                        Duration::from_secs(2),
                    )?;
                    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
                    if v.get("present").and_then(serde_json::Value::as_bool) != Some(true) {
                        return None;
                    }
                    Some(GatewayInfo {
                        host: v["host"].as_str().unwrap_or_default().to_string(),
                        port: u16::try_from(v["port"].as_u64().unwrap_or(5060)).unwrap_or(5060),
                        username: v["username"].as_str().unwrap_or_default().to_string(),
                        password: v["password"].as_str().unwrap_or_default().to_string(),
                        display_name: v["display_name"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .await
                .ok()
                .flatten();
                Message::Loaded(info)
            },
            crate::Message::VoipGateway,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(info) => {
                if let Some(g) = info {
                    self.host = g.host;
                    self.port = if g.port == 0 {
                        String::new()
                    } else {
                        g.port.to_string()
                    };
                    self.username = g.username;
                    self.password = g.password;
                    self.display_name = g.display_name;
                }
                self.busy = false;
                Task::none()
            }
            Message::HostChanged(s) => {
                self.host = s;
                Task::none()
            }
            Message::PortChanged(s) => {
                self.port = s.chars().filter(char::is_ascii_digit).collect();
                Task::none()
            }
            Message::UserChanged(s) => {
                self.username = s;
                Task::none()
            }
            Message::PassChanged(s) => {
                self.password = s;
                Task::none()
            }
            Message::DisplayChanged(s) => {
                self.display_name = s;
                Task::none()
            }
            Message::Apply => {
                if self.host.trim().is_empty() || self.username.trim().is_empty() {
                    self.status = "Enter a registrar host and a username.".to_string();
                    return Task::none();
                }
                self.busy = true;
                self.status = "Applying mesh-wide…".to_string();
                let body = serde_json::json!({
                    "host": self.host.trim(),
                    "port": self.port.parse::<u16>().unwrap_or(5060),
                    "username": self.username.trim(),
                    "password": self.password,
                    "display_name": self.display_name.trim(),
                })
                .to_string();
                Task::perform(
                    async move { Message::Applied(set_gateway(body).await) },
                    crate::Message::VoipGateway,
                )
            }
            Message::Applied(s) | Message::TestResult(s) => {
                self.status = s;
                self.busy = false;
                Task::none()
            }
            Message::Test => {
                if self.host.trim().is_empty() {
                    self.status = "Enter a registrar host to test.".to_string();
                    return Task::none();
                }
                self.busy = true;
                self.status = "Testing reachability…".to_string();
                let host = self.host.trim().to_string();
                let port = self.port.parse::<u16>().unwrap_or(5060);
                Task::perform(
                    async move { Message::TestResult(test_reach(host, port).await) },
                    crate::Message::VoipGateway,
                )
            }
            Message::Clear => {
                self.busy = true;
                self.status = "Clearing (mesh → P2P)…".to_string();
                // Empty host clears the gateway on the daemon side.
                let body = serde_json::json!({ "host": "" }).to_string();
                Task::perform(
                    async move {
                        let r = set_gateway(body).await;
                        Message::Applied(if r.contains("error") {
                            r
                        } else {
                            "Cleared — every node reverts to P2P.".to_string()
                        })
                    },
                    crate::Message::VoipGateway,
                )
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        let pal = crate::live_theme::palette();
        let m = crate::Message::VoipGateway;

        let password = text_input("registrar password", &self.password)
            .secure(true)
            .on_input(move |s| m(Message::PassChanged(s)))
            .padding(Padding {
                top: 0.0,
                right: 10.0,
                bottom: 0.0,
                left: 10.0,
            })
            .size(13);

        let form = column![
            text("Outbound SIP gateway").size(16),
            text(
                "Set once here; every mesh client registers to it. Bare numbers \
                 route out via the gateway — peer-name calls stay P2P."
            )
            .size(12),
            styled_text_input(
                "registrar host (e.g. pbx.example.com)",
                &self.host,
                move |s| m(Message::HostChanged(s)),
                pal,
            ),
            styled_text_input(
                "port (default 5060)",
                &self.port,
                move |s| m(Message::PortChanged(s)),
                pal,
            ),
            styled_text_input(
                "username",
                &self.username,
                move |s| m(Message::UserChanged(s)),
                pal,
            ),
            password,
            styled_text_input(
                "display name (optional)",
                &self.display_name,
                move |s| m(Message::DisplayChanged(s)),
                pal,
            ),
            row![
                variant_button(
                    "Apply mesh-wide",
                    ButtonVariant::Primary,
                    (!self.busy).then(|| m(Message::Apply)),
                    pal
                ),
                variant_button(
                    "Test",
                    ButtonVariant::Ghost,
                    (!self.busy).then(|| m(Message::Test)),
                    pal
                ),
                variant_button(
                    "Clear (mesh → P2P)",
                    ButtonVariant::Ghost,
                    (!self.busy).then(|| m(Message::Clear)),
                    pal
                ),
            ]
            .spacing(8),
        ]
        .spacing(8);

        container(
            column![
                text("SIP Gateway").size(20),
                form,
                text(&self.status).size(13),
            ]
            .spacing(16)
            .padding(20)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// Send the gateway config to mackesd (`action/voip/set-gateway`).
async fn set_gateway(body: String) -> String {
    let reply = tokio::task::spawn_blocking(move || {
        crate::dbus::action_request_with_body(
            "action/voip/set-gateway",
            Some(&body),
            Duration::from_secs(3),
        )
    })
    .await
    .ok()
    .flatten();
    match reply {
        Some(raw) => {
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
            if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                "Applied — every node will register to this gateway.".to_string()
            } else {
                format!(
                    "Couldn't apply: {}",
                    v.get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown error")
                )
            }
        }
        None => "Couldn't reach the mesh daemon (mackesd).".to_string(),
    }
}

/// Probe TCP reachability of the registrar (a quick liveness check — full
/// REGISTER happens on the node's voice agent once applied).
async fn test_reach(host: String, port: u16) -> String {
    tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        let addr = format!("{host}:{port}");
        match addr.to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(sa) => match std::net::TcpStream::connect_timeout(&sa, Duration::from_secs(3))
                {
                    Ok(_) => format!("Reachable: {host}:{port} accepts TCP."),
                    Err(e) => {
                        format!("Unreachable: {e} (note: a UDP-only SIP server may still work).")
                    }
                },
                None => format!("Could not resolve {host}."),
            },
            Err(e) => format!("Could not resolve {host}:{port}: {e}"),
        }
    })
    .await
    .unwrap_or_else(|_| "Test failed to run.".to_string())
}

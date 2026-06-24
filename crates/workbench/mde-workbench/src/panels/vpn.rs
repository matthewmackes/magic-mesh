//! Network ▸ VPN panel — VPN-GW-7.
//!
//! Renders the node's mesh **VPN-GW tunnels** (the `TunnelDef` model on the
//! shared substrate), NOT NetworkManager connections. It consumes the
//! `action/vpn/list-tunnels` Bus verb (responder: `mackesd/src/ipc/vpn_gw.rs`)
//! and shows one card per tunnel — provider, server/protocol, `mvpn-<id>`
//! interface name, and an up/down liveness badge — with **Up / Down / Remove**
//! actions wired back over the bus (`action/vpn/{tunnel-up,tunnel-down,
//! remove-tunnel}`), request-reply on a `spawn_blocking` task exactly like the
//! Routing panel's `action/route/trace` (`panels/routing.rs`).
//!
//! The list reply also carries each tunnel's live up/down so the panel renders
//! liveness without a second round-trip per row: `list-tunnels` returns the
//! durable `TunnelDef`s and the panel pairs each with a `tunnel-status` probe
//! batched into the same blocking task.
//!
//! The pre-VPN-GW NetworkManager/`nmcli` implementation is retired — the mesh
//! egress tunnels are a first-class mesh feature served over the Bus, not a
//! host NM connection. (Old config import via `nmcli` is out of scope here; the
//! add-tunnel wizard is VPN-GW-5's `setup-provider` flow.)

use std::time::Duration;

use cosmic::iced::widget::{column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;

/// Read budget for the `action/vpn/*` Bus probes. Matches the other panels'
/// interactive 2 s read window — the responder answers from local config +
/// `ip link` (no network round-trips), so 2 s is generous.
const VPN_TIMEOUT: Duration = Duration::from_secs(2);

/// One tunnel row, parsed from a `list-tunnels` `TunnelDef` paired with its
/// `tunnel-status` liveness. Mirrors the fields the operator acts on: which
/// provider/server, how it's brought up, the `mvpn-<id>` interface, up/down.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VpnRow {
    /// Operator-chosen tunnel id (the action argument for up/down/remove).
    pub id: String,
    /// Provider label (`mullvad`/`proton`/…/`generic-wg`/`generic-ovpn`).
    pub provider: String,
    /// Server/region selector (may be empty for a generic tunnel).
    pub server: String,
    /// Transport hint (`udp`/`tcp`); empty when the provider doesn't set one.
    pub protocol: String,
    /// How it's brought up (`wg`/`ovpn`/`cli`/`api`) — the wire method label.
    pub method: String,
    /// The dedicated `mvpn-<id>` interface name.
    pub ifname: String,
    /// Live up/down (from the paired `tunnel-status` probe).
    pub up: bool,
}

#[derive(Debug, Clone, Default)]
pub struct VpnPanel {
    /// `false` when the VPN responder didn't answer (`mackesd` down / no Bus).
    pub daemon_up: bool,
    pub tunnels: Vec<VpnRow>,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A `list-tunnels` (+ batched per-row `tunnel-status`) fetch landed.
    Loaded(Result<Vec<VpnRow>, String>),
    RefreshClicked,
    /// Up/Down a tunnel by id.
    ToggleClicked {
        id: String,
        up: bool,
    },
    /// Remove a tunnel by id.
    RemoveClicked {
        id: String,
    },
    /// An up/down/remove action reply landed.
    OperationFinished(Result<String, String>),
}

impl VpnPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch the tunnel list (+ liveness) over the Bus. The Bus client builds
    /// its own current-thread runtime, so the blocking fetch rides
    /// `spawn_blocking`, never the iced executor.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let joined = tokio::task::spawn_blocking(fetch_tunnels).await;
                let result = joined.unwrap_or_else(|e| Err(format!("vpn fetch task: {e}")));
                crate::Message::Vpn(Message::Loaded(result))
            },
            |m| m,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(tunnels)) => {
                self.daemon_up = true;
                self.tunnels = tunnels;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Loaded(Err(msg)) => {
                self.daemon_up = false;
                self.tunnels.clear();
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
            Message::ToggleClicked { id, up } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("{} {id}…", if up { "Bringing up" } else { "Bringing down" });
                let verb = if up { "tunnel-up" } else { "tunnel-down" };
                op_task(verb, id)
            }
            Message::RemoveClicked { id } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Removing {id}…");
                op_task("remove-tunnel", id)
            }
            Message::OperationFinished(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(msg) => msg,
                    Err(msg) => msg,
                };
                // Re-fetch so the row badges + presence reflect the change.
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("VPN")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("mesh egress tunnels")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let refresh_btn = variant_button(
            if self.busy { "…" } else { "Refresh" },
            ButtonVariant::Ghost,
            (!self.busy).then_some(crate::Message::Vpn(Message::RefreshClicked)),
            palette,
        );
        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut body_col = column![].spacing(8);
        if !self.status.is_empty() {
            body_col = body_col.push(status_strip(&self.status, palette));
        }

        if !self.daemon_up {
            body_col = body_col.push(empty_state(
                Icon::StatusError,
                palette.danger,
                "VPN gateway unreachable",
                "Couldn't reach the mesh VPN responder over the Bus. Is `mackesd` \
                 running on this node?",
                palette,
            ));
        } else if self.tunnels.is_empty() {
            body_col = body_col.push(empty_state(
                Icon::Vpn,
                palette.accent,
                "No tunnels configured",
                "This node has no VPN-GW egress tunnels yet. Add one with `mackesctl \
                 vpn setup-provider …`, then refresh.",
                palette,
            ));
        } else {
            for t in &self.tunnels {
                body_col = body_col.push(tunnel_card(t, self.busy, palette));
            }
            body_col = body_col.push(
                text(format!(
                    "{} tunnel(s) · {} up",
                    self.tunnels.len(),
                    self.tunnels.iter().filter(|t| t.up).count(),
                ))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
            );
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(body_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// One tunnel's card: provider · server/protocol header, the `mvpn-<id>`
/// interface + up/down badge, and the Up/Down + Remove actions.
fn tunnel_card<'a>(t: &VpnRow, busy: bool, palette: Palette) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let (badge_icon, badge_color, badge_label) = if t.up {
        (Icon::StatusOk, palette.success, "up")
    } else {
        (Icon::StatusUnknown, palette.text_muted, "down")
    };

    let head = row![
        status_icon(badge_icon, badge_color.into_cosmic_color(), 16.0),
        text(t.provider.clone())
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fill),
        text(badge_label)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(badge_color.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // Server · protocol · method — the muted detail line (each part dropped when
    // the provider didn't set it, so a generic WG tunnel reads cleanly).
    let detail = detail_line(t);
    let detail_row = text(detail)
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());
    let iface_row = text(format!("{} · {}", t.id, t.ifname))
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

    // Up XOR Down (the relevant transition), plus Remove.
    let toggle_label = if t.up { "Down" } else { "Up" };
    let toggle_btn = variant_button(
        toggle_label,
        ButtonVariant::Secondary,
        (!busy).then(|| {
            crate::Message::Vpn(Message::ToggleClicked {
                id: t.id.clone(),
                up: !t.up,
            })
        }),
        palette,
    );
    let remove_btn = variant_button(
        "Remove",
        ButtonVariant::Ghost,
        (!busy).then(|| crate::Message::Vpn(Message::RemoveClicked { id: t.id.clone() })),
        palette,
    );
    let actions = row![
        Space::new().width(Length::Fill),
        toggle_btn,
        Space::new().width(Length::Fixed(8.0)),
        remove_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    card(
        column![head, detail_row, iface_row, actions].spacing(6),
        palette,
    )
}

/// The muted `server · protocol · method` line, dropping empty parts so a
/// generic tunnel (no server/protocol) reads cleanly. Pure — unit-tested.
#[must_use]
pub fn detail_line(t: &VpnRow) -> String {
    [t.server.as_str(), t.protocol.as_str(), t.method.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A short status strip above the cards (last action result / refresh note).
fn status_strip<'a>(msg: &str, palette: Palette) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(
        text(msg.to_string())
            .size(TypeRole::Caption.size_in(FontSize::defaults()))
            .colr(palette.text.into_cosmic_color()),
    )
    .padding(Padding::from([8u16, 14u16]))
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

/// Centered empty/error state with an icon + heading + body.
fn empty_state<'a>(
    icon: Icon,
    icon_color: mde_theme::Rgba,
    heading: &'a str,
    body: &'a str,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    container(
        column![
            status_icon(icon, icon_color.into_cosmic_color(), 32.0),
            Space::new().height(Length::Fixed(8.0)),
            text(heading)
                .size(TypeRole::Subheading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            text(body)
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2)
        .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

/// Render an [`Icon`] as an SVG tinted to `color`, falling back to its glyph.
fn status_icon<'a>(icon: Icon, color: cosmic::iced::Color, px: f32) -> Element<'a, crate::Message> {
    let resolved = mde_icon(icon, IconSize::Inline);
    if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(px))
            .height(Length::Fixed(px))
            .sty(move |_t: &Theme| widget_svg::Style { color: Some(color) })
            .into()
    } else {
        text(resolved.fallback_glyph).size(px).colr(color).into()
    }
}

/// Shared card chrome (raised surface, 1 px border, 5 px corners).
fn card<'a>(
    inner: impl Into<Element<'a, crate::Message>>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(inner)
        .padding(Padding::from([12u16, 16u16]))
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

// ---- I/O ------------------------------------------------------

/// Build the task for an up/down/remove action: publish `action/vpn/<verb>`
/// with the tunnel id as the body, decode the reply, and route it back as an
/// [`Message::OperationFinished`]. Blocking (the Bus client owns a
/// current-thread runtime) → `spawn_blocking`, never the iced executor.
fn op_task(verb: &'static str, id: String) -> Task<crate::Message> {
    Task::perform(
        async move {
            let joined = tokio::task::spawn_blocking(move || request_op(verb, &id)).await;
            let result = joined.unwrap_or_else(|e| Err(format!("vpn op task: {e}")));
            crate::Message::Vpn(Message::OperationFinished(result))
        },
        |m| m,
    )
}

/// Fetch the tunnel list over the Bus and pair each with its liveness. Returns
/// `Err` when the responder doesn't answer (daemon down / no Bus). Blocking.
fn fetch_tunnels() -> Result<Vec<VpnRow>, String> {
    let raw = crate::dbus::action_request("action/vpn/list-tunnels", VPN_TIMEOUT)
        .ok_or_else(|| "mackesd not reachable over the Bus (vpn/list-tunnels)".to_string())?;
    let mut rows = parse_tunnels_reply(&raw)?;
    // Pair each tunnel with its live up/down. A status probe that doesn't answer
    // leaves the row's default `up=false` — the list still renders.
    for row in &mut rows {
        if let Some(reply) = crate::dbus::action_request_with_body(
            "action/vpn/tunnel-status",
            Some(&row.id),
            VPN_TIMEOUT,
        ) {
            row.up = parse_status_up(&reply);
        }
    }
    Ok(rows)
}

/// Request one `action/vpn/<verb>` over the Bus with `id` as the body, decoding
/// the `{ok, detail?}` reply into a human-readable result. Blocking.
fn request_op(verb: &str, id: &str) -> Result<String, String> {
    let topic = format!("action/vpn/{verb}");
    let raw = crate::dbus::action_request_with_body(&topic, Some(id), VPN_TIMEOUT)
        .ok_or_else(|| format!("mackesd not reachable over the Bus (vpn/{verb})"))?;
    parse_op_reply(verb, id, &raw)
}

/// Pure decoder for the `list-tunnels` reply envelope
/// `{"ok":true,"tunnels":[<TunnelDef>...]}` → one [`VpnRow`] per tunnel (live
/// up/down filled in later by the paired status probe). `{"error":m}` → `Err`.
/// Split out so the wire contract is unit-testable without the Bus.
#[must_use = "returns the parsed rows or the responder error"]
pub fn parse_tunnels_reply(raw: &str) -> Result<Vec<VpnRow>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad list-tunnels reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let tunnels = v
        .get("tunnels")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "list-tunnels reply missing 'tunnels'".to_string())?;
    Ok(tunnels.iter().map(row_from_tunnel).collect())
}

/// Build a [`VpnRow`] from one `TunnelDef` JSON object. The `TunnelDef` model
/// (`mackes_mesh_types::vpn`) carries `id`/`provider`/`method`/`server`/
/// `protocol`; the `mvpn-<id>` interface is derived the same way the model does
/// (`mackes_mesh_types::vpn::TunnelDef::ifname`) so the card shows the real
/// device name. `up` defaults false — the status probe fills it.
fn row_from_tunnel(t: &serde_json::Value) -> VpnRow {
    let s = |k: &str| {
        t.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let def = mackes_mesh_types::vpn::TunnelDef {
        id: s("id"),
        ..Default::default()
    };
    VpnRow {
        ifname: def.ifname(),
        id: s("id"),
        provider: s("provider"),
        server: s("server"),
        protocol: s("protocol"),
        // `method` is a kebab-case enum on the wire (`wg`/`ovpn`/`cli`/`api`);
        // default to `wg` when absent, matching `Method::default`.
        method: {
            let m = s("method");
            if m.is_empty() {
                "wg".to_string()
            } else {
                m
            }
        },
        up: false,
    }
}

/// Pure decoder for the `tunnel-status` reply `{"ok":true,"up":bool}` → the
/// `up` bool (false on any error/missing field — a status we can't read is
/// "not known up").
#[must_use]
pub fn parse_status_up(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|v| v.get("up").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Pure decoder for an up/down/remove reply into a human-readable status line.
/// The responder replies `{"ok":bool,"detail":...}` (tunnel-up/down) or
/// `{"ok":true}` (remove), or `{"error":m}`. Split out for unit-testing.
#[must_use]
pub fn parse_op_reply(verb: &str, id: &str, raw: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad {verb} reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let ok = v
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let detail = v.get("detail").and_then(serde_json::Value::as_str);
    match (verb, ok) {
        ("tunnel-up", true) => Ok(format!("{id} up — {}", detail.unwrap_or("brought up"))),
        ("tunnel-down", true) => Ok(format!("{id} down — {}", detail.unwrap_or("brought down"))),
        ("remove-tunnel", true) => Ok(format!("Removed {id}.")),
        // ok:false carries the responder's honest detail (e.g. "config missing").
        (_, false) => Err(detail.map_or_else(
            || format!("{verb} {id} did not run"),
            |d| format!("{verb} {id}: {d}"),
        )),
        (_, true) => Ok(format!("{verb} {id}: ok")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tunnels_reply_maps_tunneldefs_to_rows() {
        // The exact `list-tunnels` envelope the responder emits
        // (`json!({"ok":true,"tunnels":cfg.tunnel})`) over the `TunnelDef`
        // serde shape.
        let raw = r#"{"ok":true,"tunnels":[
            {"id":"mullvad1","provider":"mullvad","method":"wg",
             "server":"us-nyc","protocol":"udp","creds_ref":""},
            {"id":"work","provider":"generic-ovpn","method":"ovpn",
             "server":"","protocol":"tcp","creds_ref":""}
        ]}"#;
        let rows = parse_tunnels_reply(raw).expect("ok envelope decodes");
        assert_eq!(rows.len(), 2);

        let m = &rows[0];
        assert_eq!(m.id, "mullvad1");
        assert_eq!(m.provider, "mullvad");
        assert_eq!(m.method, "wg");
        assert_eq!(m.server, "us-nyc");
        assert_eq!(m.protocol, "udp");
        // The interface name is derived the same way the model does.
        assert_eq!(m.ifname, "mvpn-mullvad1");
        assert!(
            !m.up,
            "liveness defaults down until the status probe fills it"
        );

        let w = &rows[1];
        assert_eq!(w.provider, "generic-ovpn");
        assert_eq!(w.method, "ovpn");
        assert_eq!(w.ifname, "mvpn-work");
    }

    #[test]
    fn parse_tunnels_reply_defaults_absent_method_to_wg() {
        // `method` is `#[serde(default)]` on TunnelDef — an absent field means
        // the WG default, which the row mirrors so the detail line isn't blank.
        let raw = r#"{"ok":true,"tunnels":[{"id":"x","provider":"generic-wg"}]}"#;
        let rows = parse_tunnels_reply(raw).unwrap();
        assert_eq!(rows[0].method, "wg");
        assert_eq!(rows[0].ifname, "mvpn-x");
    }

    #[test]
    fn parse_tunnels_reply_empty_list_is_ok() {
        let rows = parse_tunnels_reply(r#"{"ok":true,"tunnels":[]}"#).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_tunnels_reply_surfaces_error_envelope() {
        let err = parse_tunnels_reply(r#"{"error":"vpn config unreadable"}"#).unwrap_err();
        assert!(err.contains("vpn config unreadable"));
    }

    #[test]
    fn parse_tunnels_reply_rejects_garbage_and_missing_field() {
        assert!(parse_tunnels_reply("not json").is_err());
        assert!(
            parse_tunnels_reply(r#"{"ok":true}"#).is_err(),
            "missing 'tunnels' is an error, not an empty list"
        );
    }

    #[test]
    fn parse_status_up_reads_the_up_bool() {
        assert!(parse_status_up(
            r#"{"ok":true,"ifname":"mvpn-x","up":true}"#
        ));
        assert!(!parse_status_up(
            r#"{"ok":true,"ifname":"mvpn-x","up":false}"#
        ));
        // An unreadable/garbage status is "not known up".
        assert!(!parse_status_up(r#"{"error":"no such tunnel"}"#));
        assert!(!parse_status_up("garbage"));
    }

    #[test]
    fn parse_op_reply_up_down_and_remove_humanise() {
        // tunnel-up ok carries the responder detail.
        let up = parse_op_reply(
            "tunnel-up",
            "mullvad1",
            r#"{"ok":true,"ifname":"mvpn-mullvad1","detail":"wg-quick up"}"#,
        )
        .unwrap();
        assert!(up.contains("mullvad1 up"));
        assert!(up.contains("wg-quick up"));

        let down = parse_op_reply(
            "tunnel-down",
            "mullvad1",
            r#"{"ok":true,"ifname":"mvpn-mullvad1","detail":"wg-quick down"}"#,
        )
        .unwrap();
        assert!(down.contains("mullvad1 down"));

        let removed = parse_op_reply("remove-tunnel", "old", r#"{"ok":true}"#).unwrap();
        assert_eq!(removed, "Removed old.");
    }

    #[test]
    fn parse_op_reply_ok_false_is_an_honest_error_with_detail() {
        // tunnel-up with spawn disabled / config missing replies ok:false +
        // detail — the panel surfaces that as an error, not a success.
        let e = parse_op_reply(
            "tunnel-up",
            "ovpn1",
            r#"{"ok":false,"ifname":"mvpn-ovpn1","detail":"openvpn config missing"}"#,
        )
        .unwrap_err();
        assert!(e.contains("openvpn config missing"), "{e}");
        assert!(e.contains("ovpn1"));
    }

    #[test]
    fn parse_op_reply_error_envelope_and_garbage() {
        let e = parse_op_reply(
            "remove-tunnel",
            "ghost",
            r#"{"error":"no such tunnel 'ghost'"}"#,
        )
        .unwrap_err();
        assert!(e.contains("no such tunnel"));
        assert!(parse_op_reply("tunnel-up", "x", "not json").is_err());
    }

    #[test]
    fn detail_line_drops_empty_parts() {
        let full = VpnRow {
            server: "us-nyc".into(),
            protocol: "udp".into(),
            method: "wg".into(),
            ..Default::default()
        };
        assert_eq!(detail_line(&full), "us-nyc · udp · wg");
        // A generic tunnel with no server/protocol reads as just the method.
        let bare = VpnRow {
            method: "wg".into(),
            ..Default::default()
        };
        assert_eq!(detail_line(&bare), "wg");
    }

    // ── panel state machine ──

    #[test]
    fn loaded_ok_records_tunnels_and_clears_busy() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let rows = parse_tunnels_reply(
            r#"{"ok":true,"tunnels":[{"id":"m1","provider":"mullvad","method":"wg"}]}"#,
        )
        .unwrap();
        let _ = panel.update(Message::Loaded(Ok(rows)));
        assert!(panel.daemon_up);
        assert!(!panel.busy);
        assert_eq!(panel.tunnels.len(), 1);
        assert_eq!(panel.tunnels[0].ifname, "mvpn-m1");
    }

    #[test]
    fn loaded_err_marks_daemon_down_and_clears_tunnels() {
        let mut panel = VpnPanel::new();
        panel.daemon_up = true;
        panel.tunnels = vec![VpnRow::default()];
        let _ = panel.update(Message::Loaded(Err("Bus unreachable".into())));
        assert!(!panel.daemon_up);
        assert!(panel.tunnels.is_empty());
        assert_eq!(panel.status, "Bus unreachable");
    }

    #[test]
    fn toggle_and_remove_while_busy_are_noops() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::ToggleClicked {
            id: "m1".into(),
            up: true,
        });
        assert_eq!(panel.status, "stale");
        let _ = panel.update(Message::RemoveClicked { id: "m1".into() });
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn toggle_sets_busy_and_a_pending_status() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::ToggleClicked {
            id: "m1".into(),
            up: true,
        });
        assert!(panel.busy);
        assert!(panel.status.contains("Bringing up"));
        assert!(panel.status.contains("m1"));
    }

    #[test]
    fn operation_finished_clears_busy_and_records_status() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Ok("m1 up — wg-quick up".into())));
        assert!(!panel.busy);
        assert!(panel.status.contains("m1 up"));

        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Err("auth failed".into())));
        assert_eq!(panel.status, "auth failed");
        assert!(!panel.busy);
    }

    #[test]
    fn refresh_while_busy_is_a_noop() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn view_renders_all_states_without_panic() {
        let mut panel = VpnPanel::new();
        let _ = panel.view(); // default (daemon down → unreachable card)
        panel.daemon_up = true;
        let _ = panel.view(); // up, no tunnels → empty card
        panel.tunnels = parse_tunnels_reply(
            r#"{"ok":true,"tunnels":[
                {"id":"m1","provider":"mullvad","method":"wg","server":"us-nyc","protocol":"udp"}
            ]}"#,
        )
        .unwrap();
        panel.tunnels[0].up = true;
        panel.status = "m1 up — wg-quick up".into();
        let _ = panel.view(); // a live tunnel card + status strip
    }
}

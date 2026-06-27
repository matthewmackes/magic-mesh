//! ROUTER-5 — the Router panel (read view).
//!
//! Surfaces every node's discovered router/firewall appliance, read from the
//! replicated QNM-Shared registry plane `<workgroup>/<host>/router-registry.json`
//! that each node's `router_registry` worker writes (ROUTER-4). Read-only: it
//! unions the per-node files (mirrors `compute::read_shared_inventories`), dedups
//! by appliance id (gateway MAC), and renders one card per appliance with its
//! vendor/version/managed-state. An appliance with no sealed credential shows a
//! "needs credentials" badge (lock #4). Mutating controls (firewall/port-forward/
//! VPN/reboot) land in Phase 2 (ROUTER-6..10).

use std::time::{Duration, SystemTime};

use cosmic::iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::Theme;

/// Theme-bound element alias (mirrors all_services.rs / network_hosts.rs).
type Element<'a, M> = cosmic::iced::Element<'a, M, Theme>;

use crate::cosmic_compat::prelude::*;
use mde_theme::{FontSize, Palette, TypeRole};

/// One discovered router/firewall appliance — the JSON shape written by the
/// `router_registry` worker (`RouterEntry`). Loose-coupled by JSON, like
/// `compute::BusInventory`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct RouterRow {
    /// Gateway MAC — the stable appliance id (the `router/<id>` key).
    pub id: String,
    /// Management IP.
    pub ip: String,
    /// The node this appliance sits behind (`peer:<host>`).
    #[serde(default)]
    pub node_id: String,
    /// Fingerprinted vendor/OS (`edgeos` / `vyos` / `vyatta-unknown` / `unknown`).
    #[serde(default)]
    pub vendor: String,
    /// First line of `show version` when managed + reachable; else empty.
    #[serde(default)]
    pub version: String,
    /// A `router/<id>` credential is sealed for this appliance.
    #[serde(default)]
    pub managed: bool,
    /// Discovered but no credential sealed yet (surfaced read-only).
    #[serde(default)]
    pub needs_creds: bool,
    /// The node's primary (lowest-metric) default-route appliance.
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RouterPanel {
    pub rows: Vec<RouterRow>,
    pub error: Option<String>,
    pub last_run_at: Option<SystemTime>,
    pub busy: bool,
    /// Shared "user:pass" buffer the operator types to seal an unmanaged appliance.
    pub cred_input: String,
    /// Appliance id (MAC) whose seal RPC is in flight.
    pub sealing: Option<String>,
    /// Last seal outcome: (message, is_error).
    pub seal_msg: Option<(String, bool)>,
    /// Appliance ip armed for reboot (awaiting a confirm click).
    pub reboot_armed: Option<String>,
    /// Appliance ip whose reboot RPC is in flight.
    pub rebooting: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<RouterRow>, String>),
    RefreshClicked,
    /// Operator edited the shared "user:pass" seal field.
    CredInput(String),
    /// Seal the shared cred into `router/<mac>` for this appliance.
    SealClicked(String),
    /// Seal RPC returned (Ok message / Err message).
    SealDone(Result<String, String>),
    /// Arm reboot for this appliance ip (shows a confirm).
    RebootClicked(String),
    /// Confirm + fire the reboot RPC for this appliance ip.
    RebootConfirmed(String),
    /// Cancel an armed reboot.
    RebootCancelled,
    /// Reboot RPC returned.
    RebootDone(Result<String, String>),
}

impl RouterPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the per-node router-registry files off the QNM-Shared plane on the
    /// iced executor (spawn_blocking — the read touches the filesystem).
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_routers)
                    .await
                    .unwrap_or_else(|_| Err("router fetch task panicked".into()))
            },
            |result| crate::Message::Router(Message::Loaded(result)),
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(rows)) => {
                self.rows = rows;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.rows = Vec::new();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
            Message::CredInput(v) => {
                self.cred_input = v;
                Task::none()
            }
            Message::SealClicked(mac) => {
                let cred = self.cred_input.trim().to_string();
                if cred.is_empty() {
                    self.seal_msg = Some(("enter user:pass in the seal field first".into(), true));
                    return Task::none();
                }
                self.sealing = Some(mac.clone());
                self.seal_msg = None;
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || seal_router_cred(&mac, &cred))
                            .await
                            .unwrap_or_else(|_| Err("seal task panicked".into()))
                    },
                    |r| crate::Message::Router(Message::SealDone(r)),
                )
            }
            Message::SealDone(Ok(_)) => {
                self.sealing = None;
                self.cred_input.clear(); // don't retain the password in panel memory
                self.seal_msg = Some(("sealed — refreshing".into(), false));
                Self::load()
            }
            Message::SealDone(Err(e)) => {
                self.sealing = None;
                self.seal_msg = Some((format!("seal failed: {e}"), true));
                Task::none()
            }
            Message::RebootClicked(ip) => {
                self.reboot_armed = Some(ip);
                Task::none()
            }
            Message::RebootCancelled => {
                self.reboot_armed = None;
                Task::none()
            }
            Message::RebootConfirmed(ip) => {
                self.reboot_armed = None;
                self.rebooting = Some(ip.clone());
                self.seal_msg = None;
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || reboot_gateway(&ip))
                            .await
                            .unwrap_or_else(|_| Err("reboot task panicked".into()))
                    },
                    |r| crate::Message::Router(Message::RebootDone(r)),
                )
            }
            Message::RebootDone(Ok(_)) => {
                self.rebooting = None;
                self.seal_msg = Some(("reboot issued".into(), false));
                Task::none()
            }
            Message::RebootDone(Err(e)) => {
                self.rebooting = None;
                self.seal_msg = Some((format!("reboot failed: {e}"), true));
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Routers")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_str = if self.last_run_at.is_some() {
            let n = self.rows.len();
            let managed = self.rows.iter().filter(|r| r.managed).count();
            format!(
                "{n} appliance{} · {managed} managed (EdgeRouter / VyOS, Vyatta CLI)",
                if n == 1 { "" } else { "s" },
            )
        } else {
            "click Refresh to read the per-node router registry".into()
        };
        let subtitle = text(subtitle_str)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let accent = palette.accent.into_cosmic_color();
        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                let bg = match status {
                    cosmic::iced::widget::button::Status::Hovered => Color {
                        r: (accent.r * 1.10).min(1.0),
                        g: (accent.g * 1.10).min(1.0),
                        b: (accent.b * 1.10).min(1.0),
                        a: accent.a,
                    },
                    _ => accent,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    icon_color: None,
                    text_color: Color::WHITE,
                    border_radius: 6.0.into(),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                }
            },
        )
        .on_press(crate::Message::Router(Message::RefreshClicked));

        let header = row![title, Space::new().width(Length::Fill), refresh_btn]
            .align_y(cosmic::iced::Alignment::Center);

        let body: Element<'_, crate::Message> = if let Some(ref e) = self.error {
            text(format!("Error: {e}"))
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.danger.into_cosmic_color())
                .into()
        } else if self.rows.is_empty() && self.last_run_at.is_some() {
            text(
                "No router appliances discovered yet — a node publishes its \
                 default-route appliance once mackesd's router_registry has ticked.",
            )
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
            .into()
        } else {
            let sealing = self.sealing.clone();
            let reboot_armed = self.reboot_armed.clone();
            let rebooting = self.rebooting.clone();
            let blocks: Vec<Element<'_, crate::Message>> = self
                .rows
                .iter()
                .map(|r| {
                    let card = row_view(r, palette, sizes);
                    if !r.needs_creds {
                        // managed appliance → card + a confirm-gated Reboot control (ROUTER-10)
                        let accent = palette.accent.into_cosmic_color();
                        let danger = palette.danger.into_cosmic_color();
                        let ip = r.ip.clone();
                        let is_rebooting = rebooting.as_deref() == Some(r.ip.as_str());
                        let is_armed = reboot_armed.as_deref() == Some(r.ip.as_str());
                        let ctrl: Element<'_, crate::Message> = if is_rebooting {
                            text("rebooting…")
                                .size(TypeRole::Caption.size_in(sizes))
                                .colr(palette.text_muted.into_cosmic_color())
                                .into()
                        } else if is_armed {
                            let ipc = ip.clone();
                            row![
                                text(format!("Reboot {ip}?"))
                                    .size(TypeRole::Caption.size_in(sizes))
                                    .colr(danger),
                                button(text("Confirm").size(11).colr(Color::WHITE))
                                    .padding(Padding::from([3u16, 10u16]))
                                    .sty(move |_t: &Theme, _s: cosmic::iced::widget::button::Status| pill_style(danger))
                                    .on_press(crate::Message::Router(Message::RebootConfirmed(ipc))),
                                button(text("Cancel").size(11).colr(palette.text.into_cosmic_color()))
                                    .padding(Padding::from([3u16, 10u16]))
                                    .on_press(crate::Message::Router(Message::RebootCancelled)),
                            ]
                            .spacing(6)
                            .align_y(cosmic::iced::alignment::Vertical::Center)
                            .into()
                        } else {
                            button(text("Reboot").size(11).colr(Color::WHITE))
                                .padding(Padding::from([3u16, 10u16]))
                                .sty(move |_t: &Theme, _s: cosmic::iced::widget::button::Status| pill_style(accent))
                                .on_press(crate::Message::Router(Message::RebootClicked(ip)))
                                .into()
                        };
                        return column![
                            card,
                            row![Space::new().width(Length::Fill), ctrl]
                                .align_y(cosmic::iced::alignment::Vertical::Center)
                        ]
                        .spacing(2)
                        .into();
                    }
                    let busy = sealing.as_deref() == Some(r.id.as_str());
                    let accent = palette.accent.into_cosmic_color();
                    let id = r.id.clone();
                    let seal = button(
                        text(if busy { "Sealing…" } else { "Seal cred" })
                            .size(12)
                            .colr(Color::WHITE),
                    )
                    .padding(Padding::from([4u16, 12u16]))
                    .sty(move |_t: &Theme, _s: cosmic::iced::widget::button::Status| pill_style(accent))
                    .on_press(crate::Message::Router(Message::SealClicked(id)));
                    let hint = text(format!("→ seals router/{} from the field above", r.id))
                        .size(TypeRole::Caption.size_in(sizes))
                        .colr(palette.text_muted.into_cosmic_color());
                    column![
                        card,
                        row![hint, Space::new().width(Length::Fill), seal]
                            .spacing(8)
                            .align_y(cosmic::iced::alignment::Vertical::Center)
                    ]
                    .spacing(2)
                    .into()
                })
                .collect();
            scrollable(column(blocks).spacing(6)).into()
        };

        let seal_field = row![
            text("Seal cred (user:pass):")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
            text_input("e.g. ubnt:password", &self.cred_input)
                .on_input(|v| crate::Message::Router(Message::CredInput(v)))
                .width(Length::FillPortion(2)),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let seal_status: Element<'_, crate::Message> = match &self.seal_msg {
            Some((m, is_err)) => text(m.clone())
                .size(TypeRole::Caption.size_in(sizes))
                .colr((if *is_err { palette.danger } else { palette.success }).into_cosmic_color())
                .into(),
            None => Space::new().into(),
        };
        let page = column![
            header,
            row![subtitle],
            seal_field,
            seal_status,
            Space::new().height(12),
            body
        ]
        .spacing(4);

        let surface_color = palette.surface.into_cosmic_color();
        container(page)
            .padding(24)
            .width(Length::Fill)
            .height(Length::Fill)
            .sty(move |_t: &Theme| container::Style {
                snap: false,
                background: Some(Background::Color(surface_color)),
                ..Default::default()
            })
            .into()
    }
}

/// Appliance status classes (badge). Kept palette-free so it's unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BadgeKind {
    NeedsCreds,
    Unmanaged,
    Unreachable,
    Vendor,
}

/// Status badge label + class for an appliance (no `Palette` — pure).
fn badge(r: &RouterRow) -> (String, BadgeKind) {
    if r.needs_creds {
        ("needs creds".into(), BadgeKind::NeedsCreds)
    } else if !r.managed {
        ("unmanaged".into(), BadgeKind::Unmanaged)
    } else if r.vendor == "unknown" {
        // cred sealed but the device didn't answer the version probe
        ("unreachable".into(), BadgeKind::Unreachable)
    } else {
        (r.vendor.clone(), BadgeKind::Vendor)
    }
}

/// Map a badge class to a Carbon palette color.
fn badge_color(kind: BadgeKind, palette: Palette) -> Color {
    match kind {
        BadgeKind::NeedsCreds => palette.warning.into_cosmic_color(),
        BadgeKind::Unmanaged => palette.text_muted.into_cosmic_color(),
        BadgeKind::Unreachable => palette.danger.into_cosmic_color(),
        BadgeKind::Vendor => palette.success.into_cosmic_color(),
    }
}

/// A solid-fill pill button style in `color` (white text) — shared by the
/// seal/reboot action buttons.
fn pill_style(color: Color) -> cosmic::iced::widget::button::Style {
    cosmic::iced::widget::button::Style {
        snap: false,
        background: Some(Background::Color(color)),
        icon_color: None,
        text_color: Color::WHITE,
        border_radius: 6.0.into(),
        border_width: 0.0,
        border_color: Color::TRANSPARENT,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        shadow: cosmic::iced::Shadow::default(),
    }
}

/// One appliance card: vendor/status badge · ip · node · version/detail.
fn row_view<'a>(r: &'a RouterRow, palette: Palette, sizes: FontSize) -> Element<'a, crate::Message> {
    let (badge_text, badge_kind) = badge(r);
    let badge_color = badge_color(badge_kind, palette);
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();

    let badge = container(
        text(badge_text)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(Color::WHITE),
    )
    .padding(Padding::from([2u16, 8u16]))
    .sty(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(badge_color)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    });

    let node = {
        let h = r.node_id.trim();
        let h = h.strip_prefix("peer:").unwrap_or(h);
        if h.is_empty() {
            "unknown".to_string()
        } else {
            h.to_string()
        }
    };
    let detail = if r.needs_creds {
        format!("{} — seal router/{} to manage", r.id, r.id)
    } else if r.version.is_empty() {
        r.id.clone()
    } else {
        format!("{} · {}", r.version, r.id)
    };

    container(
        row![
            badge,
            text(if r.is_default {
                format!("{} (default)", r.ip)
            } else {
                r.ip.clone()
            })
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(2)),
            text(node)
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(detail)
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(3)),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8u16, 12u16]))
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

// ── I/O ──────────────────────────────────────────────────────────────────────

/// Union every node's `router-registry.json` off the QNM-Shared plane
/// (`<workgroup>/<host>/router-registry.json`), one [`RouterRow`] per file, then
/// dedup by appliance id (gateway MAC) preferring a managed entry — different
/// nodes behind the SAME router publish the same id. Best-effort: a missing
/// share yields an empty list.
fn fetch_routers() -> Result<Vec<RouterRow>, String> {
    let root = mackes_mesh_types::peers::default_workgroup_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    let mut by_id: std::collections::BTreeMap<String, RouterRow> = std::collections::BTreeMap::new();
    for ent in entries.flatten() {
        let path = ent.path().join("router-registry.json");
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rowd) = serde_json::from_str::<RouterRow>(&body) else {
            continue;
        };
        by_id
            .entry(rowd.id.clone())
            .and_modify(|existing| {
                // Prefer the managed view of a shared appliance.
                if rowd.managed && !existing.managed {
                    *existing = rowd.clone();
                }
            })
            .or_insert(rowd);
    }
    Ok(by_id.into_values().collect())
}

/// Seal the operator-typed `user:pass` into `router/<mac>` via the mackesd
/// `action/dc/router-seal-cred` Bus RPC (so it lands in the MESH secret store,
/// not this GUI host's local store). Mirrors `datacenter::vm_power`'s round trip;
/// called inside `spawn_blocking` (it builds its own current-thread runtime).
fn seal_router_cred(mac: &str, cred: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "mac": mac, "cred": cred }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/router-seal-cred",
            mde_bus::hooks::config::Priority::Default,
            Some("router-seal-cred"),
            Some(&body),
            Duration::from_secs(10),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad seal reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok("sealed".to_string())
    } else {
        Err(format!("unexpected seal reply: {v}"))
    }
}

/// ROUTER-10 — reboot a managed appliance via the existing mackesd
/// `action/dc/gateway-reboot` handler (host_ops validates the IPv4 + reboots over
/// the sealed cred). Called inside `spawn_blocking` (builds its own runtime).
fn reboot_gateway(ip: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "host": ip, "confirm": true }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/gateway-reboot",
            mde_bus::hooks::config::Priority::Default,
            Some("gateway-reboot"),
            Some(&body),
            Duration::from_secs(15),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad reboot reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok("reboot issued".to_string())
    } else {
        Err(format!("unexpected reboot reply: {v}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, managed: bool, needs: bool, vendor: &str) -> RouterRow {
        RouterRow {
            id: id.into(),
            ip: "172.20.0.1".into(),
            node_id: "peer:eagle".into(),
            vendor: vendor.into(),
            version: String::new(),
            managed,
            needs_creds: needs,
            is_default: true,
        }
    }

    #[test]
    fn router_entry_json_deserializes() {
        let body = r#"{"id":"46:6a:7c:96:e8:aa","ip":"172.20.0.1","node_id":"peer:eagle",
            "vendor":"edgeos","version":"EdgeOS ER-8","managed":true,
            "needs_creds":false,"is_default":true}"#;
        let r: RouterRow = serde_json::from_str(body).unwrap();
        assert_eq!(r.id, "46:6a:7c:96:e8:aa");
        assert_eq!(r.vendor, "edgeos");
        assert!(r.managed);
    }

    #[test]
    fn badge_reflects_state() {
        assert_eq!(badge(&row("a", false, true, "unknown")), ("needs creds".into(), BadgeKind::NeedsCreds));
        assert_eq!(badge(&row("a", false, false, "unknown")), ("unmanaged".into(), BadgeKind::Unmanaged));
        assert_eq!(badge(&row("a", true, false, "unknown")), ("unreachable".into(), BadgeKind::Unreachable));
        assert_eq!(badge(&row("a", true, false, "edgeos")), ("edgeos".into(), BadgeKind::Vendor));
    }
}

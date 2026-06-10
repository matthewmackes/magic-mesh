//! v4.0.1 WB-2.h — Network → Mesh Control panel.
//!
//! Surfaces the mackesd cluster state the operator needs at
//! "what's wrong with my mesh?" debugging time:
//!   * Am I the cluster leader? (read the
//!     `~/QNM-Shared/.mackesd-leader.lock` file)
//!   * When did the leader last renew its lease?
//!   * What epoch are we on (bumps every force-takeover)?
//!   * `mackesd healthz` JSON output rendered as a status pill
//!     with the raw fields underneath.
//!   * Force-takeover button (shells out to `mackesd
//!     take-leadership --force`).
//!   * Refresh button — re-reads everything.
//!
//! Chrome influence (per iteration skill Phase 0.8): Win11
//! Server Manager landing — single hero card + per-property
//! list + action button row.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

#[derive(Debug, Clone, Default)]
pub struct MeshControlSnapshot {
    /// Decoded lease — `None` when the lock file is missing or
    /// malformed (= no leader elected yet, or QNM-Shared isn't
    /// mounted).
    pub lease: Option<LeaseInfo>,
    /// Whether the local node owns the lease.
    pub self_is_leader: bool,
    /// Local node id (`peer:<hostname>` by convention).
    pub self_node_id: String,
    /// Raw `mackesd healthz` JSON output; empty string when the
    /// CLI binary isn't installed or returned non-zero.
    pub healthz_raw: String,
    /// One-line summary parsed out of `healthz_raw`.
    pub healthz_summary: String,
    /// NF-11.3 — Active Nebula CA epoch this peer's cert was
    /// signed under. `None` when no CA exists yet, or the
    /// daemon's D-Bus surface is unreachable.
    pub nebula_ca_epoch: Option<i64>,
    /// NF-11.3 — Mesh-id from the SelfNode reply. Empty when no
    /// CA exists. Surfaced next to the epoch pill so operators
    /// can confirm which mesh's CA they're about to rotate.
    pub nebula_mesh_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseInfo {
    pub node_id: String,
    pub renewed_at_s: u64,
    pub epoch: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MeshControlPanel {
    pub snapshot: MeshControlSnapshot,
    pub busy: bool,
    pub last_op: String,
    pub last_run_at: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(MeshControlSnapshot),
    RefreshClicked,
    ForceTakeoverClicked,
    /// NF-11.3 — Operator clicked the "Rotate CA" button. Fires
    /// `Nebula.Status.RegenCerts` via dbus-send and refreshes.
    RotateCaClicked,
    OpFinished {
        op: String,
        success: bool,
        output: String,
    },
}

impl MeshControlPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                // probe_cluster() does blocking I/O (healthz, leader
                // lock, + the Nebula Bus probe, which spins its own
                // current-thread runtime). Run it off the async
                // executor via spawn_blocking so block_on never nests
                // a runtime, and to keep the worker responsive.
                tokio::task::spawn_blocking(probe_cluster)
                    .await
                    .expect("probe_cluster task panicked")
            },
            |snap| crate::Message::MeshControl(Message::Loaded(snap)),
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(snap) => {
                self.snapshot = snap;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                self.last_op = "refreshing…".into();
                Self::load()
            }
            Message::ForceTakeoverClicked => {
                self.busy = true;
                self.last_op = "force-take-leadership…".into();
                Task::perform(
                    async {
                        let (ok, output) = run_take_leadership_force().await;
                        ("take-leadership --force".to_string(), ok, output)
                    },
                    |(op, success, output)| {
                        crate::Message::MeshControl(Message::OpFinished {
                            op,
                            success,
                            output,
                        })
                    },
                )
            }
            Message::RotateCaClicked => {
                self.busy = true;
                self.last_op = "rotating CA epoch…".into();
                Task::perform(
                    async {
                        let (ok, output) = run_rotate_ca().await;
                        ("rotate CA".to_string(), ok, output)
                    },
                    |(op, success, output)| {
                        crate::Message::MeshControl(Message::OpFinished {
                            op,
                            success,
                            output,
                        })
                    },
                )
            }
            Message::OpFinished {
                op,
                success,
                output,
            } => {
                self.last_op = if success {
                    format!("{op}: ok")
                } else {
                    let snippet = output.lines().next().unwrap_or("").trim();
                    if snippet.is_empty() {
                        format!("{op}: FAILED")
                    } else {
                        format!("{op}: FAILED — {snippet}")
                    }
                };
                self.busy = false;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Mesh Control")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());

        let subtitle_text = if !self.last_op.is_empty() {
            self.last_op.clone()
        } else if let Some(t) = self.last_run_at {
            format!("last refresh {}", fmt_age(t))
        } else {
            "click Refresh to probe".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let refresh_btn = button(
            text(if self.busy { "Working…" } else { "Refresh" })
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
        .on_press(crate::Message::MeshControl(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let leader_card = leader_card_view(&self.snapshot, palette);
        let healthz_card = healthz_card_view(&self.snapshot, palette);

        let ghost_btn_style = {
            let border = palette.border.into_iced_color();
            let text_main = palette.text.into_iced_color();
            move |_t: &Theme, status: iced::widget::button::Status| {
                let bg = match status {
                    iced::widget::button::Status::Hovered => Color {
                        r: 0.20,
                        g: 0.20,
                        b: 0.22,
                        a: 1.0,
                    },
                    _ => Color::TRANSPARENT,
                };
                iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: text_main,
                    border: Border {
                        color: border,
                        width: 1.0,
                        radius: 5.0.into(),
                    },
                    shadow: iced::Shadow::default(),
                }
            }
        };

        let force_btn = button(
            text("Force takeover")
                .size(12)
                .color(palette.text.into_iced_color()),
        )
        .padding(Padding::from([5u16, 14u16]))
        .style(ghost_btn_style)
        .on_press(crate::Message::MeshControl(Message::ForceTakeoverClicked));

        // NF-11.3 — Rotate CA button. Disabled when there's no
        // active CA epoch (mesh hasn't been minted yet) so the
        // operator isn't offered an action that can't succeed.
        let rotate_label = text(if self.snapshot.nebula_ca_epoch.is_some() {
            "Rotate CA"
        } else {
            "Rotate CA (no mesh)"
        })
        .size(12)
        .color(palette.text.into_iced_color());
        let mut rotate_btn = button(rotate_label)
            .padding(Padding::from([5u16, 14u16]))
            .style(ghost_btn_style);
        if self.snapshot.nebula_ca_epoch.is_some() {
            rotate_btn = rotate_btn.on_press(crate::Message::MeshControl(Message::RotateCaClicked));
        }

        let action_row = row![
            text("Actions:")
                .size(11)
                .color(palette.text_muted.into_iced_color()),
            force_btn,
            rotate_btn,
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center);

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(
                    column![
                        leader_card,
                        Space::new().height(Length::Fixed(12.0)),
                        healthz_card
                    ]
                    .spacing(2),
                )
                .height(Length::FillPortion(1)),
                Space::new().height(Length::Fixed(12.0)),
                action_row,
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn leader_card_view<'a>(
    snap: &'a MeshControlSnapshot,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let (status_icon, status_color, status_label, summary) = match &snap.lease {
        Some(lease) if snap.self_is_leader => (
            Icon::StatusOk,
            palette.success.into_iced_color(),
            "LEADER",
            format!("you ({}) own the cluster lease", lease.node_id),
        ),
        Some(lease) => (
            Icon::Peer,
            palette.accent.into_iced_color(),
            "FOLLOWER",
            format!("{} owns the cluster lease", lease.node_id),
        ),
        None => (
            Icon::StatusWarning,
            palette.warning.into_iced_color(),
            "NO LEADER",
            "no .mackesd-leader.lock found — QNM-Shared not mounted, or no node has taken leadership".into(),
        ),
    };
    let resolved = mde_icon(status_icon, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(28.0))
            .height(Length::Fixed(28.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(status_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(28.0)
            .color(status_color)
            .into()
    };
    let mut details_col = column![row![
        icon_widget,
        column![
            row![
                text("Leader status")
                    .size(13)
                    .color(palette.text.into_iced_color()),
                text(status_label).size(11).color(status_color),
            ]
            .spacing(10)
            .align_y(iced::alignment::Vertical::Center),
            text(summary)
                .size(12)
                .color(palette.text_muted.into_iced_color()),
        ]
        .spacing(4),
    ]
    .spacing(12)
    .align_y(iced::alignment::Vertical::Center),]
    .spacing(8);
    if let Some(lease) = &snap.lease {
        details_col = details_col.push(
            row![
                kv_pill("renewed", fmt_unix_age(lease.renewed_at_s), palette,),
                kv_pill("epoch", lease.epoch.to_string(), palette),
                kv_pill("owner", lease.node_id.clone(), palette,),
                kv_pill("your id", snap.self_node_id.clone(), palette,),
            ]
            .spacing(6),
        );
    }
    // NF-11.3 — CA epoch + mesh-id pills. Surface when we have
    // a real CA reading (the SelfNode D-Bus call returned a
    // value); skip the pills when the mesh isn't minted yet so
    // the row doesn't display "ca_epoch: —" placeholder noise.
    if let Some(epoch) = snap.nebula_ca_epoch {
        let mesh_label = if snap.nebula_mesh_id.is_empty() {
            "—".to_string()
        } else {
            snap.nebula_mesh_id.clone()
        };
        details_col = details_col.push(
            row![
                kv_pill("ca epoch", epoch.to_string(), palette),
                kv_pill("mesh-id", mesh_label, palette),
            ]
            .spacing(6),
        );
    }

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(details_col)
        .padding(Padding::from([14u16, 18u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn healthz_card_view<'a>(
    snap: &'a MeshControlSnapshot,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let header = row![
        text("mackesd healthz")
            .size(13)
            .color(palette.text.into_iced_color()),
        Space::new().width(Length::Fill),
        text(snap.healthz_summary.clone())
            .size(11)
            .color(palette.text_muted.into_iced_color()),
    ]
    .align_y(iced::alignment::Vertical::Center);

    let body_text = if snap.healthz_raw.trim().is_empty() {
        "mackesd healthz not reachable — is the daemon installed?".to_string()
    } else {
        snap.healthz_raw.clone()
    };

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    let raw_box_bg = Color {
        r: 0.06,
        g: 0.06,
        b: 0.07,
        a: 1.0,
    };
    container(
        column![
            header,
            container(
                text(body_text)
                    .size(11)
                    .color(palette.text_muted.into_iced_color()),
            )
            .padding(Padding::from([10u16, 14u16]))
            .width(Length::Fill)
            .style(move |_| container::Style {
                snap: false,
                background: Some(Background::Color(raw_box_bg)),
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            }),
        ]
        .spacing(10),
    )
    .padding(Padding::from([14u16, 18u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn kv_pill<'a>(key: &'a str, value: String, palette: Palette) -> Element<'a, crate::Message> {
    let bg = Color {
        r: 0.10,
        g: 0.10,
        b: 0.12,
        a: 1.0,
    };
    container(
        row![
            text(key)
                .size(10)
                .color(palette.text_muted.into_iced_color()),
            text(value).size(11).color(palette.text.into_iced_color()),
        ]
        .spacing(6),
    )
    .padding(Padding::from([3u16, 8u16]))
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: palette.border.into_iced_color(),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

// ---- I/O ------------------------------------------------------

#[must_use]
pub fn probe_cluster() -> MeshControlSnapshot {
    let self_node_id = read_self_node_id();
    let lease = read_leader_lock();
    let self_is_leader = lease
        .as_ref()
        .map(|l| l.node_id == self_node_id)
        .unwrap_or(false);
    let healthz_raw = run_mackesd_healthz();
    let healthz_summary = summarise_healthz(&healthz_raw);
    let self_node_json = read_nebula_self_node();
    let (nebula_ca_epoch, nebula_mesh_id) = parse_self_node_epoch(&self_node_json);
    MeshControlSnapshot {
        lease,
        self_is_leader,
        self_node_id,
        healthz_raw,
        healthz_summary,
        nebula_ca_epoch,
        nebula_mesh_id,
    }
}

fn read_self_node_id() -> String {
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    format!("peer:{host}")
}

fn leader_lock_paths() -> Vec<PathBuf> {
    let home = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    vec![
        home.join("QNM-Shared/.mackesd-leader.lock"),
        // mackesd_core::default_qnm_shared_root() landing —
        // covers the case where QNM_SHARED_ROOT env-var pointed
        // elsewhere at the daemon's launch.
        PathBuf::from("/var/lib/mackesd/qnm-shared/.mackesd-leader.lock"),
    ]
}

fn read_leader_lock() -> Option<LeaseInfo> {
    for path in leader_lock_paths() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(info) = parse_lease(&raw) {
                return Some(info);
            }
        }
    }
    None
}

/// Pure parser for the lease file format
/// (`node_id\trenewed_at_s\tepoch\n`).
#[must_use]
pub fn parse_lease(raw: &str) -> Option<LeaseInfo> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('\t').collect();
    if parts.len() != 3 {
        return None;
    }
    Some(LeaseInfo {
        node_id: parts[0].to_string(),
        renewed_at_s: parts[1].parse().ok()?,
        epoch: parts[2].parse().ok()?,
    })
}

fn run_mackesd_healthz() -> String {
    let out = std::process::Command::new("mackesd")
        .arg("healthz")
        .output();
    let Ok(out) = out else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[must_use]
pub fn summarise_healthz(raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }
    // The healthz output is a single-line JSON object. Pull the
    // `node_id` + `ok` fields if present; otherwise just report
    // the byte count.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
        let node = json
            .get("node_id")
            .and_then(|v| v.as_str())
            .unwrap_or("<no node_id>");
        let ok = json
            .get("ok")
            .and_then(|v| v.as_bool())
            .map(|b| if b { "ok" } else { "fail" })
            .unwrap_or("unknown");
        return format!("{node} · {ok}");
    }
    format!("{} bytes", raw.len())
}

pub async fn run_take_leadership_force() -> (bool, String) {
    use tokio::process::Command;
    let out = Command::new("mackesd")
        .args(["take-leadership", "--force"])
        .output()
        .await;
    match out {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (false, format!("exec failed: {e}")),
    }
}

/// NF-11.3 / E0.3.1.a — Request the SelfNode snapshot over the mesh
/// Bus (`action/nebula/self-node`), replacing the prior `dbus-send`
/// read of the (dual-served, retiring)
/// `dev.mackes.MDE.Nebula.Status.SelfNode` D-Bus method. Returns
/// the raw JSON reply body (the daemon serializes `SelfNodeSnapshot`
/// as JSON, parsed by [`parse_self_node_epoch`]); empty string when
/// the responder is down / times out. Sync because its only caller,
/// [`probe_cluster`], runs under `spawn_blocking`.
#[must_use]
pub fn read_nebula_self_node() -> String {
    crate::dbus::nebula_request("self-node").unwrap_or_default()
}

/// NF-11.3 — Parse the SelfNode reply for the cert epoch +
/// mesh-id pair. `dbus-send --print-reply=literal` emits the
/// JSON string with a `string "..."` wrapper that we strip
/// before parsing. Returns `(None, "")` on any parse failure
/// (treated as "no CA yet").
#[must_use]
pub fn parse_self_node_epoch(raw: &str) -> (Option<i64>, String) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (None, String::new());
    }
    // Strip the dbus-send `string "..."` wrapper when present.
    let unwrapped = if let Some(rest) = trimmed.strip_prefix("string \"") {
        rest.strip_suffix('"').unwrap_or(rest)
    } else {
        trimmed
    };
    // dbus-send escapes inner quotes as \" — unescape for the
    // JSON parser.
    let payload = unwrapped.replace("\\\"", "\"").replace("\\\\", "\\");
    match serde_json::from_str::<serde_json::Value>(&payload) {
        Ok(v) => {
            let epoch = v.get("cert_epoch").and_then(|e| e.as_i64());
            let mesh_id = v
                .get("mesh_id")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            // cert_epoch == 0 means "no CA exists yet" per the
            // NebulaStatusService::build_self_node fallback.
            // Surface that as None so the panel doesn't paint
            // a "ca epoch: 0" pill against a mesh that hasn't
            // been minted.
            match epoch {
                Some(0) if mesh_id.is_empty() => (None, String::new()),
                Some(n) => (Some(n), mesh_id),
                None => (None, mesh_id),
            }
        }
        Err(_) => (None, String::new()),
    }
}

/// NF-11.3 / E0.3.1.b — Trigger a CA-epoch rotation over the mesh
/// Bus (`action/nebula/regen-certs`), replacing the prior
/// `dbus-send` write to the retired
/// `dev.mackes.MDE.Nebula.Status.RegenCerts` D-Bus method. Returns
/// `(success, human-readable message)` so the panel's `last_op`
/// field can quote the daemon's reply verbatim. The Bus client
/// spins its own current-thread runtime, so it runs under
/// `spawn_blocking`; the 30 s budget covers the `nebula-cert`
/// subprocess work the rotation performs.
pub async fn run_rotate_ca() -> (bool, String) {
    let reply = tokio::task::spawn_blocking(|| {
        crate::dbus::nebula_request_with_timeout("regen-certs", std::time::Duration::from_secs(30))
    })
    .await;
    match reply {
        Ok(Some(body)) => parse_regen_reply(&body),
        Ok(None) => (
            false,
            "mesh responder unavailable (mackesd down or timed out)".to_string(),
        ),
        Err(e) => (false, format!("rotate-ca task failed: {e}")),
    }
}

/// Parse the `action/nebula/regen-certs` reply body
/// (`{ "ok": bool, "message": str }`) into the panel's
/// `(success, message)` shape. A malformed body is surfaced as a
/// failure quoting the raw text.
#[must_use]
pub fn parse_regen_reply(body: &str) -> (bool, String) {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => {
            let ok = v
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let message = v
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            (ok, message)
        }
        Err(_) => (false, format!("unparseable regen reply: {body}")),
    }
}

fn fmt_age(t: SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    fmt_duration(elapsed)
}

fn fmt_unix_age(epoch_s: u64) -> String {
    let now_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if epoch_s > now_s {
        return "in the future".into();
    }
    fmt_duration(Duration::from_secs(now_s - epoch_s))
}

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs} s ago")
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lease_decodes_canonical_shape() {
        let raw = "peer:anvil\t1715000000\t7\n";
        let lease = parse_lease(raw).expect("decoded");
        assert_eq!(lease.node_id, "peer:anvil");
        assert_eq!(lease.renewed_at_s, 1_715_000_000);
        assert_eq!(lease.epoch, 7);
    }

    #[test]
    fn parse_lease_returns_none_for_garbage() {
        assert!(parse_lease("").is_none());
        assert!(parse_lease("just one field").is_none());
        assert!(parse_lease("a\tb\tc\textra").is_none());
        assert!(parse_lease("a\tnot-a-number\t7").is_none());
    }

    #[test]
    fn summarise_healthz_returns_empty_for_empty_input() {
        assert_eq!(summarise_healthz(""), "");
        assert_eq!(summarise_healthz("   "), "");
    }

    #[test]
    fn summarise_healthz_handles_known_json_shape() {
        let raw = r#"{"node_id":"peer:anvil","ok":true,"version":"4.0.0"}"#;
        let s = summarise_healthz(raw);
        assert!(s.contains("peer:anvil"));
        assert!(s.contains("ok"));
    }

    #[test]
    fn summarise_healthz_falls_back_to_byte_count_for_non_json() {
        let raw = "not a json doc";
        let s = summarise_healthz(raw);
        assert!(s.contains("bytes"));
    }

    #[test]
    fn read_self_node_id_starts_with_peer_prefix() {
        assert!(read_self_node_id().starts_with("peer:"));
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = MeshControlPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_snapshot_without_panic() {
        let mut p = MeshControlPanel::new();
        p.snapshot = MeshControlSnapshot {
            lease: Some(LeaseInfo {
                node_id: "peer:anvil".into(),
                renewed_at_s: 1_715_000_000,
                epoch: 1,
            }),
            self_is_leader: true,
            self_node_id: "peer:anvil".into(),
            healthz_raw: r#"{"node_id":"peer:anvil","ok":true}"#.into(),
            healthz_summary: "peer:anvil · ok".into(),
            nebula_ca_epoch: Some(3),
            nebula_mesh_id: "mesh-anvil".into(),
        };
        let _ = p.view();
    }

    // NF-11.3 — parser + view coverage for the CA-epoch indicator.

    #[test]
    fn parse_self_node_epoch_handles_empty_input() {
        assert_eq!(parse_self_node_epoch(""), (None, String::new()));
        assert_eq!(parse_self_node_epoch("   "), (None, String::new()));
    }

    #[test]
    fn parse_self_node_epoch_decodes_dbus_string_wrapper() {
        // dbus-send --print-reply=literal wraps the string reply
        // with `string "…"` and escapes inner quotes.
        let raw = r#"string "{\"node_id\":\"peer:anvil\",\"host\":\"anvil\",\"role\":\"host\",\"cert_epoch\":3,\"cert_expires_at\":9999,\"overlay_ip\":\"10.42.0.1\",\"mesh_id\":\"mesh-anvil\"}""#;
        let (epoch, mesh_id) = parse_self_node_epoch(raw);
        assert_eq!(epoch, Some(3));
        assert_eq!(mesh_id, "mesh-anvil");
    }

    #[test]
    fn parse_self_node_epoch_decodes_bare_json() {
        // When dbus-send isn't used (e.g. test fixtures), the
        // parser also accepts raw JSON.
        let raw = r#"{"cert_epoch":7,"mesh_id":"m1"}"#;
        let (epoch, mesh_id) = parse_self_node_epoch(raw);
        assert_eq!(epoch, Some(7));
        assert_eq!(mesh_id, "m1");
    }

    #[test]
    fn parse_self_node_epoch_treats_zero_with_empty_mesh_as_no_ca() {
        // The NebulaStatusService::build_self_node returns
        // (cert_epoch: 0, mesh_id: "") when no CA exists; the
        // panel reads that as "mesh not minted".
        let raw = r#"{"cert_epoch":0,"mesh_id":""}"#;
        let (epoch, mesh_id) = parse_self_node_epoch(raw);
        assert_eq!(epoch, None);
        assert_eq!(mesh_id, "");
    }

    #[test]
    fn parse_regen_reply_extracts_ok_and_message() {
        // E0.3.1.b — the action/nebula/regen-certs reply shape.
        let (ok, msg) = parse_regen_reply(r#"{"ok":true,"message":"CA rotated to epoch 4"}"#);
        assert!(ok);
        assert!(msg.contains("epoch 4"));
        let (ok2, msg2) = parse_regen_reply(r#"{"ok":false,"message":"rotation: boom"}"#);
        assert!(!ok2);
        assert!(msg2.contains("boom"));
        // A malformed body fails closed, quoting the raw text.
        let (ok3, msg3) = parse_regen_reply("not json");
        assert!(!ok3);
        assert!(msg3.contains("unparseable"));
    }

    #[test]
    fn parse_self_node_epoch_preserves_nonzero_epoch_with_empty_mesh() {
        // A non-zero epoch with empty mesh_id is degenerate but
        // the parser surfaces it rather than silently dropping —
        // the operator should see something is off.
        let raw = r#"{"cert_epoch":2,"mesh_id":""}"#;
        let (epoch, mesh_id) = parse_self_node_epoch(raw);
        assert_eq!(epoch, Some(2));
        assert_eq!(mesh_id, "");
    }

    #[test]
    fn parse_self_node_epoch_returns_none_for_garbage() {
        let (epoch, mesh_id) = parse_self_node_epoch("not json");
        assert_eq!(epoch, None);
        assert_eq!(mesh_id, "");
    }

    #[test]
    fn view_renders_with_no_ca_epoch_without_panic() {
        // Pre-mesh-init state — no CA yet. The Rotate CA button
        // appears disabled (no on_press); the pills row hides.
        let mut p = MeshControlPanel::new();
        p.snapshot = MeshControlSnapshot {
            lease: None,
            self_is_leader: false,
            self_node_id: "peer:anvil".into(),
            healthz_raw: String::new(),
            healthz_summary: String::new(),
            nebula_ca_epoch: None,
            nebula_mesh_id: String::new(),
        };
        let _ = p.view();
    }
}

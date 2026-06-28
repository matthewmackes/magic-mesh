//! v4.0.1 WB-2.f — Maintain → Health Check panel.
//!
//! Runs a curated set of local system probes and surfaces the
//! results as a status table. Each probe is `(name,
//! status, remediation)`. The original worklist spec asked for
//! `mackesd healthz` + JSON parse; since mackesd isn't running
//! as a systemd unit yet (AF-* mega registered its dbus surface
//! but the daemon isn't autostarted), this panel ships with
//! direct local probes so the operator gets useful signal
//! today. A future "mackesd is running" probe will pick up the
//! daemon's own healthz when the user starts it.
//!
//! Chrome influence (per iteration skill Phase 0.8): Win11
//! Settings → System → Recovery checks page (named probes with
//! a status pill + a one-line remediation).
//!
//! Probes implemented:
//!   * disk_space         — `/` usage via statvfs
//!   * memory             — `/proc/meminfo` MemAvailable
//!   * failed_units       — `systemctl --failed --no-pager`
//!   * dns_resolution     — `getent hosts cloudflare.com`
//!   * pending_updates    — `~/.cache/mde/dnf-updates.count`
//!   * snapshot_count     — `~/.local/share/mackes-shell/snapshots/`
//!   * parity_overlay     — `/var/log/mde-parity.log` mtime

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use cosmic::iced::font::Weight;
use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{alignment, Background, Border, Color, Font, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{FontSize, FontWeight, Icon, Palette, TypeRole};

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;
use crate::status_strip::{mono_text, pip};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStatus {
    Ok,
    Warn,
    Fail,
    Unknown,
}

impl ProbeStatus {
    fn icon(self) -> Icon {
        match self {
            Self::Ok => Icon::StatusOk,
            Self::Warn => Icon::StatusWarning,
            Self::Fail => Icon::StatusError,
            Self::Unknown => Icon::StatusUnknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub name: String,
    pub status: ProbeStatus,
    pub detail: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HealthCheckPanel {
    pub probes: Vec<ProbeResult>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    /// W20/W24 — a mesh-service start/stop/restart is in flight.
    pub service_busy: bool,
    /// W20/W24 — last service-control outcome line (empty until one runs).
    pub service_msg: String,
}

/// PLANES-6 / W20 — the mesh services the Health panel can start/stop/
/// restart inline (all System-scope systemd units). A degraded daemon
/// is actionable from the same place it's flagged, without leaving for
/// the standalone Mesh Services panel.
pub const HEALTH_SERVICES: &[&str] = &["mackesd", "nebula"];

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<ProbeResult>),
    RunClicked,
    /// PLANES-6 / W20/W24 — start/stop/restart a mesh service via systemd.
    ServiceOp {
        unit: String,
        op: &'static str,
    },
    /// W20/W24 — the op resolved; on success re-probe (honest reconnect).
    ServiceOpDone {
        unit: String,
        op: &'static str,
        ok: bool,
    },
}

impl HealthCheckPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { run_all_probes() }, |probes| {
            crate::Message::HealthCheck(Message::Loaded(probes))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(probes) => {
                self.probes = probes;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RunClicked => {
                self.busy = true;
                Self::load()
            }
            Message::ServiceOp { unit, op } => {
                if self.service_busy {
                    return Task::none();
                }
                self.service_busy = true;
                self.service_msg = format!("{op} {unit}…");
                let unit_for_done = unit.clone();
                Task::perform(
                    async move {
                        // All HEALTH_SERVICES are System-scope units
                        // (see mesh_services::MESH_UNITS).
                        crate::panels::mesh_services::run_pkexec_systemctl(
                            &crate::panels::mesh_services::UnitScope::System,
                            op,
                            &unit,
                        )
                        .await
                    },
                    move |ok| {
                        crate::Message::HealthCheck(Message::ServiceOpDone {
                            unit: unit_for_done.clone(),
                            op,
                            ok,
                        })
                    },
                )
            }
            Message::ServiceOpDone { unit, op, ok } => {
                self.service_busy = false;
                if ok {
                    // W24 "honest reconnect": re-probe so the service's
                    // status row reflects whether it actually changed.
                    self.service_msg = format!("{op} {unit} issued — re-probing…");
                    self.busy = true;
                    Self::load()
                } else {
                    self.service_msg =
                        format!("{op} {unit} failed (authorization declined or unit error).");
                    Task::none()
                }
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();
        let weights = FontWeight::defaults();

        // ---- header: title · derived verdict · run · systemd hero ----
        let title = text("Monitoring · Health")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if let Some(t) = self.last_run_at {
            format!("last run {}", fmt_age(t))
        } else if self.probes.is_empty() {
            "click Run to start".into()
        } else {
            String::new()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let run_btn = button(
            text(if self.busy {
                "Running…"
            } else {
                "Run checks"
            })
            .size(13)
            .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
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
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            }
        })
        .on_press(crate::Message::HealthCheck(Message::RunClicked));

        // PLANES-2 — Health is the systemd surface (units + the daemon).
        let systemd = crate::panel_chrome::hero_band(
            mde_theme::hero::Hero::Systemd,
            crate::panel_chrome::pkg_version_cached("systemd").as_deref(),
            palette,
        );
        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            verdict_badge(&self.probes, palette, &sizes, &weights),
            run_btn,
            systemd,
        ]
        .spacing(12)
        .align_y(alignment::Vertical::Center);

        // ---- partition the REAL probes: Doctor checks vs Netdata alarms ----
        // The Netdata-alarm rows (`netdata_alarm_probes`) carry a "Netdata"
        // name prefix; everything else is a `meshctl doctor`-style check. This
        // is a presentation split of the existing probe data (§7) — no new
        // data, no fabricated rows.
        let (alarm_probes, doctor_probes): (Vec<&ProbeResult>, Vec<&ProbeResult>) = self
            .probes
            .iter()
            .partition(|p| p.name.starts_with("Netdata"));

        // Doctor card body (or the preserved empty / loading state).
        let doctor_body: Element<'_, crate::Message> = if doctor_probes.is_empty() {
            let msg = if self.busy {
                "Running checks…"
            } else {
                "No checks have been run yet. Click \"Run checks\" above to refresh."
            };
            container(
                text(msg)
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            )
            .padding(Padding::from([14u16, 15u16]))
            .into()
        } else {
            let n = doctor_probes.len();
            let mut col = column![].width(Length::Fill);
            for (i, p) in doctor_probes.into_iter().enumerate() {
                col = col.push(doctor_row(p, palette, &sizes));
                if i + 1 < n {
                    col = col.push(hairline(palette));
                }
            }
            col.into()
        };
        let doctor_card = dense_card(
            "Doctor",
            doctor_body,
            Length::FillPortion(14),
            palette,
            &sizes,
        );

        // Netdata Alarms card body.
        let alarms_body: Element<'_, crate::Message> = if alarm_probes.is_empty() {
            container(
                text("no alarm data")
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            )
            .padding(Padding::from([14u16, 13u16]))
            .into()
        } else {
            let n = alarm_probes.len();
            let mut col = column![].width(Length::Fill);
            for (i, p) in alarm_probes.into_iter().enumerate() {
                col = col.push(alarm_row(p, palette, &sizes, &weights));
                if i + 1 < n {
                    col = col.push(hairline(palette));
                }
            }
            col.into()
        };
        let alarms_card = dense_card(
            "Netdata Alarms",
            alarms_body,
            Length::FillPortion(10),
            palette,
            &sizes,
        );

        let grid = row![doctor_card, alarms_card]
            .spacing(11)
            .align_y(alignment::Vertical::Top);

        // PLANES-6 / W20 — folded mesh-service start/stop/restart controls
        // (preserved real functionality). One row per HEALTH_SERVICES unit;
        // buttons disabled while any op (or a probe pass) is in flight, so a
        // degraded daemon flagged above is actionable right here.
        let controls_live = !self.service_busy && !self.busy;
        let mut services_inner = column![].spacing(8).width(Length::Fill);
        for unit in HEALTH_SERVICES {
            let mut btns = row![
                mono_text((*unit).to_string(), TypeRole::Body, &sizes, &weights)
                    .width(Length::Fixed(120.0))
                    .colr(palette.text.into_cosmic_color())
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center);
            for op in ["start", "stop", "restart"] {
                btns = btns.push(variant_button(
                    op,
                    ButtonVariant::Secondary,
                    controls_live.then_some(crate::Message::HealthCheck(Message::ServiceOp {
                        unit: (*unit).to_string(),
                        op,
                    })),
                    palette,
                ));
            }
            services_inner = services_inner.push(btns);
        }
        if !self.service_msg.is_empty() {
            services_inner = services_inner.push(
                text(self.service_msg.clone())
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        }
        let services_card = dense_card(
            "Mesh Services",
            container(services_inner)
                .padding(Padding::from([10u16, 13u16]))
                .width(Length::Fill)
                .into(),
            Length::Fill,
            palette,
            &sizes,
        );

        let content = column![grid, services_card].spacing(16).width(Length::Fill);

        crate::panel_chrome::panel_container(
            column![
                header,
                Space::new().height(Length::Fixed(16.0)),
                scrollable(content).height(Length::Fill),
            ]
            .width(Length::Fill)
            .into(),
            crate::live_theme::tokens().density,
        )
    }
}

/// The semantic palette colour for a probe status (§4 token map).
fn status_color(s: ProbeStatus, palette: Palette) -> Color {
    status_rgba(s, palette).into_cosmic_color()
}

/// The probe status as a palette [`mde_theme::Rgba`] (for the status pip).
fn status_rgba(s: ProbeStatus, palette: Palette) -> mde_theme::Rgba {
    match s {
        ProbeStatus::Ok => palette.success,
        ProbeStatus::Warn => palette.warning,
        ProbeStatus::Fail => palette.danger,
        ProbeStatus::Unknown => palette.text_muted,
    }
}

/// A 1 px full-width hairline divider in the border token.
fn hairline<'a>(palette: Palette) -> Element<'a, crate::Message> {
    let color = palette.border.into_cosmic_color();
    container(Space::new().height(Length::Fixed(1.0)).width(Length::Fill))
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: None,
        })
        .into()
}

/// A dense Carbon card: a surface-tinted box with a sharp 1 px hairline
/// border, an uppercase section-label header bar, an internal divider, and
/// the caller's body. Matches the design's `#262626`/`#393939` card chrome.
fn dense_card<'a>(
    title: &str,
    body: Element<'a, crate::Message>,
    width: Length,
    palette: Palette,
    sizes: &FontSize,
) -> Element<'a, crate::Message> {
    let header = container(
        text(title.to_uppercase())
            .size(TypeRole::Caption.size_in(*sizes))
            .font(Font {
                weight: Weight::Medium,
                ..Font::DEFAULT
            })
            .colr(palette.text_muted.into_cosmic_color()),
    )
    .padding(Padding::from([8u16, 13u16]))
    .width(Length::Fill);

    let inner = column![header, hairline(palette), body].width(Length::Fill);

    container(inner)
        .width(width)
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: Some(palette.text.into_cosmic_color()),
        })
        .into()
}

/// The design's derived verdict pill: a roll-up of the live probe statuses
/// (worst severity wins), with a count summary sub-line. Honest — computed
/// from the real probes, not a static "READY".
fn verdict_badge<'a>(
    probes: &[ProbeResult],
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let (mut ok, mut warn, mut fail) = (0u32, 0u32, 0u32);
    for p in probes {
        match p.status {
            ProbeStatus::Ok => ok += 1,
            ProbeStatus::Warn => warn += 1,
            ProbeStatus::Fail => fail += 1,
            ProbeStatus::Unknown => {}
        }
    }
    let (word, color) = if probes.is_empty() {
        ("—", palette.text_muted)
    } else if fail > 0 {
        ("ATTENTION", palette.danger)
    } else if warn > 0 {
        ("DEGRADED", palette.warning)
    } else {
        ("READY", palette.success)
    };
    let sub = if probes.is_empty() {
        "not yet run".to_string()
    } else {
        format!("{ok} ok · {warn} warn · {fail} fail")
    };
    let badge_color = color.into_cosmic_color();
    container(
        row![
            mono_text(word, TypeRole::Body, sizes, weights).colr(badge_color),
            text(sub)
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(9)
        .align_y(alignment::Vertical::Center),
    )
    .padding(Padding::from([7u16, 14u16]))
    .style(move |_| container::Style {
        snap: false,
        icon_color: None,
        background: Some(Background::Color(palette.surface.into_cosmic_color())),
        border: Border {
            color: badge_color,
            width: 1.0,
            radius: 0.0.into(),
        },
        shadow: Default::default(),
        text_color: Some(badge_color),
    })
    .into()
}

/// One Doctor check row: status pip · title/detail (+ remediation) · status word.
fn doctor_row<'a>(
    p: &'a ProbeResult,
    palette: Palette,
    sizes: &FontSize,
) -> Element<'a, crate::Message> {
    let col = status_color(p.status, palette);
    let mut detail_col = column![
        text(p.name.clone())
            .size(TypeRole::Body.size_in(*sizes))
            .colr(palette.text.into_cosmic_color()),
        text(p.detail.clone())
            .size(TypeRole::Caption.size_in(*sizes))
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(2);
    if let Some(rem) = &p.remediation {
        detail_col = detail_col.push(
            text(format!("→ {rem}"))
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );
    }
    container(
        row![
            pip(status_rgba(p.status, palette)),
            detail_col.width(Length::Fill),
            text(p.status.label())
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(col),
        ]
        .spacing(11)
        .align_y(alignment::Vertical::Center),
    )
    .padding(Padding::from([10u16, 15u16]))
    .width(Length::Fill)
    .into()
}

/// One Netdata alarm row: severity word · node (mono) / message.
fn alarm_row<'a>(
    p: &'a ProbeResult,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let col = status_color(p.status, palette);
    let node = p.name.strip_prefix("Netdata: ").unwrap_or(p.name.as_str());
    container(
        row![
            text(p.status.label())
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(col)
                .width(Length::Fixed(62.0)),
            column![
                mono_text(node.to_string(), TypeRole::Caption, sizes, weights)
                    .colr(palette.text.into_cosmic_color()),
                text(p.detail.clone())
                    .size(TypeRole::Caption.size_in(*sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(2)
            .width(Length::Fill),
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Top),
    )
    .padding(Padding::from([8u16, 13u16]))
    .width(Length::Fill)
    .into()
}

// ---- probes ---------------------------------------------------

#[must_use]
pub fn run_all_probes() -> Vec<ProbeResult> {
    let mut probes = vec![
        probe_disk_space(),
        probe_memory(),
        probe_failed_units(),
        probe_dns_resolution(),
        probe_pending_updates(),
        probe_snapshot_count(),
        probe_parity_overlay(),
        // PLANES-6 / ENT-7 — the `meshctl doctor` checks, folded into the
        // Health panel: the mesh prerequisites, the daemon, and the
        // overlay link. Same signal as the CLI doctor, surfaced live with
        // the panel's re-run.
        probe_mesh_binaries(),
        probe_mackesd_service(),
        probe_overlay_link(),
    ];
    // PLANES-6 / W20 — the local Netdata active-alarm list, one row per
    // firing alarm (or a single honest "none active" / "not reachable").
    probes.extend(netdata_alarm_probes());
    probes
}

/// PLANES-6 / W20 — parse Netdata's `/api/v1/alarms` reply into a list
/// of `(name, status)` for the alarms actually firing (WARNING or
/// CRITICAL); CLEAR/UNDEFINED and anything else are dropped. Sorted by
/// name for a stable render. Pure + testable (mirrors the mackesd-side
/// `descriptors::parse_netdata_alarms`, which keeps only the 3-tier
/// summary; here we keep the names for the operator-facing list).
#[must_use]
pub fn parse_netdata_alarms(body: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some(alarms) = v.get("alarms").and_then(|a| a.as_object()) {
        for (name, alarm) in alarms {
            if let Some(s @ ("WARNING" | "CRITICAL")) = alarm.get("status").and_then(|s| s.as_str())
            {
                out.push((name.clone(), s.to_string()));
            }
        }
    }
    out.sort();
    out
}

/// W20 — fetch the local Netdata active alarms over a std-only HTTP/1.0
/// GET (same approach as the Peers panel's metrics fetch — no client
/// dep). `None` when Netdata is unreachable, so the probe row can say so
/// honestly rather than imply "no alarms".
fn fetch_netdata_alarms() -> Option<Vec<(String, String)>> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:19999".parse().ok()?,
        Duration::from_millis(900),
    )
    .ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(1500)))
        .ok();
    write!(
        stream,
        "GET /api/v1/alarms?active HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    Some(parse_netdata_alarms(body))
}

/// W20 — turn the fetched alarms into probe rows.
fn netdata_alarm_probes() -> Vec<ProbeResult> {
    match fetch_netdata_alarms() {
        None => vec![ProbeResult {
            name: "Netdata alarms".into(),
            status: ProbeStatus::Unknown,
            detail: "Netdata not reachable on localhost:19999".into(),
            remediation: Some("sudo systemctl enable --now netdata".into()),
        }],
        Some(active) if active.is_empty() => vec![ProbeResult {
            name: "Netdata alarms".into(),
            status: ProbeStatus::Ok,
            detail: "no active alarms".into(),
            remediation: None,
        }],
        Some(active) => active
            .into_iter()
            .map(|(name, status)| ProbeResult {
                status: if status == "CRITICAL" {
                    ProbeStatus::Fail
                } else {
                    ProbeStatus::Warn
                },
                name: format!("Netdata: {name}"),
                detail: status,
                remediation: None,
            })
            .collect(),
    }
}

/// PLANES-6 / ENT-7 — required mesh binaries on PATH (nebula + nebula-cert).
fn probe_mesh_binaries() -> ProbeResult {
    let on_path = |bin: &str| {
        std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|p| p.join(bin).is_file()))
            .unwrap_or(false)
    };
    let missing: Vec<&str> = ["nebula", "nebula-cert"]
        .into_iter()
        .filter(|b| !on_path(b))
        .collect();
    if missing.is_empty() {
        ProbeResult {
            name: "Mesh binaries".into(),
            status: ProbeStatus::Ok,
            detail: "nebula + nebula-cert on PATH".into(),
            remediation: None,
        }
    } else {
        ProbeResult {
            name: "Mesh binaries".into(),
            status: ProbeStatus::Fail,
            detail: format!("missing: {}", missing.join(", ")),
            remediation: Some("install the nebula package (sudo dnf install nebula)".into()),
        }
    }
}

/// PLANES-6 / ENT-7 — the `mackesd` daemon is active (W24 self-restart
/// surface: a degraded daemon shows here with the restart remediation).
fn probe_mackesd_service() -> ProbeResult {
    let state = std::process::Command::new("systemctl")
        .args(["is-active", "mackesd"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    match state.as_str() {
        "active" => ProbeResult {
            name: "Mesh daemon (mackesd)".into(),
            status: ProbeStatus::Ok,
            detail: "active".into(),
            remediation: None,
        },
        "" => ProbeResult {
            name: "Mesh daemon (mackesd)".into(),
            status: ProbeStatus::Unknown,
            detail: "systemctl unavailable".into(),
            remediation: None,
        },
        other => ProbeResult {
            name: "Mesh daemon (mackesd)".into(),
            status: ProbeStatus::Fail,
            detail: other.to_string(),
            remediation: Some("sudo systemctl restart mackesd".into()),
        },
    }
}

/// PLANES-6 / ENT-7 — the Nebula overlay link is up with an address.
fn probe_overlay_link() -> ProbeResult {
    let ip = std::process::Command::new("ip")
        .args(["-4", "addr", "show", "nebula1"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout).lines().find_map(|l| {
                l.trim()
                    .strip_prefix("inet ")
                    .and_then(|rest| rest.split('/').next())
                    .map(str::to_string)
            })
        });
    match ip {
        Some(ip) => ProbeResult {
            name: "Overlay link (nebula1)".into(),
            status: ProbeStatus::Ok,
            detail: format!("up, {ip}"),
            remediation: None,
        },
        None => ProbeResult {
            name: "Overlay link (nebula1)".into(),
            status: ProbeStatus::Warn,
            detail: "no overlay IP on nebula1".into(),
            remediation: Some("enroll the node and confirm nebula is running".into()),
        },
    }
}

fn probe_disk_space() -> ProbeResult {
    // statvfs on "/" via libc; do shell-out instead since the
    // crate forbids unsafe and there's no portable statvfs in std.
    let raw = std::process::Command::new("df")
        .args(["-B1", "--output=avail,size,target", "/"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        });
    let Some(raw) = raw else {
        return ProbeResult {
            name: "Disk space (root)".into(),
            status: ProbeStatus::Unknown,
            detail: "df / failed".into(),
            remediation: Some("install coreutils".into()),
        };
    };
    let line = raw.lines().nth(1).unwrap_or("");
    let mut parts = line.split_whitespace();
    let avail: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let size: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let pct_free = (avail as f64 / size as f64 * 100.0).round() as u32;
    let detail = format!(
        "{} free of {} ({pct_free}%)",
        fmt_bytes(avail),
        fmt_bytes(size)
    );
    let (status, remediation) = if pct_free < 5 {
        (
            ProbeStatus::Fail,
            Some("free space critical — clean ~/Downloads or run `mackes snapshots prune`".into()),
        )
    } else if pct_free < 15 {
        (
            ProbeStatus::Warn,
            Some("free space tight — consider pruning old snapshots".into()),
        )
    } else {
        (ProbeStatus::Ok, None)
    };
    ProbeResult {
        name: "Disk space (root)".into(),
        status,
        detail,
        remediation,
    }
}

fn probe_memory() -> ProbeResult {
    let raw = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total_kb: u64 = 0;
    let mut avail_kb: u64 = 0;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        }
    }
    if total_kb == 0 {
        return ProbeResult {
            name: "Memory".into(),
            status: ProbeStatus::Unknown,
            detail: "/proc/meminfo unreadable".into(),
            remediation: None,
        };
    }
    let pct_free = (avail_kb as f64 / total_kb as f64 * 100.0).round() as u32;
    let detail = format!(
        "{} available of {} ({pct_free}%)",
        fmt_bytes(avail_kb * 1024),
        fmt_bytes(total_kb * 1024)
    );
    let (status, remediation) = if pct_free < 10 {
        (
            ProbeStatus::Warn,
            Some("RAM pressure — close memory-heavy apps".into()),
        )
    } else {
        (ProbeStatus::Ok, None)
    };
    ProbeResult {
        name: "Memory".into(),
        status,
        detail,
        remediation,
    }
}

fn probe_failed_units() -> ProbeResult {
    let raw = std::process::Command::new("systemctl")
        .args(["--failed", "--no-pager", "--no-legend", "--plain"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    let Some(raw) = raw else {
        return ProbeResult {
            name: "Failed systemd units".into(),
            status: ProbeStatus::Unknown,
            detail: "systemctl unavailable".into(),
            remediation: None,
        };
    };
    let n = raw.lines().filter(|l| !l.trim().is_empty()).count();
    let detail = if n == 0 {
        "no failed units".into()
    } else {
        let names: Vec<&str> = raw
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .take(3)
            .collect();
        format!("{n} failed — {} …", names.join(", "))
    };
    let (status, remediation) = if n == 0 {
        (ProbeStatus::Ok, None)
    } else {
        (
            ProbeStatus::Warn,
            Some("inspect with `systemctl --failed` + `journalctl -u <name>`".into()),
        )
    };
    ProbeResult {
        name: "Failed systemd units".into(),
        status,
        detail,
        remediation,
    }
}

fn probe_dns_resolution() -> ProbeResult {
    let ok = std::process::Command::new("getent")
        .args(["hosts", "cloudflare.com"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        ProbeResult {
            name: "DNS resolution".into(),
            status: ProbeStatus::Ok,
            detail: "cloudflare.com resolves".into(),
            remediation: None,
        }
    } else {
        ProbeResult {
            name: "DNS resolution".into(),
            status: ProbeStatus::Fail,
            detail: "cloudflare.com did not resolve".into(),
            remediation: Some("check /etc/resolv.conf + NetworkManager state".into()),
        }
    }
}

fn probe_pending_updates() -> ProbeResult {
    let cache = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache"))
        })
        .unwrap_or_default();
    let count: u32 = std::fs::read_to_string(cache.join("mde/dnf-updates.count"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let (status, remediation) = match count {
        0 => (ProbeStatus::Ok, None),
        1..=20 => (
            ProbeStatus::Ok,
            Some("run dnf upgrade at next reboot".into()),
        ),
        21..=100 => (ProbeStatus::Warn, Some("dnf upgrade overdue".into())),
        _ => (
            ProbeStatus::Warn,
            Some("large backlog — consider dnf upgrade now".into()),
        ),
    };
    ProbeResult {
        name: "Pending updates".into(),
        status,
        detail: format!(
            "{count} package{} queued",
            if count == 1 { "" } else { "s" }
        ),
        remediation,
    }
}

fn probe_snapshot_count() -> ProbeResult {
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            return ProbeResult {
                name: "Snapshots".into(),
                status: ProbeStatus::Unknown,
                detail: "$HOME unset".into(),
                remediation: None,
            };
        }
    };
    let dir = home.join(".local/share/mackes-shell/snapshots");
    let count = std::fs::read_dir(&dir)
        .ok()
        .map(|it| {
            it.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .count()
        })
        .unwrap_or(0);
    ProbeResult {
        name: "Snapshots".into(),
        status: ProbeStatus::Ok,
        detail: format!(
            "{count} snapshot{} on disk",
            if count == 1 { "" } else { "s" }
        ),
        remediation: None,
    }
}

fn probe_parity_overlay() -> ProbeResult {
    let log = PathBuf::from("/var/log/mde-parity.log");
    let Ok(meta) = std::fs::metadata(&log) else {
        return ProbeResult {
            name: "Parity overlay".into(),
            status: ProbeStatus::Unknown,
            detail: "log not found — overlay not installed".into(),
            remediation: Some(
                "install-helpers/install-parity-infra.sh + systemctl --user start mde-parity.path"
                    .into(),
            ),
        };
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let age = mtime.elapsed().unwrap_or(Duration::from_secs(u64::MAX));
    let detail = format!("last activity {} ago", fmt_duration(age));
    if age > Duration::from_secs(7 * 86_400) {
        ProbeResult {
            name: "Parity overlay".into(),
            status: ProbeStatus::Warn,
            detail,
            remediation: Some("overlay hasn't fired in a week — check .path watcher".into()),
        }
    } else {
        ProbeResult {
            name: "Parity overlay".into(),
            status: ProbeStatus::Ok,
            detail,
            remediation: None,
        }
    }
}

fn fmt_age(t: SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    fmt_duration(elapsed)
}

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        format!("{} h", secs / 3600)
    } else {
        format!("{} d", secs / 86_400)
    }
}

fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_probes_returns_all_results() {
        // 7 local probes + 3 PLANES-6/ENT-7 doctor probes + the W20
        // Netdata alarm rows (≥1: a list, or one honest summary row). The
        // alarm count varies with the environment, so assert the floor.
        let probes = run_all_probes();
        assert!(probes.len() >= 11, "got {}", probes.len());
    }

    #[test]
    fn netdata_alarm_parse_keeps_only_firing_alarms() {
        // W20 — WARNING/CRITICAL kept (sorted), CLEAR/other dropped.
        let body = r#"{"alarms":{
            "oom":{"status":"CRITICAL"},
            "disk_fill":{"status":"WARNING"},
            "cpu":{"status":"CLEAR"},
            "ram":{"status":"UNDEFINED"}
        }}"#;
        let got = parse_netdata_alarms(body);
        assert_eq!(
            got,
            vec![
                ("disk_fill".to_string(), "WARNING".to_string()),
                ("oom".to_string(), "CRITICAL".to_string()),
            ]
        );
    }

    #[test]
    fn netdata_alarm_parse_handles_empty_and_garbage() {
        assert!(parse_netdata_alarms(r#"{"alarms":{}}"#).is_empty());
        assert!(parse_netdata_alarms("not json").is_empty());
        assert!(parse_netdata_alarms("{}").is_empty());
    }

    #[test]
    fn doctor_probes_are_present_and_named() {
        let probes = run_all_probes();
        let names: Vec<&str> = probes.iter().map(|p| p.name.as_str()).collect();
        for n in [
            "Mesh binaries",
            "Mesh daemon (mackesd)",
            "Overlay link (nebula1)",
        ] {
            assert!(names.contains(&n), "missing ENT-7 doctor probe: {n}");
        }
    }

    #[test]
    fn probe_names_are_unique() {
        let probes = run_all_probes();
        let names: std::collections::HashSet<_> = probes.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names.len(), probes.len(), "duplicate probe name");
    }

    #[test]
    fn service_op_marks_busy_with_a_message() {
        // W20/W24 — issuing a service op flips busy + shows progress.
        let mut panel = HealthCheckPanel::new();
        let _ = panel.update(Message::ServiceOp {
            unit: "mackesd".into(),
            op: "restart",
        });
        assert!(panel.service_busy);
        assert!(panel.service_msg.contains("restart mackesd"));
    }

    #[test]
    fn service_op_while_busy_is_noop() {
        let mut panel = HealthCheckPanel::new();
        panel.service_busy = true;
        panel.service_msg = "restart mackesd…".into();
        let _ = panel.update(Message::ServiceOp {
            unit: "nebula".into(),
            op: "stop",
        });
        // Still the original op's message — the second was ignored.
        assert!(panel.service_msg.contains("mackesd"));
    }

    #[test]
    fn service_op_failure_reports_and_clears_busy() {
        let mut panel = HealthCheckPanel::new();
        panel.service_busy = true;
        let _ = panel.update(Message::ServiceOpDone {
            unit: "mackesd".into(),
            op: "restart",
            ok: false,
        });
        assert!(!panel.service_busy);
        assert!(panel.service_msg.to_lowercase().contains("failed"));
    }

    #[test]
    fn service_op_success_reprobes_with_honest_reconnect() {
        // W24 — success clears busy, sets the probe pass busy, re-probes.
        let mut panel = HealthCheckPanel::new();
        panel.service_busy = true;
        let _ = panel.update(Message::ServiceOpDone {
            unit: "nebula".into(),
            op: "start",
            ok: true,
        });
        assert!(!panel.service_busy);
        assert!(panel.busy, "re-probe should be in flight");
        assert!(panel.service_msg.contains("re-probing"));
    }

    #[test]
    fn health_services_are_system_units() {
        // The folded controls target the known mesh units.
        assert!(HEALTH_SERVICES.contains(&"mackesd"));
        assert!(HEALTH_SERVICES.contains(&"nebula"));
    }

    #[test]
    fn fmt_bytes_thresholds() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(2048), "2 KB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn fmt_duration_thresholds() {
        assert_eq!(fmt_duration(Duration::from_secs(30)), "30 s");
        assert_eq!(fmt_duration(Duration::from_secs(120)), "2 min");
        assert_eq!(fmt_duration(Duration::from_secs(7200)), "2 h");
        assert_eq!(fmt_duration(Duration::from_secs(2 * 86_400)), "2 d");
    }

    #[test]
    fn view_renders_without_panic() {
        let p = HealthCheckPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_with_probes_renders_without_panic() {
        let mut p = HealthCheckPanel::new();
        p.probes = run_all_probes();
        let _ = p.view();
    }

    #[test]
    fn probe_status_icons_are_distinct() {
        let icons: std::collections::HashSet<_> = [
            ProbeStatus::Ok,
            ProbeStatus::Warn,
            ProbeStatus::Fail,
            ProbeStatus::Unknown,
        ]
        .iter()
        .map(|s| s.icon())
        .collect();
        assert_eq!(icons.len(), 4);
    }
}

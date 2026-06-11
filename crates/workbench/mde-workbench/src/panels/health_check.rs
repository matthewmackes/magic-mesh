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

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

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
    /// W24 — a mackesd self-restart is in flight.
    pub restarting: bool,
    /// W24 — last restart outcome line (empty until one is issued).
    pub restart_msg: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<ProbeResult>),
    RunClicked,
    /// PLANES-6 / W24 — restart the mackesd daemon via systemd.
    RestartMackesd,
    /// W24 — the restart command resolved; re-probe for honest reconnect.
    Restarted(bool),
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
            Message::RestartMackesd => {
                if self.restarting {
                    return Task::none();
                }
                self.restarting = true;
                self.restart_msg = "Restarting mackesd…".into();
                Task::perform(
                    async {
                        // mackesd is a system unit (see mesh_services::MESH_UNITS).
                        crate::panels::mesh_services::run_pkexec_systemctl(
                            &crate::panels::mesh_services::UnitScope::System,
                            "restart",
                            "mackesd",
                        )
                        .await
                    },
                    |ok| crate::Message::HealthCheck(Message::Restarted(ok)),
                )
            }
            Message::Restarted(ok) => {
                self.restarting = false;
                if ok {
                    // W24 "honest reconnect": re-probe so the mackesd
                    // status reflects whether it actually came back.
                    self.restart_msg = "Restart issued — re-probing…".into();
                    self.busy = true;
                    Self::load()
                } else {
                    self.restart_msg =
                        "Restart failed (authorization declined or unit error).".into();
                    Task::none()
                }
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Health Check")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());

        let subtitle_text = if let Some(t) = self.last_run_at {
            format!("last run {}", fmt_age(t))
        } else if self.probes.is_empty() {
            "click Run to start".into()
        } else {
            String::new()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let run_btn = button(
            text(if self.busy {
                "Running…"
            } else {
                "Run checks"
            })
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
        .on_press(crate::Message::HealthCheck(Message::RunClicked));

        // PLANES-6 / W24 — mackesd self-restart, folded next to Run.
        // Disabled while a restart or probe pass is in flight.
        let restart_btn = crate::controls::variant_button(
            if self.restarting {
                "Restarting…"
            } else {
                "Restart mackesd"
            },
            crate::controls::ButtonVariant::Secondary,
            (!self.restarting && !self.busy)
                .then_some(crate::Message::HealthCheck(Message::RestartMackesd)),
            palette,
        );

        let mut left = column![title, subtitle].spacing(2);
        if !self.restart_msg.is_empty() {
            left = left.push(
                text(self.restart_msg.clone())
                    .size(TypeRole::Body.size_in(sizes))
                    .color(palette.text_muted.into_iced_color()),
            );
        }

        let header = row![left, Space::new().width(Length::Fill), restart_btn, run_btn,]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center);

        let mut probe_col = column![].spacing(8);
        for p in &self.probes {
            probe_col = probe_col.push(probe_row(p, palette));
        }
        if self.probes.is_empty() && !self.busy {
            probe_col = probe_col.push(
                container(
                    text("No probes have been run yet. Click \"Run checks\" above to refresh.")
                        .size(TypeRole::Body.size_in(sizes))
                        .color(palette.text_muted.into_iced_color()),
                )
                .padding(Padding::from([24u16, 0u16])),
            );
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(probe_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn probe_row<'a>(p: &'a ProbeResult, palette: Palette) -> Element<'a, crate::Message> {
    let resolved = mde_icon(p.status.icon(), IconSize::Inline);
    let icon_color = match p.status {
        ProbeStatus::Ok => palette.success.into_iced_color(),
        ProbeStatus::Warn => palette.warning.into_iced_color(),
        ProbeStatus::Fail => palette.danger.into_iced_color(),
        ProbeStatus::Unknown => palette.text_muted.into_iced_color(),
    };
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(icon_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(18.0)
            .color(icon_color)
            .into()
    };

    let label = text(p.name.clone())
        .size(14)
        .color(palette.text.into_iced_color());
    let status_chip = text(p.status.label()).size(11).color(icon_color);
    let detail = text(p.detail.clone())
        .size(12)
        .color(palette.text_muted.into_iced_color());
    let mut col = column![
        row![
            icon_widget,
            label,
            Space::new().width(Length::Fill),
            status_chip
        ]
        .spacing(10)
        .align_y(iced::alignment::Vertical::Center),
        detail,
    ]
    .spacing(2);
    if let Some(rem) = &p.remediation {
        col = col.push(
            text(format!("→ {rem}"))
                .size(11)
                .color(palette.text_muted.into_iced_color()),
        );
    }

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(col)
        .padding(Padding::from([12u16, 16u16]))
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

// ---- probes ---------------------------------------------------

#[must_use]
pub fn run_all_probes() -> Vec<ProbeResult> {
    vec![
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
    ]
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
        // 7 local probes + 3 PLANES-6/ENT-7 doctor probes (binaries,
        // daemon, overlay).
        let probes = run_all_probes();
        assert_eq!(probes.len(), 10);
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
    fn restart_click_marks_restarting_with_a_message() {
        // W24 — clicking Restart flips state + shows progress.
        let mut panel = HealthCheckPanel::new();
        let _ = panel.update(Message::RestartMackesd);
        assert!(panel.restarting);
        assert!(panel.restart_msg.contains("Restarting"));
    }

    #[test]
    fn restart_click_while_restarting_is_noop() {
        let mut panel = HealthCheckPanel::new();
        panel.restarting = true;
        panel.restart_msg = "Restarting mackesd…".into();
        let _ = panel.update(Message::RestartMackesd);
        assert!(panel.restarting);
    }

    #[test]
    fn restart_failure_reports_and_clears_restarting() {
        let mut panel = HealthCheckPanel::new();
        panel.restarting = true;
        let _ = panel.update(Message::Restarted(false));
        assert!(!panel.restarting);
        assert!(panel.restart_msg.to_lowercase().contains("failed"));
    }

    #[test]
    fn restart_success_reprobes_with_honest_reconnect() {
        // W24 — a successful restart clears restarting, sets busy (the
        // re-probe is in flight), and says it's re-probing.
        let mut panel = HealthCheckPanel::new();
        panel.restarting = true;
        let _ = panel.update(Message::Restarted(true));
        assert!(!panel.restarting);
        assert!(panel.busy, "re-probe should be in flight");
        assert!(panel.restart_msg.contains("re-probing"));
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

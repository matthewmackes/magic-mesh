//! v4.0.1 WB-2.j — Network → Mesh Services panel.
//!
//! Curated list of the mesh-fabric daemons (nebula,
//! nebula-lighthouse, mackes-nebula-https-tunnel, mackesd)
//! with active/enabled status pills + Start / Stop / Restart
//! buttons routed through `pkexec systemctl`. Each row also
//! surfaces the last journal lines so the operator can
//! diagnose failed-to-start without leaving the Workbench.
//!
//! v2.5 NF-5.4 (2026-05-24): swapped the legacy Tailscale unit
//! set (tailscaled / headscale / caddy / mackesd) for the
//! Nebula equivalents. Best-choice deviation from the worklist
//! "4 → 3" math: kept mackesd in the set (still mandatory) and
//! added all three Nebula units → 4 entries total.
//!
//! Chrome influence (per iteration skill Phase 0.8): Win11
//! Services snap-in — service name + status pill + per-row
//! action buttons.

use std::time::SystemTime;

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitScope {
    System,
    User,
}

impl UnitScope {
    fn systemctl_flag(self) -> &'static str {
        match self {
            Self::System => "--system",
            Self::User => "--user",
        }
    }
}

/// Curated list of daemons mde-workbench surfaces under Mesh
/// Services. Adjustable at compile time — operator-deployable
/// surfaces live in a future TOML, not this code.
pub const MESH_UNITS: &[(&str, UnitScope, &str)] = &[
    (
        "nebula",
        UnitScope::System,
        "Nebula overlay daemon — provides the mesh's encrypted point-to-point fabric",
    ),
    (
        "nebula-lighthouse",
        UnitScope::System,
        "Nebula lighthouse role — required on at least one always-on peer per mesh",
    ),
    (
        "mackes-nebula-https-tunnel",
        UnitScope::System,
        "TCP/443 covert tunnel for peers behind UDP-blocking firewalls (NF-1 fallback)",
    ),
    (
        "mackesd",
        UnitScope::System,
        "Mackes control plane — required for cross-mesh settings push + Files DBus surface",
    ),
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitStatus {
    pub name: String,
    pub scope: UnitScope,
    pub description: String,
    /// `active`, `inactive`, `failed`, `activating`, `not-found`.
    pub active_state: String,
    /// `enabled`, `disabled`, `static`, `not-found`.
    pub enable_state: String,
    /// Last 5 journal lines (best-effort).
    pub journal_tail: String,
}

impl Default for UnitScope {
    fn default() -> Self {
        Self::System
    }
}

#[derive(Debug, Clone, Default)]
pub struct MeshServicesPanel {
    pub units: Vec<UnitStatus>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub last_op: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<UnitStatus>),
    RefreshClicked,
    StartClicked {
        name: String,
        scope: UnitScope,
    },
    StopClicked {
        name: String,
        scope: UnitScope,
    },
    RestartClicked {
        name: String,
        scope: UnitScope,
    },
    OpFinished {
        name: String,
        op: String,
        success: bool,
    },
}

impl MeshServicesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { probe_all_units() }, |units| {
            crate::Message::MeshServices(Message::Loaded(units))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(units) => {
                self.units = units;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
            Message::StartClicked { name, scope } => {
                self.busy = true;
                self.last_op = format!("starting {name}");
                Task::perform(
                    async move {
                        let ok = run_pkexec_systemctl(&scope, "start", &name).await;
                        (name, "start".to_string(), ok)
                    },
                    |(name, op, success)| {
                        crate::Message::MeshServices(Message::OpFinished { name, op, success })
                    },
                )
            }
            Message::StopClicked { name, scope } => {
                self.busy = true;
                self.last_op = format!("stopping {name}");
                Task::perform(
                    async move {
                        let ok = run_pkexec_systemctl(&scope, "stop", &name).await;
                        (name, "stop".to_string(), ok)
                    },
                    |(name, op, success)| {
                        crate::Message::MeshServices(Message::OpFinished { name, op, success })
                    },
                )
            }
            Message::RestartClicked { name, scope } => {
                self.busy = true;
                self.last_op = format!("restarting {name}");
                Task::perform(
                    async move {
                        let ok = run_pkexec_systemctl(&scope, "restart", &name).await;
                        (name, "restart".to_string(), ok)
                    },
                    |(name, op, success)| {
                        crate::Message::MeshServices(Message::OpFinished { name, op, success })
                    },
                )
            }
            Message::OpFinished { name, op, success } => {
                self.last_op = if success {
                    format!("{op} {name}: ok")
                } else {
                    format!("{op} {name}: FAILED — see journalctl")
                };
                self.busy = false;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = Palette::dark();
        let sizes = FontSize::defaults();

        let title = text("Mesh Services")
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
        .on_press(crate::Message::MeshServices(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let mut units_col = column![].spacing(10);
        for u in &self.units {
            units_col = units_col.push(unit_row(u, palette));
        }
        if self.units.is_empty() && !self.busy {
            units_col = units_col.push(
                container(
                    text("Click \"Refresh\" to probe the mesh-fabric daemons.")
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
                scrollable(units_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn unit_row<'a>(u: &'a UnitStatus, palette: Palette) -> Element<'a, crate::Message> {
    let (status_icon, status_color, status_label) = match u.active_state.as_str() {
        "active" => (Icon::StatusOk, Color::from_rgb(0.20, 0.80, 0.40), "ACTIVE"),
        "activating" | "reloading" => (
            Icon::StatusWarning,
            Color::from_rgb(0.95, 0.70, 0.20),
            "ACTIVATING",
        ),
        "failed" => (
            Icon::StatusError,
            Color::from_rgb(0.92, 0.32, 0.30),
            "FAILED",
        ),
        "not-found" => (
            Icon::StatusUnknown,
            palette.text_muted.into_iced_color(),
            "NOT INSTALLED",
        ),
        _ => (
            Icon::StatusUnknown,
            palette.text_muted.into_iced_color(),
            "INACTIVE",
        ),
    };
    let resolved = mde_icon(status_icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(status_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(18.0)
            .color(status_color)
            .into()
    };

    let name_text = text(u.name.clone())
        .size(14)
        .color(palette.text.into_iced_color());
    let scope_chip = text(format!(
        "[{}]",
        match u.scope {
            UnitScope::System => "system",
            UnitScope::User => "user",
        }
    ))
    .size(11)
    .color(palette.text_muted.into_iced_color());
    let status_chip = text(status_label).size(11).color(status_color);
    let enable_chip = text(format!("{}", u.enable_state))
        .size(11)
        .color(palette.text_muted.into_iced_color());

    let description = text(u.description.clone())
        .size(12)
        .color(palette.text_muted.into_iced_color());

    let is_installed = u.active_state != "not-found";
    let name = u.name.clone();
    let scope = u.scope;
    let start_btn = button(text("Start").size(11).color(Color::WHITE))
        .padding(Padding::from([3u16, 10u16]))
        .style(action_btn_style(palette, false))
        .on_press(crate::Message::MeshServices(Message::StartClicked {
            name: name.clone(),
            scope,
        }));
    let stop_btn = button(text("Stop").size(11).color(palette.text.into_iced_color()))
        .padding(Padding::from([3u16, 10u16]))
        .style(action_btn_style(palette, true))
        .on_press(crate::Message::MeshServices(Message::StopClicked {
            name: name.clone(),
            scope,
        }));
    let restart_btn = button(
        text("Restart")
            .size(11)
            .color(palette.text.into_iced_color()),
    )
    .padding(Padding::from([3u16, 10u16]))
    .style(action_btn_style(palette, true))
    .on_press(crate::Message::MeshServices(Message::RestartClicked {
        name,
        scope,
    }));

    let buttons: Element<'a, crate::Message> = if is_installed {
        row![start_btn, stop_btn, restart_btn].spacing(6).into()
    } else {
        text("(unit not installed)")
            .size(11)
            .color(palette.text_muted.into_iced_color())
            .into()
    };

    let header_row = row![
        icon_widget,
        column![
            row![name_text, scope_chip, status_chip, enable_chip]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            description,
        ]
        .spacing(2),
        Space::new().width(Length::Fill),
        buttons,
    ]
    .spacing(10)
    .align_y(iced::alignment::Vertical::Center);

    let mut col = column![header_row].spacing(8);
    if !u.journal_tail.trim().is_empty() {
        col = col.push(
            container(
                text(u.journal_tail.clone())
                    .size(10)
                    .color(palette.text_muted.into_iced_color()),
            )
            .padding(Padding::from([8u16, 12u16]))
            .style(move |_| container::Style {
                snap: false,
                background: Some(Background::Color(Color {
                    r: 0.06,
                    g: 0.06,
                    b: 0.07,
                    a: 1.0,
                })),
                border: Border {
                    color: palette.border.into_iced_color(),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            }),
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

fn action_btn_style(
    palette: Palette,
    ghost: bool,
) -> impl Fn(&Theme, iced::widget::button::Status) -> iced::widget::button::Style {
    let accent = palette.accent.into_iced_color();
    let border = palette.border.into_iced_color();
    move |_t: &Theme, status: iced::widget::button::Status| {
        let (bg, text_color) = if ghost {
            let hover_bg = Color {
                r: 0.20,
                g: 0.20,
                b: 0.22,
                a: 1.0,
            };
            match status {
                iced::widget::button::Status::Hovered => (hover_bg, palette.text.into_iced_color()),
                _ => (Color::TRANSPARENT, palette.text.into_iced_color()),
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
            text_color,
            border: Border {
                color: if ghost { border } else { Color::TRANSPARENT },
                width: if ghost { 1.0 } else { 0.0 },
                radius: 4.0.into(),
            },
            shadow: iced::Shadow::default(),
        }
    }
}

// ---- probes / shell-outs --------------------------------------

#[must_use]
pub fn probe_all_units() -> Vec<UnitStatus> {
    MESH_UNITS
        .iter()
        .map(|(name, scope, description)| probe_unit(name, *scope, description))
        .collect()
}

fn probe_unit(name: &str, scope: UnitScope, description: &str) -> UnitStatus {
    // `LoadState` distinguishes "really missing" from
    // "inactive but exists". `ActiveState` reports inactive
    // for every unknown unit which would tag every uninstalled
    // optional daemon as "inactive" instead of "not installed".
    let load_state = systemctl_show_property(name, scope, "LoadState");
    let active_state = if load_state == "not-found" || load_state == "not-loaded" {
        "not-found".to_string()
    } else {
        systemctl_show_property(name, scope, "ActiveState")
    };
    let enable_state = systemctl_show_property(name, scope, "UnitFileState");
    let journal_tail = if active_state == "not-found" {
        String::new()
    } else {
        journalctl_tail(name, scope, 5)
    };
    UnitStatus {
        name: name.to_string(),
        scope,
        description: description.to_string(),
        active_state,
        enable_state,
        journal_tail,
    }
}

fn systemctl_show_property(name: &str, scope: UnitScope, property: &str) -> String {
    let out = std::process::Command::new("systemctl")
        .args([
            scope.systemctl_flag(),
            "show",
            "-p",
            property,
            "--value",
            name,
        ])
        .output();
    let Ok(out) = out else {
        return "not-found".into();
    };
    if !out.status.success() {
        return "not-found".into();
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() || s == "(null)" {
        "not-found".into()
    } else {
        s
    }
}

fn journalctl_tail(name: &str, scope: UnitScope, lines: u32) -> String {
    let out = std::process::Command::new("journalctl")
        .args([
            scope.systemctl_flag(),
            "-u",
            name,
            "-n",
            &lines.to_string(),
            "--no-pager",
            "--output=cat",
        ])
        .output();
    let Ok(out) = out else { return String::new() };
    if !out.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Shell out to `pkexec systemctl <op> <name>` (with the
/// scope flag for user units).
pub async fn run_pkexec_systemctl(scope: &UnitScope, op: &str, name: &str) -> bool {
    use tokio::process::Command;
    let scope_flag = scope.systemctl_flag();
    // pkexec doesn't preserve --user / --system order well in
    // every distro; emit the flag at the front of the systemctl
    // arg list and use the well-known absolute path.
    let status = Command::new("pkexec")
        .args(["/usr/bin/systemctl", scope_flag, op, name])
        .status()
        .await;
    status.map(|s| s.success()).unwrap_or(false)
}

fn fmt_age(t: SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    let secs = elapsed.as_secs();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_units_curated_set_size() {
        // Locked at v2.5 NF-5.4: nebula, nebula-lighthouse,
        // mackes-nebula-https-tunnel, mackesd. Extending the list
        // is a worklist change, not a code-only one.
        assert_eq!(MESH_UNITS.len(), 4);
        let names: Vec<&str> = MESH_UNITS.iter().map(|(n, _, _)| *n).collect();
        assert!(names.contains(&"nebula"));
        assert!(names.contains(&"nebula-lighthouse"));
        assert!(names.contains(&"mackes-nebula-https-tunnel"));
        assert!(names.contains(&"mackesd"));
        // Legacy Tailscale stack must not regress in.
        assert!(!names.contains(&"tailscaled"));
        assert!(!names.contains(&"headscale"));
        assert!(!names.contains(&"caddy"));
    }

    #[test]
    fn probe_all_units_returns_one_per_curated_entry() {
        let units = probe_all_units();
        assert_eq!(units.len(), MESH_UNITS.len());
    }

    #[test]
    fn probe_unit_for_nonexistent_unit_returns_not_found() {
        let u = probe_unit("definitely-not-a-real-unit-xyz", UnitScope::System, "test");
        assert_eq!(u.active_state, "not-found");
    }

    #[test]
    fn systemctl_flag_routes_user_vs_system() {
        assert_eq!(UnitScope::System.systemctl_flag(), "--system");
        assert_eq!(UnitScope::User.systemctl_flag(), "--user");
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = MeshServicesPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_units_without_panic() {
        let mut p = MeshServicesPanel::new();
        p.units = probe_all_units();
        let _ = p.view();
    }

    #[test]
    fn fmt_age_thresholds() {
        let now = SystemTime::now();
        assert_eq!(fmt_age(now), "0 s");
    }
}

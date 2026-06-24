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

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::components::connect_progress::{self, ConnectProgress};
use crate::cosmic_compat::prelude::*;

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
        "TCP/443 covert tunnel (NF-1 fallback) — in-process in mackesd: relay listener on public nodes, client fallback on NAT'd peers",
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
    /// MESH-CONNECT-DIALOG-1 — the connect/start progress modal: pending
    /// (the unit is starting/joining) → success / failure (the real
    /// post-op `systemctl` state). Carries the unit being acted on so
    /// Retry re-runs the same start.
    pub connect: ConnectProgress,
    /// The `(name, scope)` of the unit the open modal is starting — drives
    /// the modal's Retry.
    pub connect_target: Option<(String, UnitScope)>,
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
    /// MESH-CONNECT-DIALOG-1 — the post-start re-probe landed: the unit's
    /// real `ActiveState` after a connect/start, so the modal can show the
    /// terminal outcome (running = success, anything else = failure).
    ConnectProbed {
        name: String,
        active_state: String,
    },
    /// MESH-CONNECT-DIALOG-1 — re-run the start from the modal's failure state.
    ConnectRetry,
    /// MESH-CONNECT-DIALOG-1 — close the connect modal (Dismiss / backdrop).
    ConnectDismiss,
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
            Message::StartClicked { name, scope } => self.start_unit(name, scope),
            // MESH-CONNECT-DIALOG-1 — Retry re-runs the start for the unit the
            // open modal remembers (no-op if the modal has no target).
            Message::ConnectRetry => match self.connect_target.clone() {
                Some((name, scope)) => self.start_unit(name, scope),
                None => Task::none(),
            },
            Message::ConnectDismiss => {
                self.connect = ConnectProgress::Closed;
                self.connect_target = None;
                Task::none()
            }
            // MESH-CONNECT-DIALOG-1 — the post-start re-probe landed; resolve
            // the modal from the unit's real state (active = success). Guard on
            // the live target + pending: a late probe (it's delayed ~750ms) must
            // NOT resurrect a dismissed modal or clobber one already re-opened
            // for a different unit.
            Message::ConnectProbed { name, active_state } => {
                let is_current = self.connect.is_pending()
                    && self
                        .connect_target
                        .as_ref()
                        .is_some_and(|(t, _)| *t == name);
                if is_current {
                    let running = active_state == "active";
                    self.connect = if running {
                        self.connect
                            .success(format!("{name} is running and connected to the mesh."))
                    } else {
                        self.connect.failure(format!(
                            "{name} did not come up (state: {active_state}). See the journal below."
                        ))
                    };
                }
                Task::none()
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
                // MESH-CONNECT-DIALOG-1 — a connect/start with the modal open
                // doesn't trust the pkexec exit alone (a unit can exit 0 then
                // fail its ExecStart): re-probe the unit's real state to set the
                // terminal outcome, alongside the panel reload. Stop/Restart (no
                // modal) just reload.
                let is_connect_start = op == "start"
                    && self.connect.is_pending()
                    && self
                        .connect_target
                        .as_ref()
                        .is_some_and(|(t, _)| *t == name);
                if is_connect_start {
                    let scope = self
                        .connect_target
                        .as_ref()
                        .map_or(UnitScope::System, |(_, s)| *s);
                    Task::batch(vec![
                        Self::load(),
                        Task::perform(
                            async move {
                                let active_state = probe_active_state(&name, scope).await;
                                (name, active_state)
                            },
                            move |(name, active_state)| {
                                crate::Message::MeshServices(Message::ConnectProbed {
                                    name,
                                    active_state,
                                })
                            },
                        ),
                    ])
                } else {
                    Self::load()
                }
            }
        }
    }

    /// MESH-CONNECT-DIALOG-1 — start `name` and open the connect/start
    /// progress modal. Shared by the row's Start button and the modal's
    /// Retry. The post-start re-probe ([`Message::ConnectProbed`]) sets the
    /// terminal outcome from the unit's real `ActiveState`.
    fn start_unit(&mut self, name: String, scope: UnitScope) -> Task<crate::Message> {
        self.busy = true;
        self.last_op = format!("starting {name}");
        self.connect = ConnectProgress::pending(
            format!("Connect {name}"),
            format!("Starting {name} and joining the mesh fabric…"),
        );
        self.connect_target = Some((name.clone(), scope));
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

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Mesh Services")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if !self.last_op.is_empty() {
            self.last_op.clone()
        } else if let Some(t) = self.last_run_at {
            format!("last refresh {}", fmt_age(t))
        } else {
            "click Refresh to probe".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let refresh_btn = button(
            text(if self.busy { "Working…" } else { "Refresh" })
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
        .on_press(crate::Message::MeshServices(Message::RefreshClicked));

        // MOTION-NET-1 — surface the probe state through the canonical
        // LoadState indicator instead of only the button's "Working…" label, so
        // this panel reads async state the same way every other surface will.
        let load = if self.busy {
            if self.units.is_empty() {
                mde_theme::LoadState::Loading
            } else {
                mde_theme::LoadState::Refreshing { stale: true }
            }
        } else if self.units.is_empty() && self.last_run_at.is_none() {
            mde_theme::LoadState::Idle
        } else {
            mde_theme::LoadState::Loaded
        };

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            crate::panel_chrome::load_state_indicator(load, palette),
            refresh_btn,
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        // MOTION-NET-3 — during a refresh the previous units stay on screen
        // (never blank) but render DIMMED via the load state's content alpha, with
        // the header's "Refreshing" indicator (MOTION-NET-1) showing it's live;
        // they snap back to full opacity when fresh data lands.
        let unit_palette = if load.is_busy() && !self.units.is_empty() {
            palette.dimmed(load.content_alpha())
        } else {
            palette
        };
        let mut units_col = column![].spacing(10);
        for u in &self.units {
            units_col = units_col.push(unit_row(u, unit_palette));
        }
        if self.units.is_empty() {
            if load.is_busy() {
                // MOTION-NET-2 — a slow first probe shows layout-matching skeleton
                // rows (shimmering, or static grey under reduce-motion) instead of
                // a blank panel; they vanish the moment real units land.
                let phase = {
                    use std::time::{SystemTime, UNIX_EPOCH};
                    let ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    (ms % 1200) as f32 / 1200.0
                };
                units_col = units_col.push(crate::panel_chrome::skeleton(
                    5,
                    palette,
                    phase,
                    crate::live_theme::reduce_motion(),
                ));
            } else {
                units_col = units_col.push(
                    container(
                        text("Click \"Refresh\" to probe the mesh-fabric daemons.")
                            .size(TypeRole::Body.size_in(sizes))
                            .colr(palette.text_muted.into_cosmic_color()),
                    )
                    .padding(Padding::from([24u16, 0u16])),
                );
            }
        }

        let body: Element<'_, crate::Message> = container(
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
        .into();

        // MESH-CONNECT-DIALOG-1 — stack the connect/start progress modal over
        // the panel body while a Start is in flight or showing its outcome.
        connect_progress::overlay(
            &self.connect,
            body,
            palette,
            crate::Message::MeshServices(Message::ConnectRetry),
            crate::Message::MeshServices(Message::ConnectDismiss),
        )
    }
}

fn unit_row<'a>(u: &'a UnitStatus, palette: Palette) -> Element<'a, crate::Message> {
    let (status_icon, status_color, status_label) = match u.active_state.as_str() {
        "active" => (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            "ACTIVE",
        ),
        "activating" | "reloading" => (
            Icon::StatusWarning,
            palette.warning.into_cosmic_color(),
            "ACTIVATING",
        ),
        "failed" => (
            Icon::StatusError,
            palette.danger.into_cosmic_color(),
            "FAILED",
        ),
        "not-found" => (
            Icon::StatusUnknown,
            palette.text_muted.into_cosmic_color(),
            "NOT INSTALLED",
        ),
        _ => (
            Icon::StatusUnknown,
            palette.text_muted.into_cosmic_color(),
            "INACTIVE",
        ),
    };
    let resolved = mde_icon(status_icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(status_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(18.0)
            .colr(status_color)
            .into()
    };

    let name_text = text(u.name.clone())
        .size(14)
        .colr(palette.text.into_cosmic_color());
    let scope_chip = text(format!(
        "[{}]",
        match u.scope {
            UnitScope::System => "system",
            UnitScope::User => "user",
        }
    ))
    .size(11)
    .colr(palette.text_muted.into_cosmic_color());
    let status_chip = text(status_label).size(11).colr(status_color);
    let enable_chip = text(format!("{}", u.enable_state))
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());

    let description = text(u.description.clone())
        .size(12)
        .colr(palette.text_muted.into_cosmic_color());

    let is_installed = u.active_state != "not-found";
    let name = u.name.clone();
    let scope = u.scope;
    let start_btn = button(text("Start").size(11).colr(Color::WHITE))
        .padding(Padding::from([3u16, 10u16]))
        .sty(action_btn_style(palette, false))
        .on_press(crate::Message::MeshServices(Message::StartClicked {
            name: name.clone(),
            scope,
        }));
    let stop_btn = button(text("Stop").size(11).colr(palette.text.into_cosmic_color()))
        .padding(Padding::from([3u16, 10u16]))
        .sty(action_btn_style(palette, true))
        .on_press(crate::Message::MeshServices(Message::StopClicked {
            name: name.clone(),
            scope,
        }));
    let restart_btn = button(
        text("Restart")
            .size(11)
            .colr(palette.text.into_cosmic_color()),
    )
    .padding(Padding::from([3u16, 10u16]))
    .sty(action_btn_style(palette, true))
    .on_press(crate::Message::MeshServices(Message::RestartClicked {
        name,
        scope,
    }));

    let buttons: Element<'a, crate::Message> = if is_installed {
        row![start_btn, stop_btn, restart_btn].spacing(6).into()
    } else {
        text("(unit not installed)")
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .into()
    };

    let header_row = row![
        icon_widget,
        column![
            row![name_text, scope_chip, status_chip, enable_chip]
                .spacing(8)
                .align_y(cosmic::iced::alignment::Vertical::Center),
            description,
        ]
        .spacing(2),
        Space::new().width(Length::Fill),
        buttons,
    ]
    .spacing(10)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut col = column![header_row].spacing(8);
    if !u.journal_tail.trim().is_empty() {
        col = col.push(
            container(
                text(u.journal_tail.clone())
                    .size(10)
                    .colr(palette.text_muted.into_cosmic_color()),
            )
            .padding(Padding::from([8u16, 12u16]))
            .sty(move |_| container::Style {
                snap: false,
                // Recessed journal inset: the darkest surface token (Carbon Gray 100).
                background: Some(Background::Color(palette.background.into_cosmic_color())),
                border: Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            }),
        );
    }

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(col)
        .padding(Padding::from([12u16, 16u16]))
        .width(Length::Fill)
        .sty(move |_| container::Style {
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
) -> impl Fn(&Theme, cosmic::iced::widget::button::Status) -> cosmic::iced::widget::button::Style {
    let accent = palette.accent.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let hover_bg = palette.overlay.into_cosmic_color();
    move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
        let (bg, text_color) = if ghost {
            match status {
                cosmic::iced::widget::button::Status::Hovered => {
                    (hover_bg, palette.text.into_cosmic_color())
                }
                _ => (Color::TRANSPARENT, palette.text.into_cosmic_color()),
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
            text_color,
            border: Border {
                color: if ghost { border } else { Color::TRANSPARENT },
                width: if ghost { 1.0 } else { 0.0 },
                radius: 4.0.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            ..cosmic::iced::widget::button::Style::default()
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
    // SUBAUDIT-D1 — the covert :443 tunnel is an IN-PROCESS mackesd
    // capability, not a systemd unit: the relay listener binds :443 on a
    // public node, and on a NAT'd peer the client fallback rides mackesd.
    // Checking a `mackes-nebula-https-tunnel` systemd unit always returned
    // "not-found" → a permanent false "NOT INSTALLED". Report it as a
    // facet of mackesd: active wherever mackesd is, with a local :443-bound
    // upgrade to confirm the relay listener.
    if name == "mackes-nebula-https-tunnel" {
        let mackesd = systemctl_show_property("mackesd", UnitScope::System, "ActiveState");
        let relay_listening = tcp_probe_443();
        let active_state = if relay_listening {
            "active".to_string()
        } else if mackesd == "active" {
            // Client fallback present (rides mackesd); listener not bound here.
            "active".to_string()
        } else {
            mackesd
        };
        return UnitStatus {
            name: name.to_string(),
            scope,
            description: description.to_string(),
            active_state,
            enable_state: "in-process".to_string(),
            journal_tail: String::new(),
        };
    }
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

/// MESH-CONNECT-DIALOG-1 — re-probe `name`'s real `ActiveState` after a
/// short settle delay, off the iced executor. A freshly-started unit
/// often reports `activating` for a beat before it reaches `active`, so
/// give it a moment before reading the terminal state the connect modal
/// renders. `systemctl_show_property` is a sync `std::process::Command`,
/// so it rides `spawn_blocking`.
async fn probe_active_state(name: &str, scope: UnitScope) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    let name = name.to_string();
    tokio::task::spawn_blocking(move || systemctl_show_property(&name, scope, "ActiveState"))
        .await
        .unwrap_or_else(|_| "not-found".to_string())
}

/// SUBAUDIT-D1 — is the covert :443 relay listener bound locally? A quick
/// loopback TCP connect (200 ms) — `true` confirms the relay tunnel is up.
fn tcp_probe_443() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    "127.0.0.1:443"
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .map(|addr| {
            TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
        })
        .unwrap_or(false)
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

    // MESH-CONNECT-DIALOG-1 — the connect/start progress modal.

    #[test]
    fn start_unit_opens_the_pending_modal_with_a_target() {
        let mut p = MeshServicesPanel::new();
        let _ = p.start_unit("nebula".into(), UnitScope::System);
        assert!(
            p.connect.is_pending(),
            "Start opens the modal in its pending state"
        );
        assert_eq!(
            p.connect_target,
            Some(("nebula".to_string(), UnitScope::System))
        );
    }

    #[test]
    fn connect_probed_resolves_the_matching_pending_modal() {
        let mut p = MeshServicesPanel::new();
        let _ = p.start_unit("nebula".into(), UnitScope::System);
        let _ = p.update(Message::ConnectProbed {
            name: "nebula".into(),
            active_state: "active".into(),
        });
        assert!(matches!(p.connect, ConnectProgress::Success { .. }));
    }

    #[test]
    fn connect_probed_failure_for_a_non_active_state() {
        let mut p = MeshServicesPanel::new();
        let _ = p.start_unit("nebula".into(), UnitScope::System);
        let _ = p.update(Message::ConnectProbed {
            name: "nebula".into(),
            active_state: "failed".into(),
        });
        assert!(matches!(p.connect, ConnectProgress::Failure { .. }));
    }

    #[test]
    fn stale_connect_probed_does_not_resurrect_a_dismissed_modal() {
        let mut p = MeshServicesPanel::new();
        let _ = p.start_unit("nebula".into(), UnitScope::System);
        let _ = p.update(Message::ConnectDismiss); // close while the probe is delayed
                                                   // The delayed post-start probe lands — it must NOT reopen the modal.
        let _ = p.update(Message::ConnectProbed {
            name: "nebula".into(),
            active_state: "active".into(),
        });
        assert!(
            !p.connect.is_open(),
            "a stale probe must not resurrect a dismissed modal"
        );
        assert!(p.connect_target.is_none());
    }

    #[test]
    fn stale_connect_probed_for_a_different_unit_is_ignored() {
        let mut p = MeshServicesPanel::new();
        let _ = p.start_unit("nebula".into(), UnitScope::System);
        // A probe for a DIFFERENT unit (an earlier start's late probe) must not
        // clobber the modal that's now pending for nebula.
        let _ = p.update(Message::ConnectProbed {
            name: "mackesd".into(),
            active_state: "active".into(),
        });
        assert!(p.connect.is_pending(), "the nebula modal stays pending");
    }

    #[test]
    fn dismiss_closes_the_modal_and_clears_the_target() {
        let mut p = MeshServicesPanel::new();
        let _ = p.start_unit("nebula".into(), UnitScope::System);
        let _ = p.update(Message::ConnectDismiss);
        assert!(!p.connect.is_open());
        assert!(p.connect_target.is_none());
    }
}

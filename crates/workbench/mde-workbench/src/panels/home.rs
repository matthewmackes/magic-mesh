//! v2.6 OV-1..OV-11 — Workbench Overview landing page.
//!
//! Workbench's default route when launched without `--focus`.
//! Renders three stacked surfaces:
//!
//!   1. **Identity strip** — MDE X.Y.Z · Fedora N · hostname.
//!   2. **Hero stat grid** — mesh peers / pending updates /
//!      snapshots / drift count (4 cards from the original WB-2.a
//!      panel, preserved for continuity).
//!   3. **Capability list** — 11 rows mirroring the cross-host
//!      mesh capability list (mesh network, peer reachability,
//!      file sharing, SSH, RDP, VNC, media & app discovery,
//!      phone pairing, voice & video, fleet config, desktop
//!      notifications). Each row carries a live status pill +
//!      one-sentence plain-English description + jump button
//!      that deep-links to the panel where the user configures
//!      that capability.
//!
//! Backend integration:
//! - peers / snapshots / drift snapshot counts read filesystem
//!   caches (BUG-11 `~/.cache/mde/dnf-updates.count`,
//!   `~/.local/share/mackes-shell/snapshots/`);
//! - capability probes shell out to systemctl / dbus-send /
//!   `mackesd nodes list --json` / `mackesd meshfs-status --json`
//!   and read the mesh Bus (`state/voice/status`) in parallel via
//!   `tokio::join!`;
//! - live refresh comes from a D-Bus signal subscription
//!   (see `dbus_subscription`) bridging systemd1
//!   PropertiesChanged + Nebula peer/transport signals +
//!   Fleet revision signals into `Message::DbusEvent`.
//!
//! Every row carries a live status + an action: rows with a Workbench
//! settings panel deep-link to it (Configure), and Voice & Video — whose
//! config surface is the standalone `mde-voice-config` app — launches that
//! via [`crate::Message::LaunchApp`]. All eleven capabilities are real
//! (the former "Coming in vX" placeholders for File Sharing / phone
//! pairing / Voice & Video were retired once their backends shipped:
//! MeshFS chunkservers, the KDE Connect host, and the `mde-voice-hud`
//! SIP agent respectively).

use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, svg as widget_svg, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::model::Group;
use crate::panels::mesh_services::MESH_UNITS;
use crate::panels::mesh_topology::fetch_peers;

// ---------------------------------------------------------------------------
// Capability types (OV-1)
// ---------------------------------------------------------------------------

/// Stable per-row identity. The order this enum is declared in
/// matches the render order in `build_all_rows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityId {
    Mesh,
    Peers,
    Files,
    Ssh,
    Rdp,
    Vnc,
    Services,
    Phone,
    Voice,
    Fleet,
    Notifications,
}

/// What the status pill should communicate for a single
/// capability row. Color + icon match the mesh_topology palette
/// so the rest of the workbench renders status consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityStatus {
    /// Green — capability is up and serving.
    Active,
    /// Yellow — capability ships in this version but is not
    /// currently running (operator action needed).
    SetupNeeded,
    /// Red — capability shipped, ran, and is in an error state.
    /// `detail` carries an operator-readable one-liner that
    /// shows up as sub-status.
    Failed { detail: String },
    /// Probe could not determine state (mackesd down, no
    /// systemctl access, etc.). Renders gray with "Unknown".
    Unknown,
}

impl CapabilityStatus {
    #[must_use]
    pub fn icon(&self) -> Icon {
        match self {
            Self::Active => Icon::StatusOk,
            Self::SetupNeeded => Icon::StatusWarning,
            Self::Unknown => Icon::StatusUnknown,
            Self::Failed { .. } => Icon::StatusError,
        }
    }
    #[must_use]
    pub fn color(&self) -> Color {
        match self {
            Self::Active => Color::from_rgb(0.20, 0.80, 0.40),
            Self::SetupNeeded => Color::from_rgb(0.95, 0.70, 0.20),
            Self::Unknown => Color::from_rgb(0.55, 0.55, 0.55),
            Self::Failed { .. } => Color::from_rgb(0.92, 0.32, 0.30),
        }
    }
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Active => "Active".into(),
            Self::SetupNeeded => "Setup needed".into(),
            Self::Failed { .. } => "Failed".into(),
            Self::Unknown => "Unknown".into(),
        }
    }
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Failed { detail } => Some(detail.as_str()),
            _ => None,
        }
    }
}

/// One row in the capability list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRow {
    pub id: CapabilityId,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: Icon,
    pub status: CapabilityStatus,
    /// Optional secondary line under the description — e.g.
    /// "3 of 5 peers online" or "last sync 2 minutes ago".
    pub sub_status: Option<String>,
    /// Where the Configure button takes the user — a Workbench panel
    /// deep-link. `None` => no in-Workbench panel (the row may still
    /// carry a [`Self::launch`] target instead).
    pub jump: Option<(Group, &'static str)>,
    /// A standalone app to spawn when the capability is configured
    /// outside the Workbench (e.g. Voice & Video → `mde-voice-config`).
    /// Mutually exclusive with [`Self::jump`] in practice; when both are
    /// `None` the row shows status only.
    pub launch: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Snapshot + panel state (OV-3)
// ---------------------------------------------------------------------------

/// Pure-data snapshot of what the Overview shows.
#[derive(Debug, Clone, Default)]
pub struct HomeSnapshot {
    pub mde_version: String,
    pub fedora_release: String,
    pub hostname: String,
    /// Hero stat counts. `None` = unknown → renders "—".
    pub mesh_peers: Option<u32>,
    pub pending_updates: Option<u32>,
    pub snapshot_count: Option<u32>,
    pub drift_count: Option<u32>,
    /// Capability list — populated by the async load. Empty
    /// vec renders the section with a "Loading…" placeholder.
    pub capabilities: Vec<CapabilityRow>,
    /// True if `dev.mackes.MDE.Shell::Healthz()` succeeded on
    /// the last probe. False renders a top banner reminding the
    /// operator that D-Bus-sourced rows may be stale.
    pub mackesd_reachable: bool,
}

impl HomeSnapshot {
    /// Synchronous filesystem-only load. Cheap; no async.
    #[must_use]
    pub fn load_sync() -> Self {
        Self {
            mde_version: env!("CARGO_PKG_VERSION").to_string(),
            fedora_release: read_fedora_release().unwrap_or_else(|| "44".into()),
            hostname: read_hostname(),
            mesh_peers: None,
            pending_updates: Some(read_dnf_count()),
            snapshot_count: count_snapshots(),
            drift_count: None,
            capabilities: Vec::new(),
            mackesd_reachable: true,
        }
    }
}

fn read_fedora_release() -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VERSION_ID=") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn read_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "fedora".into())
}

fn read_dnf_count() -> u32 {
    let cache = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache"))
        })
        .unwrap_or_default();
    std::fs::read_to_string(cache.join("mde/dnf-updates.count"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

fn count_snapshots() -> Option<u32> {
    let home = std::env::var("HOME").ok().map(PathBuf::from)?;
    let dir = home.join(".local/share/mackes-shell/snapshots");
    let entries = std::fs::read_dir(&dir).ok()?;
    let n = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .count();
    n.try_into().ok()
}

// ---------------------------------------------------------------------------
// Panel state machine
// ---------------------------------------------------------------------------

/// What a probe reported. The status enum is the source of
/// truth for the pill; sub_status is the optional one-liner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub status: CapabilityStatus,
    pub sub_status: Option<String>,
}

impl ProbeOutcome {
    fn active(sub_status: Option<String>) -> Self {
        Self {
            status: CapabilityStatus::Active,
            sub_status,
        }
    }
    fn setup_needed(sub_status: Option<String>) -> Self {
        Self {
            status: CapabilityStatus::SetupNeeded,
            sub_status,
        }
    }
    fn unknown() -> Self {
        Self {
            status: CapabilityStatus::Unknown,
            sub_status: None,
        }
    }
    fn failed(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            status: CapabilityStatus::Failed {
                detail: detail.clone(),
            },
            sub_status: Some(detail),
        }
    }
}

/// A single D-Bus or systemd event that warrants re-probing a
/// specific capability. The subscription bridges raw signals
/// into one of these.
#[derive(Debug, Clone)]
pub enum DbusEvent {
    /// Peer-set membership or reachability changed — re-probe
    /// Mesh + Peers + Fleet (fleet revision push triggers this
    /// too, since revisions are pushed via the peer set).
    PeerChanged,
    /// Active transport rotated — re-probe Mesh.
    TransportChanged,
    /// A systemd unit's PropertiesChanged fired — re-probe SSH,
    /// RDP, VNC, or Services depending on `unit`.
    UnitChanged(String),
    // E0.3.3 — `FleetRevisionPushed` retired with the dev.mackes.MDE.Fleet
    // D-Bus surface; Phase G re-adds a Fleet event when revision-apply
    // lands (via a Bus event topic + subscription, like Nebula's).
}

#[derive(Debug, Clone)]
pub enum Message {
    Refreshed(HomeSnapshot),
    CapabilitiesRefreshed {
        rows: Vec<CapabilityRow>,
        mackesd_reachable: bool,
    },
    RefreshClicked,
    DbusEvent(DbusEvent),
}

#[derive(Debug, Clone, Default)]
pub struct HomePanel {
    pub snapshot: HomeSnapshot,
}

impl HomePanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshot: HomeSnapshot::load_sync(),
        }
    }

    /// Initial load. Fires the cheap sync snapshot immediately +
    /// the async capability fan-out in parallel.
    pub fn load() -> Task<crate::Message> {
        Task::batch(vec![
            Task::perform(async { HomeSnapshot::load_sync() }, |snap| {
                crate::Message::Home(Message::Refreshed(snap))
            }),
            Task::perform(load_capabilities(), |(rows, ok)| {
                crate::Message::Home(Message::CapabilitiesRefreshed {
                    rows,
                    mackesd_reachable: ok,
                })
            }),
        ])
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Refreshed(snap) => {
                // Preserve capabilities + mackesd_reachable from
                // prior load if the sync snapshot fired alone.
                let capabilities = std::mem::take(&mut self.snapshot.capabilities);
                let mackesd_reachable = self.snapshot.mackesd_reachable;
                self.snapshot = HomeSnapshot {
                    capabilities,
                    mackesd_reachable,
                    ..snap
                };
                Task::none()
            }
            Message::CapabilitiesRefreshed {
                rows,
                mackesd_reachable,
            } => {
                self.snapshot.capabilities = rows;
                self.snapshot.mackesd_reachable = mackesd_reachable;
                // Also refresh the hero mesh_peers count from
                // the capabilities (Peers row's sub_status
                // already carries the X/Y string; the count
                // belongs in the hero too for continuity).
                self.snapshot.mesh_peers = self
                    .snapshot
                    .capabilities
                    .iter()
                    .find(|r| r.id == CapabilityId::Peers)
                    .and_then(|r| extract_peer_count(r));
                Task::none()
            }
            Message::RefreshClicked => Self::load(),
            Message::DbusEvent(ev) => Task::perform(reprobe_for_event(ev), |(rows, ok)| {
                crate::Message::Home(Message::CapabilitiesRefreshed {
                    rows,
                    mackesd_reachable: ok,
                })
            }),
        }
    }

    /// Render the Overview.
    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = Palette::dark();
        let sizes = FontSize::defaults();

        let title = text("Overview")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());

        let identity = text(format!(
            "MDE {ver} · Fedora {rel} · {host}",
            ver = self.snapshot.mde_version,
            rel = self.snapshot.fedora_release,
            host = self.snapshot.hostname,
        ))
        .size(TypeRole::Body.size_in(sizes))
        .color(palette.text_muted.into_iced_color());

        let cards = row![
            stat_card(
                "Mesh peers",
                self.snapshot.mesh_peers,
                Icon::Peer,
                Group::Fleet,
                "inventory",
                palette,
            ),
            Space::new().width(Length::Fixed(12.0)),
            stat_card(
                "Updates pending",
                self.snapshot.pending_updates,
                Icon::Update,
                Group::Maintain,
                "snapshots",
                palette,
            ),
            Space::new().width(Length::Fixed(12.0)),
            stat_card(
                "Snapshots",
                self.snapshot.snapshot_count,
                Icon::Snapshot,
                Group::Maintain,
                "snapshots",
                palette,
            ),
            Space::new().width(Length::Fixed(12.0)),
            stat_card(
                "Drift events",
                self.snapshot.drift_count,
                Icon::Repair,
                Group::Maintain,
                "drift",
                palette,
            ),
        ];

        // Optional mackesd-down banner — only renders when the
        // last probe could not reach the control plane.
        let banner: Element<'_, crate::Message> = if self.snapshot.mackesd_reachable {
            Space::new().height(Length::Fixed(0.0)).into()
        } else {
            mackesd_banner(palette)
        };

        let section_title = text("What this Mackes mesh can do for you")
            .size(TypeRole::Heading.size_in(sizes))
            .color(palette.text.into_iced_color());
        let section_subtitle = text(
            "Each row shows a feature, whether it is running on this machine, \
             and a shortcut to set it up.",
        )
        .size(TypeRole::Body.size_in(sizes))
        .color(palette.text_muted.into_iced_color());

        let refresh_btn = refresh_button(palette);

        let capability_list: Element<'_, crate::Message> = if self.snapshot.capabilities.is_empty()
        {
            text("Loading capability status…")
                .size(TypeRole::Body.size_in(sizes))
                .color(palette.text_muted.into_iced_color())
                .into()
        } else {
            let mut col = column![].spacing(8);
            for row_data in &self.snapshot.capabilities {
                col = col.push(capability_card(row_data, palette));
            }
            col.into()
        };

        // E6.2 — role navigation + a "See also" bridge to Win10 Settings.
        let manage_label = text("Manage")
            .size(TypeRole::Heading.size_in(sizes))
            .color(palette.text.into_iced_color());
        let manage_row = row![
            role_link("Apps", Group::Apps, palette),
            role_link("Devices", Group::Devices, palette),
            role_link("Fleet", Group::Fleet, palette),
            role_link("Maintain", Group::Maintain, palette),
            role_link("System", Group::System, palette),
        ]
        .spacing(8);
        let see_also_row = row![
            text("See also:")
                .size(TypeRole::Body.size_in(sizes))
                .color(palette.text_muted.into_iced_color()),
            settings_link("All settings", "", palette),
            settings_link("Storage", "storage", palette),
            settings_link("Backup & recovery", "recovery", palette),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center);

        let body = column![
            title,
            Space::new().height(Length::Fixed(4.0)),
            identity,
            Space::new().height(Length::Fixed(24.0)),
            cards,
            Space::new().height(Length::Fixed(24.0)),
            manage_label,
            Space::new().height(Length::Fixed(8.0)),
            manage_row,
            Space::new().height(Length::Fixed(8.0)),
            see_also_row,
            Space::new().height(Length::Fixed(24.0)),
            banner,
            row![section_title, Space::new().width(Length::Fill), refresh_btn]
                .align_y(iced::alignment::Vertical::Center),
            Space::new().height(Length::Fixed(2.0)),
            section_subtitle,
            Space::new().height(Length::Fixed(12.0)),
            capability_list,
        ]
        .spacing(2);

        container(scrollable(body).width(Length::Fill))
            .padding(Padding::from([24u16, 32u16]))
            .width(Length::Fill)
            .into()
    }
}

// ---------------------------------------------------------------------------
// E6.2 — Dashboard role navigation + Win10 Settings See-also
// ---------------------------------------------------------------------------

/// A Dashboard action-link that jumps to a sibling role's landing.
fn role_link<'a>(
    label: &'static str,
    group: Group,
    palette: Palette,
) -> Element<'a, crate::Message> {
    nav_chip(label, palette, crate::Message::SelectGroup(group))
}

/// A Dashboard "See also" link that opens a Win10 Settings page (the
/// shell's `mde settings` app) at `slug` (`""` = the Settings home).
fn settings_link<'a>(
    label: &'static str,
    slug: &'static str,
    palette: Palette,
) -> Element<'a, crate::Message> {
    nav_chip(label, palette, crate::Message::OpenSettings(slug))
}

/// Shared pill-style action-link button (palette-tokened; faint raised
/// tint on hover).
fn nav_chip<'a>(
    label: &'static str,
    palette: Palette,
    msg: crate::Message,
) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    let fg = palette.text.into_iced_color();
    button(text(label).size(13).color(fg))
        .padding(Padding::from([6u16, 12u16]))
        .style(move |_t: &Theme, status: iced::widget::button::Status| {
            let hover_bg = Color {
                r: bg.r * 1.1,
                g: bg.g * 1.1,
                b: bg.b * 1.1,
                a: bg.a,
            };
            iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(match status {
                    iced::widget::button::Status::Hovered
                    | iced::widget::button::Status::Pressed => hover_bg,
                    _ => bg,
                })),
                text_color: fg,
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                shadow: iced::Shadow::default(),
            }
        })
        .on_press(msg)
        .into()
}

// ---------------------------------------------------------------------------
// Probes (OV-4)
// ---------------------------------------------------------------------------

/// Top-level async load. Fires every probe in parallel and
/// builds the full capability list.
pub async fn load_capabilities() -> (Vec<CapabilityRow>, bool) {
    let (
        nebula,
        peers,
        files,
        ssh,
        rdp,
        vnc,
        services,
        phone,
        voice,
        fleet,
        notifications,
        mackesd_ok,
    ) = tokio::join!(
        probe_nebula(),
        probe_peers(),
        probe_files(),
        probe_systemd_unit("sshd.service"),
        probe_systemd_unit("xrdp.service"),
        probe_vnc(),
        probe_mesh_services(),
        probe_phone(),
        probe_voice(),
        probe_fleet_revision(),
        probe_notifications(),
        probe_mackesd_alive(),
    );
    let rows = vec![
        build_mesh_row(&nebula),
        build_peers_row(&peers),
        build_files_row(&files),
        build_ssh_row(&ssh),
        build_rdp_row(&rdp),
        build_vnc_row(&vnc),
        build_services_row(&services),
        build_phone_row(&phone),
        build_voice_row(&voice),
        build_fleet_row(&fleet),
        build_notifications_row(&notifications),
    ];
    (rows, mackesd_ok)
}

/// Re-fire only the probes affected by a given D-Bus event,
/// merging into the existing row list. Lighter than the full
/// fan-out — keeps signal-driven refresh cheap.
pub async fn reprobe_for_event(event: DbusEvent) -> (Vec<CapabilityRow>, bool) {
    // Today: simple — just re-run the full fan-out. The event
    // type stays in the API so a future optimization can do
    // per-id reprobes without touching call sites.
    let _ = event;
    load_capabilities().await
}

// --- Nebula ----------------------------------------------------------------

async fn probe_nebula() -> ProbeOutcome {
    // action/nebula/status returns a JSON dictionary; we only need
    // active_transport for the pill. E0.3.1.a — read it over the
    // mesh Bus instead of the (dual-served, retiring) Nebula.Status
    // D-Bus method. The Bus client spins its own current-thread
    // runtime (Persist isn't Send), so run it via spawn_blocking to
    // keep this future Send for the iced executor.
    let raw = match tokio::task::spawn_blocking(|| crate::dbus::nebula_request("status")).await {
        Ok(Some(s)) => s,
        _ => return ProbeOutcome::unknown(),
    };
    let transport = extract_json_string_field(&raw, "active_transport").unwrap_or_default();
    if transport.is_empty() || transport == "offline" {
        ProbeOutcome::setup_needed(Some("Mesh fabric is not connected".into()))
    } else {
        ProbeOutcome::active(Some(format!(
            "Connected via {}",
            humanize_transport(&transport)
        )))
    }
}

fn humanize_transport(t: &str) -> String {
    match t {
        "nebula_direct" => "direct UDP".into(),
        "nebula_lighthouse_relay" => "lighthouse relay".into(),
        "nebula_https443" => "HTTPS-443 fallback".into(),
        "kdc_tls" => "KDC2 TLS".into(),
        other => other.replace('_', " "),
    }
}

// --- Peers -----------------------------------------------------------------

async fn probe_peers() -> ProbeOutcome {
    // fetch_peers shells out to `mackesd nodes list --json`.
    // Bounce it onto the executor with spawn_blocking so the
    // sync std::process::Command doesn't stall the runtime.
    let peers = tokio::task::spawn_blocking(fetch_peers).await;
    let peers = match peers {
        Ok(Ok(peers)) => peers,
        _ => return ProbeOutcome::unknown(),
    };
    let total = peers.len();
    let online = peers
        .iter()
        .filter(|p| {
            matches!(p.kind.as_str(), "host" | "peer" | "lighthouse")
                && p.addr.as_str().ne("offline")
                && !matches!(format!("{:?}", p.status).as_str(), "Offline" | "Unknown")
        })
        .count();
    if total == 0 {
        return ProbeOutcome::setup_needed(Some("No peers enrolled yet".into()));
    }
    let sub = Some(format!("{online} of {total} peers online"));
    if online == 0 {
        ProbeOutcome {
            status: CapabilityStatus::Failed {
                detail: "All peers offline".into(),
            },
            sub_status: sub,
        }
    } else {
        ProbeOutcome::active(sub)
    }
}

// --- Systemd units ---------------------------------------------------------

async fn probe_systemd_unit(unit: &str) -> ProbeOutcome {
    let state = systemctl_active_state(unit).await;
    match state.as_deref() {
        Some("active") => ProbeOutcome::active(None),
        Some("activating") => ProbeOutcome::setup_needed(Some("Starting…".into())),
        Some("failed") => ProbeOutcome::failed(format!("{unit} failed to start")),
        Some("inactive") => ProbeOutcome::setup_needed(Some(format!("{unit} is stopped"))),
        Some(other) => ProbeOutcome {
            status: CapabilityStatus::Unknown,
            sub_status: Some(format!("state: {other}")),
        },
        None => ProbeOutcome::unknown(),
    }
}

async fn systemctl_active_state(unit: &str) -> Option<String> {
    let out = tokio::process::Command::new("systemctl")
        .args(["show", "-p", "ActiveState", "--value", unit])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() || s == "(null)" {
        None
    } else {
        Some(s)
    }
}

// --- VNC -------------------------------------------------------------------

async fn probe_vnc() -> ProbeOutcome {
    let x11 = systemctl_active_state("x11vnc@:0.service").await;
    let way = systemctl_active_state("wayvnc.service").await;
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    match (x11.as_deref(), way.as_deref(), session_type.as_str()) {
        (Some("active"), _, _) => ProbeOutcome::active(Some("x11vnc serving :0".into())),
        (_, Some("active"), _) => ProbeOutcome::active(Some("wayvnc serving overlay IP".into())),
        (Some("failed"), _, "wayland") => {
            ProbeOutcome::failed("x11vnc does not run under Wayland — see RD-1..RD-5".to_string())
        }
        (Some("failed"), _, _) => ProbeOutcome::failed("x11vnc@:0.service failed".to_string()),
        (Some("inactive"), Some("inactive"), _)
        | (Some("inactive"), None, _)
        | (None, Some("inactive"), _) => {
            ProbeOutcome::setup_needed(Some("No VNC server running".into()))
        }
        (None, None, _) => ProbeOutcome::setup_needed(Some("No VNC server installed".into())),
        _ => ProbeOutcome::unknown(),
    }
}

// --- Mesh services registry -----------------------------------------------

async fn probe_mesh_services() -> ProbeOutcome {
    let mut active = 0usize;
    let total = MESH_UNITS.len();
    for (name, _, _) in MESH_UNITS.iter() {
        if let Some(state) = systemctl_active_state(name).await {
            if state == "active" {
                active += 1;
            }
        }
    }
    let sub = Some(format!("{active} of {total} services running"));
    if active == 0 {
        ProbeOutcome::setup_needed(sub)
    } else if active < total {
        ProbeOutcome {
            status: CapabilityStatus::SetupNeeded,
            sub_status: sub,
        }
    } else {
        ProbeOutcome::active(sub)
    }
}

// --- Fleet -----------------------------------------------------------------

async fn probe_fleet_revision() -> ProbeOutcome {
    // action/fleet/list-revisions over the mesh Bus (E0.3.3, was the
    // dev.mackes.MDE.Fleet D-Bus ListRevisions). The verb is a Phase-G
    // stub today → the reply is an error envelope → no "r-" id → the
    // pill reads "No revisions pushed yet" (extract_first_revision_id
    // finds no token). spawn_blocking: the Bus client spins its own
    // current-thread runtime (Persist isn't Send).
    let raw = match tokio::task::spawn_blocking(|| {
        crate::dbus::action_request(
            "action/fleet/list-revisions",
            std::time::Duration::from_secs(2),
        )
    })
    .await
    {
        Ok(Some(s)) => s,
        _ => return ProbeOutcome::unknown(),
    };
    let latest = extract_first_revision_id(&raw);
    match latest {
        Some(id) => {
            ProbeOutcome::active(Some(format!("Last update {}", humanize_revision_age(&id))))
        }
        None => ProbeOutcome::setup_needed(Some("No revisions pushed yet".into())),
    }
}

fn extract_first_revision_id(raw: &str) -> Option<String> {
    // Match the first "r-YYYY-MM-DD-NNNN" token in the raw
    // dbus-send output. dbus-send's text format is more
    // permissive than JSON so a quick token scan is the
    // most resilient parse.
    for tok in
        raw.split(|c: char| c == '"' || c == ' ' || c == '\n' || c == ',' || c == '[' || c == ']')
    {
        if tok.starts_with("r-") && tok.len() >= 12 {
            return Some(tok.to_string());
        }
    }
    None
}

fn humanize_revision_age(id: &str) -> String {
    // r-YYYY-MM-DD-NNNN — show the YYYY-MM-DD portion.
    if let Some(date) = id.strip_prefix("r-").and_then(|s| s.get(..10)) {
        date.to_string()
    } else {
        id.to_string()
    }
}

// --- Notifications ---------------------------------------------------------

async fn probe_notifications() -> ProbeOutcome {
    match dbus_call(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "GetServerInformation",
    )
    .await
    {
        Ok(raw) => {
            // Reply has 4 strings: name, vendor, version,
            // spec_version. Pluck name + version for sub-status.
            let name = extract_dbus_string_at(&raw, 0).unwrap_or_else(|| "notifications".into());
            let version = extract_dbus_string_at(&raw, 2).unwrap_or_default();
            let sub = if version.is_empty() {
                Some(format!("Daemon: {name}"))
            } else {
                Some(format!("Daemon: {name} {version}"))
            };
            ProbeOutcome::active(sub)
        }
        Err(_) => ProbeOutcome::setup_needed(Some("No notification daemon registered".into())),
    }
}

// --- File Sharing (MeshFS / LizardFS) --------------------------------------

async fn probe_files() -> ProbeOutcome {
    // Same source as the Mesh Storage panel: `mackesd meshfs-status --json`.
    // `fetch_status` is a sync std::process::Command, so bounce it onto the
    // executor with spawn_blocking. Err => mackesd absent/unreachable
    // (Unknown); Ok with no peers => master up but no chunkservers online
    // yet (SetupNeeded); Ok with peers => serving (Active).
    let status = tokio::task::spawn_blocking(crate::panels::mesh_storage::fetch_status).await;
    match status {
        Ok(Ok(s)) => {
            if s.peers.is_empty() {
                ProbeOutcome::setup_needed(Some(
                    "Mesh storage not active yet — no chunkservers online".into(),
                ))
            } else {
                let n = s.peers.len();
                ProbeOutcome::active(Some(format!(
                    "{n} chunkserver{} · replication goal {}",
                    if n == 1 { "" } else { "s" },
                    s.goal,
                )))
            }
        }
        _ => ProbeOutcome::unknown(),
    }
}

// --- Mesh peer (phone) — KDE Connect host ----------------------------------

/// Session-bus well-known name the KDE Connect host owns while running.
/// MUST match the name the KDC2 host registers + the surface the
/// `connect` ("Connected Devices") panel reads.
const KDC_BUS_NAME: &str = "dev.mackes.MDE.Connect";

async fn probe_phone() -> ProbeOutcome {
    // Reflect KDE Connect host presence honestly via the session bus —
    // the host owns `dev.mackes.MDE.Connect` while running. Owned =>
    // ready to pair (Active); not owned => host not up (SetupNeeded);
    // dbus-send unavailable => Unknown.
    match dbus_name_has_owner(KDC_BUS_NAME).await {
        Some(true) => ProbeOutcome::active(Some(
            "KDE Connect host running — pair a phone to mirror notifications, SMS & clipboard"
                .into(),
        )),
        Some(false) => ProbeOutcome::setup_needed(Some(
            "KDE Connect host not running — no phone paired yet".into(),
        )),
        None => ProbeOutcome::unknown(),
    }
}

/// Ask the session bus' `org.freedesktop.DBus.NameHasOwner` whether
/// `name` is currently owned. `None` when dbus-send is unavailable or
/// the reply is unparseable.
async fn dbus_name_has_owner(name: &str) -> Option<bool> {
    let out = tokio::process::Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--type=method_call",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.NameHasOwner",
            &format!("string:{name}"),
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    if s.contains("boolean true") {
        Some(true)
    } else if s.contains("boolean false") {
        Some(false)
    } else {
        None
    }
}

// --- Voice & Video — SIP softphone agent -----------------------------------

/// Bus topic the `mde-voice-hud --agent` publishes its retained status to.
/// MUST match `mde_voice_hud::sip::VOICE_STATUS_TOPIC`.
const VOICE_STATUS_TOPIC: &str = "state/voice/status";

/// Reader-side staleness window — a heartbeat older than this means the
/// agent stopped publishing (i.e. is not running). Mirrors the shell's
/// `birthright::VOICE_STALE_SECS`.
const VOICE_STALE_SECS: u64 = 45;

async fn probe_voice() -> ProbeOutcome {
    // Mirror the shell's birthright Voice probe: read the retained
    // `state/voice/status` heartbeat off the mesh Bus. The Persist client
    // spins its own current-thread runtime (rusqlite isn't Send), so read
    // it via spawn_blocking to keep this future Send for the iced executor.
    let body = tokio::task::spawn_blocking(read_voice_status)
        .await
        .ok()
        .flatten();
    parse_voice_outcome(body, now_unix())
}

/// Latest retained body on `state/voice/status`, or `None` when the topic
/// is empty / the Bus is unavailable.
fn read_voice_status() -> Option<String> {
    let dir = mde_bus::default_data_dir()?;
    let persist = mde_bus::persist::Persist::open(dir).ok()?;
    let msgs = persist.list_since(VOICE_STATUS_TOPIC, None).ok()?;
    msgs.last().and_then(|m| m.body.clone())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Decode the voice agent's `{registered, listening, server, detail, ts}`
/// heartbeat into a pill. A stale or absent heartbeat reads as
/// "agent not running" (SetupNeeded); `registered + listening` is Active;
/// anything in between is SetupNeeded with the agent's own detail string.
fn parse_voice_outcome(body: Option<String>, now: u64) -> ProbeOutcome {
    let Some(body) = body else {
        return ProbeOutcome::setup_needed(Some(
            "Voice agent not running — no SIP status on the Bus".into(),
        ));
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return ProbeOutcome::unknown();
    };
    let ts = v.get("ts").and_then(serde_json::Value::as_u64).unwrap_or(0);
    if now.saturating_sub(ts) > VOICE_STALE_SECS {
        return ProbeOutcome::setup_needed(Some(
            "Voice agent stopped publishing — not running".into(),
        ));
    }
    let registered = v
        .get("registered")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let listening = v
        .get("listening")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let detail = v.get("detail").and_then(|x| x.as_str()).unwrap_or("");
    let server = v.get("server").and_then(|x| x.as_str()).unwrap_or("");
    if registered && listening {
        ProbeOutcome::active(Some(format!(
            "Registered · {server} — listening for inbound calls"
        )))
    } else if listening {
        ProbeOutcome::setup_needed(Some(format!("Listening, but not registered ({detail})")))
    } else if detail.is_empty() {
        ProbeOutcome::setup_needed(Some("Not registered — add a SIP account".into()))
    } else {
        ProbeOutcome::setup_needed(Some(format!("Not registered ({detail})")))
    }
}

// --- mackesd health --------------------------------------------------------

async fn probe_mackesd_alive() -> bool {
    // E0.3.5 — mackesd liveness = its Shell Bus responder answers
    // action/shell/healthz (replacing the retired dev.mackes.MDE.Shell
    // D-Bus Healthz probe). spawn_blocking: the Bus client spins its
    // own current-thread runtime (Persist isn't Send).
    tokio::task::spawn_blocking(|| {
        crate::dbus::action_request("action/shell/healthz", std::time::Duration::from_secs(2))
    })
    .await
    .ok()
    .flatten()
    .is_some()
}

// --- D-Bus shell-out -------------------------------------------------------

async fn dbus_call(
    destination: &str,
    object_path: &str,
    interface: &str,
    method: &str,
) -> Result<String, String> {
    let out = tokio::process::Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply=literal",
            "--type=method_call",
            &format!("--dest={destination}"),
            object_path,
            &format!("{interface}.{method}"),
        ])
        .output()
        .await
        .map_err(|e| format!("dbus-send spawn: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    // Some surfaces (notifications) live on the session bus by
    // default but mackesd's interfaces may also be reachable via
    // the system bus depending on operator setup. Try the system
    // bus before giving up.
    let out = tokio::process::Command::new("dbus-send")
        .args([
            "--system",
            "--print-reply=literal",
            "--type=method_call",
            &format!("--dest={destination}"),
            object_path,
            &format!("{interface}.{method}"),
        ])
        .output()
        .await
        .map_err(|e| format!("dbus-send spawn: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn extract_json_string_field(raw: &str, field: &str) -> Option<String> {
    // dbus-send --print-reply=literal returns the value plain.
    // mackesd's Status() returns a JSON blob, so grep for
    // "<field>":"<value>" or "<field>": "<value>".
    let needle = format!("\"{field}\"");
    let idx = raw.find(&needle)?;
    let after = &raw[idx + needle.len()..];
    let after = after.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    if let Some(rest) = after.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    } else {
        // Bare token (number/identifier).
        let end = after
            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .unwrap_or(after.len());
        Some(after[..end].to_string())
    }
}

fn extract_dbus_string_at(raw: &str, idx: usize) -> Option<String> {
    // dbus-send literal output puts each string on its own line
    // (or whitespace-separated). Iterate quoted tokens; bare
    // strings appear unquoted in literal mode.
    let mut tokens = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Literal mode prints `string "name"` for variants, just
        // `name` for plain string args. Handle both.
        if let Some(stripped) = line.strip_prefix("string ") {
            tokens.push(stripped.trim().trim_matches('"').to_string());
        } else if line.starts_with('"') && line.ends_with('"') && line.len() >= 2 {
            tokens.push(line[1..line.len() - 1].to_string());
        } else {
            tokens.push(line.to_string());
        }
    }
    tokens.get(idx).cloned()
}

// ---------------------------------------------------------------------------
// Row builders (OV-5)
// ---------------------------------------------------------------------------

fn build_mesh_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Mesh,
        name: "Mesh Network",
        description: "Encrypted private network between every peer.",
        icon: Icon::Network,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Network, "mesh_control")),
        launch: None,
    }
}

fn build_peers_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Peers,
        name: "Peer Reachability",
        description: "Which of your devices are online right now.",
        icon: Icon::Peer,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Network, "mesh_topology")),
        launch: None,
    }
}

fn build_files_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Files,
        name: "File Sharing",
        description: "Every peer holds every file in your shared folders.",
        icon: Icon::Files,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Network, "mesh_storage")),
        launch: None,
    }
}

fn build_ssh_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Ssh,
        name: "SSH Across Mesh",
        description: "Open a terminal on any peer from any other peer.",
        icon: Icon::System,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Network, "mesh_ssh")),
        launch: None,
    }
}

fn build_rdp_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Rdp,
        name: "Remote Desktop (RDP)",
        description: "See and control any peer's screen with an RDP client.",
        icon: Icon::Display,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Network, "remote_desktop")),
        launch: None,
    }
}

fn build_vnc_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Vnc,
        name: "Remote Desktop (VNC)",
        description: "See and control any peer's screen with a VNC client.",
        icon: Icon::Display,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Network, "remote_desktop")),
        launch: None,
    }
}

fn build_services_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Services,
        name: "Media & App Discovery",
        description: "Find and open services running on other peers in one click.",
        icon: Icon::Apps,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Network, "mesh_services")),
        launch: None,
    }
}

fn build_phone_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Phone,
        name: "Mesh peer (phone)",
        description: "Add a phone to your mesh to mirror notifications, SMS, and clipboard.",
        icon: Icon::Devices,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Devices, "connect")),
        launch: None,
    }
}

fn build_voice_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Voice,
        name: "Voice & Video",
        description: "Call any peer or any phone number from anywhere on the mesh.",
        icon: Icon::Sound,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        // No Workbench panel — voice/SIP config is the standalone
        // `mde-voice-config` app (same surface the shell's birthright
        // OpenVoice fix launches).
        jump: None,
        launch: Some("mde-voice-config"),
    }
}

fn build_fleet_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Fleet,
        name: "Fleet Configuration",
        description: "Push the same settings to every peer at once.",
        icon: Icon::Fleet,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::Fleet, "playbooks")),
        launch: None,
    }
}

fn build_notifications_row(p: &ProbeOutcome) -> CapabilityRow {
    CapabilityRow {
        id: CapabilityId::Notifications,
        name: "Desktop Notifications",
        description: "App notifications that work across every peer.",
        icon: Icon::Notification,
        status: p.status.clone(),
        sub_status: p.sub_status.clone(),
        jump: Some((Group::System, "notifications")),
        launch: None,
    }
}

/// For tests and Overview consumers that want the literal row
/// ordering without firing any probes.
#[must_use]
pub fn build_all_rows_with_unknown_status() -> Vec<CapabilityRow> {
    vec![
        build_mesh_row(&ProbeOutcome::unknown()),
        build_peers_row(&ProbeOutcome::unknown()),
        build_files_row(&ProbeOutcome::unknown()),
        build_ssh_row(&ProbeOutcome::unknown()),
        build_rdp_row(&ProbeOutcome::unknown()),
        build_vnc_row(&ProbeOutcome::unknown()),
        build_services_row(&ProbeOutcome::unknown()),
        build_phone_row(&ProbeOutcome::unknown()),
        build_voice_row(&ProbeOutcome::unknown()),
        build_fleet_row(&ProbeOutcome::unknown()),
        build_notifications_row(&ProbeOutcome::unknown()),
    ]
}

// ---------------------------------------------------------------------------
// Widgets (OV-6 + OV-10)
// ---------------------------------------------------------------------------

fn icon_widget<'a>(icon: Icon, size: IconSize, color: Color) -> Element<'a, crate::Message> {
    let resolved = mde_icon(icon, size);
    if let Some(svg_bytes) = resolved.svg_bytes() {
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(resolved.size_px()))
            .height(Length::Fixed(resolved.size_px()))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style { color: Some(color) },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(resolved.size_px())
            .color(color)
            .into()
    }
}

fn capability_card<'a>(
    row_data: &'a CapabilityRow,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let icon = icon_widget(
        row_data.icon,
        IconSize::PanelHeader,
        palette.text.into_iced_color(),
    );
    let name = text(row_data.name)
        .size(16)
        .color(palette.text.into_iced_color());
    let description = text(row_data.description)
        .size(13)
        .color(palette.text_muted.into_iced_color());
    let sub_status: Element<'_, crate::Message> = match row_data
        .status
        .detail()
        .map(str::to_string)
        .or_else(|| row_data.sub_status.clone())
    {
        Some(s) => text(s)
            .size(12)
            .color(palette.text_muted.into_iced_color())
            .into(),
        None => Space::new().height(Length::Fixed(0.0)).into(),
    };

    let pill = status_pill(&row_data.status, palette);
    let jump = jump_button(row_data, palette);

    let top_row = row![
        icon,
        Space::new().width(Length::Fixed(12.0)),
        column![name, description].spacing(2),
        Space::new().width(Length::Fill),
        pill,
    ]
    .align_y(iced::alignment::Vertical::Center);

    let bottom_row = row![sub_status, Space::new().width(Length::Fill), jump]
        .align_y(iced::alignment::Vertical::Center);

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(column![top_row, Space::new().height(Length::Fixed(8.0)), bottom_row].spacing(0))
        .padding(Padding::from([16u16, 16u16]))
        .width(Length::Fill)
        .style(move |_t: &Theme| iced::widget::container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn status_pill(status: &CapabilityStatus, _palette: Palette) -> Element<'_, crate::Message> {
    let color = status.color();
    let label = status.label();
    row![
        icon_widget(status.icon(), IconSize::Inline, color),
        Space::new().width(Length::Fixed(4.0)),
        text(label).size(12).color(color),
    ]
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

fn jump_button<'a>(row_data: &'a CapabilityRow, palette: Palette) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    let text_color = palette.text.into_iced_color();
    let style = move |_t: &Theme, status: iced::widget::button::Status| {
        let hover_bg = Color {
            r: bg.r * 1.12,
            g: bg.g * 1.12,
            b: bg.b * 1.12,
            a: bg.a,
        };
        iced::widget::button::Style {
            snap: false,
            background: Some(Background::Color(match status {
                iced::widget::button::Status::Hovered => hover_bg,
                _ => bg,
            })),
            text_color,
            border: Border {
                color: border,
                width: 1.0,
                radius: 6.0.into(),
            },
            shadow: iced::Shadow::default(),
        }
    };

    if let Some((group, panel)) = row_data.jump {
        button(text("Configure  ▸").size(13))
            .padding(Padding::from([6u16, 14u16]))
            .style(style)
            .on_press(crate::Message::SelectPanel { group, panel })
            .into()
    } else if let Some(bin) = row_data.launch {
        button(text("Set up  ▸").size(13))
            .padding(Padding::from([6u16, 14u16]))
            .style(style)
            .on_press(crate::Message::LaunchApp(bin))
            .into()
    } else {
        column![].into()
    }
}

fn refresh_button<'a>(palette: Palette) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    let text_color = palette.text.into_iced_color();
    button(text("Refresh").size(12))
        .padding(Padding::from([4u16, 12u16]))
        .style(move |_t: &Theme, status: iced::widget::button::Status| {
            let hover_bg = Color {
                r: bg.r * 1.12,
                g: bg.g * 1.12,
                b: bg.b * 1.12,
                a: bg.a,
            };
            iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(match status {
                    iced::widget::button::Status::Hovered => hover_bg,
                    _ => bg,
                })),
                text_color,
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                shadow: iced::Shadow::default(),
            }
        })
        .on_press(crate::Message::Home(Message::RefreshClicked))
        .into()
}

fn mackesd_banner<'a>(palette: Palette) -> Element<'a, crate::Message> {
    let yellow = Color::from_rgb(0.95, 0.70, 0.20);
    let bg = palette.raised.into_iced_color();
    container(
        row![
            icon_widget(Icon::StatusWarning, IconSize::PanelHeader, yellow),
            Space::new().width(Length::Fixed(8.0)),
            text("mackesd is not responding — capability statuses may be stale.")
                .size(13)
                .color(palette.text.into_iced_color()),
        ]
        .align_y(iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([10u16, 14u16]))
    .width(Length::Fill)
    .style(move |_t: &Theme| iced::widget::container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: yellow,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    })
    .into()
}

// ---------------------------------------------------------------------------
// Stat card (existing — preserved)
// ---------------------------------------------------------------------------

fn stat_card<'a>(
    label: &'a str,
    value: Option<u32>,
    icon: Icon,
    target_group: Group,
    target_panel: &'a str,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let icon = icon_widget(
        icon,
        IconSize::PanelHeader,
        palette.text_muted.into_iced_color(),
    );
    let value_display = match value {
        Some(n) => n.to_string(),
        None => "—".into(),
    };
    let value_text = text(value_display)
        .size(28)
        .color(palette.text.into_iced_color());
    let label_text = text(label.to_string())
        .size(12)
        .color(palette.text_muted.into_iced_color());
    let card_panel_slug: &'static str = match target_panel {
        "snapshots" => "snapshots",
        "drift" => "drift",
        "inventory" => "inventory",
        _ => "snapshots",
    };
    let card = column![
        icon,
        Space::new().height(Length::Fixed(4.0)),
        value_text,
        Space::new().height(Length::Fixed(2.0)),
        label_text,
    ]
    .spacing(0)
    .align_x(iced::alignment::Horizontal::Left);

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    let muted_text = palette.text_muted.into_iced_color();
    button(card)
        .width(Length::Fill)
        .padding(Padding::from([16u16, 16u16]))
        .style(move |_t: &Theme, status: iced::widget::button::Status| {
            let hover_bg = Color {
                r: bg.r * 1.08,
                g: bg.g * 1.08,
                b: bg.b * 1.08,
                a: bg.a,
            };
            iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(match status {
                    iced::widget::button::Status::Hovered => hover_bg,
                    _ => bg,
                })),
                text_color: muted_text,
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                shadow: iced::Shadow::default(),
            }
        })
        .on_press(crate::Message::SelectPanel {
            group: target_group,
            panel: card_panel_slug,
        })
        .into()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_peer_count(row: &CapabilityRow) -> Option<u32> {
    let sub = row.sub_status.as_ref()?;
    // sub_status format: "X of Y peers online" — extract Y.
    let mut tokens = sub.split_whitespace();
    let _x = tokens.next()?;
    let _of = tokens.next()?;
    let total = tokens.next()?;
    total.parse::<u32>().ok()
}

// ---------------------------------------------------------------------------
// D-Bus signal subscription (OV-8)
// ---------------------------------------------------------------------------

/// Iced subscription that bridges live D-Bus signals from
/// `dev.mackes.MDE.Fleet` (RevisionApplied) into
/// `Message::Home(DbusEvent(...))`. The Overview re-fires its
/// probe fan-out on each event, so status pills flip without
/// the operator hitting Refresh.
///
/// E0.3.1.b — the `dev.mackes.MDE.Nebula.Status` signals
/// (PeerStateChanged / TransportChanged / EnrollmentCompleted) no
/// longer arrive here; they moved to the mesh Bus event topic,
/// polled by [`nebula_event_subscription`]. Fleet stays on D-Bus
/// until E0.3.3.
///
/// systemd1 per-unit `PropertiesChanged` (OV-8.a, shipped
/// 2026-05-25) is also subscribed: the loop calls
/// `org.freedesktop.systemd1.Manager.Subscribe()` once at
/// connection time + matches `PropertiesChanged` on every
/// `/org/freedesktop/systemd1/unit/<escaped-name>` path. Only
/// units in [`systemd_watch_list`] propagate to the Overview;
/// the rest fan out and are dropped silently.
///
/// On connection loss (mackesd restart, bus disconnect) the
/// loop re-establishes with a 5 s backoff so the Overview
/// resumes live updates without a Workbench relaunch.
pub fn dbus_subscription() -> iced::Subscription<crate::Message> {
    use iced::stream;
    iced::Subscription::run(|| {
        stream::channel(32, |mut output| async move {
            loop {
                if let Err(e) = run_subscription(&mut output).await {
                    tracing::warn!(
                        target: "mde_workbench::home::dbus_subscription",
                        "subscription dropped: {e}; reconnecting in 5s",
                    );
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        })
    })
}

/// Bus event topic the mackesd signal dispatcher fans the Nebula
/// signals out on (E0.3.1.b). MUST match mackesd's
/// `ipc::nebula::NEBULA_EVENT_TOPIC` literal.
const NEBULA_EVENT_TOPIC: &str = "event/nebula/signals";

/// E0.3.1.b — poll the Bus `event/nebula/signals` topic for the
/// Nebula signals that used to arrive as `dev.mackes.MDE.Nebula.
/// Status` D-Bus signals, mapping each to the same [`DbusEvent`]
/// the Overview reprobes on. This is the subscribe side of the
/// signal migration; `dbus_subscription` keeps only Fleet + systemd.
///
/// The cursor starts at the latest existing event so only NEW
/// signals trigger reprobes (matching the fire-and-forget D-Bus
/// semantics — a fresh subscriber doesn't replay history). Each
/// poll opens a short-lived `Persist` inside `spawn_blocking`
/// (rusqlite isn't `Send`, so it can't cross the executor's await
/// points).
pub fn nebula_event_subscription() -> iced::Subscription<crate::Message> {
    use iced::futures::SinkExt;
    use iced::stream;
    iced::Subscription::run(|| {
        stream::channel(
            32,
            |mut output: iced::futures::channel::mpsc::Sender<crate::Message>| async move {
                let mut cursor = nebula_event_cursor_init().await;
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
                    let (events, next) = nebula_poll_events(cursor.clone()).await;
                    cursor = next;
                    for ev in events {
                        let _ = output
                            .send(crate::Message::Home(Message::DbusEvent(ev)))
                            .await;
                    }
                }
            },
        )
    })
}

/// Resolve the latest existing `event/nebula/signals` ulid so the
/// subscription only reacts to events written AFTER it starts.
/// `None` when the topic is empty / the Bus is unavailable (then
/// the first poll picks up everything from the start, which is
/// still "new since we started").
async fn nebula_event_cursor_init() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let dir = mde_bus::default_data_dir()?;
        let persist = mde_bus::persist::Persist::open(dir).ok()?;
        let msgs = persist.list_since(NEBULA_EVENT_TOPIC, None).ok()?;
        msgs.last().map(|m| m.ulid.clone())
    })
    .await
    .ok()
    .flatten()
}

/// Poll the event topic since `cursor`; return the decoded
/// [`DbusEvent`]s + the advanced cursor. On a join error the cursor
/// is preserved (so the next poll doesn't replay from the start).
async fn nebula_poll_events(cursor: Option<String>) -> (Vec<DbusEvent>, Option<String>) {
    let fallback = cursor.clone();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = mde_bus::default_data_dir() else {
            return (Vec::new(), cursor);
        };
        let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
            return (Vec::new(), cursor);
        };
        let msgs = persist
            .list_since(NEBULA_EVENT_TOPIC, cursor.as_deref())
            .unwrap_or_default();
        let mut next = cursor;
        let mut events = Vec::new();
        for m in msgs {
            next = Some(m.ulid);
            if let Some(body) = m.body {
                if let Some(ev) = nebula_event_from_body(&body) {
                    events.push(ev);
                }
            }
        }
        (events, next)
    })
    .await
    .unwrap_or((Vec::new(), fallback))
}

/// Decode one `event/nebula/signals` body (written by mackesd's
/// `ipc::nebula::signal_event_body`) into the matching
/// [`DbusEvent`]. `PeerStateChanged` + `EnrollmentCompleted` both
/// map to `PeerChanged` (same as the old D-Bus dispatch). Unknown
/// or malformed bodies yield `None`.
#[must_use]
pub fn nebula_event_from_body(body: &str) -> Option<DbusEvent> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    match v.get("kind")?.as_str()? {
        "peer-state-changed" | "enrollment-completed" => Some(DbusEvent::PeerChanged),
        "transport-changed" => Some(DbusEvent::TransportChanged),
        _ => None,
    }
}

/// systemd unit names the OV-8.a subscription cares about.
/// Anything outside this list still fans out from the bus but
/// is dropped before turning into a `DbusEvent::UnitChanged`.
/// Built from the SSH / RDP / VNC slots plus every
/// `MESH_UNITS` entry so the per-row probes refresh on any
/// state flip.
#[must_use]
pub fn systemd_watch_list() -> Vec<String> {
    let mut v: Vec<String> = vec![
        "sshd.service".into(),
        "xrdp.service".into(),
        "x11vnc@:0.service".into(),
        "wayvnc.service".into(),
    ];
    for (name, _, _) in MESH_UNITS.iter() {
        v.push((*name).to_string());
    }
    v
}

async fn run_subscription(
    output: &mut iced::futures::channel::mpsc::Sender<crate::Message>,
) -> Result<(), String> {
    use iced::futures::SinkExt;
    use zbus::MatchRule;
    use zbus::MessageStream;

    let conn = zbus::Connection::session()
        .await
        .map_err(|e| format!("session bus connect: {e}"))?;

    let bus_proxy = zbus::fdo::DBusProxy::new(&conn)
        .await
        .map_err(|e| format!("DBus proxy: {e}"))?;

    // E0.3.1.b + E0.3.3 — all MDE-internal D-Bus signals are retired:
    // Nebula's moved to the Bus (event/nebula/signals, polled by
    // `nebula_event_subscription`) and Fleet's RevisionApplied retired
    // with the dev.mackes.MDE.Fleet surface (Phase G re-adds it as a
    // Bus event). The only signal this subscription still bridges is
    // systemd1 PropertiesChanged (FDO interop, kept), set up below.

    // ---- systemd1 PropertiesChanged subscription (OV-8.a) ---
    // Manager.Subscribe() is the prereq — without it systemd
    // does not emit per-unit signals to the session bus. The
    // call is idempotent on the systemd side; safe to retry
    // on every reconnect.
    if let Err(e) = subscribe_to_systemd(&conn).await {
        tracing::warn!(
            target: "mde_workbench::home::dbus_subscription",
            "systemd Manager.Subscribe failed: {e}; unit refresh stays manual",
        );
    }
    let watch = systemd_watch_list();
    let systemd_rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.systemd1")
        .map_err(|e| format!("rule systemd sender: {e}"))?
        .interface("org.freedesktop.DBus.Properties")
        .map_err(|e| format!("rule properties iface: {e}"))?
        .member("PropertiesChanged")
        .map_err(|e| format!("rule propchanged member: {e}"))?
        .build();
    bus_proxy
        .add_match_rule(systemd_rule)
        .await
        .map_err(|e| format!("add_match_rule systemd PropertiesChanged: {e}"))?;

    let stream = MessageStream::from(&conn);
    use iced::futures::StreamExt;
    let mut stream = stream;
    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => return Err(format!("message stream: {e}")),
        };
        let header = msg.header();
        let Some(member) = header.member() else {
            continue;
        };
        let Some(iface) = header.interface() else {
            continue;
        };
        let iface_str = iface.as_str();
        let member_str = member.as_str();

        // systemd1 PropertiesChanged dispatch (OV-8.a).
        if iface_str == "org.freedesktop.DBus.Properties" && member_str == "PropertiesChanged" {
            let Some(path) = header.path() else { continue };
            let path_str = path.as_str();
            let Some(unit) = unit_name_from_path(path_str) else {
                continue;
            };
            if watch.iter().any(|w| w == &unit) {
                let _ = output
                    .send(crate::Message::Home(Message::DbusEvent(
                        DbusEvent::UnitChanged(unit),
                    )))
                    .await;
            }
            continue;
        }
        // E0.3.3 — the Fleet RevisionApplied rule retired with the
        // dev.mackes.MDE.Fleet D-Bus surface; the only MDE-internal
        // signals (Nebula's) already moved to the Bus, so this stream
        // now carries ONLY systemd PropertiesChanged. Anything else
        // falls through and is ignored.
    }
    Err("message stream ended".to_string())
}

/// Call `org.freedesktop.systemd1.Manager.Subscribe()` over a
/// raw method-call message. Required prereq for systemd to
/// emit per-unit `PropertiesChanged` signals on this bus
/// connection. Idempotent on the systemd side.
async fn subscribe_to_systemd(conn: &zbus::Connection) -> Result<(), String> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(|e| format!("systemd1 manager proxy: {e}"))?;
    proxy
        .call_method("Subscribe", &())
        .await
        .map(|_| ())
        .map_err(|e| format!("Manager.Subscribe: {e}"))
}

/// Decode a systemd1 unit-object path back to the canonical
/// unit name. Returns `None` for paths outside the
/// `/org/freedesktop/systemd1/unit/` prefix.
///
/// systemd escape convention: each non-`[A-Za-z0-9_]` byte is
/// encoded as `_xx` (lowercase hex). So `sshd.service` →
/// `sshd_2eservice`, `x11vnc@:0.service` →
/// `x11vnc_40_3a0_2eservice`.
#[must_use]
pub fn unit_name_from_path(path: &str) -> Option<String> {
    let basename = path.strip_prefix("/org/freedesktop/systemd1/unit/")?;
    let mut out = String::with_capacity(basename.len());
    let bytes = basename.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'_' && i + 2 < bytes.len() {
            let h1 = (bytes[i + 1] as char).to_digit(16);
            let h2 = (bytes[i + 2] as char).to_digit(16);
            if let (Some(a), Some(b)) = (h1, h2) {
                let byte = u8::try_from(a * 16 + b).unwrap_or(b'?');
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_load_sync_populates_identity() {
        let s = HomeSnapshot::load_sync();
        assert!(!s.mde_version.is_empty());
        assert!(!s.hostname.is_empty());
        assert!(!s.fedora_release.is_empty());
        assert!(
            s.capabilities.is_empty(),
            "sync load defers capability rows"
        );
        assert!(
            s.mackesd_reachable,
            "default assumes reachable until probed"
        );
    }

    #[test]
    fn view_renders_without_panic() {
        let panel = HomePanel::new();
        let _ = panel.view();
    }

    #[test]
    fn view_renders_with_capabilities() {
        let mut panel = HomePanel::new();
        panel.snapshot.capabilities = build_all_rows_with_unknown_status();
        panel.snapshot.mackesd_reachable = false;
        let _ = panel.view();
    }

    #[test]
    fn capability_status_active_is_green() {
        let s = CapabilityStatus::Active;
        let c = s.color();
        assert!(c.g > c.r && c.g > c.b, "active pill must read as green");
        assert_eq!(s.label(), "Active");
        assert_eq!(s.icon(), Icon::StatusOk);
    }

    #[test]
    fn capability_status_setup_needed_is_yellow() {
        let s = CapabilityStatus::SetupNeeded;
        let c = s.color();
        assert!(c.r > 0.5 && c.g > 0.5 && c.b < 0.5, "yellow pill");
        assert_eq!(s.label(), "Setup needed");
    }

    #[test]
    fn capability_status_failed_is_red_with_detail() {
        let s = CapabilityStatus::Failed {
            detail: "x11vnc dead".into(),
        };
        let c = s.color();
        assert!(c.r > c.g && c.r > c.b, "failed pill must read as red");
        assert_eq!(s.label(), "Failed");
        assert_eq!(s.detail(), Some("x11vnc dead"));
    }

    #[test]
    fn formerly_coming_soon_rows_are_now_actionable() {
        // File Sharing / phone / Voice used to render a static
        // "Coming in vX" pill with no action; they are now live rows.
        // Files + phone deep-link to a Workbench panel; Voice launches
        // the standalone mde-voice-config app.
        let rows = build_all_rows_with_unknown_status();
        let files = rows.iter().find(|r| r.id == CapabilityId::Files).unwrap();
        let phone = rows.iter().find(|r| r.id == CapabilityId::Phone).unwrap();
        let voice = rows.iter().find(|r| r.id == CapabilityId::Voice).unwrap();
        assert_eq!(files.jump, Some((Group::Network, "mesh_storage")));
        assert!(files.launch.is_none());
        assert_eq!(phone.jump, Some((Group::Devices, "connect")));
        assert!(phone.launch.is_none());
        assert!(voice.jump.is_none());
        assert_eq!(voice.launch, Some("mde-voice-config"));
    }

    #[test]
    fn parse_voice_outcome_maps_agent_heartbeat() {
        // Fresh registered+listening heartbeat => Active.
        let body = r#"{"registered":true,"listening":true,"server":"sip.example:5060","detail":"","ts":1000}"#;
        let out = parse_voice_outcome(Some(body.into()), 1010);
        assert_eq!(out.status, CapabilityStatus::Active);
        // Stale heartbeat (older than the window) => SetupNeeded.
        let out = parse_voice_outcome(Some(body.into()), 1000 + VOICE_STALE_SECS + 1);
        assert_eq!(out.status, CapabilityStatus::SetupNeeded);
        // No body on the Bus => agent not running => SetupNeeded.
        assert_eq!(
            parse_voice_outcome(None, 0).status,
            CapabilityStatus::SetupNeeded
        );
        // Garbage body => Unknown.
        assert_eq!(
            parse_voice_outcome(Some("not json".into()), 0).status,
            CapabilityStatus::Unknown
        );
    }

    #[test]
    fn row_ordering_matches_spec() {
        let rows = build_all_rows_with_unknown_status();
        let order: Vec<CapabilityId> = rows.iter().map(|r| r.id).collect();
        assert_eq!(
            order,
            vec![
                CapabilityId::Mesh,
                CapabilityId::Peers,
                CapabilityId::Files,
                CapabilityId::Ssh,
                CapabilityId::Rdp,
                CapabilityId::Vnc,
                CapabilityId::Services,
                CapabilityId::Phone,
                CapabilityId::Voice,
                CapabilityId::Fleet,
                CapabilityId::Notifications,
            ]
        );
    }

    #[test]
    fn row_count_is_eleven() {
        assert_eq!(build_all_rows_with_unknown_status().len(), 11);
    }

    #[test]
    fn live_rows_jump_to_documented_panels() {
        let rows = build_all_rows_with_unknown_status();
        let lookup = |id: CapabilityId| rows.iter().find(|r| r.id == id).and_then(|r| r.jump);
        assert_eq!(
            lookup(CapabilityId::Mesh),
            Some((Group::Network, "mesh_control"))
        );
        assert_eq!(
            lookup(CapabilityId::Peers),
            Some((Group::Network, "mesh_topology"))
        );
        assert_eq!(
            lookup(CapabilityId::Files),
            Some((Group::Network, "mesh_storage"))
        );
        assert_eq!(
            lookup(CapabilityId::Phone),
            Some((Group::Devices, "connect"))
        );
        assert_eq!(
            lookup(CapabilityId::Ssh),
            Some((Group::Network, "mesh_ssh"))
        );
        assert_eq!(
            lookup(CapabilityId::Rdp),
            Some((Group::Network, "remote_desktop"))
        );
        assert_eq!(
            lookup(CapabilityId::Vnc),
            Some((Group::Network, "remote_desktop"))
        );
        assert_eq!(
            lookup(CapabilityId::Services),
            Some((Group::Network, "mesh_services"))
        );
        assert_eq!(
            lookup(CapabilityId::Fleet),
            Some((Group::Fleet, "playbooks"))
        );
        assert_eq!(
            lookup(CapabilityId::Notifications),
            Some((Group::System, "notifications"))
        );
    }

    #[test]
    fn extract_first_revision_id_picks_first_token() {
        let raw = "array [\n  string \"r-2026-05-24-0042\"\n  string \"r-2026-05-23-0017\"\n]";
        assert_eq!(
            extract_first_revision_id(raw),
            Some("r-2026-05-24-0042".into())
        );
    }

    #[test]
    fn extract_first_revision_id_handles_empty_array() {
        assert_eq!(extract_first_revision_id("array [\n]"), None);
    }

    #[test]
    fn humanize_revision_age_extracts_date() {
        assert_eq!(humanize_revision_age("r-2026-05-24-0042"), "2026-05-24");
        assert_eq!(humanize_revision_age("garbage"), "garbage");
    }

    #[test]
    fn humanize_transport_translates_known_kinds() {
        assert_eq!(humanize_transport("nebula_direct"), "direct UDP");
        assert_eq!(
            humanize_transport("nebula_lighthouse_relay"),
            "lighthouse relay"
        );
        assert_eq!(humanize_transport("nebula_https443"), "HTTPS-443 fallback");
        assert_eq!(humanize_transport("kdc_tls"), "KDC2 TLS");
        // Unknown transports fall back to a humanized form.
        assert_eq!(humanize_transport("future_thing"), "future thing");
    }

    #[test]
    fn extract_json_string_field_finds_active_transport() {
        let raw = r#"{"is_lighthouse":false,"ca_epoch":3,"peer_count":4,"mesh_id":"m1","active_transport":"nebula_direct"}"#;
        assert_eq!(
            extract_json_string_field(raw, "active_transport"),
            Some("nebula_direct".into())
        );
    }

    #[test]
    fn extract_json_string_field_returns_none_for_missing() {
        let raw = r#"{"present":"yes"}"#;
        assert!(extract_json_string_field(raw, "missing").is_none());
    }

    #[test]
    fn extract_peer_count_parses_sub_status() {
        let row = CapabilityRow {
            id: CapabilityId::Peers,
            name: "x",
            description: "x",
            icon: Icon::Peer,
            status: CapabilityStatus::Active,
            sub_status: Some("3 of 7 peers online".into()),
            jump: None,
            launch: None,
        };
        assert_eq!(extract_peer_count(&row), Some(7));
    }

    #[test]
    fn extract_peer_count_returns_none_without_sub() {
        let row = CapabilityRow {
            id: CapabilityId::Peers,
            name: "x",
            description: "x",
            icon: Icon::Peer,
            status: CapabilityStatus::Unknown,
            sub_status: None,
            jump: None,
            launch: None,
        };
        assert_eq!(extract_peer_count(&row), None);
    }

    #[test]
    fn message_refreshed_preserves_capabilities() {
        let mut panel = HomePanel::new();
        panel.snapshot.capabilities = build_all_rows_with_unknown_status();
        panel.snapshot.mackesd_reachable = false;
        let new_snap = HomeSnapshot::load_sync();
        let _ = panel.update(Message::Refreshed(new_snap));
        assert_eq!(
            panel.snapshot.capabilities.len(),
            11,
            "refresh keeps capability rows"
        );
        assert!(
            !panel.snapshot.mackesd_reachable,
            "refresh keeps mackesd_reachable"
        );
    }

    #[test]
    fn unit_name_decodes_sshd() {
        assert_eq!(
            unit_name_from_path("/org/freedesktop/systemd1/unit/sshd_2eservice").as_deref(),
            Some("sshd.service"),
        );
    }

    #[test]
    fn unit_name_decodes_x11vnc_template() {
        assert_eq!(
            unit_name_from_path("/org/freedesktop/systemd1/unit/x11vnc_40_3a0_2eservice")
                .as_deref(),
            Some("x11vnc@:0.service"),
        );
    }

    #[test]
    fn unit_name_decodes_no_escapes() {
        assert_eq!(
            unit_name_from_path("/org/freedesktop/systemd1/unit/mackesd").as_deref(),
            Some("mackesd"),
        );
    }

    #[test]
    fn unit_name_returns_none_for_non_systemd_path() {
        assert!(unit_name_from_path("/org/freedesktop/DBus").is_none());
        assert!(unit_name_from_path("").is_none());
    }

    #[test]
    fn systemd_watch_list_includes_ssh_rdp_vnc_and_mesh_units() {
        let list = systemd_watch_list();
        assert!(list.iter().any(|u| u == "sshd.service"));
        assert!(list.iter().any(|u| u == "xrdp.service"));
        assert!(list.iter().any(|u| u == "x11vnc@:0.service"));
        assert!(list.iter().any(|u| u == "wayvnc.service"));
        for (name, _, _) in MESH_UNITS.iter() {
            assert!(
                list.iter().any(|u| u == name),
                "MESH_UNITS entry {name} missing from watch list",
            );
        }
    }

    #[test]
    fn message_capabilities_refreshed_updates_state() {
        let mut panel = HomePanel::new();
        assert!(panel.snapshot.capabilities.is_empty());
        let rows = build_all_rows_with_unknown_status();
        let _ = panel.update(Message::CapabilitiesRefreshed {
            rows: rows.clone(),
            mackesd_reachable: true,
        });
        assert_eq!(panel.snapshot.capabilities, rows);
        assert!(panel.snapshot.mackesd_reachable);
    }

    #[test]
    fn nebula_event_from_body_maps_each_kind() {
        // E0.3.1.b — decodes the bodies mackesd's signal_event_body
        // writes; peer-state + enrollment both → PeerChanged.
        assert!(matches!(
            nebula_event_from_body(
                r#"{"kind":"peer-state-changed","node_id":"p","reachable":"online"}"#
            ),
            Some(DbusEvent::PeerChanged)
        ));
        assert!(matches!(
            nebula_event_from_body(r#"{"kind":"enrollment-completed","node_id":"p"}"#),
            Some(DbusEvent::PeerChanged)
        ));
        assert!(matches!(
            nebula_event_from_body(
                r#"{"kind":"transport-changed","active_transport":"nebula_direct"}"#
            ),
            Some(DbusEvent::TransportChanged)
        ));
        // Unknown kind + malformed JSON yield None (ignored).
        assert!(nebula_event_from_body(r#"{"kind":"who-knows"}"#).is_none());
        assert!(nebula_event_from_body("not json").is_none());
    }
}

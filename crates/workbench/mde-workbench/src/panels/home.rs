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
//!   `mackesd nodes list --json` / `mackesd mesh-fs-status --json`
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

use std::time::{Duration, Instant};

use cosmic::iced::widget::{
    button, column, container, row, scrollable, svg as widget_svg, text, Space,
};
use cosmic::iced::{Background, Border, Color, Element, Length, Padding, Shadow, Task, Vector};
use cosmic::Theme;
use mde_theme::feedback::{ControlFeedback, FeedbackParams};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::components::connect_progress::{self, ConnectProgress};
use crate::cosmic_compat::prelude::*;

use crate::model::Group;
use crate::panels::mesh_services::MESH_UNITS;
use crate::panels::node_roster::fetch_peers;

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
        let palette = crate::live_theme::palette();
        match self {
            Self::Active => palette.success.into_cosmic_color(),
            Self::SetupNeeded => palette.warning.into_cosmic_color(),
            Self::Unknown => palette.text_muted.into_cosmic_color(),
            Self::Failed { .. } => palette.danger.into_cosmic_color(),
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
    /// BOOT-STATUS-4 — `ready` from the boot_readiness snapshot.
    pub boot_ready: bool,
    /// BOOT-STATUS-4 — the bring-up step chain (empty = unknown/mid-boot).
    pub boot_steps: Vec<BootStep>,
    /// BOOT-STATUS-2 — supplementary app daemons (musicd / netdata / KDE Connect).
    pub boot_services: Vec<BootService>,
    /// BOOT-STATUS-2/3 — per-peer ping roll-up (reachability + RTT).
    pub boot_pings: Vec<BootPing>,
}

impl HomeSnapshot {
    /// Synchronous filesystem-only load. Cheap; no async.
    #[must_use]
    pub fn load_sync() -> Self {
        let boot = read_boot_readiness();
        // WB-OVERVIEW-1 — seed the Mesh-peers tile from the boot-readiness ping
        // roll-up (the same data the peer list below renders) so it shows a real
        // count immediately instead of "—". The async capability probe refines
        // it later if it can parse a count, but never blanks it.
        let mesh_peers = (!boot.pings.is_empty()).then_some(boot.pings.len() as u32);
        Self {
            mde_version: env!("CARGO_PKG_VERSION").to_string(),
            fedora_release: read_fedora_release().unwrap_or_else(|| "44".into()),
            hostname: read_hostname(),
            mesh_peers,
            pending_updates: Some(read_dnf_count()),
            // A real "0" rather than a bare "—" when there's simply nothing
            // (no snapshot dir / no drift tracking) — "—" reads as broken.
            snapshot_count: Some(count_snapshots().unwrap_or(0)),
            drift_count: Some(0),
            capabilities: Vec::new(),
            mackesd_reachable: true,
            boot_ready: boot.ready,
            boot_steps: boot.steps,
            boot_services: boot.services,
            boot_pings: boot.pings,
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
// BOOT-STATUS-4 — mesh bring-up chain (reads the boot_readiness snapshot)
// ---------------------------------------------------------------------------

/// One bring-up step, decoded from the `state/boot-readiness` snapshot
/// (BOOT-STATUS-1). `status` is `ok` / `pending` / `blocked`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootStep {
    /// Stable step id (`nebula` / `overlay-ip` / `mackesd` / `bus` / `qnm` /
    /// `directory`) — robust to render across panels (BOOT-PEERS-1 keys on it).
    pub id: String,
    /// Display label (e.g. "QNM-Shared mounted").
    pub label: String,
    /// `ok` | `pending` | `blocked`.
    pub status: String,
    /// Short per-step detail (overlay IP, peer count, …).
    pub detail: String,
}

/// BOOT-STATUS-2 — one app-daemon row decoded from the snapshot `services`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootService {
    /// Stable id (`musicd` / `netdata` / `kdc`) — BOOT-STATUS-6 maps it to the
    /// systemd unit + scope for the inline Restart action.
    pub id: String,
    /// Display label (e.g. "Music daemon").
    pub label: String,
    /// `ok` | `down`.
    pub status: String,
}

/// BOOT-STATUS-6 — the remediation target for a down app-daemon service:
/// `(unit, user_scope)`. `mde-musicd` is a per-user unit (so a plain
/// `systemctl --user restart`, no pkexec); netdata + the in-process KDE Connect
/// listener (hosted by `mackesd`) are system units (pkexec). `None` for an
/// unknown id (no button rendered).
#[must_use]
pub fn service_remediation(id: &str) -> Option<(&'static str, bool)> {
    match id {
        "musicd" => Some(("mde-musicd", true)),
        "netdata" => Some(("netdata", false)),
        "kdc" => Some(("mackesd", false)),
        _ => None,
    }
}

/// BOOT-STATUS-2/3 — one per-peer ping row decoded from the snapshot `pings`
/// (the roll-up rows: who's reachable + RTT).
#[derive(Debug, Clone, PartialEq)]
pub struct BootPing {
    /// Peer hostname.
    pub peer: String,
    /// Peer role (`lighthouse` / `peer`).
    pub role: String,
    /// Round-trip ms, or `None` when unreachable.
    pub rtt_ms: Option<f64>,
}

/// The decoded `state/boot-readiness` snapshot (BOOT-STATUS-2/4). A
/// missing/garbage body → an all-empty value (the section shows "unknown").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BootReadiness {
    /// Whether every fabric chain step is `ok`.
    pub ready: bool,
    /// The bring-up dependency chain.
    pub steps: Vec<BootStep>,
    /// BOOT-STATUS-2 — supplementary app daemons.
    pub services: Vec<BootService>,
    /// BOOT-STATUS-2/3 — per-peer ping roll-up.
    pub pings: Vec<BootPing>,
}

/// BOOT-STATUS-6 — restart a down app daemon from its boot-status row, returning
/// a one-line result. A user unit (`mde-musicd`) restarts as the session user
/// (`systemctl --user restart`, no privilege prompt); a system unit goes through
/// `pkexec systemctl restart` (the same path the Mesh Services panel uses).
pub async fn restart_service(unit: String, user_scope: bool) -> String {
    let ok = if user_scope {
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("systemctl")
                .args(["--user", "restart", &unit])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    } else {
        super::mesh_services::run_pkexec_systemctl(
            &super::mesh_services::UnitScope::System,
            "restart",
            &unit,
        )
        .await
    };
    if ok {
        "restart ok".to_string()
    } else {
        "restart FAILED — see journalctl".to_string()
    }
}

/// BOOT-STATUS-5 — should the boot-status auto-popup suppress itself? True only
/// when launched as the autostart popup (`--boot-popup`) AND the mesh is already
/// all-green: then the persistent applet chip / HOME glance suffices and we don't
/// pop the window. During the cold-boot warm-up (`ready == false`) the popup opens
/// so boot status is front-and-centre at login.
#[must_use]
pub fn boot_popup_should_suppress(boot_popup: bool, ready: bool) -> bool {
    boot_popup && ready
}

impl BootReadiness {
    /// BOOT-PEERS-1 — is the mesh fabric still coming up? True when a snapshot
    /// exists and any *fabric* step (everything but the final peer `directory`
    /// step) isn't `ok` yet — i.e. Nebula / overlay-IP / bus / QNM haven't all
    /// converged. An empty roster during this window is "settling", not "empty
    /// mesh". A lone healthy node (fabric up, just no peers) returns `false`, so
    /// the genuine empty state still shows.
    #[must_use]
    pub fn fabric_converging(&self) -> bool {
        !self.steps.is_empty()
            && self
                .steps
                .iter()
                .any(|s| s.id != "directory" && s.status != "ok")
    }
}

/// Parse the `state/boot-readiness` snapshot body. A missing/garbage body →
/// [`BootReadiness::default`] (the section then shows "unknown").
#[must_use]
pub fn parse_boot_readiness(reply: &str) -> BootReadiness {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(reply) else {
        return BootReadiness::default();
    };
    let ready = v
        .get("ready")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let steps = v
        .get("steps")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(BootStep {
                        id: s
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        label: s.get("label")?.as_str()?.to_string(),
                        status: s
                            .get("status")
                            .and_then(|x| x.as_str())
                            .unwrap_or("pending")
                            .to_string(),
                        detail: s
                            .get("detail")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let services = v
        .get("services")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(BootService {
                        id: s
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        label: s.get("label")?.as_str()?.to_string(),
                        status: s
                            .get("status")
                            .and_then(|x| x.as_str())
                            .unwrap_or("down")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let pings = v
        .get("pings")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(BootPing {
                        peer: s.get("peer")?.as_str()?.to_string(),
                        role: s
                            .get("role")
                            .and_then(|x| x.as_str())
                            .unwrap_or("peer")
                            .to_string(),
                        rtt_ms: s.get("rtt_ms").and_then(serde_json::Value::as_f64),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    BootReadiness {
        ready,
        steps,
        services,
        pings,
    }
}

/// Read the latest `state/boot-readiness` snapshot off the bus.
/// [`BootReadiness::default`] when the bus/topic isn't available yet (mid-boot).
#[must_use]
pub fn read_boot_readiness() -> BootReadiness {
    let Some(dir) = mde_bus::default_data_dir() else {
        return BootReadiness::default();
    };
    let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
        return BootReadiness::default();
    };
    let topic = "state/boot-readiness";
    let Ok(Some(latest)) = persist.latest_ulid(topic) else {
        return BootReadiness::default();
    };
    persist
        .list_since(topic, None)
        .ok()
        .and_then(|msgs| msgs.into_iter().rev().find(|m| m.ulid == latest))
        .and_then(|m| m.body)
        .map(|b| parse_boot_readiness(&b))
        .unwrap_or_default()
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
    /// BOOT-STATUS-6 — restart a down app daemon from its boot-status row.
    RemediateService {
        unit: String,
        user_scope: bool,
    },
    /// BOOT-STATUS-6 — the restart finished; carries the result line.
    RemediationDone(String),
    DbusEvent(DbusEvent),
    /// MESH-CONNECT-DIALOG-1 — a capability row's "Configure" was clicked:
    /// open the progress modal and re-probe that capability's real status.
    ConfigureClicked(CapabilityId),
    /// MESH-CONNECT-DIALOG-1 — the configure re-probe landed; resolves the
    /// modal to its terminal success / failure.
    ConnectProbed {
        id: CapabilityId,
        outcome: ProbeOutcome,
    },
    /// MESH-CONNECT-DIALOG-1 — re-run the probe from the modal's failure state.
    ConnectRetry,
    /// MESH-CONNECT-DIALOG-1 — close the connect modal (Dismiss / backdrop).
    ConnectDismiss,
    /// BOOT-STATUS — live push from `state/boot-readiness` subscription.
    /// Fired sub-second each time the mackesd boot_readiness worker
    /// publishes a new snapshot; replaces the stale snapshot in-place.
    BootReadinessUpdate(BootReadiness),
}

#[derive(Debug, Clone, Default)]
pub struct HomePanel {
    pub snapshot: HomeSnapshot,
    /// BOOT-STATUS-6 — the last inline-remediation result line (empty = none).
    pub remediation: String,
    /// MESH-CONNECT-DIALOG-1 — the connect/configure progress modal opened
    /// by a capability row's "Configure" button: pending (re-probing the
    /// capability's real status) → success / failure.
    pub connect: ConnectProgress,
    /// The capability the open modal is probing — drives Retry + the jump
    /// offered on success.
    pub connect_target: Option<CapabilityId>,
}

impl HomePanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshot: HomeSnapshot::load_sync(),
            remediation: String::new(),
            connect: ConnectProgress::Closed,
            connect_target: None,
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
                // WB-OVERVIEW-1 — only refine the tile when the Peers probe can
                // parse a count; otherwise keep the boot-ping seed (never blank
                // it back to "—").
                if let Some(n) = self
                    .snapshot
                    .capabilities
                    .iter()
                    .find(|r| r.id == CapabilityId::Peers)
                    .and_then(extract_peer_count)
                {
                    self.snapshot.mesh_peers = Some(n);
                }
                Task::none()
            }
            Message::RefreshClicked => Self::load(),
            // BOOT-STATUS-6 — restart a down app daemon, then re-probe so the row
            // flips to ✓ once it's back. A user unit (mde-musicd) restarts as the
            // session user (no pkexec); a system unit goes through pkexec.
            Message::RemediateService { unit, user_scope } => {
                self.remediation = format!("restarting {unit}…");
                Task::perform(restart_service(unit, user_scope), |line| {
                    crate::Message::Home(Message::RemediationDone(line))
                })
            }
            Message::RemediationDone(line) => {
                self.remediation = line;
                // Re-probe so the services row reflects the new state.
                Self::load()
            }
            Message::DbusEvent(ev) => Task::perform(reprobe_for_event(ev), |(rows, ok)| {
                crate::Message::Home(Message::CapabilitiesRefreshed {
                    rows,
                    mackesd_reachable: ok,
                })
            }),
            // MESH-CONNECT-DIALOG-1 — Configure opens the modal and re-probes
            // the capability's REAL status (the same probe the row's pill uses),
            // so the operator gets live progress + a terminal outcome instead of
            // a silent panel jump.
            Message::ConfigureClicked(id) => self.probe_capability(id),
            // Retry re-runs the same probe for the modal's remembered target.
            Message::ConnectRetry => match self.connect_target {
                Some(id) => self.probe_capability(id),
                None => Task::none(),
            },
            Message::ConnectDismiss => {
                self.connect = ConnectProgress::Closed;
                self.connect_target = None;
                Task::none()
            }
            // BOOT-STATUS — live push from the `state/boot-readiness` subscription.
            // Apply the fresh snapshot in-place: update the boot fields while
            // preserving the capability rows + mackesd_reachable already probed.
            Message::BootReadinessUpdate(boot) => {
                self.snapshot.boot_ready = boot.ready;
                self.snapshot.boot_steps = boot.steps;
                self.snapshot.boot_services = boot.services;
                // Keep the hero mesh_peers tile in sync with the ping roll-up.
                if !boot.pings.is_empty() {
                    self.snapshot.mesh_peers = Some(boot.pings.len() as u32);
                }
                self.snapshot.boot_pings = boot.pings;
                Task::none()
            }
            // The re-probe landed — resolve the modal from the capability's real
            // status. Active = connected (success); SetupNeeded/Failed/Unknown =
            // a real, operator-readable reason (failure → Retry/Dismiss).
            Message::ConnectProbed { id, outcome } => {
                if self.connect_target == Some(id) {
                    self.connect = match &outcome.status {
                        CapabilityStatus::Active => self.connect.success(
                            outcome
                                .sub_status
                                .clone()
                                .unwrap_or_else(|| "Connected and active.".into()),
                        ),
                        CapabilityStatus::SetupNeeded => {
                            self.connect
                                .failure(outcome.sub_status.clone().unwrap_or_else(|| {
                                    "Not connected yet — set this capability up.".into()
                                }))
                        }
                        CapabilityStatus::Failed { detail } => self.connect.failure(detail.clone()),
                        CapabilityStatus::Unknown => self
                            .connect
                            .failure("Status unavailable — the control plane did not answer."),
                    };
                }
                Task::none()
            }
        }
    }

    /// MESH-CONNECT-DIALOG-1 — open the connect/configure modal for `id` and
    /// fire that capability's real status probe. Shared by the row's Configure
    /// button and the modal's Retry. The probe result resolves the modal via
    /// [`Message::ConnectProbed`].
    fn probe_capability(&mut self, id: CapabilityId) -> Task<crate::Message> {
        self.connect = ConnectProgress::pending(
            format!("Configure {}", capability_label(id)),
            format!("Checking {} status…", capability_label(id)),
        );
        self.connect_target = Some(id);
        Task::perform(probe_capability_status(id), move |outcome| {
            crate::Message::Home(Message::ConnectProbed { id, outcome })
        })
    }

    /// Render the Overview.
    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Overview")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let identity = text(format!(
            "MDE {ver} · Fedora {rel} · {host}",
            ver = self.snapshot.mde_version,
            rel = self.snapshot.fedora_release,
            host = self.snapshot.hostname,
        ))
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

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
                Group::System,
                "snapshots",
                palette,
            ),
            Space::new().width(Length::Fixed(12.0)),
            stat_card(
                "Snapshots",
                self.snapshot.snapshot_count,
                Icon::Snapshot,
                Group::System,
                "snapshots",
                palette,
            ),
            Space::new().width(Length::Fixed(12.0)),
            stat_card(
                "Drift events",
                self.snapshot.drift_count,
                Icon::Repair,
                Group::Fleet,
                "drift",
                palette,
            ),
        ];

        // Optional mackesd-down banner — only renders when the
        // last probe could not reach the control plane.
        let banner: Element<'_, crate::Message, Theme> = if self.snapshot.mackesd_reachable {
            Space::new().height(Length::Fixed(0.0)).into()
        } else {
            mackesd_banner(palette)
        };

        let section_title = text("What this Mackes mesh can do for you")
            .size(TypeRole::Heading.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let section_subtitle = text(
            "Each row shows a feature, whether it is running on this machine, \
             and a shortcut to set it up.",
        )
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

        let refresh_btn = refresh_button(palette);

        let capability_list: Element<'_, crate::Message, Theme> =
            if self.snapshot.capabilities.is_empty() {
                text("Loading capability status…")
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color())
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
            .colr(palette.text.into_cosmic_color());
        let manage_row = row![
            role_link("Mesh", Group::Mesh, palette),
            role_link("Fleet", Group::Fleet, palette),
            role_link("Provisioning", Group::Provisioning, palette),
            role_link("Monitoring", Group::Monitoring, palette),
            role_link("System", Group::System, palette),
        ]
        .spacing(8);
        let see_also_row = row![
            text("See also:")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
            settings_link("All settings", "", palette),
            settings_link("Storage", "storage", palette),
            settings_link("Backup & recovery", "recovery", palette),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        // BOOT-STATUS-4 — the mesh bring-up chain. Collapses to a one-line
        // "Mesh ready" when all-green; otherwise lists each step with a status
        // glyph (✓ ok / ◷ pending / ⊘ blocked) + its detail. BOOT-STATUS-2/3
        // append the app-daemon rows + the per-peer ping roll-up beneath it
        // (always shown when present, both before and after the chain is ready).
        // Hidden entirely until the first snapshot arrives (mid-boot/unknown).
        let body_sz = TypeRole::Body.size_in(sizes);
        let cap_sz = TypeRole::Caption.size_in(sizes);
        let boot_status: Element<'_, crate::Message, Theme> = if self.snapshot.boot_steps.is_empty()
            && self.snapshot.boot_services.is_empty()
            && self.snapshot.boot_pings.is_empty()
        {
            Space::new().height(Length::Fixed(0.0)).into()
        } else {
            let mut col = column![].spacing(6);
            // The fabric chain (collapses to the green chip when ready).
            if self.snapshot.boot_ready {
                col = col.push(
                    text("✓ Mesh ready — all fabric services up")
                        .size(body_sz)
                        .colr(palette.success.into_cosmic_color()),
                );
            } else if !self.snapshot.boot_steps.is_empty() {
                col = col.push(
                    text("Mesh bring-up")
                        .size(TypeRole::Heading.size_in(sizes))
                        .colr(palette.text.into_cosmic_color()),
                );
                for st in &self.snapshot.boot_steps {
                    let (glyph, color) = match st.status.as_str() {
                        "ok" => ("✓", palette.success),
                        "blocked" => ("⊘", palette.text_muted),
                        _ => ("◷", palette.warning),
                    };
                    col = col.push(
                        row![
                            text(glyph).size(body_sz).colr(color.into_cosmic_color()),
                            text(st.label.clone())
                                .size(body_sz)
                                .colr(palette.text.into_cosmic_color()),
                            text(st.detail.clone())
                                .size(cap_sz)
                                .colr(palette.text_muted.into_cosmic_color()),
                        ]
                        .spacing(8)
                        .align_y(cosmic::iced::alignment::Vertical::Center),
                    );
                }
            }
            // BOOT-STATUS-2 — app daemons (musicd / netdata / KDE Connect), each
            // on its own row; BOOT-STATUS-6 adds an inline Restart button to a
            // down daemon that has a known remediation (wired to the same
            // service-control path the Mesh Services panel uses).
            if !self.snapshot.boot_services.is_empty() {
                col = col.push(
                    text("Services")
                        .size(cap_sz)
                        .colr(palette.text_muted.into_cosmic_color()),
                );
                for s in &self.snapshot.boot_services {
                    let (glyph, color) = if s.status == "ok" {
                        ("✓", palette.success)
                    } else {
                        ("✕", palette.warning)
                    };
                    let mut svc_row = row![
                        text(glyph).size(cap_sz).colr(color.into_cosmic_color()),
                        text(s.label.clone())
                            .size(cap_sz)
                            .colr(palette.text.into_cosmic_color()),
                    ]
                    .spacing(8)
                    .align_y(cosmic::iced::alignment::Vertical::Center);
                    // BOOT-STATUS-6 — Restart for a down, remediable daemon.
                    if s.status != "ok" {
                        if let Some((unit, user_scope)) = service_remediation(&s.id) {
                            let bg = palette.raised.into_cosmic_color();
                            let border = palette.border.into_cosmic_color();
                            let fg = palette.text.into_cosmic_color();
                            let accent = palette.accent.into_cosmic_color();
                            let reduce_motion = crate::live_theme::reduce_motion();
                            svc_row = svc_row.push(
                                button(text("Restart").size(cap_sz))
                                    .padding(Padding::from([2u16, 8u16]))
                                    .sty(move |_t: &Theme, status| {
                                        feedback_button_style(
                                            status,
                                            FeedbackStyle {
                                                bg,
                                                border,
                                                text_color: fg,
                                                accent,
                                                radius: 6.0,
                                                hover_factor: 1.12,
                                            },
                                            Instant::now(),
                                            reduce_motion,
                                        )
                                    })
                                    .on_press(crate::Message::Home(Message::RemediateService {
                                        unit: unit.to_string(),
                                        user_scope,
                                    })),
                            );
                        }
                    }
                    col = col.push(svc_row);
                }
                // BOOT-STATUS-6 — the last remediation result line.
                if !self.remediation.is_empty() {
                    col = col.push(
                        text(self.remediation.clone())
                            .size(cap_sz)
                            .colr(palette.text_muted.into_cosmic_color()),
                    );
                }
            }
            // BOOT-STATUS-2/3 — per-peer ping roll-up (reachability + RTT).
            if !self.snapshot.boot_pings.is_empty() {
                col = col.push(
                    text("Peers")
                        .size(cap_sz)
                        .colr(palette.text_muted.into_cosmic_color()),
                );
                for pg in &self.snapshot.boot_pings {
                    let (glyph, color, rtt) = match pg.rtt_ms {
                        Some(ms) => ("✓", palette.success, format!("{ms:.1} ms")),
                        None => ("✕", palette.text_muted, "unreachable".to_string()),
                    };
                    let tag = if pg.role == "lighthouse" {
                        format!("{} (lighthouse)", pg.peer)
                    } else {
                        pg.peer.clone()
                    };
                    col = col.push(
                        row![
                            text(glyph).size(cap_sz).colr(color.into_cosmic_color()),
                            text(tag)
                                .size(cap_sz)
                                .colr(palette.text.into_cosmic_color()),
                            text(rtt)
                                .size(cap_sz)
                                .colr(palette.text_muted.into_cosmic_color()),
                        ]
                        .spacing(8)
                        .align_y(cosmic::iced::alignment::Vertical::Center),
                    );
                }
            }
            col.into()
        };

        let body = column![
            title,
            Space::new().height(Length::Fixed(4.0)),
            identity,
            Space::new().height(Length::Fixed(24.0)),
            cards,
            Space::new().height(Length::Fixed(16.0)),
            boot_status,
            Space::new().height(Length::Fixed(24.0)),
            manage_label,
            Space::new().height(Length::Fixed(8.0)),
            manage_row,
            Space::new().height(Length::Fixed(8.0)),
            see_also_row,
            Space::new().height(Length::Fixed(24.0)),
            banner,
            row![section_title, Space::new().width(Length::Fill), refresh_btn]
                .align_y(cosmic::iced::alignment::Vertical::Center),
            Space::new().height(Length::Fixed(2.0)),
            section_subtitle,
            Space::new().height(Length::Fixed(12.0)),
            capability_list,
        ]
        .spacing(2);

        let panel: Element<'_, crate::Message, Theme> =
            container(scrollable(body).width(Length::Fill))
                .padding(Padding::from([24u16, 32u16]))
                .width(Length::Fill)
                .into();

        // MESH-CONNECT-DIALOG-1 — stack the connect/configure progress modal
        // over the Overview when a capability's Configure is open. The modal's
        // primary "Open settings ▸" action deep-links to that capability's panel
        // so confirming status (or finding it needs setup) doesn't dead-end —
        // the operator can still jump to where they configure it.
        let primary = self
            .connect_target
            .and_then(capability_jump)
            .map(|(group, panel)| {
                (
                    "Open settings  ▸",
                    crate::Message::SelectPanel { group, panel },
                )
            });
        connect_progress::overlay_with_action(
            &self.connect,
            panel,
            palette,
            crate::Message::Home(Message::ConnectRetry),
            crate::Message::Home(Message::ConnectDismiss),
            primary,
        )
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
) -> Element<'a, crate::Message, Theme> {
    nav_chip(label, palette, crate::Message::SelectGroup(group))
}

/// A Dashboard "See also" link that opens a Settings page at `slug`
/// (`""` = the Settings home).
fn settings_link<'a>(
    label: &'static str,
    slug: &'static str,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    nav_chip(label, palette, crate::Message::OpenSettings(slug))
}

/// Shared pill-style action-link button (palette-tokened; faint raised
/// tint on hover).
fn nav_chip<'a>(
    label: &'static str,
    palette: Palette,
    msg: crate::Message,
) -> Element<'a, crate::Message, Theme> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let fg = palette.text.into_cosmic_color();
    let accent = palette.accent.into_cosmic_color();
    let reduce_motion = crate::live_theme::reduce_motion();
    button(text(label).size(13).colr(fg))
        .padding(Padding::from([6u16, 12u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                feedback_button_style(
                    status,
                    FeedbackStyle {
                        bg,
                        border,
                        text_color: fg,
                        accent,
                        radius: 6.0,
                        hover_factor: 1.1,
                    },
                    Instant::now(),
                    reduce_motion,
                )
            },
        )
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

/// MESH-CONNECT-DIALOG-1 — the human label for a capability, reused in the
/// connect modal's title + "checking …" line. Mirrors the row builders'
/// `name` so the modal and the row agree.
#[must_use]
pub fn capability_label(id: CapabilityId) -> &'static str {
    match id {
        CapabilityId::Mesh => "Mesh Network",
        CapabilityId::Peers => "Peer Reachability",
        CapabilityId::Files => "File Sharing",
        CapabilityId::Ssh => "SSH Across Mesh",
        CapabilityId::Rdp => "Remote Desktop (RDP)",
        CapabilityId::Vnc => "Remote Desktop (VNC)",
        CapabilityId::Services => "Media & App Discovery",
        CapabilityId::Phone => "Mesh peer (phone)",
        CapabilityId::Voice => "Voice & Video",
        CapabilityId::Fleet => "Fleet Configuration",
        CapabilityId::Notifications => "Desktop Notifications",
    }
}

/// MESH-CONNECT-DIALOG-1 — the in-Workbench panel a capability's Configure
/// deep-links to (the `(Group, panel-slug)` the row builders carry). `None`
/// for a capability with no in-Workbench panel (e.g. Voice, which launches a
/// standalone app). Drives the connect modal's "Open settings ▸" primary
/// action so confirming status doesn't dead-end the operator.
#[must_use]
pub fn capability_jump(id: CapabilityId) -> Option<(Group, &'static str)> {
    match id {
        CapabilityId::Mesh => Some((Group::Mesh, "mesh_control")),
        CapabilityId::Peers => Some((Group::Mesh, "peers")),
        CapabilityId::Files => Some((Group::Mesh, "mesh_storage")),
        CapabilityId::Ssh | CapabilityId::Rdp | CapabilityId::Vnc => {
            Some((Group::ThisNode, "remote_desktop"))
        }
        CapabilityId::Services => Some((Group::ThisNode, "mesh_services")),
        CapabilityId::Phone => Some((Group::Mesh, "connect")),
        // Voice has no in-Workbench panel (standalone mde-voice-config app).
        CapabilityId::Voice => None,
        CapabilityId::Fleet => Some((Group::Fleet, "playbooks")),
        CapabilityId::Notifications => Some((Group::System, "notifications")),
    }
}

/// MESH-CONNECT-DIALOG-1 — re-run the SINGLE real status probe behind a
/// capability's pill (the same probes [`load_capabilities`] fans out), so the
/// connect modal reflects that capability's true state. Dispatches by id to the
/// existing per-capability probe — no fake/demo progress.
pub async fn probe_capability_status(id: CapabilityId) -> ProbeOutcome {
    match id {
        CapabilityId::Mesh => probe_nebula().await,
        CapabilityId::Peers => probe_peers().await,
        CapabilityId::Files => probe_files().await,
        CapabilityId::Ssh => probe_systemd_unit("sshd.service").await,
        CapabilityId::Rdp => probe_systemd_unit("xrdp.service").await,
        CapabilityId::Vnc => probe_vnc().await,
        CapabilityId::Services => probe_mesh_services().await,
        CapabilityId::Phone => probe_phone().await,
        CapabilityId::Voice => probe_voice().await,
        CapabilityId::Fleet => probe_fleet_revision().await,
        CapabilityId::Notifications => probe_notifications().await,
    }
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

/// OVERVIEW-MESHCARD-1 — `(overlay_ip, online_peer_count)` from the
/// world-readable mesh-status snapshot, the reachability source of truth the
/// shell + peers use. `None` when the snapshot is absent.
fn overlay_from_snapshot() -> Option<(String, usize)> {
    let body = std::fs::read_to_string("/run/mde/mesh-status.json").ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let ip = v
        .get("network")
        .and_then(|n| n.get("overlay_ip"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let online = usize::try_from(
        v.get("online")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    )
    .unwrap_or(0);
    Some((ip, online))
}

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
        // OVERVIEW-MESHCARD-1 — `active_transport` is unreliable on non-CA peer
        // nodes (the roster/transport state lives in the signer/CA's local DB,
        // so peers report "offline"/0 even with a healthy overlay). Fall back to
        // real reachability from the world-readable mesh-status snapshot: an
        // overlay IP + online peers means the fabric IS connected.
        if let Some((ip, online)) = overlay_from_snapshot() {
            if !ip.is_empty() && online > 0 {
                return ProbeOutcome::active(Some(format!(
                    "Overlay up — {online} peer(s) reachable"
                )));
            }
        }
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
    // action/fleet/list-revisions over the mesh Bus (FPG-4): the reply
    // is `{ok, head, revisions: [{version, author, at}]}`. The pill
    // shows the elected head version; an ok-empty log reads "No
    // revisions pushed yet". spawn_blocking: the Bus client spins its
    // own current-thread runtime (Persist isn't Send).
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
    match extract_head_version(&raw) {
        Some(head) => ProbeOutcome::active(Some(format!("Fleet at revision {head}"))),
        None => ProbeOutcome::setup_needed(Some("No revisions pushed yet".into())),
    }
}

/// Parse the FPG-4 list-revisions reply's elected `head` version.
/// `None` for an empty log, an error envelope, or non-JSON.
fn extract_head_version(raw: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    v.get("head").and_then(serde_json::Value::as_u64)
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
    // Same source as the Mesh Storage panel: `mackesd mesh-fs-status --json`.
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
    let dir = mde_bus::client_data_dir()?;
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
        jump: Some((Group::Mesh, "mesh_control")),
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
        jump: Some((Group::Mesh, "peers")),
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
        jump: Some((Group::Mesh, "mesh_storage")),
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
        jump: Some((Group::ThisNode, "remote_desktop")),
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
        jump: Some((Group::ThisNode, "remote_desktop")),
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
        jump: Some((Group::ThisNode, "remote_desktop")),
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
        jump: Some((Group::ThisNode, "mesh_services")),
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
        jump: Some((Group::Mesh, "connect")),
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

// ---------------------------------------------------------------------------
// MOTION-FEEDBACK-1/2 — shared control feedback for the Overview's interactive
// controls (stat cards, nav chips, Configure / Set up / Refresh buttons).
//
// Iced 0.13's `button::Status` delivers hover/press as discrete render-time
// states (Pressed fires on pointer-DOWN with no delay — the no-input-delay
// acceptance criterion is the toolkit's, we honor it). A `button::Style` closure
// can only express color / border / shadow (it has no transform field — and a
// geometric translate/scale would thrash layout anyway), so the hover-lift and
// press-depress are conveyed the compositor-friendly, render-expressible way:
//   * hover  → a faint brighter tint + a raised elevation shadow (rises),
//   * press  → a darker tint + the elevation collapses to flat (the surface
//     "sinks" — losing its shadow IS the depress cue) + an animated accent ring.
//
// The shared `mde_theme::feedback::ControlFeedback` vocabulary owns the two parts
// it can: the **reduce-motion contract** (whether the elevation is allowed to
// move at all — `FeedbackParams::is_at_rest()` is forced true under reduce-motion,
// pinning the shadow flat) and the **animated accent ring** (`FocusRing` grows the
// accent outline in; snaps to full under reduce-motion). These buttons have no
// separate keyboard-focus style path in the iced 0.13 fork (`button::Status` has
// no Focused variant), so the engaged/pressed control IS the "active item" the
// ring marks (MOTION-FEEDBACK-2: selection animates the accent).
//
// Reduce-motion (Q32): the STATE change is kept (the hover/press color tokens
// still swap; the accent ring is still present, at full) but the MOVEMENT is
// dropped (the elevation shadow never rises/falls).
// ---------------------------------------------------------------------------

/// Scale a base background's RGB by `factor` (>1 brightens for hover, <1 darkens
/// for press), preserving alpha. The color swap is the reduce-motion-safe state
/// cue — applied with or without motion.
fn tint(base: Color, factor: f32) -> Color {
    Color {
        r: (base.r * factor).clamp(0.0, 1.0),
        g: (base.g * factor).clamp(0.0, 1.0),
        b: (base.b * factor).clamp(0.0, 1.0),
        a: base.a,
    }
}

/// MOTION-FEEDBACK — the palette tokens + tunables one Overview control feeds the
/// shared feedback styler. Grouping them keeps [`feedback_button_style`] to a
/// single descriptor argument (and single-sources each control's colors §4).
#[derive(Clone, Copy)]
struct FeedbackStyle {
    /// Resting background fill (a `palette.raised`-derived token).
    bg: Color,
    /// Resting border color (a `palette.border`-derived token).
    border: Color,
    /// Label text color.
    text_color: Color,
    /// The accent the animated press ring blends toward (`palette.accent`).
    accent: Color,
    /// Corner radius (px) — matches the control's resting radius.
    radius: f32,
    /// Hover brighten factor (e.g. `1.1`); press darkens by the same delta.
    hover_factor: f32,
}

/// MOTION-FEEDBACK — translate the shared feedback vocabulary into a render-time
/// `button::Style` for one Overview control. `status` is iced's hit-test state;
/// `now`/`reduce_motion` resolve the motion; `s` carries the palette tokens that
/// single-source the colors (§4 — the elevation comes from the `shadows` tokens,
/// the accent ring from `palette.accent`).
fn feedback_button_style(
    status: cosmic::iced::widget::button::Status,
    s: FeedbackStyle,
    now: Instant,
    reduce_motion: bool,
) -> cosmic::iced::widget::button::Style {
    use cosmic::iced::widget::button::Status as BtnStatus;
    let FeedbackStyle {
        bg,
        border,
        text_color,
        accent,
        radius,
        hover_factor,
    } = s;

    // iced's discrete hit-test status is the render-time truth: Hovered = pointer
    // over, Pressed = pointer down (fires on the DOWN edge, no input delay).
    let pressed = matches!(status, BtnStatus::Pressed);
    let hovered = matches!(status, BtnStatus::Hovered) || pressed;

    // Drive the shared vocabulary so the reduce-motion contract + the animated
    // accent ring resolve from ONE place. This is a render-time consumer (no panel
    // tick re-renders), so each render shows the SETTLED endpoint of the state: we
    // backdate an active state's `since` past its tween so it reads fully lifted /
    // fully ringed at `now`; an inactive state keeps `ControlFeedback::new()`'s
    // settled backdate so it is exactly at rest. The vocabulary still owns the one
    // thing that varies with `reduce_motion`: whether the elevation may move
    // (`is_at_rest`) and whether the ring animates or snaps.
    let settled = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
    let mut fb = ControlFeedback::new();
    if hovered {
        fb = fb.hovered(true, settled);
    }
    // The pressed control is the engaged/active item; with no keyboard-focus style
    // path on these buttons, the accent ring marks the active item via the
    // feedback module's focus-ring helper (MOTION-FEEDBACK-2).
    if pressed {
        fb = fb.focused(true, settled);
    }
    let geom: FeedbackParams = fb.params(now, reduce_motion);
    let ring = fb.focus_ring(now, reduce_motion);

    // Background tint: hover brightens (rises), press darkens (sinks). The color
    // swap is the reduce-motion-safe cue, so it keys off the discrete status, not
    // the (motion-gated) geometry.
    let fill = if pressed {
        tint(bg, (2.0 - hover_factor).max(0.0)) // darken by the hover delta
    } else if hovered {
        tint(bg, hover_factor)
    } else {
        bg
    };

    // Elevation conveys hover-lift / press-depress. `geom.is_at_rest()` is the
    // vocabulary's reduce-motion gate: true under reduce-motion (movement dropped
    // ⇒ shadow stays flat) OR when genuinely at rest. With motion on, a HOVERED
    // (not pressed) control rises to the raised ambient shadow; a PRESSED control
    // sinks — its elevation collapses to flat (losing the shadow is the depress).
    let shadow = if hovered && !pressed && !geom.is_at_rest() {
        let raised = mde_theme::shadows::Shadow::raised();
        Shadow {
            color: raised.color.into_cosmic_color(),
            offset: Vector::new(raised.offset_x, raised.offset_y),
            blur_radius: raised.blur,
        }
    } else {
        // Rest, pressed (sunk), or reduce-motion: flat, no elevation.
        cosmic::iced::Shadow::default()
    };

    // Animated accent ring on the engaged control: the border blends base→accent
    // by the ring alpha (grows in with motion, snaps full under reduce-motion).
    // Alpha is carried from the resting border token so a translucent border isn't
    // forced opaque on press.
    let (border_color, border_width) = if ring.is_visible() {
        let a = ring.alpha.clamp(0.0, 1.0);
        let blended = Color {
            r: border.r + (accent.r - border.r) * a,
            g: border.g + (accent.g - border.g) * a,
            b: border.b + (accent.b - border.b) * a,
            a: border.a + (accent.a - border.a) * a,
        };
        (blended, 1.0 + ring.width)
    } else {
        (border, 1.0)
    };

    cosmic::iced::widget::button::Style {
        snap: false,
        background: Some(Background::Color(fill)),
        text_color,
        icon_color: None,
        border: Border {
            color: border_color,
            width: border_width,
            radius: radius.into(),
        },
        border_color,
        border_width,
        border_radius: radius.into(),
        shadow,
    }
}

fn icon_widget<'a>(icon: Icon, size: IconSize, color: Color) -> Element<'a, crate::Message, Theme> {
    let resolved = mde_icon(icon, size);
    if let Some(svg_bytes) = resolved.svg_bytes() {
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(resolved.size_px()))
            .height(Length::Fixed(resolved.size_px()))
            .sty(move |_t: &Theme| widget_svg::Style { color: Some(color) })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(resolved.size_px())
            .colr(color)
            .into()
    }
}

fn capability_card<'a>(
    row_data: &'a CapabilityRow,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let icon = icon_widget(
        row_data.icon,
        IconSize::PanelHeader,
        palette.text.into_cosmic_color(),
    );
    let name = text(row_data.name)
        .size(16)
        .colr(palette.text.into_cosmic_color());
    let description = text(row_data.description)
        .size(13)
        .colr(palette.text_muted.into_cosmic_color());
    let sub_status: Element<'_, crate::Message, Theme> = match row_data
        .status
        .detail()
        .map(str::to_string)
        .or_else(|| row_data.sub_status.clone())
    {
        Some(s) => text(s)
            .size(12)
            .colr(palette.text_muted.into_cosmic_color())
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
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let bottom_row = row![sub_status, Space::new().width(Length::Fill), jump]
        .align_y(cosmic::iced::alignment::Vertical::Center);

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(column![top_row, Space::new().height(Length::Fixed(8.0)), bottom_row].spacing(0))
        .padding(Padding::from([16u16, 16u16]))
        .width(Length::Fill)
        .sty(move |_t: &Theme| cosmic::iced::widget::container::Style {
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

fn status_pill(status: &CapabilityStatus, _palette: Palette) -> Element<'_, crate::Message, Theme> {
    let color = status.color();
    let label = status.label();
    row![
        icon_widget(status.icon(), IconSize::Inline, color),
        Space::new().width(Length::Fixed(4.0)),
        text(label).size(12).colr(color),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

fn jump_button<'a>(
    row_data: &'a CapabilityRow,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let text_color = palette.text.into_cosmic_color();
    let accent = palette.accent.into_cosmic_color();
    let reduce_motion = crate::live_theme::reduce_motion();
    let style = move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
        feedback_button_style(
            status,
            FeedbackStyle {
                bg,
                border,
                text_color,
                accent,
                radius: 6.0,
                hover_factor: 1.12,
            },
            Instant::now(),
            reduce_motion,
        )
    };

    if row_data.jump.is_some() {
        // MESH-CONNECT-DIALOG-1 — Configure opens the progress modal that
        // re-probes this capability's real status (no silent jump); the modal's
        // own actions take it from there.
        let id = row_data.id;
        button(text("Configure  ▸").size(13))
            .padding(Padding::from([6u16, 14u16]))
            .sty(style)
            .on_press(crate::Message::Home(Message::ConfigureClicked(id)))
            .into()
    } else if let Some(bin) = row_data.launch {
        button(text("Set up  ▸").size(13))
            .padding(Padding::from([6u16, 14u16]))
            .sty(style)
            .on_press(crate::Message::LaunchApp(bin))
            .into()
    } else {
        column![].into()
    }
}

fn refresh_button<'a>(palette: Palette) -> Element<'a, crate::Message, Theme> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let text_color = palette.text.into_cosmic_color();
    let accent = palette.accent.into_cosmic_color();
    let reduce_motion = crate::live_theme::reduce_motion();
    button(text("Refresh").size(12))
        .padding(Padding::from([4u16, 12u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                feedback_button_style(
                    status,
                    FeedbackStyle {
                        bg,
                        border,
                        text_color,
                        accent,
                        radius: 6.0,
                        hover_factor: 1.12,
                    },
                    Instant::now(),
                    reduce_motion,
                )
            },
        )
        .on_press(crate::Message::Home(Message::RefreshClicked))
        .into()
}

fn mackesd_banner<'a>(palette: Palette) -> Element<'a, crate::Message, Theme> {
    let yellow = palette.warning.into_cosmic_color();
    let bg = palette.raised.into_cosmic_color();
    container(
        row![
            icon_widget(Icon::StatusWarning, IconSize::PanelHeader, yellow),
            Space::new().width(Length::Fixed(8.0)),
            text("mackesd is not responding — capability statuses may be stale.")
                .size(13)
                .colr(palette.text.into_cosmic_color()),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([10u16, 14u16]))
    .width(Length::Fill)
    .sty(move |_t: &Theme| cosmic::iced::widget::container::Style {
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
) -> Element<'a, crate::Message, Theme> {
    let icon = icon_widget(
        icon,
        IconSize::PanelHeader,
        palette.text_muted.into_cosmic_color(),
    );
    let value_display = match value {
        Some(n) => n.to_string(),
        None => "—".into(),
    };
    let value_text = text(value_display)
        .size(28)
        .colr(palette.text.into_cosmic_color());
    let label_text = text(label.to_string())
        .size(12)
        .colr(palette.text_muted.into_cosmic_color());
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
    .align_x(cosmic::iced::alignment::Horizontal::Left);

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let muted_text = palette.text_muted.into_cosmic_color();
    let accent = palette.accent.into_cosmic_color();
    let reduce_motion = crate::live_theme::reduce_motion();
    button(card)
        .width(Length::Fill)
        .padding(Padding::from([16u16, 16u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                feedback_button_style(
                    status,
                    FeedbackStyle {
                        bg,
                        border,
                        text_color: muted_text,
                        accent,
                        radius: 8.0,
                        hover_factor: 1.08,
                    },
                    Instant::now(),
                    reduce_motion,
                )
            },
        )
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
// BOOT-STATUS live subscription
// ---------------------------------------------------------------------------

/// The bus topic the mackesd boot_readiness worker publishes to.
const BOOT_READINESS_TOPIC: &str = "state/boot-readiness";

/// Determine the collapsed-vs-expanded view state from a [`BootReadiness`]
/// snapshot. Returns `true` (collapsed / "all-green chip") when every step in
/// the chain is `ok` AND `ready` is true. Returns `false` (expanded detail
/// view) when any step is not `ok`, or when there is a regression (`ready`
/// flips back to false). An empty step list (mid-boot / unknown) is also
/// treated as expanded so the placeholder is visible.
///
/// This is a pure function so it can be unit-tested without spinning up Iced.
#[must_use]
pub fn boot_status_collapsed(br: &BootReadiness) -> bool {
    br.ready && br.steps.iter().all(|s| s.status == "ok")
}

/// Iced subscription that tails `state/boot-readiness` on the mesh bus and
/// emits [`Message::BootReadinessUpdate`] whenever mackesd publishes a new
/// snapshot. The poll interval is 500 ms so step transitions are surfaced
/// sub-second. The subscription is idle (not registered) when the home panel
/// is not shown — the caller (app::subscription) gates it on the active view.
///
/// The cursor is seeded at the latest existing entry on first connect so only
/// snapshots published **after** the panel opens trigger repaints (the
/// initial state is already loaded by [`HomeSnapshot::load_sync`]).
#[must_use]
pub fn boot_readiness_subscription() -> cosmic::iced::Subscription<crate::Message> {
    use cosmic::iced::futures::SinkExt;
    use cosmic::iced::stream;
    cosmic::iced::Subscription::run(|| {
        stream::channel(
            4,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<crate::Message>| async move {
                let mut cursor = boot_readiness_cursor_init().await;
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let (updates, next) = boot_readiness_poll(cursor.clone()).await;
                    cursor = next;
                    for br in updates {
                        let _ = output
                            .send(crate::Message::Home(Message::BootReadinessUpdate(br)))
                            .await;
                    }
                }
            },
        )
    })
}

/// Seed the cursor at the latest existing `state/boot-readiness` ulid so the
/// subscription only reacts to entries written **after** it starts.
/// Returns `None` when the bus is unavailable (topic empty / mid-boot).
async fn boot_readiness_cursor_init() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let dir = mde_bus::client_data_dir()?;
        let persist = mde_bus::persist::Persist::open(dir).ok()?;
        let msgs = persist.list_since(BOOT_READINESS_TOPIC, None).ok()?;
        msgs.last().map(|m| m.ulid.clone())
    })
    .await
    .ok()
    .flatten()
}

/// Poll `state/boot-readiness` since `cursor`; decode each new entry into a
/// [`BootReadiness`] value and return them together with the advanced cursor.
/// Bus unavailable → empty list, cursor unchanged.
async fn boot_readiness_poll(
    cursor: Option<String>,
) -> (Vec<BootReadiness>, Option<String>) {
    let fallback = cursor.clone();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = mde_bus::client_data_dir() else {
            return (Vec::new(), cursor);
        };
        let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
            return (Vec::new(), cursor);
        };
        let msgs = persist
            .list_since(BOOT_READINESS_TOPIC, cursor.as_deref())
            .unwrap_or_default();
        let mut next = cursor;
        let mut updates = Vec::new();
        for m in msgs {
            next = Some(m.ulid);
            if let Some(body) = m.body {
                updates.push(parse_boot_readiness(&body));
            }
        }
        (updates, next)
    })
    .await
    .unwrap_or((Vec::new(), fallback))
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
pub fn dbus_subscription() -> cosmic::iced::Subscription<crate::Message> {
    use cosmic::iced::stream;
    cosmic::iced::Subscription::run(|| {
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
pub fn nebula_event_subscription() -> cosmic::iced::Subscription<crate::Message> {
    use cosmic::iced::futures::SinkExt;
    use cosmic::iced::stream;
    cosmic::iced::Subscription::run(|| {
        stream::channel(
            32,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<crate::Message>| async move {
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
        let dir = mde_bus::client_data_dir()?;
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
        let Some(dir) = mde_bus::client_data_dir() else {
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
    output: &mut cosmic::iced::futures::channel::mpsc::Sender<crate::Message>,
) -> Result<(), String> {
    use cosmic::iced::futures::SinkExt;
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
    use cosmic::iced::futures::StreamExt;
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
    fn parse_boot_readiness_reads_ready_and_steps() {
        let snap = r#"{"ok":true,"ready":false,"ts_ms":9,"steps":[
            {"id":"nebula","label":"Nebula overlay","status":"ok","detail":"up"},
            {"id":"qnm","label":"QNM-Shared mounted","status":"pending","detail":"not mounted"},
            {"id":"directory","label":"Peer directory replicated","status":"blocked","detail":"0 peer(s)","blocked_by":"qnm"}
        ],
        "services":[
            {"id":"musicd","label":"Music daemon","active":true,"reachable":null,"status":"ok"},
            {"id":"netdata","label":"Live metrics","active":false,"reachable":false,"status":"down"}
        ],
        "pings":[
            {"peer":"lh-01","overlay_ip":"10.42.0.1","role":"lighthouse","rtt_ms":3.2,"reachable":true},
            {"peer":"anvil","overlay_ip":"","role":"peer","rtt_ms":null,"reachable":false}
        ]}"#;
        let r = parse_boot_readiness(snap);
        assert!(!r.ready);
        assert_eq!(r.steps.len(), 3);
        assert_eq!(r.steps[0].label, "Nebula overlay");
        assert_eq!(r.steps[0].status, "ok");
        assert_eq!(r.steps[1].status, "pending");
        assert_eq!(r.steps[2].detail, "0 peer(s)");
        // BOOT-STATUS-2 — services + pings decode too.
        assert_eq!(r.services.len(), 2);
        assert_eq!(r.services[0].id, "musicd");
        assert_eq!(r.services[0].status, "ok");
        assert_eq!(r.services[1].status, "down");
        assert_eq!(r.pings.len(), 2);
        assert_eq!(r.pings[0].role, "lighthouse");
        assert_eq!(r.pings[0].rtt_ms, Some(3.2));
        assert_eq!(r.pings[1].rtt_ms, None);
        // BOOT-PEERS-1 — fabric still converging (a non-directory step pending).
        assert!(r.fabric_converging());
        // ready snapshot + garbage.
        assert!(parse_boot_readiness(r#"{"ready":true,"steps":[]}"#).ready);
        assert_eq!(parse_boot_readiness("nope"), BootReadiness::default());
    }

    #[test]
    fn service_remediation_maps_known_daemons() {
        // BOOT-STATUS-6 — musicd is a per-user unit (no pkexec); netdata + the
        // in-process KDE Connect listener (mackesd) are system units.
        assert_eq!(service_remediation("musicd"), Some(("mde-musicd", true)));
        assert_eq!(service_remediation("netdata"), Some(("netdata", false)));
        assert_eq!(service_remediation("kdc"), Some(("mackesd", false)));
        assert_eq!(service_remediation("nope"), None);
    }

    #[test]
    fn boot_popup_suppresses_only_when_ready() {
        // BOOT-STATUS-5 — suppress the auto-popup iff it's the boot-popup launch
        // AND the mesh is already all-green.
        assert!(boot_popup_should_suppress(true, true)); // ready → no window
        assert!(!boot_popup_should_suppress(true, false)); // converging → open
        assert!(!boot_popup_should_suppress(false, true)); // normal launch → open
        assert!(!boot_popup_should_suppress(false, false));
    }

    #[test]
    fn fabric_converging_distinguishes_settling_from_lone_node() {
        // BOOT-PEERS-1 — fabric up but no peers (lone healthy node) is NOT
        // converging (the genuine empty state should show).
        let lone = r#"{"ready":false,"steps":[
            {"id":"nebula","label":"Nebula overlay","status":"ok"},
            {"id":"overlay-ip","label":"Overlay IP assigned","status":"ok"},
            {"id":"mackesd","label":"mackesd serving","status":"ok"},
            {"id":"bus","label":"Message bus broker","status":"ok"},
            {"id":"qnm","label":"QNM-Shared mounted","status":"ok"},
            {"id":"directory","label":"Peer directory replicated","status":"pending"}
        ]}"#;
        assert!(!parse_boot_readiness(lone).fabric_converging());
        // No snapshot at all → not converging (we have no evidence of mid-boot).
        assert!(!BootReadiness::default().fabric_converging());
    }

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
        assert_eq!(files.jump, Some((Group::Mesh, "mesh_storage")));
        assert!(files.launch.is_none());
        assert_eq!(phone.jump, Some((Group::Mesh, "connect")));
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
        // PLANES-1 — capability jumps follow the panels into their planes.
        assert_eq!(
            lookup(CapabilityId::Mesh),
            Some((Group::Mesh, "mesh_control"))
        );
        assert_eq!(lookup(CapabilityId::Peers), Some((Group::Mesh, "peers")));
        assert_eq!(
            lookup(CapabilityId::Files),
            Some((Group::Mesh, "mesh_storage"))
        );
        assert_eq!(lookup(CapabilityId::Phone), Some((Group::Mesh, "connect")));
        assert_eq!(
            lookup(CapabilityId::Ssh),
            Some((Group::ThisNode, "remote_desktop"))
        );
        assert_eq!(
            lookup(CapabilityId::Rdp),
            Some((Group::ThisNode, "remote_desktop"))
        );
        assert_eq!(
            lookup(CapabilityId::Vnc),
            Some((Group::ThisNode, "remote_desktop"))
        );
        assert_eq!(
            lookup(CapabilityId::Services),
            Some((Group::ThisNode, "mesh_services"))
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
    fn extract_head_version_reads_the_fpg4_reply() {
        let raw = r#"{"ok":true,"head":7,"revisions":[{"version":7,"author":"peer:pine","at":1}]}"#;
        assert_eq!(extract_head_version(raw), Some(7));
    }

    #[test]
    fn extract_head_version_none_for_empty_log_or_errors() {
        assert_eq!(
            extract_head_version(r#"{"ok":true,"head":null,"revisions":[]}"#),
            None
        );
        assert_eq!(extract_head_version(r#"{"ok":false,"error":"x"}"#), None);
        assert_eq!(extract_head_version("not json"), None);
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
    fn feedback_button_style_lifts_on_hover_and_sinks_on_press() {
        // MOTION-FEEDBACK-1/2 — the shared control feedback maps iced's discrete
        // hit-test status onto hover-lift + press-depress + animated accent.
        use cosmic::iced::widget::button::Status as BtnStatus;
        let now = Instant::now();
        let bg = Color::from_rgb(0.2, 0.2, 0.2); // carbon-ok: test fixture (not a render-path token)
        let border = Color::from_rgb(0.4, 0.4, 0.4); // carbon-ok: test fixture (not a render-path token)
        let fg = Color::WHITE;
        let accent = Color::from_rgb(0.2, 0.4, 1.0); // carbon-ok: test fixture (not a render-path token)
        let style = FeedbackStyle {
            bg,
            border,
            text_color: fg,
            accent,
            radius: 6.0,
            hover_factor: 1.1,
        };
        let mk = |status, reduce| feedback_button_style(status, style, now, reduce);

        // At rest: flat (no elevation), base border, base fill.
        let rest = mk(BtnStatus::Active, false);
        assert_eq!(rest.shadow.offset, Vector::new(0.0, 0.0));
        assert!((rest.border.width - 1.0).abs() < 1e-6, "no ring at rest");

        // Hovered (full motion): raised elevation shadow + brighter fill. The
        // shadow color is the `shadows::raised()` token (its real alpha), not a
        // hardcoded literal (§4).
        let hov = mk(BtnStatus::Hovered, false);
        assert!(hov.shadow.offset.y > 0.0, "hover lifts → ambient shadow");
        let raised = mde_theme::shadows::Shadow::raised();
        assert!(
            (hov.shadow.color.a - raised.color.a).abs() < 1e-6,
            "hover shadow uses the raised token alpha"
        );
        if let Some(Background::Color(c)) = hov.background {
            assert!(c.r > bg.r, "hover brightens the tint");
        } else {
            panic!("expected a solid hover fill");
        }

        // Pressed: the surface SINKS — the elevation collapses to flat (losing the
        // shadow is the depress cue, NOT a bigger shadow), the fill darkens, and
        // the animated accent ring grows onto the border (engaged/active item).
        let pressed = mk(BtnStatus::Pressed, false);
        assert_eq!(
            pressed.shadow.offset,
            Vector::new(0.0, 0.0),
            "press sinks → elevation collapses to flat (no raised shadow)"
        );
        if let Some(Background::Color(c)) = pressed.background {
            assert!(c.r < bg.r, "press darkens the tint (sinks)");
        } else {
            panic!("expected a solid press fill");
        }
        assert!(
            pressed.border.width >= 1.0 && pressed.border.color.b > border.b,
            "press grows an accent ring (border blends toward accent)"
        );
    }

    #[test]
    fn feedback_button_style_reduce_motion_keeps_state_drops_movement() {
        // Q32: under reduce-motion the STATE change is kept (color tokens swap,
        // the accent ring is present at full) but the MOVEMENT is dropped (the
        // elevation shadow never lifts).
        use cosmic::iced::widget::button::Status as BtnStatus;
        let now = Instant::now();
        let bg = Color::from_rgb(0.2, 0.2, 0.2); // carbon-ok: test fixture (not a render-path token)
        let border = Color::from_rgb(0.4, 0.4, 0.4); // carbon-ok: test fixture (not a render-path token)
        let accent = Color::from_rgb(0.2, 0.4, 1.0); // carbon-ok: test fixture (not a render-path token)
        let style = FeedbackStyle {
            bg,
            border,
            text_color: Color::WHITE,
            accent,
            radius: 6.0,
            hover_factor: 1.1,
        };

        // Hover under reduce-motion: brighter fill (state kept) but NO elevation.
        let hov = feedback_button_style(BtnStatus::Hovered, style, now, true);
        assert_eq!(
            hov.shadow.offset,
            Vector::new(0.0, 0.0),
            "no lift movement under reduce-motion"
        );
        if let Some(Background::Color(c)) = hov.background {
            assert!(c.r > bg.r, "hover color state is still kept");
        }

        // Press under reduce-motion: accent ring present at FULL width immediately
        // (snaps, doesn't grow) — state kept, animation dropped.
        let pressed = feedback_button_style(BtnStatus::Pressed, style, now, true);
        assert_eq!(
            pressed.shadow.offset,
            Vector::new(0.0, 0.0),
            "no depress movement under reduce-motion"
        );
        assert!(
            pressed.border.color.b > border.b,
            "accent ring present (full) under reduce-motion"
        );
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

    #[test]
    fn capability_jump_matches_the_row_builders() {
        // MESH-CONNECT-DIALOG-1 — the connect modal's "Open settings ▸" target
        // (`capability_jump`) MUST agree with each row's own Configure deep-link
        // (`CapabilityRow::jump`), or the modal would send the operator to a
        // different panel than the row's button. Lock them together so a future
        // row-target edit that forgets `capability_jump` fails here.
        for row in build_all_rows_with_unknown_status() {
            assert_eq!(
                capability_jump(row.id),
                row.jump,
                "capability_jump({:?}) drifted from the row builder's jump target",
                row.id
            );
        }
    }

    #[test]
    fn capability_label_matches_the_row_builders() {
        // The modal title reuses `capability_label`; keep it equal to the row's
        // own `name` so the dialog and the row agree on what's being configured.
        for row in build_all_rows_with_unknown_status() {
            assert_eq!(
                capability_label(row.id),
                row.name,
                "label drift for {:?}",
                row.id
            );
        }
    }

    // -----------------------------------------------------------------------
    // BOOT-STATUS: all-green collapse + regression re-expand logic
    // -----------------------------------------------------------------------

    fn make_step(id: &str, status: &str) -> BootStep {
        BootStep {
            id: id.to_string(),
            label: id.to_string(),
            status: status.to_string(),
            detail: String::new(),
        }
    }

    #[test]
    fn boot_status_collapsed_all_green_collapses() {
        // When ready=true and every step is "ok", the panel should collapse
        // to the glanceable green chip.
        let br = BootReadiness {
            ready: true,
            steps: vec![
                make_step("nebula", "ok"),
                make_step("overlay-ip", "ok"),
                make_step("mackesd", "ok"),
                make_step("bus", "ok"),
                make_step("qnm", "ok"),
                make_step("directory", "ok"),
            ],
            services: vec![],
            pings: vec![],
        };
        assert!(
            boot_status_collapsed(&br),
            "all-ok steps + ready=true must collapse to chip"
        );
    }

    #[test]
    fn boot_status_collapsed_pending_step_expands() {
        // Any non-ok step keeps the detail view expanded even when ready=true.
        let br = BootReadiness {
            ready: true,
            steps: vec![
                make_step("nebula", "ok"),
                make_step("qnm", "pending"),
            ],
            services: vec![],
            pings: vec![],
        };
        assert!(
            !boot_status_collapsed(&br),
            "a pending step must expand the detail view"
        );
    }

    #[test]
    fn boot_status_collapsed_regression_re_expands() {
        // When ready flips back to false (regression), the detail view
        // must re-expand even if all individual steps happened to be "ok".
        let br = BootReadiness {
            ready: false,
            steps: vec![
                make_step("nebula", "ok"),
                make_step("qnm", "ok"),
            ],
            services: vec![],
            pings: vec![],
        };
        assert!(
            !boot_status_collapsed(&br),
            "ready=false (regression) must re-expand the detail view"
        );
    }

    #[test]
    fn boot_status_collapsed_empty_steps_expands() {
        // An empty step list (mid-boot / bus unavailable) must show the
        // expanded placeholder rather than collapsing to the chip.
        let br = BootReadiness::default();
        assert!(
            !boot_status_collapsed(&br),
            "unknown/empty state must show expanded view"
        );
    }

    #[test]
    fn boot_readiness_update_applies_live_snapshot() {
        // The BootReadinessUpdate handler must overwrite boot fields in-place
        // while leaving capabilities + mackesd_reachable untouched.
        let mut panel = HomePanel::new();
        panel.snapshot.boot_ready = false;
        panel.snapshot.boot_steps = vec![make_step("nebula", "pending")];
        panel.snapshot.mackesd_reachable = false; // must survive the update

        let fresh = BootReadiness {
            ready: true,
            steps: vec![
                make_step("nebula", "ok"),
                make_step("qnm", "ok"),
            ],
            services: vec![],
            pings: vec![BootPing {
                peer: "lh-01".to_string(),
                role: "lighthouse".to_string(),
                rtt_ms: Some(4.5),
            }],
        };
        let _ = panel.update(Message::BootReadinessUpdate(fresh));

        assert!(panel.snapshot.boot_ready, "boot_ready must be updated");
        assert_eq!(panel.snapshot.boot_steps.len(), 2);
        assert_eq!(panel.snapshot.boot_pings.len(), 1);
        assert_eq!(panel.snapshot.mesh_peers, Some(1), "ping count seeds hero tile");
        // Unrelated field must be preserved.
        assert!(
            !panel.snapshot.mackesd_reachable,
            "mackesd_reachable must not be touched by BootReadinessUpdate"
        );
    }
}

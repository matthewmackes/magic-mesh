//! PD-3 — the **Peers** directory panel: the platform Front Door.
//!
//! Master-detail over `action/mesh/directory` (PD-1): peer list left
//! — self pinned "(this machine)" first, then Online, Idle, Offline
//! (grayed) groups — detail pane right with the identity header,
//! presence + health, revision currency + voice presence, and the
//! Services Provided inventory (PD-2 descriptors: remote access, Podman
//! containers, libvirt guests, media). A type-to-filter box matches
//! hostname or offered service (L2). Degraded mesh states render the
//! guided empty states (L3): no mackesd → "Start the mesh service",
//! empty roster → "Invite a peer".
//!
//! Per-peer ops (Call/SSH/RDP/VNC, PD-5), tag chips (L1 — tags join
//! the directory record with the tag-manifest merge) and the live map
//! (PD-7) layer onto this surface. L6 adds the **Devices** group: the
//! paired KDE-Connect roster (`action/connect/devices`) renders below
//! the peers with presence + battery, Ring + Send-file (the live
//! Connect verbs), and a jump to the KDC hub.

use std::time::Duration;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::Task;
use cosmic::iced::{Background, Border, Length, Padding};
use cosmic::Element;
use mde_theme::{carbon, FontSize, FontWeight, Palette, Rgba, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::status_strip::{mono_text, pip};

/// One row of the directory reply, parsed leniently.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeerRow {
    pub hostname: String,
    pub presence: String,
    pub health: String,
    pub version: String,
    pub overlay_ip: String,
    pub role: String,
    pub currency: String,
    /// PEERS-DT — last-seen wall-clock (epoch ms) from the directory record;
    /// drives the "Last seen" column + its sort. `0` when the field is absent.
    pub last_seen_ms: i64,
    /// L1 — the peer's capability tags (`hop`/`execution`/`headless`)
    /// from the directory record. Rendered as chips in the detail pane
    /// and folded into the filter; empty when the peer has none.
    pub tags: Vec<String>,
    /// Flattened "what this peer offers" lines for the detail pane +
    /// the service filter (L2).
    pub services: Vec<String>,
    /// PD-5 op gates from the descriptors (false when unpublished).
    pub ssh: bool,
    pub rdp: bool,
    pub vnc: bool,
    /// PD-12 — the peer's published LAN MACs (Wake-on-LAN targets).
    pub lan_macs: Vec<String>,
    /// PD-11 — structured (name, state) for the lifecycle buttons.
    pub containers: Vec<(String, String)>,
    pub vms: Vec<(String, String)>,
}

/// PD-3/L6 — one paired KDE-Connect device as the directory renders it,
/// parsed from the `action/connect/devices` roster (the daemon's
/// `WireDevice`: `{id, name, online, battery}`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub battery: Option<u8>,
}

/// UNIFY-16 — the LOCAL node's voice (SIP) presence, parsed from the
/// `state/voice/status` snapshot the running `mde-voice-hud --agent` publishes
/// to the Bus (VOIP-28). The topic is this node's own agent heartbeat — there
/// is no per-peer voice in the directory — so it is rendered only for the self
/// row; every other peer shows an honest "—" (never a fabricated value).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceStatus {
    /// The agent is registered with its SIP server.
    pub registered: bool,
    /// The agent is listening for inbound (P2P / registrar) calls.
    pub listening: bool,
    /// The heartbeat `ts` is within [`VOICE_STALE_SECS`] (the agent is live).
    /// A stale snapshot means the agent stopped publishing (not running).
    pub fresh: bool,
}

/// PEERS-DT — the sortable columns of the Carbon data table. Default sort is
/// `Status` (online first) then Name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    Name,
    #[default]
    Status,
    Role,
    OverlayIp,
    Latency,
    Services,
    LastSeen,
}

impl SortColumn {
    /// Column header label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Status => "Status",
            Self::Role => "Role",
            Self::OverlayIp => "Overlay IP",
            Self::Latency => "Latency",
            Self::Services => "Services",
            Self::LastSeen => "Last seen",
        }
    }
}

/// PEERS-DT — presence → sort rank (online first, offline last) for the default
/// Status sort.
#[must_use]
pub fn presence_rank(presence: &str) -> u8 {
    match presence {
        "online" => 0,
        "idle" => 1,
        _ => 2, // offline / unknown
    }
}

/// Panel state.
#[derive(Debug, Clone, Default)]
pub struct PeersPanel {
    pub rows: Vec<PeerRow>,
    pub filter: String,
    /// PEERS-DT — the host whose row is expanded inline (Carbon expandable row),
    /// replacing the old side detail pane. `None` = all rows collapsed.
    pub selected: Option<String>,
    /// PEERS-DT — active sort column + direction.
    pub sort: SortColumn,
    pub sort_asc: bool,
    /// `None` = loading; `Some(Err)` = mackesd unreachable (L3 state).
    pub loaded: Option<Result<(), String>>,
    pub self_hostname: String,
    /// PD-5 — the inline result strip under the op toolbar (Q22).
    pub op_result: String,
    /// PD-8 — live Netdata series for the selected peer (L14).
    pub metrics: Option<PeerMetrics>,
    pub metrics_err: Option<String>,
    /// PD-11/L16 — the armed stop/restart awaiting its second click:
    /// (host, kind, name, op).
    pub pending_confirm: Option<(String, String, String, String)>,
    /// PD-7 — List (master-detail) or Map (the live canvas).
    pub map_view: bool,
    /// PD-7 — host→RTT from the mesh-latency cache.
    pub rtt: std::collections::HashMap<String, Option<f64>>,
    /// NET-3 (PD-6/PD-7) — host→underlay path (direct/relay/overlay + endpoint
    /// + relay peer) from the Nebula debug-SSH hostmap, read from the same
    /// latency cache. Drives the trace card's path line.
    pub paths: std::collections::HashMap<String, super::peers_map::PathInfo>,
    /// PD-3/L6 — the paired KDE-Connect devices, rendered as their own
    /// "Devices" group in the master list (fetched from
    /// `action/connect/devices` alongside the directory).
    pub devices: Vec<DeviceRow>,
    /// PD-3/L6 — the selected device id (mutually exclusive with
    /// `selected`: a peer selection clears this and vice-versa). When set,
    /// the detail pane renders the device card instead of a peer.
    pub selected_device: Option<String>,
    /// PD-7/L19-20 — the peer whose self→peer edge trace card is open
    /// (Map view). `None` = no card.
    pub traced_edge: Option<String>,
    /// PD-7/L20 — the session RTT sparkline for the traced edge: samples
    /// accumulated (oldest→newest) while the card stays open. Cleared when
    /// the card opens on a different edge.
    pub trace_rtt: Vec<f64>,
    /// PD-7/L19 — the expandable underlay traceroute: `None` = collapsed /
    /// not yet run; `Some(Ok(hops))` / `Some(Err(why))` once it resolves.
    pub traceroute: Option<Result<Vec<String>, String>>,
    /// PD-7/L19 — a traceroute is in flight (button shows "Tracing…").
    pub traceroute_running: bool,
    /// PD-7/L18 + MESHMAP-6 — host→per-link flow (tx/rx, each 0.0..=1.0) for
    /// the flow particles, refreshed while the Map view is open. The source
    /// is the mackesd `link-traffic` collector's REAL per-link byte rates
    /// when its cache is present, falling back to the per-node Netdata
    /// `sample_flows` proxy (tx = the proxy busy-ness, rx = 0) otherwise.
    pub flows: std::collections::HashMap<String, super::peers_map::LinkFlow>,
    /// PD-7/L18 — the flow-particle animation phase (0.0..=1.0), advanced
    /// by the animation tick (registered only while real traffic flows).
    pub flow_phase: f32,
    /// BOOT-PEERS-1 — captured at load: the mesh fabric is still coming up
    /// (Nebula/overlay-IP/bus/QNM not all converged). When true AND the roster
    /// is empty, the panel shows a "peers settling…" state instead of the
    /// "empty mesh" guidance, so the multi-minute cold-boot warm-up doesn't look
    /// broken. Read from `state/boot-readiness` only when the roster is empty.
    pub boot_converging: bool,
    /// UNIFY-16 — the LOCAL node's voice (SIP) presence, read from the
    /// `state/voice/status` Bus snapshot alongside the directory. `None` until
    /// the first read resolves, or when no voice agent has ever published (an
    /// honest unknown). Rendered in the VOICE stat tile for the self row only.
    pub voice: Option<VoiceStatus>,
}

/// PD-8 — the four L14 series, oldest→newest over the last ~60 s.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeerMetrics {
    pub cpu: Vec<f64>,
    pub load: Vec<f64>,
    pub net: Vec<f64>,
    pub disk: Vec<f64>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<PeerRow>, String>),
    /// UNIFY-16 — the LOCAL node's voice (SIP) presence snapshot resolved
    /// (read from `state/voice/status` alongside the directory). `None` = the
    /// Bus is down or no agent has ever published (rendered as an honest "—").
    VoiceLoaded(Option<VoiceStatus>),
    FilterChanged(String),
    Select(String),
    /// PEERS-DT — sort the table by a column (re-click toggles direction).
    SortBy(SortColumn),
    RefreshClicked,
    /// PD-5 — launch a connection op against the selected peer.
    Op(crate::launcher::Protocol, String),
    /// PD-5 — a launch finished (spawned or failed).
    OpFinished {
        label: &'static str,
        host: String,
        ok: bool,
    },
    /// PD-5 — Call (voice): publish `action/voice/dial` so the voice HUD
    /// resolves the peer's extension and rings it.
    CallClicked(String),
    Called(String),
    /// PD-9 — "Apply now": nudge a behind peer to reconcile.
    NudgeClicked(String),
    NudgeFinished {
        host: String,
        ok: bool,
    },
    /// PD-12 — Wake-on-LAN an offline peer (first published MAC).
    WakeClicked {
        host: String,
        mac: String,
    },
    WakeFinished {
        host: String,
        ok: bool,
    },
    /// PD-7 — flip between the List and Map views.
    ToggleMap,
    /// PD-3/Q10 — the 30 s directory-refresh tick (app-level,
    /// view-gated). Re-reads the directory so presence/health/tags
    /// stay live without an operator click; the reload preserves the
    /// current filter + selection.
    PollTick,
    /// PD-8 — the 2 s metrics tick (app-level, view-gated).
    MetricsTick,
    /// PD-8 — open the peer's full Netdata dashboard in the browser.
    OpenDashboard(String),
    /// PD-8 — a metrics fetch resolved for `host`.
    MetricsLoaded {
        host: String,
        result: Result<PeerMetrics, String>,
    },
    /// PD-11 — a lifecycle button: start is one-click; stop/restart
    /// arm [`PeersPanel::pending_confirm`] first (L16).
    Lifecycle {
        host: String,
        kind: String,
        name: String,
        op: String,
    },
    /// PD-11 — the verb replied; begin polling for the result.
    LifecycleSent {
        host: String,
        id: Option<String>,
    },
    /// PD-11 — one result poll resolved.
    LifecyclePolled {
        host: String,
        id: String,
        attempts_left: u8,
        outcome: Option<Result<(), String>>,
    },
    /// PD-3/L6 — the paired-device roster resolved (fetched alongside the
    /// directory). Replaces the `devices` list; preserves the device
    /// selection if it's still present.
    DevicesLoaded(Vec<DeviceRow>),
    /// PD-3/L6 — a device row was clicked: select it (clears the peer
    /// selection) so the detail pane shows the device card.
    SelectDevice(String),
    /// PD-3/L6 — Ring a paired device (`action/connect/ring`).
    RingDevice {
        id: String,
        name: String,
    },
    /// PD-3/L6 — Send a file to a paired device: pick a file, then
    /// `action/connect/share`. `None` path = the picker was cancelled.
    SendFile {
        id: String,
        name: String,
    },
    /// PD-3/L6 — a Ring/Send-file verb resolved for `name`.
    DeviceActionFinished {
        name: String,
        verb: &'static str,
        ok: bool,
    },
    /// PD-7/L19-20 — a self→peer edge was clicked on the map: open its
    /// trace card (resets the session sparkline + the traceroute).
    EdgeClicked(String),
    /// PD-7/L19-20 — close the trace card.
    CloseTrace,
    /// PD-7/L20 — the ~2 s trace tick: re-probe the traced peer's overlay
    /// RTT and append it to the session sparkline.
    TraceTick,
    /// PD-7/L20 — one RTT sample resolved for the traced host.
    TraceRttSampled {
        host: String,
        rtt_ms: Option<f64>,
    },
    /// PD-7/L19 — run the expandable underlay traceroute for the traced peer.
    RunTraceroute(String),
    /// PD-7/L19 — the traceroute resolved (hops or an honest error).
    TracerouteDone {
        host: String,
        hops: Result<Vec<String>, String>,
    },
    /// PD-7/L18 — the flow-data tick (~3 s): re-sample each online peer's
    /// overlay throughput for the particle density.
    FlowTick,
    /// PD-7/L18 — the sampled per-host flow resolved.
    FlowsSampled(std::collections::HashMap<String, super::peers_map::LinkFlow>),
    /// PD-7/L18 — the animation tick: advance the particle phase.
    FlowAnim,
}

/// FRONTDOOR-4 — fetch + parse the live mesh directory in one blocking call,
/// reusing the same `action/mesh/directory` Bus RPC + [`parse_directory`] the
/// panel's own [`PeersPanel::load`] drives. Returns the parsed rows, or `None`
/// on an unreachable / not-ok / unparseable reply (the caller treats that as "no
/// data" — it never fabricates a roster). MUST be called OUTSIDE an async
/// runtime (it wraps the synchronous [`crate::dbus::action_request`]); the Front
/// Door's loader runs it on a `spawn_blocking` thread. Kept here so there is one
/// directory reader (§6 glue), shared by the Peers panel and the Front Door.
#[must_use]
pub fn action_directory() -> Option<Vec<PeerRow>> {
    let raw = crate::dbus::action_request("action/mesh/directory", Duration::from_secs(2))?;
    parse_directory(&raw).ok()
}

/// Parse the PD-1 directory JSON into rows (pure, testable).
#[must_use]
pub fn parse_directory(raw: &str) -> Result<Vec<PeerRow>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad directory reply: {e}"))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(v
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("directory verb replied not-ok")
            .to_string());
    }
    let rows = v
        .get("peers")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let hostname = p.get("hostname")?.as_str()?.to_string();
                    let gs = |k: &str| {
                        p.get(k)
                            .and_then(|x| x.as_str())
                            .unwrap_or_default()
                            .to_string()
                    };
                    // L1 — capability tags from the directory record.
                    let tags: Vec<String> = p
                        .get("tags")
                        .and_then(|t| t.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut services = Vec::new();
                    let (mut ssh, mut rdp, mut vnc) = (false, false, false);
                    let mut lan_macs: Vec<String> = Vec::new();
                    let mut containers: Vec<(String, String)> = Vec::new();
                    let mut vms: Vec<(String, String)> = Vec::new();
                    if let Some(d) = p.get("descriptors").filter(|d| !d.is_null()) {
                        let ra = &d["remote_access"];
                        for (label, key, flag) in [
                            ("SSH", "ssh", &mut ssh),
                            ("RDP", "rdp", &mut rdp),
                            ("VNC", "vnc", &mut vnc),
                        ] {
                            if ra.get(key).and_then(serde_json::Value::as_bool) == Some(true) {
                                services.push(label.to_string());
                                *flag = true;
                            }
                        }
                        for c in d["containers"].as_array().into_iter().flatten() {
                            containers.push((
                                c["name"].as_str().unwrap_or("?").to_string(),
                                c["state"].as_str().unwrap_or("?").to_string(),
                            ));
                            services.push(format!(
                                "podman: {} ({}) {}",
                                c["name"].as_str().unwrap_or("?"),
                                c["state"].as_str().unwrap_or("?"),
                                c["image"].as_str().unwrap_or(""),
                            ));
                        }
                        for vm in d["vms"].as_array().into_iter().flatten() {
                            vms.push((
                                vm["name"].as_str().unwrap_or("?").to_string(),
                                vm["state"].as_str().unwrap_or("?").to_string(),
                            ));
                            let specs = match (vm["vcpus"].as_u64(), vm["memory_mb"].as_u64()) {
                                (Some(c), Some(m)) => format!(" · {c} vCPU / {m} MiB"),
                                _ => String::new(),
                            };
                            services.push(format!(
                                "kvm: {} ({}){specs}",
                                vm["name"].as_str().unwrap_or("?"),
                                vm["state"].as_str().unwrap_or("?"),
                            ));
                        }
                        for mac in d["lan_macs"].as_array().into_iter().flatten() {
                            if let Some(m) = mac.as_str() {
                                lan_macs.push(m.to_string());
                            }
                        }
                        for m in d["media"].as_array().into_iter().flatten() {
                            services.push(format!(
                                "media: {} :{}",
                                m["name"].as_str().unwrap_or("?"),
                                m["port"].as_u64().unwrap_or(0),
                            ));
                        }
                    }
                    Some(PeerRow {
                        hostname,
                        presence: gs("presence"),
                        health: gs("health"),
                        version: gs("mde_version"),
                        overlay_ip: gs("overlay_ip"),
                        role: gs("role"),
                        currency: p["revision"]["currency"]
                            .as_str()
                            .unwrap_or("unknown")
                            .to_string(),
                        last_seen_ms: p
                            .get("last_seen_ms")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        tags,
                        services,
                        ssh,
                        rdp,
                        vnc,
                        lan_macs,
                        containers,
                        vms,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(rows)
}

/// PD-3/L6 — parse the `action/connect/devices` reply (a JSON array of
/// `{id, name, online, battery}`) into device rows. A non-array / bad reply
/// degrades to an empty roster (the Devices group simply doesn't render) —
/// the KDC host may be absent on a peer that never paired anything.
#[must_use]
pub fn parse_devices(raw: &str) -> Vec<DeviceRow> {
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    let id = d.get("id")?.as_str()?.to_string();
                    let name = d
                        .get("name")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&id)
                        .to_string();
                    Some(DeviceRow {
                        id,
                        name,
                        online: d.get("online").and_then(serde_json::Value::as_bool) == Some(true),
                        battery: d
                            .get("battery")
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|b| u8::try_from(b).ok()),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// UNIFY-16 — reader-side staleness window for the voice agent's
/// `state/voice/status` heartbeat: a snapshot older than this means the agent
/// stopped publishing (not running). A small multiple of the agent's
/// `STATUS_HEARTBEAT_SECS` (15 s), matching the birthright/notify-center
/// readers so the whole desktop agrees on "live vs dead agent".
const VOICE_STALE_SECS: u64 = 45;

/// UNIFY-16 — read the LOCAL node's latest `state/voice/status` snapshot
/// (published by `mde-voice-hud --agent`, VOIP-28) from the Bus. `None` when
/// the Bus is unavailable or no agent has ever published — the caller renders
/// that as an honest "—", never a fabricated state. The parsed status carries
/// a `fresh` flag (heartbeat within [`VOICE_STALE_SECS`]) so a dead agent reads
/// honestly as offline rather than its last-registered value. Blocking — the
/// Bus client must run inside `spawn_blocking`.
#[must_use]
pub fn read_voice_status() -> Option<VoiceStatus> {
    let dir = mde_bus::client_data_dir()?;
    let persist = mde_bus::persist::Persist::open(dir).ok()?;
    // Newest message on the topic = the last row (list_since orders ulid asc).
    let last = persist.list_since("state/voice/status", None).ok()?.pop()?;
    let v: serde_json::Value = serde_json::from_str(last.body.as_deref()?).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let ts = v.get("ts").and_then(serde_json::Value::as_u64).unwrap_or(0);
    Some(VoiceStatus {
        registered: v
            .get("registered")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        listening: v
            .get("listening")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        fresh: now.saturating_sub(ts) <= VOICE_STALE_SECS,
    })
}

/// Filter predicate (L2): hostname OR capability tag (L1) OR any
/// offered-service line.
#[must_use]
pub fn matches_filter(row: &PeerRow, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    row.hostname.to_lowercase().contains(&f)
        // Match the role too (now populated mesh-wide: peer/lighthouse) so
        // typing "peer" surfaces the peers instead of an empty list.
        || row.role.to_lowercase().contains(&f)
        || row.tags.iter().any(|t| t.to_lowercase().contains(&f))
        || row.services.iter().any(|s| s.to_lowercase().contains(&f))
}

/// PD-5 gating: an op button is live only when the peer's
/// descriptors offer the service AND the peer isn't offline AND it
/// isn't this machine (no SSH-to-self chrome — self-inapplicable ops
/// hidden, Q5).
#[must_use]
pub fn op_enabled(row: &PeerRow, offered: bool, self_hostname: &str) -> bool {
    offered && row.presence != "offline" && row.hostname != self_hostname
}

/// Group order for the list (Q4/Q5): self first, then online, idle,
/// offline. Returns the group label for a row.
#[must_use]
pub fn group_of(row: &PeerRow, self_hostname: &str) -> &'static str {
    if row.hostname == self_hostname {
        "This machine"
    } else {
        match row.presence.as_str() {
            "online" => "Online",
            "idle" => "Idle",
            _ => "Offline",
        }
    }
}

/// PD-8 — the view-gated 2 s metrics tick (registered by `App::
/// subscription` only while the Peers panel is the active view, so
/// nothing polls when the operator is elsewhere — the Compute-panel
/// pattern).
#[must_use]
pub fn metrics_subscription() -> cosmic::iced::Subscription<crate::Message> {
    cosmic::iced::time::every(Duration::from_secs(2))
        .map(|_| crate::Message::Peers(Message::MetricsTick))
}

/// PD-3/Q10 — the view-gated 30 s directory-refresh tick (registered
/// by `App::subscription` only while the Peers panel is the active
/// view). Re-reads `action/mesh/directory` so presence/health/tags
/// stay current without an operator click; the reload preserves the
/// current filter + selection.
#[must_use]
pub fn directory_subscription() -> cosmic::iced::Subscription<crate::Message> {
    cosmic::iced::time::every(Duration::from_secs(30))
        .map(|_| crate::Message::Peers(Message::PollTick))
}

/// PD-3/Q10 — the **Bus-push** half: subscribe to the directory-changed
/// event the responder publishes (`event/mesh/directory`) and reload the
/// instant the roster changes, instead of waiting out the 30 s poll. The
/// poll stays registered as a backstop in case an event is missed.
const DIRECTORY_EVENT_TOPIC: &str = "event/mesh/directory";

#[must_use]
pub fn directory_event_subscription() -> cosmic::iced::Subscription<crate::Message> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::Subscription::run(|| {
        cosmic::iced::stream::channel(
            8,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<crate::Message>| async move {
                let mut cursor = dir_event_cursor_init().await;
                loop {
                    tokio::time::sleep(Duration::from_millis(900)).await;
                    let (n, next) = dir_event_poll(cursor.clone()).await;
                    cursor = next;
                    for _ in 0..n {
                        let _ = output.send(crate::Message::Peers(Message::PollTick)).await;
                    }
                }
            },
        )
    })
}

/// Seed the cursor at the latest existing event so the subscription only
/// reacts to changes published *after* it starts.
async fn dir_event_cursor_init() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let dir = mde_bus::client_data_dir()?;
        let persist = mde_bus::persist::Persist::open(dir).ok()?;
        persist
            .list_since(DIRECTORY_EVENT_TOPIC, None)
            .ok()?
            .last()
            .map(|m| m.ulid.clone())
    })
    .await
    .ok()
    .flatten()
}

/// Count new directory-changed events since `cursor`; return the count +
/// the advanced cursor. Bus unavailable → no events, cursor unchanged.
async fn dir_event_poll(cursor: Option<String>) -> (usize, Option<String>) {
    tokio::task::spawn_blocking(move || {
        let Some(dir) = mde_bus::client_data_dir() else {
            return (0, cursor);
        };
        let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
            return (0, cursor);
        };
        let msgs = persist
            .list_since(DIRECTORY_EVENT_TOPIC, cursor.as_deref())
            .unwrap_or_default();
        let next = msgs.last().map(|m| m.ulid.clone()).or(cursor);
        (msgs.len(), next)
    })
    .await
    .unwrap_or((0, None))
}

/// PD-3/L6 — fire a Connect verb (`action/connect/<verb>`) with a JSON body
/// over the Bus and report whether the reply was `{"ok":true}`. Runs the
/// blocking Bus client off-thread.
async fn device_verb(topic: &'static str, body: String) -> bool {
    tokio::task::spawn_blocking(move || {
        crate::dbus::action_request_with_body(topic, Some(&body), Duration::from_secs(2))
    })
    .await
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    .map(|v| v["ok"] == true)
    .unwrap_or(false)
}

/// PD-3/L6 — shell out to the system file picker (`zenity --file-selection`)
/// and return the chosen file's basename + byte size. `None` = cancelled,
/// no selection, or no picker installed. The D-W1 no-toolkit-dep pattern.
async fn pick_file() -> Option<(String, u64)> {
    tokio::task::spawn_blocking(|| {
        let out = std::process::Command::new("zenity")
            .args(["--file-selection", "--title=Send a file to this device"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let path = String::from_utf8(out.stdout).ok()?.trim().to_string();
        if path.is_empty() {
            return None;
        }
        let size = std::fs::metadata(&path).ok()?.len();
        let filename = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        Some((filename, size))
    })
    .await
    .ok()
    .flatten()
}

/// PD-7/L20 — probe one peer's overlay RTT (ms) by timing a TCP handshake
/// to the discard port through the tunnel — the same SYN→RST methodology as
/// the daemon's PD-6 `transport_probe` (a refused connect is a *successful*
/// measurement). `None` on timeout / bad address. Blocking; call off-thread.
fn probe_overlay_rtt(ip: &str) -> Option<f64> {
    use std::net::TcpStream;
    let addr: std::net::SocketAddr = format!("{ip}:9").parse().ok()?;
    let start = std::time::Instant::now();
    match TcpStream::connect_timeout(&addr, Duration::from_millis(1500)) {
        // Connected or refused — the stack answered through the tunnel.
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
        Err(_) => return None,
    }
    #[allow(clippy::cast_precision_loss)]
    Some(start.elapsed().as_micros() as f64 / 1000.0)
}

/// PD-7/L19 — the expandable underlay trace: shell `tracepath`/`traceroute`
/// to the peer's overlay address and collect the hop lines. Honest about the
/// substrate boundary: through Nebula the overlay address resolves in few
/// hops; the *true public-endpoint* trace (direct vs relay) needs the Nebula
/// admin socket, which the OSS substrate doesn't expose (noted at PD-6).
/// Blocking; call off-thread.
fn run_traceroute(ip: &str) -> Result<Vec<String>, String> {
    // Prefer tracepath (no root); fall back to traceroute. -m caps hops.
    let attempt = |cmd: &str, args: &[&str]| -> Option<std::process::Output> {
        std::process::Command::new(cmd).args(args).output().ok()
    };
    let out = attempt("tracepath", &["-m", "8", ip])
        .filter(|o| o.status.success())
        .or_else(|| attempt("traceroute", &["-m", "8", "-w", "2", ip]))
        .ok_or_else(|| "no tracepath/traceroute on this host".to_string())?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let hops: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if hops.is_empty() {
        Err("trace produced no hops".to_string())
    } else {
        Ok(hops)
    }
}

/// PD-7/L18 — sample each `(host, overlay_ip)`'s recent overlay throughput
/// from its Netdata `system.net`, normalized to 0.0..=1.0 against a 12 MB/s
/// reference so a busy link saturates the particle stream. Unreachable
/// Netdata → that host simply omitted (no particles). Blocking; off-thread.
///
/// MESHMAP-3 — `pub` so the `mde-mesh-wallpaper` bin reuses the exact sampler
/// the Peers Map uses (no reimplementation), wiring real per-node flow into the
/// desktop wallpaper's particle streams. MESHMAP-6 — returns `LinkFlow` (the
/// proxy fills `tx` = busy-ness, `rx` = 0; it can't split direction).
#[must_use]
pub fn sample_flows(
    targets: &[(String, String)],
) -> std::collections::HashMap<String, super::peers_map::LinkFlow> {
    /// Normalization reference: ~100 Mbit/s in bytes/s. A link at/above this
    /// saturates the stream; idle links round to ~0 and draw nothing.
    const REF_BYTES_PER_S: f64 = 12_000_000.0;
    let mut out = std::collections::HashMap::new();
    for (host, ip) in targets {
        if let Ok(series) = fetch_series(ip, "system.net") {
            if let Some(last) = series.last() {
                let norm = (last / REF_BYTES_PER_S).clamp(0.0, 1.0);
                // MESHMAP-6 — the per-node proxy attributes a node's TOTAL
                // throughput to its self→peer edge; it can't split direction,
                // so `tx` carries the busy-ness and `rx` stays 0 (no fake
                // reverse stream). MESHMAP-6's real source fills both.
                out.insert(
                    host.clone(),
                    super::peers_map::LinkFlow { tx: norm, rx: 0.0 },
                );
            }
        }
    }
    out
}

/// PD-7/L20 — the ~2 s trace tick, registered only while a trace card is
/// open (App::subscription gates on `traced_edge`).
#[must_use]
pub fn trace_subscription() -> cosmic::iced::Subscription<crate::Message> {
    cosmic::iced::time::every(Duration::from_secs(2))
        .map(|_| crate::Message::Peers(Message::TraceTick))
}

/// PD-7/L18 — the ~3 s flow-data tick (re-sample peer overlay throughput),
/// registered only while the Map view is open.
#[must_use]
pub fn flow_data_subscription() -> cosmic::iced::Subscription<crate::Message> {
    cosmic::iced::time::every(Duration::from_secs(3))
        .map(|_| crate::Message::Peers(Message::FlowTick))
}

/// PD-7/L18/L22 — the ~90 ms particle-animation tick. Registered ONLY when
/// real traffic is flowing (App::subscription gates on [`PeersPanel::
/// has_flow`]), so an idle mesh runs no animation loop (idle CPU).
#[must_use]
pub fn flow_anim_subscription() -> cosmic::iced::Subscription<crate::Message> {
    cosmic::iced::time::every(Duration::from_millis(90))
        .map(|_| crate::Message::Peers(Message::FlowAnim))
}

/// PD-11 — poll the executor's result file via the verb, with a 2 s
/// gap between attempts.
fn poll_lifecycle_result(host: String, id: String, attempts_left: u8) -> Task<crate::Message> {
    Task::perform(
        async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let body = format!(r#"{{"peer":"{host}","id":"{id}"}}"#);
            let outcome = tokio::task::spawn_blocking(move || {
                crate::dbus::action_request_with_body(
                    "action/services/lifecycle-result",
                    Some(&body),
                    Duration::from_secs(2),
                )
            })
            .await
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| {
                if v["found"] == true {
                    Some(if v["result"]["ok"] == true {
                        Ok(())
                    } else {
                        Err(v["result"]["error"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string())
                    })
                } else {
                    None
                }
            });
            (host, id, attempts_left.saturating_sub(1), outcome)
        },
        |(host, id, attempts_left, outcome)| {
            crate::Message::Peers(Message::LifecyclePolled {
                host,
                id,
                attempts_left,
                outcome,
            })
        },
    )
}

/// Fetch one Netdata chart's last-60s series over the overlay
/// (std-only HTTP/1.0 GET — no HTTP-client dep, the D-W1 pattern).
/// Blocking — call inside `spawn_blocking`.
fn fetch_series(ip: &str, chart: &str) -> Result<Vec<f64>, String> {
    use std::io::{Read, Write};
    let addr = format!("{ip}:19999");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr
            .parse()
            .map_err(|e| format!("bad address {addr}: {e}"))?,
        Duration::from_millis(900),
    )
    .map_err(|e| format!("netdata unreachable: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(1500)))
        .ok();
    write!(
        stream,
        "GET /api/v1/data?chart={chart}&after=-60&points=20&format=json HTTP/1.0\r\nHost: {ip}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).map_err(|e| e.to_string())?;
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .ok_or("malformed HTTP reply")?;
    parse_netdata_series(body)
}

/// Parse Netdata's `/api/v1/data` JSON into a chronological series:
/// each row's non-time columns summed (abs — net in/out are signed).
#[must_use = "the parsed series"]
pub fn parse_netdata_series(body: &str) -> Result<Vec<f64>, String> {
    let v: serde_json::Value =
        serde_json::from_str(body.trim()).map_err(|e| format!("bad netdata json: {e}"))?;
    let rows = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or("netdata reply missing data")?;
    let mut series: Vec<f64> = rows
        .iter()
        .filter_map(|row| {
            let cols = row.as_array()?;
            // Column 0 is the timestamp; the rest are dimensions.
            Some(
                cols[1..]
                    .iter()
                    .filter_map(|c| c.as_f64())
                    .map(f64::abs)
                    .sum(),
            )
        })
        .collect();
    series.reverse(); // netdata returns newest-first
    Ok(series)
}

/// Render a unicode sparkline (L14 — the Carbon-restrained v1; the
/// canvas treatment can layer on in the /preview pass).
#[must_use]
pub fn sparkline(values: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() {
        return String::new();
    }
    let max = values.iter().copied().fold(f64::MIN, f64::max);
    let min = values.iter().copied().fold(f64::MAX, f64::min);
    let span = (max - min).max(f64::EPSILON);
    values
        .iter()
        .map(|v| {
            let idx = (((v - min) / span) * 7.0).round() as usize;
            BARS[idx.min(7)]
        })
        .collect()
}

impl PeersPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            self_hostname: detect_hostname(),
            // PEERS-DT — default sort Status, online-first (ascending rank).
            sort: SortColumn::Status,
            sort_asc: true,
            ..Self::default()
        }
    }

    /// PEERS-DT — the rows in current sort order, with the search filter
    /// applied. Default sort is Status (online first) then Name.
    #[must_use]
    pub fn sorted_rows(&self) -> Vec<&PeerRow> {
        let mut v: Vec<&PeerRow> = self
            .rows
            .iter()
            .filter(|r| matches_filter(r, &self.filter))
            .collect();
        let rtt_of = |r: &PeerRow| {
            self.rtt
                .get(&r.hostname)
                .copied()
                .flatten()
                .unwrap_or(f64::INFINITY)
        };
        v.sort_by(|a, b| {
            use std::cmp::Ordering;
            let name = |r: &PeerRow| r.hostname.to_lowercase();
            let o = match self.sort {
                SortColumn::Name => name(a).cmp(&name(b)),
                SortColumn::Status => presence_rank(&a.presence)
                    .cmp(&presence_rank(&b.presence))
                    .then_with(|| name(a).cmp(&name(b))),
                SortColumn::Role => a.role.cmp(&b.role).then_with(|| name(a).cmp(&name(b))),
                SortColumn::OverlayIp => a.overlay_ip.cmp(&b.overlay_ip),
                SortColumn::Latency => rtt_of(a).partial_cmp(&rtt_of(b)).unwrap_or(Ordering::Equal),
                SortColumn::Services => a.services.len().cmp(&b.services.len()),
                SortColumn::LastSeen => a.last_seen_ms.cmp(&b.last_seen_ms),
            };
            if self.sort_asc {
                o
            } else {
                o.reverse()
            }
        });
        v
    }

    /// PD-7/L18/L22 — any edge currently carrying enough flow to animate?
    /// The animation subscription is registered only when this holds, so an
    /// idle mesh runs no particle loop.
    #[must_use]
    pub fn has_flow(&self) -> bool {
        self.map_view && self.flows.values().any(|f| f.tx > 0.02 || f.rx > 0.02)
    }

    /// Fetch the directory + the paired-device roster (each Bus client
    /// needs its own thread — same contract as the home-panel probes). The
    /// two run as a batch so the Devices group (L6) lands alongside peers.
    pub fn load() -> Task<crate::Message> {
        let directory = Task::perform(
            async {
                tokio::task::spawn_blocking(|| {
                    crate::dbus::action_request("action/mesh/directory", Duration::from_secs(2))
                })
                .await
                .ok()
                .flatten()
                .map_or_else(
                    || Err("mackesd unreachable (is the mesh service running?)".to_string()),
                    |raw| parse_directory(&raw),
                )
            },
            |result| crate::Message::Peers(Message::Loaded(result)),
        );
        Task::batch([directory, Self::fetch_devices(), Self::fetch_voice()])
    }

    /// UNIFY-16 — read the LOCAL node's voice (SIP) presence snapshot from the
    /// `state/voice/status` Bus topic, off-thread (the Bus client is blocking).
    /// Resolves to `None` when no agent has published / the Bus is down, which
    /// the VOICE tile renders as an honest "—" — never a fabricated state.
    pub fn fetch_voice() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(read_voice_status)
                    .await
                    .ok()
                    .flatten()
            },
            |voice| crate::Message::Peers(Message::VoiceLoaded(voice)),
        )
    }

    /// PD-3/L6 — query the KDC host's live roster over the Bus. A missing
    /// host / empty roster resolves to no devices (the group hides).
    pub fn fetch_devices() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(|| {
                    crate::dbus::action_request("action/connect/devices", Duration::from_secs(2))
                })
                .await
                .ok()
                .flatten()
                .map(|raw| parse_devices(&raw))
                .unwrap_or_default()
            },
            |devices| crate::Message::Peers(Message::DevicesLoaded(devices)),
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(result) => {
                match result {
                    Ok(rows) => {
                        self.rows = rows;
                        self.loaded = Some(Ok(()));
                        // BOOT-PEERS-1 — only when the roster is empty do we read
                        // the boot snapshot (cheap, and avoids a per-poll sqlite
                        // open on a populated mesh) to tell "still settling" from
                        // a genuinely empty mesh.
                        self.boot_converging = self.rows.is_empty()
                            && crate::panels::home::read_boot_readiness().fabric_converging();
                        // PEERS-DT — the table opens with every row collapsed (no
                        // auto-expand); a stale selection that's no longer in the
                        // roster is cleared so it can't expand a missing row.
                        if let Some(sel) = self.selected.clone() {
                            if !self.rows.iter().any(|r| r.hostname == sel) {
                                self.selected = None;
                            }
                        }
                    }
                    Err(e) => self.loaded = Some(Err(e)),
                }
                Task::none()
            }
            Message::VoiceLoaded(voice) => {
                // UNIFY-16 — the LOCAL node's voice presence (or `None` when no
                // agent has published); rendered in the self row's VOICE tile.
                self.voice = voice;
                Task::none()
            }
            Message::FilterChanged(f) => {
                self.filter = f;
                Task::none()
            }
            Message::Select(host) => {
                // PEERS-DT — clicking a row toggles its inline expansion. A
                // map-node click (map_view) always expands the clicked peer.
                if self.selected.as_deref() == Some(host.as_str()) && !self.map_view {
                    self.selected = None;
                } else {
                    self.selected = Some(host);
                }
                // L6 — peer and device selection are mutually exclusive.
                self.selected_device = None;
                self.metrics = None;
                self.metrics_err = None;
                // A map-node click lands you on the peer's detail (W87).
                self.map_view = false;
                Task::none()
            }
            Message::SortBy(col) => {
                // PEERS-DT — re-click toggles direction; a new column resets to
                // ascending.
                if self.sort == col {
                    self.sort_asc = !self.sort_asc;
                } else {
                    self.sort = col;
                    self.sort_asc = true;
                }
                Task::none()
            }
            Message::ToggleMap => {
                self.map_view = !self.map_view;
                if self.map_view {
                    self.rtt = super::peers_map::read_latency_cache();
                    self.paths = super::peers_map::read_latency_paths();
                } else {
                    // Leaving the map closes any open trace card + stops flow.
                    self.traced_edge = None;
                    self.trace_rtt.clear();
                    self.traceroute = None;
                    self.flows.clear();
                }
                Task::none()
            }
            // PD-3/Q10 — the manual button and the 30 s tick both
            // re-read the directory; the Loaded handler keeps the
            // operator's filter + selection across the swap.
            Message::RefreshClicked | Message::PollTick => Self::load(),
            Message::Op(proto, host) => {
                self.op_result = format!("launching {} → {host}…", proto.label());
                let label = proto.label();
                let h = host.clone();
                Task::perform(
                    async move {
                        let ok = crate::launcher::launch(&h, proto).await;
                        (label, h, ok)
                    },
                    |(label, host, ok)| {
                        crate::Message::Peers(Message::OpFinished { label, host, ok })
                    },
                )
            }
            Message::CallClicked(host) => {
                // PD-5 — fire-and-forget a dial request to the voice HUD,
                // which resolves the hostname to its extension and rings it.
                self.op_result = format!("ringing {host} on the voice HUD…");
                let body = serde_json::json!({ "target": host }).to_string();
                Task::perform(
                    async move {
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Some(dir) = mde_bus::client_data_dir() {
                                if let Ok(p) = mde_bus::persist::Persist::open(dir) {
                                    let _ = p.write(
                                        "action/voice/dial",
                                        mde_bus::hooks::config::Priority::Default,
                                        None,
                                        Some(&body),
                                    );
                                }
                            }
                        })
                        .await;
                        host
                    },
                    |host| crate::Message::Peers(Message::Called(host)),
                )
            }
            Message::Called(host) => {
                self.op_result = format!("dial request sent for {host} — answer on the voice HUD");
                Task::none()
            }
            Message::NudgeClicked(host) => {
                self.op_result = format!("nudging {host} to reconcile…");
                let h = host.clone();
                Task::perform(
                    async move {
                        let body = format!(r#"{{"peer":"{h}"}}"#);
                        let ok = tokio::task::spawn_blocking(move || {
                            crate::dbus::action_request_with_body(
                                "action/fleet/nudge",
                                Some(&body),
                                Duration::from_secs(2),
                            )
                        })
                        .await
                        .ok()
                        .flatten()
                        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        .map(|v| v["ok"] == true)
                        .unwrap_or(false);
                        (h, ok)
                    },
                    |(host, ok)| crate::Message::Peers(Message::NudgeFinished { host, ok }),
                )
            }
            Message::NudgeFinished { host, ok } => {
                self.op_result = if ok {
                    format!("{host}: nudged — it reconciles within ~10 s")
                } else {
                    format!("{host}: nudge failed (mackesd unreachable?)")
                };
                Task::none()
            }
            Message::WakeClicked { host, mac } => {
                self.op_result = format!("waking {host} ({mac})…");
                let h = host.clone();
                Task::perform(
                    async move {
                        let body = format!(r#"{{"mac":"{mac}"}}"#);
                        let ok = tokio::task::spawn_blocking(move || {
                            crate::dbus::action_request_with_body(
                                "action/mesh/wake",
                                Some(&body),
                                Duration::from_secs(2),
                            )
                        })
                        .await
                        .ok()
                        .flatten()
                        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        .map(|v| v["ok"] == true)
                        .unwrap_or(false);
                        (h, ok)
                    },
                    |(host, ok)| crate::Message::Peers(Message::WakeFinished { host, ok }),
                )
            }
            Message::WakeFinished { host, ok } => {
                self.op_result = if ok {
                    format!("{host}: magic packet sent — watch for it to come online")
                } else {
                    format!("{host}: wake failed (mackesd unreachable?)")
                };
                Task::none()
            }
            Message::MetricsTick => {
                // Fetch only for an online selected peer with an
                // overlay address; otherwise the pane shows why not.
                let Some(r) = self
                    .rows
                    .iter()
                    .find(|r| Some(r.hostname.as_str()) == self.selected.as_deref())
                else {
                    return Task::none();
                };
                if r.presence == "offline" || r.overlay_ip.is_empty() {
                    return Task::none();
                }
                let ip = r.overlay_ip.clone();
                let host = r.hostname.clone();
                Task::perform(
                    async move {
                        let result = tokio::task::spawn_blocking(move || {
                            Ok(PeerMetrics {
                                cpu: fetch_series(&ip, "system.cpu")?,
                                load: fetch_series(&ip, "system.load")?,
                                net: fetch_series(&ip, "system.net")?,
                                disk: fetch_series(&ip, "system.io")?,
                            })
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()));
                        (host, result)
                    },
                    |(host, result)| crate::Message::Peers(Message::MetricsLoaded { host, result }),
                )
            }
            Message::OpenDashboard(ip) => {
                self.op_result = format!("opening dashboard http://{ip}:19999 …");
                Task::perform(
                    async move {
                        let url = format!("http://{ip}:19999");
                        matches!(
                            tokio::process::Command::new("xdg-open").arg(&url).spawn(),
                            Ok(_)
                        )
                    },
                    |_| crate::Message::Peers(Message::MetricsTick),
                )
            }
            Message::MetricsLoaded { host, result } => {
                // Stale fetches (selection moved on) are dropped.
                if Some(host.as_str()) == self.selected.as_deref() {
                    match result {
                        Ok(m) => {
                            self.metrics = Some(m);
                            self.metrics_err = None;
                        }
                        Err(e) => {
                            self.metrics = None;
                            self.metrics_err = Some(e);
                        }
                    }
                }
                Task::none()
            }
            Message::Lifecycle {
                host,
                kind,
                name,
                op,
            } => {
                let key = (host.clone(), kind.clone(), name.clone(), op.clone());
                // L16 — stop/restart need the armed second click.
                if op != "start" && self.pending_confirm.as_ref() != Some(&key) {
                    self.pending_confirm = Some(key);
                    self.op_result = format!("click again to confirm {op} of {name} on {host}");
                    return Task::none();
                }
                self.pending_confirm = None;
                self.op_result = format!("{op} {name} on {host}…");
                Task::perform(
                    async move {
                        let body = format!(
                            r#"{{"peer":"{host}","kind":"{kind}","name":"{name}","op":"{op}"}}"#
                        );
                        let id = tokio::task::spawn_blocking(move || {
                            crate::dbus::action_request_with_body(
                                "action/services/lifecycle",
                                Some(&body),
                                Duration::from_secs(2),
                            )
                        })
                        .await
                        .ok()
                        .flatten()
                        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        .filter(|v| v["ok"] == true)
                        .and_then(|v| v["id"].as_str().map(str::to_string));
                        (host, id)
                    },
                    |(host, id)| crate::Message::Peers(Message::LifecycleSent { host, id }),
                )
            }
            Message::LifecycleSent { host, id } => match id {
                None => {
                    self.op_result = format!("{host}: lifecycle request failed to send");
                    Task::none()
                }
                Some(id) => {
                    self.op_result = format!("{host}: request sent — waiting for the executor…");
                    poll_lifecycle_result(host, id, 5)
                }
            },
            Message::LifecyclePolled {
                host,
                id,
                attempts_left,
                outcome,
            } => match outcome {
                Some(Ok(())) => {
                    self.op_result = format!("{host}: done — inventory updates within a heartbeat");
                    Self::load()
                }
                Some(Err(e)) => {
                    self.op_result = format!("{host}: executor refused/failed — {e}");
                    Task::none()
                }
                None if attempts_left == 0 => {
                    self.op_result = format!(
                        "{host}: no result yet (peer slow or offline) — inventory will catch up"
                    );
                    Task::none()
                }
                None => poll_lifecycle_result(host, id, attempts_left),
            },
            Message::DevicesLoaded(devices) => {
                // Drop a stale device selection (device unpaired between polls).
                if let Some(sel) = &self.selected_device {
                    if !devices.iter().any(|d| &d.id == sel) {
                        self.selected_device = None;
                    }
                }
                self.devices = devices;
                Task::none()
            }
            Message::SelectDevice(id) => {
                self.selected_device = Some(id);
                // L6 — clear the peer selection so the device card shows.
                self.selected = None;
                self.metrics = None;
                self.metrics_err = None;
                self.map_view = false;
                Task::none()
            }
            Message::RingDevice { id, name } => {
                self.op_result = format!("ringing {name}…");
                Task::perform(
                    async move {
                        let body = serde_json::json!({ "device_id": id }).to_string();
                        let ok = device_verb("action/connect/ring", body).await;
                        (name, "Ring", ok)
                    },
                    |(name, verb, ok)| {
                        crate::Message::Peers(Message::DeviceActionFinished { name, verb, ok })
                    },
                )
            }
            Message::SendFile { id, name } => {
                self.op_result = format!("choose a file to send to {name}…");
                Task::perform(
                    async move {
                        // D-W1 — shell out to the system file picker (no GUI
                        // toolkit dep); the chosen path's basename + size ride
                        // the share announce. Cancel / no picker → no send.
                        let Some((filename, size)) = pick_file().await else {
                            return (name, "Send-file", false, true);
                        };
                        let body = serde_json::json!({
                            "device_id": id,
                            "filename": filename,
                            "payload_size": size,
                        })
                        .to_string();
                        let ok = device_verb("action/connect/share", body).await;
                        (name, "Send-file", ok, false)
                    },
                    |(name, verb, ok, cancelled)| {
                        if cancelled {
                            crate::Message::Peers(Message::DeviceActionFinished {
                                name,
                                verb: "Send-file-cancel",
                                ok: false,
                            })
                        } else {
                            crate::Message::Peers(Message::DeviceActionFinished { name, verb, ok })
                        }
                    },
                )
            }
            Message::DeviceActionFinished { name, verb, ok } => {
                self.op_result = match (verb, ok) {
                    ("Send-file-cancel", _) => format!("send to {name} cancelled"),
                    ("Ring", true) => format!("{name}: ringing now"),
                    ("Send-file", true) => format!("{name}: file queued to send"),
                    (v, _) => format!("{name}: {v} failed (device offline or unpaired?)"),
                };
                Task::none()
            }
            Message::EdgeClicked(host) => {
                // Open (or re-open) the trace card; reset the session series
                // + the traceroute so they belong to this edge.
                self.traced_edge = Some(host.clone());
                self.trace_rtt.clear();
                self.traceroute = None;
                self.traceroute_running = false;
                // Seed the first sample immediately from the latency cache.
                if let Some(Some(rtt)) = self.rtt.get(&host) {
                    self.trace_rtt.push(*rtt);
                }
                Task::none()
            }
            Message::CloseTrace => {
                self.traced_edge = None;
                self.trace_rtt.clear();
                self.traceroute = None;
                self.traceroute_running = false;
                Task::none()
            }
            Message::TraceTick => {
                // Re-probe the traced peer's overlay RTT (same TCP-handshake
                // methodology as the PD-6 transport probe), client-side, for
                // a responsive session sparkline.
                let Some(host) = self.traced_edge.clone() else {
                    return Task::none();
                };
                let Some(ip) = self
                    .rows
                    .iter()
                    .find(|r| r.hostname == host)
                    .map(|r| r.overlay_ip.clone())
                    .filter(|ip| !ip.is_empty())
                else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let rtt = tokio::task::spawn_blocking(move || probe_overlay_rtt(&ip))
                            .await
                            .ok()
                            .flatten();
                        (host, rtt)
                    },
                    |(host, rtt_ms)| {
                        crate::Message::Peers(Message::TraceRttSampled { host, rtt_ms })
                    },
                )
            }
            Message::TraceRttSampled { host, rtt_ms } => {
                // Drop a stale sample (card moved to another edge / closed).
                if self.traced_edge.as_deref() == Some(host.as_str()) {
                    if let Some(rtt) = rtt_ms {
                        self.trace_rtt.push(rtt);
                        // Keep the last ~40 samples (a bounded session window).
                        let len = self.trace_rtt.len();
                        if len > 40 {
                            self.trace_rtt.drain(0..len - 40);
                        }
                    }
                }
                Task::none()
            }
            Message::RunTraceroute(host) => {
                let Some(ip) = self
                    .rows
                    .iter()
                    .find(|r| r.hostname == host)
                    .map(|r| r.overlay_ip.clone())
                    .filter(|ip| !ip.is_empty())
                else {
                    self.traceroute = Some(Err("no overlay address for this peer".to_string()));
                    return Task::none();
                };
                self.traceroute_running = true;
                self.traceroute = None;
                Task::perform(
                    async move {
                        let hops = tokio::task::spawn_blocking(move || run_traceroute(&ip))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));
                        (host, hops)
                    },
                    |(host, hops)| crate::Message::Peers(Message::TracerouteDone { host, hops }),
                )
            }
            Message::TracerouteDone { host, hops } => {
                if self.traced_edge.as_deref() == Some(host.as_str()) {
                    self.traceroute_running = false;
                    self.traceroute = Some(hops);
                }
                Task::none()
            }
            Message::FlowTick => {
                // MESHMAP-6 — prefer the REAL per-link byte rates from the
                // mackesd `link-traffic` collector (nftables accounting). It's
                // a cheap local file read, done synchronously here. Any online
                // peer the collector covers gets its true per-edge tx/rx; the
                // rest fall back to the per-node Netdata `sample_flows` proxy
                // (MESHMAP-3) so a counter-less node keeps working exactly as
                // before — honest degradation, never a fake split.
                let real = super::peers_map::read_link_traffic();
                let online: Vec<&PeerRow> = self
                    .rows
                    .iter()
                    .filter(|r| {
                        r.presence != "offline"
                            && !r.overlay_ip.is_empty()
                            && r.hostname != self.self_hostname
                    })
                    .collect();
                if online.is_empty() {
                    self.flows.clear();
                    return Task::none();
                }
                // Seed the real per-link flows immediately; only the
                // proxy-fallback peers need the (slower) off-thread Netdata
                // sample.
                let mut seeded = std::collections::HashMap::new();
                let mut proxy_targets: Vec<(String, String)> = Vec::new();
                for r in &online {
                    if let Some(flow) = real.get(&r.hostname) {
                        seeded.insert(r.hostname.clone(), *flow);
                    } else {
                        proxy_targets.push((r.hostname.clone(), r.overlay_ip.clone()));
                    }
                }
                self.flows = seeded;
                // MESHMAP-3 (W3) — also proxy-sample SELF's own throughput (its
                // local Netdata on loopback) so the self→peer particle stream
                // (self is the sender that direction) has a real density to ride.
                // Self has no per-link self→self counter, so it's always proxied;
                // keyed under the self hostname so the Map view reads it as
                // `self_flow` (via `LinkFlow.tx`).
                if !self.self_hostname.is_empty() {
                    proxy_targets.push((self.self_hostname.clone(), "127.0.0.1".to_string()));
                }
                if proxy_targets.is_empty() {
                    // Every online peer is real-sourced (and no self) — no
                    // Netdata sampling needed.
                    return Task::none();
                }
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || sample_flows(&proxy_targets))
                            .await
                            .unwrap_or_default()
                    },
                    |flows| crate::Message::Peers(Message::FlowsSampled(flows)),
                )
            }
            Message::FlowsSampled(flows) => {
                // MESHMAP-6 — these are the PROXY-fallback peers (the real
                // ones were seeded in FlowTick). Merge them in without
                // clobbering the real per-link flows already set.
                for (host, flow) in flows {
                    self.flows.entry(host).or_insert(flow);
                }
                Task::none()
            }
            Message::FlowAnim => {
                // Advance the particle phase; wraps at 1.0.
                self.flow_phase = (self.flow_phase + 0.06).fract();
                Task::none()
            }
            Message::OpFinished { label, host, ok } => {
                self.op_result = if ok {
                    format!("{label} {host}: launched")
                } else {
                    format!(
                        "{label} {host}: failed to launch — is {} installed?",
                        if label == "SSH" {
                            "cosmic-term"
                        } else {
                            "remmina"
                        }
                    )
                };
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = mde_theme::FontSize::defaults();
        let title = text("Peers")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        // L3 — guided empty states.
        match &self.loaded {
            None => {
                return shell(title, text("Loading the mesh directory…").into(), palette);
            }
            Some(Err(e)) => {
                let body = column![
                    text("The mesh service isn't answering.")
                        .size(16)
                        .colr(palette.text.into_cosmic_color()),
                    text(e.clone())
                        .size(12)
                        .colr(palette.text_muted.into_cosmic_color()),
                    text("Start it from Network → Mesh Services, then refresh.")
                        .size(13)
                        .colr(palette.text_muted.into_cosmic_color()),
                    refresh_btn(palette),
                ]
                .spacing(8);
                return shell(title, body.into(), palette);
            }
            // BOOT-PEERS-1 — a cold reboot's multi-minute warm-up (Nebula
            // overlay-IP → bus → QNM directory replication → first peer sweep)
            // leaves the roster transiently empty. Show a "settling" state, not
            // the "empty mesh" guidance, so it doesn't look broken.
            Some(Ok(())) if self.rows.is_empty() && self.boot_converging => {
                let body = column![
                    text("Peers settling…")
                        .size(16)
                        .colr(palette.text.into_cosmic_color()),
                    text("The mesh fabric is still coming up (overlay network, message bus, shared-storage directory). Peers appear here as the directory replicates — usually within a minute or two of boot.")
                        .size(13)
                        .colr(palette.text_muted.into_cosmic_color()),
                    refresh_btn(palette),
                ]
                .spacing(8);
                return shell(title, body.into(), palette);
            }
            Some(Ok(())) if self.rows.is_empty() => {
                let body = column![
                    text("No peers in this mesh yet.")
                        .size(16)
                        .colr(palette.text.into_cosmic_color()),
                    text("Invite a peer: mint a join token with `mackesd enroll-token` and run `mackesd enroll --token …` on the new box.")
                        .size(13)
                        .colr(palette.text_muted.into_cosmic_color()),
                    refresh_btn(palette),
                ]
                .spacing(8);
                return shell(title, body.into(), palette);
            }
            Some(Ok(())) => {}
        }

        // UNIFY-9 — the Unified Workbench master-detail layout: a 262 px
        // grouped filter list on the left and the peer/device detail (or the
        // live map) on the right. Data, the map toggle, and selection state are
        // all unchanged — this is presentation only.
        let weights = FontWeight::defaults();
        let left = self.peer_list_panel(palette, &sizes, &weights);
        let right = self.detail_pane(palette, &sizes, &weights);
        container(row![left, right].width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// UNIFY-9 — the left filter list (the design's 262 px column): a live
    /// search box over presence-grouped peer rows (Online / Idle / Offline)
    /// plus the paired-device group, each row a presence dot + host +
    /// `role · ip`. Filter + selection wiring is unchanged.
    fn peer_list_panel<'a>(
        &'a self,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        let search = container(crate::controls::styled_text_input(
            "Filter by host, role, tag…",
            &self.filter,
            |f| crate::Message::Peers(Message::FilterChanged(f)),
            palette,
        ))
        .padding(Padding::from([11u16, 13u16]))
        .width(Length::Fill);

        // Presence buckets over the filtered + sorted rows (the data logic is
        // unchanged — `sorted_rows` still applies the filter + sort).
        let (mut online, mut idle, mut offline) = (Vec::new(), Vec::new(), Vec::new());
        for r in self.sorted_rows() {
            match r.presence.as_str() {
                "online" => online.push(r),
                "idle" => idle.push(r),
                _ => offline.push(r),
            }
        }

        let mut list = column![].width(Length::Fill);
        for (label, rows) in [("ONLINE", online), ("IDLE", idle), ("OFFLINE", offline)] {
            if rows.is_empty() {
                continue;
            }
            list = list.push(list_group_header(label, rows.len(), palette, sizes));
            for r in rows {
                let selected = self.selected.as_deref() == Some(r.hostname.as_str());
                list = list.push(self.peer_list_row(r, selected, palette, sizes, weights));
            }
        }

        // PD-3/L6 — the paired KDE-Connect devices as their own group.
        let devices: Vec<&DeviceRow> = self
            .devices
            .iter()
            .filter(|d| {
                self.filter.is_empty()
                    || d.name.to_lowercase().contains(&self.filter.to_lowercase())
            })
            .collect();
        if !devices.is_empty() {
            list = list.push(list_group_header("DEVICES", devices.len(), palette, sizes));
            for d in devices {
                let selected = self.selected_device.as_deref() == Some(d.id.as_str());
                list = list.push(device_list_row(d, selected, palette, sizes, weights));
            }
        }

        let body = column![
            search,
            divider(palette),
            scrollable(list).height(Length::Fill)
        ]
        .height(Length::Fill);

        container(body)
            .width(Length::Fixed(262.0))
            .height(Length::Fill)
            .sty(move |_| container::Style {
                snap: false,
                icon_color: None,
                background: Some(Background::Color(palette.background.into_cosmic_color())),
                border: Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                text_color: Some(palette.text.into_cosmic_color()),
            })
            .into()
    }

    /// UNIFY-9 — one peer row in the left list: presence dot (teal for self) +
    /// host + `role · overlay-ip` sub-line. Click selects (unchanged wiring).
    fn peer_list_row<'a>(
        &self,
        r: &PeerRow,
        selected: bool,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        let is_self = r.hostname == self.self_hostname;
        let dot = if is_self {
            carbon::TEAL_30
        } else {
            presence_color(&r.presence)
        };
        let host_label = if is_self {
            format!("{} · you", r.hostname)
        } else {
            r.hostname.clone()
        };
        let ip = if r.overlay_ip.is_empty() {
            "—"
        } else {
            r.overlay_ip.as_str()
        };
        let role = if r.role.is_empty() {
            "peer"
        } else {
            r.role.as_str()
        };
        let inner = row![
            selected_bar(selected, palette),
            Space::new().width(Length::Fixed(8.0)),
            pip(dot),
            Space::new().width(Length::Fixed(9.0)),
            column![
                text(host_label)
                    .size(TypeRole::Body.size_in(*sizes))
                    .colr(palette.text.into_cosmic_color()),
                mono_text(format!("{role} · {ip}"), TypeRole::Caption, sizes, weights)
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(1)
            .width(Length::Fill),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let content = container(inner)
            .padding(Padding::from([8u16, 11u16]))
            .width(Length::Fill);
        list_row(
            content,
            selected,
            palette,
            crate::Message::Peers(Message::Select(r.hostname.clone())),
        )
    }

    /// UNIFY-9 — the right detail pane: the identity header (host + role +
    /// List / Live-map toggle) over the list detail, the device card, or the
    /// live map. Selection + map-toggle state is unchanged.
    fn detail_pane<'a>(
        &'a self,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        let selected_device = self
            .selected_device
            .as_deref()
            .and_then(|id| self.devices.iter().find(|d| d.id == id));
        let selected_peer = self
            .rows
            .iter()
            .find(|r| Some(r.hostname.as_str()) == self.selected.as_deref());

        let (title, role, role_col): (String, String, Rgba) = if let Some(d) = selected_device {
            (
                d.name.clone(),
                "KDE Connect device".to_string(),
                carbon::TEAL_30,
            )
        } else if let Some(r) = selected_peer {
            let col = if r.hostname == self.self_hostname {
                carbon::TEAL_30
            } else if r.role.to_lowercase().contains("lighthouse") {
                carbon::BLUE_50
            } else {
                palette.text_muted
            };
            let role = if r.role.is_empty() {
                "peer".to_string()
            } else {
                r.role.clone()
            };
            (r.hostname.clone(), role, col)
        } else {
            (
                "Select a peer".to_string(),
                String::new(),
                palette.text_muted,
            )
        };

        let header = self.detail_header(title, role, role_col, palette, sizes);

        let body: Element<'a, crate::Message> = if self.map_view {
            self.map_body(palette, sizes, weights)
        } else if let Some(d) = selected_device {
            device_detail(d, &self.op_result, palette, sizes, weights)
        } else if let Some(r) = selected_peer {
            self.peer_detail_body(r, palette, sizes, weights)
        } else {
            container(
                text("Select a peer from the list to inspect it.")
                    .size(TypeRole::Body.size_in(*sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            )
            .padding(Padding::from([16u16, 16u16]))
            .into()
        };

        container(column![header, body].height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// UNIFY-9 — the detail header: host title + role label + the List /
    /// Live-map segmented toggle. The toggle emits the unchanged `ToggleMap`
    /// (only the inactive segment is clickable, so it just flips the view).
    fn detail_header<'a>(
        &self,
        title: String,
        role: String,
        role_col: Rgba,
        palette: Palette,
        sizes: &FontSize,
    ) -> Element<'a, crate::Message> {
        let list_btn = crate::controls::variant_button(
            "List",
            if self.map_view {
                crate::controls::ButtonVariant::Secondary
            } else {
                crate::controls::ButtonVariant::Primary
            },
            self.map_view
                .then(|| crate::Message::Peers(Message::ToggleMap)),
            palette,
        );
        let map_btn = crate::controls::variant_button(
            "Live map",
            if self.map_view {
                crate::controls::ButtonVariant::Primary
            } else {
                crate::controls::ButtonVariant::Secondary
            },
            (!self.map_view).then(|| crate::Message::Peers(Message::ToggleMap)),
            palette,
        );
        let role_el: Element<'a, crate::Message> = if role.is_empty() {
            Space::new().width(Length::Fixed(0.0)).into()
        } else {
            text(role)
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(role_col.into_cosmic_color())
                .into()
        };
        let header_row = row![
            text(title)
                .size(TypeRole::Subheading.size_in(*sizes))
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fixed(10.0)),
            role_el,
            Space::new().width(Length::Fill),
            list_btn,
            map_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);
        column![
            container(header_row)
                .padding(Padding::from([11u16, 16u16]))
                .width(Length::Fill),
            divider(palette),
        ]
        .into()
    }

    /// UNIFY-9 — the peer list-view detail body: the op toolbar, the live op
    /// result strip, capability-tag chips, the 4-up stat tiles, the
    /// CPU/NET/LOAD/DISK sparkline row (real Netdata series), and the Services
    /// Provided cards. Every op + the PD-11 lifecycle controls + the PD-8
    /// metrics states are preserved; only the layout/styling changed.
    fn peer_detail_body<'a>(
        &'a self,
        r: &'a PeerRow,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        // --- op toolbar (descriptor- + presence-gated; wiring unchanged) ---
        let mut ops = row![].spacing(7);
        let can_call = r.presence != "offline" && r.hostname != self.self_hostname;
        ops = ops.push(crate::controls::variant_button(
            "Call",
            crate::controls::ButtonVariant::Secondary,
            can_call.then(|| crate::Message::Peers(Message::CallClicked(r.hostname.clone()))),
            palette,
        ));
        for (offered, proto) in [
            (r.ssh, crate::launcher::Protocol::Ssh),
            (r.rdp, crate::launcher::Protocol::Rdp),
            (r.vnc, crate::launcher::Protocol::Vnc),
        ] {
            let target = if r.overlay_ip.is_empty() {
                r.hostname.clone()
            } else {
                r.overlay_ip.clone()
            };
            let live = op_enabled(r, offered, &self.self_hostname)
                .then(|| crate::Message::Peers(Message::Op(proto, target)));
            ops = ops.push(crate::controls::variant_button(
                proto.label(),
                crate::controls::ButtonVariant::Secondary,
                live,
                palette,
            ));
        }
        if r.currency == "behind" {
            ops = ops.push(crate::controls::variant_button(
                "Apply now",
                crate::controls::ButtonVariant::Primary,
                Some(crate::Message::Peers(Message::NudgeClicked(
                    r.hostname.clone(),
                ))),
                palette,
            ));
        }
        if r.presence == "offline" {
            if let Some(mac) = r.lan_macs.first() {
                ops = ops.push(crate::controls::variant_button(
                    "Wake",
                    crate::controls::ButtonVariant::Primary,
                    Some(crate::Message::Peers(Message::WakeClicked {
                        host: r.hostname.clone(),
                        mac: mac.clone(),
                    })),
                    palette,
                ));
            }
        }
        if !r.overlay_ip.is_empty() && r.presence != "offline" {
            ops = ops.push(crate::controls::variant_button(
                "Metrics ↗",
                crate::controls::ButtonVariant::Secondary,
                Some(crate::Message::Peers(Message::OpenDashboard(
                    r.overlay_ip.clone(),
                ))),
                palette,
            ));
        }

        // --- live op-result strip (PD-5) ---
        let strip: Element<'a, crate::Message> = if self.op_result.is_empty() {
            Space::new().height(Length::Fixed(0.0)).into()
        } else {
            result_strip(&self.op_result, palette, sizes, weights)
        };

        // --- L1 capability-tag chips ---
        let tags: Element<'a, crate::Message> = if r.tags.is_empty() {
            Space::new().height(Length::Fixed(0.0)).into()
        } else {
            let mut chips = row![text("Tags")
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color())]
            .spacing(6)
            .align_y(cosmic::iced::alignment::Vertical::Center);
            for t in &r.tags {
                chips = chips.push(badge(t.as_str(), palette));
            }
            chips.into()
        };

        // --- 4-up stat tiles (presence / health / revision / voice) ---
        // UNIFY-16 — the VOICE tile carries the LOCAL node's real SIP presence
        // from the `state/voice/status` Bus snapshot (published by
        // `mde-voice-hud --agent`). That topic is this node's own agent
        // heartbeat — there is no per-peer voice in the directory — so only the
        // self row shows a real value; every other peer renders an honest "—"
        // (never a fabricated per-peer voice state).
        let local_voice = (r.hostname == self.self_hostname)
            .then_some(self.voice.as_ref())
            .flatten();
        let (voice_value, voice_dot) = voice_tile_state(local_voice);
        let tiles = row![
            stat_tile(
                "PRESENCE",
                cap(&r.presence),
                presence_color(&r.presence),
                palette,
                sizes
            ),
            stat_tile(
                "HEALTH",
                cap(&r.health),
                health_color(&r.health),
                palette,
                sizes
            ),
            stat_tile(
                "REVISION",
                cap(&r.currency),
                currency_color(&r.currency),
                palette,
                sizes
            ),
            stat_tile("VOICE", voice_value, voice_dot, palette, sizes),
        ]
        .spacing(8);

        // --- CPU/NET/LOAD/DISK sparkline row (real PD-8 Netdata series) ---
        let spark_row: Element<'a, crate::Message> = match (&self.metrics, &self.metrics_err) {
            (Some(m), _) => row![
                spark_card(
                    "CPU",
                    fmt_last(&m.cpu),
                    &m.cpu,
                    carbon::BLUE_50,
                    palette,
                    sizes,
                    weights
                ),
                spark_card(
                    "NET",
                    fmt_last(&m.net),
                    &m.net,
                    carbon::GREEN_40,
                    palette,
                    sizes,
                    weights
                ),
                spark_card(
                    "LOAD",
                    fmt_last(&m.load),
                    &m.load,
                    carbon::YELLOW_30,
                    palette,
                    sizes,
                    weights
                ),
                spark_card(
                    "DISK",
                    fmt_last(&m.disk),
                    &m.disk,
                    carbon::GRAY_50,
                    palette,
                    sizes,
                    weights
                ),
            ]
            .spacing(12)
            .into(),
            (None, Some(e)) => flat_panel(
                text(format!("Netdata not answering on this peer: {e}"))
                    .size(TypeRole::Caption.size_in(*sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
                palette,
                [11u16, 13u16],
            ),
            (None, None) => flat_panel(
                text(if r.presence == "offline" {
                    "Peer offline — no live metrics."
                } else {
                    "Waiting for the first metrics sample…"
                })
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color()),
                palette,
                [11u16, 13u16],
            ),
        };

        // --- Services Provided (Podman + KVM/Media) ---
        let services_header = text("SERVICES PROVIDED")
            .size(TypeRole::Caption.size_in(*sizes))
            .colr(palette.text_muted.into_cosmic_color());
        let services = row![
            self.podman_card(r, palette, sizes, weights),
            self.kvm_card(r, palette, sizes, weights),
        ]
        .spacing(12);

        let content = column![
            ops,
            strip,
            tags,
            tiles,
            spark_row,
            services_header,
            services
        ]
        .spacing(11)
        .width(Length::Fill);
        scrollable(
            container(content)
                .padding(Padding::from([16u16, 16u16]))
                .width(Length::Fill),
        )
        .height(Length::Fill)
        .into()
    }

    /// UNIFY-9 — the Podman services card: the peer's containers with their
    /// PD-11 lifecycle controls (start one-click; stop/restart armed-confirm).
    fn podman_card<'a>(
        &'a self,
        r: &'a PeerRow,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        let mut col = column![text("Podman")
            .size(TypeRole::Caption.size_in(*sizes))
            .colr(palette.text_muted.into_cosmic_color())]
        .spacing(2);
        if r.containers.is_empty() {
            col = col.push(
                text("No containers published.")
                    .size(TypeRole::Caption.size_in(*sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            for (name, state) in &r.containers {
                col = col.push(self.service_row(
                    r,
                    "container",
                    name.as_str(),
                    state.as_str(),
                    palette,
                    sizes,
                    weights,
                ));
            }
        }
        flat_panel(col, palette, [12u16, 13u16])
    }

    /// UNIFY-9 — the KVM Guests + Media card: libvirt guests (with the PD-11
    /// lifecycle controls) and the published media endpoints (PD-2).
    fn kvm_card<'a>(
        &'a self,
        r: &'a PeerRow,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        let mut col = column![text("KVM Guests")
            .size(TypeRole::Caption.size_in(*sizes))
            .colr(palette.text_muted.into_cosmic_color())]
        .spacing(2);
        if r.vms.is_empty() {
            col = col.push(
                text("No guests.")
                    .size(TypeRole::Caption.size_in(*sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            for (name, state) in &r.vms {
                col = col.push(self.service_row(
                    r,
                    "vm",
                    name.as_str(),
                    state.as_str(),
                    palette,
                    sizes,
                    weights,
                ));
            }
        }
        let media: Vec<&String> = r
            .services
            .iter()
            .filter(|s| s.starts_with("media:"))
            .collect();
        if !media.is_empty() {
            col = col.push(
                text("Media")
                    .size(TypeRole::Caption.size_in(*sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
            for m in media {
                let label = m
                    .strip_prefix("media:")
                    .map(str::trim)
                    .unwrap_or(m.as_str());
                col = col.push(
                    container(
                        mono_text(label.to_string(), TypeRole::Caption, sizes, weights)
                            .colr(palette.text.into_cosmic_color()),
                    )
                    .padding(Padding::from([5u16, 0u16]))
                    .width(Length::Fill),
                );
            }
        }
        flat_panel(col, palette, [12u16, 13u16])
    }

    /// UNIFY-9 — one service row (container or guest): a state dot + mono name
    /// + state, with the PD-11 lifecycle button(s). Self is excluded (local
    /// service control lives in Mesh Services). Wiring is unchanged.
    fn service_row<'a>(
        &'a self,
        r: &'a PeerRow,
        kind: &'a str,
        name: &'a str,
        state: &'a str,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        let running = state == "running";
        let mut rrow = row![
            pip(lifecycle_color(state)),
            Space::new().width(Length::Fixed(9.0)),
            column![
                mono_text(name.to_string(), TypeRole::Mono, sizes, weights)
                    .colr(palette.text.into_cosmic_color()),
                mono_text(state.to_string(), TypeRole::Caption, sizes, weights)
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(1)
            .width(Length::Fill),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);
        if r.hostname != self.self_hostname {
            let mk = |op: &str| {
                crate::Message::Peers(Message::Lifecycle {
                    host: r.hostname.clone(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                    op: op.to_string(),
                })
            };
            if running {
                let armed = |op: &str| {
                    self.pending_confirm
                        == Some((
                            r.hostname.clone(),
                            kind.to_string(),
                            name.to_string(),
                            op.to_string(),
                        ))
                };
                rrow = rrow.push(crate::controls::variant_button(
                    if armed("stop") {
                        "Confirm stop"
                    } else {
                        "Stop"
                    },
                    crate::controls::ButtonVariant::Secondary,
                    Some(mk("stop")),
                    palette,
                ));
                rrow = rrow.push(crate::controls::variant_button(
                    if armed("restart") {
                        "Confirm restart"
                    } else {
                        "Restart"
                    },
                    crate::controls::ButtonVariant::Secondary,
                    Some(mk("restart")),
                    palette,
                ));
            } else {
                rrow = rrow.push(crate::controls::variant_button(
                    "Start",
                    crate::controls::ButtonVariant::Secondary,
                    Some(mk("start")),
                    palette,
                ));
            }
        }
        container(rrow)
            .padding(Padding::from([6u16, 0u16]))
            .width(Length::Fill)
            .into()
    }

    /// UNIFY-9 — the live-map body (PD-7): the real mesh canvas (with the trace
    /// card overlay when an edge is open) plus the legend. Unchanged map data,
    /// flow, and trace wiring — only the surrounding chrome moved into the
    /// shared detail header.
    fn map_body<'a>(
        &'a self,
        palette: Palette,
        sizes: &FontSize,
        weights: &FontWeight,
    ) -> Element<'a, crate::Message> {
        let lh_ips = super::peers_map::lighthouse_overlay_ips();
        let paths = super::peers_map::read_latency_paths();
        let ip_to_host: std::collections::HashMap<&str, &str> = self
            .rows
            .iter()
            .filter(|r| !r.overlay_ip.is_empty())
            .map(|r| (r.overlay_ip.as_str(), r.hostname.as_str()))
            .collect();
        let nodes: Vec<super::peers_map::MapNode> = self
            .rows
            .iter()
            .map(|r| super::peers_map::MapNode {
                hostname: r.hostname.clone(),
                presence: r.presence.clone(),
                rtt_ms: self.rtt.get(&r.hostname).copied().flatten(),
                is_self: r.hostname == self.self_hostname,
                lighthouse: super::peers_map::is_lighthouse(&r.role, &r.overlay_ip, &lh_ips),
                flow: self.flows.get(&r.hostname).map(|f| f.tx).unwrap_or(0.0),
                flow_rx: self.flows.get(&r.hostname).map(|f| f.rx).unwrap_or(0.0),
                relay_via: paths.get(&r.hostname).and_then(|pi| {
                    pi.relay_via
                        .as_deref()
                        .and_then(|ip| ip_to_host.get(ip).map(|h| (*h).to_string()))
                }),
            })
            .collect();
        let geo = super::peers_map::any_geo_known(&nodes);
        let positions = super::peers_map::geo_layout(&nodes);
        let self_flow = self
            .flows
            .get(&self.self_hostname)
            .map(|f| f.tx)
            .unwrap_or(0.0);
        let canvas_stock: cosmic::iced::Element<'_, crate::Message, cosmic::iced::Theme> =
            cosmic::iced::widget::canvas(super::peers_map::MapProgram {
                nodes,
                positions,
                palette,
                flow_phase: self.flow_phase,
                self_flow,
                geo,
                reduce_motion: crate::live_theme::reduce_motion(),
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let canvas: Element<'_, crate::Message> =
            cosmic::iced::widget::themer(None, canvas_stock).into();
        let map_inner: Element<'a, crate::Message> = match self.traced_edge.as_deref() {
            Some(host) => row![
                container(canvas).width(Length::FillPortion(2)),
                container(self.trace_card(host, palette))
                    .width(Length::FillPortion(1))
                    .padding(Padding::from([0u16, 8u16])),
            ]
            .spacing(12)
            .height(Length::Fill)
            .into(),
            None => canvas,
        };
        let canvas_box = container(map_inner)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding::from([1u16, 1u16]))
            .sty(move |_| container::Style {
                snap: false,
                icon_color: None,
                background: Some(Background::Color(palette.background.into_cosmic_color())),
                border: Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                text_color: Some(palette.text.into_cosmic_color()),
            });
        let legend = row![
            legend_bar(palette.accent),
            Space::new().width(Length::Fixed(6.0)),
            mono_text("direct", TypeRole::Caption, sizes, weights)
                .colr(palette.text_muted.into_cosmic_color()),
            Space::new().width(Length::Fixed(16.0)),
            legend_bar(palette.overlay),
            Space::new().width(Length::Fixed(6.0)),
            mono_text("via relay", TypeRole::Caption, sizes, weights)
                .colr(palette.text_muted.into_cosmic_color()),
            Space::new().width(Length::Fixed(16.0)),
            pip(carbon::TEAL_30),
            Space::new().width(Length::Fixed(6.0)),
            mono_text("this node", TypeRole::Caption, sizes, weights)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);
        column![
            container(canvas_box)
                .padding(Padding::from([8u16, 16u16]))
                .width(Length::Fill)
                .height(Length::Fill),
            container(legend).padding(Padding::from([8u16, 16u16])),
        ]
        .height(Length::Fill)
        .into()
    }

    /// PD-7/L19-20 — the augmented trace card for the self→`host` edge:
    /// overlay RTT report + the session RTT sparkline (L20) + an expandable
    /// underlay traceroute (L19). Direct/relay/NAT classification is the one
    /// datum the OSS Nebula substrate can't expose (no per-tunnel admin API),
    /// so the path line says so honestly (the PD-6 boundary).
    fn trace_card<'a>(
        &'a self,
        host: &'a str,
        palette: mde_theme::Palette,
    ) -> Element<'a, crate::Message> {
        let row = self.rows.iter().find(|r| r.hostname == host);
        let reachable =
            self.trace_rtt.last().is_some() || matches!(self.rtt.get(host), Some(Some(_)));
        let last_rtt = self
            .trace_rtt
            .last()
            .copied()
            .or_else(|| self.rtt.get(host).copied().flatten());
        let header = row![
            text(format!("Trace · {host}"))
                .size(16)
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fill),
            crate::controls::variant_button(
                "Close",
                crate::controls::ButtonVariant::Secondary,
                Some(crate::Message::Peers(Message::CloseTrace)),
                palette,
            ),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let rtt_str =
            last_rtt.map_or_else(|| "unreachable".to_string(), |ms| format!("{ms:.1} ms"));
        let rtt_line = row![
            text("Overlay RTT")
                .size(12)
                .width(Length::Fixed(100.0))
                .colr(palette.text_muted.into_cosmic_color()),
            text(rtt_str)
                .size(12)
                .colr(palette.text.into_cosmic_color()),
        ];
        let reach_line = fact("Reachable", if reachable { "yes" } else { "no" }, palette);
        // NET-3 (PD-6/PD-7) — real path classification from the Nebula
        // debug-SSH hostmap (mesh_latency join). Direct → the chosen remote
        // endpoint; relay → the relay peer; overlay → still handshaking or the
        // debug SSH is unavailable (honest, never guessed). NAT class remains
        // the one datum OSS Nebula doesn't expose.
        let info = self.paths.get(host);
        let (path_value, path_note): (String, String) = match info.map(|i| i.path.as_str()) {
            Some("direct") => (
                "direct".to_string(),
                info.and_then(|i| i.endpoint.clone())
                    .map_or_else(|| "hole-punched".to_string(), |e| format!("endpoint {e}")),
            ),
            Some("relay") => (
                "relayed".to_string(),
                info.and_then(|i| i.relay_via.clone()).map_or_else(
                    || "via a lighthouse relay".to_string(),
                    |r| format!("via {r}"),
                ),
            ),
            _ => (
                "overlay (tunnelled)".to_string(),
                "direct/relay pending handshake or debug SSH; NAT class not exposed by OSS Nebula"
                    .to_string(),
            ),
        };
        // Inline (not `fact`) so the owned path string moves into `text`
        // rather than being borrowed from this local.
        let path_line = column![
            row![
                text("Path")
                    .size(12)
                    .width(Length::Fixed(100.0))
                    .colr(palette.text_muted.into_cosmic_color()),
                text(path_value)
                    .size(12)
                    .colr(palette.text.into_cosmic_color()),
            ],
            text(path_note)
                .size(10)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2);
        // L20 — the session RTT sparkline, built while the card is open.
        let spark: Element<'_, crate::Message> = if self.trace_rtt.is_empty() {
            text("sampling session RTT…")
                .size(11)
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            row![
                text("Session RTT")
                    .size(12)
                    .width(Length::Fixed(90.0))
                    .colr(palette.text_muted.into_cosmic_color()),
                text(sparkline(&self.trace_rtt))
                    .size(12)
                    .colr(palette.accent.into_cosmic_color()),
            ]
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
        };
        // L19 — the expandable underlay traceroute.
        let trace_target = host.to_string();
        let mut trace_block = column![row![
            text("Underlay traceroute")
                .size(12)
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fixed(8.0)),
            crate::controls::variant_button(
                if self.traceroute_running {
                    "Tracing…"
                } else if self.traceroute.is_some() {
                    "Re-run"
                } else {
                    "Run"
                },
                crate::controls::ButtonVariant::Secondary,
                (!self.traceroute_running)
                    .then(|| crate::Message::Peers(Message::RunTraceroute(trace_target))),
                palette,
            ),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center)]
        .spacing(4);
        match &self.traceroute {
            None if !self.traceroute_running => {
                trace_block = trace_block.push(
                    text("Trace the path to this peer's overlay address.")
                        .size(10)
                        .colr(palette.text_muted.into_cosmic_color()),
                );
            }
            None => {}
            Some(Ok(hops)) => {
                for h in hops {
                    trace_block = trace_block.push(
                        text(h)
                            .size(11)
                            .colr(palette.text_muted.into_cosmic_color()),
                    );
                }
            }
            Some(Err(e)) => {
                trace_block = trace_block.push(
                    text(format!("trace failed: {e}"))
                        .size(11)
                        .colr(palette.text_muted.into_cosmic_color()),
                );
            }
        }
        let services_hint: Element<'_, crate::Message> = match row {
            Some(r) if !r.overlay_ip.is_empty() => fact("Overlay IP", &r.overlay_ip, palette),
            _ => Space::new().height(Length::Fixed(0.0)).into(),
        };
        container(
            scrollable(
                column![
                    header,
                    rtt_line,
                    reach_line,
                    services_hint,
                    path_line,
                    spark,
                    trace_block,
                ]
                .spacing(12),
            )
            .height(Length::Fill),
        )
        .padding(Padding::from([12u16, 12u16]))
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
    }
}

/// PD-3/L6 — the device detail card body: the live Connect ops (Ring, Send
/// file, open the KDC hub) over the op-result strip and the presence/battery
/// tiles. Identity now lives in the shared detail header (UNIFY-9), so this is
/// the body only. Ring / Send-file wiring is unchanged.
fn device_detail<'a>(
    d: &'a DeviceRow,
    op_result: &'a str,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let ring = crate::controls::variant_button(
        "Ring",
        crate::controls::ButtonVariant::Secondary,
        d.online.then(|| {
            crate::Message::Peers(Message::RingDevice {
                id: d.id.clone(),
                name: d.name.clone(),
            })
        }),
        palette,
    );
    let send = crate::controls::variant_button(
        "Send file…",
        crate::controls::ButtonVariant::Secondary,
        d.online.then(|| {
            crate::Message::Peers(Message::SendFile {
                id: d.id.clone(),
                name: d.name.clone(),
            })
        }),
        palette,
    );
    let hub = crate::controls::variant_button(
        "Open in Connect hub",
        crate::controls::ButtonVariant::Secondary,
        Some(crate::Message::SelectPanel {
            group: crate::model::Group::Mesh,
            panel: "connect",
        }),
        palette,
    );
    let ops = row![ring, send, hub].spacing(7);
    let strip: Element<'a, crate::Message> = if op_result.is_empty() {
        Space::new().height(Length::Fixed(0.0)).into()
    } else {
        result_strip(op_result, palette, sizes, weights)
    };
    let presence = if d.online { "online" } else { "offline" };
    let battery = d
        .battery
        .map(|b| format!("{b}%"))
        .unwrap_or_else(|| "—".to_string());
    let tiles = row![
        stat_tile(
            "PRESENCE",
            cap(presence),
            presence_color(presence),
            palette,
            sizes
        ),
        stat_tile("BATTERY", battery, palette.accent, palette, sizes),
    ]
    .spacing(8);
    let content = column![ops, strip, tiles].spacing(11).width(Length::Fill);
    scrollable(
        container(content)
            .padding(Padding::from([16u16, 16u16]))
            .width(Length::Fill),
    )
    .height(Length::Fill)
    .into()
}

/// UNIFY-9 — a flat Carbon surface tile/card: square corners + a 1 px hairline
/// border on `palette.surface`. The design's dense panels are flat; `card()`'s
/// rounded + lifted treatment would fight that spec, so this is the local flat
/// variant styled from the same tokens.
fn flat_panel<'a>(
    body: impl Into<Element<'a, crate::Message>>,
    palette: Palette,
    pad: [u16; 2],
) -> Element<'a, crate::Message> {
    container(body)
        .width(Length::Fill)
        .padding(Padding::from(pad))
        .sty(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            text_color: Some(palette.text.into_cosmic_color()),
        })
        .into()
}

/// UNIFY-9 — one stat tile: an uppercase caption label over a status dot +
/// value (the design's 4-up presence/health/revision/version row).
fn stat_tile<'a>(
    label: &str,
    value: String,
    dot: Rgba,
    palette: Palette,
    sizes: &FontSize,
) -> Element<'a, crate::Message> {
    let body = column![
        text(label.to_string())
            .size(TypeRole::Caption.size_in(*sizes))
            .colr(palette.text_muted.into_cosmic_color()),
        row![
            pip(dot),
            Space::new().width(Length::Fixed(7.0)),
            text(value)
                .size(TypeRole::Body.size_in(*sizes))
                .colr(palette.text.into_cosmic_color()),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center),
    ]
    .spacing(6);
    flat_panel(body, palette, [11u16, 13u16])
}

/// UNIFY-9 — one sparkline card: caption label + current value (mono) over the
/// unicode sparkline of the real Netdata series, tinted by the metric token.
fn spark_card<'a>(
    label: &str,
    value: String,
    series: &[f64],
    color: Rgba,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let header = row![
        text(label.to_string())
            .size(TypeRole::Caption.size_in(*sizes))
            .colr(palette.text_muted.into_cosmic_color()),
        Space::new().width(Length::Fill),
        mono_text(value, TypeRole::Mono, sizes, weights).colr(palette.text.into_cosmic_color()),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);
    let spark = mono_text(sparkline(series), TypeRole::Mono, sizes, weights)
        .colr(color.into_cosmic_color());
    flat_panel(column![header, spark].spacing(6), palette, [11u16, 13u16])
}

/// UNIFY-9 — a left-list group header: uppercase label + the group count.
fn list_group_header<'a>(
    label: &str,
    count: usize,
    palette: Palette,
    sizes: &FontSize,
) -> Element<'a, crate::Message> {
    container(
        row![
            text(label.to_string())
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color()),
            Space::new().width(Length::Fill),
            text(count.to_string())
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8u16, 14u16]))
    .width(Length::Fill)
    .into()
}

/// PD-3/L6 — one paired-device row in the left list (presence dot + name +
/// `KDE Connect · battery`). Click selects the device (unchanged wiring).
fn device_list_row<'a>(
    d: &DeviceRow,
    selected: bool,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let dot = if d.online {
        carbon::GREEN_40
    } else {
        carbon::RED_50
    };
    let battery = d
        .battery
        .map(|b| format!("{b}%"))
        .unwrap_or_else(|| "—".to_string());
    let inner = row![
        selected_bar(selected, palette),
        Space::new().width(Length::Fixed(8.0)),
        pip(dot),
        Space::new().width(Length::Fixed(9.0)),
        column![
            text(d.name.clone())
                .size(TypeRole::Body.size_in(*sizes))
                .colr(palette.text.into_cosmic_color()),
            mono_text(
                format!("KDE Connect · {battery}"),
                TypeRole::Caption,
                sizes,
                weights
            )
            .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(1)
        .width(Length::Fill),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);
    let content = container(inner)
        .padding(Padding::from([8u16, 11u16]))
        .width(Length::Fill);
    list_row(
        content,
        selected,
        palette,
        crate::Message::Peers(Message::SelectDevice(d.id.clone())),
    )
}

/// UNIFY-9 — wrap a list row's content in the clickable button + selection /
/// hover background (selected = raised, hover = surface).
fn list_row<'a>(
    content: impl Into<Element<'a, crate::Message>>,
    selected: bool,
    palette: Palette,
    msg: crate::Message,
) -> Element<'a, crate::Message> {
    button(content)
        .width(Length::Fill)
        .padding(Padding::from([0u16, 0u16]))
        .on_press(msg)
        .sty(move |_t, status| {
            let bg = if selected {
                palette.raised.into_cosmic_color()
            } else if matches!(status, cosmic::iced::widget::button::Status::Hovered) {
                palette.surface.into_cosmic_color()
            } else {
                cosmic::iced::Color::TRANSPARENT
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: palette.text.into_cosmic_color(),
                icon_color: None,
                border_radius: 0.0.into(),
                border_width: 0.0,
                border_color: cosmic::iced::Color::TRANSPARENT,
                border: Border {
                    color: cosmic::iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
            }
        })
        .into()
}

/// UNIFY-9 — the 3 px left selection rule (accent when selected, else an
/// invisible spacer so the row indent stays constant).
fn selected_bar<'a>(selected: bool, palette: Palette) -> Element<'a, crate::Message> {
    let bg = if selected {
        Some(Background::Color(palette.accent.into_cosmic_color()))
    } else {
        None
    };
    container(
        Space::new()
            .width(Length::Fixed(3.0))
            .height(Length::Fixed(22.0)),
    )
    .sty(move |_| container::Style {
        snap: false,
        icon_color: None,
        background: bg,
        border: Border {
            color: cosmic::iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        shadow: cosmic::iced::Shadow::default(),
        text_color: None,
    })
    .into()
}

/// UNIFY-9 — a 1 px full-width hairline divider in the border token.
fn divider<'a>(palette: Palette) -> Element<'a, crate::Message> {
    container(Space::new().height(Length::Fixed(1.0)).width(Length::Fill))
        .sty(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(palette.border.into_cosmic_color())),
            border: Border {
                color: cosmic::iced::Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            text_color: None,
        })
        .into()
}

/// UNIFY-9 — a short legend rule (the map's direct/relay key swatch).
fn legend_bar<'a>(color: Rgba) -> Element<'a, crate::Message> {
    container(
        Space::new()
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(2.0)),
    )
    .sty(move |_| container::Style {
        snap: false,
        icon_color: None,
        background: Some(Background::Color(color.into_cosmic_color())),
        border: Border {
            color: cosmic::iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        shadow: cosmic::iced::Shadow::default(),
        text_color: None,
    })
    .into()
}

/// UNIFY-9 — the live op-result strip: an accent left rule + mono message on a
/// flat surface (the design's `border-left:3px` result box).
fn result_strip<'a>(
    msg: &str,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    container(
        row![
            container(
                Space::new()
                    .width(Length::Fixed(3.0))
                    .height(Length::Fixed(18.0))
            )
            .sty(move |_| container::Style {
                snap: false,
                icon_color: None,
                background: Some(Background::Color(palette.accent.into_cosmic_color())),
                border: Border {
                    color: cosmic::iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                text_color: None,
            }),
            Space::new().width(Length::Fixed(9.0)),
            mono_text(msg.to_string(), TypeRole::Mono, sizes, weights)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8u16, 12u16]))
    .width(Length::Fill)
    .sty(move |_| container::Style {
        snap: false,
        icon_color: None,
        background: Some(Background::Color(palette.surface.into_cosmic_color())),
        border: Border {
            color: palette.border.into_cosmic_color(),
            width: 1.0,
            radius: 0.0.into(),
        },
        shadow: cosmic::iced::Shadow::default(),
        text_color: Some(palette.text.into_cosmic_color()),
    })
    .into()
}

/// UNIFY-9 — presence → Carbon status hue (online green / idle amber / offline
/// red / unknown gray), the design's dot ramp read from the carbon tokens.
fn presence_color(presence: &str) -> Rgba {
    match presence {
        "online" => carbon::GREEN_40,
        "idle" => carbon::YELLOW_30,
        "offline" => carbon::RED_50,
        _ => carbon::GRAY_50,
    }
}

/// UNIFY-9 — health → Carbon status hue.
fn health_color(health: &str) -> Rgba {
    match health {
        "healthy" => carbon::GREEN_40,
        "degraded" | "warning" => carbon::YELLOW_30,
        "critical" | "unhealthy" => carbon::RED_50,
        _ => carbon::GRAY_50,
    }
}

/// UNIFY-9 — revision currency → Carbon status hue.
fn currency_color(currency: &str) -> Rgba {
    match currency {
        "synced" | "current" => carbon::GREEN_40,
        "behind" => carbon::YELLOW_30,
        "ahead" => carbon::BLUE_50,
        _ => carbon::GRAY_50,
    }
}

/// UNIFY-9 — container/guest state → Carbon status hue.
fn lifecycle_color(state: &str) -> Rgba {
    match state {
        "running" => carbon::GREEN_40,
        "paused" => carbon::YELLOW_30,
        _ => carbon::GRAY_50,
    }
}

/// UNIFY-16 — the local voice (SIP) presence → the VOICE stat tile's
/// (value, dot), mirroring the sibling `*_color` Carbon-token ramp. `None` (a
/// non-self peer, or no/never-published local agent) renders an honest "—" in
/// the neutral token — never a fabricated voice state. A fresh snapshot maps
/// registered + listening → "Registered" (green), listening-only → "Listening"
/// (amber), neither → "Offline" (red); a stale heartbeat (the agent stopped
/// publishing) → "Offline".
fn voice_tile_state(voice: Option<&VoiceStatus>) -> (String, Rgba) {
    match voice {
        None => ("—".to_string(), carbon::GRAY_50),
        Some(v) if !v.fresh => ("Offline".to_string(), carbon::RED_50),
        Some(v) if v.registered && v.listening => ("Registered".to_string(), carbon::GREEN_40),
        Some(v) if v.listening => ("Listening".to_string(), carbon::YELLOW_30),
        Some(_) => ("Offline".to_string(), carbon::RED_50),
    }
}

/// UNIFY-9 — the current (last) sample of a series, one decimal.
fn fmt_last(series: &[f64]) -> String {
    format!("{:.1}", series.last().copied().unwrap_or(0.0))
}

/// UNIFY-9 — capitalize the first character (the design's `capitalize` on the
/// presence/health words).
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// PEERS-DT — humanize an epoch-ms `last_seen` against now. `0` (absent) → "—".
#[must_use]
pub fn humanize_last_seen(ms: i64, now_ms: i64) -> String {
    if ms <= 0 {
        return "—".to_string();
    }
    let secs = (now_ms - ms).max(0) / 1000;
    if secs < 10 {
        "now".to_string()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn shell<'a>(
    title: cosmic::iced::widget::Text<'a, cosmic::Theme>,
    body: Element<'a, crate::Message>,
    _palette: mde_theme::Palette,
) -> Element<'a, crate::Message> {
    container(column![title, Space::new().height(Length::Fixed(14.0)), body].spacing(2))
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn fact<'a>(
    label: &'a str,
    value: &'a str,
    palette: mde_theme::Palette,
) -> Element<'a, crate::Message> {
    row![
        text(label)
            .size(12)
            .width(Length::Fixed(100.0))
            .colr(palette.text_muted.into_cosmic_color()),
        text(value).size(12).colr(palette.text.into_cosmic_color()),
    ]
    .into()
}

fn badge<'a>(label: &'a str, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    container(text(label).size(11).colr(palette.text.into_cosmic_color()))
        .padding(Padding::from([2u16, 8u16]))
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(palette.raised.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn refresh_btn(palette: mde_theme::Palette) -> Element<'static, crate::Message> {
    crate::controls::variant_button(
        "Refresh",
        crate::controls::ButtonVariant::Secondary,
        Some(crate::Message::Peers(Message::RefreshClicked)),
        palette,
    )
}

fn detect_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"ok":true,"head":2,"peers":[
        {"hostname":"pine","presence":"online","health":"healthy","mde_version":"4.2.1",
         "overlay_ip":"10.42.0.2","role":"host","tags":["execution","headless"],
         "revision":{"currency":"synced"},
         "descriptors":{"remote_access":{"ssh":true,"rdp":false,"vnc":false},
            "containers":[{"name":"nginx","image":"nginx:latest","state":"running","ports":["8080->80/tcp"]}],
            "vms":[{"name":"win11","state":"running","vcpus":4,"memory_mb":8192,"addresses":["192.168.122.5"]}],
            "media":[{"name":"mpd","port":6600}],
            "alarms":{"tier":"healthy","worst":null}}},
        {"hostname":"oak","presence":"offline","health":"unknown","mde_version":null,
         "overlay_ip":null,"role":null,"revision":{"currency":"unknown"},"descriptors":null}
    ]}"#;

    #[test]
    fn parse_directory_reads_rows_and_services() {
        let rows = parse_directory(REPLY).unwrap();
        assert_eq!(rows.len(), 2);
        let pine = &rows[0];
        assert_eq!(pine.presence, "online");
        assert_eq!(pine.currency, "synced");
        assert!(pine.services.contains(&"SSH".to_string()));
        assert!(pine.services.iter().any(|s| s.contains("podman: nginx")));
        assert!(pine
            .services
            .iter()
            .any(|s| s.contains("kvm: win11") && s.contains("4 vCPU / 8192 MiB")));
        assert!(pine.services.iter().any(|s| s.contains("media: mpd :6600")));
        // L1 — capability tags parse; a peer without a `tags` key has none.
        assert_eq!(
            pine.tags,
            vec!["execution".to_string(), "headless".to_string()]
        );
        // Descriptor-less peer degrades honestly.
        assert!(rows[1].services.is_empty());
        assert!(rows[1].tags.is_empty());
    }

    #[test]
    fn parse_directory_surfaces_errors() {
        assert!(parse_directory("not json").is_err());
        assert!(parse_directory(r#"{"ok":false,"error":"nope"}"#).is_err());
    }

    #[test]
    fn filter_matches_hostname_tag_or_service() {
        let rows = parse_directory(REPLY).unwrap();
        assert!(matches_filter(&rows[0], ""));
        assert!(matches_filter(&rows[0], "pine"));
        assert!(matches_filter(&rows[0], "podman"));
        assert!(matches_filter(&rows[0], "WIN11"));
        // L1 — the filter narrows by capability tag too (case-insensitive).
        assert!(matches_filter(&rows[0], "execution"));
        assert!(matches_filter(&rows[0], "HEADLESS"));
        assert!(!matches_filter(&rows[0], "hop"));
        assert!(!matches_filter(&rows[1], "podman"));
        assert!(!matches_filter(&rows[1], "execution"));
    }

    #[test]
    fn grouping_pins_self_then_presence() {
        let rows = parse_directory(REPLY).unwrap();
        assert_eq!(group_of(&rows[0], "pine"), "This machine");
        assert_eq!(group_of(&rows[0], "elsewhere"), "Online");
        assert_eq!(group_of(&rows[1], "elsewhere"), "Offline");
    }

    #[test]
    fn select_and_filter_reduce_through_update() {
        let mut p = PeersPanel::new();
        let rows = parse_directory(REPLY).unwrap();
        let _ = p.update(Message::Loaded(Ok(rows)));
        assert!(p.loaded == Some(Ok(())));
        // PEERS-DT — the table opens with every row collapsed (no auto-select).
        assert!(p.selected.is_none(), "no row expanded by default");
        let _ = p.update(Message::Select("oak".into()));
        assert_eq!(p.selected.as_deref(), Some("oak"));
        // PEERS-DT — re-clicking the expanded row collapses it.
        let _ = p.update(Message::Select("oak".into()));
        assert!(p.selected.is_none(), "re-click collapses the row");
        let _ = p.update(Message::FilterChanged("podman".into()));
        assert_eq!(p.filter, "podman");
    }

    #[test]
    fn peers_dt_default_sort_is_status_then_name() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        // Default sort = Status (online first), Name as tiebreak.
        assert_eq!(p.sort, SortColumn::Status);
        assert!(p.sort_asc);
        let order: Vec<&str> = p
            .sorted_rows()
            .iter()
            .map(|r| r.hostname.as_str())
            .collect();
        // Within each presence bucket, names are ascending; offline last.
        let ranks: Vec<u8> = p
            .sorted_rows()
            .iter()
            .map(|r| presence_rank(&r.presence))
            .collect();
        let mut sorted_ranks = ranks.clone();
        sorted_ranks.sort_unstable();
        assert_eq!(
            ranks, sorted_ranks,
            "rows grouped online→idle→offline: {order:?}"
        );
    }

    #[test]
    fn peers_dt_sort_by_name_toggles_direction() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        let _ = p.update(Message::SortBy(SortColumn::Name));
        assert_eq!(p.sort, SortColumn::Name);
        assert!(p.sort_asc);
        let asc: Vec<String> = p.sorted_rows().iter().map(|r| r.hostname.clone()).collect();
        let _ = p.update(Message::SortBy(SortColumn::Name));
        assert!(!p.sort_asc, "re-click flips direction");
        let desc: Vec<String> = p.sorted_rows().iter().map(|r| r.hostname.clone()).collect();
        let mut rev = asc.clone();
        rev.reverse();
        assert_eq!(desc, rev, "descending is the reverse of ascending");
    }

    #[test]
    fn humanize_last_seen_buckets() {
        let now = 1_000_000_000_000i64;
        assert_eq!(humanize_last_seen(0, now), "—");
        assert_eq!(humanize_last_seen(now - 5_000, now), "now");
        assert_eq!(humanize_last_seen(now - 30_000, now), "30s");
        assert_eq!(humanize_last_seen(now - 120_000, now), "2m");
        assert_eq!(humanize_last_seen(now - 7_200_000, now), "2h");
        assert_eq!(humanize_last_seen(now - 172_800_000, now), "2d");
    }

    #[test]
    fn poll_reload_preserves_filter_and_selection_q10() {
        // PD-3/Q10 — the 30 s tick (PollTick) re-reads the directory.
        // The reload must NOT clobber the operator's current filter or
        // selection, otherwise a background refresh would yank the UI.
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        let _ = p.update(Message::Select("oak".into()));
        let _ = p.update(Message::FilterChanged("podman".into()));
        // A fresh directory arrives (what PollTick → load → Loaded does).
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        assert_eq!(
            p.selected.as_deref(),
            Some("oak"),
            "selection survives reload"
        );
        assert_eq!(p.filter, "podman", "filter survives reload");
    }

    #[test]
    fn op_gating_honors_descriptors_presence_and_self() {
        let rows = parse_directory(REPLY).unwrap();
        let pine = &rows[0]; // online, ssh offered
        assert!(pine.ssh && !pine.rdp);
        assert!(op_enabled(pine, pine.ssh, "elsewhere"));
        assert!(
            !op_enabled(pine, pine.rdp, "elsewhere"),
            "unoffered stays dead"
        );
        assert!(!op_enabled(pine, pine.ssh, "pine"), "no SSH-to-self");
        let oak = &rows[1]; // offline
        assert!(!op_enabled(oak, true, "elsewhere"), "offline disables ops");
    }

    #[test]
    fn netdata_series_parse_sums_dimensions_chronologically() {
        // Netdata returns newest-first rows; col 0 = timestamp.
        let body = r#"{"labels":["time","in","out"],"data":[[300,5.0,-2.0],[298,1.0,-1.0]]}"#;
        let s = parse_netdata_series(body).unwrap();
        assert_eq!(s, vec![2.0, 7.0], "abs-summed, oldest first");
        assert!(parse_netdata_series("nope").is_err());
    }

    #[test]
    fn sparkline_normalizes_into_eight_bars() {
        let s = sparkline(&[0.0, 50.0, 100.0]);
        assert_eq!(s.chars().count(), 3);
        assert!(s.starts_with('▁') && s.ends_with('█'));
        assert_eq!(sparkline(&[]), "");
        // Flat series renders without panic (span clamp).
        assert_eq!(sparkline(&[5.0, 5.0]).chars().count(), 2);
    }

    #[test]
    fn stale_metrics_are_dropped_fresh_applied() {
        let mut p = PeersPanel::new();
        let rows = parse_directory(REPLY).unwrap();
        let _ = p.update(Message::Loaded(Ok(rows)));
        let _ = p.update(Message::Select("pine".into()));
        // A fetch for a peer no longer selected is dropped.
        let _ = p.update(Message::MetricsLoaded {
            host: "oak".into(),
            result: Ok(PeerMetrics::default()),
        });
        assert!(p.metrics.is_none());
        let _ = p.update(Message::MetricsLoaded {
            host: "pine".into(),
            result: Ok(PeerMetrics {
                cpu: vec![1.0],
                ..PeerMetrics::default()
            }),
        });
        assert!(p.metrics.is_some());
    }

    #[test]
    fn stop_arms_then_fires_on_second_click_l16() {
        let mut p = PeersPanel::new();
        let msg = Message::Lifecycle {
            host: "oak".into(),
            kind: "vm".into(),
            name: "win11".into(),
            op: "stop".into(),
        };
        let _ = p.update(msg.clone());
        assert!(p.pending_confirm.is_some(), "first click arms");
        assert!(p.op_result.contains("confirm"));
        let _ = p.update(msg);
        assert!(p.pending_confirm.is_none(), "second click fires + disarms");
        assert!(p.op_result.contains("stop win11"));
    }

    #[test]
    fn start_is_one_click_and_different_op_rearms() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Lifecycle {
            host: "oak".into(),
            kind: "container".into(),
            name: "nginx".into(),
            op: "start".into(),
        });
        assert!(p.pending_confirm.is_none(), "start never arms");
        // Arming stop then clicking restart re-arms for restart.
        for op in ["stop", "restart"] {
            let _ = p.update(Message::Lifecycle {
                host: "oak".into(),
                kind: "container".into(),
                name: "nginx".into(),
                op: op.into(),
            });
        }
        assert_eq!(
            p.pending_confirm.as_ref().map(|c| c.3.as_str()),
            Some("restart"),
            "switching ops re-arms instead of firing"
        );
    }

    #[test]
    fn structured_lifecycle_entries_parse() {
        let rows = parse_directory(REPLY).unwrap();
        assert_eq!(
            rows[0].containers,
            [("nginx".to_string(), "running".to_string())]
        );
        assert_eq!(rows[0].vms, [("win11".to_string(), "running".to_string())]);
    }

    #[test]
    fn wake_results_land_in_the_strip() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::WakeFinished {
            host: "oak".into(),
            ok: true,
        });
        assert!(p.op_result.contains("magic packet sent"));
    }

    #[test]
    fn nudge_results_land_in_the_strip() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::NudgeFinished {
            host: "oak".into(),
            ok: true,
        });
        assert!(p.op_result.contains("nudged"));
        let _ = p.update(Message::NudgeFinished {
            host: "oak".into(),
            ok: false,
        });
        assert!(p.op_result.contains("failed"));
    }

    #[test]
    fn op_results_land_in_the_strip() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::OpFinished {
            label: "SSH",
            host: "10.42.0.2".into(),
            ok: false,
        });
        assert!(p.op_result.contains("failed to launch"));
        assert!(p.op_result.contains("cosmic-term"));
    }

    #[test]
    fn parse_devices_reads_roster_and_degrades_on_garbage() {
        // PD-3/L6 — the `action/connect/devices` reply is a JSON array.
        let raw = r#"[
            {"id":"d1","name":"Pixel","online":true,"battery":83},
            {"id":"d2","name":"","online":false,"battery":null}
        ]"#;
        let devs = parse_devices(raw);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].name, "Pixel");
        assert!(devs[0].online);
        assert_eq!(devs[0].battery, Some(83));
        // Empty name falls back to the id; null battery → None.
        assert_eq!(devs[1].name, "d2");
        assert!(!devs[1].online);
        assert_eq!(devs[1].battery, None);
        // A non-array / bad reply degrades to an empty roster.
        assert!(parse_devices("not json").is_empty());
        assert!(parse_devices(r#"{"ok":false}"#).is_empty());
    }

    #[test]
    fn select_device_and_peer_are_mutually_exclusive_l6() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        let _ = p.update(Message::DevicesLoaded(parse_devices(
            r#"[{"id":"d1","name":"Pixel","online":true,"battery":50}]"#,
        )));
        let _ = p.update(Message::Select("pine".into()));
        assert_eq!(p.selected.as_deref(), Some("pine"));
        // Selecting a device clears the peer selection…
        let _ = p.update(Message::SelectDevice("d1".into()));
        assert_eq!(p.selected_device.as_deref(), Some("d1"));
        assert!(p.selected.is_none());
        // …and selecting a peer clears the device selection.
        let _ = p.update(Message::Select("oak".into()));
        assert!(p.selected_device.is_none());
        assert_eq!(p.selected.as_deref(), Some("oak"));
    }

    #[test]
    fn stale_device_selection_drops_when_unpaired() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::DevicesLoaded(parse_devices(
            r#"[{"id":"d1","name":"Pixel","online":true,"battery":50}]"#,
        )));
        let _ = p.update(Message::SelectDevice("d1".into()));
        assert_eq!(p.selected_device.as_deref(), Some("d1"));
        // d1 unpairs (no longer in the roster) → selection drops.
        let _ = p.update(Message::DevicesLoaded(vec![]));
        assert!(p.selected_device.is_none());
    }

    #[test]
    fn device_action_results_land_in_the_strip() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::DeviceActionFinished {
            name: "Pixel".into(),
            verb: "Ring",
            ok: true,
        });
        assert!(p.op_result.contains("ringing now"));
        let _ = p.update(Message::DeviceActionFinished {
            name: "Pixel".into(),
            verb: "Send-file",
            ok: false,
        });
        assert!(p.op_result.contains("failed"));
        let _ = p.update(Message::DeviceActionFinished {
            name: "Pixel".into(),
            verb: "Send-file-cancel",
            ok: false,
        });
        assert!(p.op_result.contains("cancelled"));
    }

    #[test]
    fn device_card_renders_without_panic() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        let _ = p.update(Message::DevicesLoaded(parse_devices(
            r#"[{"id":"d1","name":"Pixel","online":true,"battery":50}]"#,
        )));
        let _ = p.update(Message::SelectDevice("d1".into()));
        let _ = p.view();
    }

    #[test]
    fn edge_click_opens_a_fresh_trace_card_l19_20() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        p.rtt.insert("pine".into(), Some(12.0));
        let _ = p.update(Message::EdgeClicked("pine".into()));
        assert_eq!(p.traced_edge.as_deref(), Some("pine"));
        // Seeded from the latency cache so the sparkline isn't empty.
        assert_eq!(p.trace_rtt, vec![12.0]);
        // A live sample appends; a stale one (other host) is dropped.
        let _ = p.update(Message::TraceRttSampled {
            host: "pine".into(),
            rtt_ms: Some(15.0),
        });
        assert_eq!(p.trace_rtt, vec![12.0, 15.0]);
        let _ = p.update(Message::TraceRttSampled {
            host: "oak".into(),
            rtt_ms: Some(99.0),
        });
        assert_eq!(p.trace_rtt, vec![12.0, 15.0], "stale sample dropped");
        // Re-opening on another edge resets the session series.
        let _ = p.update(Message::EdgeClicked("oak".into()));
        assert!(p.trace_rtt.is_empty() || p.trace_rtt.len() == 1);
        // Close clears the card.
        let _ = p.update(Message::CloseTrace);
        assert!(p.traced_edge.is_none());
        assert!(p.trace_rtt.is_empty());
    }

    #[test]
    fn traceroute_result_lands_for_the_open_edge_only() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::EdgeClicked("pine".into()));
        let _ = p.update(Message::TracerouteDone {
            host: "pine".into(),
            hops: Ok(vec!["1: gateway".into(), "2: peer".into()]),
        });
        assert!(matches!(&p.traceroute, Some(Ok(h)) if h.len() == 2));
        // A result for a different (now-closed) edge is ignored.
        let _ = p.update(Message::EdgeClicked("oak".into()));
        let _ = p.update(Message::TracerouteDone {
            host: "pine".into(),
            hops: Ok(vec!["stale".into()]),
        });
        assert!(p.traceroute.is_none(), "stale traceroute dropped");
    }

    #[test]
    fn flow_anim_advances_phase_and_has_flow_gates_the_loop_l18_22() {
        let mut p = PeersPanel::new();
        // No flow + not in map view → no animation loop.
        assert!(!p.has_flow());
        p.map_view = true;
        // MESHMAP-6 — a real per-link flow (proxy or collector) arms the loop.
        // `FlowsSampled` now MERGES the proxy-fallback subset (the real flows
        // are seeded in FlowTick), so seed directly here.
        p.flows.insert(
            "pine".to_string(),
            crate::panels::peers_map::LinkFlow { tx: 0.5, rx: 0.0 },
        );
        assert!(p.has_flow(), "real traffic arms the animation loop");
        let before = p.flow_phase;
        let _ = p.update(Message::FlowAnim);
        assert!(p.flow_phase > before, "phase advances");
        // MESHMAP-6 — the reverse (rx) stream alone also arms the loop.
        p.flows.clear();
        p.flows.insert(
            "oak".to_string(),
            crate::panels::peers_map::LinkFlow { tx: 0.0, rx: 0.4 },
        );
        assert!(p.has_flow(), "a peer→self stream arms the loop too");
        // Idle traffic disarms the loop (L22 — idle mesh, idle CPU).
        p.flows.clear();
        assert!(!p.has_flow());
    }

    #[test]
    fn trace_card_renders_without_panic() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_directory(REPLY).unwrap())));
        p.map_view = true;
        let _ = p.update(Message::EdgeClicked("pine".into()));
        let _ = p.update(Message::TracerouteDone {
            host: "pine".into(),
            hops: Ok(vec!["1: 10.42.0.1".into()]),
        });
        let _ = p.view();
    }

    #[test]
    fn unreachable_mackesd_is_the_l3_state() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Err("mackesd unreachable".into())));
        assert!(matches!(&p.loaded, Some(Err(e)) if e.contains("unreachable")));
        let _ = p.view(); // renders the guided state without panic
    }

    #[test]
    fn unify16_voice_tile_state_maps_the_real_statuses() {
        // No snapshot (non-self peer, or no agent) → honest dash, neutral token.
        assert_eq!(voice_tile_state(None), ("—".to_string(), carbon::GRAY_50));
        // Fresh + registered + listening → Registered (green).
        let reg = VoiceStatus {
            registered: true,
            listening: true,
            fresh: true,
        };
        assert_eq!(
            voice_tile_state(Some(&reg)),
            ("Registered".to_string(), carbon::GREEN_40)
        );
        // Fresh, listening only → Listening (amber).
        let listening = VoiceStatus {
            registered: false,
            listening: true,
            fresh: true,
        };
        assert_eq!(
            voice_tile_state(Some(&listening)),
            ("Listening".to_string(), carbon::YELLOW_30)
        );
        // Fresh but neither → Offline (red).
        let idle = VoiceStatus {
            registered: false,
            listening: false,
            fresh: true,
        };
        assert_eq!(
            voice_tile_state(Some(&idle)),
            ("Offline".to_string(), carbon::RED_50)
        );
        // A stale heartbeat (agent stopped) is offline regardless of last state.
        let stale = VoiceStatus {
            registered: true,
            listening: true,
            fresh: false,
        };
        assert_eq!(
            voice_tile_state(Some(&stale)),
            ("Offline".to_string(), carbon::RED_50)
        );
    }

    #[test]
    fn unify16_voice_loaded_sets_the_local_presence() {
        let mut p = PeersPanel::new();
        assert!(p.voice.is_none());
        let _ = p.update(Message::VoiceLoaded(Some(VoiceStatus {
            registered: true,
            listening: true,
            fresh: true,
        })));
        assert_eq!(
            p.voice,
            Some(VoiceStatus {
                registered: true,
                listening: true,
                fresh: true,
            })
        );
        // `peer_detail_body` passes `Some(voice)` for the self row and `None`
        // for every other peer: the self row shows the real value, others "—".
        assert_eq!(voice_tile_state(p.voice.as_ref()).0, "Registered");
        assert_eq!(voice_tile_state(None).0, "—");
    }
}

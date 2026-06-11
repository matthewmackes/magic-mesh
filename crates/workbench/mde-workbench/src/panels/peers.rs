//! PD-3 — the **Peers** directory panel: the platform Front Door.
//!
//! Master-detail over `action/mesh/directory` (PD-1): peer list left
//! — self pinned "(this machine)" first, then Online, Idle, Offline
//! (grayed) groups — detail pane right with the identity header,
//! presence + health, version + revision currency, and the Services
//! Provided inventory (PD-2 descriptors: remote access, Podman
//! containers, libvirt guests, media). A type-to-filter box matches
//! hostname or offered service (L2). Degraded mesh states render the
//! guided empty states (L3): no mackesd → "Start the mesh service",
//! empty roster → "Invite a peer".
//!
//! Per-peer ops (Call/SSH/RDP/VNC, PD-5), tag chips (L1 — tags join
//! the directory record with the tag-manifest merge), device rows
//! (L6) and the live map (PD-7) layer onto this surface in their own
//! tasks. The legacy Mesh Topology panel keeps the graph until PD-7
//! absorbs it.

use std::time::Duration;

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Background, Border, Element, Length, Padding, Task};
use mde_theme::TypeRole;

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

/// Panel state.
#[derive(Debug, Clone, Default)]
pub struct PeersPanel {
    pub rows: Vec<PeerRow>,
    pub filter: String,
    pub selected: Option<String>,
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
    FilterChanged(String),
    Select(String),
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

/// Filter predicate (L2): hostname OR capability tag (L1) OR any
/// offered-service line.
#[must_use]
pub fn matches_filter(row: &PeerRow, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    row.hostname.to_lowercase().contains(&f)
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
pub fn metrics_subscription() -> iced::Subscription<crate::Message> {
    iced::time::every(Duration::from_secs(2)).map(|_| crate::Message::Peers(Message::MetricsTick))
}

/// PD-3/Q10 — the view-gated 30 s directory-refresh tick (registered
/// by `App::subscription` only while the Peers panel is the active
/// view). Re-reads `action/mesh/directory` so presence/health/tags
/// stay current without an operator click; the reload preserves the
/// current filter + selection.
#[must_use]
pub fn directory_subscription() -> iced::Subscription<crate::Message> {
    iced::time::every(Duration::from_secs(30)).map(|_| crate::Message::Peers(Message::PollTick))
}

/// PD-3/Q10 — the **Bus-push** half: subscribe to the directory-changed
/// event the responder publishes (`event/mesh/directory`) and reload the
/// instant the roster changes, instead of waiting out the 30 s poll. The
/// poll stays registered as a backstop in case an event is missed.
const DIRECTORY_EVENT_TOPIC: &str = "event/mesh/directory";

#[must_use]
pub fn directory_event_subscription() -> iced::Subscription<crate::Message> {
    use iced::futures::SinkExt;
    iced::Subscription::run(|| {
        iced::stream::channel(
            8,
            |mut output: iced::futures::channel::mpsc::Sender<crate::Message>| async move {
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
        let dir = mde_bus::default_data_dir()?;
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
        let Some(dir) = mde_bus::default_data_dir() else {
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
            ..Self::default()
        }
    }

    /// Fetch the directory (the Bus client needs its own thread —
    /// same contract as the home-panel probes).
    pub fn load() -> Task<crate::Message> {
        Task::perform(
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
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(result) => {
                match result {
                    Ok(rows) => {
                        self.rows = rows;
                        self.loaded = Some(Ok(()));
                        if self.selected.is_none() {
                            // Default-select self, else the first row.
                            let self_row = self
                                .rows
                                .iter()
                                .find(|r| r.hostname == self.self_hostname)
                                .or_else(|| self.rows.first());
                            self.selected = self_row.map(|r| r.hostname.clone());
                        }
                    }
                    Err(e) => self.loaded = Some(Err(e)),
                }
                Task::none()
            }
            Message::FilterChanged(f) => {
                self.filter = f;
                Task::none()
            }
            Message::Select(host) => {
                self.selected = Some(host);
                self.metrics = None;
                self.metrics_err = None;
                // A map-node click lands you on the peer's detail (W87).
                self.map_view = false;
                Task::none()
            }
            Message::ToggleMap => {
                self.map_view = !self.map_view;
                if self.map_view {
                    self.rtt = super::peers_map::read_latency_cache();
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
                            if let Some(dir) = mde_bus::default_data_dir() {
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
            .color(palette.text.into_iced_color());

        // L3 — guided empty states.
        match &self.loaded {
            None => {
                return shell(title, text("Loading the mesh directory…").into(), palette);
            }
            Some(Err(e)) => {
                let body = column![
                    text("The mesh service isn't answering.")
                        .size(16)
                        .color(palette.text.into_iced_color()),
                    text(e.clone())
                        .size(12)
                        .color(palette.text_muted.into_iced_color()),
                    text("Start it from Network → Mesh Services, then refresh.")
                        .size(13)
                        .color(palette.text_muted.into_iced_color()),
                    refresh_btn(palette),
                ]
                .spacing(8);
                return shell(title, body.into(), palette);
            }
            Some(Ok(())) if self.rows.is_empty() => {
                let body = column![
                    text("No peers in this mesh yet.")
                        .size(16)
                        .color(palette.text.into_iced_color()),
                    text("Invite a peer: mint a join token with `mackesd enroll-token` and run `mackesd enroll --token …` on the new box.")
                        .size(13)
                        .color(palette.text_muted.into_iced_color()),
                    refresh_btn(palette),
                ]
                .spacing(8);
                return shell(title, body.into(), palette);
            }
            Some(Ok(())) => {}
        }

        // Left: filter + grouped list.
        let filter_box = text_input("Filter peers or services…", &self.filter)
            .on_input(|f| crate::Message::Peers(Message::FilterChanged(f)))
            .padding(Padding::from([6u16, 10u16]))
            .width(Length::Fill);
        let mut list = column![].spacing(4);
        for group in ["This machine", "Online", "Idle", "Offline"] {
            let members: Vec<&PeerRow> = self
                .rows
                .iter()
                .filter(|r| group_of(r, &self.self_hostname) == group)
                .filter(|r| matches_filter(r, &self.filter))
                .collect();
            if members.is_empty() {
                continue;
            }
            list = list.push(
                text(group)
                    .size(11)
                    .color(palette.text_muted.into_iced_color()),
            );
            for r in members {
                let selected = self.selected.as_deref() == Some(r.hostname.as_str());
                let dimmed = group == "Offline";
                let label = format!("{} {}", presence_pip(&r.presence), r.hostname);
                let fg = if dimmed {
                    palette.text_muted
                } else {
                    palette.text
                };
                let bg = if selected {
                    palette.raised
                } else {
                    palette.surface
                };
                list = list.push(
                    button(text(label).size(13).color(fg.into_iced_color()))
                        .width(Length::Fill)
                        .padding(Padding::from([6u16, 10u16]))
                        .style(move |_t, _s| iced::widget::button::Style {
                            snap: false,
                            background: Some(Background::Color(bg.into_iced_color())),
                            text_color: fg.into_iced_color(),
                            border: Border {
                                color: iced::Color::TRANSPARENT,
                                width: 0.0,
                                radius: 4.0.into(),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(crate::Message::Peers(Message::Select(r.hostname.clone()))),
                );
            }
        }
        let left = column![filter_box, scrollable(list).height(Length::Fill)]
            .spacing(8)
            .width(Length::Fixed(240.0));

        // Right: the detail pane.
        let detail: Element<'_, crate::Message> = match self
            .rows
            .iter()
            .find(|r| Some(r.hostname.as_str()) == self.selected.as_deref())
        {
            None => text("Select a peer.")
                .color(palette.text_muted.into_iced_color())
                .into(),
            Some(r) => {
                let header = row![
                    text(&r.hostname)
                        .size(20)
                        .color(palette.text.into_iced_color()),
                    Space::new().width(Length::Fixed(10.0)),
                    badge(
                        if r.role.is_empty() {
                            "role: -"
                        } else {
                            &r.role
                        },
                        palette
                    ),
                    Space::new().width(Length::Fill),
                    refresh_btn(palette),
                ]
                .align_y(iced::alignment::Vertical::Center);
                // L1 — capability-tag chips; honest absence when none.
                let tags_row: Element<'_, crate::Message> = if r.tags.is_empty() {
                    Space::new().height(Length::Fixed(0.0)).into()
                } else {
                    let mut chips = row![text("Tags")
                        .size(11)
                        .color(palette.text_muted.into_iced_color())]
                    .spacing(6)
                    .align_y(iced::alignment::Vertical::Center);
                    for t in &r.tags {
                        chips = chips.push(badge(t.as_str(), palette));
                    }
                    chips.into()
                };
                // PD-5 — the op toolbar, descriptor- + presence-gated.
                let mut ops = row![].spacing(8);
                // PD-5 — Call (voice): rings an online, non-self peer via
                // the voice HUD's Bus dial subscriber.
                let can_call = r.presence != "offline" && r.hostname != self.self_hostname;
                ops = ops.push(crate::controls::variant_button(
                    "Call",
                    crate::controls::ButtonVariant::Secondary,
                    can_call
                        .then(|| crate::Message::Peers(Message::CallClicked(r.hostname.clone()))),
                    palette,
                ));
                for (offered, proto) in [
                    (r.ssh, crate::launcher::Protocol::Ssh),
                    (r.rdp, crate::launcher::Protocol::Rdp),
                    (r.vnc, crate::launcher::Protocol::Vnc),
                ] {
                    let target = r.overlay_ip.clone();
                    let target = if target.is_empty() {
                        r.hostname.clone()
                    } else {
                        target
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
                let strip: Element<'_, crate::Message> = if self.op_result.is_empty() {
                    Space::new().height(Length::Fixed(0.0)).into()
                } else {
                    text(self.op_result.clone())
                        .size(12)
                        .color(palette.text_muted.into_iced_color())
                        .into()
                };
                // PD-12 — Wake is the one op an offline peer offers (L4).
                let wake: Element<'_, crate::Message> =
                    match (r.presence.as_str(), r.lan_macs.first()) {
                        ("offline", Some(mac)) => crate::controls::variant_button(
                            "Wake",
                            crate::controls::ButtonVariant::Primary,
                            Some(crate::Message::Peers(Message::WakeClicked {
                                host: r.hostname.clone(),
                                mac: mac.clone(),
                            })),
                            palette,
                        ),
                        _ => Space::new().height(Length::Fixed(0.0)).into(),
                    };
                // PD-9 — Apply now appears only for a behind peer (Q16).
                let nudge: Element<'_, crate::Message> = if r.currency == "behind" {
                    crate::controls::variant_button(
                        "Apply now",
                        crate::controls::ButtonVariant::Primary,
                        Some(crate::Message::Peers(Message::NudgeClicked(
                            r.hostname.clone(),
                        ))),
                        palette,
                    )
                } else {
                    Space::new().height(Length::Fixed(0.0)).into()
                };
                let facts = column![
                    fact("Presence", &r.presence, palette),
                    fact("Health", &r.health, palette),
                    fact(
                        "Overlay IP",
                        if r.overlay_ip.is_empty() {
                            "-"
                        } else {
                            &r.overlay_ip
                        },
                        palette
                    ),
                    fact(
                        "Version",
                        if r.version.is_empty() {
                            "-"
                        } else {
                            &r.version
                        },
                        palette
                    ),
                    fact("Revision", &r.currency, palette),
                    nudge,
                ]
                .spacing(4);
                // PD-8 — live Netdata block (L14): four sparklines
                // while the 2s tick feeds; honest absence otherwise.
                let mut metrics_col = column![row![
                    text("Live metrics")
                        .size(13)
                        .color(palette.text.into_iced_color()),
                    Space::new().width(Length::Fixed(10.0)),
                    if !r.overlay_ip.is_empty() && r.presence != "offline" {
                        crate::controls::variant_button(
                            "Metrics ↗",
                            crate::controls::ButtonVariant::Secondary,
                            Some(crate::Message::Peers(Message::OpenDashboard(
                                r.overlay_ip.clone(),
                            ))),
                            palette,
                        )
                    } else {
                        Space::new().height(Length::Fixed(0.0)).into()
                    },
                ]
                .align_y(iced::alignment::Vertical::Center)]
                .spacing(4);
                match (&self.metrics, &self.metrics_err) {
                    (Some(m), _) => {
                        for (label, series) in [
                            ("CPU %", &m.cpu),
                            ("Load", &m.load),
                            ("Net", &m.net),
                            ("Disk I/O", &m.disk),
                        ] {
                            let last = series.last().copied().unwrap_or(0.0);
                            metrics_col = metrics_col.push(
                                row![
                                    text(label)
                                        .size(12)
                                        .width(Length::Fixed(100.0))
                                        .color(palette.text_muted.into_iced_color()),
                                    text(sparkline(series))
                                        .size(12)
                                        .color(palette.accent.into_iced_color()),
                                    text(format!(" {last:.1}"))
                                        .size(12)
                                        .color(palette.text.into_iced_color()),
                                ]
                                .align_y(iced::alignment::Vertical::Center),
                            );
                        }
                    }
                    (None, Some(e)) => {
                        metrics_col = metrics_col.push(
                            text(format!("Netdata not answering on this peer: {e}"))
                                .size(12)
                                .color(palette.text_muted.into_iced_color()),
                        );
                    }
                    (None, None) => {
                        metrics_col = metrics_col.push(
                            text(if r.presence == "offline" {
                                "Peer offline — no live metrics."
                            } else {
                                "Waiting for the first sample…"
                            })
                            .size(12)
                            .color(palette.text_muted.into_iced_color()),
                        );
                    }
                }

                let mut services = column![text("Services provided")
                    .size(13)
                    .color(palette.text.into_iced_color())]
                .spacing(4);
                // PD-11 — lifecycle rows: start one-click; stop/
                // restart armed-confirm (L16). Self excluded (local
                // service control lives in Mesh Services).
                if r.hostname != self.self_hostname {
                    for (kind, list) in [("container", &r.containers), ("vm", &r.vms)] {
                        for (name, state) in list {
                            let running = state == "running";
                            let mut ops_row = row![text(format!("{kind}: {name} ({state})"))
                                .size(12)
                                .color(palette.text_muted.into_iced_color())]
                            .spacing(8)
                            .align_y(iced::alignment::Vertical::Center);
                            let mk = |op: &str| {
                                crate::Message::Peers(Message::Lifecycle {
                                    host: r.hostname.clone(),
                                    kind: kind.to_string(),
                                    name: name.clone(),
                                    op: op.to_string(),
                                })
                            };
                            if running {
                                let armed = |op: &str| {
                                    self.pending_confirm
                                        == Some((
                                            r.hostname.clone(),
                                            kind.to_string(),
                                            name.clone(),
                                            op.to_string(),
                                        ))
                                };
                                ops_row = ops_row.push(crate::controls::variant_button(
                                    if armed("stop") {
                                        "Confirm stop"
                                    } else {
                                        "Stop"
                                    },
                                    crate::controls::ButtonVariant::Secondary,
                                    Some(mk("stop")),
                                    palette,
                                ));
                                ops_row = ops_row.push(crate::controls::variant_button(
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
                                ops_row = ops_row.push(crate::controls::variant_button(
                                    "Start",
                                    crate::controls::ButtonVariant::Secondary,
                                    Some(mk("start")),
                                    palette,
                                ));
                            }
                            services = services.push(ops_row);
                        }
                    }
                }
                if r.services.is_empty() {
                    services = services.push(
                        text("Nothing published (peer pre-PD-2, or nothing offered).")
                            .size(12)
                            .color(palette.text_muted.into_iced_color()),
                    );
                } else {
                    for s in &r.services {
                        services = services.push(
                            text(format!("• {s}"))
                                .size(12)
                                .color(palette.text_muted.into_iced_color()),
                        );
                    }
                }
                column![
                    header,
                    tags_row,
                    ops,
                    wake,
                    strip,
                    facts,
                    metrics_col,
                    services
                ]
                .spacing(16)
                .into()
            }
        };
        let right = container(scrollable(detail))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding::from([0u16, 16u16]));

        // PD-7 — the Map view replaces the master-detail body; a node
        // click selects the peer and returns to the detail view.
        let toggle = crate::controls::variant_button(
            if self.map_view {
                "List view"
            } else {
                "Map view"
            },
            crate::controls::ButtonVariant::Secondary,
            Some(crate::Message::Peers(Message::ToggleMap)),
            palette,
        );
        if self.map_view {
            let nodes: Vec<super::peers_map::MapNode> = self
                .rows
                .iter()
                .map(|r| super::peers_map::MapNode {
                    hostname: r.hostname.clone(),
                    presence: r.presence.clone(),
                    rtt_ms: self.rtt.get(&r.hostname).copied().flatten(),
                    is_self: r.hostname == self.self_hostname,
                })
                .collect();
            let positions = super::peers_map::layout(&nodes);
            let canvas: Element<'_, crate::Message> =
                iced::widget::canvas(super::peers_map::MapProgram {
                    nodes,
                    positions,
                    palette,
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
            let body = column![toggle, canvas].spacing(8).height(Length::Fill);
            return shell(title, body.into(), palette);
        }
        let body = column![toggle, row![left, right].spacing(16).height(Length::Fill)]
            .spacing(8)
            .height(Length::Fill);
        shell(title, body.into(), palette)
    }
}

fn shell<'a>(
    title: iced::widget::Text<'a, iced::Theme>,
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
            .color(palette.text_muted.into_iced_color()),
        text(value).size(12).color(palette.text.into_iced_color()),
    ]
    .into()
}

fn badge<'a>(label: &'a str, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    container(text(label).size(11).color(palette.text.into_iced_color()))
        .padding(Padding::from([2u16, 8u16]))
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(palette.raised.into_iced_color())),
            border: Border {
                color: palette.border.into_iced_color(),
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

fn presence_pip(presence: &str) -> &'static str {
    match presence {
        "online" => "●",
        "idle" => "◐",
        _ => "○",
    }
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
        assert!(p.selected.is_some(), "something selected by default");
        let _ = p.update(Message::Select("oak".into()));
        assert_eq!(p.selected.as_deref(), Some("oak"));
        let _ = p.update(Message::FilterChanged("podman".into()));
        assert_eq!(p.filter, "podman");
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
    fn unreachable_mackesd_is_the_l3_state() {
        let mut p = PeersPanel::new();
        let _ = p.update(Message::Loaded(Err("mackesd unreachable".into())));
        assert!(matches!(&p.loaded, Some(Err(e)) if e.contains("unreachable")));
        let _ = p.view(); // renders the guided state without panic
    }
}

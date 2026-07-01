//! Workbench · Network — live mesh-network status (WB-Network).
//!
//! The Network plane, wired off the SAME world-readable mesh-status snapshot the
//! chrome bar + This Node plane fold (`/run/mde/mesh-status.json`, written every
//! ~30s by the root `mesh-status.timer`). The desktop user can't read the
//! root-only replicated peer directory, so this JSON is the desktop tier's read
//! path — the shell leans on no `mackesd` IPC (§6). Every field here is real,
//! live-updating network reality; nothing is a stand-in (§7):
//!
//! * **Overlay** — the Nebula fabric this node rides: its overlay `overlay_ip`,
//!   the tunnel `overlay_if`, the overlay subnet (`overlay_cidr`), the tunnel
//!   `cipher`, and the elected mesh `leader` (with a "this node is leader" chip
//!   when this node holds the lease).
//! * **Mesh links** — the peer directory rendered as network links: per-peer
//!   overlay IP + directory `presence` (online / idle / offline), the live
//!   online / total link count, and a lighthouse chip on the anchor nodes
//!   (`overlay_ip ∈ lighthouse_ips`, or `role == lighthouse` — the LIGHTHOUSE-9
//!   membership signal the snapshot generator stamps).
//! * **Network services** — the network-scoped subset of this node's own
//!   `services` map (Overlay / Nebula, mesh DNS, Syncthing), the same map each
//!   node publishes into its `shell-status.json`.
//! * **Routing & reachability** — the lighthouse public endpoints
//!   (`gateway_endpoints`), the overlay-routable subnets (`routes`), and the
//!   node's `default_gw`, when the snapshot carries them.
//!
//! What this surface honestly **cannot** show: live per-link throughput, latency,
//! or per-tunnel handshake state. Those aren't in the world-readable snapshot —
//! they're live Nebula tunnel telemetry, and §6 keeps the shell off that path.
//! The panel renders an explicit "not published to this surface" note rather than
//! a fabricated gauge (§7), exactly as This Node did for CPU / memory / disk.
//!
//! `project` is pure (no IO, no egui, no GPU), so it's unit-tested directly; the
//! only IO is the snapshot read in [`NetworkState::poll`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use serde_json::Value;

/// The world-readable mesh-status snapshot — the same source the chrome bar +
/// This Node plane read (the desktop user can't read the root-only replicated
/// peer directory).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a peer join/leave, a leader change, or a service flip surfaces
/// within this window. Matches the chrome bar + the This Node / Fleet poll; the
/// read is a cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip
/// / This Node use, so a link dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

/// The network-scoped subset of a node's `services` map: the daemons that make up
/// the mesh network fabric (the overlay tunnel, mesh DNS, the replicated share),
/// paired with the label the plane renders. Fixed order so the list is stable
/// frame-to-frame; a key absent from the snapshot is simply not listed (never a
/// false "down").
const NET_SERVICE_CATALOG: [(&str, &str); 3] = [
    ("nebula", "Overlay (Nebula)"),
    ("dns", "Mesh DNS"),
    ("sync", "Sync (Syncthing)"),
];

// ──────────────────────────── projected view ────────────────────────────

/// One peer rendered as a network link: hostname, overlay address, directory
/// presence, and whether it anchors the overlay as a lighthouse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PeerLink {
    /// The peer's hostname (the directory key).
    hostname: String,
    /// The peer's Nebula overlay IP, when known.
    overlay_ip: Option<String>,
    /// Directory presence tier: `online` / `idle` / `offline`, when known.
    presence: Option<String>,
    /// `true` when this peer anchors the overlay — its overlay IP is in the
    /// `lighthouse_ips` set, or its `role` is `lighthouse` (the LIGHTHOUSE-9
    /// membership signal: anchor nodes run as Server tier, so role alone
    /// under-reports).
    is_lighthouse: bool,
    /// `true` when this link is this node's own directory row.
    is_self: bool,
}

/// The mesh network's live status, folded from the mesh-status snapshot. Pure data
/// (parsed without egui/IO/GPU), so it's unit-tested directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NetStatus {
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the connecting state) from a parsed one.
    seen: bool,
    /// This node's hostname — the snapshot's `self` marker (local hostname when the
    /// snapshot omits it). Used to resolve the leader chip + the own-row services.
    hostname: String,
    /// This node's Nebula overlay IP (the network overview's locally-probed
    /// address, falling back to this node's directory row), when known.
    overlay_ip: Option<String>,
    /// The overlay tunnel interface (e.g. `nebula1`), when known.
    overlay_if: Option<String>,
    /// The overlay subnet (the connected kernel route on the tunnel), when known.
    overlay_cidr: Option<String>,
    /// The Nebula tunnel cipher label, when nebula is up.
    cipher: Option<String>,
    /// The elected mesh leader's hostname, when one holds the lease.
    leader: Option<String>,
    /// The peer directory as network links (every node the snapshot names).
    peers: Vec<PeerLink>,
    /// This node's own network-scoped daemon health, in catalog order (label, up).
    services: Vec<(&'static str, bool)>,
    /// The lighthouse public endpoints (`ip:port`) — the overlay's external
    /// reachability anchors, when the snapshot carries them.
    gateway_endpoints: Vec<String>,
    /// The subnets routable through the overlay (overlay subnet + `unsafe_routes`).
    routes: Vec<String>,
    /// The node's default gateway, when known.
    default_gw: Option<String>,
}

/// Read a non-empty string field off a JSON object, or `None`.
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read a `key` array of non-empty strings off an optional JSON object. A missing
/// object / key / non-array yields an empty list (the view then says "not
/// published" rather than fabricating a route).
fn string_list(obj: Option<&Value>, key: &str) -> Vec<String> {
    obj.and_then(|v| v.get(key))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the network-scoped subset of the `services` map into catalog-ordered
/// (label, up) rows actually present. A missing map yields an empty list → the
/// view says "not yet reported" rather than a false all-down.
fn parse_net_services(services: Option<&Value>) -> Vec<(&'static str, bool)> {
    let Some(obj) = services.and_then(Value::as_object) else {
        return Vec::new();
    };
    NET_SERVICE_CATALOG
        .iter()
        .filter_map(|(key, label)| {
            obj.get(*key)
                .and_then(Value::as_bool)
                .map(|up| (*label, up))
        })
        .collect()
}

impl NetStatus {
    /// Fold the mesh-status snapshot into the network's status. `fallback_host` is
    /// the locally-resolved hostname, used only when the snapshot omits its `self`
    /// marker (so the leader chip + own-row services still resolve). A missing /
    /// garbage / non-mesh snapshot yields the honest unseen status (drives the
    /// connecting state), never a panic — mirroring the chrome bar's tolerance.
    fn project(snapshot: &str, fallback_host: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(snapshot) else {
            return Self::default();
        };
        let self_host = nonempty(&v, "self");
        let nodes = v.get("nodes").and_then(Value::as_array);
        // A real snapshot names at least `self` or a `nodes` array; anything else
        // (an empty object, an array, a fragment) reads as unseen.
        if self_host.is_none() && nodes.is_none() {
            return Self::default();
        }

        let hostname = self_host.unwrap_or_else(|| fallback_host.to_string());
        let network = v.get("network");
        let lighthouse_ips = string_list(network, "lighthouse_ips");

        // The peer directory as network links. A lighthouse is an overlay anchor —
        // its IP is in `lighthouse_ips` OR its role is `lighthouse` (LIGHTHOUSE-9).
        let peers: Vec<PeerLink> = nodes
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| {
                        let host = nonempty(n, "hostname")?;
                        let overlay_ip = nonempty(n, "overlay_ip");
                        let is_lighthouse = nonempty(n, "role").as_deref() == Some("lighthouse")
                            || overlay_ip
                                .as_deref()
                                .is_some_and(|ip| lighthouse_ips.iter().any(|l| l == ip));
                        Some(PeerLink {
                            is_self: host == hostname,
                            is_lighthouse,
                            presence: nonempty(n, "presence"),
                            overlay_ip,
                            hostname: host,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // This node's own directory row → its network-scoped service subset.
        let own = nodes.and_then(|arr| {
            arr.iter()
                .find(|n| n.get("hostname").and_then(Value::as_str) == Some(hostname.as_str()))
        });

        Self {
            seen: true,
            // Prefer the network overview's locally-probed overlay address; fall
            // back to this node's directory-row overlay IP.
            overlay_ip: network
                .and_then(|n| nonempty(n, "overlay_ip"))
                .or_else(|| own.and_then(|n| nonempty(n, "overlay_ip"))),
            overlay_if: network.and_then(|n| nonempty(n, "overlay_if")),
            overlay_cidr: network.and_then(|n| nonempty(n, "overlay_cidr")),
            cipher: network.and_then(|n| nonempty(n, "cipher")),
            leader: network.and_then(|n| nonempty(n, "leader")),
            services: parse_net_services(own.and_then(|n| n.get("services"))),
            gateway_endpoints: string_list(network, "gateway_endpoints"),
            routes: string_list(network, "routes"),
            default_gw: network.and_then(|n| nonempty(n, "default_gw")),
            peers,
            hostname,
        }
    }

    /// `true` when this node holds the mesh leader lease.
    fn is_leader(&self) -> bool {
        self.leader.as_deref() == Some(self.hostname.as_str())
    }

    /// Links currently `online`.
    fn peers_online(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| p.presence.as_deref() == Some("online"))
            .count()
    }

    /// Links in the directory (every node the snapshot names).
    fn peers_total(&self) -> usize {
        self.peers.len()
    }

    /// `true` when the snapshot carries no overlay routing data at all — no
    /// lighthouse endpoints, no routes, no default gateway.
    fn routing_empty(&self) -> bool {
        self.gateway_endpoints.is_empty() && self.routes.is_empty() && self.default_gw.is_none()
    }
}

/// Directory presence tier → tone: online is healthy, idle warns, offline is a
/// danger, anything else reads dim.
fn presence_tone(presence: &str) -> Color32 {
    match presence {
        "online" => Style::OK,
        "idle" => Style::WARN,
        "offline" => Style::DANGER,
        _ => Style::TEXT_DIM,
    }
}

// ──────────────────────────── the Network state ────────────────────────────

/// The Network plane's live state: the projected status plus the small IO context
/// to refresh it on the shared cadence.
pub(crate) struct NetworkState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// This node's locally-resolved hostname — the fallback `self` when the
    /// snapshot omits it (resolved once).
    local_host: String,
    /// The latest projection. Unseen until the first snapshot lands (drives the
    /// connecting state).
    status: NetStatus,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for NetworkState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            local_host: local_hostname(),
            status: NetStatus::default(),
            last_poll: None,
        }
    }
}

impl NetworkState {
    /// The poll seam: refresh the projection from the snapshot when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a peer join/leave or a
    /// leader flip surfaces without input. Cheap enough to call every frame — it
    /// self-gates. A missing / unreadable snapshot yields the unseen status, never
    /// a panic.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = std::fs::read_to_string(&self.snapshot_path).unwrap_or_default();
            self.status = NetStatus::project(&snapshot, &self.local_host);
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Render the plane's live content into `ui`.
    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        show_status(ui, &self.status);
    }
}

/// The local hostname — `$HOSTNAME` → `/proc/sys/kernel/hostname` (what the
/// snapshot generator stamps as `self`) → `/etc/hostname` → `"localhost"`. Only a
/// fallback: the snapshot's own `self` marker is preferred.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

// ──────────────────────────── render ────────────────────────────

/// Render the mesh network's live status: the connecting state before the first
/// snapshot, else the overlay / links / services / routing cards over an honest
/// telemetry note.
fn show_status(ui: &mut egui::Ui, status: &NetStatus) {
    if !status.seen {
        ui.add_space(Style::SP_S);
        ui.colored_label(Style::TEXT_DIM, "Reading the mesh network status…");
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(
                "The overlay address, tunnel cipher, elected leader, and peer links fold from \
                 the world-readable mesh-status snapshot.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.group(|ui| show_overlay(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Mesh links")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_links(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Network services")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_services(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Routing & reachability")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_routing(ui, status));
            ui.add_space(Style::SP_S);

            // Honest boundary (§6/§7): live per-link tunnel telemetry isn't on this
            // world-readable surface — never fake a gauge.
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(
                    "Live link throughput, latency, and per-tunnel handshake state aren't \
                     published to this surface — the shell reads the mesh directory, not live \
                     Nebula tunnel telemetry.",
                )
                .size(Style::SMALL),
            );
        });
}

/// The overlay card: the Nebula fabric title + a "this node is leader" chip, then
/// the overlay IP, interface, subnet, tunnel cipher, and the elected leader.
fn show_overlay(ui: &mut egui::Ui, status: &NetStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Nebula overlay")
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        if status.is_leader() {
            ui.add_space(Style::SP_S);
            ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
            ui.colored_label(
                Style::OK,
                RichText::new("this node is leader").size(Style::SMALL),
            );
        }
    });
    ui.add_space(Style::SP_XS);

    field(
        ui,
        "Overlay IP",
        status.overlay_ip.as_deref().unwrap_or("—"),
        if status.overlay_ip.is_some() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        },
    );
    if let Some(iface) = &status.overlay_if {
        field(ui, "Interface", iface, Style::TEXT);
    }
    if let Some(cidr) = &status.overlay_cidr {
        field(ui, "Overlay subnet", cidr, Style::TEXT);
    }
    if let Some(cipher) = &status.cipher {
        field(ui, "Tunnel cipher", cipher, Style::TEXT);
    }
    match &status.leader {
        Some(leader) => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Leader")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(Style::TEXT, RichText::new(leader).size(Style::SMALL));
                if status.is_leader() {
                    ui.add_space(Style::SP_XS);
                    ui.colored_label(
                        Style::TEXT_DIM,
                        RichText::new("\u{00B7} this node").size(Style::SMALL),
                    );
                }
            });
        }
        None => field(ui, "Leader", "no leader elected", Style::TEXT_DIM),
    }
}

/// The mesh-links card: the live online / total link count, then one row per peer
/// (presence dot · hostname · this-node / lighthouse chips · overlay IP · presence).
fn show_links(ui: &mut egui::Ui, status: &NetStatus) {
    if status.peers.is_empty() {
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new("No peers in the directory yet.").size(Style::SMALL),
        );
        return;
    }

    let (online, total) = (status.peers_online(), status.peers_total());
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Links")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let tone = if total == 0 {
            Style::TEXT_DIM
        } else if online == total {
            Style::OK
        } else {
            Style::WARN
        };
        ui.colored_label(
            tone,
            RichText::new(format!("{online}/{total} online")).size(Style::SMALL),
        );
    });
    ui.add_space(Style::SP_XS);

    for peer in &status.peers {
        let tone = peer
            .presence
            .as_deref()
            .map_or(Style::TEXT_DIM, presence_tone);
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(&peer.hostname)
                    .color(Style::TEXT)
                    .size(Style::SMALL),
            );
            if peer.is_self {
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new("\u{00B7} this node").size(Style::SMALL),
                );
            }
            if peer.is_lighthouse {
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::ACCENT,
                    RichText::new("lighthouse").size(Style::SMALL),
                );
            }
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(peer.overlay_ip.as_deref().unwrap_or("—")).size(Style::SMALL),
            );
            if let Some(p) = &peer.presence {
                ui.add_space(Style::SP_S);
                ui.colored_label(tone, RichText::new(p).size(Style::SMALL));
            }
        });
    }
}

/// The network-services card: one health row per network-scoped daemon present in
/// the snapshot, or an honest "not yet reported" when this node hasn't published a
/// status record.
fn show_services(ui: &mut egui::Ui, status: &NetStatus) {
    if status.services.is_empty() {
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new("Network service health not yet reported by this node.")
                .size(Style::SMALL),
        );
        return;
    }
    for (label, up) in &status.services {
        ui.horizontal(|ui| {
            let (dot, word, tone) = if *up {
                (Style::OK, "up", Style::TEXT_DIM)
            } else {
                (Style::TEXT_DIM, "down", Style::WARN)
            };
            ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new(*label).color(Style::TEXT).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
        });
    }
}

/// The routing card: the lighthouse public endpoints, the overlay-routable subnets,
/// and the default gateway — or an honest note when the snapshot carries none.
fn show_routing(ui: &mut egui::Ui, status: &NetStatus) {
    if status.routing_empty() {
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new("No overlay routes or gateways published yet.").size(Style::SMALL),
        );
        return;
    }
    if !status.gateway_endpoints.is_empty() {
        field(
            ui,
            "Lighthouse endpoints",
            &status.gateway_endpoints.join(", "),
            Style::TEXT,
        );
    }
    if !status.routes.is_empty() {
        field(
            ui,
            "Routable subnets",
            &status.routes.join(", "),
            Style::TEXT,
        );
    }
    if let Some(gw) = &status.default_gw {
        field(ui, "Default gateway", gw, Style::TEXT);
    }
}

/// A labelled value row on the spacing grid: a dim label + a toned value.
fn field(ui: &mut egui::Ui, label: &str, value: &str, tone: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(tone, RichText::new(value).size(Style::SMALL));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A faithful mesh-status snapshot: `self` + a `nodes` directory (this node plus
    /// two peers) + the network overview — the exact shape `mesh-status-snapshot.sh`
    /// writes. `leader` names the mesh leader so both the is-leader and not-leader
    /// paths are reachable from one fixture. The two peers exercise BOTH lighthouse
    /// membership paths: `anchor` is an overlay anchor by IP (its `overlay_ip` is in
    /// `lighthouse_ips`) while running as Server tier (LIGHTHOUSE-9), and `role-lh`
    /// is a lighthouse by role only (its IP is NOT in the set).
    fn snapshot(self_host: &str, leader: &str) -> String {
        format!(
            r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "online": 2,
              "total": 3,
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "services":{{"mackesd":true,"nebula":true,"sync":true,"dns":true,
                    "voice":false,"music":false}},
                  "role":"workstation"}},
                {{"hostname":"anchor","overlay_ip":"10.42.0.1","presence":"online",
                  "services":{{}},"role":"server"}},
                {{"hostname":"role-lh","overlay_ip":"10.42.0.9","presence":"offline",
                  "services":{{}},"role":"lighthouse"}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"{leader}","overlay_ip":"10.42.0.7",
                "overlay_cidr":"10.42.0.0/16","routes":["10.42.0.0/16","10.8.0.0/24"],
                "default_gw":"192.168.1.1",
                "gateway_endpoints":["203.0.113.7:4242"],
                "lighthouse_ips":["10.42.0.1"],"cipher":"AES-256-GCM"}}
            }}"#
        )
    }

    /// Drive one headless 960×640 frame of `show_status` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives minus
    /// the GPU. Returns whether it produced any draw primitives.
    fn renders(status: &NetStatus) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_status(ui, status));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    #[test]
    fn unseen_before_the_first_snapshot() {
        let s = NetStatus::default();
        assert!(!s.seen, "the pre-read status is unseen (connecting)");
        // Even the connecting state is a full paint path, never a blank panel.
        assert!(
            renders(&s),
            "the connecting state produced no draw primitives"
        );
    }

    #[test]
    fn garbage_or_fragment_snapshot_stays_unseen() {
        for bad in ["", "not json", "{}", "[]", r#"{"network":{}}"#] {
            let s = NetStatus::project(bad, "this-node");
            assert!(!s.seen, "{bad:?} must not read as a live snapshot");
        }
    }

    #[test]
    fn project_folds_the_overlay_peer_links_and_routing() {
        // The mesh leader is a peer (anchor), so this node is NOT the leader.
        let s = NetStatus::project(&snapshot("this-node", "anchor"), "fallback");
        assert!(s.seen, "a real snapshot reads as seen");

        // Overlay — every field is the fabric's real reality (§7).
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.overlay_if.as_deref(), Some("nebula1"));
        assert_eq!(s.overlay_cidr.as_deref(), Some("10.42.0.0/16"));
        assert_eq!(s.cipher.as_deref(), Some("AES-256-GCM"));
        assert_eq!(s.leader.as_deref(), Some("anchor"));
        assert!(!s.is_leader(), "the leader is a peer, not this node");

        // Mesh links — the full peer directory, with the live online/total count.
        assert_eq!(s.peers_total(), 3, "every named node is a link");
        assert_eq!(s.peers_online(), 2, "two of three links are online");
        let this = s
            .peers
            .iter()
            .find(|p| p.hostname == "this-node")
            .expect("this node is a link");
        assert!(this.is_self, "this node's own link is marked");
        assert!(
            !this.is_lighthouse,
            "an ordinary workstation isn't an anchor"
        );
        assert_eq!(this.overlay_ip.as_deref(), Some("10.42.0.7"));

        // Both lighthouse membership paths resolve (LIGHTHOUSE-9): by overlay-IP
        // membership (anchor, running Server tier) AND by role (role-lh).
        let anchor = s.peers.iter().find(|p| p.hostname == "anchor").unwrap();
        assert!(anchor.is_lighthouse, "anchor is a lighthouse by overlay IP");
        let role_lh = s.peers.iter().find(|p| p.hostname == "role-lh").unwrap();
        assert!(role_lh.is_lighthouse, "role-lh is a lighthouse by role");

        // Network services — the network-scoped subset of this node's own map, in
        // catalog order (Nebula, DNS, Syncthing); non-network daemons are excluded.
        assert_eq!(s.services.len(), NET_SERVICE_CATALOG.len(), "all 3 present");
        assert_eq!(s.services[0], ("Overlay (Nebula)", true));
        assert!(s.services.iter().any(|(l, up)| *l == "Mesh DNS" && *up));
        assert!(s
            .services
            .iter()
            .any(|(l, up)| *l == "Sync (Syncthing)" && *up));

        // Routing & reachability — the genuine overlay routing carried on the wire.
        assert!(!s.routing_empty());
        assert_eq!(s.gateway_endpoints, vec!["203.0.113.7:4242".to_string()]);
        assert_eq!(
            s.routes,
            vec!["10.42.0.0/16".to_string(), "10.8.0.0/24".to_string()]
        );
        assert_eq!(s.default_gw.as_deref(), Some("192.168.1.1"));

        // And the whole live panel tessellates.
        assert!(
            renders(&s),
            "the live Network panel produced no draw primitives"
        );
    }

    #[test]
    fn leader_chip_identifies_this_node_when_it_holds_the_lease() {
        let s = NetStatus::project(&snapshot("this-node", "this-node"), "fallback");
        assert!(s.is_leader(), "this node holds the leader lease");
        assert!(renders(&s));
    }

    #[test]
    fn self_marker_absent_falls_back_to_local_hostname() {
        // A snapshot with a nodes directory but no `self` marker → the plane still
        // resolves this node (for the self link + leader chip) by the locally-
        // resolved hostname.
        let snap = r#"{"generated_ms":1,"online":1,"total":1,
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "role":"workstation","services":{"nebula":true}}],
            "network":{"leader":"this-node","cipher":"ChaCha20-Poly1305"}}"#;
        let s = NetStatus::project(snap, "this-node");
        assert!(s.seen);
        assert_eq!(s.hostname, "this-node");
        assert!(
            s.is_leader(),
            "leader resolves against the fallback hostname"
        );
        assert!(s.peers.iter().any(|p| p.is_self), "the self link is marked");
        assert_eq!(s.cipher.as_deref(), Some("ChaCha20-Poly1305"));
    }

    #[test]
    fn seen_but_network_block_absent_renders_the_honest_partial() {
        // The directory is readable but the snapshot carries no `network` block:
        // the peer links still render, and the overlay/routing fields honestly say
        // so (never a fabricated value, §7).
        let snap = r#"{"self":"this-node","online":1,"total":1,
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "role":"workstation","services":{}}]}"#;
        let s = NetStatus::project(snap, "fallback");
        assert!(s.seen, "the snapshot was parsed");
        assert!(s.overlay_if.is_none() && s.cipher.is_none() && s.leader.is_none());
        assert!(s.routing_empty(), "no routing without a network block");
        assert_eq!(s.peers_total(), 1, "the peer link still renders");
        // Overlay IP still folds from this node's own directory row.
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert!(renders(&s), "the honest-partial panel still fully paints");
    }

    #[test]
    fn network_state_defaults_to_the_snapshot_path_unseen() {
        let st = NetworkState::default();
        assert_eq!(st.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!st.status.seen);
        assert!(st.last_poll.is_none());
    }
}

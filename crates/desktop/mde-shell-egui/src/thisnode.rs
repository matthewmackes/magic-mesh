//! Workbench · This Node — live local-node status (WB-ThisNode).
//!
//! The first Workbench plane, wired off the SAME world-readable mesh-status
//! snapshot the chrome bar folds (`/run/mde/mesh-status.json`, written every ~30s
//! by the root `mesh-status.timer`). The desktop user can't read the root-only
//! replicated peer directory, so this JSON is the desktop tier's read path — the
//! shell leans on no `mackesd` IPC (§6). Every field here is real, live-updating
//! node reality; nothing is a stand-in (§7):
//!
//! * **Identity** — this node's hostname (the snapshot's own `self` marker), its
//!   pinned `role`, its Nebula `overlay_ip`, and the tunnel `cipher`.
//! * **Presence + heartbeat** — the node's directory `presence` tier
//!   (online/idle/offline) and the freshness of its last heartbeat, measured
//!   against the snapshot's own `generated_ms` clock (no desktop-clock skew).
//! * **Version** — the installed `mde-core` version and whether a newer one is
//!   live on the mesh (the snapshot's fleet-wide `latest_version` fold).
//! * **Node services** — this node's own daemon health (mackesd / Nebula /
//!   Syncthing / Bus / DNS / Voice / Music / KDE-Connect / Workbench), the
//!   `services` map each node publishes into its `shell-status.json`.
//! * **Mesh context** — the live peer count (online / total) and the elected mesh
//!   leader.
//!
//! What this surface honestly **cannot** show: live CPU / memory / disk
//! utilisation. Those aren't in the world-readable snapshot — they're node-local
//! telemetry (a `mackesd` / Netdata concern), and §6 keeps the shell off that
//! path. The panel renders an explicit "not published to this surface" note
//! rather than a fabricated gauge (§7).
//!
//! `project` is pure (no IO, no egui, no GPU), so it's unit-tested directly; the
//! only IO is the snapshot read in [`ThisNodeState::poll`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use serde_json::Value;

/// The world-readable mesh-status snapshot — the same source the chrome bar reads
/// (the desktop user can't read the root-only replicated peer directory).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a heartbeat, a service flip, or a role change surfaces within
/// this window. Matches the chrome bar + the Fleet datacenter poll; the read is a
/// cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip
/// use, so a service dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

/// This node's daemon catalog: the `services` map key each node publishes into its
/// `shell-status.json`, paired with the label the plane renders. Fixed order so the
/// health list is stable frame-to-frame; a key absent from the snapshot is simply
/// not listed (never rendered as a false "down").
const SERVICE_CATALOG: [(&str, &str); 9] = [
    ("mackesd", "Mesh daemon"),
    ("nebula", "Overlay (Nebula)"),
    ("sync", "Sync (Syncthing)"),
    ("bus", "Mesh Bus"),
    ("dns", "Mesh DNS"),
    ("voice", "Voice HUD"),
    ("music", "Music"),
    ("kdc", "KDE Connect"),
    ("workbench", "Workbench"),
];

// ──────────────────────────── projected view ────────────────────────────

/// This node's live status, folded from the mesh-status snapshot. Pure data
/// (parsed without egui/IO/GPU), so it's unit-tested directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NodeStatus {
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the connecting state) from a parsed one.
    seen: bool,
    /// `true` when this node's OWN row was found in the snapshot's directory
    /// (`nodes[]`). `false` when the snapshot is readable but this node hasn't
    /// published a heartbeat record yet — the per-node fields then render honest
    /// "not yet in the peer directory", never a fabricated value.
    in_directory: bool,
    /// This node's hostname — the snapshot's `self` marker (local hostname when the
    /// snapshot omits it).
    hostname: String,
    /// Pinned deployment role (`lighthouse` / `server` / `workstation`), when known.
    role: Option<String>,
    /// This node's Nebula overlay IP, when known.
    overlay_ip: Option<String>,
    /// Directory presence tier: `online` / `idle` / `offline`, when known.
    presence: Option<String>,
    /// Wall-clock ms of this node's last heartbeat (`0` when never reported).
    last_seen_ms: u64,
    /// When the snapshot was generated — the reference clock for heartbeat age (so
    /// freshness can't skew against the desktop's own clock).
    generated_ms: u64,
    /// Installed `mde-core` version, when known.
    version: Option<String>,
    /// `true` when a newer version than this node's is live on the mesh.
    update_available: bool,
    /// The newest version seen across the mesh (for the update hint).
    latest_version: Option<String>,
    /// This node's own daemon health, in catalog order (label, up).
    services: Vec<(&'static str, bool)>,
    /// Peers in the directory currently `online`.
    peers_online: u64,
    /// Peers in the directory (every node the snapshot names).
    peers_total: u64,
    /// The elected mesh leader's hostname, when one holds the lease.
    leader: Option<String>,
    /// The Nebula tunnel cipher label, when nebula is up.
    cipher: Option<String>,
}

/// Read a non-empty string field off a JSON object, or `None`.
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse the `services` map into the catalog-ordered (label, up) rows actually
/// present. A missing map (an older writer / a node with no `shell-status.json`)
/// yields an empty list → the view says "not yet reported" rather than a false
/// all-down.
fn parse_services(services: Option<&Value>) -> Vec<(&'static str, bool)> {
    let Some(obj) = services.and_then(Value::as_object) else {
        return Vec::new();
    };
    SERVICE_CATALOG
        .iter()
        .filter_map(|(key, label)| {
            obj.get(*key)
                .and_then(Value::as_bool)
                .map(|up| (*label, up))
        })
        .collect()
}

impl NodeStatus {
    /// Fold the mesh-status snapshot into this node's status. `fallback_host` is the
    /// locally-resolved hostname, used only when the snapshot omits its `self`
    /// marker. A missing / garbage / non-mesh snapshot yields the honest unseen
    /// status (drives the connecting state), never a panic — mirroring the chrome
    /// bar's tolerance.
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
        let own = nodes.and_then(|arr| {
            arr.iter()
                .find(|n| n.get("hostname").and_then(Value::as_str) == Some(hostname.as_str()))
        });

        Self {
            seen: true,
            in_directory: own.is_some(),
            // Prefer this node's own directory-row overlay IP; fall back to the
            // network overview's locally-probed overlay address.
            overlay_ip: own
                .and_then(|n| nonempty(n, "overlay_ip"))
                .or_else(|| network.and_then(|n| nonempty(n, "overlay_ip"))),
            role: own.and_then(|n| nonempty(n, "role")),
            presence: own.and_then(|n| nonempty(n, "presence")),
            last_seen_ms: own
                .and_then(|n| n.get("last_seen_ms").and_then(Value::as_u64))
                .unwrap_or(0),
            version: own.and_then(|n| nonempty(n, "version")),
            update_available: own
                .and_then(|n| n.get("update").and_then(Value::as_bool))
                .unwrap_or(false),
            services: parse_services(own.and_then(|n| n.get("services"))),
            generated_ms: v.get("generated_ms").and_then(Value::as_u64).unwrap_or(0),
            latest_version: nonempty(&v, "latest_version"),
            peers_online: v.get("online").and_then(Value::as_u64).unwrap_or(0),
            peers_total: v.get("total").and_then(Value::as_u64).unwrap_or(0),
            leader: network.and_then(|n| nonempty(n, "leader")),
            cipher: network.and_then(|n| nonempty(n, "cipher")),
            hostname,
        }
    }

    /// `true` when this node holds the mesh leader lease.
    fn is_leader(&self) -> bool {
        self.leader.as_deref() == Some(self.hostname.as_str())
    }

    /// A human "N ago" freshness for this node's last heartbeat, measured against
    /// the snapshot's own `generated_ms` clock. `None` when no heartbeat has been
    /// recorded yet.
    fn heartbeat_label(&self) -> Option<String> {
        if self.last_seen_ms == 0 {
            return None;
        }
        let secs = self.generated_ms.saturating_sub(self.last_seen_ms) / 1000;
        Some(if secs < 5 {
            "just now".to_string()
        } else if secs < 90 {
            format!("{secs}s ago")
        } else if secs < 90 * 60 {
            format!("{}m ago", secs / 60)
        } else {
            format!("{}h ago", secs / 3600)
        })
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

// ──────────────────────────── the ThisNode state ────────────────────────────

/// The This Node plane's live state: the projected status plus the small IO
/// context to refresh it on the shared cadence.
pub(crate) struct ThisNodeState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// This node's locally-resolved hostname — the fallback `self` when the
    /// snapshot omits it (resolved once).
    local_host: String,
    /// The latest projection. Unseen until the first snapshot lands (drives the
    /// connecting state).
    status: NodeStatus,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ThisNodeState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            local_host: local_hostname(),
            status: NodeStatus::default(),
            last_poll: None,
        }
    }
}

impl ThisNodeState {
    /// The poll seam: refresh the projection from the snapshot when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a heartbeat / service flip
    /// surfaces without input. Cheap enough to call every frame — it self-gates. A
    /// missing / unreadable snapshot yields the unseen status, never a panic.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = std::fs::read_to_string(&self.snapshot_path).unwrap_or_default();
            self.status = NodeStatus::project(&snapshot, &self.local_host);
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

/// Render this node's live status: the connecting state before the first snapshot,
/// else the identity / services / mesh cards over an honest telemetry note.
fn show_status(ui: &mut egui::Ui, status: &NodeStatus) {
    if !status.seen {
        ui.add_space(Style::SP_S);
        ui.colored_label(Style::TEXT_DIM, "Reading this node's status…");
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(
                "This node's role, overlay address, and daemon health fold from the \
                 world-readable mesh-status snapshot.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.group(|ui| show_identity(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Node services")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_services(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Mesh")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_mesh(ui, status));
            ui.add_space(Style::SP_S);

            // Honest boundary (§6/§7): node-local hardware telemetry isn't on this
            // world-readable surface — never fake a gauge.
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(
                    "Live CPU, memory, and disk aren't published to this surface — the shell \
                     reads the mesh directory, not node-local telemetry.",
                )
                .size(Style::SMALL),
            );
        });
}

/// The identity card: hostname + role + a leader marker, then overlay IP, cipher,
/// presence + heartbeat freshness, and the installed version + update hint.
fn show_identity(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&status.hostname)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        if let Some(role) = &status.role {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::ACCENT, RichText::new(role).size(Style::SMALL));
        }
        if status.is_leader() {
            ui.add_space(Style::SP_S);
            ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
            ui.colored_label(Style::OK, RichText::new("mesh leader").size(Style::SMALL));
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
    if let Some(cipher) = &status.cipher {
        field(ui, "Tunnel cipher", cipher, Style::TEXT);
    }

    // Presence + heartbeat freshness.
    match &status.presence {
        Some(p) => {
            let tone = presence_tone(p);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Presence")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.colored_label(tone, RichText::new(p).size(Style::SMALL));
                if let Some(age) = status.heartbeat_label() {
                    ui.add_space(Style::SP_S);
                    ui.colored_label(
                        Style::TEXT_DIM,
                        RichText::new(format!("\u{00B7} heartbeat {age}")).size(Style::SMALL),
                    );
                }
            });
        }
        None => field(
            ui,
            "Presence",
            "not yet in the peer directory",
            Style::TEXT_DIM,
        ),
    }

    // Installed version + update hint.
    match &status.version {
        Some(ver) => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Version")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(Style::TEXT, RichText::new(ver).size(Style::SMALL));
                if status.update_available {
                    ui.add_space(Style::SP_S);
                    let hint = status.latest_version.as_deref().map_or_else(
                        || "update available".to_string(),
                        |latest| format!("update available \u{2192} {latest}"),
                    );
                    ui.colored_label(Style::WARN, RichText::new(hint).size(Style::SMALL));
                }
            });
        }
        None => field(ui, "Version", "unknown", Style::TEXT_DIM),
    }
}

/// The node-services card: one health row per catalog daemon present in the
/// snapshot, or an honest "not yet reported" when this node hasn't published a
/// status record.
fn show_services(ui: &mut egui::Ui, status: &NodeStatus) {
    if status.services.is_empty() {
        let msg = if status.in_directory {
            "Service health not yet reported by this node."
        } else {
            "This node hasn't published a status record yet."
        };
        ui.colored_label(Style::TEXT_DIM, RichText::new(msg).size(Style::SMALL));
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

/// The mesh-context card: the live peer count (online / total) and the elected
/// leader.
fn show_mesh(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Peers")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let tone = if status.peers_total == 0 {
            Style::TEXT_DIM
        } else if status.peers_online == status.peers_total {
            Style::OK
        } else {
            Style::WARN
        };
        ui.colored_label(
            tone,
            RichText::new(format!(
                "{}/{} live",
                status.peers_online, status.peers_total
            ))
            .size(Style::SMALL),
        );
    });
    match &status.leader {
        Some(leader) => field(ui, "Leader", leader, Style::TEXT),
        None => field(ui, "Leader", "no leader elected", Style::TEXT_DIM),
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
    /// two peers), the fleet counts, and the network overview — the exact shape
    /// `mesh-status-snapshot.sh` writes. `leader` names the mesh leader so both the
    /// is-leader and not-leader paths are reachable from one fixture.
    fn snapshot(self_host: &str, leader: &str) -> String {
        format!(
            r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "latest_version": "11.2.0",
              "online": 2,
              "total": 3,
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "last_seen_ms":990000,"version":"11.1.0",
                  "services":{{"mackesd":true,"nebula":true,"sync":true,"bus":true,"dns":true,
                    "voice":false,"music":false,"kdc":true,"workbench":true}},
                  "role":"workstation","update":true}},
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online",
                  "last_seen_ms":995000,"version":"11.2.0","services":{{}},
                  "role":"lighthouse","update":false}},
                {{"hostname":"peer-2","overlay_ip":"10.42.0.9","presence":"offline",
                  "last_seen_ms":100,"version":"11.1.0","services":{{}},
                  "role":"server","update":true}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"{leader}","overlay_ip":"10.42.0.7",
                "overlay_cidr":"10.42.0.0/16","routes":[],"default_gw":"",
                "gateway_endpoints":[],"lighthouse_ips":["10.42.0.1"],"cipher":"AES-256-GCM"}}
            }}"#
        )
    }

    /// Drive one headless 960×640 frame of `show_status` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives minus
    /// the GPU. Returns whether it produced any draw primitives.
    fn renders(status: &NodeStatus) -> bool {
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
        let s = NodeStatus::default();
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
            let s = NodeStatus::project(bad, "this-node");
            assert!(!s.seen, "{bad:?} must not read as a live snapshot");
        }
    }

    #[test]
    fn project_folds_this_nodes_own_row_with_real_fields() {
        // The mesh leader is a peer (lh-01), so this node is NOT the leader.
        let s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert!(s.seen && s.in_directory, "this node's own row was found");

        // Identity — every field is the node's real directory reality (§7).
        assert_eq!(s.hostname, "this-node");
        assert_eq!(s.role.as_deref(), Some("workstation"));
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.cipher.as_deref(), Some("AES-256-GCM"));

        // Presence + heartbeat: generated 1_000_000, last_seen 990_000 → 10s ago.
        assert_eq!(s.presence.as_deref(), Some("online"));
        assert_eq!(s.heartbeat_label().as_deref(), Some("10s ago"));

        // Version + the fleet-wide update hint (this node runs 11.1.0 < 11.2.0).
        assert_eq!(s.version.as_deref(), Some("11.1.0"));
        assert!(s.update_available);
        assert_eq!(s.latest_version.as_deref(), Some("11.2.0"));

        // Node services parse in catalog order; the map's real up/down is kept.
        assert_eq!(
            s.services.len(),
            SERVICE_CATALOG.len(),
            "all 9 daemons present"
        );
        assert_eq!(s.services[0], ("Mesh daemon", true));
        assert!(s.services.iter().any(|(l, up)| *l == "Voice HUD" && !*up));

        // Mesh context — the live peer count + the elected leader.
        assert_eq!((s.peers_online, s.peers_total), (2, 3));
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert!(!s.is_leader(), "the leader is a peer, not this node");

        // And the whole live panel tessellates.
        assert!(
            renders(&s),
            "the live ThisNode panel produced no draw primitives"
        );
    }

    #[test]
    fn leader_row_identifies_this_node_when_it_holds_the_lease() {
        let s = NodeStatus::project(&snapshot("this-node", "this-node"), "fallback");
        assert!(s.is_leader(), "this node holds the leader lease");
        assert!(renders(&s));
    }

    #[test]
    fn self_marker_absent_falls_back_to_local_hostname() {
        // A snapshot with a nodes directory but no `self` marker → the plane still
        // identifies this node by the locally-resolved hostname.
        let snap = r#"{"generated_ms":1,"online":1,"total":1,
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "last_seen_ms":1,"role":"workstation","services":{"mackesd":true}}],
            "network":{"leader":"","cipher":""}}"#;
        let s = NodeStatus::project(snap, "this-node");
        assert!(s.seen && s.in_directory);
        assert_eq!(s.hostname, "this-node");
        assert_eq!(s.role.as_deref(), Some("workstation"));
    }

    #[test]
    fn seen_but_not_in_directory_shows_identity_without_fabricating_a_row() {
        // The snapshot is readable, but this node's heartbeat record isn't in the
        // directory yet: identity + mesh context still render off `self`/`network`,
        // and the per-node fields honestly say so (never a fake value, §7).
        let s = NodeStatus::project(&snapshot("ghost-node", "lh-01"), "fallback");
        assert!(s.seen, "the snapshot was parsed");
        assert!(!s.in_directory, "no matching directory row for this node");
        assert_eq!(s.hostname, "ghost-node");
        // Network-sourced identity is still available.
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert_eq!((s.peers_online, s.peers_total), (2, 3));
        // Per-node fields are honestly empty, not fabricated.
        assert!(s.role.is_none());
        assert!(s.presence.is_none());
        assert!(s.services.is_empty());
        assert!(s.heartbeat_label().is_none());
        // The honest-partial panel still fully paints.
        assert!(renders(&s));
    }

    #[test]
    fn heartbeat_label_is_none_without_a_recorded_beat() {
        let mut s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        s.last_seen_ms = 0;
        assert!(
            s.heartbeat_label().is_none(),
            "no heartbeat recorded → no freshness claimed"
        );
    }

    #[test]
    fn thisnode_state_defaults_to_the_snapshot_path_unseen() {
        let st = ThisNodeState::default();
        assert_eq!(st.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!st.status.seen);
        assert!(st.last_poll.is_none());
    }
}

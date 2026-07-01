//! Workbench · Controller — live mesh control-plane status (WB-Controller).
//!
//! The Controller plane, wired off the SAME world-readable mesh-status snapshot
//! the chrome bar + This Node + Network planes fold (`/run/mde/mesh-status.json`,
//! written every ~30s by the root `mesh-status.timer`). The desktop user can't
//! read the root-only replicated peer directory, so this JSON is the desktop
//! tier's read path — the shell leans on no `mackesd` IPC (§6). Every field here
//! is real, live-updating control-plane reality; nothing is a stand-in (§7):
//!
//! * **Elected controller** — the mesh `leader` the snapshot names (the
//!   etcd-backed leader lease, SUBSTRATE-9), resolved against the peer directory
//!   for the controller's overlay IP + deployment tier, with a "this node is the
//!   controller" chip when this node holds the lease. A held lease is the live,
//!   observable consensus outcome the snapshot carries.
//! * **Control-plane services, fleet-wide** — a per-node rollup of the
//!   control-scoped subset of each node's `services` map (the mesh daemon
//!   `mackesd`, Syncthing state replication, the mesh Bus): which nodes run the
//!   control daemon and whether each control service is healthy. This is the
//!   plane's signature view — distinct from This Node (this host only) and
//!   Network (this host's network services only).
//!
//! What this surface honestly **cannot** show: live etcd raft term, quorum size,
//! or per-member health — that's live consensus telemetry, not on the
//! world-readable snapshot, and §6 keeps the shell off that path. The mesh daemon
//! also hosts the scheduler / session-broker / session-roaming / DNS workers
//! in-process, so they aren't separately-published service keys — the `mackesd`
//! row reflects them. The panel renders an explicit "not published to this
//! surface" note rather than a fabricated per-worker gauge (§7), exactly as This
//! Node did for CPU / memory / disk.
//!
//! `project` is pure (no IO, no egui, no GPU), so it's unit-tested directly; the
//! only IO is the snapshot read in [`ControllerState::poll`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use serde_json::Value;

/// The world-readable mesh-status snapshot — the same source the chrome bar +
/// This Node + Network planes read (the desktop user can't read the root-only
/// replicated peer directory).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a leader change, a control-service flip, or a node join/leave
/// surfaces within this window. Matches the chrome bar + the This Node / Network /
/// Fleet poll; the read is a cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip
/// / This Node / Network use, so a control-service dot reads one `Style` size +
/// colour.
const DOT: &str = "\u{25CF}";

/// The control-scoped subset of a node's `services` map: the daemons that make up
/// the mesh's control plane — the mesh daemon (`mackesd`, which also hosts the
/// leader election / scheduler / session-broker / session-roaming / DNS workers
/// in-process), Syncthing state replication, and the mesh Bus — paired with the
/// label the plane renders. Fixed order so the rollup is stable frame-to-frame; a
/// key absent from the snapshot is simply not listed (never a false "down").
const CONTROL_SERVICE_CATALOG: [(&str, &str); 3] = [
    ("mackesd", "Mesh daemon"),
    ("sync", "State sync (Syncthing)"),
    ("bus", "Mesh Bus"),
];

// ──────────────────────────── projected view ────────────────────────────

/// One node in the control-plane rollup: its identity in the mesh, its directory
/// presence, and its control-scoped service health.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ControlNode {
    /// The node's hostname (the directory key).
    hostname: String,
    /// `true` when this row is this node's own directory row.
    is_self: bool,
    /// `true` when this node holds the mesh leader lease (the elected controller).
    is_leader: bool,
    /// Directory presence tier: `online` / `idle` / `offline`, when known.
    presence: Option<String>,
    /// `true` when this node published a (non-empty) `services` map at all — so an
    /// absent map reads as "not yet reported" rather than a false all-down.
    reported: bool,
    /// This node's control-scoped daemon health, in catalog order (label, up).
    services: Vec<(&'static str, bool)>,
}

impl ControlNode {
    /// `true` when this node runs the control daemon (`mackesd` up) — the fleet-wide
    /// "who runs the control plane" signal, read off the parsed control services (the
    /// mesh daemon is the catalog's first entry).
    fn runs_daemon(&self) -> bool {
        self.services
            .iter()
            .any(|(label, up)| *label == CONTROL_SERVICE_CATALOG[0].1 && *up)
    }
}

/// The mesh control plane's live status, folded from the mesh-status snapshot. Pure
/// data (parsed without egui/IO/GPU), so it's unit-tested directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CtrlStatus {
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the connecting state) from a parsed one.
    seen: bool,
    /// This node's hostname — the snapshot's `self` marker (local hostname when the
    /// snapshot omits it). Used to resolve the "this node is the controller" chip.
    hostname: String,
    /// The elected controller's hostname — the mesh `leader` lease holder, when one
    /// holds the lease.
    leader: Option<String>,
    /// The controller's Nebula overlay IP, resolved from its directory row, when
    /// known.
    leader_overlay_ip: Option<String>,
    /// The controller's deployment tier (`lighthouse` / `server` / `workstation`),
    /// resolved from its directory row, when known.
    leader_role: Option<String>,
    /// The peer directory as control-plane rollup rows (every node the snapshot
    /// names).
    nodes: Vec<ControlNode>,
}

/// Read a non-empty string field off a JSON object, or `None`.
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse the control-scoped subset of the `services` map into catalog-ordered
/// (label, up) rows actually present. A missing map yields an empty list → the view
/// says "not yet reported" rather than a false all-down.
fn parse_control_services(services: Option<&Value>) -> Vec<(&'static str, bool)> {
    let Some(obj) = services.and_then(Value::as_object) else {
        return Vec::new();
    };
    CONTROL_SERVICE_CATALOG
        .iter()
        .filter_map(|(key, label)| {
            obj.get(*key)
                .and_then(Value::as_bool)
                .map(|up| (*label, up))
        })
        .collect()
}

impl CtrlStatus {
    /// Fold the mesh-status snapshot into the control plane's status. `fallback_host`
    /// is the locally-resolved hostname, used only when the snapshot omits its `self`
    /// marker (so the "this node is the controller" chip still resolves). A missing /
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
        let leader = network.and_then(|n| nonempty(n, "leader"));

        // Resolve the controller's own directory row for its overlay IP + tier — the
        // elected leader IS one of the directory nodes, so these are real, not
        // fabricated.
        let leader_row = leader.as_deref().and_then(|ldr| {
            nodes.and_then(|arr| {
                arr.iter()
                    .find(|n| n.get("hostname").and_then(Value::as_str) == Some(ldr))
            })
        });

        // The peer directory as control-plane rollup rows.
        let rollup: Vec<ControlNode> = nodes
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| {
                        let host = nonempty(n, "hostname")?;
                        let services_val = n.get("services");
                        let reported = services_val
                            .and_then(Value::as_object)
                            .is_some_and(|o| !o.is_empty());
                        Some(ControlNode {
                            is_self: host == hostname,
                            is_leader: Some(host.as_str()) == leader.as_deref(),
                            presence: nonempty(n, "presence"),
                            services: parse_control_services(services_val),
                            reported,
                            hostname: host,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            seen: true,
            leader_overlay_ip: leader_row.and_then(|n| nonempty(n, "overlay_ip")),
            leader_role: leader_row.and_then(|n| nonempty(n, "role")),
            nodes: rollup,
            leader,
            hostname,
        }
    }

    /// `true` when this node holds the mesh leader lease (this node is the
    /// controller).
    fn is_leader(&self) -> bool {
        self.leader.as_deref() == Some(self.hostname.as_str())
    }

    /// Nodes currently running the control daemon (`mackesd` up).
    fn daemon_nodes(&self) -> usize {
        self.nodes.iter().filter(|n| n.runs_daemon()).count()
    }

    /// Nodes in the directory (every node the snapshot names).
    fn directory_size(&self) -> usize {
        self.nodes.len()
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

// ──────────────────────────── the Controller state ────────────────────────────

/// The Controller plane's live state: the projected status plus the small IO
/// context to refresh it on the shared cadence.
pub(crate) struct ControllerState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// This node's locally-resolved hostname — the fallback `self` when the
    /// snapshot omits it (resolved once).
    local_host: String,
    /// The latest projection. Unseen until the first snapshot lands (drives the
    /// connecting state).
    status: CtrlStatus,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            local_host: local_hostname(),
            status: CtrlStatus::default(),
            last_poll: None,
        }
    }
}

impl ControllerState {
    /// The poll seam: refresh the projection from the snapshot when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a leader flip or a
    /// control-service change surfaces without input. Cheap enough to call every
    /// frame — it self-gates. A missing / unreadable snapshot yields the unseen
    /// status, never a panic.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = std::fs::read_to_string(&self.snapshot_path).unwrap_or_default();
            self.status = CtrlStatus::project(&snapshot, &self.local_host);
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

/// Render the mesh control plane's live status: the connecting state before the
/// first snapshot, else the controller + control-plane-services cards over an
/// honest consensus-telemetry note.
fn show_status(ui: &mut egui::Ui, status: &CtrlStatus) {
    if !status.seen {
        ui.add_space(Style::SP_S);
        ui.colored_label(Style::TEXT_DIM, "Reading the mesh control-plane status…");
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(
                "The elected controller, its leader lease, and the fleet-wide control-service \
                 health fold from the world-readable mesh-status snapshot.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.group(|ui| show_controller(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Control-plane services")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_control_services(ui, status));
            ui.add_space(Style::SP_S);

            // Honest boundary (§6/§7): live consensus telemetry isn't on this
            // world-readable surface — never fake a gauge.
            mde_egui::muted_note(
                ui,
                "Live etcd raft term, quorum size, and per-member health aren't published to \
                     this surface — the shell reads the mesh directory (the leader lease etcd \
                     backs), not live consensus telemetry. The scheduler, session-broker, and \
                     session-roaming run inside the mesh daemon, so the Mesh-daemon row reflects \
                     them rather than reporting a separate service.",
            );
        });
}

/// The controller card: the elected leader + a "this node is the controller" chip,
/// then the controller's overlay IP, deployment tier, and the live leader-lease
/// (consensus) signal — or an honest "no leader elected" when the lease is vacant.
fn show_controller(ui: &mut egui::Ui, status: &CtrlStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Mesh controller")
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        if status.is_leader() {
            ui.add_space(Style::SP_S);
            ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
            ui.colored_label(
                Style::OK,
                RichText::new("this node is the controller").size(Style::SMALL),
            );
        }
    });
    ui.add_space(Style::SP_XS);

    if let Some(leader) = &status.leader {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Controller")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::TEXT, RichText::new(leader).size(Style::SMALL));
            if status.is_leader() {
                ui.add_space(Style::SP_XS);
                mde_egui::muted_note(ui, "\u{00B7} this node");
            }
        });
        mde_egui::field(
            ui,
            "Overlay IP",
            status.leader_overlay_ip.as_deref().unwrap_or("—"),
            if status.leader_overlay_ip.is_some() {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            },
        );
        if let Some(role) = &status.leader_role {
            mde_egui::field(ui, "Tier", role, Style::TEXT);
        }
        // The held leader lease is the live, observable consensus outcome the
        // snapshot carries (the lease auto-expires, so a named leader is a live
        // one). Raw raft term / quorum stay off this surface (see the note below).
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Leader lease")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.colored_label(Style::OK, RichText::new("held").size(Style::SMALL));
        });
    } else {
        mde_egui::field(ui, "Controller", "no leader elected", Style::TEXT_DIM);
        ui.add_space(Style::SP_XS);
        ui.colored_label(
            Style::WARN,
            RichText::new(
                "No node currently holds the leader lease — the control plane is mid-election \
                 or without quorum.",
            )
            .size(Style::SMALL),
        );
    }
}

/// The control-plane-services card: the fleet-wide "control daemon" count, then one
/// rollup row per node — its presence, the this-node / controller chips, and the
/// health of its control-scoped services — or an honest note when the directory is
/// empty.
fn show_control_services(ui: &mut egui::Ui, status: &CtrlStatus) {
    if status.nodes.is_empty() {
        mde_egui::muted_note(ui, "No nodes in the directory yet.");
        return;
    }

    let (daemon, total) = (status.daemon_nodes(), status.directory_size());
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Control daemon")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let tone = if total == 0 {
            Style::TEXT_DIM
        } else if daemon == total {
            Style::OK
        } else {
            Style::WARN
        };
        ui.colored_label(
            tone,
            RichText::new(format!("{daemon}/{total} nodes")).size(Style::SMALL),
        );
    });
    ui.add_space(Style::SP_XS);

    for node in &status.nodes {
        let tone = node
            .presence
            .as_deref()
            .map_or(Style::TEXT_DIM, presence_tone);
        // Identity row: presence dot · hostname · this-node / controller chips ·
        // presence word.
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(&node.hostname)
                    .color(Style::TEXT)
                    .size(Style::SMALL),
            );
            if node.is_self {
                ui.add_space(Style::SP_XS);
                mde_egui::muted_note(ui, "\u{00B7} this node");
            }
            if node.is_leader {
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::ACCENT,
                    RichText::new("controller").size(Style::SMALL),
                );
            }
            if let Some(p) = &node.presence {
                ui.add_space(Style::SP_S);
                ui.colored_label(tone, RichText::new(p).size(Style::SMALL));
            }
        });
        // Control-service row: indented chips for this node's control-scoped
        // services, or an honest note when this node hasn't published a status
        // record / carries no control services.
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_M);
            if node.services.is_empty() {
                let msg = if node.reported {
                    "no control services on this node"
                } else {
                    "control status not yet reported"
                };
                mde_egui::muted_note(ui, msg);
            } else {
                for (label, up) in &node.services {
                    control_chip(ui, label, *up);
                }
            }
        });
        ui.add_space(Style::SP_XS);
    }
}

/// A compact control-service chip: a status dot + the service label, toned by
/// health. Up reads healthy (OK dot + normal label); down reads a warning (dim dot
/// + amber label) so a degraded control service is unmissable in the inline rollup.
fn control_chip(ui: &mut egui::Ui, label: &str, up: bool) {
    let (dot, tone) = if up {
        (Style::OK, Style::TEXT)
    } else {
        (Style::TEXT_DIM, Style::WARN)
    };
    ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
    ui.add_space(Style::SP_XS);
    ui.colored_label(tone, RichText::new(label).size(Style::SMALL));
    ui.add_space(Style::SP_S);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A faithful mesh-status snapshot: `self` + a `nodes` directory (this node plus
    /// two peers, each with a real control-scoped `services` map) + the network
    /// overview naming the leader — the exact shape `mesh-status-snapshot.sh` writes.
    /// `leader` names the elected controller so both the is-controller and
    /// not-controller paths are reachable from one fixture. The peers exercise the
    /// rollup's up/down control-service tones and the daemon count (only `this-node`
    /// and `lh-01` run `mackesd`).
    fn snapshot(self_host: &str, leader: &str) -> String {
        format!(
            r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "online": 2,
              "total": 3,
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "services":{{"mackesd":true,"sync":true,"bus":true,"nebula":true}},
                  "role":"workstation"}},
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online",
                  "services":{{"mackesd":true,"sync":true,"bus":false}},"role":"lighthouse"}},
                {{"hostname":"srv-02","overlay_ip":"10.42.0.9","presence":"offline",
                  "services":{{"mackesd":false,"sync":true,"bus":false}},"role":"server"}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"{leader}","overlay_ip":"10.42.0.7",
                "overlay_cidr":"10.42.0.0/16","cipher":"AES-256-GCM",
                "lighthouse_ips":["10.42.0.1"]}}
            }}"#
        )
    }

    /// Drive one headless 960×640 frame of `show_status` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives minus
    /// the GPU. Returns whether it produced any draw primitives.
    fn renders(status: &CtrlStatus) -> bool {
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
        let s = CtrlStatus::default();
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
            let s = CtrlStatus::project(bad, "this-node");
            assert!(!s.seen, "{bad:?} must not read as a live snapshot");
        }
    }

    #[test]
    fn project_folds_the_controller_and_the_control_service_rollup() {
        // The elected controller is a peer (lh-01), so this node is NOT the leader.
        let s = CtrlStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert!(s.seen, "a real snapshot reads as seen");

        // Elected controller — the leader, resolved to its real overlay IP + tier
        // off the directory (§7), not this node.
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert_eq!(s.leader_overlay_ip.as_deref(), Some("10.42.0.1"));
        assert_eq!(s.leader_role.as_deref(), Some("lighthouse"));
        assert!(!s.is_leader(), "the controller is a peer, not this node");

        // The fleet-wide control-plane rollup — every named node, with the live
        // daemon count (this-node + lh-01 run mackesd; srv-02 does not).
        assert_eq!(s.directory_size(), 3, "every named node is a rollup row");
        assert_eq!(s.daemon_nodes(), 2, "two of three run the control daemon");

        let this = s
            .nodes
            .iter()
            .find(|n| n.hostname == "this-node")
            .expect("this node is in the rollup");
        assert!(this.is_self, "this node's own row is marked");
        assert!(!this.is_leader, "this node isn't the controller");
        assert!(this.runs_daemon(), "this node runs the control daemon");
        // Control-scoped services parse in catalog order, keeping the map's real
        // up/down; non-control daemons (nebula) are excluded.
        assert_eq!(
            this.services.len(),
            CONTROL_SERVICE_CATALOG.len(),
            "all 3 control services present"
        );
        assert_eq!(this.services[0], ("Mesh daemon", true));
        assert!(this.services.iter().any(|(l, up)| *l == "Mesh Bus" && *up));

        let lh = s.nodes.iter().find(|n| n.hostname == "lh-01").unwrap();
        assert!(
            lh.is_leader,
            "lh-01 is flagged as the controller in the rollup"
        );
        assert!(lh.runs_daemon());
        // lh-01 runs the Bus DOWN — the real degraded state is kept, not masked.
        assert!(lh.services.iter().any(|(l, up)| *l == "Mesh Bus" && !*up));

        let srv = s.nodes.iter().find(|n| n.hostname == "srv-02").unwrap();
        assert!(!srv.runs_daemon(), "srv-02 doesn't run the control daemon");
        assert_eq!(srv.presence.as_deref(), Some("offline"));

        // And the whole live panel — controller card + rollup + honest note —
        // tessellates (proves it's real render, not placeholder copy).
        assert!(
            renders(&s),
            "the live Controller panel produced no draw primitives"
        );
    }

    #[test]
    fn controller_chip_identifies_this_node_when_it_holds_the_lease() {
        let s = CtrlStatus::project(&snapshot("this-node", "this-node"), "fallback");
        assert!(s.is_leader(), "this node holds the leader lease");
        // The controller's own overlay IP + tier still resolve off its directory row.
        assert_eq!(s.leader_overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.leader_role.as_deref(), Some("workstation"));
        assert!(renders(&s));
    }

    #[test]
    fn self_marker_absent_falls_back_to_local_hostname() {
        // A snapshot with a nodes directory but no `self` marker → the plane still
        // resolves this node (for the is-controller chip) by the locally-resolved
        // hostname.
        let snap = r#"{"generated_ms":1,"online":1,"total":1,
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "role":"workstation","services":{"mackesd":true,"sync":true}}],
            "network":{"leader":"this-node"}}"#;
        let s = CtrlStatus::project(snap, "this-node");
        assert!(s.seen);
        assert_eq!(s.hostname, "this-node");
        assert!(
            s.is_leader(),
            "the controller resolves against the fallback hostname"
        );
        assert_eq!(s.daemon_nodes(), 1);
    }

    #[test]
    fn seen_but_no_leader_renders_the_honest_partial() {
        // The directory is readable but no node holds the leader lease: the rollup
        // still renders, and the controller card honestly says the lease is vacant
        // (never a fabricated leader, §7).
        let snap = r#"{"self":"this-node","online":1,"total":1,
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "role":"workstation","services":{"mackesd":true,"sync":true,"bus":true}}],
            "network":{"leader":""}}"#;
        let s = CtrlStatus::project(snap, "fallback");
        assert!(s.seen, "the snapshot was parsed");
        assert!(s.leader.is_none(), "no leader lease held");
        assert!(s.leader_overlay_ip.is_none() && s.leader_role.is_none());
        assert!(
            !s.is_leader(),
            "this node isn't the controller without a lease"
        );
        assert_eq!(s.directory_size(), 1, "the rollup row still renders");
        assert_eq!(s.daemon_nodes(), 1);
        assert!(renders(&s), "the honest-partial panel still fully paints");
    }

    #[test]
    fn node_without_a_services_map_reads_as_not_reported() {
        // A directory row that published no `services` map is honestly "not yet
        // reported" — never rendered as a false all-down control node (§7).
        let snap = r#"{"self":"this-node","online":1,"total":2,
            "nodes":[
              {"hostname":"this-node","presence":"online","role":"workstation",
                "services":{"mackesd":true}},
              {"hostname":"ghost","presence":"idle","role":"server"}
            ],
            "network":{"leader":"this-node"}}"#;
        let s = CtrlStatus::project(snap, "fallback");
        let ghost = s.nodes.iter().find(|n| n.hostname == "ghost").unwrap();
        assert!(!ghost.reported, "ghost published no services map");
        assert!(
            !ghost.runs_daemon(),
            "an unreported node isn't a false daemon host"
        );
        assert!(ghost.services.is_empty());
        assert!(renders(&s));
    }

    #[test]
    fn controller_state_defaults_to_the_snapshot_path_unseen() {
        let st = ControllerState::default();
        assert_eq!(st.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!st.status.seen);
        assert!(st.last_poll.is_none());
    }
}

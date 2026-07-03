//! Surface · Mesh Map (OW-10) — the live mesh canvas, the egui reincarnation of
//! MESHMAP.
//!
//! Two small pieces of glue live here; neither reimplements anything (§6):
//!
//! * [`MeshViewState`] folds the SAME world-readable mesh-status snapshot the
//!   Workbench planes read (`/run/mde/mesh-status.json`, written every ~30s by the
//!   root `mesh-status.timer`) into a [`mde_mesh_view::MeshState`] and hands it to
//!   the [`mde_mesh_view::MeshView`] painter each frame. The widget owns all the
//!   drawing; this module owns only the projection + the poll. Every node, role,
//!   health tier, and leader ring is real directory reality (§7) — an empty /
//!   unreadable snapshot yields an empty `MeshState`, which the widget paints as its
//!   honest "waiting for mesh" `EmptyState`, never a fabricated peer.
//!
//! * [`SelfTestWatch`] observes the onboard self-test verdict on the mesh Bus
//!   (`event/onboard/self-test`) and reports the moment a node's self-test goes
//!   **all-green**, so the shell can auto-open this Mesh Map (OW-10's acceptance).
//!   It reuses the shell's existing persist-first Bus read (the same `Persist`
//!   `list_since` cursor drain the KIRON toast lane uses) and decodes the report
//!   through a §6 wire-mirror of its `ok` verdict — the shell never depends on the
//!   `mackesd` daemon crate that assembles the report. The far half (a node
//!   publishing its self-test verdict) is integration-gated exactly like the VDI /
//!   Browser transports; the reachable near half — receiving it and opening the map
//!   — is real here, and the Mesh Map is independently reachable from the dock rail
//!   and the `shell/goto/mesh-map` nav grammar besides.
//!
//! Both `project` and the verdict decode are pure (no IO, no egui, no GPU), so they
//! are unit-tested directly; the only IO is the snapshot read + the Bus drain in the
//! two `poll` seams.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_bus::persist::Persist;
use mde_egui::egui;
use serde_json::Value;

use mde_mesh_view::{Health, MeshLink, MeshNode, MeshState, MeshView, Role};

/// The world-readable mesh-status snapshot — the same source This Node / Network /
/// the chrome bar read (the desktop user can't read the root-only replicated peer
/// directory, so this JSON is the desktop tier's read path, §6).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// The onboard self-test verdict topic — where a node announces its
/// `mackesd onboard self-test` result on the mesh Bus, alongside the existing
/// `event/onboard/apply` + `event/onboard/service-add` onboard events. The report
/// body is the `SelfTestReport` JSON; only its `ok` verdict is read here (§6 wire
/// mirror).
const SELF_TEST_TOPIC: &str = "event/onboard/self-test";

/// Poll cadence — a peer join/leave, a leader change, or a presence flip surfaces
/// within this window. Matches This Node / Network; the snapshot read is a cheap
/// local file scan and the Bus drain an incremental spool read, so it stays tight.
const REFRESH: Duration = Duration::from_secs(5);

// ─────────────────────────── the snapshot → MeshState fold ───────────────────────────

/// Read a non-empty string field off a JSON object, or `None`. (The same tiny
/// per-module helper This Node / Network keep — a pure field read, duplicated rather
/// than threaded through a shared crate, matching the surrounding idiom.)
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// A JSON string-array field as owned `String`s (the Network plane's `lighthouse_ips`
/// read), or an empty vec when absent.
fn string_list(obj: Option<&Value>, key: &str) -> Vec<String> {
    obj.and_then(|o| o.get(key))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Fold the mesh-status snapshot into a [`MeshState`] for the painter. Pure (no IO /
/// egui / GPU), so it's unit-tested directly. A missing / garbage / non-mesh snapshot
/// (or one with no `nodes` directory) yields an EMPTY `MeshState` — the widget then
/// paints its honest "waiting for mesh" `EmptyState` rather than a fabricated peer
/// (§6/§7).
///
/// The mapping is glue over the directory the Network plane already renders:
/// * a node's `role`/`overlay_ip` → [`Role::Lighthouse`] (an overlay anchor: role is
///   `lighthouse`, or its IP is in `lighthouse_ips`) vs [`Role::Workstation`];
/// * its directory `presence` → [`Health`] (online → Ok, idle → Warn, offline → Down,
///   unknown → Warn — surfaced as a concern, never a fabricated "OK");
/// * the elected `leader` → the pulsing leader ring.
///
/// Links draw the real overlay topology — every node tunnels to the lighthouse
/// anchor(s), and the anchors mesh with each other (falling back to a star around the
/// leader, then the first node, on a LAN-only mesh with no lighthouse). Link
/// `activity` is `0.0`: per-link throughput isn't on this world-readable surface (the
/// same honest boundary the Network plane draws), so links render as real topology
/// hairlines without a fabricated pulse.
fn project(snapshot: &str) -> MeshState {
    let Ok(v) = serde_json::from_str::<Value>(snapshot) else {
        return MeshState::default();
    };
    let Some(rows) = v.get("nodes").and_then(Value::as_array) else {
        return MeshState::default();
    };
    let network = v.get("network");
    let leader = network.and_then(|n| nonempty(n, "leader"));
    let lighthouse_ips = string_list(network, "lighthouse_ips");

    let mut nodes: Vec<MeshNode> = Vec::with_capacity(rows.len());
    let mut lighthouse_hosts: Vec<String> = Vec::new();
    for n in rows {
        let Some(hostname) = nonempty(n, "hostname") else {
            continue;
        };
        let overlay_ip = nonempty(n, "overlay_ip");
        let is_lighthouse = nonempty(n, "role").as_deref() == Some("lighthouse")
            || overlay_ip
                .as_deref()
                .is_some_and(|ip| lighthouse_ips.iter().any(|l| l == ip));
        let role = if is_lighthouse {
            Role::Lighthouse
        } else {
            Role::Workstation
        };
        let health = match nonempty(n, "presence").as_deref() {
            Some("online") => Health::Ok,
            Some("offline") => Health::Down,
            // idle, or an unknown/absent presence tier: a surfaced concern, never a
            // fabricated "OK" for a node whose liveness we can't vouch for (§7).
            _ => Health::Warn,
        };
        let mut node = MeshNode::new(hostname.clone(), hostname.clone(), role, health);
        if leader.as_deref() == Some(hostname.as_str()) {
            node = node.leader();
        }
        if is_lighthouse {
            lighthouse_hosts.push(hostname.clone());
        }
        nodes.push(node);
    }

    let links = topology_links(&nodes, &lighthouse_hosts, leader.as_deref());
    MeshState { nodes, links }
}

/// The overlay-topology links: peers tunnel to the anchor(s) and the anchors mesh
/// together. Anchors are the lighthouse nodes when any exist, else the elected
/// leader, else the first node — so a mesh-of-few always reads as connected rather
/// than a scatter of unlinked dots. `activity` is `0.0` (no fabricated throughput).
fn topology_links(
    nodes: &[MeshNode],
    lighthouse_hosts: &[String],
    leader: Option<&str>,
) -> Vec<MeshLink> {
    let anchors: Vec<String> = if !lighthouse_hosts.is_empty() {
        lighthouse_hosts.to_vec()
    } else if let Some(l) = leader.filter(|l| nodes.iter().any(|n| n.id == *l)) {
        vec![l.to_string()]
    } else if let Some(first) = nodes.first() {
        vec![first.id.clone()]
    } else {
        Vec::new()
    };
    let anchor_set: HashSet<&str> = anchors.iter().map(String::as_str).collect();

    let mut links = Vec::new();
    for node in nodes {
        if anchor_set.contains(node.id.as_str()) {
            continue;
        }
        for a in &anchors {
            links.push(MeshLink::new(node.id.clone(), a.clone(), 0.0));
        }
    }
    // The anchors mesh with each other (a two-lighthouse fleet draws its inter-anchor
    // tunnel).
    for i in 0..anchors.len() {
        for j in (i + 1)..anchors.len() {
            links.push(MeshLink::new(anchors[i].clone(), anchors[j].clone(), 0.0));
        }
    }
    links
}

/// The Mesh Map surface's live state: the projected [`MeshState`] plus the small IO
/// context to refresh it on the shared cadence.
pub(crate) struct MeshViewState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// The latest projection — empty until the first snapshot lands (the widget's
    /// honest "waiting for mesh" `EmptyState`).
    state: MeshState,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for MeshViewState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            state: MeshState::default(),
            last_poll: None,
        }
    }
}

impl MeshViewState {
    /// The poll seam: refold the [`MeshState`] from the snapshot when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a peer join / presence flip
    /// surfaces without input (the animated widget self-repaints while the leader ring
    /// breathes, but a still map with no leader would otherwise idle). Cheap enough to
    /// call every frame — it self-gates. A missing / unreadable snapshot yields the
    /// empty state, never a panic.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = std::fs::read_to_string(&self.snapshot_path).unwrap_or_default();
            self.state = project(&snapshot);
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Render the live mesh canvas into `ui` — the `mde-mesh-view` painter fed the
    /// current [`MeshState`]. The widget draws everything (nodes, links, the leader
    /// ring, or the `EmptyState` when the mesh has no nodes).
    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        MeshView::new(&self.state).show(ui);
    }
}

// ─────────────────────────── the self-test → open-map watch ───────────────────────────

/// A §6 wire-mirror of the `SelfTestReport` body: only the `ok` verdict is read here
/// (the shell decodes the report's contract, not the `mackesd` daemon's Rust type —
/// the same discipline `discovery`/`datacenter` mirror the broker/lifecycle wire
/// shapes with). `ok` defaults to `false`, so a malformed / partial body is never a
/// false all-green.
#[derive(serde::Deserialize)]
struct SelfTestVerdict {
    #[serde(default)]
    ok: bool,
}

/// Decode a self-test report body to its all-green verdict. A body that isn't a
/// decodable report, or one whose `ok` is absent/false, is honestly not-green (never
/// a false open, §7).
fn report_is_all_green(body: &str) -> bool {
    serde_json::from_str::<SelfTestVerdict>(body).is_ok_and(|v| v.ok)
}

/// Watches the onboard self-test verdict lane and reports the moment a node's
/// self-test goes all-green, so the shell auto-opens the Mesh Map (OW-10).
///
/// It drains `event/onboard/self-test` on the shared cadence over the shell's
/// existing persist-first Bus read. The FIRST drain only establishes the cursor
/// baseline (any historical verdict is "already seen") so the map isn't force-opened
/// on every launch; a live all-green verdict arriving afterwards raises the one-shot
/// [`take_all_green`](Self::take_all_green) edge.
pub(crate) struct SelfTestWatch {
    /// The client Bus root (the same `mde_bus::client_data_dir()` the toast lane
    /// reads); `None` off a mesh (no Bus) — the watch then simply never fires.
    bus_root: Option<PathBuf>,
    /// Bus ULID cursor for `list_since` — advances on each drain.
    cursor: Option<String>,
    /// When the lane was last drained (drives the cadence).
    last_poll: Option<Instant>,
    /// `false` until the first drain establishes the baseline cursor. While unprimed,
    /// verdicts advance the cursor but never fire — cold-start history isn't a live
    /// edge.
    primed: bool,
    /// A live all-green verdict landed and hasn't been consumed yet.
    pending_open: bool,
}

impl Default for SelfTestWatch {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            cursor: None,
            last_poll: None,
            primed: false,
            pending_open: false,
        }
    }
}

impl SelfTestWatch {
    /// The poll seam: drain any new self-test verdicts on the cadence, then keep the
    /// repaint heartbeat alive so an all-green verdict can open the map even while the
    /// shell is otherwise idle. Cheap — self-gates on the cadence, and a missing Bus
    /// is a silent no-op.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.drain();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Consume the one-shot "a live self-test just went all-green" edge — `true` at
    /// most once per verdict. The shell calls this each frame and opens the Mesh Map
    /// when it fires.
    pub(crate) fn take_all_green(&mut self) -> bool {
        std::mem::take(&mut self.pending_open)
    }

    /// Drain new verdicts after the cursor, decoding each through the wire mirror. The
    /// first drain only primes the baseline; later all-green verdicts raise the edge.
    fn drain(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(msgs) = persist.list_since(SELF_TEST_TOPIC, self.cursor.as_deref()) else {
            return;
        };
        for msg in msgs {
            self.cursor = Some(msg.ulid.clone());
            if let Some(body) = msg.body.as_deref() {
                self.admit(body);
            }
        }
        // Cold-start history is a baseline, not a live edge: the first drain arms the
        // watch without firing.
        self.primed = true;
    }

    /// Apply one verdict body: once primed, an all-green report raises the open edge.
    /// Split from the Bus read so the whole policy is unit-tested without a spool.
    fn admit(&mut self, body: &str) {
        if self.primed && report_is_all_green(body) {
            self.pending_open = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::Surface;
    use crate::toast_bridge::{resolve_action, Navigate};
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_egui::Style;

    /// A faithful mesh-status snapshot — the exact shape `mesh-status-snapshot.sh`
    /// writes: a `nodes` directory (a lighthouse + two workstations, one offline) and
    /// a `network` overview naming the leader + the lighthouse anchor IP.
    fn snapshot() -> String {
        r#"{
          "generated_ms": 1000000,
          "self": "ws-1",
          "online": 2,
          "total": 3,
          "nodes": [
            {"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online","role":"lighthouse"},
            {"hostname":"ws-1","overlay_ip":"10.42.0.7","presence":"online","role":"workstation"},
            {"hostname":"ws-2","overlay_ip":"10.42.0.9","presence":"offline","role":"workstation"}
          ],
          "network": {"leader":"lh-01","lighthouse_ips":["10.42.0.1"],"cipher":"AES-256-GCM"}
        }"#
        .to_string()
    }

    /// A green (`ok:true`) self-test report body — the `SelfTestReport` JSON shape.
    fn green_report() -> String {
        r#"{"node_id":"ws-1","ok":true,"checks":[{"id":"mesh","status":"pass","critical":true,"detail":"3 peers"}]}"#.to_string()
    }

    /// A failing (`ok:false`) self-test report body.
    fn red_report() -> String {
        r#"{"node_id":"ws-1","ok":false,"checks":[{"id":"identity","status":"fail","critical":true,"detail":"absent"}]}"#.to_string()
    }

    /// Drive one headless 480×360 frame that shows `state` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives minus
    /// the GPU. Returns whether it produced any draw primitives.
    fn renders(state: &MeshState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                MeshView::new(state).show(ui);
            });
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    #[test]
    fn project_folds_the_directory_into_real_nodes_links_and_a_leader() {
        let state = project(&snapshot());
        // One MeshNode per directory row.
        assert_eq!(state.nodes.len(), 3);
        let node = |id: &str| {
            state
                .nodes
                .iter()
                .find(|n| n.id == id)
                .expect("node present")
        };

        // Roles: the lighthouse anchor vs the workstations.
        assert_eq!(node("lh-01").role, Role::Lighthouse);
        assert_eq!(node("ws-1").role, Role::Workstation);

        // Health folds directory presence honestly (offline → Down, online → Ok).
        assert_eq!(node("lh-01").health, Health::Ok);
        assert_eq!(node("ws-2").health, Health::Down);

        // The elected leader gets the pulsing ring; the peers don't.
        assert!(node("lh-01").is_leader, "the elected leader pulses");
        assert!(!node("ws-1").is_leader);

        // Overlay topology: both workstations tunnel to the single lighthouse anchor.
        assert_eq!(
            state.links.len(),
            2,
            "each non-anchor links to the lighthouse"
        );
        assert!(state
            .links
            .iter()
            .all(|l| l.b == "lh-01" && l.activity == 0.0));

        // And the whole live map tessellates.
        assert!(
            renders(&state),
            "the live mesh map produced no draw primitives"
        );
    }

    #[test]
    fn an_empty_or_garbage_snapshot_yields_the_honest_empty_state() {
        // A missing / non-mesh snapshot has no nodes → the widget's "waiting for mesh"
        // EmptyState, never a fabricated peer (§7). Each still fully paints.
        for bad in ["", "not json", "{}", r#"{"network":{}}"#] {
            let state = project(bad);
            assert!(state.nodes.is_empty(), "{bad:?} must yield no nodes");
            assert!(renders(&state), "{bad:?} EmptyState produced no primitives");
        }
    }

    #[test]
    fn no_lighthouse_falls_back_to_a_star_around_the_leader() {
        // A LAN-only mesh with no lighthouse still reads as connected: the elected
        // leader anchors the star.
        let snap = r#"{"nodes":[
            {"hostname":"a","presence":"online","role":"workstation"},
            {"hostname":"b","presence":"online","role":"workstation"},
            {"hostname":"c","presence":"idle","role":"workstation"}
          ],"network":{"leader":"a"}}"#;
        let state = project(snap);
        assert_eq!(state.nodes.len(), 3);
        assert!(state.nodes.iter().all(|n| n.role == Role::Workstation));
        // b and c link to the leader a (idle presence → Warn, still connected).
        assert_eq!(state.links.len(), 2);
        assert!(state.links.iter().all(|l| l.b == "a"));
        assert_eq!(
            state.nodes.iter().find(|n| n.id == "c").unwrap().health,
            Health::Warn
        );
    }

    #[test]
    fn a_report_body_decodes_to_its_verdict() {
        // The §6 wire mirror reads only `ok`; a malformed / partial body is never a
        // false all-green.
        assert!(report_is_all_green(&green_report()));
        assert!(!report_is_all_green(&red_report()));
        for bad in ["", "not json", "{}", r#"{"ok":"yes"}"#] {
            assert!(!report_is_all_green(bad), "{bad:?} must not read as green");
        }
    }

    /// A detached watch (no Bus) with the baseline already primed — the test seam for
    /// feeding verdict bodies directly, mirroring the live `drain` → `admit` path.
    fn primed_watch() -> SelfTestWatch {
        SelfTestWatch {
            primed: true,
            ..SelfTestWatch::default()
        }
    }

    #[test]
    fn an_all_green_self_test_raises_a_one_shot_open_edge() {
        let mut watch = primed_watch();
        assert!(!watch.take_all_green(), "no verdict yet → no edge");

        // A live all-green verdict raises the edge exactly once.
        watch.admit(&green_report());
        assert!(watch.take_all_green(), "all-green opens the map");
        assert!(!watch.take_all_green(), "the edge is one-shot");

        // A failing verdict never opens the map.
        watch.admit(&red_report());
        assert!(
            !watch.take_all_green(),
            "a critical-fail verdict must not open"
        );
    }

    #[test]
    fn an_unprimed_watch_treats_history_as_a_baseline_not_a_live_edge() {
        // Before the first drain primes the cursor, even an all-green body is baseline
        // (a stale verdict from a past session must not force-open the map on launch).
        let mut watch = SelfTestWatch::default();
        assert!(!watch.primed);
        watch.admit(&green_report());
        assert!(
            !watch.take_all_green(),
            "unprimed history is not a live edge"
        );
    }

    #[test]
    fn the_all_green_edge_opens_the_mesh_view_surface() {
        // The shell drives the auto-open through the SAME `shell/goto/<surface>` nav
        // grammar the chrome unread indicator + the KIRON chyron use — the verb the
        // all-green edge fires resolves to the Mesh Map surface, so opening it needs no
        // second navigation path.
        assert!(matches!(
            resolve_action("shell/goto/mesh-map"),
            Some(Navigate::Surface(Surface::MeshView))
        ));
    }
}

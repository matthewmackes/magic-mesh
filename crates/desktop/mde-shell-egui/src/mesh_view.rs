//! Surface · Mesh Map (OW-10) — the live mesh canvas, the egui reincarnation of
//! MESHMAP.
//!
//! A few small pieces of glue live here; none reimplements anything (§6):
//!
//! * The surface embeds the shared **top menu bar** (MENUBAR-ALL): [`MeshMenuBar`]
//!   renders `mde-mesh-view`'s [`MeshViewOptions`] (Reduce Motion + the role /
//!   health filters) with a live health-count status cluster, and hands back the
//!   one host-owned command — a manual Refresh, which forces the next re-poll. The
//!   filtered projection is what the widget paints and the badge overlay pins to.
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
//! * [`CoEditWatch`] (EDITOR-COLLAB-3) watches the editor share-session lanes on
//!   the mesh Bus (`collab/session/<id>` — the SAME frames `mde-editor-egui`'s
//!   `CollabSession` publishes, decoded through the editor crate's own exported
//!   wire types, §6 — no drifting second decoder) and folds them into per-session
//!   participant rosters stamped with each frame's Bus write time. A session with
//!   **two or more participants** and traffic inside the activity window is an
//!   *active co-editing session*; every participant whose identity matches a map
//!   node's hostname gets an accent-tinted Editor badge pinned to its disc — the
//!   QBRAND-8 per-node adornment idiom, on the NW edge where the role badge holds
//!   NE. Honest boundaries (§7): a solo session (a host sharing to nobody) is not
//!   "co-editing" and badges nothing, a peer identity that isn't a hostname on
//!   this map badges nothing, and a silent session ages out of the badge rather
//!   than pinning a stale "editing" marker forever.
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

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

// arch-11: prod now opens via the BusReader seam; only the tests still name
// `Persist` (through `use super::*`), so the import is test-only.
#[cfg(test)]
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;
use mde_editor_egui::collab_session::{CollabMessage, FrameKind, COLLAB_TOPIC_PREFIX};
use mde_egui::egui::{self, Color32, Pos2, Rect, TextureHandle};
use mde_egui::Style;
use mde_theme::brand::icons::{icon_image, IconId};
use serde_json::Value;

use mde_mesh_view::{
    layout, Health, MeshLink, MeshMenuBar, MeshNode, MeshOutcome, MeshState, MeshView,
    MeshViewOptions, Role,
};

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
///   `lighthouse`, or its IP is in `lighthouse_ips`), [`Role::Server`] (a headless
///   `server`-tier box) or [`Role::Workstation`] — each drawn with its brand role
///   badge in the map overlay;
/// * its directory `presence` → [`Health`] (online → Ok, idle → Warn, offline → Down,
///   unknown → Warn — surfaced as a concern, never a fabricated "OK");
/// * its per-node `version` → the node's version sub-label, and the snapshot's
///   `update` flag → the stale marker (a node the fleet has moved past); an absent
///   version stays `None`, drawn as an honest `—`, never a fabricated build (§7);
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
        let role_field = nonempty(n, "role");
        // A node is a lighthouse by Nebula membership (its overlay IP anchors the
        // static_host_map) OR by its declared role — the storage anchors run the
        // Server tier but are lighthouses, so the IP check catches them (§7).
        let is_lighthouse = role_field.as_deref() == Some("lighthouse")
            || overlay_ip
                .as_deref()
                .is_some_and(|ip| lighthouse_ips.iter().any(|l| l == ip));
        let role = if is_lighthouse {
            Role::Lighthouse
        } else if role_field.as_deref() == Some("server") {
            Role::Server
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
        // The node's running build + whether the fleet has moved past it: the
        // snapshot's per-node `version` (from each node's shell-status) and the
        // `update` flag it derives against the newest version on the mesh. Absent
        // version ⇒ left `None`, drawn as an honest `—` (never a fabricated build).
        if let Some(version) = nonempty(n, "version") {
            node = node.version(version);
        }
        if n.get("update").and_then(Value::as_bool).unwrap_or(false) {
            node = node.stale();
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
    /// The health-tinted brand role badges overlaid on the map, rasterized once
    /// per (glyph, tint) and cached.
    badges: BadgeCache,
    /// The shared top menu bar (MENUBAR-ALL) — its live legend-window flag.
    menubar: MeshMenuBar,
    /// The bar-driven view controls (Reduce Motion + the role/health filters).
    options: MeshViewOptions,
    /// The editor share-session watch — which nodes are in an active co-editing
    /// session right now (EDITOR-COLLAB-3), for the co-edit badge overlay.
    coedit: CoEditWatch,
}

impl Default for MeshViewState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            state: MeshState::default(),
            last_poll: None,
            badges: BadgeCache::default(),
            menubar: MeshMenuBar::new(),
            options: MeshViewOptions::default(),
            coedit: CoEditWatch::default(),
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
            // Same cadence: drain the editor share-session lanes so the co-edit
            // badges track session join/leave/idle within the window.
            self.coedit.drain(now_unix_ms());
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Render the live mesh canvas into `ui` — the `mde-mesh-view` painter fed the
    /// current [`MeshState`] — then overlay each node's brand role badge. The widget
    /// draws the topology (nodes, links, the leader ring, the version sub-labels, or
    /// the `EmptyState` when the mesh has no nodes); this pass lays a health-tinted
    /// [`brand::icons`](mde_theme::brand::icons) role badge over each node so its
    /// Workstation / Server / Lighthouse role reads at a glance (QBRAND-8).
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        // MENUBAR-ALL: the shared top bar drives the view options (Reduce Motion,
        // the role / health filters) and a manual Refresh — the one out-of-band
        // command the surface owns, which forces the next snapshot re-poll.
        if self.menubar.ui(ui, &self.state, &mut self.options) == Some(MeshOutcome::Refresh) {
            self.last_poll = None;
        }
        // Paint (and badge) exactly the filtered projection, so the canvas, its
        // layout, and the badge overlay all stay consistent under a filter.
        let view = self.options.filter(&self.state);
        let response = MeshView::new(&view)
            .reduce_motion(self.options.reduce_motion)
            .show(ui);
        self.overlay_role_badges(ui, response.rect, &view);
        self.overlay_coedit_badges(ui, response.rect, &view);
    }

    /// Overlay the health-tinted brand role badge on every node of the painted
    /// (post-filter) `state`, pinned to the same disc the widget painted
    /// (re-resolving the shared `layout::place` over the widget's own
    /// [`MeshView::DEFAULT_MARGIN`] against the SAME state the widget drew, so the
    /// badge lands exactly on its node even under an active filter). No nodes ⇒
    /// nothing to badge (the widget already painted its honest `EmptyState`). The
    /// badge sits as a pip at the disc's NE edge, sized to the node's role radius.
    /// Reuses the QBRAND-2 icon raster + texture wrap (§6).
    fn overlay_role_badges(&mut self, ui: &egui::Ui, area: Rect, state: &MeshState) {
        if state.nodes.is_empty() {
            return;
        }
        let centres = layout::place(state, area, MeshView::DEFAULT_MARGIN);
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(area);
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        for (node, centre) in state.nodes.iter().zip(&centres) {
            let icon = role_icon(node.role);
            let tint = health_tint(node.health);
            let Some(texture) = self.badges.texture(&ctx, icon, tint) else {
                continue; // unreachable zero-size / asset-parse path — skip, never panic
            };
            let r = node.role.radius();
            let diameter = (r * BADGE_SCALE).max(BADGE_MIN_PX);
            let pip = *centre + egui::vec2(r, -r); // NE edge of the node disc
            let rect = Rect::from_center_size(pip, egui::Vec2::splat(diameter));
            painter.image(texture.id(), rect, uv, Color32::WHITE);
        }
    }

    /// Overlay the **co-editing badge** (EDITOR-COLLAB-3) on every painted node
    /// participating in an active share-session: the accent-tinted Editor glyph
    /// pinned to the disc's NW edge (the role badge holds NE), placed against
    /// the SAME filtered state + `layout::place` geometry the widget drew —
    /// the QBRAND-8 per-node adornment idiom, through the same [`BadgeCache`]
    /// raster + texture path (§6). A peer identity that isn't a hostname on
    /// this map simply badges nothing (§7 — never a fabricated node).
    fn overlay_coedit_badges(&mut self, ui: &egui::Ui, area: Rect, state: &MeshState) {
        if state.nodes.is_empty() {
            return;
        }
        let active = self.coedit.active_peers(now_unix_ms());
        if active.is_empty() {
            return;
        }
        let centres = layout::place(state, area, MeshView::DEFAULT_MARGIN);
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(area);
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        for (node, centre) in state.nodes.iter().zip(&centres) {
            if !active.contains(node.id.as_str()) {
                continue;
            }
            let Some(texture) = self.badges.texture(&ctx, IconId::Editor, COEDIT_TINT) else {
                continue; // unreachable zero-size / asset-parse path — skip, never panic
            };
            let r = node.role.radius();
            let diameter = (r * BADGE_SCALE).max(BADGE_MIN_PX);
            let pip = *centre + egui::vec2(-r, -r); // NW edge — NE holds the role badge
            let rect = Rect::from_center_size(pip, egui::Vec2::splat(diameter));
            painter.image(texture.id(), rect, uv, Color32::WHITE);
        }
    }
}

/// Pixel size the role badges are rasterized at — crisp, then displayed scaled to
/// each node's disc.
const BADGE_TEX_PX: u32 = 32;
/// Badge display diameter as a multiple of the node's disc radius.
const BADGE_SCALE: f32 = 1.5;
/// Floor on the badge display diameter so a small workstation pip stays legible.
const BADGE_MIN_PX: f32 = 13.0;

/// The brand role badge glyph for a mesh role (QBRAND-2 `brand::icons`).
const fn role_icon(role: Role) -> IconId {
    match role {
        Role::Lighthouse => IconId::Lighthouse,
        Role::Server => IconId::Server,
        Role::Workstation => IconId::Workstation,
    }
}

/// The health → badge tint, from the shared status tokens (no raw hex, §4):
/// Ok → [`Style::OK`], Warn → [`Style::WARN`], Down → [`Style::DANGER`].
const fn health_tint(health: Health) -> [u8; 4] {
    let color = match health {
        Health::Ok => Style::OK,
        Health::Warn => Style::WARN,
        Health::Down => Style::DANGER,
    };
    [color.r(), color.g(), color.b(), color.a()]
}

/// The co-editing badge tint — the shared accent (an *activity* accent, kept
/// distinct from the health tints the role badges carry), from the shared
/// palette (no raw hex, §4).
const COEDIT_TINT: [u8; 4] = [
    Style::ACCENT.r(),
    Style::ACCENT.g(),
    Style::ACCENT.b(),
    Style::ACCENT.a(),
];

/// Rasterizes the brand role badges once per (glyph, health-tint) and caches the
/// egui textures for the map overlay — 3 roles × 3 health tints at most, loaded
/// lazily on first use. Reuses the QBRAND-2 `brand::icons` raster + the one-line
/// texture wrap (§6): no second icon source, no redraw.
#[derive(Default)]
struct BadgeCache {
    textures: HashMap<(IconId, [u8; 4]), TextureHandle>,
}

impl BadgeCache {
    /// The cached texture for a role glyph at a health tint, rasterizing + loading
    /// it on first use. `None` only on the unreachable zero-size / asset-parse error
    /// paths (the badge is then simply skipped, never a panic).
    fn texture(
        &mut self,
        ctx: &egui::Context,
        icon: IconId,
        tint: [u8; 4],
    ) -> Option<&TextureHandle> {
        match self.textures.entry((icon, tint)) {
            Entry::Occupied(slot) => Some(slot.into_mut()),
            Entry::Vacant(slot) => {
                let img = icon_image(icon, BADGE_TEX_PX, tint).ok()?;
                let color = egui::ColorImage::from_rgba_unmultiplied(img.size_usize(), &img.rgba);
                let [r, g, b, a] = tint;
                let name = format!("qbrand8-role-{}-{r:02x}{g:02x}{b:02x}{a:02x}", icon.name());
                let handle = ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
                Some(slot.insert(handle))
            }
        }
    }
}

// ─────────────────────────── the co-editing presence watch ───────────────────────────

/// How recently an active co-editing session must have Bus traffic for its
/// participants to badge on the map. A live session emits frames on every
/// keystroke / caret move (`Update` / `Presence`), so a genuinely active pair
/// re-arms this continuously; a session everyone walked away from ages out
/// instead of pinning a stale "editing" marker (§7).
const COEDIT_ACTIVE_MS: i64 = 5 * 60 * 1000;

/// Roster retention: a session silent this long is dropped from memory
/// entirely (its per-peer cursors have already advanced past its frames, so a
/// dropped roster only rebuilds from *new* traffic). Deliberately wider than
/// the activity window so a briefly-idle session keeps its roster and lights
/// back up from a single fresh frame.
const COEDIT_RETAIN_MS: i64 = 6 * COEDIT_ACTIVE_MS;

/// Wall-clock Unix milliseconds — the same scale `StoredMessage::ts_unix_ms`
/// (the Bus write stamp) carries, so frame freshness compares directly.
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Decode one share-session frame body to `(participant, is_leave)`, or `None`
/// for an undecodable body. Every decodable frame — `Hello`, `Sync`, `Update`,
/// `Presence`, `Grant` — is participation *activity* from its sender; a `Leave`
/// prunes the sender from the session's roster. Decoded through the editor
/// crate's own exported [`CollabMessage`] wire type (§6 — the one frame shape,
/// no drifting shell-side mirror).
fn coedit_frame(body: &str) -> Option<(String, bool)> {
    let msg = serde_json::from_str::<CollabMessage>(body).ok()?;
    if msg.from.is_empty() {
        return None;
    }
    let leaving = matches!(msg.kind, FrameKind::Leave);
    Some((msg.from, leaving))
}

/// Watches the editor share-session lanes (`collab/session/<id>`) on the mesh
/// Bus and answers "which nodes are co-editing right now?" for the map's badge
/// overlay (EDITOR-COLLAB-3).
///
/// Per session topic it keeps a roster of participants stamped with their last
/// frame's Bus write time (`ts_unix_ms` — honest wall-clock freshness even for
/// history drained at shell launch, no cold-start false positives and no missed
/// already-live sessions). A session is **active** when its roster holds two or
/// more participants and any frame landed inside [`COEDIT_ACTIVE_MS`]; every
/// participant of an active session badges. Peer identities are the stable mesh
/// identities the sessions publish (hostnames — the same string
/// `collab_session::client_id_for` hashes), matched against map node ids.
pub(crate) struct CoEditWatch {
    /// The client Bus root (the same `mde_bus::client_data_dir()` the toast
    /// lane reads); `None` off a mesh (no Bus) — the watch then never badges.
    bus_root: Option<PathBuf>,
    /// Per-topic Bus ULID cursors — each drain reads only new frames.
    cursors: HashMap<String, Option<String>>,
    /// Per-session participant rosters: topic → (peer → last frame ts, Unix ms).
    sessions: HashMap<String, HashMap<String, i64>>,
}

impl Default for CoEditWatch {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            cursors: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}

impl CoEditWatch {
    /// Drain every share-session lane past its cursor, folding new frames into
    /// the rosters, then age out long-silent sessions. Cheap: an incremental
    /// indexed spool read per topic; a missing Bus is a silent no-op (never a
    /// panic — the honest off-mesh state).
    fn drain(&mut self, now_ms: i64) {
        // arch-11: open through the shared BusReader seam.
        if let Some(persist) = BusReader::new(self.bus_root.clone()).open() {
            let topics = persist.list_topics().unwrap_or_default();
            for topic in topics
                .into_iter()
                .filter(|t| t.starts_with(COLLAB_TOPIC_PREFIX))
            {
                let cursor = self.cursors.entry(topic.clone()).or_default();
                let Ok(msgs) = persist.list_since(&topic, cursor.as_deref()) else {
                    continue;
                };
                let mut fresh = Vec::with_capacity(msgs.len());
                for msg in msgs {
                    *cursor = Some(msg.ulid);
                    if let Some(body) = msg.body {
                        fresh.push((body, msg.ts_unix_ms));
                    }
                }
                for (body, ts_ms) in fresh {
                    self.admit(&topic, &body, ts_ms);
                }
            }
        }
        self.prune(now_ms);
    }

    /// Fold one frame body into its session's roster: activity stamps the
    /// sender's last-seen time, a `Leave` prunes it (an emptied session is
    /// dropped). Split from the Bus read so the whole policy is unit-tested
    /// without a spool (the same drain/admit split [`SelfTestWatch`] uses).
    fn admit(&mut self, topic: &str, body: &str, ts_ms: i64) {
        let Some((peer, leaving)) = coedit_frame(body) else {
            return;
        };
        if leaving {
            if let Some(roster) = self.sessions.get_mut(topic) {
                roster.remove(&peer);
                if roster.is_empty() {
                    self.sessions.remove(topic);
                }
            }
        } else {
            self.sessions
                .entry(topic.to_string())
                .or_default()
                .entry(peer)
                .and_modify(|last| *last = ts_ms.max(*last))
                .or_insert(ts_ms);
        }
    }

    /// Drop sessions whose newest frame is older than the retention window —
    /// the memory bound (rosters never grow past the live session set).
    fn prune(&mut self, now_ms: i64) {
        self.sessions
            .retain(|_, roster| roster.values().any(|&ts| now_ms - ts <= COEDIT_RETAIN_MS));
    }

    /// The peers currently in an **active co-editing session**: a roster of two
    /// or more with any frame inside the activity window badges ALL its
    /// participants (both sides of a live pair light up, even when only one of
    /// them typed last). A solo session is honestly not *co*-editing (§7).
    fn active_peers(&self, now_ms: i64) -> HashSet<String> {
        let mut active = HashSet::new();
        for roster in self.sessions.values() {
            if roster.len() >= 2 && roster.values().any(|&ts| now_ms - ts <= COEDIT_ACTIVE_MS) {
                active.extend(roster.keys().cloned());
            }
        }
        active
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
        // arch-11: open through the shared BusReader seam.
        let Some(persist) = BusReader::new(self.bus_root.clone()).open() else {
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
          "latest_version": "12.0.0",
          "nodes": [
            {"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online","role":"lighthouse","version":"12.0.0","update":false},
            {"hostname":"ws-1","overlay_ip":"10.42.0.7","presence":"online","role":"workstation","version":"12.0.0","update":false},
            {"hostname":"ws-2","overlay_ip":"10.42.0.9","presence":"offline","role":"workstation","version":"11.4.1","update":true}
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

        // Per-node versions surface from the snapshot; the node the fleet has moved
        // past (`update:true`) is flagged stale so it's distinguishable (QBRAND-8).
        assert_eq!(node("lh-01").version.as_deref(), Some("12.0.0"));
        assert_eq!(node("ws-2").version.as_deref(), Some("11.4.1"));
        assert!(node("ws-2").stale, "an older-build node is flagged stale");
        assert!(!node("ws-1").stale, "a current-build node is not stale");

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
    fn a_server_node_folds_to_the_server_role_and_absent_version_stays_none() {
        // A `server`-tier node that isn't a lighthouse anchor folds to Role::Server;
        // a node with no version stays None (drawn as an honest "—", not fabricated).
        let snap = r#"{"nodes":[
            {"hostname":"srv","overlay_ip":"10.42.0.20","presence":"online","role":"server","version":"12.0.0"},
            {"hostname":"ws","overlay_ip":"10.42.0.21","presence":"idle","role":"workstation"}
          ],"network":{"leader":"srv"}}"#;
        let state = project(snap);
        let srv = state
            .nodes
            .iter()
            .find(|n| n.id == "srv")
            .expect("server node");
        assert_eq!(srv.role, Role::Server);
        assert_eq!(srv.version.as_deref(), Some("12.0.0"));
        let ws = state
            .nodes
            .iter()
            .find(|n| n.id == "ws")
            .expect("workstation");
        assert_eq!(
            ws.version, None,
            "absent version stays None (honest placeholder)"
        );
        assert!(!ws.stale);
    }

    #[test]
    fn the_map_overlays_health_tinted_role_badges() {
        // Driving a real headless frame through `show` exercises the overlay pass
        // end-to-end: it rasterizes each node's brand role badge at its health tint,
        // loads the texture, and paints it — the whole map tessellates.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut mv = MeshViewState {
            state: project(&snapshot()),
            ..Default::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                mv.show(ui);
            });
        });
        assert!(
            !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
            "the map + badge overlay produced no draw primitives"
        );
        // lh-01(Lighthouse,Ok), ws-1(Workstation,Ok), ws-2(Workstation,Down) →
        // three distinct (glyph, tint) badges rasterized + cached.
        assert_eq!(mv.badges.textures.len(), 3, "one badge per (role, health)");
    }

    #[test]
    fn the_empty_map_overlays_no_badges() {
        // No nodes ⇒ the widget paints its honest EmptyState and the overlay adds
        // nothing (no fabricated node, no badge).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut mv = MeshViewState::default(); // default state has no nodes
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                mv.show(ui);
            });
        });
        assert!(mv.badges.textures.is_empty(), "no nodes ⇒ no badges");
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

    // ── the co-editing presence watch (EDITOR-COLLAB-3) ──

    use mde_editor_egui::collab_session::Presence as CollabPresence;

    /// Encode one share-session frame exactly as the editor's `CollabSession`
    /// publishes it — the same [`CollabMessage`] type both ends use (§6).
    fn coedit_body(peer: &str, kind: FrameKind) -> String {
        serde_json::to_string(&CollabMessage {
            session: "docs-1".into(),
            from: peer.into(),
            kind,
        })
        .expect("encode")
    }

    /// A presence frame from `peer` (no cursor/viewport — join-time shape).
    fn presence_frame(peer: &str) -> String {
        coedit_body(
            peer,
            FrameKind::Presence {
                presence: CollabPresence {
                    peer: peer.into(),
                    name: peer.into(),
                    cursor: None,
                    viewport: None,
                },
            },
        )
    }

    /// An incremental CRDT update frame from `peer` (a keystroke on the wire).
    fn update_frame(peer: &str) -> String {
        coedit_body(
            peer,
            FrameKind::Update {
                update: vec![1, 2, 3],
            },
        )
    }

    /// A leave frame from `peer`.
    fn leave_frame(peer: &str) -> String {
        coedit_body(peer, FrameKind::Leave)
    }

    /// A watch with no Bus — the seam for feeding frames straight into `admit`,
    /// mirroring the live `drain` → `admit` path (the `SelfTestWatch` idiom).
    fn detached_watch() -> CoEditWatch {
        CoEditWatch {
            bus_root: None,
            cursors: HashMap::new(),
            sessions: HashMap::new(),
        }
    }

    #[test]
    fn a_session_frame_decodes_to_participation_or_leave() {
        // Any decodable frame is participation activity from its sender; a
        // Leave prunes; garbage decodes to nothing (never a fabricated peer).
        assert_eq!(
            coedit_frame(&presence_frame("ws-1")),
            Some(("ws-1".into(), false))
        );
        assert_eq!(
            coedit_frame(&update_frame("eagle")),
            Some(("eagle".into(), false))
        );
        assert_eq!(
            coedit_frame(&leave_frame("ws-1")),
            Some(("ws-1".into(), true))
        );
        for bad in ["", "not json", "{}"] {
            assert_eq!(coedit_frame(bad), None, "{bad:?} must not decode");
        }
        assert_eq!(
            coedit_frame(&coedit_body("", FrameKind::Leave)),
            None,
            "an anonymous frame is dropped"
        );
    }

    #[test]
    fn a_pair_with_fresh_traffic_is_active_and_a_solo_or_silent_one_is_not() {
        let now = 1_750_000_000_000_i64;
        let mut watch = detached_watch();

        // A solo session (a host sharing to nobody) is not CO-editing.
        watch.admit("collab/session/a", &presence_frame("ws-1"), now - 1_000);
        assert!(watch.active_peers(now).is_empty(), "solo ≠ co-editing");

        // A second participant with fresh traffic lights BOTH sides up.
        watch.admit("collab/session/a", &update_frame("lh-01"), now);
        let active = watch.active_peers(now);
        assert!(active.contains("ws-1"), "the quiet side badges too");
        assert!(active.contains("lh-01"));

        // No frame inside the activity window → the badge honestly ages out.
        assert!(
            watch.active_peers(now + COEDIT_ACTIVE_MS + 1).is_empty(),
            "a silent session must not pin a stale badge"
        );
    }

    #[test]
    fn a_leave_dissolves_the_pair_and_an_emptied_session_is_dropped() {
        let now = 1_750_000_000_000_i64;
        let mut watch = detached_watch();
        watch.admit("collab/session/b", &presence_frame("ws-1"), now);
        watch.admit("collab/session/b", &presence_frame("ws-2"), now);
        assert_eq!(watch.active_peers(now).len(), 2);

        watch.admit("collab/session/b", &leave_frame("ws-2"), now);
        assert!(
            watch.active_peers(now).is_empty(),
            "one peer left → the session is no longer co-editing"
        );
        watch.admit("collab/session/b", &leave_frame("ws-1"), now);
        assert!(watch.sessions.is_empty(), "an emptied session is dropped");
    }

    #[test]
    fn retention_drops_long_silent_sessions_from_memory() {
        let now = 1_750_000_000_000_i64;
        let mut watch = detached_watch();
        watch.admit(
            "collab/session/old",
            &presence_frame("a"),
            now - COEDIT_RETAIN_MS - 1,
        );
        watch.admit("collab/session/live", &presence_frame("b"), now);
        watch.prune(now);
        assert!(
            !watch.sessions.contains_key("collab/session/old"),
            "the memory bound: dead sessions don't accumulate"
        );
        assert!(watch.sessions.contains_key("collab/session/live"));
    }

    #[test]
    fn the_watch_drains_a_real_persist_spool() {
        // The live path over a REAL local Persist (a throwaway dir, not the
        // mesh bus): frames written by one side surface as active co-editing,
        // the per-topic cursor advances, and a Leave dissolves the pair.
        use mde_bus::hooks::config::Priority;
        let tmp = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(tmp.path().to_path_buf()).expect("open persist");
        let topic = format!("{COLLAB_TOPIC_PREFIX}live-doc");
        for body in [presence_frame("ws-1"), update_frame("lh-01")] {
            persist
                .write(&topic, Priority::Default, None, Some(&body))
                .expect("write frame");
        }
        // An unrelated lane on the same spool is never a co-edit signal.
        persist
            .write(
                SELF_TEST_TOPIC,
                Priority::Default,
                None,
                Some(&green_report()),
            )
            .expect("write unrelated");

        let mut watch = CoEditWatch {
            bus_root: Some(tmp.path().to_path_buf()),
            cursors: HashMap::new(),
            sessions: HashMap::new(),
        };
        let now = now_unix_ms();
        watch.drain(now);
        let active = watch.active_peers(now);
        assert!(active.contains("ws-1") && active.contains("lh-01"));
        assert_eq!(active.len(), 2, "the unrelated lane added nobody");

        // Only NEW frames replay next drain (the cursor advanced): the Leave.
        persist
            .write(&topic, Priority::Default, None, Some(&leave_frame("lh-01")))
            .expect("write leave");
        let now = now_unix_ms();
        watch.drain(now);
        assert!(watch.active_peers(now).is_empty(), "the pair dissolved");
    }

    #[test]
    fn the_map_overlays_coedit_badges_for_active_session_peers() {
        // Driving a real headless frame through `show` exercises the co-edit
        // overlay end-to-end over the same snapshot the role badges paint: the
        // two on-map participants of an active session rasterize + paint the
        // accent Editor badge; an off-map participant identity badges nothing.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let now = now_unix_ms();
        let mut coedit = detached_watch();
        coedit.admit("collab/session/pair", &presence_frame("ws-1"), now);
        coedit.admit("collab/session/pair", &update_frame("lh-01"), now);
        coedit.admit(
            "collab/session/pair",
            &presence_frame("laptop-elsewhere"),
            now,
        );
        let mut mv = MeshViewState {
            state: project(&snapshot()),
            coedit,
            ..Default::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                mv.show(ui);
            });
        });
        assert!(
            !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
            "the map + co-edit overlay produced no draw primitives"
        );
        assert!(
            mv.badges
                .textures
                .contains_key(&(IconId::Editor, COEDIT_TINT)),
            "the Editor glyph rasterized at the accent tint"
        );
        // 3 role badges (QBRAND-8) + exactly 1 co-edit badge texture — the
        // off-map identity fabricated no node and no extra texture.
        assert_eq!(mv.badges.textures.len(), 4);
    }

    #[test]
    fn a_solo_session_paints_no_coedit_badge() {
        // One participant on the map, nobody else in the session → the map
        // stays honest: role badges only, no "co-editing" marker.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut coedit = detached_watch();
        coedit.admit(
            "collab/session/solo",
            &presence_frame("ws-1"),
            now_unix_ms(),
        );
        let mut mv = MeshViewState {
            state: project(&snapshot()),
            coedit,
            ..Default::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                mv.show(ui);
            });
        });
        assert!(
            !mv.badges
                .textures
                .contains_key(&(IconId::Editor, COEDIT_TINT)),
            "no co-edit badge for a solo session"
        );
        assert_eq!(mv.badges.textures.len(), 3, "role badges only");
    }
}

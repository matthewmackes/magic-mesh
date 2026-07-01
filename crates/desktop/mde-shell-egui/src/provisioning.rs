//! Workbench · Provisioning — the mesh's live onboarding / deployment posture
//! (WB-Provisioning).
//!
//! The fourth (and last) snapshot-fed Workbench plane, wired off the SAME
//! world-readable mesh-status snapshot the chrome bar + This Node + Network planes
//! fold (`/run/mde/mesh-status.json`, written every ~30s by the root
//! `mesh-status.timer`). The desktop user can't read the root-only replicated peer
//! directory, so this JSON is the desktop tier's read path — the shell leans on no
//! `mackesd` IPC (§6). Every field here is real, live-updating provisioning
//! reality; nothing is a stand-in (§7):
//!
//! * **Fleet deployment posture** — every node's provisioning tier (`nodes[].role`
//!   → Lighthouse / Server / XCP-NG / Workstation), rolled up into a per-tier count
//!   so "what's deployed where" reads at a glance. An empty / unrecognised role
//!   token buckets honestly as "Unassigned" (no role pinned yet), never guessed.
//! * **Version posture** — the fleet-wide `latest_version` (the provisioning /
//!   update target) against each node's reported `version` + its `update` flag: how
//!   many nodes are provisioned & current vs how many have an update available
//!   (the update surface), and how many are enrolled but haven't reported a build.
//! * **Enrollment / identity readiness** — per node, its tier, presence in the
//!   directory (the mesh-identity signal — a node only appears here once it's
//!   enrolled), its reported build, and whether it's current / pending-update /
//!   not-yet-reporting. The honest "is this node fully provisioned" view.
//!
//! What this surface honestly **cannot** show: a live onboarding wizard's state or
//! in-progress install steps (kickstart phase, enrollment handshake, image pull).
//! Those aren't in the world-readable snapshot — they're the `mackesd` /
//! onboard-engine's live state, and §6 keeps the shell off that path. The panel
//! renders an explicit "not published to this surface" note rather than a
//! fabricated progress bar (§7), exactly as This Node / Network did for their
//! off-surface telemetry.
//!
//! `project` is pure (no IO, no egui, no GPU), so it's unit-tested directly; the
//! only IO is the snapshot read in [`ProvisioningState::poll`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use serde_json::Value;

/// The world-readable mesh-status snapshot — the same source the chrome bar + the
/// This Node / Network planes read (the desktop user can't read the root-only
/// replicated peer directory).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a node enrolling, a role pin, or an update landing surfaces
/// within this window. Matches the chrome bar + the This Node / Network / Fleet
/// poll; the read is a cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip /
/// sibling planes use, so a presence / update dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

/// The deployment tiers in a canonical rollup order (control-plane inward → the
/// workstation edge → the honest "no role pinned" / "unrecognised" buckets). The
/// rollup counts nodes per tier and renders them in THIS order so the list is
/// stable frame-to-frame; a tier with no nodes is simply omitted (never a false
/// "0 of tier").
const TIER_ORDER: [&str; 6] = [
    "Lighthouse",
    "Server",
    "XCP-NG",
    "Workstation",
    "Unassigned",
    "Other",
];

/// Map a node's snapshot `role` token onto its provisioning tier label. The
/// snapshot's `role` records what the node was *provisioned* as (its `role.toml` /
/// peer-record token), a broader deployment vocabulary than the live 2-role
/// `mde-role` model (which folds the retired Server / XCP-NG tiers into
/// Workstation). An empty / `-` / unknown token is honestly "Unassigned" (no role
/// pinned yet), never guessed; a genuinely unrecognised token buckets as "Other"
/// rather than being silently dropped from the rollup (§7).
fn role_tier(role: Option<&str>) -> &'static str {
    let token = role
        .map(|r| r.trim().to_ascii_lowercase())
        .unwrap_or_default();
    match token.as_str() {
        "lighthouse" => "Lighthouse",
        "workstation" | "full" => "Workstation",
        "server" | "headless" => "Server",
        "xcp-ng" | "xcpng" | "xcp" => "XCP-NG",
        "" | "-" | "unknown" | "unassigned" | "none" => "Unassigned",
        _ => "Other",
    }
}

/// English plural suffix for a count.
const fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// ──────────────────────────── projected view ────────────────────────────

/// One node's provisioning posture, folded from its snapshot directory row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProvNode {
    /// The node's hostname (the directory key).
    hostname: String,
    /// The provisioning tier label, normalised from the snapshot `role` token.
    tier: &'static str,
    /// The node's reported `mde-core` build, once it has published a
    /// shell-status; `None` when enrolled but not yet reporting.
    version: Option<String>,
    /// `true` when a newer version than this node's is live on the mesh (the
    /// snapshot's per-node `update` flag) — the update / provisioning surface.
    update_available: bool,
    /// Directory presence tier: `online` / `idle` / `offline`, when known.
    presence: Option<String>,
    /// `true` when this row is this node's own directory row.
    is_self: bool,
}

impl ProvNode {
    /// This node's provisioning state as a (label, tone): provisioned & current,
    /// an update pending, or enrolled but not yet reporting a build.
    const fn state(&self) -> (&'static str, Color32) {
        match (&self.version, self.update_available) {
            (Some(_), false) => ("provisioned · current", Style::OK),
            (Some(_), true) => ("update available", Style::WARN),
            (None, _) => ("enrolled · build not reported", Style::TEXT_DIM),
        }
    }
}

/// The mesh's live provisioning status, folded from the mesh-status snapshot. Pure
/// data (parsed without egui/IO/GPU), so it's unit-tested directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProvStatus {
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the connecting state) from a parsed one.
    seen: bool,
    /// The newest version seen across the mesh — the provisioning / update target.
    latest_version: Option<String>,
    /// Per-node provisioning rows (every node the snapshot names).
    nodes: Vec<ProvNode>,
}

/// Read a non-empty string field off a JSON object, or `None`.
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl ProvStatus {
    /// Fold the mesh-status snapshot into the mesh's provisioning status.
    /// `fallback_host` is the locally-resolved hostname, used only when the
    /// snapshot omits its `self` marker (so the "this node" chip still resolves). A
    /// missing / garbage / non-mesh snapshot yields the honest unseen status
    /// (drives the connecting state), never a panic — mirroring the sibling planes'
    /// tolerance.
    fn project(snapshot: &str, fallback_host: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(snapshot) else {
            return Self::default();
        };
        let self_host = nonempty(&v, "self");
        let nodes_arr = v.get("nodes").and_then(Value::as_array);
        // A real snapshot names at least `self` or a `nodes` array; anything else
        // (an empty object, an array, a fragment) reads as unseen.
        if self_host.is_none() && nodes_arr.is_none() {
            return Self::default();
        }

        let hostname = self_host.unwrap_or_else(|| fallback_host.to_string());
        let nodes = nodes_arr
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| {
                        let host = nonempty(n, "hostname")?;
                        Some(ProvNode {
                            is_self: host == hostname,
                            tier: role_tier(nonempty(n, "role").as_deref()),
                            version: nonempty(n, "version"),
                            update_available: n
                                .get("update")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            presence: nonempty(n, "presence"),
                            hostname: host,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            seen: true,
            latest_version: nonempty(&v, "latest_version"),
            nodes,
        }
    }

    /// The per-tier deployment rollup in canonical order, omitting empty tiers.
    fn role_rollup(&self) -> Vec<(&'static str, usize)> {
        TIER_ORDER
            .iter()
            .filter_map(|&tier| {
                let n = self.nodes.iter().filter(|node| node.tier == tier).count();
                (n > 0).then_some((tier, n))
            })
            .collect()
    }

    /// Nodes that have reported a build (published a version).
    fn reported(&self) -> usize {
        self.nodes.iter().filter(|n| n.version.is_some()).count()
    }

    /// Reporting nodes provisioned to the latest build (no update pending).
    fn current(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.version.is_some() && !n.update_available)
            .count()
    }

    /// Nodes with an update available (the update surface; implies a reported
    /// build, since the snapshot only sets `update` when a version is present).
    fn behind(&self) -> usize {
        self.nodes.iter().filter(|n| n.update_available).count()
    }

    /// Nodes enrolled in the directory but not yet reporting a build.
    fn unreported(&self) -> usize {
        self.nodes.iter().filter(|n| n.version.is_none()).count()
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

// ──────────────────────────── the Provisioning state ────────────────────────────

/// The Provisioning plane's live state: the projected status plus the small IO
/// context to refresh it on the shared cadence.
pub(crate) struct ProvisioningState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// This node's locally-resolved hostname — the fallback `self` when the
    /// snapshot omits it (resolved once).
    local_host: String,
    /// The latest projection. Unseen until the first snapshot lands (drives the
    /// connecting state).
    status: ProvStatus,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ProvisioningState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            local_host: local_hostname(),
            status: ProvStatus::default(),
            last_poll: None,
        }
    }
}

impl ProvisioningState {
    /// The poll seam: refresh the projection from the snapshot when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a node enrolling, a role
    /// pin, or an update landing surfaces without input. Cheap enough to call every
    /// frame — it self-gates. A missing / unreadable snapshot yields the unseen
    /// status, never a panic.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = std::fs::read_to_string(&self.snapshot_path).unwrap_or_default();
            self.status = ProvStatus::project(&snapshot, &self.local_host);
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

/// Render the mesh's live provisioning status: the connecting state before the
/// first snapshot, else the deployment / version / nodes cards over an honest
/// onboarding-boundary note.
fn show_status(ui: &mut egui::Ui, status: &ProvStatus) {
    if !status.seen {
        ui.add_space(Style::SP_S);
        ui.colored_label(Style::TEXT_DIM, "Reading the mesh provisioning status…");
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(
                "Per-node deployment role, the fleet version target, and each node's \
                 enrollment posture fold from the world-readable mesh-status snapshot.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.group(|ui| show_posture(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Version posture")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_versions(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Nodes")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_nodes(ui, status));
            ui.add_space(Style::SP_S);

            // Honest boundary (§6/§7): a live onboarding wizard's state / in-progress
            // install steps aren't on this world-readable surface — never fake one.
            mde_egui::muted_note(
                ui,
                "Live install progress and a node's onboarding-wizard state aren't published \
                     to this surface — the shell reads the mesh directory, not the onboarding \
                     engine.",
            );
        });
}

/// The deployment-posture card: the fleet node count + the per-tier role rollup —
/// how many nodes of each provisioning tier are deployed.
fn show_posture(ui: &mut egui::Ui, status: &ProvStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Fleet provisioning")
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        let total = status.nodes.len();
        mde_egui::muted_note(ui, format!("{total} node{}", plural(total)));
    });
    ui.add_space(Style::SP_XS);

    let rollup = status.role_rollup();
    if rollup.is_empty() {
        mde_egui::muted_note(ui, "No nodes in the directory yet.");
        return;
    }
    for (tier, n) in rollup {
        ui.horizontal(|ui| {
            ui.colored_label(Style::ACCENT, RichText::new(tier).size(Style::SMALL));
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::TEXT, RichText::new(n.to_string()).size(Style::SMALL));
        });
    }
}

/// The version-posture card: the fleet-wide update target, then the provisioned /
/// pending-update / not-yet-reporting rollup — the update surface.
fn show_versions(ui: &mut egui::Ui, status: &ProvStatus) {
    match &status.latest_version {
        Some(v) => mde_egui::field(ui, "Latest on mesh", v, Style::TEXT),
        None => mde_egui::field(
            ui,
            "Latest on mesh",
            "no published version yet",
            Style::TEXT_DIM,
        ),
    }

    let (current, behind, unreported, reported) = (
        status.current(),
        status.behind(),
        status.unreported(),
        status.reported(),
    );

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Provisioned")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let tone = if reported == 0 {
            Style::TEXT_DIM
        } else if behind == 0 {
            Style::OK
        } else {
            Style::WARN
        };
        ui.colored_label(
            tone,
            RichText::new(format!("{current}/{reported} current")).size(Style::SMALL),
        );
    });

    if behind > 0 {
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(Style::WARN).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::WARN,
                RichText::new(format!("{behind} node{} to update", plural(behind)))
                    .size(Style::SMALL),
            );
        });
    }
    if unreported > 0 {
        mde_egui::field(
            ui,
            "Not yet reporting",
            &format!("{unreported} enrolled node{}", plural(unreported)),
            Style::TEXT_DIM,
        );
    }
}

/// The nodes card: one row per node — presence dot · hostname · this-node chip ·
/// tier · reported build · provisioning state.
fn show_nodes(ui: &mut egui::Ui, status: &ProvStatus) {
    if status.nodes.is_empty() {
        mde_egui::muted_note(ui, "No nodes in the directory yet.");
        return;
    }
    for node in &status.nodes {
        let ptone = node
            .presence
            .as_deref()
            .map_or(Style::TEXT_DIM, presence_tone);
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(ptone).size(Style::SMALL));
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
            ui.add_space(Style::SP_XS);
            ui.colored_label(Style::ACCENT, RichText::new(node.tier).size(Style::SMALL));
            ui.add_space(Style::SP_S);
            mde_egui::muted_note(ui, node.version.as_deref().unwrap_or("—"));
            ui.add_space(Style::SP_S);
            let (label, tone) = node.state();
            ui.colored_label(tone, RichText::new(label).size(Style::SMALL));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A faithful mesh-status snapshot: `self` + the fleet `latest_version` + a
    /// `nodes` directory spanning every provisioning posture — the exact shape
    /// `mesh-status-snapshot.sh` writes. The four nodes exercise all the plane's
    /// paths: distinct deployment tiers (Workstation / Lighthouse / Server /
    /// XCP-NG), a node current on the latest build, a node with an update pending
    /// (`update:true`), and a freshly-enrolled node that hasn't reported a build
    /// yet (no `version`).
    fn snapshot(self_host: &str) -> String {
        format!(
            r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "latest_version": "11.2.0",
              "online": 2,
              "total": 4,
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "version":"11.2.0","role":"workstation","update":false,
                  "services":{{"mackesd":true,"nebula":true}}}},
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online",
                  "version":"11.1.0","role":"lighthouse","update":true,"services":{{}}}},
                {{"hostname":"store-1","overlay_ip":"10.42.0.3","presence":"idle",
                  "version":"11.2.0","role":"server","update":false,"services":{{}}}},
                {{"hostname":"new-node","overlay_ip":"10.42.0.9","presence":"offline",
                  "role":"xcp-ng","services":{{}}}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"lh-01","overlay_ip":"10.42.0.7",
                "cipher":"AES-256-GCM"}}
            }}"#
        )
    }

    /// Drive one headless 960×640 frame of `show_status` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives minus
    /// the GPU. Returns whether it produced any draw primitives.
    fn renders(status: &ProvStatus) -> bool {
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
        let s = ProvStatus::default();
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
            let s = ProvStatus::project(bad, "this-node");
            assert!(!s.seen, "{bad:?} must not read as a live snapshot");
        }
    }

    #[test]
    fn role_tier_normalises_the_deployment_vocabulary() {
        // The four named deployment tiers, incl. XCP-NG's spellings.
        assert_eq!(role_tier(Some("lighthouse")), "Lighthouse");
        assert_eq!(role_tier(Some("Workstation")), "Workstation");
        assert_eq!(role_tier(Some("server")), "Server");
        assert_eq!(role_tier(Some("xcp-ng")), "XCP-NG");
        assert_eq!(role_tier(Some("xcp")), "XCP-NG");
        // No role pinned → honest "Unassigned", never guessed.
        assert_eq!(role_tier(Some("")), "Unassigned");
        assert_eq!(role_tier(Some("-")), "Unassigned");
        assert_eq!(role_tier(None), "Unassigned");
        // An unrecognised token is bucketed, never silently dropped (§7).
        assert_eq!(role_tier(Some("gateway")), "Other");
    }

    #[test]
    fn project_folds_the_rollup_version_posture_and_node_rows() {
        let s = ProvStatus::project(&snapshot("this-node"), "fallback");
        assert!(s.seen, "a real snapshot reads as seen");

        // The provisioning / update target.
        assert_eq!(s.latest_version.as_deref(), Some("11.2.0"));

        // Deployment posture — every named node is a row, one of each tier, and the
        // rollup renders in canonical order (Lighthouse first, Workstation later).
        assert_eq!(s.nodes.len(), 4, "every named node is a provisioning row");
        assert_eq!(
            s.role_rollup(),
            vec![
                ("Lighthouse", 1),
                ("Server", 1),
                ("XCP-NG", 1),
                ("Workstation", 1),
            ],
            "the per-tier rollup, in canonical order, omitting empty tiers"
        );

        // Version posture — this-node + store-1 are current on 11.2.0, lh-01 has an
        // update pending, new-node hasn't reported a build yet.
        assert_eq!(s.reported(), 3, "three nodes have reported a build");
        assert_eq!(
            s.current(),
            2,
            "two reporting nodes are on the latest build"
        );
        assert_eq!(s.behind(), 1, "one node has an update available");
        assert_eq!(
            s.unreported(),
            1,
            "one enrolled node hasn't reported a build"
        );

        // Per-node provisioning state — each of the three states is reachable, and
        // the self chip resolves.
        let this = s.nodes.iter().find(|n| n.hostname == "this-node").unwrap();
        assert!(this.is_self, "this node's own row is marked");
        assert_eq!(this.tier, "Workstation");
        assert_eq!(this.state().0, "provisioned · current");
        let lh = s.nodes.iter().find(|n| n.hostname == "lh-01").unwrap();
        assert_eq!(lh.state().0, "update available");
        let fresh = s.nodes.iter().find(|n| n.hostname == "new-node").unwrap();
        assert_eq!(fresh.tier, "XCP-NG");
        assert!(fresh.version.is_none());
        assert_eq!(fresh.state().0, "enrolled · build not reported");

        // And the whole live panel — rollup + version posture + node rows — paints,
        // NOT the connecting placeholder copy (the live branch, `seen`, is taken).
        assert!(
            renders(&s),
            "the live Provisioning panel produced no draw primitives"
        );
    }

    #[test]
    fn self_marker_absent_falls_back_to_local_hostname() {
        // A snapshot with a nodes directory but no `self` marker → the plane still
        // resolves this node's own row (for the "this node" chip) by the locally-
        // resolved hostname.
        let snap = r#"{"generated_ms":1,"online":1,"total":1,"latest_version":"11.2.0",
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "version":"11.2.0","role":"workstation","update":false,"services":{}}]}"#;
        let s = ProvStatus::project(snap, "this-node");
        assert!(s.seen);
        assert!(
            s.nodes
                .iter()
                .any(|n| n.is_self && n.hostname == "this-node"),
            "the self row resolves against the fallback hostname"
        );
    }

    #[test]
    fn seen_but_no_versions_renders_the_honest_partial() {
        // The directory is readable but no node has reported a build yet (no
        // versions, no `latest_version`): the tier rollup still renders, and the
        // version posture honestly says so rather than fabricating "current" (§7).
        let snap = r#"{"self":"this-node","online":1,"total":2,
            "nodes":[{"hostname":"this-node","presence":"online","role":"workstation"},
                     {"hostname":"lh-01","presence":"online","role":"lighthouse"}]}"#;
        let s = ProvStatus::project(snap, "fallback");
        assert!(s.seen, "the snapshot was parsed");
        assert!(
            s.latest_version.is_none(),
            "no published version on the mesh"
        );
        assert_eq!(s.reported(), 0, "no node has reported a build");
        assert_eq!(s.current(), 0, "nothing claimed current without a build");
        assert_eq!(s.unreported(), 2, "both nodes are enrolled, not reporting");
        assert_eq!(
            s.role_rollup(),
            vec![("Lighthouse", 1), ("Workstation", 1)],
            "the tier rollup still renders off the role tokens"
        );
        assert!(renders(&s), "the honest-partial panel still fully paints");
    }

    #[test]
    fn provisioning_state_defaults_to_the_snapshot_path_unseen() {
        let st = ProvisioningState::default();
        assert_eq!(st.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!st.status.seen);
        assert!(st.last_poll.is_none());
    }
}

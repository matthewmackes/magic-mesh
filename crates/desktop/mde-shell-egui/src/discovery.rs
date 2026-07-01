//! E12-5b — the shell **remote-desktop picker** (discovery).
//!
//! The operator-facing half of the E12-5 remote-desktop milestone: where
//! [`crate::vdi`] *renders* a brokered VM desktop and the mackesd `session_broker`
//! *tracks* the live sessions, this view lists the VMs the mesh is advertising and,
//! on **Connect**, emits the session-`Open` request the broker folds into the
//! roaming-session roster — then hands the picked target to the [`VdiState`] so the
//! Desktop surface takes over (an honest "connecting" caption until the gated
//! E12-4 wire transport attaches the live decoder).
//!
//! ## One inventory, one wire contract (§6 glue)
//!
//! * **The VM list** reuses the Fleet plane's live `event/vm/instances` roster via
//!   [`crate::datacenter::read_inventory`] — there is no second VM source. Each row
//!   is a [`VmRow`] (peer · name · state) flattened from the same projection the
//!   Datacenter view renders.
//! * **The Connect request** reuses the broker's *wire contract*, not its Rust
//!   type. §6 keeps the shell in the desktop tier — it leans inward only on
//!   `mde-bus`, never the `mackesd` daemon crate (whose `SessionRequest` is gated
//!   behind the heavy `async-services` feature: tokio / zbus / etcd). So, exactly
//!   as [`crate::datacenter`]'s `Lifecycle` mirrors the VM-lifecycle action, the
//!   local [`ConnectRequest`] serialises to the identical `action/vdi/session`
//!   body the broker's `parse_request` decodes — reusing the contract, not
//!   inventing a parallel one (a round-trip test pins the shape).
//!
//! The live cross-peer serving is **gated** downstream in the broker (its
//! `MeshSessionStore` returns a typed gated error); publishing the `Open` request
//! and driving the surface are the reachable near half of the flow, so this view is
//! a real caller — never a placeholder (§7).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{self, RichText};
use mde_egui::{Motion, Style};
use serde::Serialize;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::datacenter::{read_inventory, VmRow};
use crate::vdi::RequestedTarget;

/// The broker's session-lifecycle topic — the exact wire topic
/// `mackesd::workers::session_broker::ACTION_TOPIC` drains. We publish the `Open`
/// verb here; the leader-gated broker folds it into the roaming-session roster.
const ACTION_TOPIC: &str = "action/vdi/session";

/// Inventory refresh cadence. The Bus read is a cheap local scan and a VM roster is
/// a slow, human-paced event, so a 5 s poll surfaces a new/removed VM without
/// spinning — the same cadence the Fleet plane refreshes at.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip
/// / This Node / Network use, so a VM state dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

// ─────────────────────────── the Open request (wire mirror) ───────────────────────────

/// The shell's local mirror of the broker's `SessionRequest::Open` — the ONE
/// session verb the discovery picker emits. See the module doc for why this is a
/// wire mirror rather than a direct dependency on the daemon's type (§6). The
/// remaining verbs (`Active` / `Disconnect` / `Close`) are the broker's own
/// lifecycle transitions, not the shell's to publish, so only `Open` is mirrored —
/// exactly as `datacenter::Lifecycle` mirrors only the verbs the Fleet view emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ConnectRequest {
    /// Open a new session for `vm_id` on `serving_peer`, driven by `client_peer`
    /// (this node's shell). Serialises to the `SessionRequest::Open` body shape.
    Open {
        /// The session id to mint (the roster key).
        id: String,
        /// The peer that will serve the VM (a scheduler node id).
        serving_peer: String,
        /// The target VM. The reused Fleet inventory advertises the libvirt domain
        /// *name* (the UUID isn't on the `event/vm/instances` wire), so the
        /// (`serving_peer`, name) pair is the VM handle — the broker's `VmId` is a
        /// plain string that accepts it; a later compute-registry UUID drops in
        /// here without touching the wire shape.
        vm_id: String,
        /// The peer whose shell drives the desktop — this node.
        client_peer: String,
    },
}

impl ConnectRequest {
    /// Serialise to the `action/vdi/session` request body. A fixed derive-backed
    /// shape ⇒ serialisation can't realistically fail; an empty body (never
    /// produced here) would simply be rejected by the broker's parser.
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Mint the opaque session id the broker keys the roster on. Production uses a
/// ULID; here it's a `vdi-<ms>-<vm>` id — unique per connect on a node without
/// pulling a ULID dep, and deterministic given `now_ms` (its only entropy) so the
/// pure request builder stays testable, mirroring the broker's no-ambient-clock
/// core.
fn mint_session_id(vm: &str, now_ms: u64) -> String {
    format!("vdi-{now_ms}-{vm}")
}

/// Build the broker `Open` request for `row`, stamping this node as `client_peer`.
/// Pure + deterministic given `now_ms` so the emitted wire shape is unit-testable.
fn build_open(row: &VmRow, client_peer: &str, now_ms: u64) -> ConnectRequest {
    ConnectRequest::Open {
        id: mint_session_id(&row.name, now_ms),
        serving_peer: row.host.clone(),
        vm_id: row.name.clone(),
        client_peer: client_peer.to_string(),
    }
}

/// Wall-clock milliseconds since the Unix epoch (saturated, never panicking) — the
/// session-id entropy at Connect time. Passed in on the pure path (`build_open`).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The local peer name stamped as the session's `client_peer`: `$HOSTNAME` →
/// `/etc/hostname` → `"localhost"` (the desktop-tier idiom, shared with
/// `mde-panel-egui`). The mesh identifies nodes by hostname.
fn local_peer() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    "localhost".to_string()
}

/// Publish an `Open` request `body` to `action/vdi/session` via the persist-first
/// path (`mde-bus publish`'s own path): the write is recorded locally and the Bus
/// replicates it to the serving peer. Records any failure in `last_error` — never
/// panics. (The cross-peer *serving* is gated downstream in the broker; the publish
/// itself is the reachable near half.)
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, body: &str) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — can't request a desktop session.".to_string());
        return;
    };
    match Persist::open(root.to_path_buf())
        .and_then(|p| p.write(ACTION_TOPIC, Priority::Default, None, Some(body)))
    {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't request the session: {e}")),
    }
}

// ──────────────────────────── the discovery state ────────────────────────────

/// The remote-desktop picker's state: the mesh VM inventory (reused from the Fleet
/// plane), the selected row, the small IO/error context, and the one-shot Connect
/// hand-off the shell drains to drive the Desktop surface.
pub(crate) struct DiscoveryState {
    /// Desktop-client Bus spool (resolved once). `None` on a box with no Bus dir —
    /// the view then shows its empty state / records an error, never panics.
    bus_root: Option<PathBuf>,
    /// This node's peer name — the session's `client_peer` (resolved once).
    client_peer: String,
    /// The mesh-wide VM inventory (peer · name · state), refreshed off the Bus.
    vms: Vec<VmRow>,
    /// The selected row (index into `vms`), if any — Connect acts on it.
    selected: Option<usize>,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
    /// The last publish error, surfaced inline (honest; never a panic).
    last_error: Option<String>,
    /// The target chosen this frame, if Connect fired — drained by the shell via
    /// [`DiscoveryState::take_connect`] and handed to [`crate::vdi::VdiState`].
    connect: Option<RequestedTarget>,
}

impl Default for DiscoveryState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            client_peer: local_peer(),
            vms: Vec::new(),
            selected: None,
            last_poll: None,
            last_error: None,
            connect: None,
        }
    }
}

impl DiscoveryState {
    /// The bus-poll seam: refresh the inventory when the cadence has elapsed, then
    /// keep the repaint heartbeat alive so a new/removed VM surfaces without input.
    /// Cheap enough to call every frame — it self-gates.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Re-read the mesh VM inventory off the Bus (reusing the Fleet plane's reader).
    /// A stale selection that no longer indexes a row is dropped. Split from the
    /// cadence gate; a missing dir yields an empty inventory, never a panic.
    fn refresh(&mut self) {
        self.vms = read_inventory(self.bus_root.as_deref());
        if self.selected.is_some_and(|i| i >= self.vms.len()) {
            self.selected = None;
        }
    }

    /// Build + emit the broker `Open` request for the selected VM and record the
    /// target to hand to the Desktop surface. Returns the published wire body (for
    /// the test to assert the shape); `None` when nothing valid is selected. The
    /// Connect button drives exactly this path.
    fn connect_selected(&mut self, now_ms: u64) -> Option<String> {
        let row = self.vms.get(self.selected?)?.clone();
        let body = build_open(&row, &self.client_peer, now_ms).to_body();
        publish(self.bus_root.as_deref(), &mut self.last_error, &body);
        self.connect = Some(RequestedTarget::new(row.host, row.name));
        Some(body)
    }

    /// Take (and clear) the target the picker chose this frame — the shell hands it
    /// to [`crate::vdi::VdiState`] so the Desktop surface takes over.
    pub(crate) fn take_connect(&mut self) -> Option<RequestedTarget> {
        self.connect.take()
    }
}

/// Render the remote-desktop picker into `ui`: the mesh's advertised VMs
/// (peer · name · state) with a Connect action, or an honest `EmptyState` when no
/// peer is advertising a VM.
pub(crate) fn discovery_panel(ui: &mut egui::Ui, state: &mut DiscoveryState) {
    if let Some(err) = state.last_error.as_deref() {
        ui.colored_label(Style::DANGER, err);
        ui.add_space(Style::SP_S);
    }

    if state.vms.is_empty() {
        let (title, subtitle) = empty_copy(state.bus_root.is_some());
        crate::session::empty_state(ui, title, subtitle);
        return;
    }

    // Section label — the mature planes' idiom (dim, small, sentence case).
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Remote desktops")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.add_space(Style::SP_XS);

    // Row list — clicking a row selects it (the Connect action reads the selection
    // after the shared borrow of `vms` ends).
    let selected = state.selected;
    let mut clicked: Option<usize> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, vm) in state.vms.iter().enumerate() {
                let running = vm.state.trim() == "running";
                let dot = if running { Style::OK } else { Style::TEXT_DIM };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
                    ui.add_space(Style::SP_XS);
                    if ui
                        .selectable_label(
                            selected == Some(i),
                            RichText::new(format!("{} \u{00B7} {}", vm.host, vm.name))
                                .size(Style::BODY),
                        )
                        .clicked()
                    {
                        clicked = Some(i);
                    }
                    ui.add_space(Style::SP_S);
                    mde_egui::muted_note(ui, &vm.state);
                });
            }
        });
    if let Some(i) = clicked {
        state.selected = Some(i);
    }

    // Connect — enabled only with a live selection; drives the broker request +
    // the hand-off to the Desktop surface.
    ui.add_space(Style::SP_S);
    let can_connect = state.selected.is_some_and(|i| i < state.vms.len());
    // Row-select feedback: the picked-target hint eases in on the shared FAST
    // curve (§4 — motion via the shared table only, no bespoke engine).
    let hint = Motion::animate(
        ui.ctx(),
        "discovery-connect-hint",
        can_connect,
        Motion::FAST,
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                can_connect,
                egui::Button::new(RichText::new("Connect").size(Style::BODY)),
            )
            .clicked()
        {
            state.connect_selected(now_ms());
        }
        if let Some(vm) = state.selected.and_then(|i| state.vms.get(i)) {
            ui.add_space(Style::SP_S);
            ui.scope(|ui| {
                ui.set_opacity(hint);
                mde_egui::muted_note(ui, format!("\u{2192} {} on {}", vm.name, vm.host));
            });
        }
    });

    ui.add_space(Style::SP_XS);
    mde_egui::muted_note(
        ui,
        "Connecting brokers the desktop over the mesh — the live cross-peer transport is \
             gated (E12-4).",
    );
}

/// The empty-picker copy — honest about *why* there is nothing to connect to.
/// With no mesh Bus directory the VM inventory is unreadable (a gated read),
/// which must not render as a live-looking "no desktops" (§7).
const fn empty_copy(has_bus: bool) -> (&'static str, &'static str) {
    if has_bus {
        (
            "No remote desktops available",
            "No peer is advertising a VM — start one on a mesh node and it appears here within a few seconds.",
        )
    } else {
        (
            "Remote desktops unavailable",
            "No mesh Bus directory on this node, so the VM inventory can't be read — joining the mesh (the mde-bus spool) unblocks the picker.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A `DiscoveryState` seeded with inventory rows directly (bypassing the Bus),
    /// so the picker's render + Connect paths are testable headless.
    fn state_with(rows: &[(&str, &str, &str)]) -> DiscoveryState {
        DiscoveryState {
            bus_root: None,
            client_peer: "client-node".to_string(),
            vms: rows
                .iter()
                .map(|(h, n, s)| VmRow {
                    host: (*h).to_string(),
                    name: (*n).to_string(),
                    state: (*s).to_string(),
                })
                .collect(),
            selected: None,
            last_poll: None,
            last_error: None,
            connect: None,
        }
    }

    /// Drive one headless 960×640 frame of `discovery_panel` and tessellate it on
    /// the CPU — the same `Context::run` → `tessellate` path the DRM runner drives
    /// minus the GPU. Returns whether it produced any draw primitives.
    fn run_panel(state: &mut DiscoveryState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| discovery_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn empty_inventory_paints_the_honest_empty_state() {
        // No peer advertising a VM → the honest EmptyState, not a fake list (§7).
        let mut state = state_with(&[]);
        assert!(
            run_panel(&mut state),
            "the no-desktops EmptyState produced no draw primitives"
        );
        // Rendering the empty state raises no connect target.
        assert!(state.take_connect().is_none());
    }

    #[test]
    fn empty_copy_distinguishes_a_missing_bus_from_a_quiet_inventory() {
        // A live-but-quiet inventory reads as "no desktops"; a missing Bus must
        // NOT (§7 — a gated read never renders as a live-looking empty state).
        let (title, _) = empty_copy(true);
        assert_eq!(title, "No remote desktops available");
        let (title, subtitle) = empty_copy(false);
        assert_eq!(title, "Remote desktops unavailable");
        assert!(
            subtitle.contains("Bus") && subtitle.contains("unblocks"),
            "the gated copy names what's missing and what unblocks it: {subtitle}"
        );
    }

    #[test]
    fn a_populated_inventory_lists_the_rows_and_tessellates() {
        let mut state = state_with(&[("node-a", "web1", "running"), ("node-b", "db1", "shut off")]);
        assert_eq!(state.vms.len(), 2, "both advertised VMs are listed");
        assert!(
            run_panel(&mut state),
            "the VM list produced no draw primitives"
        );
    }

    #[test]
    fn connect_emits_the_broker_open_request_and_hands_off_the_target() {
        let mut state = state_with(&[("node-a", "web1", "running"), ("node-b", "db1", "shut off")]);
        // The operator selects the second VM and connects.
        state.selected = Some(1);
        let body = state
            .connect_selected(1_717_000_000_000)
            .expect("a selected row yields a request");

        // The published body is the broker's `SessionRequest::Open` wire shape —
        // the same `action/vdi/session` payload `parse_request` decodes.
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        assert_eq!(v["op"], "open");
        assert_eq!(v["serving_peer"], "node-b");
        assert_eq!(v["vm_id"], "db1");
        assert_eq!(v["client_peer"], "client-node");
        assert!(
            v["id"].as_str().is_some_and(|s| !s.is_empty()),
            "a session id is minted"
        );

        // The picked target is handed to the Desktop surface (VdiState) exactly once.
        let target = state.take_connect().expect("Connect records a target");
        assert_eq!(target.serving_peer, "node-b");
        assert_eq!(target.name, "db1");
        assert!(state.take_connect().is_none(), "the hand-off drains once");
    }

    #[test]
    fn connect_without_a_selection_is_a_no_op() {
        let mut state = state_with(&[("node-a", "web1", "running")]);
        assert!(
            state.connect_selected(1).is_none(),
            "no selection → no request"
        );
        assert!(state.take_connect().is_none(), "and no target handed off");
    }

    #[test]
    fn the_open_request_serialises_to_the_snake_case_tagged_shape() {
        // Pin the wire contract: internally `op`-tagged, snake_case — byte-for-byte
        // what the broker's `#[serde(tag = "op", rename_all = "snake_case")]`
        // `SessionRequest` expects, so this mirror can't silently drift from it.
        let row = VmRow {
            host: "peer-x".to_string(),
            name: "vm-y".to_string(),
            state: "running".to_string(),
        };
        let body = build_open(&row, "me", 42).to_body();
        assert_eq!(
            body,
            r#"{"op":"open","id":"vdi-42-vm-y","serving_peer":"peer-x","vm_id":"vm-y","client_peer":"me"}"#
        );
    }
}

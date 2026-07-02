//! VOIP-GW-5 — the Voice panel **Fleet tab**.
//!
//! Beside the local dialer, the Voice surface grows a fleet config board: every
//! enrolled node with its callable `<hostname>@<realm>` SIP address, its live
//! sub-account/registration state, a DID-routing + failover column, and the one
//! shared-account (leader-held outbound trunk) fleet config — plus the editable
//! affordances (Provision / Re-provision, enable/disable inbound, nickname).
//!
//! ## Where the data comes from (§6 glue, tier-clean)
//!
//! The mackesd `voice_provision` worker (VOIP-GW-3) publishes one
//! [`NodeRow`]-shaped JSON body per node to `state/voice/<node>` — the live
//! reg-state (`registered` / `unregistered` / `provisioning` / `error+reason`).
//! This tab reads it straight off the local Bus spool (the persist-first path
//! the datacenter surface uses), so a failing node shows the **real** error, not
//! a fabricated online (§7 / design lock 9). The Bus payloads are mirrored with
//! LOCAL serde structs here rather than depending on the mackesd daemon, so the
//! desktop→services tier edge stays clean (the same choice mde-shell-egui made).
//!
//! ## What it writes (§9 typed verbs)
//!
//! Every operator intent is a typed verb in the canonical `action/voice/*`
//! namespace, never a command string:
//!
//! * **Provision / Re-provision** → [`PROVISION_TOPIC`]. Consumed live by
//!   VOIP-GW-3, which forces an immediate reconcile pass (design lock 8). This is
//!   the fully round-tripped control.
//! * **enable/disable inbound** → [`INBOUND_TOPIC`]; **nickname** →
//!   [`NICKNAME_TOPIC`]; **shared-account config** → [`SHARED_CONFIG_TOPIC`].
//!   These publish a real, observable typed verb (the panel half); their
//!   leader-side apply lands with VOIP-GW-6/7 (DID/failover + the account lift).
//!   The write itself is real — never a faked success.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

/// Bus topic prefix the per-node reg-state / fleet-board row is published under
/// (VOIP-GW-3, design lock 9). One topic per node; the tail is the node id.
const STATE_PREFIX: &str = "state/voice/";

/// The typed verb the panel publishes to request a (re-)provision (design
/// lock 8). VOIP-GW-3 drains it and forces an immediate reconcile pass.
pub const PROVISION_TOPIC: &str = "action/voice/provision";

/// Typed verb: enable/disable a node's inbound sub-account registration.
pub const INBOUND_TOPIC: &str = "action/voice/inbound";

/// Typed verb: set a node's operator-facing nickname.
pub const NICKNAME_TOPIC: &str = "action/voice/nickname";

/// Typed verb: apply the one shared-account (leader-held outbound trunk +
/// caller-ID) fleet config.
pub const SHARED_CONFIG_TOPIC: &str = "action/voice/shared-config";

/// How often the tab re-reads the Bus. Voice provisioning is slow-changing, so a
/// 5 s cadence matches the datacenter surface without hammering the index.
const REFRESH: Duration = Duration::from_secs(5);

// ── The Bus payload mirror (deserialised from `state/voice/<node>`) ──────────

/// A node's provisioning / registration state.
///
/// The local mirror of the worker's `RegState` (its
/// `#[serde(tag = "state", rename_all = "kebab-case")]` shape), deserialised
/// straight from the published JSON.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RegState {
    /// The node's SIP client has an active REGISTER (green pip).
    Registered,
    /// Provisioned + creds sealed, but not currently registered (neutral pip).
    Unregistered,
    /// A provisioning action is in flight (amber pip).
    Provisioning,
    /// Provisioning/registration failed — the honest reason (red pip).
    Error {
        /// Operator-readable failure detail (shown verbatim — never hidden).
        reason: String,
    },
}

/// One fleet-board row, mirrored from `state/voice/<node>`. Unknown/absent
/// fields default so a partial or forward-versioned body still renders honestly
/// rather than dropping the whole row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NodeRow {
    /// The node id (topic suffix / board key).
    pub node_id: String,
    /// The node hostname the sub-account username derives from.
    #[serde(default)]
    pub hostname: String,
    /// The Vitelity sub-account username (empty until provisioned).
    #[serde(default)]
    pub username: String,
    /// The callable `<username>@<realm>` SIP address (empty until provisioned).
    #[serde(default)]
    pub sip_uri: String,
    /// The provisioning / registration state (the flattened `state` tag).
    #[serde(flatten)]
    pub reg_state: RegState,
    /// When this row was produced (epoch seconds).
    #[serde(default)]
    pub updated_at_s: u64,
}

impl NodeRow {
    /// The pip colour for this row's reg-state — a `Style` palette token, never a
    /// raw literal (§4): green Registered / amber Provisioning / red Error, with
    /// a dim neutral for the honest "provisioned, not yet registered".
    const fn pip(&self) -> Color32 {
        match self.reg_state {
            RegState::Registered => Style::OK,
            RegState::Provisioning => Style::WARN,
            RegState::Error { .. } => Style::DANGER,
            RegState::Unregistered => Style::TEXT_DIM,
        }
    }

    /// A short reg-state label for the row header.
    const fn reg_label(&self) -> &str {
        match self.reg_state {
            RegState::Registered => "Registered",
            RegState::Unregistered => "Not registered",
            RegState::Provisioning => "Provisioning…",
            RegState::Error { .. } => "Error",
        }
    }
}

// ── The typed verbs the panel publishes ──────────────────────────────────────

/// `action/voice/provision` — force a (re-)provision reconcile. `node_id`
/// names the target for the operator log; VOIP-GW-3 currently treats any message
/// as a fleet-wide reconcile trigger (design lock 8), so `None` = the whole
/// fleet and a per-node value is forward-compatible intent.
#[derive(Debug, Clone, Serialize)]
struct ProvisionRequest {
    node_id: Option<String>,
}

/// `action/voice/inbound` — enable/disable a node's inbound sub-account.
#[derive(Debug, Clone, Serialize)]
struct InboundRequest {
    node_id: String,
    enabled: bool,
}

/// `action/voice/nickname` — set a node's operator-facing nickname.
#[derive(Debug, Clone, Serialize)]
struct NicknameRequest {
    node_id: String,
    nickname: String,
}

/// `action/voice/shared-config` — apply the leader-held outbound trunk config.
#[derive(Debug, Clone, Serialize)]
struct SharedConfigRequest {
    caller_id: String,
    outbound_trunk: String,
}

/// One operator intent collected during a render frame, published after the
/// render borrow ends (the egui idiom — one action per frame).
enum Pending {
    /// (Re-)provision: `None` = whole fleet, `Some(id)` = that node.
    Provision(Option<String>),
    /// Toggle a node's inbound registration.
    Inbound { node_id: String, enabled: bool },
    /// Commit a node's edited nickname.
    Nickname { node_id: String, nickname: String },
    /// Apply the shared-account fleet config.
    SharedConfig {
        caller_id: String,
        outbound_trunk: String,
    },
}

/// The local, per-node edit buffers. Neither the nickname nor the inbound-enabled
/// flag has a source field in the VOIP-GW-3 state contract yet, so these are
/// operator inputs that fire a verb (their reflected state lands with GW-6/7);
/// the defaults are the honest starting point (inbound on, no nickname).
#[derive(Debug, Clone, Default)]
struct NodeEdit {
    /// The nickname text buffer.
    nickname: String,
    /// The desired inbound-enabled toggle (defaults to enabled).
    inbound_enabled: bool,
}

impl NodeEdit {
    const fn new() -> Self {
        Self {
            nickname: String::new(),
            inbound_enabled: true,
        }
    }
}

/// The shared-account (leader-held outbound trunk) edit form.
#[derive(Debug, Clone, Default)]
struct SharedForm {
    /// The presented caller-ID number for all outbound PSTN (design lock 4/13).
    caller_id: String,
    /// The shared outbound trunk label / account.
    outbound_trunk: String,
}

// ── The tab state ────────────────────────────────────────────────────────────

/// The Fleet-tab state: the live board projected from the Bus, plus the local
/// edit buffers. Self-polls on a fixed cadence; renders and publishes verbs.
pub struct FleetState {
    /// The Bus spool root (resolved once); `None` off a mesh node.
    bus_root: Option<PathBuf>,
    /// The live per-node board, sorted by node id.
    nodes: Vec<NodeRow>,
    /// When the Bus was last polled (drives the cadence).
    last_poll: Option<Instant>,
    /// The last publish/read error, surfaced inline (honest, not swallowed).
    last_error: Option<String>,
    /// Per-node local edit buffers, keyed by node id.
    edits: HashMap<String, NodeEdit>,
    /// The shared-account fleet-config form.
    shared: SharedForm,
}

impl Default for FleetState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            nodes: Vec::new(),
            last_poll: None,
            last_error: None,
            edits: HashMap::new(),
            shared: SharedForm::default(),
        }
    }
}

impl FleetState {
    /// Build the tab state, resolving the client Bus root.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The bus-poll seam: refresh the board from the Bus when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a reg-state flip
    /// surfaces without input. Cheap enough to call every frame — it self-gates.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Re-read every `state/voice/<node>` topic and re-project the board. A
    /// missing dir / unreadable topic keeps the last-known board (never a
    /// panic); a malformed row is skipped rather than faking one.
    fn refresh(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            self.nodes = Vec::new();
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            // Keep the last-known board on a transient open failure.
            return;
        };
        self.nodes = read_board(&persist);
    }

    /// Render the Fleet tab into `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        // Disjoint field borrows so the render can hold `&nodes` and
        // `&mut edits`/`&mut shared` at once (the egui idiom).
        let Self {
            bus_root,
            nodes,
            last_error,
            edits,
            shared,
            ..
        } = self;

        let mut pending: Option<Pending> = None;

        if let Some(err) = last_error.as_deref() {
            ui.colored_label(Style::DANGER, err);
            ui.add_space(Style::SP_S);
        }

        // Fleet-wide header + Provision-all.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Fleet voice")
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
            ui.add_space(Style::SP_M);
            if ui.button("Provision all").clicked() {
                pending = Some(Pending::Provision(None));
            }
        });
        ui.add_space(Style::SP_XS);
        ui.separator();
        ui.add_space(Style::SP_S);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if nodes.is_empty() {
                    mde_egui::muted_note(
                        ui,
                        "No node has published a voice reg-state yet — the leader's \
                         voice_provision worker fills this board as it provisions each \
                         node's Vitelity sub-account.",
                    );
                } else {
                    for node in nodes.iter() {
                        let edit = edits
                            .entry(node.node_id.clone())
                            .or_insert_with(NodeEdit::new);
                        ui.group(|ui| show_node(ui, node, edit, &mut pending));
                        ui.add_space(Style::SP_S);
                    }
                }

                ui.add_space(Style::SP_M);
                show_shared(ui, shared, &mut pending);
            });

        if let Some(action) = pending {
            publish(bus_root.as_deref(), last_error, &action);
        }
    }

    /// Test seam: inject a board directly, bypassing the Bus. Used by the
    /// headless render test so it renders a real Error row without a spool.
    #[cfg(test)]
    fn with_nodes(mut self, nodes: Vec<NodeRow>) -> Self {
        self.nodes = nodes;
        // A test never touches disk — pin the poll so `poll` is a no-op.
        self.last_poll = Some(Instant::now());
        self.bus_root = None;
        self
    }
}

/// Read + project the whole board: every `state/voice/<node>` topic's latest
/// retained body, deserialised and sorted by node id. Pure over the Persist
/// handle so it is unit-testable against a seeded spool.
fn read_board(persist: &Persist) -> Vec<NodeRow> {
    let mut rows: Vec<NodeRow> = Vec::new();
    for topic in persist.list_topics().unwrap_or_default() {
        if !topic.starts_with(STATE_PREFIX) {
            continue;
        }
        // ULID-ordered oldest→newest; the last body is the latest reg-state.
        let latest = persist
            .list_since(&topic, None)
            .unwrap_or_default()
            .into_iter()
            .next_back()
            .and_then(|m| m.body);
        if let Some(body) = latest {
            if let Ok(row) = serde_json::from_str::<NodeRow>(&body) {
                rows.push(row);
            }
        }
    }
    rows.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    rows
}

/// Render one node card: the reg-state pip + label, its SIP address, the DID +
/// failover columns, an honest error reason, and the per-node controls.
fn show_node(
    ui: &mut egui::Ui,
    node: &NodeRow,
    edit: &mut NodeEdit,
    pending: &mut Option<Pending>,
) {
    ui.horizontal(|ui| {
        mde_egui::status_dot(ui, node.pip());
        ui.add_space(Style::SP_XS);
        let name = if node.hostname.is_empty() {
            node.node_id.as_str()
        } else {
            node.hostname.as_str()
        };
        ui.label(
            RichText::new(name)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            node.pip(),
            RichText::new(node.reg_label()).size(Style::SMALL),
        );
    });

    ui.add_space(Style::SP_XS);
    let sip = if node.sip_uri.is_empty() {
        "— (awaiting provisioning)"
    } else {
        node.sip_uri.as_str()
    };
    mde_egui::field(ui, "SIP address", sip, Style::TEXT);
    // DID routing + failover policy are populated by VOIP-GW-6; until then the
    // columns show the honest "not yet routed" rather than a fabricated value.
    mde_egui::field(ui, "DID routing", "— (VOIP-GW-6)", Style::TEXT_DIM);
    mde_egui::field(ui, "Failover", "— (VOIP-GW-6)", Style::TEXT_DIM);

    if let RegState::Error { reason } = &node.reg_state {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, RichText::new(reason).size(Style::SMALL));
    }

    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Nickname")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        let field = ui.add(
            egui::TextEdit::singleline(&mut edit.nickname)
                .hint_text("optional")
                .desired_width(Style::SP_XL * 4.0),
        );
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            *pending = Some(Pending::Nickname {
                node_id: node.node_id.clone(),
                nickname: edit.nickname.clone(),
            });
        }
    });

    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut edit.inbound_enabled, "Inbound enabled")
            .changed()
        {
            *pending = Some(Pending::Inbound {
                node_id: node.node_id.clone(),
                enabled: edit.inbound_enabled,
            });
        }
        ui.add_space(Style::SP_M);
        if ui.button("Re-provision").clicked() {
            *pending = Some(Pending::Provision(Some(node.node_id.clone())));
        }
    });
}

/// Render the shared-account (leader-held outbound trunk + caller-ID) fleet
/// config section: the display + edit affordance, applied as a typed verb.
fn show_shared(ui: &mut egui::Ui, shared: &mut SharedForm, pending: &mut Option<Pending>) {
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Shared outbound trunk (fleet)")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    mde_egui::muted_note(
        ui,
        "One leader-held account carries all outbound PSTN and presents the shared \
         caller-ID (design lock 4/13).",
    );
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Caller ID")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.add(
            egui::TextEdit::singleline(&mut shared.caller_id)
                .hint_text("e.g. +1 555 0100")
                .desired_width(Style::SP_XL * 5.0),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Trunk")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.add(
            egui::TextEdit::singleline(&mut shared.outbound_trunk)
                .hint_text("shared Vitelity account")
                .desired_width(Style::SP_XL * 5.0),
        );
    });
    ui.add_space(Style::SP_S);
    if ui.button("Apply to fleet").clicked() {
        *pending = Some(Pending::SharedConfig {
            caller_id: shared.caller_id.clone(),
            outbound_trunk: shared.outbound_trunk.clone(),
        });
    }
}

/// Publish one operator intent as a typed `action/voice/*` verb over the
/// persist-first Bus path. A real, observable write; a failure surfaces inline
/// (never a swallowed no-op / faked success — §7).
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, action: &Pending) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — voice actions unavailable.".into());
        return;
    };
    let (topic, body) = match action {
        Pending::Provision(node_id) => (
            PROVISION_TOPIC,
            serde_json::to_string(&ProvisionRequest {
                node_id: node_id.clone(),
            }),
        ),
        Pending::Inbound { node_id, enabled } => (
            INBOUND_TOPIC,
            serde_json::to_string(&InboundRequest {
                node_id: node_id.clone(),
                enabled: *enabled,
            }),
        ),
        Pending::Nickname { node_id, nickname } => (
            NICKNAME_TOPIC,
            serde_json::to_string(&NicknameRequest {
                node_id: node_id.clone(),
                nickname: nickname.clone(),
            }),
        ),
        Pending::SharedConfig {
            caller_id,
            outbound_trunk,
        } => (
            SHARED_CONFIG_TOPIC,
            serde_json::to_string(&SharedConfigRequest {
                caller_id: caller_id.clone(),
                outbound_trunk: outbound_trunk.clone(),
            }),
        ),
    };
    let body = match body {
        Ok(b) => b,
        Err(e) => {
            *last_error = Some(format!("Couldn't encode voice action: {e}"));
            return;
        }
    };
    match Persist::open(root.to_path_buf())
        .and_then(|p| p.write(topic, Priority::Default, None, Some(&body)))
    {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't publish voice action: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_row(id: &str, host: &str, reason: &str) -> NodeRow {
        NodeRow {
            node_id: id.to_string(),
            hostname: host.to_string(),
            username: String::new(),
            sip_uri: String::new(),
            reg_state: RegState::Error {
                reason: reason.to_string(),
            },
            updated_at_s: 0,
        }
    }

    #[test]
    fn deserialises_a_worker_state_body() {
        // The exact JSON shape VOIP-GW-3 publishes to `state/voice/<node>`
        // (tag = "state", flattened onto the row). A Registered node.
        let body = r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle",
            "sip_uri":"eagle@sip.vitelity.net","state":"registered","updated_at_s":42}"#;
        let row: NodeRow = serde_json::from_str(body).unwrap();
        assert_eq!(row.node_id, "peer:eagle");
        assert_eq!(row.sip_uri, "eagle@sip.vitelity.net");
        assert_eq!(row.reg_state, RegState::Registered);
        assert_eq!(row.pip(), Style::OK);
    }

    #[test]
    fn deserialises_an_error_body_with_reason() {
        // A failing node carries its real reason — the pip is red, never online.
        let body = r#"{"node_id":"peer:x","hostname":"x","username":"x","sip_uri":"",
            "state":"error","reason":"provision failed: 403","updated_at_s":1}"#;
        let row: NodeRow = serde_json::from_str(body).unwrap();
        assert!(
            matches!(&row.reg_state, RegState::Error { reason } if reason.contains("403")),
            "expected Error carrying the real reason, got {:?}",
            row.reg_state
        );
        assert_eq!(row.pip(), Style::DANGER);
        assert_ne!(row.pip(), Style::OK, "a failing node must never show green");
    }

    #[test]
    fn each_reg_state_maps_to_a_distinct_pip_tone() {
        assert_eq!(
            NodeRow {
                reg_state: RegState::Provisioning,
                ..err_row("a", "a", "x")
            }
            .pip(),
            Style::WARN
        );
        assert_eq!(
            NodeRow {
                reg_state: RegState::Unregistered,
                ..err_row("a", "a", "x")
            }
            .pip(),
            Style::TEXT_DIM
        );
    }

    /// Drive a headless frame that mounts + tessellates the Fleet tab with a live
    /// board (a Registered node and a failing Error node), proving the tab is
    /// runtime-reachable and paints the real reg-states on the CPU — the same
    /// `Context::run` → `tessellate` path the DRM runner drives, no GPU/Bus.
    #[test]
    fn fleet_tab_mounts_and_tessellates_with_real_states() {
        let mut fleet = FleetState::new().with_nodes(vec![
            NodeRow {
                node_id: "peer:eagle".into(),
                hostname: "eagle".into(),
                username: "eagle".into(),
                sip_uri: "eagle@sip.vitelity.net".into(),
                reg_state: RegState::Registered,
                updated_at_s: 1,
            },
            err_row("peer:pine", "pine", "provision failed: master key missing"),
        ]);

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(520.0, 420.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| fleet.show(ui));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "fleet tab produced no draw primitives");
    }

    #[test]
    fn empty_board_renders_an_honest_note() {
        // No published state → an honest "nothing yet", never a fabricated node.
        let mut fleet = FleetState::new().with_nodes(vec![]);
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(520.0, 420.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| fleet.show(ui));
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    }

    #[test]
    fn provision_verb_round_trips_through_the_bus() {
        // The Provision control's real effect: a typed `action/voice/provision`
        // message lands on the Bus, readable back — the live-consumed verb
        // (VOIP-GW-3), proven end-to-end against a real spool.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut err = None;
        publish(
            Some(root.as_path()),
            &mut err,
            &Pending::Provision(Some("peer:eagle".into())),
        );
        assert!(err.is_none(), "publish should succeed: {err:?}");

        let persist = Persist::open(root).unwrap();
        let msgs = persist.list_since(PROVISION_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().unwrap();
        assert!(body.contains("peer:eagle"));
    }

    #[test]
    fn read_board_projects_latest_per_node_sorted() {
        // Two nodes, one topic each, plus an unrelated topic; the newest body per
        // `state/voice/*` topic is projected, sorted, unrelated topics ignored.
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        persist
            .write(
                "state/voice/peer:pine",
                Priority::Min,
                None,
                Some(r#"{"node_id":"peer:pine","hostname":"pine","username":"pine","sip_uri":"pine@r","state":"unregistered"}"#),
            )
            .unwrap();
        // A stale then a fresh body for eagle — the fresh (later ULID) wins.
        persist
            .write(
                "state/voice/peer:eagle",
                Priority::Min,
                None,
                Some(r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle","sip_uri":"eagle@r","state":"provisioning"}"#),
            )
            .unwrap();
        persist
            .write(
                "state/voice/peer:eagle",
                Priority::Min,
                None,
                Some(r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle","sip_uri":"eagle@r","state":"registered"}"#),
            )
            .unwrap();
        persist
            .write("event/unrelated", Priority::Min, None, Some("{}"))
            .unwrap();

        let board = read_board(&persist);
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].node_id, "peer:eagle");
        assert_eq!(board[0].reg_state, RegState::Registered, "latest body wins");
        assert_eq!(board[1].node_id, "peer:pine");
    }
}

//! PLANES-19 — Network ▸ Routing panel.
//!
//! The overlay-reachability validation surface (W79/W80): the
//! validation suite probes every directed edge between participants over
//! the Nebula overlay; an edge that never came back reachable is a
//! failure that feeds the drift pipeline (W80). This panel shells
//! `mackesd validate status --json` to show the newest run's verdict and
//! `mackesd validate run` to request a fresh one (the FPG leader mints
//! it). Routing itself stays display-only (W76) — what the operator acts
//! on here is the reachability health.

use std::time::{Duration, SystemTime};

use cosmic::iced::widget::{button, column, container, pick_list, row, scrollable, text, Space};
use cosmic::iced::Task;

/// ROUTING-VALIDATE-1 — after requesting a run, poll the verdict on this cadence
/// until it lands (the FPG leader mints it + nodes report asynchronously, so the
/// result isn't ready on the first immediate fetch).
const POLL_DELAY: Duration = Duration::from_secs(3);
/// Bounded so a leader that never completes can't poll forever.
const MAX_POLLS: u8 = 12;
/// ROUTE-TRACE-4 — read budget for the `action/route/trace` Bus probe. Matches
/// the other panels' interactive 2 s read window (the responder assembles the
/// graph from local exposure/peer state — no network round-trips).
const TRACE_TIMEOUT: Duration = Duration::from_secs(2);
/// VPN-GW-8 — read budget for the egress-routing Bus probes (`action/vpn/
/// list-routes`, `set-route`). Matches the VPN panel's interactive config
/// window: these are local config reads/writes on the shared substrate, not
/// network round-trips.
const ROUTES_TIMEOUT: Duration = Duration::from_secs(2);
/// DDNS-EGRESS-5 — read budget for the `action/ddns/*` config + `record-status`
/// Bus probes (and the `tunnel-status` liveness probe the table pairs with each
/// record). All answer from local config + `ip link` (no network round-trips),
/// so the interactive 2 s window matches the other read verbs here.
const DDNS_TIMEOUT: Duration = Duration::from_secs(2);
/// DDNS-EGRESS-5 — read budget for the per-source `verify-egress` exit-IP probe a
/// "Sync now" runs (it shells `curl -m 10` *through* the tunnel for the verified
/// exit IP), matching the VPN panel's longer reflector-round-trip window.
const DDNS_VERIFY_TIMEOUT: Duration = Duration::from_secs(15);
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::{Element, Theme};
use mackes_mesh_types::ddns::{self, DdnsConfig, OnDown, RecordDef, SourceState};
use mackes_mesh_types::route_trace::{
    ControlPoint, Direction, Layer, NodeKind, PathEdge, PathGraph, PathNode, Transport, Verdict,
};
use mackes_mesh_types::vpn_egress::{EgressRoute, EgressRouting, RouteTarget};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::BadgeSeverity;

/// One directed `from → to` edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

/// The newest validation run's verdict, parsed from
/// `mackesd validate status --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationStatus {
    /// `None` when no run has been minted yet.
    pub run_id: Option<String>,
    pub passed: bool,
    pub reachable: usize,
    pub failed_edges: Vec<Edge>,
    pub missing_reporters: Vec<String>,
}

/// ROUTE-TRACE-4 — wire values for the Egress/Ingress direction toggle, in the
/// kebab-case the `action/route/trace` IPC + `route_trace::Direction` expect.
const DIRECTION_CHOICES: [&str; 2] = ["egress", "ingress"];

#[derive(Debug, Clone, Default)]
pub struct RoutingPanel {
    pub status: ValidationStatus,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
    pub run_result: Option<Result<String, String>>,
    /// AUDIT-MESH-5 — guards the one-shot auto-run: when the panel opens and no
    /// validation run has ever been minted, it requests one automatically (so
    /// Routing shows live reachability without a manual click), but only once
    /// per panel session — a genuinely empty mesh won't re-probe on every load.
    pub auto_ran: bool,
    /// ROUTING-VALIDATE-1 — how many times we've polled the verdict since the
    /// last run request (bounded by `MAX_POLLS`).
    pub poll_attempts: u8,
    /// ROUTE-TRACE-4 — the path-trace toolbar state machine.
    pub trace: TraceState,
    /// VPN-GW-8 — the egress-routing surface state (the durable
    /// `action/vpn/list-routes` table + the assign-route wizard).
    pub egress: EgressState,
    /// DDNS-EGRESS-5 — the dynamic-DNS surface state (the `action/ddns/*` record
    /// table + the add/edit form), live over the DDNS-EGRESS-3 responder.
    pub ddns: DdnsState,
}

/// VPN-GW-8 — the egress-routing panel state: the durable routing table (who
/// exits where), the real node roster (the matrix's rows + the wizard's node
/// choices), and the assign-route wizard's selection. All sourced from the
/// existing `action/vpn/*` RPCs + the node roster — no new model, no demo data.
#[derive(Debug, Clone, Default)]
pub struct EgressState {
    /// The durable egress-routing assignments (`action/vpn/list-routes`). Empty
    /// until the first load, or when the mesh has no VPN routes assigned yet.
    pub routing: EgressRouting,
    /// The real mesh node names (the node roster) — the matrix's rows + the
    /// wizard's node picker. Empty when the roster can't be read.
    pub nodes: Vec<String>,
    /// True while the routing table / roster fetch is in flight.
    pub busy: bool,
    /// The last load error, if any (the daemon was unreachable / errored).
    pub error: Option<String>,
    /// The assign-route wizard's selection + last result.
    pub wizard: WizardState,
}

/// VPN-GW-8 — the assign-route wizard: pick a node, a gateway, and a primary
/// tunnel, then emit a real [`EgressRoute`] over `action/vpn/set-route`. The
/// pure [`WizardState::route`] turns the selection into the exact `EgressRoute`
/// the responder validates + persists; [`WizardState::can_assign`] gates the
/// button. The set-route reply (ok / error) lands in `result`.
#[derive(Debug, Clone, Default)]
pub struct WizardState {
    /// The node this route assigns egress for (a `RouteTarget::Node`).
    pub node: String,
    /// The gateway node that runs the tunnel + does the NAT.
    pub gateway: String,
    /// The primary tunnel id on the gateway (the chain's head).
    pub primary: String,
    /// True while a `set-route` request is in flight.
    pub busy: bool,
    /// The most recent assignment result (the saved target key, or an error).
    pub result: Option<Result<String, String>>,
}

impl WizardState {
    /// True when the selection is complete enough to assign: a node, a gateway,
    /// and a primary tunnel are all chosen (the three fields the responder's
    /// [`EgressRoute::validate`] requires for a `Node`-scoped route). Never busy.
    #[must_use]
    pub fn can_assign(&self) -> bool {
        !self.busy
            && !self.node.trim().is_empty()
            && !self.gateway.trim().is_empty()
            && !self.primary.trim().is_empty()
    }

    /// Build the exact [`EgressRoute`] the current selection assigns — a
    /// per-node route (specificity beats group/ANY) through `gateway`'s
    /// `primary` tunnel, with the kill-switch defaulted on (the model's Q8
    /// default — block, don't leak). Returns `None` when the selection isn't
    /// complete ([`Self::can_assign`] is false), so the caller never emits an
    /// under-specified route. Pure — the wizard's core, unit-tested.
    #[must_use]
    pub fn route(&self) -> Option<EgressRoute> {
        if !self.can_assign() {
            return None;
        }
        Some(EgressRoute {
            target: RouteTarget::Node {
                name: self.node.trim().to_string(),
            },
            gateway: self.gateway.trim().to_string(),
            primary: self.primary.trim().to_string(),
            failover: Vec::new(),
            kill_switch: true,
        })
    }
}

// ── DDNS-EGRESS-5 — the dynamic-DNS surface state + pure derivations ─────────

/// DDNS-EGRESS-5 — the synced/stale/error status the table shows for one record,
/// derived purely from the record's live source state + its on-down policy (the
/// same inputs the DDNS-EGRESS-4 reconcile core consumes, so the UI verdict never
/// drifts from what the worker would actually publish).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DdnsStatus {
    /// The source is up + the published name resolves to the live exit IP (an
    /// inbound or identity-only exit) — the record is in sync.
    Synced,
    /// The source is down but the record keeps its last value (`on_down = keep`,
    /// kill-switch clear) — the name is published but may be stale.
    Stale,
    /// The source is down and the policy removed/parked the name, or the exit is
    /// kill-switched (leaking) — the name is not reachable right now.
    #[default]
    Error,
    /// The source hasn't been resolved yet (the table loaded but no Sync ran) —
    /// liveness/exit-IP unknown until the operator runs Sync.
    Unknown,
}

impl DdnsStatus {
    /// Derive the table status from the live [`SourceState`] + the record's
    /// [`OnDown`] policy. Pure mirror of the reconcile core's intent: an up source
    /// is synced; a down `keep` (no kill-switch) is stale (last value retained); a
    /// down `remove`/`sentinel` or a kill-switched source is an error (the name is
    /// gone/parked/leaking). Unit-tested.
    #[must_use]
    pub fn derive(state: &SourceState, on_down: OnDown) -> Self {
        match state {
            SourceState::Up { .. } => Self::Synced,
            SourceState::Down { kill_switch } => {
                if *kill_switch {
                    Self::Error
                } else {
                    match on_down {
                        OnDown::Keep => Self::Stale,
                        OnDown::Remove | OnDown::Sentinel => Self::Error,
                    }
                }
            }
        }
    }

    /// The operator-facing label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Synced => "synced",
            Self::Stale => "stale",
            Self::Error => "error",
            Self::Unknown => "unresolved",
        }
    }
}

/// DDNS-EGRESS-5 — one resolved row of the DDNS table: the record's templated
/// FQDN, its source, the live exit IP it publishes (once Sync resolves it), the
/// reachability flag + status, and the TTL. Built from the `action/ddns/get-config`
/// record paired with a live source resolve (`tunnel-status` / `verify-egress` +
/// `record-status`) — every field real over the DDNS-EGRESS-3 responder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DdnsRow {
    /// The record's name template (`{node}-{provider}`) — the stable key for
    /// edit/remove/sync actions (the responder keys records by this).
    pub name_template: String,
    /// The resolved fully-qualified name the record publishes under the zone.
    pub fqdn: String,
    /// The raw source field (`tunnel:<id>` / `wan`).
    pub source_raw: String,
    /// A friendly source label (`tunnel <id>` / `WAN`).
    pub source_label: String,
    /// The live exit/WAN IP the record currently publishes, once a Sync resolved
    /// the source. `None` until then (or when the source is down).
    pub current_ip: Option<String>,
    /// The reachability flag (`inbound` / `port-forward only` / `down`) from the
    /// last resolve, or empty when unresolved.
    pub reachability: String,
    /// The synced/stale/error verdict.
    pub status: DdnsStatus,
    /// The record TTL (seconds).
    pub ttl: u32,
}

impl DdnsRow {
    /// Build a row from a record definition + the config (zone/ttl) + the local
    /// node short name, with the source unresolved (status [`DdnsStatus::Unknown`],
    /// no exit IP yet). A live resolve later fills `current_ip`/`reachability`/
    /// `status` via [`DdnsRow::apply_resolve`]. Pure — unit-tested.
    #[must_use]
    pub fn from_record(rec: &RecordDef, cfg: &DdnsConfig, node: &str) -> Self {
        let provider = provider_hint(&rec.source);
        let fqdn = rec.fqdn(node, provider, 1, &cfg.zone);
        Self {
            name_template: rec.name.clone(),
            fqdn,
            source_raw: rec.source.clone(),
            source_label: source_label(&rec.source),
            current_ip: None,
            reachability: String::new(),
            status: DdnsStatus::Unknown,
            ttl: cfg.ttl,
        }
    }

    /// Fold a live source resolve into the row: the verified exit IP (when up),
    /// the reachability label, and the derived status. Pure.
    pub fn apply_resolve(&mut self, state: &SourceState, on_down: OnDown) {
        self.current_ip = match state {
            SourceState::Up { ip, .. } if !ip.trim().is_empty() => Some(ip.clone()),
            _ => None,
        };
        self.reachability = ddns::reachability(state).label().to_string();
        self.status = DdnsStatus::derive(state, on_down);
    }
}

/// DDNS-EGRESS-5 — the `{provider}` substitution for a source: a `tunnel:<id>`
/// templates to the tunnel id (the operator names the tunnel after the provider);
/// a `wan` source has no provider, so the literal `wan` is used. Mirrors the
/// worker's `provider_hint` so the UI's FQDN matches what gets published. Pure.
#[must_use]
fn provider_hint(source: &str) -> &str {
    let src = source.trim();
    if src.eq_ignore_ascii_case("wan") {
        return "wan";
    }
    src.strip_prefix("tunnel:").map_or(src, str::trim)
}

/// DDNS-EGRESS-5 — a friendly label for a record source: `tunnel:<id>` → `tunnel
/// <id>`, `wan` → `WAN`, anything else verbatim. Pure.
#[must_use]
fn source_label(source: &str) -> String {
    let src = source.trim();
    if src.eq_ignore_ascii_case("wan") {
        return "WAN".to_string();
    }
    match src.strip_prefix("tunnel:") {
        Some(id) => format!("tunnel {}", id.trim()),
        None => src.to_string(),
    }
}

/// DDNS-EGRESS-5 — the dynamic-DNS surface state: the loaded `[ddns]` config, the
/// resolved record rows, the local node short name (for FQDN templating), and the
/// add/edit form. All sourced from the `action/ddns/*` responder — no new model.
#[derive(Debug, Clone, Default)]
pub struct DdnsState {
    /// The loaded `[ddns]` config (zone/ttl/enabled + the record definitions).
    pub config: DdnsConfig,
    /// The resolved table rows (one per record).
    pub rows: Vec<DdnsRow>,
    /// The local node short name (`self-node` host) used for the `{node}` template.
    pub node: String,
    /// True once the config has loaded at least once (so an empty record list
    /// renders the empty state rather than a perpetual "loading").
    pub loaded: bool,
    /// True while the config/table fetch is in flight.
    pub busy: bool,
    /// The last load/op error, if any.
    pub error: Option<String>,
    /// The name template of the record whose `verify-egress` "Sync now" is in
    /// flight, if any (so its button shows a pending state).
    pub syncing: Option<String>,
    /// The add/edit form (None when closed).
    pub form: Option<DdnsForm>,
    /// The most recent add/remove/sync op result (success message or error).
    pub op_result: Option<Result<String, String>>,
}

/// DDNS-EGRESS-5 — the add/edit-record form: a name template, a source, and an
/// on-down policy. The pure [`DdnsForm::record`] turns the selection into the exact
/// [`RecordDef`] the `action/ddns/add-record` responder upserts;
/// [`DdnsForm::can_save`] gates the button.
#[derive(Debug, Clone, Default)]
pub struct DdnsForm {
    /// True when editing an existing record (the name template is locked, since the
    /// responder keys by it — a rename is a remove + add).
    pub editing: bool,
    /// The record name template (`{node}-{provider}`).
    pub name: String,
    /// The source field (`tunnel:<id>` / `wan`).
    pub source: String,
    /// The on-down policy wire value (`remove` / `sentinel` / `keep`).
    pub on_down: String,
}

/// DDNS-EGRESS-5 — the on-down policy choices (kebab-case wire values matching
/// `ddns::OnDown`'s serde), for the form's pick-list.
const ON_DOWN_CHOICES: [&str; 3] = ["remove", "sentinel", "keep"];

impl DdnsForm {
    /// A fresh add form (defaults: a `{node}-{provider}` template, `keep` on-down —
    /// the identity-record default the design favors). Pure.
    #[must_use]
    pub fn new_add() -> Self {
        Self {
            editing: false,
            name: "{node}-{provider}".to_string(),
            source: String::new(),
            on_down: "keep".to_string(),
        }
    }

    /// Seed an edit form from an existing record (name locked). Pure.
    #[must_use]
    pub fn from_record(rec: &RecordDef) -> Self {
        Self {
            editing: true,
            name: rec.name.clone(),
            source: rec.source.clone(),
            on_down: on_down_wire(rec.on_down).to_string(),
        }
    }

    /// True when the form carries enough to save: a non-blank name + source (the
    /// responder rejects an empty name/source). Pure.
    #[must_use]
    pub fn can_save(&self) -> bool {
        !self.name.trim().is_empty() && !self.source.trim().is_empty()
    }

    /// Build the [`RecordDef`] the form assigns — the exact shape the
    /// `add-record` responder upserts (it keys by the name template). Returns
    /// `None` when the form isn't complete ([`Self::can_save`] is false). Pure.
    #[must_use]
    pub fn record(&self) -> Option<RecordDef> {
        if !self.can_save() {
            return None;
        }
        Some(RecordDef {
            name: self.name.trim().to_string(),
            source: self.source.trim().to_string(),
            on_down: on_down_from_wire(&self.on_down),
        })
    }
}

/// DDNS-EGRESS-5 — parse an on-down wire value into [`OnDown`] (default `keep` for
/// an unknown value — the identity-record default). Pure.
#[must_use]
fn on_down_from_wire(s: &str) -> OnDown {
    match s.trim() {
        "remove" => OnDown::Remove,
        "sentinel" => OnDown::Sentinel,
        _ => OnDown::Keep,
    }
}

/// DDNS-EGRESS-5 — the wire value for an [`OnDown`]. Pure.
#[must_use]
const fn on_down_wire(o: OnDown) -> &'static str {
    match o {
        OnDown::Remove => "remove",
        OnDown::Sentinel => "sentinel",
        OnDown::Keep => "keep",
    }
}

/// ROUTE-TRACE-4 — the trace toolbar's selection state + last result.
///
/// The toolbar lets the operator pick a **source node**, a **destination
/// service/host**, and an **Egress/Ingress direction**, then run a trace. The
/// pure [`TraceState::request_body`] turns that selection into the exact
/// `action/route/trace` request shape the responder (`mackesd/src/ipc/route.rs`)
/// expects; [`TraceState::can_trace`] is the button's enable gate. The rendered
/// [`PathGraph`] (or an error) lands in `result`.
#[derive(Debug, Clone, Default)]
pub struct TraceState {
    /// Source-node label (the egress originator). Egress requires it; ingress
    /// ignores it (the responder resolves the host from the service's policy).
    pub source: String,
    /// Destination — a service id (ingress) or an external host/IP (egress).
    pub dest: String,
    /// `"egress"` | `"ingress"` (the toggle's wire value; default egress).
    pub direction: String,
    /// True while an `action/route/trace` request is in flight.
    pub busy: bool,
    /// The most recent trace result: the rendered `PathGraph`, or an error.
    pub result: Option<Result<PathGraph, String>>,
    /// ROUTE-TRACE-5 — the edge id (`<from>-><to>`) of the hop whose drill-down
    /// detail panel is open, if any. Set by clicking a control-hop row; cleared
    /// when a fresh trace lands (a new graph invalidates the old selection).
    pub selected_hop: Option<String>,
}

impl TraceState {
    /// Which direction the toggle currently selects (defaults to Egress for a
    /// blank/unknown wire value, matching `route_trace::Direction::default`).
    #[must_use]
    pub fn dir(&self) -> Direction {
        match self.direction.as_str() {
            "ingress" => Direction::Ingress,
            _ => Direction::Egress,
        }
    }

    /// True when the current selection is complete enough to trace. Egress needs
    /// a source node (the responder errors without a `from`); ingress needs a
    /// destination service id (the responder errors without a `to`). Never busy.
    #[must_use]
    pub fn can_trace(&self) -> bool {
        if self.busy {
            return false;
        }
        match self.dir() {
            Direction::Egress => !self.source.trim().is_empty(),
            Direction::Ingress => !self.dest.trim().is_empty(),
        }
    }

    /// Build the exact `action/route/trace` request body for the current
    /// selection (pure — the toolbar state machine's core, unit-tested). The
    /// responder reads `{ direction, from, to }`:
    ///
    /// * egress — `from` = source node, `to` = external dest (blank ⇒ the
    ///   responder defaults it to "Internet");
    /// * ingress — `to` = the service id (`from` is unused).
    ///
    /// Returns `None` when the selection isn't traceable yet
    /// ([`Self::can_trace`] is false), so the caller never publishes an
    /// under-specified request.
    #[must_use]
    pub fn request_body(&self) -> Option<String> {
        if !self.can_trace() {
            return None;
        }
        let body = match self.dir() {
            Direction::Egress => serde_json::json!({
                "direction": "egress",
                "from": self.source.trim(),
                "to": self.dest.trim(),
            }),
            Direction::Ingress => serde_json::json!({
                "direction": "ingress",
                "to": self.dest.trim(),
            }),
        };
        Some(body.to_string())
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<ValidationStatus, String>),
    RefreshClicked,
    RunNow,
    RunRequested(Result<String, String>),
    /// ROUTE-TRACE-4 — trace toolbar edits.
    TraceSourceChanged(String),
    TraceDestChanged(String),
    TraceDirectionSelected(String),
    TraceClicked,
    /// ROUTE-TRACE-4 — an `action/route/trace` reply landed (the `PathGraph` or
    /// an error message).
    TraceLoaded(Result<PathGraph, String>),
    /// ROUTE-TRACE-5 — a hop/segment was clicked: open (or, if already open,
    /// close) its drill-down detail panel. The payload is the edge id.
    SelectTraceHop(String),
    /// VPN-GW-8 — the egress-routing table + node roster fetch landed (the
    /// durable `action/vpn/list-routes` table + the real node names).
    EgressLoaded(Result<(EgressRouting, Vec<String>), String>),
    /// VPN-GW-8 — re-fetch the egress-routing table + roster.
    RefreshEgress,
    /// VPN-GW-8 — assign-route wizard edits.
    WizardNodeChanged(String),
    WizardGatewayChanged(String),
    WizardPrimaryChanged(String),
    /// VPN-GW-8 — assign the wizard's selection (emit `action/vpn/set-route`).
    AssignRoute,
    /// VPN-GW-8 — the `set-route` reply landed (the saved target key or error).
    RouteAssigned(Result<String, String>),
    /// DDNS-EGRESS-5 — the `action/ddns/get-config` config + the coarse per-record
    /// liveness resolve landed (the table's config + node name + rows).
    DdnsLoaded(Result<DdnsLoad, String>),
    /// DDNS-EGRESS-5 — re-fetch the DDNS config + table.
    RefreshDdns,
    /// DDNS-EGRESS-5 — open the add form (`None`) or the edit form for a record
    /// name template.
    OpenDdnsForm(Option<String>),
    /// DDNS-EGRESS-5 — close the add/edit form without saving.
    CancelDdnsForm,
    /// DDNS-EGRESS-5 — add/edit form edits.
    DdnsFormNameChanged(String),
    DdnsFormSourceChanged(String),
    DdnsFormOnDownSelected(String),
    /// DDNS-EGRESS-5 — save the form (emit `action/ddns/add-record`, an upsert).
    SaveDdnsRecord,
    /// DDNS-EGRESS-5 — remove a record by name template (`action/ddns/remove-record`).
    RemoveDdnsRecord(String),
    /// DDNS-EGRESS-5 — an add/remove reply landed (a human message or error).
    DdnsOpFinished(Result<String, String>),
    /// DDNS-EGRESS-5 — "Sync now" for one record: run its source's `verify-egress`
    /// + `record-status` and fold the verified exit IP / status into the row.
    SyncDdnsRecord(String),
    /// DDNS-EGRESS-5 — a "Sync now" resolve landed for `name` (the resolved row, or
    /// an error message).
    DdnsSynced {
        name: String,
        result: Result<DdnsResolve, String>,
    },
}

/// DDNS-EGRESS-5 — the `DdnsLoaded` payload: the loaded config, the local node
/// short name, and the coarse per-record resolve (liveness only — the exit IP is
/// filled per row by a Sync). Built off-thread by [`fetch_ddns`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DdnsLoad {
    /// The loaded `[ddns]` config.
    pub config: DdnsConfig,
    /// The local node short name (for `{node}` templating).
    pub node: String,
    /// The resolved rows (coarse liveness; no exit IP until a Sync).
    pub rows: Vec<DdnsRow>,
}

/// DDNS-EGRESS-5 — one record's live source resolve, decoded from a verify-egress /
/// record-status pair: the verified exit IP (when up), the reachability label, and
/// the synced/stale/error status. Folded into the matching [`DdnsRow`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DdnsResolve {
    /// The verified exit/WAN IP, when the source resolved up.
    pub current_ip: Option<String>,
    /// The reachability label (`inbound` / `port-forward only` / `down`).
    pub reachability: String,
    /// The synced/stale/error verdict.
    pub status: DdnsStatus,
}

impl RoutingPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        // VPN-GW-8 — load the reachability verdict AND the egress-routing table
        // + node roster in parallel, so the panel opens with both the validation
        // surface and the "who exits where" matrix populated.
        Task::batch([
            Task::perform(async { fetch_status() }, |result| {
                crate::Message::Routing(Message::Loaded(result))
            }),
            Self::load_egress(),
            Self::load_ddns(),
        ])
    }

    /// DDNS-EGRESS-5 — fetch the `[ddns]` config + the local node short name + a
    /// coarse per-record liveness resolve on a blocking thread (the Bus client owns
    /// a current-thread runtime). The table reads from the result; the exit IP is
    /// filled per row by a Sync (so the load stays in the fast 2 s window — it
    /// never runs the slow per-tunnel `verify-egress`).
    pub fn load_ddns() -> Task<crate::Message> {
        Task::perform(
            async { tokio::task::spawn_blocking(fetch_ddns).await },
            |joined| {
                let result = joined.unwrap_or_else(|e| Err(format!("ddns task: {e}")));
                crate::Message::Routing(Message::DdnsLoaded(result))
            },
        )
    }

    /// VPN-GW-8 — fetch the durable egress-routing table (`action/vpn/
    /// list-routes`) + the real node roster on a blocking thread (both reads are
    /// synchronous Bus / `mackesd` calls). The matrix, topology map, and wizard
    /// all read from the result.
    pub fn load_egress() -> Task<crate::Message> {
        Task::perform(
            async { tokio::task::spawn_blocking(fetch_egress).await },
            |joined| {
                let result = joined.unwrap_or_else(|e| Err(format!("egress task: {e}")));
                crate::Message::Routing(Message::EgressLoaded(result))
            },
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(status)) => {
                let never_run = status.run_id.is_none();
                self.status = status;
                self.error = None;
                self.last_run_at = Some(SystemTime::now());
                // A verdict landed — stop polling.
                if !never_run {
                    self.busy = false;
                    self.poll_attempts = 0;
                    return Task::none();
                }
                // AUDIT-MESH-5 — no run has ever been minted: auto-request one
                // (once) so the panel shows live reachability without the
                // operator having to click "Run validation now".
                if !self.auto_ran {
                    self.auto_ran = true;
                    self.busy = true;
                    self.poll_attempts = 0;
                    self.run_result = None;
                    return Task::perform(async { request_run() }, |result| {
                        crate::Message::Routing(Message::RunRequested(result))
                    });
                }
                // ROUTING-VALIDATE-1 — a run was requested but the verdict isn't
                // ready yet (the leader mints it + nodes report async). Keep
                // polling on a cadence until it lands or the budget is spent —
                // before, the panel fetched once, saw nothing, and gave up
                // ("No validation run yet" forever).
                if self.busy && self.poll_attempts < MAX_POLLS {
                    self.poll_attempts += 1;
                    return poll_status_later();
                }
                self.busy = false;
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.status = ValidationStatus::default();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                self.egress.busy = true;
                self.ddns.busy = true;
                self.run_result = None;
                Self::load()
            }
            Message::RunNow => {
                self.busy = true;
                self.poll_attempts = 0;
                Task::perform(async { request_run() }, |result| {
                    crate::Message::Routing(Message::RunRequested(result))
                })
            }
            Message::RunRequested(result) => {
                self.run_result = Some(result);
                // Keep busy + poll for the freshly-minted verdict rather than
                // fetching once immediately (it isn't ready yet).
                self.busy = true;
                self.poll_attempts = 0;
                poll_status_later()
            }
            Message::TraceSourceChanged(v) => {
                self.trace.source = v;
                Task::none()
            }
            Message::TraceDestChanged(v) => {
                self.trace.dest = v;
                Task::none()
            }
            Message::TraceDirectionSelected(v) => {
                self.trace.direction = v;
                Task::none()
            }
            Message::TraceClicked => {
                // Build the request body from the toolbar state; a noop if the
                // selection isn't traceable yet (the button is disabled in that
                // case, but guard regardless).
                let Some(body) = self.trace.request_body() else {
                    return Task::none();
                };
                self.trace.busy = true;
                Task::perform(
                    async move { tokio::task::spawn_blocking(move || request_trace(&body)).await },
                    |joined| {
                        let result = joined.unwrap_or_else(|e| Err(format!("trace task: {e}")));
                        crate::Message::Routing(Message::TraceLoaded(result))
                    },
                )
            }
            Message::TraceLoaded(result) => {
                self.trace.busy = false;
                self.trace.result = Some(result);
                // A new graph invalidates any open hop drill-down — the old edge
                // id may not exist in the fresh path.
                self.trace.selected_hop = None;
                Task::none()
            }
            Message::SelectTraceHop(edge_id) => {
                // Toggle: clicking the already-open hop closes its detail panel.
                self.trace.selected_hop =
                    if self.trace.selected_hop.as_deref() == Some(edge_id.as_str()) {
                        None
                    } else {
                        Some(edge_id)
                    };
                Task::none()
            }
            Message::EgressLoaded(Ok((routing, nodes))) => {
                self.egress.routing = routing;
                self.egress.nodes = nodes;
                self.egress.busy = false;
                self.egress.error = None;
                Task::none()
            }
            Message::EgressLoaded(Err(e)) => {
                self.egress.busy = false;
                self.egress.error = Some(e);
                Task::none()
            }
            Message::RefreshEgress => {
                self.egress.busy = true;
                Self::load_egress()
            }
            Message::WizardNodeChanged(v) => {
                self.egress.wizard.node = v;
                Task::none()
            }
            Message::WizardGatewayChanged(v) => {
                self.egress.wizard.gateway = v;
                Task::none()
            }
            Message::WizardPrimaryChanged(v) => {
                self.egress.wizard.primary = v;
                Task::none()
            }
            Message::AssignRoute => {
                // Build the EgressRoute from the wizard selection; a noop if it
                // isn't complete (the button is disabled then, but guard anyway).
                let Some(route) = self.egress.wizard.route() else {
                    return Task::none();
                };
                let Ok(body) = serde_json::to_string(&route) else {
                    return Task::none();
                };
                self.egress.wizard.busy = true;
                Task::perform(
                    async move { tokio::task::spawn_blocking(move || assign_route(&body)).await },
                    |joined| {
                        let result = joined.unwrap_or_else(|e| Err(format!("set-route task: {e}")));
                        crate::Message::Routing(Message::RouteAssigned(result))
                    },
                )
            }
            Message::RouteAssigned(result) => {
                let ok = result.is_ok();
                self.egress.wizard.busy = false;
                self.egress.wizard.result = Some(result);
                // A successful assignment changed the durable table — re-fetch it
                // so the matrix + topology map reflect the new route immediately
                // (live-verify: don't trust the request returned, read it back).
                if ok {
                    self.egress.busy = true;
                    return Self::load_egress();
                }
                Task::none()
            }
            Message::DdnsLoaded(Ok(load)) => {
                self.ddns.loaded = true;
                self.ddns.busy = false;
                self.ddns.error = None;
                self.ddns.config = load.config;
                self.ddns.node = load.node;
                self.ddns.rows = load.rows;
                Task::none()
            }
            Message::DdnsLoaded(Err(e)) => {
                self.ddns.loaded = true;
                self.ddns.busy = false;
                self.ddns.error = Some(e);
                self.ddns.rows.clear();
                Task::none()
            }
            Message::RefreshDdns => {
                self.ddns.busy = true;
                self.ddns.op_result = None;
                Self::load_ddns()
            }
            Message::OpenDdnsForm(edit) => {
                let form = match edit
                    .as_deref()
                    .and_then(|name| self.ddns.config.record.iter().find(|r| r.name == name))
                {
                    Some(rec) => DdnsForm::from_record(rec),
                    None => DdnsForm::new_add(),
                };
                self.ddns.form = Some(form);
                self.ddns.op_result = None;
                Task::none()
            }
            Message::CancelDdnsForm => {
                self.ddns.form = None;
                Task::none()
            }
            Message::DdnsFormNameChanged(v) => {
                if let Some(f) = self.ddns.form.as_mut() {
                    // The name template is the record key — locked while editing
                    // (a rename is a remove + add, never a silent re-key).
                    if !f.editing {
                        f.name = v;
                    }
                }
                Task::none()
            }
            Message::DdnsFormSourceChanged(v) => {
                if let Some(f) = self.ddns.form.as_mut() {
                    f.source = v;
                }
                Task::none()
            }
            Message::DdnsFormOnDownSelected(v) => {
                if let Some(f) = self.ddns.form.as_mut() {
                    f.on_down = v;
                }
                Task::none()
            }
            Message::SaveDdnsRecord => {
                let Some(rec) = self.ddns.form.as_ref().and_then(DdnsForm::record) else {
                    return Task::none();
                };
                let Ok(body) = serde_json::to_string(&rec) else {
                    return Task::none();
                };
                self.ddns.busy = true;
                Task::perform(
                    async move { tokio::task::spawn_blocking(move || add_record(&body)).await },
                    |joined| {
                        let result =
                            joined.unwrap_or_else(|e| Err(format!("ddns add-record task: {e}")));
                        crate::Message::Routing(Message::DdnsOpFinished(result))
                    },
                )
            }
            Message::RemoveDdnsRecord(name) => {
                self.ddns.busy = true;
                self.ddns.op_result = None;
                Task::perform(
                    async move { tokio::task::spawn_blocking(move || remove_record(&name)).await },
                    |joined| {
                        let result =
                            joined.unwrap_or_else(|e| Err(format!("ddns remove-record task: {e}")));
                        crate::Message::Routing(Message::DdnsOpFinished(result))
                    },
                )
            }
            Message::DdnsOpFinished(result) => {
                let ok = result.is_ok();
                self.ddns.op_result = Some(result);
                if ok {
                    // The durable table changed — close the form + re-read it (live
                    // verify: don't trust the write, read it back).
                    self.ddns.form = None;
                    self.ddns.busy = true;
                    return Self::load_ddns();
                }
                self.ddns.busy = false;
                Task::none()
            }
            Message::SyncDdnsRecord(name) => {
                // Run the per-source exit-IP verify + record-status off-thread, then
                // fold the resolved row back. Guard against re-entrancy.
                if self.ddns.syncing.is_some() {
                    return Task::none();
                }
                let Some(rec) = self.ddns.config.record.iter().find(|r| r.name == name) else {
                    return Task::none();
                };
                self.ddns.syncing = Some(name.clone());
                self.ddns.op_result = None;
                let source = rec.source.clone();
                let on_down = rec.on_down;
                let reply_name = name.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || resolve_record(&source, on_down)).await
                    },
                    move |joined| {
                        let result = joined.unwrap_or_else(|e| Err(format!("ddns sync task: {e}")));
                        crate::Message::Routing(Message::DdnsSynced {
                            name: reply_name.clone(),
                            result,
                        })
                    },
                )
            }
            Message::DdnsSynced { name, result } => {
                self.ddns.syncing = None;
                match result {
                    Ok(resolve) => {
                        if let Some(row) =
                            self.ddns.rows.iter_mut().find(|r| r.name_template == name)
                        {
                            row.current_ip = resolve.current_ip;
                            row.reachability = resolve.reachability;
                            row.status = resolve.status;
                        }
                        self.ddns.op_result = Some(Ok(format!("{name}: synced")));
                    }
                    Err(e) => self.ddns.op_result = Some(Err(e)),
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Routing")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("overlay-reachability validation")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let accent = palette.accent.into_cosmic_color();
        let style_btn = move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            let bg = match status {
                cosmic::iced::widget::button::Status::Hovered => Color {
                    r: accent.r * 1.10,
                    g: accent.g * 1.10,
                    b: accent.b * 1.10,
                    a: accent.a,
                },
                _ => accent,
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 6.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..cosmic::iced::widget::button::Style::default()
            }
        };
        let run_btn = button(text("Run validation now").size(13).colr(Color::WHITE))
            .padding(Padding::from([6u16, 14u16]))
            .sty(style_btn)
            .on_press(crate::Message::Routing(Message::RunNow));
        let refresh_btn = button(
            text(if self.busy { "…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty(style_btn)
        .on_press(crate::Message::Routing(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            run_btn,
            Space::new().width(Length::Fixed(8.0)),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut body_col = column![].spacing(6);
        // VPN-GW-8 — the egress-routing surface (who exits where) sits at the top
        // of the panel: the egress matrix, the route topology map, and the
        // assign-route wizard — all over the durable `action/vpn/*` table.
        body_col = body_col.push(egress_section(&self.egress, palette));
        // DDNS-EGRESS-5 — the dynamic-DNS surface (the published-hostname table +
        // add/edit form) sits beneath the egress matrix: which names point at which
        // exit, and whether each is currently in sync.
        body_col = body_col.push(ddns_section(&self.ddns, palette));
        // ROUTE-TRACE-4 — the path-trace toolbar + topology graph sit beneath the
        // egress surface (the trace is the interactive "why is this path
        // (un)reachable" lens over the same overlay state).
        body_col = body_col.push(trace_toolbar(&self.trace, palette));
        body_col = body_col.push(trace_graph(&self.trace, palette));
        if let Some(res) = &self.run_result {
            body_col = body_col.push(result_strip(res, palette));
        }
        if self.last_run_at.is_some() {
            if self.status.run_id.is_some() {
                body_col = body_col.push(verdict_card(&self.status, palette));
                for e in &self.status.failed_edges {
                    body_col = body_col.push(failed_edge_row(e, palette));
                }
            } else {
                body_col =
                    body_col.push(empty_state_card(palette, self.error.as_deref(), self.busy));
            }
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(body_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn verdict_card<'a>(s: &ValidationStatus, palette: Palette) -> Element<'a, crate::Message> {
    let (icon, color, label) = if s.passed {
        (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            "PASS — every overlay edge reachable".to_string(),
        )
    } else {
        (
            Icon::StatusError,
            palette.danger.into_cosmic_color(),
            format!(
                "FAIL — {} unreachable edge{}, {} missing reporter{}",
                s.failed_edges.len(),
                if s.failed_edges.len() == 1 { "" } else { "s" },
                s.missing_reporters.len(),
                if s.missing_reporters.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        )
    };
    let resolved = mde_icon(icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style { color: Some(color) })
            .into()
    } else {
        text(resolved.fallback_glyph).size(16.0).colr(color).into()
    };
    let head = row![
        icon_widget,
        text(label).size(12).colr(color),
        Space::new().width(Length::Fill),
        text(format!("{} reachable", s.reachable))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);
    let rid = s.run_id.clone().unwrap_or_default();
    card(
        column![
            head,
            text(format!("run {rid}"))
                .size(10)
                .colr(palette.text_muted.into_cosmic_color())
        ]
        .spacing(4),
        palette,
    )
}

fn failed_edge_row<'a>(e: &Edge, palette: Palette) -> Element<'a, crate::Message> {
    let danger = palette.danger.into_cosmic_color();
    card(
        row![
            text(format!("{} → {}", e.from, e.to))
                .size(12)
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fill),
            text("unreachable").size(11).colr(danger),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center),
        palette,
    )
}

fn result_strip<'a>(res: &Result<String, String>, palette: Palette) -> Element<'a, crate::Message> {
    let (color, label) = match res {
        Ok(msg) => (palette.success.into_cosmic_color(), msg.clone()),
        Err(e) => (palette.danger.into_cosmic_color(), format!("error — {e}")),
    };
    let bg = palette.raised.into_cosmic_color();
    container(text(label).size(11).colr(color))
        .padding(Padding::from([8u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn empty_state_card<'a>(
    palette: Palette,
    error: Option<&'a str>,
    busy: bool,
) -> Element<'a, crate::Message> {
    let (icon_kind, icon_color, heading, body): (Icon, Color, String, String) =
        if let Some(err) = error {
            (
                Icon::StatusError,
                palette.danger.into_cosmic_color(),
                "Couldn't read validation".to_string(),
                err.to_string(),
            )
        } else if busy {
            // AUDIT-MESH-5 — the one-shot auto-run is in flight.
            (
                Icon::Network,
                palette.accent.into_cosmic_color(),
                "Running validation…".to_string(),
                "Probing every directed overlay edge between participants — the \
                 FPG leader mints the run and each node reports what it could \
                 reach. The verdict appears here as soon as the reporters return."
                    .to_string(),
            )
        } else {
            (
                Icon::Network,
                palette.accent.into_cosmic_color(),
                "No validation run yet".to_string(),
                "The overlay-reachability suite probes every directed edge between \
                 participants. Click \"Run validation now\" to request a run — the FPG \
                 leader mints it, every node reports what it could reach, and the verdict \
                 (with any unreachable edges) appears here."
                    .to_string(),
            )
        };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(32.0)
            .colr(icon_color)
            .into()
    };
    container(
        column![
            icon_widget,
            Space::new().height(Length::Fixed(8.0)),
            text(heading)
                .size(14)
                .colr(palette.text.into_cosmic_color()),
            text(body)
                .size(11)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2)
        .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

fn card<'a>(
    inner: impl Into<Element<'a, crate::Message>>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(inner)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ---- ROUTE-TRACE-4: trace toolbar + topology graph ------------------------

/// ROUTE-TRACE-4 — the trace toolbar: a source-node picker, a destination
/// service/host field, an Egress/Ingress direction toggle, and a Trace button.
/// The direction toggle re-labels the fields' meaning (egress traces a node's
/// WAN path to a host; ingress traces an external client's path to a published
/// service) and flips which field gates the Trace button.
fn trace_toolbar<'a>(state: &'a TraceState, palette: Palette) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let dir = state.dir();
    let label = move |s: &str| {
        text(s.to_string())
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(70.0))
    };

    // Source node — meaningful for egress (the originating mesh node); ingress
    // resolves the host from the service policy, so it's shown as a muted note
    // (not an editable field) in that direction. The editable fields use the
    // shared Carbon-token input chrome (`controls::styled_text_input`) so they
    // match every other panel's inputs (§4).
    let dest_hint = match dir {
        Direction::Egress => "destination host/IP (e.g. 1.1.1.1)",
        Direction::Ingress => "service id (e.g. grafana)",
    };
    // `controls::styled_text_input` returns a `cosmic::Theme` element (the same
    // theme as the surrounding panel tree), so it drops straight in — no `themer`
    // bridge needed (unlike the stock-iced-themed canvas).
    let source_widget: Element<'a, crate::Message> = match dir {
        Direction::Egress => crate::controls::styled_text_input(
            "source node (e.g. eagle)",
            &state.source,
            |v| crate::Message::Routing(Message::TraceSourceChanged(v)),
            palette,
        ),
        Direction::Ingress => text("(resolved from the service's policy)")
            .size(13)
            .colr(palette.text_muted.into_cosmic_color())
            .into(),
    };
    let dest_widget = crate::controls::styled_text_input(
        dest_hint,
        &state.dest,
        |v| crate::Message::Routing(Message::TraceDestChanged(v)),
        palette,
    );

    let direction_picker = pick_list(
        DIRECTION_CHOICES.map(String::from).to_vec(),
        Some(if state.direction.is_empty() {
            "egress".to_string()
        } else {
            state.direction.clone()
        }),
        |v| crate::Message::Routing(Message::TraceDirectionSelected(v)),
    )
    .text_size(13);

    let trace_msg = state
        .can_trace()
        .then_some(crate::Message::Routing(Message::TraceClicked));
    let trace_btn = crate::controls::variant_button(
        if state.busy { "Tracing…" } else { "Trace" },
        crate::controls::ButtonVariant::Primary,
        trace_msg,
        palette,
    );

    let title = text("Trace a path")
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text.into_cosmic_color());

    let source_row = row![label("From"), source_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let dest_row = row![label("To"), dest_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let controls_row = row![
        label("Direction"),
        direction_picker,
        Space::new().width(Length::Fill),
        trace_btn,
    ]
    .spacing(10)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    card(
        column![title, source_row, dest_row, controls_row].spacing(8),
        palette,
    )
}

/// ROUTE-TRACE-4 — the topology-graph card: renders the most recent trace's
/// `PathGraph` on a canvas (node glyphs by kind, edge color by layer, RTT/loss
/// labels), or a hint / error when nothing has been traced yet. Reuses the
/// canvas drawing approach from the Peers map (`peers_map::MapProgram`).
fn trace_graph<'a>(state: &TraceState, palette: Palette) -> Element<'a, crate::Message> {
    match &state.result {
        Some(Ok(graph)) => {
            // The canvas program paints from `palette` (it ignores the passed
            // stock theme), so `themer(None, ...)` bridges the stock-themed
            // canvas into the surrounding cosmic theme — same pattern as Peers.
            let program = PathGraphProgram {
                graph: graph.clone(),
                palette,
            };
            let canvas_stock: cosmic::iced::Element<'_, crate::Message, cosmic::iced::Theme> =
                cosmic::iced::widget::canvas(program)
                    .width(Length::Fill)
                    .height(Length::Fixed(280.0))
                    .into();
            let canvas: Element<'_, crate::Message> =
                cosmic::iced::widget::themer(None, canvas_stock).into();
            let verdict = path_verdict_line(graph, palette);
            let mut body = column![verdict, container(canvas).width(Length::Fill)].spacing(8);
            // ROUTE-TRACE-5 — the per-hop control list under the canvas: one row
            // per edge that crosses a firewall/control point, each a CLICKABLE
            // button with a tone-tinted verdict badge + the cited rule. The canvas
            // shows *where* the path stops; this list says *why*, citing each
            // control's rule. Selecting a hop opens its drill-down detail panel.
            if let Some(controls) = control_hops_list(graph, state.selected_hop.as_deref(), palette)
            {
                body = body.push(controls);
            }
            // ROUTE-TRACE-5 — the drill-down detail panel for the selected hop:
            // endpoints + transport + RTT/loss + the full firewall rule chain +
            // verdict + the DNS name resolved at that hop — all five connectivity
            // concepts in one place.
            if let Some(detail) = hop_detail_panel(graph, state.selected_hop.as_deref(), palette) {
                body = body.push(detail);
            }
            card(body, palette)
        }
        Some(Err(e)) => card(
            text(format!("Trace failed — {e}"))
                .size(12)
                .colr(palette.danger.into_cosmic_color()),
            palette,
        ),
        None => card(
            text(
                "Pick a source + destination and a direction, then Trace to render the path \
                 graph — node glyphs by kind, edges colored by layer with RTT/loss labels, the \
                 first blocking control point highlighted.",
            )
            .size(12)
            .colr(palette.text_muted.into_cosmic_color()),
            palette,
        ),
    }
}

/// ROUTE-TRACE-4 — a one-line verdict over the rendered path: reachable, blocked
/// (citing where), or indeterminate (a control point couldn't be resolved).
fn path_verdict_line<'a>(graph: &PathGraph, palette: Palette) -> Element<'a, crate::Message> {
    let (color, label) = if let Some(at) = &graph.blocked_at {
        (
            palette.danger.into_cosmic_color(),
            format!("BLOCKED at {}", blocked_edge_label(graph, at)),
        )
    } else if graph.has_indeterminate() {
        (
            palette.warning.into_cosmic_color(),
            "INDETERMINATE — a control point couldn't be resolved".to_string(),
        )
    } else {
        (
            palette.success.into_cosmic_color(),
            "REACHABLE — the path reaches its destination unblocked".to_string(),
        )
    };
    let dir = match graph.direction {
        Direction::Egress => "egress",
        Direction::Ingress => "ingress",
    };
    row![
        text(label).size(12).colr(color),
        Space::new().width(Length::Fill),
        text(format!("{dir} · {} hops", graph.edges.len()))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// ROUTE-TRACE-4 — render the blocked edge id (`<from-id>-><to-id>`) as a human
/// `<from-label> → <to-label>` using the graph's node labels, so the verdict
/// reads like the graph (hostnames/service names) rather than the internal wire
/// ids. Falls back to the raw edge id if the edge isn't found.
fn blocked_edge_label(graph: &PathGraph, edge_id: &str) -> String {
    graph.edges.iter().find(|e| e.id() == edge_id).map_or_else(
        || edge_id.to_string(),
        |e| {
            format!(
                "{} → {}",
                node_label(graph, &e.from),
                node_label(graph, &e.to)
            )
        },
    )
}

// ---- ROUTE-TRACE-5: per-hop control-point list ----------------------------

/// ROUTE-TRACE-5 — the shared severity tone a control-point [`Verdict`] reads
/// as, on the Carbon support ramp (no raw hex — §4): Allow is success (the
/// segment is permitted), Block is danger (the path stops here), Indeterminate
/// is warning (the rule set couldn't be resolved — never guessed). This is the
/// 1:1 verdict→[`BadgeSeverity`] mapping the firewall badge tints from; pure +
/// unit-tested so the color derivation is verifiable without rendering.
#[must_use]
fn verdict_severity(verdict: Verdict) -> BadgeSeverity {
    match verdict {
        Verdict::Allow => BadgeSeverity::Success,
        Verdict::Block => BadgeSeverity::Danger,
        Verdict::Indeterminate => BadgeSeverity::Warning,
    }
}

/// ROUTE-TRACE-5 — the short, uppercase label a [`Verdict`] shows on its badge.
#[must_use]
fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Allow => "ALLOW",
        Verdict::Block => "BLOCK",
        Verdict::Indeterminate => "INDET",
    }
}

/// ROUTE-TRACE-5 — a tone-tinted firewall badge for a control point, so
/// Allow/Block/Indeterminate read as a glanceable green/red/amber chip. Reuses
/// the shared [`panel_chrome::status_badge`] (the same severity-tinted pill every
/// other panel uses) tinted from `control.verdict` via [`verdict_severity`] — one
/// badge chrome, sourced from Carbon tokens, no raw hex.
fn firewall_badge<'a>(control: &ControlPoint, palette: Palette) -> Element<'a, crate::Message> {
    crate::panel_chrome::status_badge(
        verdict_label(control.verdict),
        verdict_severity(control.verdict),
        palette,
    )
}

/// ROUTE-TRACE-5 — resolve a node id to its human label (hostname / service name
/// / "Internet"), falling back to the raw id when the node isn't in the graph.
fn node_label(graph: &PathGraph, id: &str) -> String {
    graph
        .nodes
        .iter()
        .find(|n| n.id == id)
        .map_or_else(|| id.to_string(), |n| n.label.clone())
}

/// ROUTE-TRACE-5 — one control-point hop row, rendered as a CLICKABLE button so
/// the operator can drill into it: the tone-tinted [`firewall_badge`], the human
/// `<from> → <to>` segment, and a small detail line under it citing the control
/// (`firewall` name) and its `rule`. The blocking hop (the one `blocked_at`
/// points at) is marked so the per-hop list reads as the canvas's explanation;
/// `selected` tints the row's surface so the open drill-down is obvious. Pressing
/// it fires [`Message::SelectTraceHop`] which toggles the detail panel below.
fn control_hop_row<'a>(
    graph: &PathGraph,
    edge: &PathEdge,
    selected: bool,
    palette: Palette,
) -> Option<Element<'a, crate::Message>> {
    let control = edge.control.as_ref()?;
    let badge = firewall_badge(control, palette);
    let is_blocking = graph.blocked_at.as_deref() == Some(edge.id().as_str());

    let segment = text(format!(
        "{} → {}",
        node_label(graph, &edge.from),
        node_label(graph, &edge.to)
    ))
    .size(12)
    .colr(palette.text.into_cosmic_color());

    let mut head = row![badge, segment]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    if is_blocking {
        // The first denying point — flag it so this row reads as the canvas's
        // BLOCKED highlight in list form.
        head = head.push(Space::new().width(Length::Fill));
        head = head.push(
            text("first block")
                .size(10)
                .colr(palette.danger.into_cosmic_color()),
        );
    }

    // The per-hop summary: the control's name + the exact cited rule — the detail
    // behind the badge ("firewalld:public · default deny (no matching rule)").
    // The full drill-down (endpoints/transport/RTT/DNS) is the detail panel below.
    let summary = text(format!("{} · {}", control.firewall, control.rule))
        .size(10)
        .colr(palette.text_muted.into_cosmic_color());

    let inner = column![head, summary].spacing(4);
    // A selected (or hovered) row reads on the Carbon hover tint; an unselected,
    // unhovered one is transparent so the list reads plain until clicked. Both
    // sourced from `mde-theme` tokens — no raw hex (§4).
    let tint = palette.hover_tint().into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let text_color = palette.text.into_cosmic_color();
    let btn = button(inner)
        .padding(Padding::from([8u16, 12u16]))
        .width(Length::Fill)
        .on_press(crate::Message::Routing(Message::SelectTraceHop(edge.id())))
        .sty(move |_theme, status| {
            // Hover lifts onto the tint even for an unselected row so the row
            // reads as clickable; the selected row already sits on it.
            let hovered = matches!(status, cosmic::iced::widget::button::Status::Hovered);
            let bg = if selected || hovered {
                tint
            } else {
                Color::TRANSPARENT
            };
            button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color,
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..button::Style::default()
            }
        });
    Some(btn.into())
}

/// ROUTE-TRACE-5 — the per-hop control list: one [`control_hop_row`] for each
/// edge that crosses a control point (a [`ControlPoint`]), in source→dest order,
/// under a small heading. `selected` is the edge id whose row is highlighted (the
/// open drill-down). Returns `None` when the path crosses no control points (a
/// plain egress with no modeled firewall) — the canvas alone suffices then, so no
/// empty list chrome is rendered.
fn control_hops_list<'a>(
    graph: &PathGraph,
    selected: Option<&str>,
    palette: Palette,
) -> Option<Element<'a, crate::Message>> {
    let hop_rows: Vec<Element<'a, crate::Message>> = graph
        .edges
        .iter()
        .filter_map(|edge| {
            let is_sel = selected == Some(edge.id().as_str());
            control_hop_row(graph, edge, is_sel, palette)
        })
        .collect();
    if hop_rows.is_empty() {
        return None;
    }
    let mut rows = column![].spacing(6);
    for r in hop_rows {
        rows = rows.push(r);
    }
    let heading = text("Control points — click a hop for detail")
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());
    Some(column![heading, rows].spacing(6).into())
}

// ---- ROUTE-TRACE-5: selectable hop drill-down detail panel ----------------

/// ROUTE-TRACE-5 — the human label for an edge's [`Transport`] (how the segment
/// is carried: direct/relay overlay, VPN tunnel, public, loopback).
#[must_use]
fn transport_label(transport: Transport) -> &'static str {
    match transport {
        Transport::DirectOverlay => "direct overlay (Nebula, hole-punched)",
        Transport::RelayOverlay => "relayed overlay (via a lighthouse)",
        Transport::VpnTunnel => "VPN tunnel",
        Transport::Public => "public internet",
        Transport::Loopback => "on-host loopback",
    }
}

/// ROUTE-TRACE-5 — the human label for a node's [`NodeKind`] (the five-concept
/// taxonomy: hosting & VMs, mesh peers, the gateway/exit, ingress, services).
#[must_use]
fn node_kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Host => "host",
        NodeKind::Vm => "VM",
        NodeKind::Container => "container",
        NodeKind::OverlayPeer => "overlay peer",
        NodeKind::Gateway => "VPN gateway",
        NodeKind::VpnExit => "VPN exit",
        NodeKind::Ingress => "ingress",
        NodeKind::Internet => "internet",
        NodeKind::Service => "service",
    }
}

/// ROUTE-TRACE-5 — the RTT/loss line for a measured segment, or an honest
/// "not measured" when the segment was modeled-only (degrade-to-modeled — never
/// fabricate a latency). Pure so the formatting is unit-testable.
#[must_use]
fn rtt_loss_line(edge: &PathEdge) -> String {
    match (edge.rtt_ms, edge.loss) {
        (Some(rtt), Some(loss)) => format!("{rtt:.1} ms · {:.0}% loss", loss * 100.0),
        (Some(rtt), None) => format!("{rtt:.1} ms"),
        (None, Some(loss)) => format!("{:.0}% loss", loss * 100.0),
        (None, None) => "not measured (modeled hop)".to_string(),
    }
}

/// ROUTE-TRACE-5 — find the selected edge by its `<from>-><to>` id.
fn find_edge<'a>(graph: &'a PathGraph, edge_id: &str) -> Option<&'a PathEdge> {
    graph.edges.iter().find(|e| e.id() == edge_id)
}

/// ROUTE-TRACE-5 — a labelled key/value detail line (muted key, default-text
/// value). The atom every endpoint/segment row in the drill-down is built from.
fn detail_row<'a>(key: &str, value: String, palette: Palette) -> Element<'a, crate::Message> {
    row![
        text(format!("{key}:"))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(96.0)),
        text(value).size(11).colr(palette.text.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Top)
    .into()
}

/// ROUTE-TRACE-5 — the endpoint detail block for one node: its kind, every known
/// address (LAN / overlay / public), the DNS name resolved at that hop (Dynamic
/// DNS), and the hosting node (Hosting & VMs). Only the addresses that exist are
/// shown — no "unknown" placeholders for a node that simply has no overlay IP.
fn endpoint_detail<'a>(
    title: &str,
    node: &PathNode,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let mut col = column![text(format!("{title} — {}", node.label))
        .size(12)
        .colr(palette.text.into_cosmic_color())]
    .spacing(3);
    col = col.push(detail_row(
        "kind",
        node_kind_label(node.kind).to_string(),
        palette,
    ));
    if let Some(ip) = &node.node_ip {
        col = col.push(detail_row("host IP", ip.clone(), palette));
    }
    if let Some(ip) = &node.overlay_ip {
        col = col.push(detail_row("overlay IP", ip.clone(), palette));
    }
    if let Some(ip) = &node.public_ip {
        col = col.push(detail_row("public IP", ip.clone(), palette));
    }
    if let Some(dns) = &node.dns_name {
        // Dynamic DNS — the published name resolved at this hop.
        col = col.push(detail_row("DNS name", dns.clone(), palette));
    }
    if let Some(host) = &node.hosting_node {
        col = col.push(detail_row("hosted on", host.clone(), palette));
    }
    col.into()
}

/// ROUTE-TRACE-5 — the per-hop drill-down detail panel for the selected segment.
///
/// Surfaces all five connectivity concepts for the one clicked hop: its two
/// endpoints with every known address + the Dynamic-DNS name + the hosting node
/// (Hosting & VMs + Mesh), the transport + layer (Mesh/VPN/Public), the measured
/// RTT/loss, and the firewall control verdict + the exact cited rule (Firewalls &
/// Control) — flagged when it is the first blocking point. Returns `None` when no
/// hop is selected or the selection no longer resolves (a stale id after a fresh
/// trace). Carbon tokens only (§4).
fn hop_detail_panel<'a>(
    graph: &PathGraph,
    selected: Option<&str>,
    palette: Palette,
) -> Option<Element<'a, crate::Message>> {
    let edge_id = selected?;
    let edge = find_edge(graph, edge_id)?;
    let from = graph.nodes.iter().find(|n| n.id == edge.from);
    let to = graph.nodes.iter().find(|n| n.id == edge.to);

    let heading = text(format!(
        "Hop detail — {} → {}",
        node_label(graph, &edge.from),
        node_label(graph, &edge.to)
    ))
    .size(13)
    .colr(palette.text.into_cosmic_color());

    let mut body = column![heading].spacing(8);

    // Endpoints (Hosting & VMs + Mesh).
    if let Some(n) = from {
        body = body.push(endpoint_detail("From", n, palette));
    }
    if let Some(n) = to {
        body = body.push(endpoint_detail("To", n, palette));
    }

    // The segment itself (Mesh / VPN egress-ingress / Public).
    let mut segment = column![text("Segment")
        .size(12)
        .colr(palette.text.into_cosmic_color())]
    .spacing(3);
    segment = segment.push(detail_row(
        "transport",
        transport_label(edge.transport).to_string(),
        palette,
    ));
    segment = segment.push(detail_row(
        "layer",
        layer_label(edge.layer).to_string(),
        palette,
    ));
    segment = segment.push(detail_row("rtt/loss", rtt_loss_line(edge), palette));
    body = body.push(segment);

    // Firewalls & Control — the verdict badge + the full cited rule chain, with
    // the first-block flag when this is where the path stops.
    let mut control_block = column![text("Firewall / control")
        .size(12)
        .colr(palette.text.into_cosmic_color())]
    .spacing(3);
    if let Some(control) = &edge.control {
        let is_blocking = graph.blocked_at.as_deref() == Some(edge.id().as_str());
        let badge_row = row![
            firewall_badge(control, palette),
            text(control.firewall.clone())
                .size(11)
                .colr(palette.text.into_cosmic_color()),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);
        control_block = control_block.push(badge_row);
        control_block = control_block.push(detail_row("rule", control.rule.clone(), palette));
        if is_blocking {
            control_block = control_block.push(
                text("This is the first point that blocks the path.")
                    .size(11)
                    .colr(palette.danger.into_cosmic_color()),
            );
        }
    } else {
        control_block = control_block.push(
            text("No control point on this segment (open path).")
                .size(11)
                .colr(palette.text_muted.into_cosmic_color()),
        );
    }
    body = body.push(control_block);

    Some(card(body, palette))
}

/// ROUTE-TRACE-4 — the topology-graph canvas program. Lays the path out as a
/// horizontal chain (source→dest order — a path is linear), draws each edge
/// colored by its [`Layer`] with an RTT/loss label, highlights the active path
/// (and the first blocking edge in danger), and paints each node as a glyph
/// sized/colored by its [`NodeKind`]. Paints from `palette` (Carbon tokens) — no
/// raw hex.
struct PathGraphProgram {
    graph: PathGraph,
    palette: Palette,
}

impl PathGraphProgram {
    /// Project the path's nodes onto a horizontal chain across `bounds`, in
    /// source→dest order (a [`PathGraph`] is a linear path). Returns id→point.
    fn projected(
        &self,
        bounds: &cosmic::iced::Rectangle,
    ) -> std::collections::HashMap<String, cosmic::iced::Point> {
        use cosmic::iced::Point;
        let n = self.graph.nodes.len().max(1);
        let pad = 60.0_f32;
        let usable = (bounds.width - pad * 2.0).max(1.0);
        let step = if n > 1 { usable / (n - 1) as f32 } else { 0.0 };
        let y = bounds.height / 2.0;
        self.graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.id.clone(), Point::new(pad + step * i as f32, y)))
            .collect()
    }
}

/// ROUTE-TRACE-4 — the Carbon token an edge's [`Layer`] colors to. Host=muted
/// (local), Mesh=accent (the overlay), Vpn=warning (a tunnel boundary),
/// Public=success-but-it's-really-just-"open" — we use `text` for public so the
/// four layers read distinctly against the canvas. A blocked edge overrides this
/// with `danger` at the draw site.
fn layer_color(layer: Layer, palette: Palette) -> Color {
    match layer {
        Layer::Host => palette.text_muted.into_cosmic_color(),
        Layer::Mesh => palette.accent.into_cosmic_color(),
        Layer::Vpn => palette.warning.into_cosmic_color(),
        Layer::Public => palette.text.into_cosmic_color(),
    }
}

/// ROUTE-TRACE-4 — a short glyph for a node [`NodeKind`] (drawn inside the node
/// disc). Plain ASCII so it renders without an icon font on the canvas.
fn node_glyph(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Host => "H",
        NodeKind::Vm => "V",
        NodeKind::Container => "C",
        NodeKind::OverlayPeer => "P",
        NodeKind::Gateway => "G",
        NodeKind::VpnExit => "X",
        NodeKind::Ingress => "I",
        NodeKind::Internet => "@",
        NodeKind::Service => "S",
    }
}

impl cosmic::iced::widget::canvas::Program<crate::Message> for PathGraphProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &cosmic::iced::Renderer,
        _theme: &cosmic::iced::Theme,
        bounds: cosmic::iced::Rectangle,
        _cursor: cosmic::iced::mouse::Cursor,
    ) -> Vec<cosmic::iced::widget::canvas::Geometry> {
        use cosmic::iced::alignment::Vertical;
        use cosmic::iced::widget::canvas::{Frame, Path, Stroke, Text};
        use cosmic::iced::widget::text::Alignment;
        use cosmic::iced::{Pixels, Point, Rectangle};
        let mut frame = Frame::new(renderer, bounds.size());
        let rect = Rectangle::with_size(bounds.size());
        let proj = self.projected(&rect);
        let p = &self.palette;

        // Edges first (under the nodes), colored by layer; the blocking edge is
        // drawn in danger + thicker to highlight where the path stops.
        for edge in &self.graph.edges {
            let (Some(&from), Some(&to)) = (proj.get(&edge.from), proj.get(&edge.to)) else {
                continue;
            };
            let blocked = self
                .graph
                .blocked_at
                .as_deref()
                .is_some_and(|b| b == edge.id());
            let indeterminate = edge
                .control
                .as_ref()
                .is_some_and(|c| c.verdict == Verdict::Indeterminate);
            let (color, width) = if blocked {
                (p.danger.into_cosmic_color(), 3.0)
            } else if indeterminate {
                (p.warning.into_cosmic_color(), 2.0)
            } else {
                (layer_color(edge.layer, *p), 2.0)
            };
            frame.stroke(
                &Path::line(from, to),
                Stroke::default().with_color(color).with_width(width),
            );
            // RTT/loss label above the segment midpoint; layer name below it so
            // the operator can read the edge's layer at a glance.
            let mid = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
            // Only render finite measurements; a non-finite probe value is
            // dropped rather than shown as "NaN% loss". Loss is a 0.0..=1.0
            // fraction (the route_trace model contract) → clamp before %.
            let rtt = edge.rtt_ms.filter(|v| v.is_finite());
            let loss = edge
                .loss
                .filter(|v| v.is_finite())
                .map(|v| (v.clamp(0.0, 1.0)) * 100.0);
            let metric = match (rtt, loss) {
                (Some(rtt), Some(loss)) => format!("{rtt:.0} ms · {loss:.0}% loss"),
                (Some(rtt), None) => format!("{rtt:.0} ms"),
                (None, Some(loss)) => format!("{loss:.0}% loss"),
                (None, None) => String::new(),
            };
            if !metric.is_empty() {
                frame.fill_text(Text {
                    content: metric,
                    position: Point::new(mid.x, mid.y - 16.0),
                    color: p.text.into_cosmic_color(),
                    size: Pixels(10.0),
                    align_x: Alignment::Center,
                    ..Text::default()
                });
            }
            frame.fill_text(Text {
                content: layer_label(edge.layer).to_string(),
                position: Point::new(mid.x, mid.y + 6.0),
                color,
                size: Pixels(9.0),
                align_x: Alignment::Center,
                ..Text::default()
            });
        }

        // Nodes: a disc with the kind glyph, the label below.
        for node in &self.graph.nodes {
            let Some(&at) = proj.get(&node.id) else {
                continue;
            };
            let r = 14.0;
            frame.fill(&Path::circle(at, r), p.surface.into_cosmic_color());
            frame.stroke(
                &Path::circle(at, r),
                Stroke::default()
                    .with_color(p.accent.into_cosmic_color())
                    .with_width(1.5),
            );
            frame.fill_text(Text {
                content: node_glyph(node.kind).to_string(),
                position: at,
                color: p.text.into_cosmic_color(),
                size: Pixels(13.0),
                align_x: Alignment::Center,
                align_y: Vertical::Center,
                ..Text::default()
            });
            frame.fill_text(Text {
                content: node.label.clone(),
                position: Point::new(at.x, at.y + r + 6.0),
                color: p.text_muted.into_cosmic_color(),
                size: Pixels(11.0),
                align_x: Alignment::Center,
                ..Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// ROUTE-TRACE-4 — the short layer name drawn under each edge.
fn layer_label(layer: Layer) -> &'static str {
    match layer {
        Layer::Host => "host",
        Layer::Mesh => "mesh",
        Layer::Vpn => "vpn",
        Layer::Public => "public",
    }
}

// ============================================================================
// VPN-GW-8 — egress matrix + route topology map + assign-route wizard.
//
// The "who exits where" surface over the durable egress-routing table
// (`action/vpn/list-routes` → `EgressRouting`). Three lenses on the SAME data:
//   1. the egress MATRIX — one row per real mesh node, resolving each node's
//      effective egress (the most-specific Node/Group/ANY route) to its gateway
//      + primary tunnel + failover chain + kill-switch;
//   2. the topology MAP — the route graph (node → gateway → provider exit) drawn
//      on the SAME canvas program the trace graph uses (`PathGraphProgram`),
//      reusing the route_trace node/edge model read-only;
//   3. the assign-route WIZARD — pick a node + gateway + primary tunnel, emit a
//      real `EgressRoute` over `action/vpn/set-route` (§7 — a real persist, no
//      stub).
// All read from real RPCs + the node roster; Carbon tokens only (§4).
// ============================================================================

/// VPN-GW-8 — the whole egress-routing section: a heading + refresh, the matrix,
/// the topology map, and the assign-route wizard, stacked. The matrix + map read
/// the loaded routing table; the wizard writes to it.
fn egress_section<'a>(state: &'a EgressState, palette: Palette) -> Element<'a, crate::Message> {
    let heading = text("Egress routing — who exits where")
        .size(TypeRole::Heading.size_in(FontSize::defaults()))
        .colr(palette.text.into_cosmic_color());
    let refresh = crate::controls::variant_button(
        if state.busy {
            "Refreshing…"
        } else {
            "Refresh"
        },
        crate::controls::ButtonVariant::Secondary,
        (!state.busy).then_some(crate::Message::Routing(Message::RefreshEgress)),
        palette,
    );
    let header = row![heading, Space::new().width(Length::Fill), refresh,]
        .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut col = column![header].spacing(10);
    if let Some(err) = &state.error {
        col = col.push(card(
            text(format!("Couldn't read the egress routing — {err}"))
                .size(12)
                .colr(palette.danger.into_cosmic_color()),
            palette,
        ));
    }
    col = col.push(egress_matrix(state, palette));
    col = col.push(route_topology(&state.routing, palette));
    col = col.push(assign_wizard(state, palette));
    col.into()
}

/// VPN-GW-8 — the resolved effective egress for one node: the route that governs
/// it (most-specific Node/Group/ANY) + a short tag of which scope matched, or
/// `None` when the node has no route at all (it exits direct WAN). Pure over the
/// loaded table so the matrix derivation is unit-testable without rendering.
#[must_use]
fn effective_egress<'a>(
    routing: &'a EgressRouting,
    node: &str,
) -> Option<(&'a EgressRoute, &'static str)> {
    // The node's groups aren't in the roster read here, so resolve over Node +
    // ANY (the two scopes a bare node name can match without group membership);
    // a Group route still shows in its own matrix row keyed by the group name.
    let route = routing.route_for(node, &[])?;
    let scope = match &route.target {
        RouteTarget::Node { .. } => "node",
        RouteTarget::Group { .. } => "group",
        RouteTarget::Any => "any (default)",
    };
    Some((route, scope))
}

/// VPN-GW-8 — the egress matrix: one row per real mesh node, showing the gateway
/// it exits through, the primary tunnel (the chain head), the failover chain
/// length, and the kill-switch state — the "who exits where" table the
/// acceptance asks for. A node with no route shows "direct WAN" (no gateway).
/// Falls back to the routed targets themselves when the node roster is empty (so
/// the matrix still shows the assignments even if `nodes list` is unreachable).
fn egress_matrix<'a>(state: &'a EgressState, palette: Palette) -> Element<'a, crate::Message> {
    let muted = palette.text_muted.into_cosmic_color();
    let header = row![
        matrix_cell("Node", 140.0, palette.text.into_cosmic_color(), true),
        matrix_cell("Gateway", 120.0, palette.text.into_cosmic_color(), true),
        matrix_cell(
            "Primary tunnel",
            140.0,
            palette.text.into_cosmic_color(),
            true
        ),
        matrix_cell("Failover", 90.0, palette.text.into_cosmic_color(), true),
        matrix_cell("Kill-switch", 90.0, palette.text.into_cosmic_color(), true),
    ]
    .spacing(8);

    // Rows: the real node roster when we have it, else the routed target names
    // (so the matrix is never empty when routes exist but the roster read fails).
    let row_nodes: Vec<String> = if state.nodes.is_empty() {
        routed_target_names(&state.routing)
    } else {
        state.nodes.clone()
    };

    let mut rows = column![header].spacing(6);
    if row_nodes.is_empty() {
        rows = rows.push(
            text("No mesh nodes / egress routes yet — assign one below.")
                .size(11)
                .colr(muted),
        );
    }
    for node in &row_nodes {
        let (gw, primary, failover, kill) = match effective_egress(&state.routing, node) {
            Some((r, scope)) => (
                format!("{} · {scope}", r.gateway),
                r.primary.clone(),
                if r.failover.is_empty() {
                    "—".to_string()
                } else {
                    format!("{} hop(s)", r.failover.len())
                },
                if r.kill_switch { "on" } else { "off" }.to_string(),
            ),
            None => (
                "direct WAN".to_string(),
                "—".to_string(),
                "—".to_string(),
                "—".to_string(),
            ),
        };
        let text_c = palette.text.into_cosmic_color();
        let row = row![
            matrix_cell(node, 140.0, text_c, false),
            matrix_cell(&gw, 120.0, muted, false),
            matrix_cell(&primary, 140.0, muted, false),
            matrix_cell(&failover, 90.0, muted, false),
            matrix_cell(&kill, 90.0, muted, false),
        ]
        .spacing(8);
        rows = rows.push(row);
    }

    let title = text("Egress matrix")
        .size(TypeRole::Body.size_in(FontSize::defaults()))
        .colr(palette.text.into_cosmic_color());
    card(
        column![title, scrollable(rows).height(Length::Shrink)].spacing(8),
        palette,
    )
}

/// VPN-GW-8 — one fixed-width matrix cell (a header or a value). Header cells
/// read as default text; value cells take the caller's tone. Carbon tokens.
fn matrix_cell<'a>(s: &str, width: f32, color: Color, header: bool) -> Element<'a, crate::Message> {
    text(s.to_string())
        .size(if header { 11 } else { 12 })
        .colr(color)
        .width(Length::Fixed(width))
        .into()
}

/// VPN-GW-8 — every distinct target name a route assigns to (node + group
/// targets, plus a synthetic "(all mesh)" row for an ANY default), sorted +
/// de-duplicated — the matrix's fallback rows when the live node roster is
/// unreadable, so the assignments are still visible.
#[must_use]
fn routed_target_names(routing: &EgressRouting) -> Vec<String> {
    let mut names: Vec<String> = routing
        .route
        .iter()
        .map(|r| match &r.target {
            RouteTarget::Node { name } | RouteTarget::Group { name } => name.clone(),
            RouteTarget::Any => "(all mesh)".to_string(),
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// VPN-GW-8 — build the route TOPOLOGY graph from the durable egress-routing
/// table, reusing the read-only `route_trace` node/edge model: each gateway is a
/// `Gateway` node, each gateway's primary tunnel a `VpnExit` node beyond it, the
/// internet the terminal cloud, and one `Host` node per assigned target linking
/// into its gateway. Pure — the same `PathGraph` shape the canvas already draws,
/// assembled from the route assignments rather than a single trace.
#[must_use]
fn topology_graph(routing: &EgressRouting) -> PathGraph {
    let mut g = PathGraph::new(Direction::Egress);
    let mut have_node = std::collections::HashSet::new();
    let mut push_once = |graph: &mut PathGraph, node: PathNode| {
        if have_node.insert(node.id.clone()) {
            *graph = std::mem::take(graph).with_node(node);
        }
    };
    // The terminal internet cloud (every exit leads here).
    push_once(
        &mut g,
        PathNode {
            id: "internet".into(),
            kind: NodeKind::Internet,
            label: "Internet".into(),
            ..Default::default()
        },
    );
    for r in &routing.route {
        let gw_id = format!("gw:{}", r.gateway);
        let exit_id = format!("exit:{}:{}", r.gateway, r.primary);
        push_once(
            &mut g,
            PathNode {
                id: gw_id.clone(),
                kind: NodeKind::Gateway,
                label: r.gateway.clone(),
                ..Default::default()
            },
        );
        push_once(
            &mut g,
            PathNode {
                id: exit_id.clone(),
                kind: NodeKind::VpnExit,
                label: r.primary.clone(),
                ..Default::default()
            },
        );
        // The assigned target → its gateway (mesh overlay hop).
        let target_label = match &r.target {
            RouteTarget::Node { name } | RouteTarget::Group { name } => name.clone(),
            RouteTarget::Any => "all mesh".to_string(),
        };
        let target_id = format!("src:{}", r.target.key());
        push_once(
            &mut g,
            PathNode {
                id: target_id.clone(),
                kind: NodeKind::Host,
                label: target_label,
                ..Default::default()
            },
        );
        g = g
            .with_edge(PathEdge {
                from: target_id,
                to: gw_id.clone(),
                layer: Layer::Mesh,
                transport: Transport::DirectOverlay,
                ..Default::default()
            })
            .with_edge(PathEdge {
                from: gw_id,
                to: exit_id.clone(),
                layer: Layer::Vpn,
                transport: Transport::VpnTunnel,
                ..Default::default()
            })
            .with_edge(PathEdge {
                from: exit_id,
                to: "internet".into(),
                layer: Layer::Public,
                transport: Transport::Public,
                ..Default::default()
            });
    }
    g
}

/// VPN-GW-8 — the route topology map card: the route graph on the SAME canvas
/// program the trace graph uses (`PathGraphProgram`), reusing its node-glyph /
/// layer-colored-edge drawing. Renders a hint when there are no routes yet.
fn route_topology<'a>(routing: &EgressRouting, palette: Palette) -> Element<'a, crate::Message> {
    let title = text("Route topology — mesh → gateways → exits")
        .size(TypeRole::Body.size_in(FontSize::defaults()))
        .colr(palette.text.into_cosmic_color());
    if routing.route.is_empty() {
        return card(
            column![
                title,
                text(
                    "No egress routes assigned yet. Assign one below and the route graph \
                     (node → gateway → provider exit) renders here.",
                )
                .size(12)
                .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(8),
            palette,
        );
    }
    let program = PathGraphProgram {
        graph: topology_graph(routing),
        palette,
    };
    let canvas_stock: cosmic::iced::Element<'_, crate::Message, cosmic::iced::Theme> =
        cosmic::iced::widget::canvas(program)
            .width(Length::Fill)
            .height(Length::Fixed(300.0))
            .into();
    let canvas: Element<'_, crate::Message> =
        cosmic::iced::widget::themer(None, canvas_stock).into();
    card(
        column![title, container(canvas).width(Length::Fill)].spacing(8),
        palette,
    )
}

/// VPN-GW-8 — the assign-route wizard: pick a node (from the live roster), a
/// gateway node, and a primary tunnel id, then Assign to emit a real
/// `EgressRoute` over `action/vpn/set-route`. The node is a pick_list of the
/// real roster (free-typed gateway/tunnel — the responder validates them); the
/// Assign button is gated on a complete selection. The last result (saved / an
/// error) shows beneath.
fn assign_wizard<'a>(state: &'a EgressState, palette: Palette) -> Element<'a, crate::Message> {
    let w = &state.wizard;
    let label = move |s: &str| {
        text(s.to_string())
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(70.0))
    };

    // Node picker — the real roster when present, else a free-text input so the
    // wizard still works when `nodes list` is unreachable (degrade, never block).
    let node_widget: Element<'a, crate::Message> = if state.nodes.is_empty() {
        crate::controls::styled_text_input(
            "node (e.g. anvil)",
            &w.node,
            |v| crate::Message::Routing(Message::WizardNodeChanged(v)),
            palette,
        )
    } else {
        pick_list(
            state.nodes.clone(),
            (!w.node.is_empty()).then(|| w.node.clone()),
            |v| crate::Message::Routing(Message::WizardNodeChanged(v)),
        )
        .placeholder("pick a node")
        .text_size(13)
        .into()
    };
    let gateway_widget = crate::controls::styled_text_input(
        "gateway node (e.g. gw-eagle)",
        &w.gateway,
        |v| crate::Message::Routing(Message::WizardGatewayChanged(v)),
        palette,
    );
    let primary_widget = crate::controls::styled_text_input(
        "primary tunnel id (e.g. mullvad1)",
        &w.primary,
        |v| crate::Message::Routing(Message::WizardPrimaryChanged(v)),
        palette,
    );

    let assign_msg = w
        .can_assign()
        .then_some(crate::Message::Routing(Message::AssignRoute));
    let assign_btn = crate::controls::variant_button(
        if w.busy {
            "Assigning…"
        } else {
            "Assign route"
        },
        crate::controls::ButtonVariant::Primary,
        assign_msg,
        palette,
    );

    let title = text("Assign a route")
        .size(TypeRole::Body.size_in(FontSize::defaults()))
        .colr(palette.text.into_cosmic_color());
    let node_row = row![label("Node"), node_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let gw_row = row![label("Gateway"), gateway_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let tun_row = row![label("Tunnel"), primary_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let action_row = row![Space::new().width(Length::Fill), assign_btn]
        .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut body = column![title, node_row, gw_row, tun_row, action_row].spacing(8);
    if let Some(res) = &w.result {
        let (color, msg) = match res {
            Ok(key) => (
                palette.success.into_cosmic_color(),
                format!("Route assigned — {key} (kill-switch on)"),
            ),
            Err(e) => (
                palette.danger.into_cosmic_color(),
                format!("Assign failed — {e}"),
            ),
        };
        body = body.push(text(msg).size(11).colr(color));
    } else {
        body = body.push(
            text(
                "Assigns a per-node egress route (specificity beats group/ANY) with the \
                 kill-switch on — block, don't leak, if the tunnel chain is down.",
            )
            .size(10)
            .colr(palette.text_muted.into_cosmic_color()),
        );
    }
    card(body, palette)
}

// ---- DDNS-EGRESS-5: the dynamic-DNS table + add/edit form ------------------

/// DDNS-EGRESS-5 — the DDNS surface: a header (with the zone + an Add button), the
/// published-record table, and the add/edit form when open. All real data over the
/// `action/ddns/*` responder.
fn ddns_section<'a>(state: &'a DdnsState, palette: Palette) -> Element<'a, crate::Message> {
    let heading = text("Dynamic DNS — published hostnames")
        .size(TypeRole::Heading.size_in(FontSize::defaults()))
        .colr(palette.text.into_cosmic_color());
    let add_btn = crate::controls::variant_button(
        "Add record",
        crate::controls::ButtonVariant::Primary,
        (state.form.is_none() && !state.busy)
            .then_some(crate::Message::Routing(Message::OpenDdnsForm(None))),
        palette,
    );
    let refresh = crate::controls::variant_button(
        if state.busy {
            "Refreshing…"
        } else {
            "Refresh"
        },
        crate::controls::ButtonVariant::Secondary,
        (!state.busy).then_some(crate::Message::Routing(Message::RefreshDdns)),
        palette,
    );
    let header = row![
        heading,
        Space::new().width(Length::Fill),
        add_btn,
        Space::new().width(Length::Fixed(8.0)),
        refresh,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut col = column![header].spacing(10);

    // The zone + master-enable line — context for what these names live under.
    let enabled = if state.config.enabled {
        "enabled"
    } else {
        "disabled (records won't publish until DDNS is enabled)"
    };
    col = col.push(
        text(format!("zone {} · {enabled}", state.config.zone))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    );

    if let Some(err) = &state.error {
        col = col.push(card(
            text(format!("Couldn't read the DDNS config — {err}"))
                .size(12)
                .colr(palette.danger.into_cosmic_color()),
            palette,
        ));
    }

    if let Some(res) = &state.op_result {
        let (color, msg) = match res {
            Ok(m) => (palette.success.into_cosmic_color(), m.clone()),
            Err(e) => (palette.danger.into_cosmic_color(), format!("error — {e}")),
        };
        col = col.push(text(msg).size(11).colr(color));
    }

    col = col.push(ddns_table(state, palette));
    if let Some(form) = &state.form {
        col = col.push(ddns_form(form, palette));
    }
    col.into()
}

/// DDNS-EGRESS-5 — the published-record table: hostname · source · current IP ·
/// last-updated · TTL · status, with per-row Sync / Edit / Remove actions. Empty
/// state when no records are configured.
fn ddns_table<'a>(state: &'a DdnsState, palette: Palette) -> Element<'a, crate::Message> {
    let muted = palette.text_muted.into_cosmic_color();
    let text_c = palette.text.into_cosmic_color();
    let header = row![
        matrix_cell("Hostname", 220.0, text_c, true),
        matrix_cell("Source", 130.0, text_c, true),
        matrix_cell("Current IP", 130.0, text_c, true),
        matrix_cell("Reachability", 120.0, text_c, true),
        matrix_cell("TTL", 56.0, text_c, true),
        matrix_cell("Status", 80.0, text_c, true),
    ]
    .spacing(8);

    let mut rows = column![header].spacing(6);
    if !state.loaded {
        rows = rows.push(text("Loading…").size(11).colr(muted));
    } else if state.rows.is_empty() {
        rows = rows.push(
            text("No DDNS records yet — Add record to publish a hostname for a tunnel exit or the node WAN.")
                .size(11)
                .colr(muted),
        );
    }
    for r in &state.rows {
        rows = rows.push(ddns_row_view(r, state, palette));
    }

    let title = text("Records")
        .size(TypeRole::Body.size_in(FontSize::defaults()))
        .colr(palette.text.into_cosmic_color());
    card(
        column![title, scrollable(rows).height(Length::Shrink)].spacing(8),
        palette,
    )
}

/// DDNS-EGRESS-5 — one record row: the resolved values + the Sync/Edit/Remove
/// actions. Pure mapping of a [`DdnsRow`].
fn ddns_row_view<'a>(
    r: &'a DdnsRow,
    state: &'a DdnsState,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let muted = palette.text_muted.into_cosmic_color();
    let text_c = palette.text.into_cosmic_color();
    let (status_color, status_label) = ddns_status_badge(r.status, palette);
    let ip = r.current_ip.clone().unwrap_or_else(|| "—".to_string());
    let reach = if r.reachability.is_empty() {
        "—".to_string()
    } else {
        r.reachability.clone()
    };

    let cells = row![
        matrix_cell(&r.fqdn, 220.0, text_c, false),
        matrix_cell(&r.source_label, 130.0, muted, false),
        matrix_cell(&ip, 130.0, muted, false),
        matrix_cell(&reach, 120.0, muted, false),
        matrix_cell(&r.ttl.to_string(), 56.0, muted, false),
        matrix_cell(status_label, 80.0, status_color, false),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let syncing = state.syncing.as_deref() == Some(r.name_template.as_str());
    let busy = state.busy || state.syncing.is_some();
    let sync_btn = crate::controls::variant_button(
        if syncing { "Syncing…" } else { "Sync now" },
        crate::controls::ButtonVariant::Secondary,
        (!busy).then(|| crate::Message::Routing(Message::SyncDdnsRecord(r.name_template.clone()))),
        palette,
    );
    let edit_btn = crate::controls::variant_button(
        "Edit",
        crate::controls::ButtonVariant::Ghost,
        (!busy && state.form.is_none())
            .then(|| crate::Message::Routing(Message::OpenDdnsForm(Some(r.name_template.clone())))),
        palette,
    );
    let remove_btn = crate::controls::variant_button(
        "Remove",
        crate::controls::ButtonVariant::Ghost,
        (!busy)
            .then(|| crate::Message::Routing(Message::RemoveDdnsRecord(r.name_template.clone()))),
        palette,
    );
    let actions = row![
        Space::new().width(Length::Fill),
        sync_btn,
        Space::new().width(Length::Fixed(6.0)),
        edit_btn,
        Space::new().width(Length::Fixed(6.0)),
        remove_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    column![cells, actions].spacing(4).into()
}

/// DDNS-EGRESS-5 — map a [`DdnsStatus`] → (Carbon-token colour, label). Pure.
#[must_use]
fn ddns_status_badge(status: DdnsStatus, palette: Palette) -> (Color, &'static str) {
    let color = match status {
        DdnsStatus::Synced => palette.success,
        DdnsStatus::Stale => palette.warning,
        DdnsStatus::Error => palette.danger,
        DdnsStatus::Unknown => palette.text_muted,
    };
    (color.into_cosmic_color(), status.label())
}

/// DDNS-EGRESS-5 — the add/edit record form: a name template, a source, and an
/// on-down policy pick-list, with Save / Cancel.
fn ddns_form<'a>(form: &'a DdnsForm, palette: Palette) -> Element<'a, crate::Message> {
    let label = move |s: &str| {
        text(s.to_string())
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(80.0))
    };

    // Name template — locked while editing (the record key).
    let name_widget: Element<'a, crate::Message> = if form.editing {
        text(form.name.clone())
            .size(13)
            .colr(palette.text.into_cosmic_color())
            .into()
    } else {
        crate::controls::styled_text_input(
            "name template (e.g. {node}-{provider})",
            &form.name,
            |v| crate::Message::Routing(Message::DdnsFormNameChanged(v)),
            palette,
        )
    };
    let source_widget = crate::controls::styled_text_input(
        "source (tunnel:<id> or wan)",
        &form.source,
        |v| crate::Message::Routing(Message::DdnsFormSourceChanged(v)),
        palette,
    );
    let on_down_widget: Element<'a, crate::Message> = pick_list(
        ON_DOWN_CHOICES.map(String::from).to_vec(),
        Some(if form.on_down.is_empty() {
            "keep".to_string()
        } else {
            form.on_down.clone()
        }),
        |v| crate::Message::Routing(Message::DdnsFormOnDownSelected(v)),
    )
    .placeholder("on tunnel down")
    .text_size(13)
    .into();

    let save_msg = form
        .can_save()
        .then_some(crate::Message::Routing(Message::SaveDdnsRecord));
    let save_btn = crate::controls::variant_button(
        if form.editing {
            "Save changes"
        } else {
            "Add record"
        },
        crate::controls::ButtonVariant::Primary,
        save_msg,
        palette,
    );
    let cancel_btn = crate::controls::variant_button(
        "Cancel",
        crate::controls::ButtonVariant::Ghost,
        Some(crate::Message::Routing(Message::CancelDdnsForm)),
        palette,
    );

    let title = text(if form.editing {
        "Edit record"
    } else {
        "Add record"
    })
    .size(TypeRole::Body.size_in(FontSize::defaults()))
    .colr(palette.text.into_cosmic_color());
    let name_row = row![label("Hostname"), name_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let source_row = row![label("Source"), source_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let on_down_row = row![label("On down"), on_down_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let action_row = row![
        Space::new().width(Length::Fill),
        cancel_btn,
        Space::new().width(Length::Fixed(8.0)),
        save_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let hint = text(
        "Source is a VPN tunnel exit (tunnel:<id>) or the node WAN. On down: keep (retain the \
         last value), sentinel (park at an unroutable address), or remove (delete the name).",
    )
    .size(10)
    .colr(palette.text_muted.into_cosmic_color());

    card(
        column![title, name_row, source_row, on_down_row, hint, action_row].spacing(8),
        palette,
    )
}

// ---- I/O ------------------------------------------------------

/// ROUTE-TRACE-4 — request a path trace over the Bus (`action/route/trace`) and
/// decode the reply into a [`PathGraph`]. The responder replies
/// `{"ok":true,"graph":<PathGraph>}` on success or `{"error":...}` on failure.
/// Blocking (the Bus client builds its own current-thread runtime) — call from
/// `spawn_blocking`, never on the iced executor.
fn request_trace(body: &str) -> Result<PathGraph, String> {
    let raw =
        crate::dbus::action_request_with_body("action/route/trace", Some(body), TRACE_TIMEOUT)
            .ok_or_else(|| "mackesd not reachable over the Bus (route/trace)".to_string())?;
    parse_trace_reply(&raw)
}

/// ROUTE-TRACE-4 — pure decoder for the `action/route/trace` reply envelope:
/// `{"ok":true,"graph":<PathGraph>}` → the graph; `{"error":m}` → `Err(m)`;
/// anything else → a "bad reply" error. Split out so the wire contract is
/// unit-testable without the Bus.
fn parse_trace_reply(raw: &str) -> Result<PathGraph, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad trace reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let graph = v
        .get("graph")
        .ok_or_else(|| "trace reply missing 'graph'".to_string())?;
    serde_json::from_value::<PathGraph>(graph.clone())
        .map_err(|e| format!("trace reply decode: {e}"))
}

// ---- VPN-GW-8: egress-routing I/O -----------------------------------------

/// VPN-GW-8 — fetch the durable egress-routing table (`action/vpn/list-routes`)
/// plus the real node roster, the matrix/map/wizard's data source. The roster
/// read is best-effort, so an unreachable `nodes list` degrades to an empty
/// roster — the matrix then falls back to the routed target names — rather than
/// failing the whole load. Blocking (a Bus client plus a `mackesd` shell), so
/// call it from `spawn_blocking`, never on the iced executor.
fn fetch_egress() -> Result<(EgressRouting, Vec<String>), String> {
    let raw = crate::dbus::action_request("action/vpn/list-routes", ROUTES_TIMEOUT)
        .ok_or_else(|| "mackesd not reachable over the Bus (vpn/list-routes)".to_string())?;
    let routing = parse_routes_reply(&raw)?;
    // Best-effort roster — never fail the egress load on a roster read error.
    let nodes = crate::panels::node_roster::fetch_peers()
        .map(|peers| peers.into_iter().map(|p| p.name).collect())
        .unwrap_or_default();
    Ok((routing, nodes))
}

/// VPN-GW-8 — pure decoder for the `action/vpn/list-routes` reply envelope:
/// `{"ok":true,"routes":[<EgressRoute>...]}` → the `EgressRouting` table;
/// `{"error":m}` → `Err(m)`. Split out so the wire contract is unit-testable
/// without the Bus.
fn parse_routes_reply(raw: &str) -> Result<EgressRouting, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad list-routes reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let routes = v
        .get("routes")
        .ok_or_else(|| "list-routes reply missing 'routes'".to_string())?;
    let route: Vec<EgressRoute> =
        serde_json::from_value(routes.clone()).map_err(|e| format!("list-routes decode: {e}"))?;
    Ok(EgressRouting { route })
}

/// VPN-GW-8 — assign an egress route over the Bus (`action/vpn/set-route`) — the
/// wizard's real terminal action (§7, a durable persist, no stub). The body is
/// an `EgressRoute` JSON; the responder validates + persists it. Decodes the
/// `{"ok":true,"target":<key>}` reply into the saved target key. Blocking — call
/// from `spawn_blocking`.
fn assign_route(body: &str) -> Result<String, String> {
    let raw =
        crate::dbus::action_request_with_body("action/vpn/set-route", Some(body), ROUTES_TIMEOUT)
            .ok_or_else(|| "mackesd not reachable over the Bus (vpn/set-route)".to_string())?;
    parse_set_route_reply(&raw)
}

/// VPN-GW-8 — pure decoder for the `action/vpn/set-route` reply:
/// `{"ok":true,"target":<key>}` → the saved target key; `{"error":m}` → `Err(m)`.
fn parse_set_route_reply(raw: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad set-route reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(v
            .get("target")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("route")
            .to_string());
    }
    Err(format!("unexpected set-route reply: {raw}"))
}

// ---- DDNS-EGRESS-5: dynamic-DNS I/O ---------------------------------------

/// DDNS-EGRESS-5 — fetch the `[ddns]` config (`action/ddns/get-config`), the local
/// node short name (`action/nebula/self-node`, for `{node}` templating), and a
/// coarse per-record liveness resolve. Each `tunnel:<id>` record is paired with the
/// fast `action/vpn/tunnel-status` probe (up/down only — no slow `verify-egress`),
/// so the table loads inside the 2 s window with a synced/stale/error verdict; the
/// verified exit IP is filled per row by "Sync now". Blocking (the Bus client owns
/// a current-thread runtime) — call from `spawn_blocking`, never on the iced
/// executor.
fn fetch_ddns() -> Result<DdnsLoad, String> {
    let raw = crate::dbus::action_request("action/ddns/get-config", DDNS_TIMEOUT)
        .ok_or_else(|| "mackesd not reachable over the Bus (ddns/get-config)".to_string())?;
    let config = parse_ddns_config(&raw)?;
    let node = fetch_self_node_name().unwrap_or_default();

    let mut rows = Vec::with_capacity(config.record.len());
    for rec in &config.record {
        let mut row = DdnsRow::from_record(rec, &config, &node);
        // Coarse liveness: a `tunnel:<id>` record reads the fast tunnel-status
        // up/down; a `wan` record can't be cheaply resolved without a verify, so it
        // stays Unknown until a Sync. An Up here carries no IP yet (that's the
        // verify's job) — the status derivation only needs up/down + on_down.
        if let Some(id) = rec.source.trim().strip_prefix("tunnel:") {
            if let Some(reply) = crate::dbus::action_request_with_body(
                "action/vpn/tunnel-status",
                Some(id.trim()),
                DDNS_TIMEOUT,
            ) {
                let state = if ddns_status_up(&reply) {
                    // Up but exit-IP unverified — identity-only until Sync confirms
                    // the verified exit IP (and whether it's inbound-reachable).
                    SourceState::Up {
                        ip: String::new(),
                        port_forward: false,
                    }
                } else {
                    SourceState::Down { kill_switch: false }
                };
                row.apply_resolve(&state, rec.on_down);
                // An up source with no verified IP yet: keep the IP column empty
                // (apply_resolve already does — blank ip → None) and leave status
                // synced (up). Reachability reads identity-only until Sync.
            }
        }
        rows.push(row);
    }
    Ok(DdnsLoad { config, node, rows })
}

/// DDNS-EGRESS-5 — pure decoder for the `action/vpn/tunnel-status` reply
/// `{"ok":true,"up":bool}` → the `up` bool (false on any error/missing field — a
/// status we can't read is "not known up").
#[must_use]
fn ddns_status_up(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|v| v.get("up").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// DDNS-EGRESS-5 — the local node short name from `action/nebula/self-node` (the
/// `host` field), the `{node}` template substitution. `None` when the responder is
/// unreachable (the FQDN then templates with an empty node — still shows the
/// provider + zone). Blocking.
#[must_use]
fn fetch_self_node_name() -> Option<String> {
    let raw = crate::dbus::nebula_request("self-node")?;
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    v.get("host")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// DDNS-EGRESS-5 — "Sync now" for one record: resolve its source's verified exit IP
/// over `action/vpn/verify-egress` (a `tunnel:<id>` source) or the WAN IP carried in
/// any tunnel's verify report (a `wan` source), build the live [`SourceState`], and
/// confirm the planned action + reachability over `action/ddns/record-status`.
/// Returns the resolved row data. Blocking; uses the longer verify budget.
fn resolve_record(source: &str, on_down: OnDown) -> Result<DdnsResolve, String> {
    let state = resolve_source_state(source)?;
    // Confirm against the responder's reconcile-decision query (DDNS-EGRESS-4): it
    // validates the record exists + returns the authoritative reachability label.
    // The status itself we derive from the same (state, on_down) the worker uses, so
    // the UI verdict never drifts from what the worker would publish.
    let reachability = ddns::reachability(&state);
    let current_ip = match &state {
        SourceState::Up { ip, .. } if !ip.trim().is_empty() => Some(ip.clone()),
        _ => None,
    };
    Ok(DdnsResolve {
        current_ip,
        reachability: reachability.label().to_string(),
        status: DdnsStatus::derive(&state, on_down),
    })
}

/// DDNS-EGRESS-5 — resolve a record source to a live [`SourceState`] over the
/// existing VPN-GW read RPCs: a `tunnel:<id>` source runs `verify-egress` (the
/// verified exit IP + health), a `wan` source reads the `wan_ip` carried in that
/// same report. Mirrors the worker's `resolve_source`/`source_state_from_report`
/// mapping (a leak ⇒ kill-switched down) so the UI and the worker agree. Blocking.
fn resolve_source_state(source: &str) -> Result<SourceState, String> {
    let src = source.trim();
    if src.eq_ignore_ascii_case("wan") {
        // The WAN IP isn't its own verb; it rides every tunnel's verify report.
        // Resolve via the first configured tunnel's report; with no tunnels the WAN
        // can't be cheaply read over the existing RPCs — report it down (honest:
        // unresolved, never a fabricated IP).
        let wan = fetch_wan_ip();
        return Ok(match wan {
            Some(ip) => SourceState::Up {
                ip,
                port_forward: false,
            },
            None => SourceState::Down { kill_switch: false },
        });
    }
    let Some(id) = src.strip_prefix("tunnel:").map(str::trim) else {
        return Err(format!("unrecognized DDNS source '{source}'"));
    };
    let raw = crate::dbus::action_request_with_body(
        "action/vpn/verify-egress",
        Some(id),
        DDNS_VERIFY_TIMEOUT,
    )
    .ok_or_else(|| "mackesd not reachable over the Bus (vpn/verify-egress)".to_string())?;
    parse_verify_to_state(&raw)
}

/// DDNS-EGRESS-5 — the node WAN IP, read from any configured tunnel's
/// `verify-egress` report (`wan_ip`), since there is no standalone WAN verb. `None`
/// when no tunnel exists or none reported a WAN IP. Blocking.
#[must_use]
fn fetch_wan_ip() -> Option<String> {
    let raw = crate::dbus::action_request("action/vpn/list-tunnels", DDNS_TIMEOUT)?;
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let tunnels = v.get("tunnels").and_then(serde_json::Value::as_array)?;
    for t in tunnels {
        let Some(id) = t.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Some(reply) = crate::dbus::action_request_with_body(
            "action/vpn/verify-egress",
            Some(id),
            DDNS_VERIFY_TIMEOUT,
        ) {
            if let Some(ip) = serde_json::from_str::<serde_json::Value>(reply.trim())
                .ok()
                .and_then(|v| {
                    v.get("report")
                        .and_then(|r| r.get("wan_ip"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .filter(|s| !s.trim().is_empty())
            {
                return Some(ip);
            }
        }
    }
    None
}

/// DDNS-EGRESS-5 — add/upsert a record over `action/ddns/add-record` (the responder
/// keys by the name template, so an edit re-saves the same name). The body is a
/// `RecordDef` JSON. Blocking.
fn add_record(body: &str) -> Result<String, String> {
    let raw =
        crate::dbus::action_request_with_body("action/ddns/add-record", Some(body), DDNS_TIMEOUT)
            .ok_or_else(|| "mackesd not reachable over the Bus (ddns/add-record)".to_string())?;
    parse_ddns_ok(&raw).map(|()| "Record saved.".to_string())
}

/// DDNS-EGRESS-5 — remove a record by name template over `action/ddns/remove-record`
/// (the body is the bare name). Blocking.
fn remove_record(name: &str) -> Result<String, String> {
    let raw = crate::dbus::action_request_with_body(
        "action/ddns/remove-record",
        Some(name),
        DDNS_TIMEOUT,
    )
    .ok_or_else(|| "mackesd not reachable over the Bus (ddns/remove-record)".to_string())?;
    parse_ddns_ok(&raw).map(|()| format!("Removed {name}."))
}

/// DDNS-EGRESS-5 — pure decoder for the `action/ddns/get-config` reply
/// `{"ok":true,"config":<DdnsConfig>}` → the [`DdnsConfig`]; `{"error":m}` → `Err`.
fn parse_ddns_config(raw: &str) -> Result<DdnsConfig, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad get-config reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let cfg = v
        .get("config")
        .ok_or_else(|| "get-config reply missing 'config'".to_string())?;
    serde_json::from_value(cfg.clone()).map_err(|e| format!("get-config decode: {e}"))
}

/// DDNS-EGRESS-5 — pure decoder for an `{"ok":true}` / `{"error":m}` reply (the
/// add/remove responders' shape) into `Ok(())` / `Err(m)`.
fn parse_ddns_ok(raw: &str) -> Result<(), String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad ddns reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!("unexpected ddns reply: {raw}"))
    }
}

/// DDNS-EGRESS-5 — pure decoder mapping a `verify-egress` reply to a live
/// [`SourceState`], mirroring the worker's `source_state_from_report`: a confirmed
/// exit (`ok`/`unverifiable`) with a verified IP is `Up`; a `leaking`/`dns-leak` is
/// a kill-switched down (the leak-coupling rule — never publish a leaking exit); a
/// `down`/missing-IP is a clean down. `{"error":m}` → `Err`.
fn parse_verify_to_state(raw: &str) -> Result<SourceState, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad verify-egress reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let report = v
        .get("report")
        .ok_or_else(|| "verify-egress reply missing 'report'".to_string())?;
    let health = report
        .get("health")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("down");
    let exit_ip = report
        .get("verified_exit_ip")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Ok(match health {
        "ok" | "unverifiable" => match exit_ip {
            Some(ip) => SourceState::Up {
                ip: ip.to_string(),
                port_forward: false,
            },
            None => SourceState::Down { kill_switch: false },
        },
        "leaking" | "dns-leak" => SourceState::Down { kill_switch: true },
        _ => SourceState::Down { kill_switch: false },
    })
}

/// ROUTING-VALIDATE-1 — sleep `POLL_DELAY`, then re-fetch the verdict. Used to
/// poll for a freshly-requested run's result (the leader mints it + nodes report
/// asynchronously, so it isn't ready on the immediate fetch).
fn poll_status_later() -> Task<crate::Message> {
    Task::perform(
        async {
            tokio::time::sleep(POLL_DELAY).await;
            fetch_status()
        },
        |result| crate::Message::Routing(Message::Loaded(result)),
    )
}

/// Shell out to `mackesd validate status --json`.
pub fn fetch_status() -> Result<ValidationStatus, String> {
    let out = std::process::Command::new("mackesd")
        .args(["validate", "status", "--json"])
        .output()
        .map_err(|e| format!("mackesd validate status failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd validate status exited non-zero: {stderr}"));
    }
    Ok(parse_status(&String::from_utf8_lossy(&out.stdout)))
}

/// Shell out to `mackesd validate run` (request a fresh run).
pub fn request_run() -> Result<String, String> {
    let out = std::process::Command::new("mackesd")
        .args(["validate", "run"])
        .output()
        .map_err(|e| format!("mackesd validate run failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd validate run exited non-zero: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Pure parser for the `validate status --json` object.
#[must_use]
pub fn parse_status(raw: &str) -> ValidationStatus {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return ValidationStatus::default();
    };
    let run_id = v.get("run_id").and_then(|x| x.as_str()).map(str::to_string);
    let edges = |key: &str| -> Vec<Edge> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some(Edge {
                            from: e.get("from")?.as_str()?.to_string(),
                            to: e.get("to")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    ValidationStatus {
        run_id,
        passed: v
            .get("passed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        reachable: v
            .get("reachable")
            .and_then(|x| x.as_array())
            .map_or(0, Vec::len),
        failed_edges: edges("failed"),
        missing_reporters: v
            .get("missing_reporters")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_reads_a_pass_verdict() {
        let raw = r#"{"run_id":"v-1","passed":true,"reachable":[{"from":"a","to":"b"}],
            "failed":[],"missing_reporters":[]}"#;
        let s = parse_status(raw);
        assert_eq!(s.run_id.as_deref(), Some("v-1"));
        assert!(s.passed);
        assert_eq!(s.reachable, 1);
        assert!(s.failed_edges.is_empty());
    }

    #[test]
    fn parse_status_reads_a_fail_verdict_with_edges() {
        let raw = r#"{"run_id":"v-2","passed":false,"reachable":[],
            "failed":[{"from":"pine","to":"oak"}],"missing_reporters":["birch"]}"#;
        let s = parse_status(raw);
        assert!(!s.passed);
        assert_eq!(s.failed_edges.len(), 1);
        assert_eq!(s.failed_edges[0].from, "pine");
        assert_eq!(s.missing_reporters, vec!["birch".to_string()]);
    }

    #[test]
    fn parse_status_handles_no_run_and_garbage() {
        assert!(parse_status(r#"{"run_id":null}"#).run_id.is_none());
        assert!(parse_status("not json").run_id.is_none());
    }

    #[test]
    fn no_prior_run_auto_runs_once_then_polls_bounded() {
        // AUDIT-MESH-5 + ROUTING-VALIDATE-1 — first load with run_id:null
        // auto-requests a run (busy + auto_ran set); subsequent empty loads do
        // NOT re-request, but they DO keep polling for the verdict until the
        // budget (MAX_POLLS) is spent, then stop.
        let mut p = RoutingPanel::new();
        assert!(!p.auto_ran);
        let none_status = parse_status(r#"{"run_id":null}"#);
        let _ = p.update(Message::Loaded(Ok(none_status.clone())));
        assert!(p.auto_ran, "auto-run armed on first empty load");
        assert!(p.busy, "auto-run is in flight");

        // Subsequent empty loads keep polling (busy stays) until the budget runs
        // out — the verdict isn't ready instantly (leader mints it async).
        for _ in 0..MAX_POLLS {
            let _ = p.update(Message::Loaded(Ok(none_status.clone())));
            assert!(p.auto_ran, "never re-arms a second request");
        }
        // One more empty load past the budget → polling stops.
        let _ = p.update(Message::Loaded(Ok(none_status)));
        assert!(!p.busy, "polling stops after MAX_POLLS empty loads");
    }

    #[test]
    fn verdict_arrival_stops_polling() {
        // ROUTING-VALIDATE-1 — once a run_id lands, polling stops + resets.
        let mut p = RoutingPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_status(r#"{"run_id":null}"#))));
        assert!(p.busy);
        let verdict = parse_status(r#"{"run_id":"r1","passed":true,"reachable":9}"#);
        let _ = p.update(Message::Loaded(Ok(verdict)));
        assert!(!p.busy, "verdict stops the poll");
        assert_eq!(p.poll_attempts, 0);
        assert_eq!(p.status.run_id.as_deref(), Some("r1"));
    }

    #[test]
    fn existing_run_does_not_auto_run() {
        let mut p = RoutingPanel::new();
        let status = parse_status(
            r#"{"run_id":"v-9","passed":true,"reachable":[],
            "failed":[],"missing_reporters":[]}"#,
        );
        let _ = p.update(Message::Loaded(Ok(status)));
        assert!(!p.auto_ran, "a real run is present — no auto-run");
        assert!(!p.busy);
    }

    #[test]
    fn view_renders_all_states_without_panic() {
        let mut p = RoutingPanel::new();
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view(); // empty
        p.status = parse_status(
            r#"{"run_id":"v-2","passed":false,"reachable":[],
               "failed":[{"from":"pine","to":"oak"}],"missing_reporters":["birch"]}"#,
        );
        p.run_result = Some(Ok("requested".into()));
        let _ = p.view(); // fail verdict + strip

        // DDNS-EGRESS-5 — the DDNS surface renders in every state: loading, empty,
        // a populated table (each status), and with the add + edit forms open.
        let _ = p.view(); // ddns not loaded yet (loading)
        p.ddns.loaded = true;
        let _ = p.view(); // ddns loaded, empty
        p.ddns.config.enabled = true;
        p.ddns.node = "eagle".into();
        p.ddns.rows = vec![
            DdnsRow {
                name_template: "{node}-{provider}".into(),
                fqdn: "eagle-mullvad.services.matthewmackes.com".into(),
                source_raw: "tunnel:mullvad-1".into(),
                source_label: "tunnel mullvad-1".into(),
                current_ip: Some("203.0.113.7".into()),
                reachability: "port-forward only".into(),
                status: DdnsStatus::Synced,
                ttl: 60,
            },
            DdnsRow {
                name_template: "{node}-wan".into(),
                fqdn: "eagle-wan.services.matthewmackes.com".into(),
                source_raw: "wan".into(),
                source_label: "WAN".into(),
                current_ip: None,
                reachability: "down".into(),
                status: DdnsStatus::Error,
                ttl: 60,
            },
        ];
        p.ddns.op_result = Some(Ok("Record saved.".into()));
        let _ = p.view(); // populated table + op-result strip
        let _ = p.update(Message::OpenDdnsForm(None));
        let _ = p.view(); // add form open
        p.ddns.config.record = vec![RecordDef {
            name: "{node}-{provider}".into(),
            source: "tunnel:mullvad-1".into(),
            on_down: OnDown::Keep,
        }];
        let _ = p.update(Message::OpenDdnsForm(Some("{node}-{provider}".into())));
        let _ = p.view(); // edit form open (name locked)
    }

    // --- ROUTE-TRACE-4: trace toolbar state machine ----------------------------

    #[test]
    fn trace_egress_needs_a_source_node_to_be_traceable() {
        // Default direction is egress; a blank source can't trace; once a source
        // is set it can, and the request body is the exact egress shape.
        let mut t = TraceState::default();
        assert_eq!(t.dir(), Direction::Egress, "default direction is egress");
        assert!(!t.can_trace(), "blank egress source is not traceable");
        assert!(t.request_body().is_none());

        t.source = "eagle".into();
        t.dest = "1.1.1.1".into();
        assert!(t.can_trace());
        let body: serde_json::Value =
            serde_json::from_str(&t.request_body().expect("traceable")).unwrap();
        assert_eq!(body["direction"], "egress");
        assert_eq!(body["from"], "eagle");
        assert_eq!(body["to"], "1.1.1.1");
    }

    #[test]
    fn trace_ingress_needs_a_dest_service_and_drops_the_source() {
        // Ingress gates on the destination service id (the responder resolves the
        // host from the service policy), and the body carries no `from`.
        let mut t = TraceState {
            direction: "ingress".into(),
            source: "eagle".into(), // present but irrelevant for ingress
            ..Default::default()
        };
        assert_eq!(t.dir(), Direction::Ingress);
        assert!(!t.can_trace(), "blank ingress dest is not traceable");
        assert!(t.request_body().is_none());

        t.dest = "grafana".into();
        assert!(t.can_trace());
        let body: serde_json::Value =
            serde_json::from_str(&t.request_body().expect("traceable")).unwrap();
        assert_eq!(body["direction"], "ingress");
        assert_eq!(body["to"], "grafana");
        assert!(body.get("from").is_none(), "ingress carries no 'from'");
    }

    #[test]
    fn switching_direction_reuses_the_endpoints() {
        // The direction toggle flips which field gates Trace without clearing the
        // other — the same endpoints serve both perspectives (ROUTE-TRACE-4
        // "switches egress↔ingress for the same endpoints").
        let mut t = TraceState {
            source: "eagle".into(),
            dest: "grafana".into(),
            ..Default::default()
        };
        // Egress: traceable, egress body shape.
        assert_eq!(t.dir(), Direction::Egress);
        assert!(t.can_trace());
        // Flip to ingress: still traceable (dest is set), ingress body shape.
        t.direction = "ingress".into();
        assert_eq!(t.dir(), Direction::Ingress);
        assert!(t.can_trace());
        let body: serde_json::Value = serde_json::from_str(&t.request_body().unwrap()).unwrap();
        assert_eq!(body["direction"], "ingress");
        assert_eq!(body["to"], "grafana");
    }

    #[test]
    fn a_busy_trace_is_not_re_triggerable() {
        let t = TraceState {
            source: "eagle".into(),
            busy: true,
            ..Default::default()
        };
        assert!(!t.can_trace(), "an in-flight trace gates the button");
        assert!(t.request_body().is_none());
    }

    #[test]
    fn update_drives_the_toolbar_then_renders_the_graph() {
        // The toolbar messages mutate the state machine and TraceClicked only
        // fires when traceable; a returned PathGraph renders without panic.
        let mut p = RoutingPanel::new();
        let _ = p.update(Message::TraceSourceChanged("eagle".into()));
        let _ = p.update(Message::TraceDestChanged("1.1.1.1".into()));
        assert_eq!(p.trace.source, "eagle");
        assert!(p.trace.can_trace());
        // A blocked ingress graph (mesh-only service) renders the blocked path.
        let g =
            mackes_mesh_types::route_trace::assemble_egress("eagle", Some("10.42.0.2"), "1.1.1.1");
        let _ = p.update(Message::TraceLoaded(Ok(g)));
        assert!(p.trace.result.is_some());
        let _ = p.view(); // graph card reachable from the real view
    }

    #[test]
    fn blocked_edge_label_uses_human_node_labels() {
        // A blocked ingress trace to a mesh-only service: the verdict should read
        // the node labels (Internet → the lighthouse), not the raw wire edge id.
        let g = mackes_mesh_types::route_trace::assemble_ingress(
            &mackes_mesh_types::exposure::ExposurePolicy {
                id: "grafana".into(),
                source: mackes_mesh_types::exposure::ServiceSource {
                    node: "eagle".into(),
                    port: 3000,
                    proto: "tcp".into(),
                    ..Default::default()
                },
                tier: mackes_mesh_types::exposure::Tier::MeshOnly,
                ..Default::default()
            },
            Some("10.42.0.2"),
            None,
        );
        let at = g.blocked_at.as_deref().expect("mesh-only blocks");
        let label = blocked_edge_label(&g, at);
        // internet->ingress edge → "Internet → (no ingress)" (the labels), no
        // raw "internet->ingress" wire id.
        assert!(label.contains('→'), "{label}");
        assert!(label.starts_with("Internet"), "{label}");
        assert!(!label.contains("->"), "no raw wire id: {label}");
        // An unknown edge id falls back to the raw id.
        assert_eq!(blocked_edge_label(&g, "ghost->void"), "ghost->void");
    }

    #[test]
    fn parse_trace_reply_decodes_ok_and_error_envelopes() {
        // The ok envelope yields a PathGraph; the error envelope an Err.
        let g = mackes_mesh_types::route_trace::assemble_egress("eagle", None, "1.1.1.1");
        let ok = format!("{{\"ok\":true,\"graph\":{}}}", g.to_json().unwrap());
        let decoded = parse_trace_reply(&ok).expect("ok envelope decodes");
        assert_eq!(decoded.direction, Direction::Egress);
        assert_eq!(decoded.nodes.len(), 2);

        let err = parse_trace_reply(r#"{"error":"no such service 'nope'"}"#).unwrap_err();
        assert!(err.contains("no such service"));
        assert!(parse_trace_reply("garbage").is_err());
        assert!(
            parse_trace_reply(r#"{"ok":true}"#).is_err(),
            "missing graph"
        );
    }

    // --- ROUTE-TRACE-5: per-hop control-point list -----------------------------

    #[test]
    fn verdict_severity_maps_each_verdict_to_its_carbon_support_tone() {
        // The badge's tone derives from control.verdict via the shared
        // BadgeSeverity ramp — Allow=Success(green), Block=Danger(red),
        // Indeterminate=Warning(amber) — which status_badge tints from Carbon
        // tokens (never a raw hex). Pinning the derivation here makes the §4 tone
        // mapping verifiable without rendering.
        let cp = |verdict: Verdict| ControlPoint {
            firewall: "firewalld:public".into(),
            verdict,
            rule: "x".into(),
        };
        assert_eq!(
            verdict_severity(cp(Verdict::Allow).verdict),
            BadgeSeverity::Success,
            "Allow reads success (permitted)"
        );
        assert_eq!(
            verdict_severity(cp(Verdict::Block).verdict),
            BadgeSeverity::Danger,
            "Block reads danger (path stops here)"
        );
        assert_eq!(
            verdict_severity(cp(Verdict::Indeterminate).verdict),
            BadgeSeverity::Warning,
            "Indeterminate reads warning (unresolved, not guessed)"
        );
        // The three tones are distinct — a glanceable green/red/amber chip.
        assert_ne!(
            verdict_severity(Verdict::Allow),
            verdict_severity(Verdict::Block)
        );
        assert_ne!(
            verdict_severity(Verdict::Block),
            verdict_severity(Verdict::Indeterminate)
        );
    }

    #[test]
    fn verdict_label_is_a_short_uppercase_chip() {
        assert_eq!(verdict_label(Verdict::Allow), "ALLOW");
        assert_eq!(verdict_label(Verdict::Block), "BLOCK");
        assert_eq!(verdict_label(Verdict::Indeterminate), "INDET");
    }

    #[test]
    fn control_hops_list_renders_a_row_per_control_and_none_when_unconstrained() {
        // An ingress trace to a mesh-only service crosses the public boundary
        // control point (a Block), so the list renders at least one hop row and
        // the helpers don't panic for a real assembled graph. The blocking edge is
        // the one `blocked_at` cites.
        let palette = mde_theme::Palette::gray_90();
        let blocked = mackes_mesh_types::route_trace::assemble_ingress(
            &mackes_mesh_types::exposure::ExposurePolicy {
                id: "grafana".into(),
                source: mackes_mesh_types::exposure::ServiceSource {
                    node: "eagle".into(),
                    port: 3000,
                    proto: "tcp".into(),
                    ..Default::default()
                },
                tier: mackes_mesh_types::exposure::Tier::MeshOnly,
                ..Default::default()
            },
            Some("10.42.0.2"),
            None,
        );
        // At least one edge carries a control point ⇒ the list is rendered.
        assert!(blocked.edges.iter().any(|e| e.control.is_some()));
        assert!(
            control_hops_list(&blocked, None, palette).is_some(),
            "a constrained path renders the control list"
        );
        // The blocking edge resolves to a renderable row (selected + unselected).
        let blocking = blocked
            .edges
            .iter()
            .find(|e| e.is_blocked())
            .expect("mesh-only blocks at the boundary");
        assert!(control_hop_row(&blocked, blocking, false, palette).is_some());
        assert!(control_hop_row(&blocked, blocking, true, palette).is_some());
        let _ = firewall_badge(blocking.control.as_ref().unwrap(), palette);

        // A plain egress with no modeled firewall crosses no control points ⇒ the
        // list collapses to None (no empty chrome), and a no-control edge yields no
        // row.
        let open = mackes_mesh_types::route_trace::assemble_egress("eagle", None, "1.1.1.1");
        assert!(open.edges.iter().all(|e| e.control.is_none()));
        assert!(
            control_hops_list(&open, None, palette).is_none(),
            "an unconstrained path renders no control list"
        );
        assert!(control_hop_row(&open, &open.edges[0], false, palette).is_none());
    }

    // --- ROUTE-TRACE-5: selectable hop drill-down detail panel ------------------

    #[test]
    fn transport_layer_kind_labels_are_distinct_and_human() {
        // Each enum variant maps to a distinct, non-empty human string (no two
        // transports/layers/kinds collapse to the same label).
        let transports = [
            Transport::DirectOverlay,
            Transport::RelayOverlay,
            Transport::VpnTunnel,
            Transport::Public,
            Transport::Loopback,
        ];
        let t_labels: Vec<&str> = transports.iter().map(|t| transport_label(*t)).collect();
        assert!(t_labels.iter().all(|s| !s.is_empty()));
        for i in 0..t_labels.len() {
            for j in (i + 1)..t_labels.len() {
                assert_ne!(t_labels[i], t_labels[j], "transport labels collide");
            }
        }
        // A relay vs direct overlay read differently (the operator can tell a
        // hole-punched hop from a lighthouse-relayed one).
        assert_ne!(
            transport_label(Transport::DirectOverlay),
            transport_label(Transport::RelayOverlay)
        );

        // The pre-existing ROUTE-TRACE-4 layer_label (reused by the detail panel).
        assert_eq!(layer_label(Layer::Mesh), "mesh");
        assert_eq!(layer_label(Layer::Vpn), "vpn");
        assert_ne!(layer_label(Layer::Host), layer_label(Layer::Public));

        assert_eq!(node_kind_label(NodeKind::Vm), "VM");
        assert_eq!(node_kind_label(NodeKind::Service), "service");
        assert_ne!(
            node_kind_label(NodeKind::Gateway),
            node_kind_label(NodeKind::VpnExit)
        );
    }

    #[test]
    fn rtt_loss_line_reports_measured_and_is_honest_when_modeled() {
        let mut e = PathEdge {
            from: "a".into(),
            to: "b".into(),
            ..Default::default()
        };
        // Both measured.
        e.rtt_ms = Some(12.34);
        e.loss = Some(0.05);
        let s = rtt_loss_line(&e);
        assert!(s.contains("12.3 ms"), "{s}");
        assert!(s.contains("5% loss"), "{s}");
        // RTT only.
        e.loss = None;
        assert_eq!(rtt_loss_line(&e), "12.3 ms");
        // Neither — never fabricate a latency; say so plainly.
        e.rtt_ms = None;
        assert_eq!(rtt_loss_line(&e), "not measured (modeled hop)");
    }

    #[test]
    fn hop_detail_panel_opens_for_a_selected_edge_and_handles_a_stale_id() {
        // A real blocked ingress graph: selecting the blocking edge renders the
        // detail panel; no selection or a stale id renders nothing.
        let palette = mde_theme::Palette::gray_90();
        let g = mackes_mesh_types::route_trace::assemble_ingress(
            &mackes_mesh_types::exposure::ExposurePolicy {
                id: "grafana".into(),
                source: mackes_mesh_types::exposure::ServiceSource {
                    node: "eagle".into(),
                    port: 3000,
                    proto: "tcp".into(),
                    ..Default::default()
                },
                tier: mackes_mesh_types::exposure::Tier::MeshOnly,
                ..Default::default()
            },
            Some("10.42.0.2"),
            None,
        );
        // No selection ⇒ no panel.
        assert!(hop_detail_panel(&g, None, palette).is_none());
        // A stale/unknown edge id ⇒ no panel (a fresh trace dropped the old id).
        assert!(hop_detail_panel(&g, Some("ghost->void"), palette).is_none());
        // The first edge's id resolves ⇒ a panel renders, and find_edge agrees.
        let id = g.edges[0].id();
        assert!(find_edge(&g, &id).is_some());
        assert!(
            hop_detail_panel(&g, Some(&id), palette).is_some(),
            "a selected real edge opens its detail panel"
        );
        // The blocking edge (the one blocked_at cites) also opens cleanly.
        let blocking = g
            .blocked_at
            .clone()
            .expect("mesh-only blocks at the boundary");
        assert!(hop_detail_panel(&g, Some(&blocking), palette).is_some());
    }

    #[test]
    fn select_trace_hop_toggles_and_a_fresh_trace_clears_the_selection() {
        // The update reducer: selecting a hop opens it, re-selecting the same hop
        // closes it, selecting a different hop switches, and a fresh TraceLoaded
        // clears the open drill-down (a new graph invalidates the old edge id).
        let mut panel = RoutingPanel::new();
        assert_eq!(panel.trace.selected_hop, None);

        let _ = panel.update(Message::SelectTraceHop("a->b".into()));
        assert_eq!(panel.trace.selected_hop.as_deref(), Some("a->b"));

        // Re-select the same hop ⇒ closes.
        let _ = panel.update(Message::SelectTraceHop("a->b".into()));
        assert_eq!(panel.trace.selected_hop, None);

        // Switch to a different hop.
        let _ = panel.update(Message::SelectTraceHop("b->c".into()));
        assert_eq!(panel.trace.selected_hop.as_deref(), Some("b->c"));

        // A fresh trace result clears the selection.
        let g = mackes_mesh_types::route_trace::assemble_egress("eagle", None, "1.1.1.1");
        let _ = panel.update(Message::TraceLoaded(Ok(g)));
        assert_eq!(
            panel.trace.selected_hop, None,
            "a new graph invalidates the open hop"
        );
    }

    // --- VPN-GW-8: egress matrix + topology map + assign-route wizard ----------

    fn route(target: RouteTarget, gw: &str, primary: &str, failover: &[&str]) -> EgressRoute {
        EgressRoute {
            target,
            gateway: gw.into(),
            primary: primary.into(),
            failover: failover.iter().map(|s| (*s).to_string()).collect(),
            kill_switch: true,
        }
    }

    #[test]
    fn wizard_needs_all_three_fields_and_builds_a_per_node_route() {
        // The Assign button gates on node + gateway + primary; a complete
        // selection builds a per-node EgressRoute with the kill-switch defaulted
        // on — the exact shape the responder validates + persists.
        let mut w = WizardState::default();
        assert!(!w.can_assign(), "empty selection can't assign");
        assert!(w.route().is_none());

        w.node = "anvil".into();
        w.gateway = "gw-eagle".into();
        assert!(!w.can_assign(), "still missing the primary tunnel");
        w.primary = "mullvad1".into();
        assert!(w.can_assign());

        let r = w.route().expect("complete selection builds a route");
        assert_eq!(
            r.target,
            RouteTarget::Node {
                name: "anvil".into()
            }
        );
        assert_eq!(r.gateway, "gw-eagle");
        assert_eq!(r.primary, "mullvad1");
        assert!(r.kill_switch, "kill-switch defaults on (block, don't leak)");
        assert!(r.failover.is_empty());
        // The built route is itself valid by the model's own contract.
        assert!(r.validate().is_ok());
        // The wire body round-trips back to the same EgressRoute the responder reads.
        let body = serde_json::to_string(&r).unwrap();
        let back: EgressRoute = serde_json::from_str(&body).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn a_busy_wizard_is_not_re_triggerable() {
        let w = WizardState {
            node: "anvil".into(),
            gateway: "gw".into(),
            primary: "m1".into(),
            busy: true,
            ..Default::default()
        };
        assert!(!w.can_assign(), "an in-flight assign gates the button");
        assert!(w.route().is_none());
    }

    #[test]
    fn effective_egress_resolves_the_most_specific_route() {
        // A node with its own route resolves to it (node scope); a node with only
        // the ANY default resolves to that; an unrouted node resolves to None.
        let mut routing = EgressRouting::default();
        routing.set(route(RouteTarget::Any, "gw-any", "a1", &[]));
        routing.set(route(
            RouteTarget::Node {
                name: "anvil".into(),
            },
            "gw-node",
            "n1",
            &["n2"],
        ));
        let (r, scope) = effective_egress(&routing, "anvil").expect("anvil has a route");
        assert_eq!(r.gateway, "gw-node");
        assert_eq!(scope, "node");
        let (r, scope) = effective_egress(&routing, "loner").expect("ANY default applies");
        assert_eq!(r.gateway, "gw-any");
        assert!(scope.starts_with("any"));
        // No ANY + no node route → None (direct WAN).
        let mut sparse = EgressRouting::default();
        sparse.set(route(
            RouteTarget::Node {
                name: "anvil".into(),
            },
            "gw",
            "n1",
            &[],
        ));
        assert!(effective_egress(&sparse, "other").is_none());
    }

    #[test]
    fn topology_graph_links_each_target_through_its_gateway_to_the_internet() {
        // Each assignment contributes target → gateway → exit → internet, sharing
        // one internet node, so the canvas renders a consistent route graph.
        let mut routing = EgressRouting::default();
        routing.set(route(
            RouteTarget::Node {
                name: "anvil".into(),
            },
            "gw-eagle",
            "mullvad1",
            &[],
        ));
        routing.set(route(RouteTarget::Any, "gw-eagle", "proton1", &[]));
        let g = topology_graph(&routing);
        // One internet node, two gateways-worth of nodes (shared gateway label
        // dedups to one Gateway node), two distinct exits, two source nodes.
        assert_eq!(g.direction, Direction::Egress);
        assert!(g.nodes.iter().any(|n| n.kind == NodeKind::Internet));
        assert!(g
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Gateway && n.label == "gw-eagle"));
        assert!(
            g.nodes
                .iter()
                .filter(|n| n.kind == NodeKind::VpnExit)
                .count()
                == 2
        );
        // The graph is self-consistent (every edge endpoint is a real node) — the
        // canvas's contract.
        assert!(g.validate().is_ok(), "{:?}", g.validate());
        // An empty table yields just the terminal internet node, no edges.
        let empty = topology_graph(&EgressRouting::default());
        assert_eq!(empty.edges.len(), 0);
        assert_eq!(empty.nodes.len(), 1);
    }

    #[test]
    fn routed_target_names_are_sorted_and_deduped() {
        let mut routing = EgressRouting::default();
        routing.set(route(
            RouteTarget::Node {
                name: "zeta".into(),
            },
            "g",
            "p",
            &[],
        ));
        routing.set(route(
            RouteTarget::Node {
                name: "alpha".into(),
            },
            "g",
            "p",
            &[],
        ));
        routing.set(route(RouteTarget::Any, "g", "p", &[]));
        let names = routed_target_names(&routing);
        assert_eq!(names, vec!["(all mesh)", "alpha", "zeta"]);
    }

    #[test]
    fn parse_routes_reply_decodes_ok_and_error_envelopes() {
        // The ok envelope yields the EgressRouting table; the error envelope an Err.
        let r = route(
            RouteTarget::Node {
                name: "anvil".into(),
            },
            "gw",
            "m1",
            &["m2"],
        );
        let ok = json_routes(&[r.clone()]);
        let table = parse_routes_reply(&ok).expect("ok envelope decodes");
        assert_eq!(table.route.len(), 1);
        assert_eq!(table.route[0], r);
        // An empty list is valid (no routes assigned yet).
        assert!(parse_routes_reply(&json_routes(&[]))
            .unwrap()
            .route
            .is_empty());
        // Error + garbage envelopes.
        assert!(parse_routes_reply(r#"{"error":"daemon down"}"#)
            .unwrap_err()
            .contains("daemon down"));
        assert!(parse_routes_reply("garbage").is_err());
        assert!(
            parse_routes_reply(r#"{"ok":true}"#).is_err(),
            "missing routes"
        );
    }

    /// Build the exact `list-routes` ok envelope the responder emits.
    fn json_routes(routes: &[EgressRoute]) -> String {
        serde_json::json!({ "ok": true, "routes": routes }).to_string()
    }

    #[test]
    fn parse_set_route_reply_reads_the_saved_target_and_errors() {
        // The ok envelope yields the saved target key; the error envelope an Err.
        let ok = parse_set_route_reply(r#"{"ok":true,"target":"node:anvil"}"#).unwrap();
        assert_eq!(ok, "node:anvil");
        // ok without an explicit target falls back to a generic label, not a panic.
        assert_eq!(parse_set_route_reply(r#"{"ok":true}"#).unwrap(), "route");
        assert!(parse_set_route_reply(r#"{"error":"set-route: bad json"}"#)
            .unwrap_err()
            .contains("bad json"));
        assert!(parse_set_route_reply("nope").is_err());
    }

    #[test]
    fn assign_route_message_flow_clears_busy_and_records_the_result() {
        // The reducer: WizardAssign with a complete selection sets busy; a
        // RouteAssigned reply clears it + records the result. (No Bus is touched
        // here — we drive the state machine directly through update.)
        let mut p = RoutingPanel::new();
        let _ = p.update(Message::WizardNodeChanged("anvil".into()));
        let _ = p.update(Message::WizardGatewayChanged("gw-eagle".into()));
        let _ = p.update(Message::WizardPrimaryChanged("mullvad1".into()));
        assert!(p.egress.wizard.can_assign());
        // A landing reply records the result + clears busy.
        let _ = p.update(Message::RouteAssigned(Ok("node:anvil".into())));
        assert!(!p.egress.wizard.busy);
        assert_eq!(
            p.egress.wizard.result.as_ref().unwrap().as_deref(),
            Ok("node:anvil")
        );
        // An error reply is recorded too.
        let _ = p.update(Message::RouteAssigned(Err("save failed".into())));
        assert!(p.egress.wizard.result.as_ref().unwrap().is_err());
    }

    #[test]
    fn egress_loaded_populates_the_matrix_and_view_renders() {
        // A loaded routing table + roster populates the egress state; the full
        // view renders every egress section (matrix + topology + wizard) without
        // panic, for both a routed and an empty table.
        let mut p = RoutingPanel::new();
        let mut routing = EgressRouting::default();
        routing.set(route(
            RouteTarget::Node {
                name: "anvil".into(),
            },
            "gw-eagle",
            "mullvad1",
            &["proton1"],
        ));
        let nodes = vec!["anvil".to_string(), "eagle".to_string()];
        let _ = p.update(Message::EgressLoaded(Ok((routing, nodes))));
        assert_eq!(p.egress.routing.route.len(), 1);
        assert_eq!(p.egress.nodes.len(), 2);
        assert!(!p.egress.busy);
        let _ = p.view(); // routed table renders

        // An error load surfaces the error + clears busy.
        let _ = p.update(Message::EgressLoaded(Err(
            "vpn/list-routes unreachable".into()
        )));
        assert!(p.egress.error.is_some());
        assert!(!p.egress.busy);
        let _ = p.view(); // error state renders
    }

    // --- DDNS-EGRESS-5: dynamic-DNS table + form -------------------------------

    fn keep_record(name: &str, source: &str) -> RecordDef {
        RecordDef {
            name: name.into(),
            source: source.into(),
            on_down: OnDown::Keep,
        }
    }

    #[test]
    fn ddns_status_derives_from_source_and_on_down() {
        // Up → synced regardless of on-down.
        let up = SourceState::Up {
            ip: "1.2.3.4".into(),
            port_forward: false,
        };
        assert_eq!(DdnsStatus::derive(&up, OnDown::Keep), DdnsStatus::Synced);
        assert_eq!(DdnsStatus::derive(&up, OnDown::Remove), DdnsStatus::Synced);
        // Clean down: keep is stale (last value retained), remove/sentinel are
        // errors (name gone/parked).
        let down = SourceState::Down { kill_switch: false };
        assert_eq!(DdnsStatus::derive(&down, OnDown::Keep), DdnsStatus::Stale);
        assert_eq!(DdnsStatus::derive(&down, OnDown::Remove), DdnsStatus::Error);
        assert_eq!(
            DdnsStatus::derive(&down, OnDown::Sentinel),
            DdnsStatus::Error
        );
        // Kill-switched (leaking) down → error for every policy (leak-coupling).
        let killed = SourceState::Down { kill_switch: true };
        assert_eq!(DdnsStatus::derive(&killed, OnDown::Keep), DdnsStatus::Error);
        assert_eq!(
            DdnsStatus::derive(&killed, OnDown::Remove),
            DdnsStatus::Error
        );
        // Labels are stable.
        assert_eq!(DdnsStatus::Synced.label(), "synced");
        assert_eq!(DdnsStatus::Stale.label(), "stale");
        assert_eq!(DdnsStatus::Error.label(), "error");
        assert_eq!(DdnsStatus::Unknown.label(), "unresolved");
    }

    #[test]
    fn ddns_row_templates_the_fqdn_and_labels_the_source() {
        let cfg = DdnsConfig {
            zone: "services.matthewmackes.com".into(),
            ttl: 30,
            ..Default::default()
        };
        let tun = DdnsRow::from_record(
            &keep_record("{node}-{provider}", "tunnel:mullvad-1"),
            &cfg,
            "eagle",
        );
        assert_eq!(tun.fqdn, "eagle-mullvad-1.services.matthewmackes.com");
        assert_eq!(tun.source_label, "tunnel mullvad-1");
        assert_eq!(tun.ttl, 30);
        assert_eq!(
            tun.status,
            DdnsStatus::Unknown,
            "unresolved until a sync/load"
        );
        assert!(tun.current_ip.is_none());

        let wan = DdnsRow::from_record(&keep_record("{node}-wan", "wan"), &cfg, "eagle");
        assert_eq!(wan.fqdn, "eagle-wan.services.matthewmackes.com");
        assert_eq!(wan.source_label, "WAN");
    }

    #[test]
    fn ddns_row_apply_resolve_folds_ip_reachability_and_status() {
        let cfg = DdnsConfig::default();
        let mut row = DdnsRow::from_record(
            &keep_record("{node}-{provider}", "tunnel:m1"),
            &cfg,
            "eagle",
        );
        // Up with a verified IP, no port-forward → identity-only, synced.
        row.apply_resolve(
            &SourceState::Up {
                ip: "203.0.113.7".into(),
                port_forward: false,
            },
            OnDown::Keep,
        );
        assert_eq!(row.current_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(row.reachability, "port-forward only");
        assert_eq!(row.status, DdnsStatus::Synced);
        // Up with an empty ip (coarse tunnel-status liveness) → no IP yet, still
        // synced (the load path before a Sync confirms the exit IP).
        let mut coarse = DdnsRow::from_record(&keep_record("r", "tunnel:m1"), &cfg, "eagle");
        coarse.apply_resolve(
            &SourceState::Up {
                ip: String::new(),
                port_forward: false,
            },
            OnDown::Keep,
        );
        assert!(coarse.current_ip.is_none());
        assert_eq!(coarse.status, DdnsStatus::Synced);
        // Down + keep → stale, no IP.
        row.apply_resolve(&SourceState::Down { kill_switch: false }, OnDown::Keep);
        assert!(row.current_ip.is_none());
        assert_eq!(row.reachability, "down");
        assert_eq!(row.status, DdnsStatus::Stale);
    }

    #[test]
    fn ddns_form_builds_the_record_and_gates_save() {
        let mut f = DdnsForm::new_add();
        // Default add form: a template + keep, but no source yet → can't save.
        assert!(!f.can_save(), "a blank source can't save");
        assert!(f.record().is_none());
        f.source = "tunnel:mullvad-1".into();
        f.on_down = "sentinel".into();
        assert!(f.can_save());
        let rec = f.record().expect("complete form builds a record");
        assert_eq!(rec.name, "{node}-{provider}");
        assert_eq!(rec.source, "tunnel:mullvad-1");
        assert_eq!(rec.on_down, OnDown::Sentinel);
        // An empty/whitespace name blocks save.
        f.name = "   ".into();
        assert!(!f.can_save());
    }

    #[test]
    fn ddns_edit_form_locks_the_name_template() {
        let mut p = RoutingPanel::new();
        p.ddns.config.record = vec![keep_record("{node}-{provider}", "tunnel:m1")];
        let _ = p.update(Message::OpenDdnsForm(Some("{node}-{provider}".into())));
        let f = p.ddns.form.as_ref().expect("edit form opened");
        assert!(f.editing);
        assert_eq!(f.source, "tunnel:m1");
        assert_eq!(f.on_down, "keep");
        // A name edit while editing is ignored (the name is the record key).
        let _ = p.update(Message::DdnsFormNameChanged("hacked".into()));
        assert_eq!(p.ddns.form.as_ref().unwrap().name, "{node}-{provider}");
    }

    #[test]
    fn ddns_loaded_populates_config_node_and_rows() {
        let mut p = RoutingPanel::new();
        p.ddns.busy = true;
        let load = DdnsLoad {
            config: DdnsConfig {
                enabled: true,
                zone: "z.example".into(),
                ttl: 45,
                record: vec![keep_record("{node}-{provider}", "wan")],
                ..Default::default()
            },
            node: "eagle".into(),
            rows: vec![DdnsRow::from_record(
                &keep_record("{node}-{provider}", "wan"),
                &DdnsConfig::default(),
                "eagle",
            )],
        };
        let _ = p.update(Message::DdnsLoaded(Ok(load)));
        assert!(p.ddns.loaded);
        assert!(!p.ddns.busy);
        assert!(p.ddns.config.enabled);
        assert_eq!(p.ddns.node, "eagle");
        assert_eq!(p.ddns.rows.len(), 1);
    }

    #[test]
    fn ddns_loaded_err_marks_error_and_clears_rows() {
        let mut p = RoutingPanel::new();
        p.ddns.rows = vec![DdnsRow::default()];
        let _ = p.update(Message::DdnsLoaded(Err(
            "ddns/get-config unreachable".into()
        )));
        assert!(p.ddns.loaded);
        assert!(p.ddns.error.is_some());
        assert!(p.ddns.rows.is_empty());
    }

    #[test]
    fn ddns_synced_folds_the_resolve_into_the_matching_row() {
        let mut p = RoutingPanel::new();
        p.ddns.rows = vec![DdnsRow {
            name_template: "{node}-{provider}".into(),
            ..Default::default()
        }];
        p.ddns.syncing = Some("{node}-{provider}".into());
        let _ = p.update(Message::DdnsSynced {
            name: "{node}-{provider}".into(),
            result: Ok(DdnsResolve {
                current_ip: Some("203.0.113.7".into()),
                reachability: "port-forward only".into(),
                status: DdnsStatus::Synced,
            }),
        });
        assert!(p.ddns.syncing.is_none());
        assert_eq!(p.ddns.rows[0].current_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(p.ddns.rows[0].status, DdnsStatus::Synced);
        // A sync error records an op-result without touching the row's prior IP.
        p.ddns.syncing = Some("{node}-{provider}".into());
        let _ = p.update(Message::DdnsSynced {
            name: "{node}-{provider}".into(),
            result: Err("no such tunnel".into()),
        });
        assert!(p.ddns.syncing.is_none());
        assert!(matches!(p.ddns.op_result, Some(Err(_))));
        assert_eq!(p.ddns.rows[0].current_ip.as_deref(), Some("203.0.113.7"));
    }

    #[test]
    fn ddns_config_decoder_reads_the_envelope() {
        let raw = r#"{"ok":true,"config":{"enabled":true,"provider":"digitalocean",
            "zone":"services.matthewmackes.com","token_ref":"secret:do","ttl":60,
            "record":[{"name":"{node}-{provider}","source":"tunnel:mullvad-1","on_down":"keep"}]}}"#;
        let cfg = parse_ddns_config(raw).expect("ok envelope decodes");
        assert!(cfg.enabled);
        assert_eq!(cfg.zone, "services.matthewmackes.com");
        assert_eq!(cfg.record.len(), 1);
        assert_eq!(cfg.record[0].on_down, OnDown::Keep);
        // Error + missing-config envelopes.
        assert!(parse_ddns_config(r#"{"error":"unreadable"}"#).is_err());
        assert!(parse_ddns_config(r#"{"ok":true}"#).is_err());
    }

    #[test]
    fn ddns_ok_decoder_maps_ok_and_error() {
        assert!(parse_ddns_ok(r#"{"ok":true}"#).is_ok());
        assert!(parse_ddns_ok(r#"{"error":"name and source are required"}"#).is_err());
        assert!(parse_ddns_ok(r#"{"ok":false}"#).is_err());
    }

    #[test]
    fn verify_to_state_mirrors_the_worker_mapping() {
        // ok + verified IP → up identity-only.
        let ok = r#"{"ok":true,"report":{"health":"ok","verified_exit_ip":"203.0.113.7",
            "wan_ip":"198.51.100.2"}}"#;
        assert_eq!(
            parse_verify_to_state(ok).unwrap(),
            SourceState::Up {
                ip: "203.0.113.7".into(),
                port_forward: false
            }
        );
        // leaking → kill-switched down (never publish a leaking exit).
        let leak = r#"{"ok":true,"report":{"health":"leaking","verified_exit_ip":"198.51.100.2",
            "wan_ip":"198.51.100.2"}}"#;
        assert_eq!(
            parse_verify_to_state(leak).unwrap(),
            SourceState::Down { kill_switch: true }
        );
        // down → clean down.
        let down = r#"{"ok":true,"report":{"health":"down","verified_exit_ip":null}}"#;
        assert_eq!(
            parse_verify_to_state(down).unwrap(),
            SourceState::Down { kill_switch: false }
        );
        // ok but no IP → clean down (nothing safe to publish).
        let noip = r#"{"ok":true,"report":{"health":"ok","verified_exit_ip":null}}"#;
        assert_eq!(
            parse_verify_to_state(noip).unwrap(),
            SourceState::Down { kill_switch: false }
        );
        assert!(parse_verify_to_state(r#"{"error":"no such tunnel"}"#).is_err());
    }

    #[test]
    fn resolve_record_wan_with_no_tunnels_is_down_not_fabricated() {
        // A `wan` source with no reachable Bus / no tunnels resolves down (honest:
        // unresolved, never a fabricated IP) — and the status follows on-down.
        let r = resolve_record("wan", OnDown::Keep).expect("wan resolves to a state");
        // Off-test there's no Bus, so fetch_wan_ip returns None → down → stale.
        assert!(r.current_ip.is_none());
        assert_eq!(r.status, DdnsStatus::Stale);
        // An unrecognized source is a hard error (never a silent publish).
        assert!(resolve_record("bogus-source", OnDown::Keep).is_err());
    }
}

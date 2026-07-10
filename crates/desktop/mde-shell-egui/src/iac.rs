//! The **Infra as Code (`IaC`)** surface — the `OpenStack` `IaaS` control plane.
//!
//! `docs/design/iac-workspace.md` (the 25-lock design). IAC-2 landed the surface
//! shell + the **Overview** tab; **IAC-3** adds the `Style`-tokened
//! **Overview | Resources | Heat** tab bar + the **Resources** tab. The Overview
//! is two stacked sections:
//!
//! 1. the **`OpenStack` API status band** — a rich tile per cataloged service
//!    (name/type · health dot + latency · microversion/version · region ·
//!    public/internal/admin endpoints + port); and
//! 2. the **merged service directory** — the Keystone catalog services grouped
//!    by type (Compute / Network / Image / …), rich rows.
//!
//! The **Resources** tab (IAC-3) renders per-service read-only resource tables
//! driven by the live `action/cloud/list-resources` Bus verb (one section per
//! drillable cataloged service, sortable, row + bulk select), with the
//! catalog-driven per-service **menu verbs** (Drill / Refresh + Compute's armed
//! Nova lifecycle), **typed-arming** on every destructive mutation (#22), and a
//! **linked cross-service** jump bar (#16). The **Heat** tab (IAC-4) is an honest
//! forward-looking empty state until that unit lands (§7 — not a disabled tab,
//! not fabricated stacks). The merged "Mesh services" directory group (folded
//! from `descriptors` / `probe_nmap` / the mackesd registries) remains an IAC
//! follow-up; the seam is a code-level note here, never rendered copy.
//!
//! ## How the catalog is consumed (§6)
//!
//! The shell never depends on `mackesd`. The catalog + health ride the **Bus**:
//! `mackesd`'s `openstack` worker serves the QC-11 read verb
//! **`action/cloud/get-catalog`** (a typed request/reply — the answer lands on
//! `reply/<request-ulid>`). This surface publishes an empty `get-catalog`
//! request on its poll cadence, reads the reply off the Bus, and folds the
//! shared [`mackes_mesh_types::openstack`] types ([`ServiceCatalog`] /
//! [`ServiceHealth`]) it carries. Neither crate depends on the other — only the
//! mesh-neutral shape crate is shared ([`fold_reply`] mirrors the reply's
//! payload fields).
//!
//! Every state is honest (§7): a node with no `clouds.yaml` reads **`OpenStack`
//! not configured** (the worker's gated reply), an auth/transport failure reads
//! **unreachable**, and nothing is ever fabricated — no mock catalog, no fake
//! "up".

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use mackes_mesh_types::openstack::{
    default_collection, CatalogService, EndpointInterface, HealthState, HeatPreview,
    HeatStackDetail, ResourceRow, ResourceTable, ServiceCatalog, ServiceHealth,
};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

/// The QC-11 Bus read verb this surface consumes (IAC-1) — the Keystone service
/// directory + per-endpoint API health, served on `reply/<request-ulid>`.
const CATALOG_ACTION: &str = "action/cloud/get-catalog";

/// The IAC-3 Bus read verb the Resources tab consumes — one cataloged service's
/// resource rows, served on `reply/<request-ulid>`.
const RESOURCES_ACTION: &str = "action/cloud/list-resources";

/// The `action/cloud/` namespace every cloud verb request rides (the lifecycle
/// mutations `instance-*` are published under it, armed).
const CLOUD_ACTION_PREFIX: &str = "action/cloud/";

/// The IAC-4 Heat Bus verbs the Heat tab consumes. The three reads carry the
/// `get-` prefix so they are audit-exempt (the mackesd `is_auditable` guard); the
/// four mutations audit. Each is served on `reply/<request-ulid>` per the same
/// non-blocking request/reply idiom as the catalog + resources polls.
const HEAT_SHOW_VERB: &str = "get-heat-detail";
const HEAT_PREVIEW_VERB: &str = "get-heat-preview";
const HEAT_REVERSE_VERB: &str = "get-heat-reverse";
const HEAT_CHECK_VERB: &str = "heat-check";
const HEAT_CREATE_VERB: &str = "heat-create";
const HEAT_UPDATE_VERB: &str = "heat-update";
const HEAT_DELETE_VERB: &str = "heat-delete";

/// The Keystone service type Heat (orchestration) is cataloged under — the Heat
/// tab is live only when the catalog advertises it (else an honest "no Heat").
const HEAT_SERVICE: &str = "orchestration";

/// The live-health auto-poll cadence (design Q12, ~15 s): how long a settled
/// catalog is kept before a fresh request goes out (when auto-refresh is on).
const CATALOG_REFRESH: Duration = Duration::from_secs(15);

/// How long a published request waits for its reply before the surface reads it
/// as unanswered — an honest "the cloud catalog service isn't responding" (§7),
/// distinct from the worker's own gated/failed replies.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

/// The in-view repaint cadence — keeps the poll heartbeat alive while the
/// surface is showing (mirrors the Chat surface's tail).
const POLL_REPAINT: Duration = Duration::from_secs(1);

/// The shared filled-circle status dot (the datacenter / Instances glyph), so a
/// service health pip reads one `Style` size + colour (§4).
const DOT: &str = "\u{25CF}";

/// One tile's fixed width in the status band — seven grid units, so tiles wrap
/// into an even grid regardless of the service count.
const TILE_W: f32 = Style::SP_XL * 7.0;

/// The service-type buckets the merged directory groups by (design lock #10),
/// in canonical top-to-bottom order. Every cataloged service maps to exactly one
/// via [`service_bucket`] (`Other` is the honest catch-all for a type outside
/// this set — never dropped).
const BUCKETS: [&str; 10] = [
    "Compute",
    "Network",
    "Image",
    "Volume",
    "Orchestration",
    "Identity",
    "DNS",
    "Object Storage",
    "Placement",
    "Other",
];

// ─────────────────────────────── the Bus reply ──────────────────────────────

/// The shell-side mirror of `mackesd`'s `CloudReply` for `get-catalog` (§6 — the
/// shell reads the reply's shape without depending on `mackesd`; the payload
/// types are the shared [`ServiceCatalog`] / [`ServiceHealth`]).
///
/// Only the fields this surface folds are named; the rest of the unified reply
/// (`status` / `services` / `instances` / …) is ignored. `ok` + `gated` + `error`
/// are the honest tri-state the worker answers with.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CatalogReply {
    /// `true` when a directory payload answers; `false` on gate/failure.
    ok: bool,
    /// The Keystone service directory (present on success).
    catalog: Option<ServiceCatalog>,
    /// The per-endpoint API health rows, paired with the catalog.
    health: Option<Vec<ServiceHealth>>,
    /// One cataloged service's resource table (present on a `list-resources`
    /// success) — IAC-3's Resources tab payload.
    resources: Option<ResourceTable>,
    /// A Heat stack's full detail (present on a `get-heat-detail` success) —
    /// IAC-4's Heat tab payload.
    heat_detail: Option<HeatStackDetail>,
    /// A Heat preview-update dry-run diff (present on a `get-heat-preview`
    /// success).
    heat_preview: Option<HeatPreview>,
    /// A reverse-generated HOT template (present on a `get-heat-reverse`
    /// success).
    template: Option<String>,
    /// The stack a Heat mutation acted on / created, on success.
    stack: Option<String>,
    /// An honest gate reason — the cloud isn't in a state to answer (no
    /// clouds.yaml / doctrine off). Reads as "not configured".
    gated: Option<String>,
    /// A rejection or a seam failure (auth / transport). Reads as "unreachable".
    error: Option<String>,
}

/// The Keystone catalog + its per-endpoint health, folded from one `get-catalog`
/// reply — the payload the Overview renders.
#[derive(Debug, Clone, Default)]
struct CatalogView {
    /// The authoritative service directory.
    catalog: ServiceCatalog,
    /// The per-endpoint health rows (one per probed `(service_type, interface)`).
    health: Vec<ServiceHealth>,
}

impl CatalogView {
    /// The health row for `svc` — preferring its **public** interface (what a
    /// mesh client reaches), falling back to any probed interface. `None` when no
    /// health was reported for the service (an honest "unprobed", never a faked
    /// up).
    fn health_for(&self, svc: &CatalogService) -> Option<&ServiceHealth> {
        let mut fallback: Option<&ServiceHealth> = None;
        for h in &self.health {
            if h.service_type != svc.service_type {
                continue;
            }
            if h.interface == EndpointInterface::Public {
                return Some(h);
            }
            fallback = fallback.or(Some(h));
        }
        fallback
    }

    /// How many cataloged services answered their probe [`HealthState::Up`] —
    /// the status cluster's "healthy" tally.
    fn healthy_count(&self) -> usize {
        self.catalog
            .services
            .iter()
            .filter(|s| {
                self.health_for(s)
                    .is_some_and(|h| h.state == HealthState::Up)
            })
            .count()
    }
}

/// The honest outcome of the last catalog request (§7) — never a fabricated
/// directory.
#[derive(Debug, Clone)]
enum CatalogOutcome {
    /// No reply has landed yet — the pre-poll "querying" state.
    Querying,
    /// The live Keystone catalog + per-endpoint health.
    Ready(CatalogView),
    /// The node has no `clouds.yaml` / the cloud doctrine is off — the worker's
    /// gated reply. Reads the honest "not configured" state.
    NotConfigured(String),
    /// A real failure — an auth/transport error, a rejected request, a timed-out
    /// (no-responder) request, or an unreachable Bus. Reads "unreachable".
    Failed(String),
}

/// One in-flight `get-catalog` request awaiting its reply.
#[derive(Debug, Clone)]
struct Pending {
    /// The request ULID — the correlation key its reply rides (`reply/<ulid>`).
    ulid: String,
    /// When the request was published (drives the [`REQUEST_TIMEOUT`]).
    sent: Instant,
}

/// Which tab of the surface is showing (design #21: Overview | Resources | Heat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum IacTab {
    /// The `OpenStack` API status band + merged directory (IAC-2).
    #[default]
    Overview,
    /// Per-service resource tables + catalog-driven menu verbs (IAC-3).
    Resources,
    /// Stacks / templates / drift (IAC-4) — an honest forward-looking empty
    /// state until that unit lands.
    Heat,
}

/// The honest outcome of one service's last `list-resources` request (§7) —
/// never a fabricated table.
///
/// A pane with no outcome yet (`None`) reads as the pre-poll "querying" state, so
/// there is no separate querying variant.
#[derive(Debug, Clone)]
enum ResourceOutcome {
    /// The live resource table (possibly empty = an honest "no resources").
    Ready(ResourceTable),
    /// The node has no `clouds.yaml` / the cloud doctrine is off (the worker's
    /// gated reply). Reads the honest "not configured" state.
    NotConfigured(String),
    /// A real failure — an auth/transport/parse error, a rejection, or a
    /// timed-out (no-responder) request. Reads "unreachable".
    Failed(String),
}

/// One service's Resources-tab pane — its Bus poll bookkeeping, last outcome,
/// and its table sort. Keyed by service type in [`InfraCodeState::resources`].
#[derive(Debug, Default)]
struct ResourcePane {
    /// The in-flight `list-resources` request, if any.
    pending: Option<Pending>,
    /// When the last request settled (the auto-refresh cadence anchor).
    settled_at: Option<Instant>,
    /// A manual refresh is queued (the per-service menu) — fires one request on
    /// the next poll regardless of the cadence.
    forced: bool,
    /// The last honest outcome. `None` before the first request (renders as
    /// "querying").
    outcome: Option<ResourceOutcome>,
    /// The table sort: `(header index, ascending)`. Header index 0 is the name
    /// column; 1.. are the value columns. `None` = catalog (unsorted) order.
    sort: Option<(usize, bool)>,
}

/// A pending typed-arming confirm for a destructive lifecycle op (design #22) —
/// the operator must type the instance's name before the Bus request publishes.
#[derive(Debug, Clone)]
struct Arming {
    /// The lifecycle verb (`instance-reboot` / `instance-delete`).
    verb: &'static str,
    /// The Nova instance id the op targets.
    instance_id: String,
    /// The instance's display name — the arming echo the operator must type.
    target_name: String,
    /// The operator's typed echo (armed when it equals `target_name`).
    typed: String,
}

/// The honest outcome of a Heat sub-request (§7) — never a fabricated stack /
/// diff / template. Generic over the payload so `get-heat-detail` / preview /
/// reverse share one shape; `None` (no outcome yet) reads as the pre-poll
/// "querying" state.
#[derive(Debug, Clone)]
enum HeatOutcome<T> {
    /// The live payload (a stack detail / a preview diff / a HOT template).
    Ready(T),
    /// The node has no `clouds.yaml` / the cloud doctrine is off — the honest
    /// "not configured" gate.
    NotConfigured(String),
    /// A real failure (auth / transport / parse / rejection). Reads "unreachable".
    Failed(String),
}

/// Which Heat mutation a typed-arming confirm targets (#22) — create / update /
/// delete each require the operator to type the stack name before the Bus
/// request publishes. Stack-check is non-destructive (issued directly, unarmed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeatOp {
    /// Create a new stack from the create form's template.
    Create,
    /// Update the selected stack with the edited template buffer.
    Update,
    /// Delete the selected stack.
    Delete,
}

impl HeatOp {
    /// The Bus verb this op publishes.
    const fn verb(self) -> &'static str {
        match self {
            Self::Create => HEAT_CREATE_VERB,
            Self::Update => HEAT_UPDATE_VERB,
            Self::Delete => HEAT_DELETE_VERB,
        }
    }

    /// The button/label word for this op.
    const fn label(self) -> &'static str {
        match self {
            Self::Create => "Create",
            Self::Update => "Update",
            Self::Delete => "Delete",
        }
    }
}

/// A pending typed-arming confirm for a Heat stack mutation (design #22) — the
/// operator must type the stack name before create / update / delete publishes.
#[derive(Debug, Clone)]
struct HeatArming {
    /// Which mutation.
    op: HeatOp,
    /// The stack name — the arming echo the operator must type.
    stack_name: String,
    /// The stack id (empty for create — the stack doesn't exist yet).
    stack_id: String,
    /// The HOT template to submit (create / update).
    template: String,
    /// The operator's typed echo (armed when it equals `stack_name`).
    typed: String,
}

/// The **Heat** tab's poll bookkeeping + last honest outcomes + the editable
/// template buffer + the create form (IAC-4). The stack list itself rides the
/// shared `list-resources` pane for `orchestration` (no separate poll).
#[derive(Default)]
struct HeatState {
    /// The selected stack `(stack_id, display name)`, if any — drives the detail.
    selected: Option<(String, String)>,
    /// The in-flight `get-heat-detail` request.
    show_pending: Option<Pending>,
    /// The stack id the current detail was requested for (so a selection change
    /// re-fetches, on-demand — the detail read is not auto-polled).
    show_for: Option<String>,
    /// The last honest detail outcome.
    detail: Option<HeatOutcome<HeatStackDetail>>,
    /// The editable template buffer (loaded from the detail's template on a fresh
    /// selection; the operator edits it for preview / update).
    template_buf: String,
    /// The stack id the buffer was last loaded for (so a new selection reloads it
    /// and an in-progress edit isn't clobbered on every detail refresh).
    template_for: Option<String>,
    /// The in-flight `get-heat-preview` request.
    preview_pending: Option<Pending>,
    /// The last honest preview-diff outcome.
    preview: Option<HeatOutcome<HeatPreview>>,
    /// The in-flight `get-heat-reverse` request.
    reverse_pending: Option<Pending>,
    /// The last honest reverse-generated HOT outcome.
    reverse: Option<HeatOutcome<String>>,
    /// Whether the create-stack form is open.
    show_create: bool,
    /// The create form's stack name.
    create_name: String,
    /// The create form's HOT template buffer.
    create_template: String,
    /// A pending typed-arming confirm for a create / update / delete (#22).
    arming: Option<HeatArming>,
    /// The in-flight create / update / delete / check mutation request — its
    /// reply surfaces the honest result in the action note.
    mutation_pending: Option<Pending>,
}

/// The **Infra as Code** surface state — the Bus poll bookkeeping + the last
/// honest outcome + the two Overview view toggles.
// Three independent view/poll bools (`forced` / `auto_refresh` / `show_urls`) —
// at, not over, the `struct_excessive_bools` bar (they don't fold into one enum).
// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form for a
// crate-visible item in a private module — the dock.rs idiom.)
pub struct InfraCodeState {
    /// The Bus persist root (the client data dir). `None` when the Bus is
    /// unavailable — an honest degrade (§7), not a crash.
    bus_root: Option<PathBuf>,
    /// The in-flight request, if any.
    pending: Option<Pending>,
    /// When the last request settled (answered or timed out) — the auto-refresh
    /// cadence anchor. `None` before the first request.
    settled_at: Option<Instant>,
    /// A manual **Refresh now** is queued (the Catalog menu) — fires one request
    /// on the next poll regardless of the cadence.
    forced: bool,
    /// Whether the ~15 s auto-poll is on (the Catalog menu toggle).
    auto_refresh: bool,
    /// Whether the status-band tiles list the full public/internal/admin URLs
    /// (the View menu toggle); compact host:port otherwise.
    show_urls: bool,
    /// The last honest outcome the Overview renders.
    outcome: CatalogOutcome,
    /// Which tab is showing (Overview | Resources | Heat).
    tab: IacTab,
    /// The per-service Resources-tab panes, keyed by service type. Populated on
    /// the Resources poll from the drillable cataloged services (IAC-3).
    resources: BTreeMap<String, ResourcePane>,
    /// The current row selection across the resource tables — `(service_type,
    /// resource id)` (design #15 row + bulk select; the lifecycle verbs act on
    /// it).
    selected: BTreeSet<(String, String)>,
    /// A pending typed-arming confirm for a destructive mutation (#22), if any.
    arming: Option<Arming>,
    /// The service type a linked cross-service jump last focused (#16) — its
    /// Resources section reads as highlighted. Cleared when leaving Resources.
    linked_focus: Option<String>,
    /// The Heat tab's control-loop state (IAC-4).
    heat: HeatState,
    /// A transient one-line status note (the last issued action / its error) —
    /// honest feedback, never a silent op.
    note: Option<String>,
}

impl Default for InfraCodeState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            pending: None,
            settled_at: None,
            forced: false,
            auto_refresh: true,
            show_urls: false,
            outcome: CatalogOutcome::Querying,
            tab: IacTab::Overview,
            resources: BTreeMap::new(),
            selected: BTreeSet::new(),
            arming: None,
            linked_focus: None,
            heat: HeatState::default(),
            note: None,
        }
    }
}

impl InfraCodeState {
    /// WIN7-4 — `(total, healthy)` cataloged-service counts, folded from the
    /// SAME `self.outcome` [`Self::poll`] already keeps current (the
    /// identical `get-catalog` reply fold `build_status`'s own status chips
    /// read; no second read, §7). Backs the Start Menu Infra as Code tile's
    /// live facts. `None` while `Querying`/`NotConfigured`/`Failed` — the
    /// honest "nothing to count yet" state, matching this module's own
    /// Overview render.
    pub(crate) fn service_summary(&self) -> Option<(usize, usize)> {
        let CatalogOutcome::Ready(view) = &self.outcome else {
            return None;
        };
        Some((view.catalog.services.len(), view.healthy_count()))
    }

    /// Poll the Bus for the catalog on the shared cadence + keep the repaint
    /// heartbeat alive — the shell calls this each frame while the surface is in
    /// view (the Chat / Storage tail idiom). Resolves any in-flight reply, then
    /// issues a fresh request when due (auto-refresh cadence or a queued
    /// **Refresh now**). No blocking await: the request is published sync and its
    /// reply is read off the Bus on a later tick (§7 — honest, never a stalled
    /// frame).
    pub fn poll(&mut self, ctx: &egui::Context) {
        let now = Instant::now();

        // 1. Resolve a pending request — its reply, or an honest timeout. Extract
        //    the ulid + sent-time first so the `self.pending` borrow releases before
        //    the reply is folded back into `self`.
        if let Some((ulid, sent)) = self.pending.as_ref().map(|p| (p.ulid.clone(), p.sent)) {
            if let Some(reply) = self.read_reply(&ulid) {
                self.outcome = fold_reply(reply);
                self.pending = None;
                self.settled_at = Some(now);
            } else if sent.elapsed() >= REQUEST_TIMEOUT {
                // No responder answered — honest, and only when nothing better is
                // already showing (a prior good/gated read stays rather than being
                // clobbered by a transient miss).
                if matches!(self.outcome, CatalogOutcome::Querying) {
                    self.outcome = CatalogOutcome::Failed(
                        "the cloud catalog service did not respond — OpenStack may not be running \
                         on this node"
                            .to_string(),
                    );
                }
                self.pending = None;
                self.settled_at = Some(now);
            }
        }

        // 2. Issue a fresh request when due: a queued manual refresh, or the
        //    auto-poll cadence elapsed while idle.
        let cadence_due = self.auto_refresh
            && self
                .settled_at
                .is_none_or(|t| now.duration_since(t) >= CATALOG_REFRESH);
        if self.pending.is_none() && (self.forced || cadence_due) {
            self.forced = false;
            self.send_request();
        }

        // 3. When the Resources tab is showing, drive each drillable service's
        //    `list-resources` request/reply on the same non-blocking cadence; on
        //    the Heat tab, drive the Heat control loop (IAC-4).
        if self.tab == IacTab::Resources {
            self.poll_resources(now);
        } else if self.tab == IacTab::Heat {
            self.poll_heat(now);
        }

        ctx.request_repaint_after(POLL_REPAINT);
    }

    /// Drive the per-service `list-resources` request/reply while the Resources
    /// tab is showing (IAC-3). Only when the catalog is [`CatalogOutcome::Ready`]
    /// do we know which services exist; each drillable service (one with a
    /// [`default_collection`]) gets a pane, its pending reply resolved, and a
    /// fresh request issued when due (first fetch, a queued refresh, or the
    /// auto-poll cadence). No blocking await — the same idiom as the catalog poll.
    fn poll_resources(&mut self, now: Instant) {
        let CatalogOutcome::Ready(view) = &self.outcome else {
            return;
        };
        // Snapshot the drillable (service_type, collection) pairs so the
        // `self.outcome` borrow releases before the panes are mutated.
        let services: Vec<(String, String)> = view
            .catalog
            .services
            .iter()
            .filter_map(|s| {
                default_collection(&s.service_type).map(|c| (s.service_type.clone(), c.to_string()))
            })
            .collect();

        for (ty, collection) in services {
            self.poll_resource_service(now, &ty, &collection);
        }
    }

    /// Drive one service's `list-resources` request/reply on the non-blocking
    /// cadence: ensure its pane, resolve any pending reply (or an honest
    /// timeout), then issue a fresh request when due (the first fetch, a queued
    /// refresh, or the auto-poll cadence). Shared by the Resources tab (every
    /// drillable service) and the Heat tab (the `orchestration` stack list, so
    /// the two tabs never diverge or double-poll).
    fn poll_resource_service(&mut self, now: Instant, ty: &str, collection: &str) {
        self.resources.entry(ty.to_string()).or_default();

        // Resolve a pending request — its reply, or an honest timeout.
        let pending = self
            .resources
            .get(ty)
            .and_then(|p| p.pending.as_ref().map(|q| (q.ulid.clone(), q.sent)));
        if let Some((ulid, sent)) = pending {
            if let Some(reply) = self.read_reply(&ulid) {
                let outcome = fold_resource_reply(reply);
                if let Some(pane) = self.resources.get_mut(ty) {
                    pane.outcome = Some(outcome);
                    pane.pending = None;
                    pane.settled_at = Some(now);
                }
            } else if sent.elapsed() >= REQUEST_TIMEOUT {
                if let Some(pane) = self.resources.get_mut(ty) {
                    if pane.outcome.is_none() {
                        pane.outcome = Some(ResourceOutcome::Failed(
                            "the cloud did not answer the resource request".to_string(),
                        ));
                    }
                    pane.pending = None;
                    pane.settled_at = Some(now);
                }
            }
        }

        // Issue a fresh request when due: the first fetch (no settle yet), a
        // queued refresh, or the auto-poll cadence.
        let Some(pane) = self.resources.get(ty) else {
            return;
        };
        let never = pane.settled_at.is_none();
        let cadence_due = self.auto_refresh
            && pane
                .settled_at
                .is_none_or(|t| now.duration_since(t) >= CATALOG_REFRESH);
        if pane.pending.is_none() && (pane.forced || never || cadence_due) {
            self.send_resource_request(ty, collection);
        }
    }

    /// Drive the Heat tab's control loop while it is showing (IAC-4): the stack
    /// list rides the shared `orchestration` `list-resources` pane; the stack
    /// detail (`get-heat-detail`) is fetched **on demand** — once per selection +
    /// on an explicit refresh, never on the auto-poll cadence — so the read stays
    /// cheap; and any in-flight detail / preview / reverse / mutation reply is
    /// resolved. No blocking await — the same non-blocking idiom as the catalog +
    /// resources polls.
    fn poll_heat(&mut self, now: Instant) {
        // 1. The stack list rides the shared list-resources pane for orchestration.
        if let CatalogOutcome::Ready(view) = &self.outcome {
            if let Some(collection) = view
                .catalog
                .service(HEAT_SERVICE)
                .and_then(|s| default_collection(&s.service_type))
                .map(str::to_string)
            {
                self.poll_resource_service(now, HEAT_SERVICE, &collection);
            }
        }

        // 2. Fetch the selected stack's detail on demand — when the selection
        //    changed (or a refresh cleared `show_for`), fire one `get-heat-detail`.
        let want = self.heat.selected.as_ref().map(|(id, _)| id.clone());
        if want != self.heat.show_for && self.heat.show_pending.is_none() {
            if let Some(id) = want {
                self.send_heat_show(&id);
            } else {
                self.heat.detail = None;
                self.heat.show_for = None;
            }
        }

        // 3. Resolve any in-flight Heat sub-request reply / honest timeout.
        self.resolve_heat_pendings(now);
    }

    /// Resolve any in-flight Heat request (detail / preview / reverse / mutation)
    /// against its `reply/<ulid>` lane, or an honest timeout — never a fabricated
    /// answer (§7).
    fn resolve_heat_pendings(&mut self, now: Instant) {
        // The stack detail (loads the editable template buffer on a fresh stack).
        if let Some((ulid, sent)) = self
            .heat
            .show_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent))
        {
            if let Some(reply) = self.read_reply(&ulid) {
                let outcome = fold_heat(reply, |r| r.heat_detail.clone());
                if let HeatOutcome::Ready(detail) = &outcome {
                    // Load the buffer only for a freshly-selected stack, so an
                    // in-progress edit survives a detail refresh.
                    if self.heat.template_for.as_deref() != Some(detail.stack_id.as_str()) {
                        self.heat.template_buf.clone_from(&detail.template);
                        self.heat.template_for = Some(detail.stack_id.clone());
                    }
                }
                self.heat.detail = Some(outcome);
                self.heat.show_pending = None;
            } else if sent.elapsed() >= REQUEST_TIMEOUT {
                if self.heat.detail.is_none() {
                    self.heat.detail = Some(HeatOutcome::Failed(
                        "the cloud did not answer the stack-detail request".to_string(),
                    ));
                }
                self.heat.show_pending = None;
            }
            let _ = now;
        }

        // The preview-update diff.
        if let Some((ulid, sent)) = self
            .heat
            .preview_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent))
        {
            if let Some(reply) = self.read_reply(&ulid) {
                self.heat.preview = Some(fold_heat(reply, |r| r.heat_preview.clone()));
                self.heat.preview_pending = None;
            } else if sent.elapsed() >= REQUEST_TIMEOUT {
                self.heat.preview = Some(HeatOutcome::Failed(
                    "the cloud did not answer the preview-update request".to_string(),
                ));
                self.heat.preview_pending = None;
            }
        }

        // The reverse-generated HOT template.
        if let Some((ulid, sent)) = self
            .heat
            .reverse_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent))
        {
            if let Some(reply) = self.read_reply(&ulid) {
                self.heat.reverse = Some(fold_heat(reply, |r| r.template.clone()));
                self.heat.reverse_pending = None;
            } else if sent.elapsed() >= REQUEST_TIMEOUT {
                self.heat.reverse = Some(HeatOutcome::Failed(
                    "the cloud did not answer the reverse-generate request".to_string(),
                ));
                self.heat.reverse_pending = None;
            }
        }

        // A create / update / delete / check mutation — surface its honest result.
        if let Some((ulid, sent)) = self
            .heat
            .mutation_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent))
        {
            if let Some(reply) = self.read_reply(&ulid) {
                self.note = Some(heat_mutation_note(&reply));
                self.heat.mutation_pending = None;
                // Re-list + re-fetch the detail so the change reflects.
                if let Some(pane) = self.resources.get_mut(HEAT_SERVICE) {
                    pane.forced = true;
                }
                self.heat.show_for = None;
            } else if sent.elapsed() >= REQUEST_TIMEOUT {
                self.note = Some("the cloud did not answer the Heat request".to_string());
                self.heat.mutation_pending = None;
            }
        }
    }

    /// Publish a `list-resources` request for one service + collection and record
    /// its pending ULID on the service's pane. A missing Bus / a publish failure
    /// is an honest [`ResourceOutcome::Failed`], never a panic.
    fn send_resource_request(&mut self, service_type: &str, collection: &str) {
        let body =
            serde_json::json!({ "service": service_type, "collection": collection }).to_string();
        let Some(persist) = self.persist() else {
            if let Some(pane) = self.resources.get_mut(service_type) {
                pane.outcome = Some(ResourceOutcome::Failed(
                    "the local mesh Bus is unavailable".to_string(),
                ));
                pane.settled_at = Some(Instant::now());
                pane.forced = false;
            }
            return;
        };
        match publish_request(
            &persist,
            RESOURCES_ACTION,
            Priority::Default,
            None,
            Some(&body),
        ) {
            Ok(ulid) => {
                if let Some(pane) = self.resources.get_mut(service_type) {
                    pane.pending = Some(Pending {
                        ulid,
                        sent: Instant::now(),
                    });
                    pane.forced = false;
                }
            }
            Err(e) => {
                if let Some(pane) = self.resources.get_mut(service_type) {
                    pane.outcome = Some(ResourceOutcome::Failed(format!(
                        "could not list resources: {e}"
                    )));
                    pane.settled_at = Some(Instant::now());
                    pane.forced = false;
                }
            }
        }
    }

    /// Publish a Nova lifecycle request (`action/cloud/instance-*`) for one
    /// instance — the real armed mutation seam (design #11/#22). Fire-and-poll:
    /// the request rides the Bus; the compute pane is nudged to re-list so the
    /// change reflects. An honest `note` records the outcome (never a silent op).
    fn issue_lifecycle(&mut self, verb: &str, instance_id: &str, label: &str) {
        let Some(persist) = self.persist() else {
            self.note = Some("the local mesh Bus is unavailable".to_string());
            return;
        };
        let topic = format!("{CLOUD_ACTION_PREFIX}{verb}");
        let body = serde_json::json!({ "instance": instance_id }).to_string();
        match publish_request(&persist, &topic, Priority::Default, None, Some(&body)) {
            Ok(_) => {
                self.note = Some(format!("Requested {} on {label}.", verb_label(verb)));
                // Nudge every compute pane to re-list so the new state shows.
                for (ty, pane) in &mut self.resources {
                    if service_bucket(ty) == "Compute" {
                        pane.forced = true;
                    }
                }
            }
            Err(e) => {
                self.note = Some(format!("Could not request {}: {e}", verb_label(verb)));
            }
        }
    }

    // ─────────────────────────── the Heat control loop (IAC-4) ───────────────────────────

    /// Publish an `action/cloud/<verb>` request and record its pending ULID, or an
    /// honest error string. Shared by every Heat request (§7 — a missing Bus is an
    /// honest degrade, never a panic).
    fn publish_cloud(&self, verb: &str, body: &str) -> Result<Pending, String> {
        let persist = self
            .persist()
            .ok_or_else(|| "the local mesh Bus is unavailable".to_string())?;
        let topic = format!("{CLOUD_ACTION_PREFIX}{verb}");
        publish_request(&persist, &topic, Priority::Default, None, Some(body))
            .map(|ulid| Pending {
                ulid,
                sent: Instant::now(),
            })
            .map_err(|e| e.to_string())
    }

    /// Fire an on-demand `get-heat-detail` for the stack (records `show_for` so a
    /// selection change re-fetches, and clears the stale preview).
    fn send_heat_show(&mut self, stack_id: &str) {
        self.heat.preview = None;
        self.heat.show_for = Some(stack_id.to_string());
        let body = serde_json::json!({ "stack": stack_id }).to_string();
        match self.publish_cloud(HEAT_SHOW_VERB, &body) {
            Ok(pending) => self.heat.show_pending = Some(pending),
            Err(e) => self.heat.detail = Some(HeatOutcome::Failed(e)),
        }
    }

    /// Fire a `get-heat-preview` (dry-run diff) for the selected stack with the
    /// edited template buffer. A no-selection is a silent no-op (the button that
    /// drives it is disabled without one).
    fn send_heat_preview(&mut self) {
        let Some((id, name)) = self.heat.selected.clone() else {
            return;
        };
        let body = serde_json::json!({
            "stack_name": name,
            "stack_id": id,
            "template": self.heat.template_buf,
        })
        .to_string();
        match self.publish_cloud(HEAT_PREVIEW_VERB, &body) {
            Ok(pending) => {
                self.heat.preview = None;
                self.heat.preview_pending = Some(pending);
            }
            Err(e) => self.heat.preview = Some(HeatOutcome::Failed(e)),
        }
    }

    /// Fire a `get-heat-reverse` over the live drillable services (excluding
    /// orchestration itself — reverse-generate captures raw infra, not existing
    /// stacks).
    fn send_heat_reverse(&mut self) {
        let services = self.heat_reverse_services();
        let body = serde_json::json!({ "services": services }).to_string();
        match self.publish_cloud(HEAT_REVERSE_VERB, &body) {
            Ok(pending) => {
                self.heat.reverse = None;
                self.heat.reverse_pending = Some(pending);
            }
            Err(e) => self.heat.reverse = Some(HeatOutcome::Failed(e)),
        }
    }

    /// The `(service_type, collection)` list reverse-generate enumerates — the
    /// drillable cataloged services except orchestration.
    fn heat_reverse_services(&self) -> Vec<(String, String)> {
        let CatalogOutcome::Ready(view) = &self.outcome else {
            return Vec::new();
        };
        view.catalog
            .services
            .iter()
            .filter(|s| s.service_type != HEAT_SERVICE)
            .filter_map(|s| {
                default_collection(&s.service_type).map(|c| (s.service_type.clone(), c.to_string()))
            })
            .collect()
    }

    /// Issue a Heat stack-check (drift) on the selected stack — non-destructive,
    /// so issued directly (unarmed), tracked as a mutation so its honest result
    /// surfaces.
    fn issue_heat_check(&mut self) {
        let Some((id, name)) = self.heat.selected.clone() else {
            return;
        };
        let body = serde_json::json!({ "stack_name": name, "stack_id": id }).to_string();
        self.issue_heat_mutation(HEAT_CHECK_VERB, &body, &format!("stack-check on {name}"));
    }

    /// Publish a Heat mutation (`heat-check`/`heat-create`/`heat-update`/
    /// `heat-delete`) and track its reply so the honest outcome (ok / gated /
    /// failed) surfaces in the note (§7 — never a silent op).
    fn issue_heat_mutation(&mut self, verb: &str, body: &str, label: &str) {
        match self.publish_cloud(verb, body) {
            Ok(pending) => {
                self.heat.mutation_pending = Some(pending);
                self.note = Some(format!("Requested {label}\u{2026}"));
            }
            Err(e) => self.note = Some(format!("Could not request {label}: {e}")),
        }
    }

    /// Open the typed-arming confirm for updating the selected stack with the
    /// edited template buffer (#22).
    fn arm_heat_update(&mut self) {
        if let Some((id, name)) = self.heat.selected.clone() {
            self.heat.arming = Some(HeatArming {
                op: HeatOp::Update,
                stack_name: name,
                stack_id: id,
                template: self.heat.template_buf.clone(),
                typed: String::new(),
            });
        }
    }

    /// Open the typed-arming confirm for deleting the selected stack (#22).
    fn arm_heat_delete(&mut self) {
        if let Some((id, name)) = self.heat.selected.clone() {
            self.heat.arming = Some(HeatArming {
                op: HeatOp::Delete,
                stack_name: name,
                stack_id: id,
                template: String::new(),
                typed: String::new(),
            });
        }
    }

    /// Open the typed-arming confirm for creating a stack from the create form
    /// (#22 — the echo is the entered name). An empty name is an honest note.
    fn arm_heat_create(&mut self) {
        let name = self.heat.create_name.trim().to_string();
        if name.is_empty() {
            self.note = Some("enter a stack name to create.".to_string());
            return;
        }
        self.heat.arming = Some(HeatArming {
            op: HeatOp::Create,
            stack_name: name,
            stack_id: String::new(),
            template: self.heat.create_template.clone(),
            typed: String::new(),
        });
    }

    /// Perform an armed Heat mutation — the confirm button (or the tests) call
    /// this only past the typed-arming gate (#22). Publishes the create / update /
    /// delete request and, for create, closes the form.
    fn perform_heat_mutation(&mut self, op: HeatOp, name: &str, id: &str, template: &str) {
        let body = match op {
            HeatOp::Create => serde_json::json!({ "stack_name": name, "template": template }),
            HeatOp::Update => {
                serde_json::json!({ "stack_name": name, "stack_id": id, "template": template })
            }
            HeatOp::Delete => serde_json::json!({ "stack_name": name, "stack_id": id }),
        }
        .to_string();
        self.issue_heat_mutation(
            op.verb(),
            &body,
            &format!("{} stack {name}", op.label().to_lowercase()),
        );
        if op == HeatOp::Create {
            self.heat.show_create = false;
        }
    }

    /// The single selected compute instance, as `(id, display name)` — `Some`
    /// only when exactly one compute-bucket row is selected (the destructive
    /// lifecycle verbs are single-target so the typed-arming echo is a real
    /// instance name, #22). The name is resolved from the compute pane's table,
    /// falling back to the id.
    fn single_selected_instance(&self) -> Option<(String, String)> {
        let mut compute: Vec<&String> = self
            .selected
            .iter()
            .filter(|(ty, _)| service_bucket(ty) == "Compute")
            .map(|(_, id)| id)
            .collect();
        if compute.len() != 1 {
            return None;
        }
        let id = compute.remove(0).clone();
        let name = self
            .resources
            .iter()
            .filter(|(ty, _)| service_bucket(ty) == "Compute")
            .find_map(|(_, pane)| match &pane.outcome {
                Some(ResourceOutcome::Ready(table)) => table
                    .rows
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| table.row_label(r).to_string()),
                _ => None,
            })
            .unwrap_or_else(|| id.clone());
        Some((id, name))
    }

    /// Publish an empty `get-catalog` request and record the pending ULID. A
    /// missing Bus / a publish failure is an honest [`CatalogOutcome::Failed`],
    /// never a panic.
    fn send_request(&mut self) {
        let Some(persist) = self.persist() else {
            self.outcome = CatalogOutcome::Failed("the local mesh Bus is unavailable".to_string());
            self.settled_at = Some(Instant::now());
            return;
        };
        match publish_request(&persist, CATALOG_ACTION, Priority::Default, None, None) {
            Ok(ulid) => {
                self.pending = Some(Pending {
                    ulid,
                    sent: Instant::now(),
                });
            }
            Err(e) => {
                self.outcome =
                    CatalogOutcome::Failed(format!("could not ask for the cloud catalog: {e}"));
                self.settled_at = Some(Instant::now());
            }
        }
    }

    /// Read the reply on `reply/<ulid>`, if one has landed. The first (oldest)
    /// reply wins — the RPC convention.
    fn read_reply(&self, ulid: &str) -> Option<CatalogReply> {
        let persist = self.persist()?;
        let msgs = persist.list_since(&reply_topic(ulid), None).ok()?;
        let body = msgs.first()?.body.as_deref()?;
        serde_json::from_str::<CatalogReply>(body).ok()
    }

    /// Open the Bus persist mirror at the client data dir, if reachable.
    fn persist(&self) -> Option<Persist> {
        Persist::open(self.bus_root.clone()?).ok()
    }
}

/// Fold a `get-catalog` reply into an honest [`CatalogOutcome`] (§7) — the pure
/// seam shared by the poll path and the tests. A successful reply with a
/// directory is [`CatalogOutcome::Ready`]; a gated reply is
/// [`CatalogOutcome::NotConfigured`]; an error / an `ok` reply that carries no
/// directory is [`CatalogOutcome::Failed`]. Never a fabricated catalog.
fn fold_reply(reply: CatalogReply) -> CatalogOutcome {
    if reply.ok {
        reply.catalog.map_or_else(
            || {
                CatalogOutcome::Failed(
                    "the cloud catalog reply carried no service directory".to_string(),
                )
            },
            |catalog| {
                CatalogOutcome::Ready(CatalogView {
                    catalog,
                    health: reply.health.unwrap_or_default(),
                })
            },
        )
    } else if let Some(gated) = reply.gated {
        CatalogOutcome::NotConfigured(gated)
    } else if let Some(error) = reply.error {
        CatalogOutcome::Failed(error)
    } else {
        CatalogOutcome::Failed("the cloud catalog request was rejected".to_string())
    }
}

/// Fold a `list-resources` reply into an honest [`ResourceOutcome`] (§7) — the
/// pure seam shared by the poll path and the tests. A successful reply carries a
/// table (possibly empty = an honest "no resources"); a gated reply is
/// [`ResourceOutcome::NotConfigured`]; an error / an `ok` reply with no table is
/// [`ResourceOutcome::Failed`]. Never a fabricated table.
fn fold_resource_reply(reply: CatalogReply) -> ResourceOutcome {
    if reply.ok {
        reply.resources.map_or_else(
            || ResourceOutcome::Failed("the resource reply carried no table".to_string()),
            ResourceOutcome::Ready,
        )
    } else if let Some(gated) = reply.gated {
        ResourceOutcome::NotConfigured(gated)
    } else if let Some(error) = reply.error {
        ResourceOutcome::Failed(error)
    } else {
        ResourceOutcome::Failed("the resource request was rejected".to_string())
    }
}

/// Fold a Heat reply into an honest [`HeatOutcome`] (§7) — the pure seam shared
/// by the poll path and the tests. `payload` extracts the verb's payload from a
/// successful reply (a stack detail / a preview diff / a HOT template); an `ok`
/// reply with no payload, a gate, an error, and a bare rejection each read
/// honestly. Never a fabricated stack / diff / template.
fn fold_heat<T>(
    reply: CatalogReply,
    payload: impl FnOnce(&CatalogReply) -> Option<T>,
) -> HeatOutcome<T> {
    if reply.ok {
        payload(&reply).map_or_else(
            || HeatOutcome::Failed("the Heat reply carried no payload".to_string()),
            HeatOutcome::Ready,
        )
    } else if let Some(gated) = reply.gated {
        HeatOutcome::NotConfigured(gated)
    } else if let Some(error) = reply.error {
        HeatOutcome::Failed(error)
    } else {
        HeatOutcome::Failed("the Heat request was rejected".to_string())
    }
}

/// The honest one-line note for a settled Heat mutation reply (§7): a success,
/// the gate reason, or the failure — never a silent op.
fn heat_mutation_note(reply: &CatalogReply) -> String {
    if reply.ok {
        reply.stack.as_ref().map_or_else(
            || "Heat request completed.".to_string(),
            |s| format!("Heat request completed on {s}."),
        )
    } else if let Some(gated) = &reply.gated {
        format!("Heat request gated: {gated}")
    } else {
        format!(
            "Heat request failed: {}",
            reply.error.as_deref().unwrap_or("unknown error")
        )
    }
}

/// The typed-arming gate (#22): the operator's echo, trimmed, must equal the
/// target instance name exactly before a destructive mutation may publish. The
/// single decision the confirm button + the tests share, so "unconfirmed ⇒
/// blocked" is proven without a render.
fn armed(typed: &str, target: &str) -> bool {
    typed.trim() == target
}

/// The verb button/label for a lifecycle verb (`instance-delete` → `Delete`).
fn verb_label(verb: &str) -> &'static str {
    match verb {
        "instance-start" => "Start",
        "instance-stop" => "Stop",
        "instance-reboot" => "Reboot",
        "instance-delete" => "Delete",
        _ => "Run",
    }
}

// ───────────────────────────────── the render ───────────────────────────────

/// Render the Infra-as-Code surface into `ui`: the shared MENUBAR-ALL bar
/// (INFRA AS CODE, Workloads accent) over the **Overview** body — the `OpenStack`
/// API status band + the merged service directory, or an honest not-configured /
/// unreachable / querying empty state when the Bus verb hasn't answered with a
/// catalog (§7).
pub fn infra_code_panel(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    // MENUBAR-ALL — the shared bar. Its items are real seams (§6, one dispatch
    // path): Catalog → Refresh now / Auto-refresh; View → Show endpoint URLs;
    // plus the catalog-driven per-service menus (Compute/Network/… → drill /
    // refresh / the armed lifecycle verbs). The status cluster counts the live
    // catalog (N services · M healthy · region).
    if let Some(action) = menubar::show(ui, state) {
        menubar::apply(state, action);
    }
    ui.separator();
    ui.add_space(Style::SP_XS);

    // The Overview | Resources | Heat tab bar (design #21).
    tab_bar(ui, state);
    ui.add_space(Style::SP_S);

    // A pending typed-arming confirm (a destructive mutation — instance or Heat
    // stack) + the transient action note render above the tab body — honest
    // feedback, never silent.
    render_arming(ui, state);
    render_heat_arming(ui, state);
    render_note(ui, state);

    match state.tab {
        IacTab::Overview => match &state.outcome {
            CatalogOutcome::Ready(view) => render_overview(ui, view, state.show_urls),
            other => render_catalog_absent(ui, other),
        },
        IacTab::Resources => render_resources_tab(ui, state),
        IacTab::Heat => render_heat_tab(ui, state),
    }
}

/// The Overview | Resources | Heat tab strip (design #21), using the shared
/// `Style` accents. Switching away from Resources clears the linked focus.
fn tab_bar(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    ui.horizontal(|ui| {
        for (tab, label) in [
            (IacTab::Overview, "Overview"),
            (IacTab::Resources, "Resources"),
            (IacTab::Heat, "Heat"),
        ] {
            let selected = state.tab == tab;
            let color = if selected {
                Style::ACCENT_WORKLOADS
            } else {
                Style::TEXT_DIM
            };
            let text = RichText::new(label).size(Style::BODY).color(color).strong();
            if ui.selectable_label(selected, text).clicked() {
                state.tab = tab;
                if tab != IacTab::Resources {
                    state.linked_focus = None;
                }
            }
        }
    });
}

/// The transient one-line action note (last issued lifecycle op / its error),
/// with a dismiss affordance — honest feedback for a mutation the operator just
/// armed.
fn render_note(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    let Some(note) = state.note.clone() else {
        return;
    };
    ui.horizontal(|ui| {
        ui.colored_label(Style::ACCENT, RichText::new(note).size(Style::SMALL));
        if ui.small_button("dismiss").clicked() {
            state.note = None;
        }
    });
    ui.add_space(Style::SP_XS);
}

/// The honest non-`Ready` catalog states (querying / not-configured / failed),
/// shared by the Overview and Resources tabs — both need the live catalog and
/// read the same story until it answers (§7). `Ready` is handled by the caller.
fn render_catalog_absent(ui: &mut egui::Ui, outcome: &CatalogOutcome) {
    match outcome {
        CatalogOutcome::Querying => crate::session::empty_state(
            ui,
            "Querying the cloud catalog",
            "Reading the Keystone service directory from the mesh cloud control plane\u{2026}",
        ),
        CatalogOutcome::NotConfigured(reason) => {
            crate::session::empty_state(ui, "OpenStack not configured", reason);
        }
        CatalogOutcome::Failed(reason) => {
            ui.colored_label(Style::DANGER, RichText::new(reason).size(Style::SMALL));
            ui.add_space(Style::SP_S);
            crate::session::empty_state(
                ui,
                "Cloud catalog unavailable",
                "The OpenStack control plane appears here once the mesh cloud answers.",
            );
        }
        CatalogOutcome::Ready(_) => {}
    }
}

/// The **Heat** tab body (IAC-4) — the native `IaC` control loop: the stack list
/// (over the shared `orchestration` `list-resources` pane), the reverse-generate
/// and new-stack toolbar, and — when a stack is selected — its detail (status /
/// resources / events / outputs / editable template) with preview-update,
/// stack-check, and armed update/delete. Needs the catalog + a cataloged Heat
/// service; degrades honestly otherwise (§7 — a disabled/absent state, never a
/// fabricated stack).
fn render_heat_tab(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    // The Heat tab is driven by the live catalog; until it is Ready, read the
    // same honest story as the Overview.
    let catalog = match &state.outcome {
        CatalogOutcome::Ready(view) => view.catalog.clone(),
        other => {
            render_catalog_absent(ui, other);
            return;
        }
    };
    // Honest: no orchestration endpoint ⇒ no Heat loop (never a fabricated one).
    if catalog.service(HEAT_SERVICE).is_none() {
        crate::session::empty_state(
            ui,
            "No Heat orchestration service",
            "The Keystone catalog advertises no orchestration (Heat) endpoint on this cloud, so \
             there is no native IaC engine to drive here.",
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_heat_toolbar(ui, state);
            render_heat_reverse_output(ui, state);
            render_heat_create_form(ui, state);
            render_heat_stack_list(ui, state);
            render_heat_detail(ui, state);
        });
}

/// The Heat toolbar: reverse-generate (capture reality as code, #5) + the
/// new-stack form toggle.
fn render_heat_toolbar(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    ui.horizontal(|ui| {
        if ui.button("Reverse-generate template").clicked() {
            state.send_heat_reverse();
        }
        let label = if state.heat.show_create {
            "Close new-stack form"
        } else {
            "New stack\u{2026}"
        };
        if ui.button(label).clicked() {
            state.heat.show_create = !state.heat.show_create;
        }
    });
    ui.add_space(Style::SP_XS);
}

/// The reverse-generated HOT template output (#5) — a copyable monospace view, or
/// the honest not-configured / unavailable read.
fn render_heat_reverse_output(ui: &mut egui::Ui, state: &InfraCodeState) {
    let Some(outcome) = state.heat.reverse.clone() else {
        return;
    };
    egui::CollapsingHeader::new("Reverse-generated HOT template")
        .default_open(true)
        .show(ui, |ui| match &outcome {
            HeatOutcome::NotConfigured(reason) => {
                mde_egui::muted_note(ui, format!("OpenStack not configured \u{2014} {reason}"));
            }
            HeatOutcome::Failed(reason) => {
                ui.colored_label(
                    Style::DANGER,
                    RichText::new(format!("unavailable \u{2014} {reason}")).size(Style::SMALL),
                );
            }
            HeatOutcome::Ready(hot) => {
                mde_egui::muted_note(
                    ui,
                    "Captured from live infrastructure \u{2014} review before applying.",
                );
                let mut buf = hot.clone();
                ui.add(
                    egui::TextEdit::multiline(&mut buf)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(10)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Copy template").clicked() {
                    ui.ctx().copy_text(hot.clone());
                }
            }
        });
    ui.add_space(Style::SP_XS);
}

/// The create-stack form (#6) — name + HOT template; Create is typed-armed (#22).
fn render_heat_create_form(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    if !state.heat.show_create {
        return;
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(
            RichText::new("New stack")
                .size(Style::BODY)
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Name")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add(egui::TextEdit::singleline(&mut state.heat.create_name).hint_text("stack name"));
        });
        ui.label(
            RichText::new("Template (HOT)")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add(
            egui::TextEdit::multiline(&mut state.heat.create_template)
                .font(egui::TextStyle::Monospace)
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("Create\u{2026}").color(Style::ACCENT))
                .clicked()
            {
                state.arm_heat_create();
            }
            if ui.button("Cancel").clicked() {
                state.heat.show_create = false;
            }
        });
    });
    ui.add_space(Style::SP_XS);
}

/// The stack list — single-select over the shared `orchestration` resource pane
/// (the same live `list-resources` the Resources tab uses), with an honest
/// querying / not-configured / unreachable / no-stacks read (§7).
fn render_heat_stack_list(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new("Stacks")
            .color(Style::TEXT)
            .size(Style::TITLE)
            .strong(),
    );
    let table = match state
        .resources
        .get(HEAT_SERVICE)
        .and_then(|p| p.outcome.as_ref())
    {
        Some(ResourceOutcome::Ready(t)) => t.clone(),
        Some(ResourceOutcome::NotConfigured(reason)) => {
            mde_egui::muted_note(ui, format!("OpenStack not configured \u{2014} {reason}"));
            return;
        }
        Some(ResourceOutcome::Failed(reason)) => {
            ui.colored_label(
                Style::DANGER,
                RichText::new(format!("unreachable \u{2014} {reason}")).size(Style::SMALL),
            );
            return;
        }
        None => {
            mde_egui::muted_note(ui, "querying stacks\u{2026}");
            return;
        }
    };
    if table.is_empty() {
        mde_egui::muted_note(ui, "no stacks \u{2014} create one with New stack\u{2026}");
        return;
    }
    let status_col = table
        .column_index("stack_status")
        .or_else(|| table.column_index("status"));
    egui::Grid::new("iac-heat-stack-list")
        .striped(true)
        .show(ui, |ui| {
            for h in ["Name", "Status"] {
                ui.label(
                    RichText::new(h)
                        .size(Style::SMALL)
                        .strong()
                        .color(Style::ACCENT_WORKLOADS),
                );
            }
            ui.end_row();
            for row in &table.rows {
                let name = table.row_label(row).to_string();
                let is_sel = state.heat.selected.as_ref().map(|(id, _)| id.as_str())
                    == Some(row.id.as_str());
                if ui
                    .selectable_label(
                        is_sel,
                        RichText::new(&name).size(Style::SMALL).color(Style::TEXT),
                    )
                    .clicked()
                    && !is_sel
                {
                    state.heat.selected = Some((row.id.clone(), name.clone()));
                    state.heat.preview = None;
                }
                let status = status_col
                    .and_then(|c| row.cells.get(c))
                    .cloned()
                    .unwrap_or_default();
                ui.label(
                    RichText::new(status)
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.end_row();
            }
        });
}

/// The selected stack's detail (#6): status, the action buttons, the
/// preview-update diff, and collapsible resources / events / outputs / editable
/// template sections — or the honest querying / not-configured / unreachable read.
fn render_heat_detail(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    let Some((_id, name)) = state.heat.selected.clone() else {
        ui.add_space(Style::SP_S);
        mde_egui::muted_note(
            ui,
            "Select a stack above to see its resources, events, outputs, and template.",
        );
        return;
    };
    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new(format!("Stack \u{00B7} {name}"))
            .color(Style::TEXT)
            .size(Style::TITLE)
            .strong(),
    );

    let detail = match &state.heat.detail {
        Some(HeatOutcome::Ready(d)) => d.clone(),
        Some(HeatOutcome::NotConfigured(reason)) => {
            mde_egui::muted_note(ui, format!("OpenStack not configured \u{2014} {reason}"));
            return;
        }
        Some(HeatOutcome::Failed(reason)) => {
            ui.colored_label(
                Style::DANGER,
                RichText::new(format!("unreachable \u{2014} {reason}")).size(Style::SMALL),
            );
            return;
        }
        None => {
            mde_egui::muted_note(ui, "querying stack detail\u{2026}");
            return;
        }
    };

    ui.horizontal(|ui| {
        ui.colored_label(
            Style::ACCENT_WORKLOADS,
            RichText::new(&detail.status).size(Style::SMALL).strong(),
        );
        if let Some(reason) = &detail.status_reason {
            mde_egui::muted_note(ui, reason);
        }
        if let Some(updated) = &detail.updated {
            mde_egui::muted_note(ui, format!("updated {updated}"));
        }
    });

    render_heat_detail_actions(ui, state);
    render_heat_preview(ui, state);
    render_heat_sections(ui, &detail);

    // The editable template buffer — Preview / Update act on it.
    egui::CollapsingHeader::new("Template (HOT)")
        .default_open(true)
        .show(ui, |ui| {
            mde_egui::muted_note(
                ui,
                "Edit, then Preview update (dry-run) or Update stack (armed).",
            );
            ui.add(
                egui::TextEdit::multiline(&mut state.heat.template_buf)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.small_button("Reset to live").clicked() {
                    state.heat.template_buf.clone_from(&detail.template);
                }
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(state.heat.template_buf.clone());
                }
            });
        });
}

/// The selected stack's action row (#6): refresh detail, preview-update
/// (dry-run), stack-check (drift), and the armed update/delete (#22).
fn render_heat_detail_actions(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        if ui.button("Refresh").clicked() {
            state.heat.show_for = None;
        }
        if ui.button("Preview update").clicked() {
            state.send_heat_preview();
        }
        if ui.button("Stack-check").clicked() {
            state.issue_heat_check();
        }
        if ui.button(RichText::new("Update\u{2026}")).clicked() {
            state.arm_heat_update();
        }
        if ui
            .button(RichText::new("Delete\u{2026}").color(Style::DANGER))
            .clicked()
        {
            state.arm_heat_delete();
        }
    });
    ui.add_space(Style::SP_XS);
}

/// The preview-update dry-run diff (#6) — the resource change classes, or the
/// honest no-change / not-configured / failed read.
fn render_heat_preview(ui: &mut egui::Ui, state: &InfraCodeState) {
    let Some(outcome) = state.heat.preview.clone() else {
        return;
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(
            RichText::new("Preview update (dry-run)")
                .size(Style::SMALL)
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
        match &outcome {
            HeatOutcome::NotConfigured(reason) => {
                mde_egui::muted_note(ui, format!("OpenStack not configured \u{2014} {reason}"));
            }
            HeatOutcome::Failed(reason) => {
                ui.colored_label(
                    Style::DANGER,
                    RichText::new(format!("preview failed \u{2014} {reason}")).size(Style::SMALL),
                );
            }
            HeatOutcome::Ready(preview) => render_preview_diff(ui, preview),
        }
    });
    ui.add_space(Style::SP_XS);
}

/// The four change classes of a preview-update diff, each tinted by intent (added
/// = ok, deleted = danger, replaced = warn, updated = accent), or an honest "no
/// changes".
fn render_preview_diff(ui: &mut egui::Ui, preview: &HeatPreview) {
    if preview.is_no_change() {
        mde_egui::muted_note(
            ui,
            "no changes \u{2014} the template matches the live stack",
        );
        return;
    }
    for (color, label, items) in [
        (Style::OK, "added", &preview.added),
        (Style::DANGER, "deleted", &preview.deleted),
        (Style::WARN, "replaced", &preview.replaced),
        (Style::ACCENT, "updated", &preview.updated),
    ] {
        if items.is_empty() {
            continue;
        }
        ui.horizontal(|ui| {
            ui.colored_label(
                color,
                RichText::new(format!("{label} ({})", items.len()))
                    .size(Style::SMALL)
                    .strong(),
            );
            mde_egui::muted_note(ui, items.join(", "));
        });
    }
}

/// The collapsible resources / events / outputs sections of a stack detail (#6).
fn render_heat_sections(ui: &mut egui::Ui, detail: &HeatStackDetail) {
    render_heat_resource_section(ui, detail);
    render_heat_event_section(ui, detail);
    render_heat_output_section(ui, detail);
}

/// One accent header cell for a Heat detail grid.
fn heat_grid_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(Style::SMALL)
            .strong()
            .color(Style::ACCENT_WORKLOADS),
    );
}

/// A small tinted cell in a Heat detail grid.
fn heat_cell(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(RichText::new(text).size(Style::SMALL).color(color));
}

/// The stack's resources table (#6).
fn render_heat_resource_section(ui: &mut egui::Ui, detail: &HeatStackDetail) {
    egui::CollapsingHeader::new(format!("Resources ({})", detail.resources.len()))
        .default_open(true)
        .show(ui, |ui| {
            if detail.resources.is_empty() {
                mde_egui::muted_note(ui, "no resources");
                return;
            }
            egui::Grid::new("iac-heat-resources")
                .striped(true)
                .show(ui, |ui| {
                    for h in ["Name", "Type", "Status", "Physical id"] {
                        heat_grid_header(ui, h);
                    }
                    ui.end_row();
                    for r in &detail.resources {
                        heat_cell(ui, &r.name, Style::TEXT);
                        heat_cell(ui, &r.resource_type, Style::TEXT_DIM);
                        heat_cell(ui, &r.status, Style::TEXT_DIM);
                        heat_cell(ui, &r.physical_id, Style::TEXT_DIM);
                        ui.end_row();
                    }
                });
        });
}

/// The stack's event timeline (#6).
fn render_heat_event_section(ui: &mut egui::Ui, detail: &HeatStackDetail) {
    egui::CollapsingHeader::new(format!("Events ({})", detail.events.len())).show(ui, |ui| {
        if detail.events.is_empty() {
            mde_egui::muted_note(ui, "no events");
            return;
        }
        egui::Grid::new("iac-heat-events")
            .striped(true)
            .show(ui, |ui| {
                for h in ["Time", "Resource", "Status", "Reason"] {
                    heat_grid_header(ui, h);
                }
                ui.end_row();
                for e in &detail.events {
                    heat_cell(ui, &e.time, Style::TEXT_DIM);
                    heat_cell(ui, &e.resource, Style::TEXT);
                    heat_cell(ui, &e.status, Style::TEXT_DIM);
                    heat_cell(ui, &e.reason, Style::TEXT_DIM);
                    ui.end_row();
                }
            });
    });
}

/// The stack's outputs (#6).
fn render_heat_output_section(ui: &mut egui::Ui, detail: &HeatStackDetail) {
    egui::CollapsingHeader::new(format!("Outputs ({})", detail.outputs.len())).show(ui, |ui| {
        if detail.outputs.is_empty() {
            mde_egui::muted_note(ui, "no outputs");
            return;
        }
        for o in &detail.outputs {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&o.key)
                        .size(Style::SMALL)
                        .strong()
                        .color(Style::TEXT),
                );
                heat_cell(ui, &o.value, Style::TEXT_DIM);
                if let Some(desc) = &o.description {
                    mde_egui::muted_note(ui, format!("\u{2014} {desc}"));
                }
            });
        }
    });
}

/// A pending typed-arming confirm for a Heat stack mutation (design #22): the
/// operator must type the stack name before create / update / delete publishes.
/// The decision is read out of the borrow first so the confirm can drive the
/// (mutating) `perform_heat_mutation` seam.
fn render_heat_arming(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    // (confirmed, op, name, id, template) — captured so the arming borrow drops
    // before the seam is driven.
    let mut act: Option<(bool, HeatOp, String, String, String)> = None;
    if let Some(arming) = state.heat.arming.as_mut() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.colored_label(
                Style::WARN,
                RichText::new(format!(
                    "Confirm {} stack",
                    arming.op.label().to_lowercase()
                ))
                .size(Style::BODY)
                .strong(),
            );
            mde_egui::muted_note(
                ui,
                format!(
                    "Type the stack name \u{201C}{}\u{201D} to arm this {} \u{2014} it acts on the \
                     live cloud.",
                    arming.stack_name,
                    arming.op.label().to_lowercase()
                ),
            );
            ui.add(
                egui::TextEdit::singleline(&mut arming.typed).hint_text(arming.stack_name.as_str()),
            );
            let is_armed = armed(&arming.typed, &arming.stack_name);
            ui.horizontal(|ui| {
                let confirm = ui.add_enabled(
                    is_armed,
                    egui::Button::new(RichText::new(arming.op.label()).color(Style::DANGER)),
                );
                if confirm.clicked() && is_armed {
                    act = Some((
                        true,
                        arming.op,
                        arming.stack_name.clone(),
                        arming.stack_id.clone(),
                        arming.template.clone(),
                    ));
                } else if ui.button("Cancel").clicked() {
                    act = Some((
                        false,
                        arming.op,
                        String::new(),
                        String::new(),
                        String::new(),
                    ));
                }
            });
        });
        ui.add_space(Style::SP_S);
    }
    if let Some((confirmed, op, name, id, template)) = act {
        state.heat.arming = None;
        if confirmed {
            state.perform_heat_mutation(op, &name, &id, &template);
        }
    }
}

/// A pending typed-arming confirm for a destructive lifecycle op (design #22):
/// the operator must type the instance's name before the Bus request publishes.
/// The decision is read out of the borrow first so the confirm can drive the
/// (mutating) `issue_lifecycle` seam.
fn render_arming(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    // (confirmed, verb, instance id, name) — captured so the arming borrow drops
    // before the seam is driven.
    let mut act: Option<(bool, &'static str, String, String)> = None;
    if let Some(arming) = state.arming.as_mut() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.colored_label(
                Style::WARN,
                RichText::new(format!("Confirm {} instance", verb_label(arming.verb)))
                    .size(Style::BODY)
                    .strong(),
            );
            mde_egui::muted_note(
                ui,
                format!(
                    "Type the instance name \u{201C}{}\u{201D} to arm this {} \u{2014} it acts on \
                     the live cloud and cannot be undone.",
                    arming.target_name,
                    verb_label(arming.verb).to_lowercase()
                ),
            );
            ui.add(
                egui::TextEdit::singleline(&mut arming.typed)
                    .hint_text(arming.target_name.as_str()),
            );
            let is_armed = armed(&arming.typed, &arming.target_name);
            ui.horizontal(|ui| {
                let confirm = ui.add_enabled(
                    is_armed,
                    egui::Button::new(RichText::new(verb_label(arming.verb)).color(Style::DANGER)),
                );
                if confirm.clicked() && is_armed {
                    act = Some((
                        true,
                        arming.verb,
                        arming.instance_id.clone(),
                        arming.target_name.clone(),
                    ));
                } else if ui.button("Cancel").clicked() {
                    act = Some((false, arming.verb, String::new(), String::new()));
                }
            });
        });
        ui.add_space(Style::SP_S);
    }
    if let Some((confirmed, verb, id, name)) = act {
        state.arming = None;
        if confirmed {
            state.issue_lifecycle(verb, &id, &name);
        }
    }
}

/// The **Resources** tab body (IAC-3): per-service read-only resource tables
/// driven by the live `list-resources` replies, with row + bulk selection, a
/// linked cross-service bar, and honest per-service querying / not-configured /
/// unreachable / no-resources states. Needs the catalog to know which services
/// exist — until it answers, the same honest catalog-absent story shows.
fn render_resources_tab(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    // The Resources tab is driven by the live catalog; until it is Ready, read
    // the same honest story as the Overview.
    let catalog = match &state.outcome {
        CatalogOutcome::Ready(view) => view.catalog.clone(),
        other => {
            render_catalog_absent(ui, other);
            return;
        }
    };
    let drillable: Vec<CatalogService> = catalog
        .services
        .iter()
        .filter(|s| default_collection(&s.service_type).is_some())
        .cloned()
        .collect();
    if drillable.is_empty() {
        mde_egui::muted_note(
            ui,
            "the catalog advertises no services with a drillable resource table",
        );
        return;
    }

    // Borrow the disjoint state fields the sections mutate (panes / selection /
    // linked focus) so one section's table can toggle selection while another's
    // pane is read.
    let InfraCodeState {
        resources,
        selected,
        linked_focus,
        ..
    } = state;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_linked_bar(ui, &catalog, selected, linked_focus);
            for svc in &drillable {
                render_service_section(ui, resources, selected, linked_focus.as_deref(), svc);
            }
        });
}

/// The linked cross-service bar (design #16): when exactly one compute instance
/// is selected, offer jumps to the other cataloged services (Network / Volume /
/// Orchestration) — selection follows the link (the target section highlights).
/// A link whose service isn't cataloged reads honestly "not cataloged", never a
/// dead jump.
fn render_linked_bar(
    ui: &mut egui::Ui,
    catalog: &ServiceCatalog,
    selected: &BTreeSet<(String, String)>,
    linked_focus: &mut Option<String>,
) {
    let compute_selected = selected
        .iter()
        .filter(|(ty, _)| service_bucket(ty) == "Compute")
        .count();
    if compute_selected != 1 {
        return;
    }
    ui.horizontal(|ui| {
        mde_egui::muted_note(ui, "Linked from the selected instance:");
        for bucket in ["Network", "Volume", "Orchestration"] {
            let target = catalog.services.iter().find(|s| {
                service_bucket(&s.service_type) == bucket
                    && default_collection(&s.service_type).is_some()
            });
            match target {
                Some(svc) => {
                    if ui.button(format!("\u{2192} {bucket}")).clicked() {
                        *linked_focus = Some(svc.service_type.clone());
                    }
                }
                // Honest: the link target isn't in this reply (not cataloged) —
                // shown, never a dead jump (#16 / §7).
                None => {
                    mde_egui::muted_note(ui, format!("{bucket} (not cataloged)"));
                }
            }
        }
    });
    ui.add_space(Style::SP_S);
}

/// One service's Resources section: an accent header (highlighted when it is the
/// linked-focus target) over its resource table, or the honest querying /
/// not-configured / unreachable / no-resources read when the pane's reply is
/// absent / gated / failed / empty (§7).
fn render_service_section(
    ui: &mut egui::Ui,
    resources: &mut BTreeMap<String, ResourcePane>,
    selected: &mut BTreeSet<(String, String)>,
    linked_focus: Option<&str>,
    svc: &CatalogService,
) {
    let ty = svc.service_type.clone();
    let focused = linked_focus == Some(ty.as_str());
    let pane = resources.entry(ty.clone()).or_default();

    ui.add_space(Style::SP_XS);
    let header = format!(
        "{} \u{00B7} {}",
        service_bucket(&ty),
        service_display_name(svc)
    );
    let header_color = if focused {
        Style::ACCENT
    } else {
        Style::ACCENT_WORKLOADS
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(header)
                .color(header_color)
                .size(Style::BODY)
                .strong(),
        );
        if focused {
            mde_egui::muted_note(ui, "\u{2190} linked");
        }
    });

    let ResourcePane { outcome, sort, .. } = pane;
    match outcome {
        None => {
            mde_egui::muted_note(ui, "querying resources\u{2026}");
        }
        Some(ResourceOutcome::NotConfigured(reason)) => {
            mde_egui::muted_note(ui, format!("OpenStack not configured \u{2014} {reason}"));
        }
        Some(ResourceOutcome::Failed(reason)) => {
            ui.colored_label(
                Style::DANGER,
                RichText::new(format!("unreachable \u{2014} {reason}")).size(Style::SMALL),
            );
        }
        Some(ResourceOutcome::Ready(table)) => {
            if table.is_empty() {
                mde_egui::muted_note(ui, "no resources");
            } else {
                render_table(ui, table, sort, &ty, selected);
            }
        }
    }
}

/// Render one sortable resource table (design #15): a leading selection column,
/// a clickable header per column (toggles the sort), then the rows in the sorted
/// order. Header index 0 is the name/id label; 1.. are the value columns.
/// Selection toggles the `(service_type, id)` key in the shared set.
fn render_table(
    ui: &mut egui::Ui,
    table: &ResourceTable,
    sort: &mut Option<(usize, bool)>,
    service_type: &str,
    selected: &mut BTreeSet<(String, String)>,
) {
    // headers[0] is the name column; the rest mirror `table.columns`.
    let headers: Vec<String> = std::iter::once("Name".to_string())
        .chain(table.columns.iter().cloned())
        .collect();
    let cell = |row: &ResourceRow, h: usize| -> String {
        if h == 0 {
            table.row_label(row).to_string()
        } else {
            row.cells.get(h - 1).cloned().unwrap_or_default()
        }
    };

    // The row order under the current sort (stable catalog order when unsorted).
    let mut order: Vec<usize> = (0..table.rows.len()).collect();
    if let Some((col, asc)) = *sort {
        order.sort_by(|&a, &b| {
            let ord = cell(&table.rows[a], col).cmp(&cell(&table.rows[b], col));
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });
    }

    egui::Grid::new((service_type, "iac-resource-table"))
        .striped(true)
        .show(ui, |ui| {
            // Header row: a blank selection cell + a clickable header per column.
            ui.label("");
            for (i, h) in headers.iter().enumerate() {
                let arrow = match *sort {
                    Some((c, true)) if c == i => " \u{25B2}",
                    Some((c, false)) if c == i => " \u{25BC}",
                    _ => "",
                };
                let label = RichText::new(format!("{h}{arrow}"))
                    .size(Style::SMALL)
                    .strong()
                    .color(Style::ACCENT_WORKLOADS);
                if ui.add(egui::Button::new(label).frame(false)).clicked() {
                    *sort = match *sort {
                        Some((c, asc)) if c == i => Some((i, !asc)),
                        _ => Some((i, true)),
                    };
                }
            }
            ui.end_row();

            for &ri in &order {
                let row = &table.rows[ri];
                let key = (service_type.to_string(), row.id.clone());
                let mut sel = selected.contains(&key);
                if ui.checkbox(&mut sel, "").changed() {
                    if sel {
                        selected.insert(key.clone());
                    } else {
                        selected.remove(&key);
                    }
                }
                for i in 0..headers.len() {
                    let color = if i == 0 { Style::TEXT } else { Style::TEXT_DIM };
                    ui.label(RichText::new(cell(row, i)).size(Style::SMALL).color(color));
                }
                ui.end_row();
            }
        });
}

/// The Overview body: the **`OpenStack` API status band** (rich tiles) over the
/// **merged service directory** (the Keystone catalog grouped by type). The
/// "Mesh services" group (mesh/LAN discovery) folds in at IAC-3; here the
/// directory is the `OpenStack` side only (a clean seam, no rendered placeholder).
fn render_overview(ui: &mut egui::Ui, view: &CatalogView, show_urls: bool) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ── the OpenStack API status band ──
            section_header(ui, "OpenStack API status");
            if view.catalog.services.is_empty() {
                mde_egui::muted_note(ui, "the Keystone catalog advertises no services");
            } else {
                ui.horizontal_wrapped(|ui| {
                    for svc in &view.catalog.services {
                        service_tile(ui, svc, view.health_for(svc), show_urls);
                    }
                });
            }

            ui.add_space(Style::SP_M);
            ui.separator();
            ui.add_space(Style::SP_S);

            // ── the merged service directory, grouped by type ──
            section_header(ui, "Service directory");
            render_directory(ui, view);
        });
}

/// A section heading in the shared TITLE tier (§4).
fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .color(Style::TEXT)
            .size(Style::TITLE)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
}

/// One rich status tile for a cataloged service: the health dot + name, the
/// type/bucket, the health state + latency, the version/microversion, the
/// region, and the endpoints (full URLs when `show_urls`, else compact
/// host:port + a count). Every colour + size is a `Style` token (§4).
fn service_tile(
    ui: &mut egui::Ui,
    svc: &CatalogService,
    health: Option<&ServiceHealth>,
    show_urls: bool,
) {
    let (dot, state_label) =
        health.map_or((Style::TEXT_DIM, "unprobed"), |h| health_style(h.state));
    ui.group(|ui| {
        ui.set_width(TILE_W);
        ui.vertical(|ui| {
            // Name + health dot.
            ui.horizontal(|ui| {
                ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(service_display_name(svc))
                        .color(Style::TEXT)
                        .size(Style::BODY)
                        .strong(),
                );
            });
            // Bucket · raw type.
            mde_egui::muted_note(
                ui,
                format!(
                    "{} \u{00B7} {}",
                    service_bucket(&svc.service_type),
                    svc.service_type
                ),
            );
            // Health state + latency.
            let line = health_line(state_label, health.and_then(|h| h.latency_ms));
            ui.colored_label(dot, RichText::new(line).size(Style::SMALL));
            // Version + microversion (never guessed — only when the probe read one).
            if let Some(h) = health {
                let mut meta = Vec::new();
                if let Some(v) = &h.version_id {
                    meta.push(v.clone());
                }
                if let Some(mv) = &h.microversion {
                    meta.push(format!("\u{00B5}v {mv}"));
                }
                if !meta.is_empty() {
                    mde_egui::muted_note(ui, meta.join(" \u{00B7} "));
                }
            }
            // Region (the endpoint's, when advertised).
            if let Some(region) = svc.endpoints.iter().find_map(|e| e.region.as_deref()) {
                mde_egui::muted_note(ui, format!("region {region}"));
            }
            // Endpoints.
            render_tile_endpoints(ui, svc, show_urls);
        });
    });
}

/// The endpoints line(s) of a status tile: every public/internal/admin URL when
/// `show_urls`, else the primary host:port plus an endpoint count.
fn render_tile_endpoints(ui: &mut egui::Ui, svc: &CatalogService, show_urls: bool) {
    if svc.endpoints.is_empty() {
        mde_egui::muted_note(ui, "no endpoints advertised");
        return;
    }
    if show_urls {
        for e in &svc.endpoints {
            mde_egui::muted_note(ui, format!("{} {}", e.interface.as_str(), e.url));
        }
    } else if let Some(url) = svc.primary_url() {
        let n = svc.endpoints.len();
        mde_egui::muted_note(
            ui,
            format!(
                "{} \u{00B7} {n} endpoint{}",
                authority(url),
                if n == 1 { "" } else { "s" }
            ),
        );
    }
}

/// The merged service directory: the Keystone catalog services grouped by type
/// bucket (Compute / Network / …), each a small accent sub-header over its rich
/// rows. Empty buckets are skipped (never an empty header). The mesh/LAN
/// "Mesh services" group folds in at IAC-3.
fn render_directory(ui: &mut egui::Ui, view: &CatalogView) {
    if view.catalog.services.is_empty() {
        mde_egui::muted_note(ui, "no services to list");
        return;
    }
    for bucket in BUCKETS {
        let services: Vec<&CatalogService> = view
            .catalog
            .services
            .iter()
            .filter(|s| service_bucket(&s.service_type) == bucket)
            .collect();
        if services.is_empty() {
            continue;
        }
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(bucket)
                .color(Style::ACCENT_WORKLOADS)
                .size(Style::SMALL)
                .strong(),
        );
        for svc in services {
            directory_row(ui, svc, view.health_for(svc));
        }
    }
}

/// One rich directory row: the health dot + name, the primary endpoint
/// host:port, the health state + latency, and the region — all `Style`-tokened.
fn directory_row(ui: &mut egui::Ui, svc: &CatalogService, health: Option<&ServiceHealth>) {
    let (dot, state_label) =
        health.map_or((Style::TEXT_DIM, "unprobed"), |h| health_style(h.state));
    ui.horizontal(|ui| {
        ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(service_display_name(svc))
                .color(Style::TEXT)
                .size(Style::BODY),
        );
        if let Some(url) = svc.primary_url() {
            ui.add_space(Style::SP_S);
            mde_egui::muted_note(ui, authority(url).to_string());
        }
        ui.add_space(Style::SP_S);
        let line = health_line(state_label, health.and_then(|h| h.latency_ms));
        ui.colored_label(dot, RichText::new(line).size(Style::SMALL));
        if let Some(region) = svc.endpoints.iter().find_map(|e| e.region.as_deref()) {
            ui.add_space(Style::SP_S);
            mde_egui::muted_note(ui, region.to_string());
        }
    });
}

// ─────────────────────────────── pure helpers ───────────────────────────────

/// The service's human name when the catalog carries one, else its type (never
/// guessed / never blank — the type is always present).
fn service_display_name(svc: &CatalogService) -> String {
    svc.name.as_deref().unwrap_or(&svc.service_type).to_string()
}

/// The health state + latency line for a tile / directory row: `"up · 12 ms"`
/// when the probe timed a round-trip, else the bare state label (never a
/// fabricated latency, §7). Shared by the status band + the directory.
fn health_line(state_label: &str, latency_ms: Option<u64>) -> String {
    latency_ms.map_or_else(
        || state_label.to_string(),
        |ms| format!("{state_label} \u{00B7} {ms} ms"),
    )
}

/// The health dot colour + short label for a [`HealthState`] — a `Style` token
/// each (§4). Up is green, down is danger red, absent is dim.
const fn health_style(state: HealthState) -> (Color32, &'static str) {
    match state {
        HealthState::Up => (Style::OK, "up"),
        HealthState::Down => (Style::DANGER, "down"),
        HealthState::Absent => (Style::TEXT_DIM, "absent"),
    }
}

/// The directory bucket a Keystone service **type** groups under (design lock
/// #10). The common `OpenStack` types map to their family; anything else honestly
/// falls to `Other` (a new/unknown service is grouped, never dropped).
fn service_bucket(service_type: &str) -> &'static str {
    match service_type {
        "compute" | "compute_legacy" => "Compute",
        "network" => "Network",
        "image" => "Image",
        "volume" | "volumev2" | "volumev3" | "block-storage" | "block-store" => "Volume",
        "orchestration" | "cloudformation" => "Orchestration",
        "identity" => "Identity",
        "dns" => "DNS",
        "object-store" => "Object Storage",
        "placement" => "Placement",
        _ => "Other",
    }
}

/// The authority (`host:port`) of a URL — the scheme + path stripped, any
/// userinfo dropped. Best-effort string parsing (no URL crate): the catalog URLs
/// are plain `scheme://host:port/path`. Returns the input's host portion, so the
/// listening port is always shown when the URL carries one.
fn authority(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    host.rsplit_once('@').map_or(host, |(_, h)| h)
}

// ─────────────────────────── the MENUBAR-ALL bar ────────────────────────────

/// MENUBAR-ALL (Infra as Code) — the shared bar over the `OpenStack` control
/// plane. Every item is a real seam (§6): the **Catalog** spine (Refresh now /
/// Auto-refresh) + **View** (Show endpoint URLs), then the **catalog-driven
/// per-service menus**, then **Help**. IAC-5 makes the per-service menus **one
/// per advertised Keystone service**, so the bar grows + shrinks with the live
/// catalog (design #17): each carries that service's full landed verb set (Drill
/// / Refresh, Compute's armed lifecycle, the Orchestration menu's folded-in Heat
/// set), the governing-principle headline — comprehensive, yet every item maps to
/// a landed Bus seam; an absent verb is omitted, a context-gated one disabled, a
/// verb-less service an honest caption, never a dead entry (§8). The status
/// cluster reads the live catalog.
mod menubar {
    use std::collections::BTreeSet;

    use super::{
        service_bucket, service_display_name, Arming, CatalogOutcome, HeatOp, IacTab,
        InfraCodeState, BUCKETS, DOT, HEAT_SERVICE,
    };
    use mackes_mesh_types::openstack::default_collection;
    use mde_egui::egui::Ui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};

    /// One menu action — each routes to a real Infra-as-Code seam in [`apply`].
    /// The catalog-driven per-service verbs carry their target (service type /
    /// instance id), so this is `Clone`, not `Copy`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// Force an immediate catalog re-poll (`Catalog → Refresh now`).
        Refresh,
        /// Toggle the ~15 s auto-poll (`Catalog → Auto-refresh`).
        ToggleAuto,
        /// Toggle full endpoint URLs on the tiles (`View → Show endpoint URLs`).
        ToggleUrls,
        /// Open the Resources tab focused on a service (`<Service> → Drill`).
        Drill(String),
        /// Force a re-poll of one service's resource pane (`<Service> → Refresh`).
        RefreshResources(String),
        /// A non-destructive Nova lifecycle op on the selected instance (Start /
        /// Stop) — issues the armed Bus request directly.
        Lifecycle {
            /// The lifecycle verb (`instance-start` / `instance-stop`).
            verb: &'static str,
            /// The target Nova instance id.
            instance_id: String,
            /// Its display name (for the honest action note).
            name: String,
        },
        /// A destructive Nova lifecycle op (Reboot / Delete) — opens the typed-
        /// arming confirm before anything publishes (#22).
        ArmLifecycle {
            /// The destructive verb (`instance-reboot` / `instance-delete`).
            verb: &'static str,
            /// The target Nova instance id.
            instance_id: String,
            /// Its display name — the typed-arming echo.
            name: String,
        },
        /// Heat: refresh the stack list (and re-fetch the open detail).
        HeatRefresh,
        /// Heat: (re)load the selected stack's detail.
        HeatShow,
        /// Heat: preview-update the selected stack with the edited template.
        HeatPreview,
        /// Heat: stack-check (drift) the selected stack.
        HeatCheck,
        /// Heat: reverse-generate a HOT template from live infra.
        HeatReverse,
        /// Heat: open the create-stack form.
        HeatNewStack,
        /// Heat: open the typed-arming confirm for a create / update / delete
        /// (#22).
        HeatArm(HeatOp),
        /// Help → surface the audit + notify posture in the action note (a real
        /// seam — the `note` renders; IAC-5's honest one-liner).
        HelpAbout,
    }

    /// Render the INFRA AS CODE bar and return the action picked this frame
    /// (IAC-5).
    ///
    /// The bar is the **Catalog / View** spine, then the **catalog-driven
    /// per-service menus** (one menu per advertised Keystone service, growing +
    /// shrinking with the live catalog, design #17), then the **Help** menu. The
    /// governing principle (MENUBAR-ALL): every real control — incl. the armed
    /// Nova lifecycle + the full Heat verb set (folded into the Orchestration
    /// service's menu, lock 9 — no separate ad-hoc menu) — is here; a verb with no
    /// landed seam is omitted and a context-gated one disabled, never a dead entry
    /// (§8).
    pub(super) fn show(ui: &mut Ui, state: &InfraCodeState) -> Option<MenuAction> {
        let mut menus = build_menus(state.auto_refresh, state.show_urls);
        menus.extend(build_service_menus(state));
        menus.push(build_help_menu());
        let status = build_status(state);
        let model = MenuBarModel {
            // The dock groups Infra as Code under **Workloads** (purple), so the
            // title wears that categorical accent (design lock #17 / §4).
            title: "Infra as Code",
            accent: Style::ACCENT_WORKLOADS,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// Build the catalog-driven per-service menus (design #17) — **one menu per
    /// advertised Keystone service**, so the bar auto-grows + shrinks as the live
    /// catalog changes (a newly-advertised service automatically gets its menu).
    ///
    /// The known families (Compute / Network / Image / Volume / Orchestration /
    /// Identity / DNS / Object Storage / Placement) render in canonical order,
    /// collapsing their type variants (`volume`/`volumev3` → one **Volume** menu);
    /// the `Other` catch-all fans out **one menu per distinct unknown type**, so a
    /// brand-new service still surfaces its own menu. Each menu carries that
    /// service's full landed verb set ([`service_menu`]) — the governing principle
    /// (surface ALL controls), with honest omit/disable (§8). Empty until the
    /// catalog is [`CatalogOutcome::Ready`].
    fn build_service_menus(state: &InfraCodeState) -> Vec<Menu<MenuAction>> {
        let CatalogOutcome::Ready(view) = &state.outcome else {
            return Vec::new();
        };
        let selected = state.single_selected_instance();
        let has_stack = state.heat.selected.is_some();
        let mut menus = Vec::new();
        for bucket in BUCKETS {
            if bucket == "Other" {
                // Fan out one menu per distinct unknown service type (deduped) — the
                // auto-grow showcase: a service Keystone advertises that isn't a
                // known family still gets its own menu, titled by its identity.
                let mut seen: BTreeSet<&str> = BTreeSet::new();
                for svc in &view.catalog.services {
                    if service_bucket(&svc.service_type) == "Other"
                        && seen.insert(svc.service_type.as_str())
                    {
                        menus.push(service_menu(
                            &svc.service_type,
                            service_display_name(svc),
                            selected.as_ref(),
                            has_stack,
                        ));
                    }
                }
            } else if let Some(svc) = view
                .catalog
                .services
                .iter()
                .find(|s| service_bucket(&s.service_type) == bucket)
            {
                // One menu per known family present in the catalog, titled by the
                // family, targeting the first advertised type in it (they share a
                // resource collection, so any is a valid drill target).
                menus.push(service_menu(
                    &svc.service_type,
                    bucket.to_string(),
                    selected.as_ref(),
                    has_stack,
                ));
            }
        }
        menus
    }

    /// Build one advertised service's menu (design #17) — its **full landed verb
    /// set**, honestly gated (§8):
    ///
    /// - the read seam **Drill into resources** + **Refresh resources** when the
    ///   service has a resource collection (a landed `list-resources` seam);
    /// - for **Compute**, the armed Nova lifecycle verbs on the selected instance
    ///   (Start / Stop direct; Reboot / Delete typed-armed, #22), disabled until
    ///   exactly one instance is selected (context-gated, §7);
    /// - for **Orchestration**, the folded-in full **Heat** verb set (Refresh /
    ///   Reverse-generate / New stack / Show / Preview / Stack-check / Update /
    ///   Delete — lock 9, no separate ad-hoc menu);
    /// - and, for a service with **no** landed management verb (Identity / DNS /
    ///   Placement / Object Storage / an unknown type), a single honest
    ///   [`Entry::Caption`] — the service is advertised, but nothing is wired to
    ///   fake (§8, never a dead/greyed actionable entry).
    fn service_menu(
        ty: &str,
        title: impl Into<String>,
        selected: Option<&(String, String)>,
        has_stack: bool,
    ) -> Menu<MenuAction> {
        let bucket = service_bucket(ty);
        let mut entries = Vec::new();
        // The read seam — omitted (§8) when the service has no resource collection.
        if default_collection(ty).is_some() {
            entries.push(Entry::Item(Item::new(
                MenuAction::Drill(ty.to_string()),
                "Drill into resources",
            )));
            // Orchestration's own Heat set carries "Refresh stacks", so we don't
            // duplicate a generic Refresh there (lock 9).
            if bucket != "Orchestration" {
                entries.push(Entry::Item(Item::new(
                    MenuAction::RefreshResources(ty.to_string()),
                    "Refresh resources",
                )));
            }
        }
        match bucket {
            "Compute" => {
                if !entries.is_empty() {
                    entries.push(Entry::Separator);
                }
                push_lifecycle_verbs(&mut entries, selected);
            }
            "Orchestration" => {
                if !entries.is_empty() {
                    entries.push(Entry::Separator);
                }
                entries.extend(heat_entries(has_stack));
            }
            _ => {}
        }
        if entries.is_empty() {
            // Honest §8 — advertised, but no management verb is wired for its type.
            entries.push(Entry::Caption(
                "No management verbs are wired for this service yet.".to_string(),
            ));
        }
        Menu::new(title, entries)
    }

    /// Push the armed Nova lifecycle verbs (Start / Stop direct; Reboot / Delete
    /// typed-armed, #22) onto a Compute menu. They act on the single selected
    /// instance, so they are disabled (context-gated, §7 — not omitted) when the
    /// selection isn't exactly one.
    fn push_lifecycle_verbs(
        entries: &mut Vec<Entry<MenuAction>>,
        selected: Option<&(String, String)>,
    ) {
        let (enabled, id, name) = selected.map_or_else(
            || (false, String::new(), String::new()),
            |(id, name)| (true, id.clone(), name.clone()),
        );
        for (verb, label) in [
            ("instance-start", "Start instance"),
            ("instance-stop", "Stop instance"),
        ] {
            entries.push(Entry::Item(
                Item::new(
                    MenuAction::Lifecycle {
                        verb,
                        instance_id: id.clone(),
                        name: name.clone(),
                    },
                    label,
                )
                .enabled(enabled),
            ));
        }
        for (verb, label) in [
            ("instance-reboot", "Reboot instance\u{2026}"),
            ("instance-delete", "Delete instance\u{2026}"),
        ] {
            entries.push(Entry::Item(
                Item::new(
                    MenuAction::ArmLifecycle {
                        verb,
                        instance_id: id.clone(),
                        name: name.clone(),
                    },
                    label,
                )
                .enabled(enabled),
            ));
        }
    }

    /// The full native-IaC **Heat** verb set (IAC-4), folded into the
    /// Orchestration service's menu (lock 9): Refresh stacks / Reverse-generate /
    /// New stack, plus the selection-gated Show / Preview / Stack-check / Update /
    /// Delete. The mutating Update / Delete are typed-armed (#22); the reads + the
    /// non-destructive stack-check act directly; Create rides the New-stack form
    /// (armed there). `has_sel` disables the selection-gated verbs (§7 — not
    /// omitted) until a stack is selected.
    fn heat_entries(has_sel: bool) -> Vec<Entry<MenuAction>> {
        vec![
            Entry::Item(Item::new(MenuAction::HeatRefresh, "Refresh stacks")),
            Entry::Item(Item::new(
                MenuAction::HeatReverse,
                "Reverse-generate template",
            )),
            Entry::Item(Item::new(MenuAction::HeatNewStack, "New stack\u{2026}")),
            Entry::Separator,
            Entry::Item(Item::new(MenuAction::HeatShow, "Show / refresh detail").enabled(has_sel)),
            Entry::Item(
                Item::new(MenuAction::HeatPreview, "Preview update (dry-run)\u{2026}")
                    .enabled(has_sel),
            ),
            Entry::Item(Item::new(MenuAction::HeatCheck, "Stack-check (drift)").enabled(has_sel)),
            Entry::Item(
                Item::new(MenuAction::HeatArm(HeatOp::Update), "Update stack\u{2026}")
                    .enabled(has_sel),
            ),
            Entry::Item(
                Item::new(MenuAction::HeatArm(HeatOp::Delete), "Delete stack\u{2026}")
                    .enabled(has_sel),
            ),
        ]
    }

    /// The **Help** menu (the MENUBAR-ALL spine) — an honest surface identity
    /// caption + a real seam: **Audit + notify posture** writes IAC-5's one-line
    /// note (every mutating op audits; notify fires only on failure/service-down),
    /// so even Help carries no dead entry (§8).
    fn build_help_menu() -> Menu<MenuAction> {
        Menu::new(
            "Help",
            vec![
                Entry::Caption(
                    "Infra as Code \u{2014} the OpenStack IaaS control plane.".to_string(),
                ),
                Entry::Item(Item::new(
                    MenuAction::HelpAbout,
                    "Audit + notify posture\u{2026}",
                )),
            ],
        )
    }

    /// Build the Catalog + View menus, reflecting the two live toggles.
    fn build_menus(auto_refresh: bool, show_urls: bool) -> Vec<Menu<MenuAction>> {
        vec![
            Menu::new(
                "Catalog",
                vec![
                    Entry::Item(Item::new(MenuAction::Refresh, "Refresh now")),
                    Entry::Separator,
                    Entry::Item(
                        Item::new(MenuAction::ToggleAuto, "Auto-refresh (15\u{202F}s)")
                            .checked(auto_refresh),
                    ),
                ],
            ),
            Menu::new(
                "View",
                vec![Entry::Item(
                    Item::new(MenuAction::ToggleUrls, "Show endpoint URLs").checked(show_urls),
                )],
            ),
        ]
    }

    /// The live status cluster: N services · M healthy · the region — or the
    /// honest not-configured / unreachable / querying read when there's no
    /// catalog yet (§7).
    fn build_status(state: &InfraCodeState) -> Vec<StatusChip> {
        match &state.outcome {
            CatalogOutcome::Ready(view) => {
                let total = view.catalog.services.len();
                let healthy = view.healthy_count();
                let mut chips = vec![StatusChip::new(
                    format!("{total} service{}", if total == 1 { "" } else { "s" }),
                    ChipTone::Neutral,
                )];
                if total > 0 {
                    let tone = if healthy == total {
                        ChipTone::Ok
                    } else {
                        ChipTone::Warn
                    };
                    chips.push(StatusChip::with_icon(
                        DOT,
                        format!("{healthy} healthy"),
                        tone,
                    ));
                }
                if let Some(region) = &view.catalog.region {
                    chips.push(StatusChip::new(region.clone(), ChipTone::Info));
                }
                chips
            }
            CatalogOutcome::Querying => {
                vec![StatusChip::new("querying\u{2026}", ChipTone::Neutral)]
            }
            CatalogOutcome::NotConfigured(_) => {
                vec![StatusChip::with_icon(DOT, "not configured", ChipTone::Warn)]
            }
            CatalogOutcome::Failed(_) => {
                vec![StatusChip::with_icon(DOT, "unreachable", ChipTone::Danger)]
            }
        }
    }

    /// Apply a picked action to its real seam (§6). Refresh queues one immediate
    /// request (clearing any in-flight one so it fires on the next poll); the two
    /// toggles flip the matching view/poll state.
    pub(super) fn apply(state: &mut InfraCodeState, action: MenuAction) {
        match action {
            MenuAction::Refresh => {
                state.forced = true;
                state.pending = None;
            }
            MenuAction::ToggleAuto => state.auto_refresh = !state.auto_refresh,
            MenuAction::ToggleUrls => state.show_urls = !state.show_urls,
            MenuAction::Drill(ty) => {
                state.tab = IacTab::Resources;
                state.linked_focus = Some(ty);
            }
            MenuAction::RefreshResources(ty) => {
                let pane = state.resources.entry(ty).or_default();
                pane.forced = true;
                pane.pending = None;
            }
            // Start / Stop are non-destructive — issue the armed request directly.
            MenuAction::Lifecycle {
                verb,
                instance_id,
                name,
            } => state.issue_lifecycle(verb, &instance_id, &name),
            // Reboot / Delete open the typed-arming confirm before anything
            // publishes (#22) — nothing reaches the Bus until the name is typed.
            MenuAction::ArmLifecycle {
                verb,
                instance_id,
                name,
            } => {
                state.arming = Some(Arming {
                    verb,
                    instance_id,
                    target_name: name,
                    typed: String::new(),
                });
            }
            // ── Heat (IAC-4) — every item maps to a real seam (§6). ──
            MenuAction::HeatRefresh => {
                if let Some(pane) = state.resources.get_mut(HEAT_SERVICE) {
                    pane.forced = true;
                    pane.pending = None;
                }
                state.heat.show_for = None;
                state.tab = IacTab::Heat;
            }
            MenuAction::HeatShow => {
                state.heat.show_for = None;
                state.tab = IacTab::Heat;
            }
            MenuAction::HeatPreview => {
                state.tab = IacTab::Heat;
                state.send_heat_preview();
            }
            MenuAction::HeatCheck => {
                state.tab = IacTab::Heat;
                state.issue_heat_check();
            }
            MenuAction::HeatReverse => {
                state.tab = IacTab::Heat;
                state.send_heat_reverse();
            }
            MenuAction::HeatNewStack => {
                state.tab = IacTab::Heat;
                state.heat.show_create = true;
            }
            // The armed Heat mutations open the typed-arming confirm (#22).
            MenuAction::HeatArm(HeatOp::Update) => state.arm_heat_update(),
            MenuAction::HeatArm(HeatOp::Delete) => state.arm_heat_delete(),
            MenuAction::HeatArm(HeatOp::Create) => state.arm_heat_create(),
            // Help → the honest IAC-5 audit + notify posture, surfaced in the note.
            MenuAction::HelpAbout => {
                state.note = Some(
                    "Every mutating op is audited to the KDC hash-chained log; the mesh notify \
                     feed fires only on a failure or a service going down."
                        .to_string(),
                );
            }
        }
    }

    #[cfg(test)]
    #[allow(clippy::panic)]
    mod tests {
        use super::super::tests::{fixture_view, heat_view};
        use super::super::{CatalogOutcome, HeatOp, InfraCodeState, HEAT_SERVICE};
        use super::{
            apply, build_help_menu, build_menus, build_service_menus, build_status, MenuAction,
        };
        use mde_egui::menubar::{Entry, Item, Menu};
        use mde_egui::ChipTone;

        /// The service menu titled `title`, if the generator built one.
        fn menu<'a>(menus: &'a [Menu<MenuAction>], title: &str) -> Option<&'a Menu<MenuAction>> {
            menus.iter().find(|m| m.title == title)
        }

        /// The item ids of a menu, in order.
        fn menu_action_ids(menu: &super::Menu<MenuAction>) -> Vec<MenuAction> {
            menu.entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some(i.id.clone()),
                    _ => None,
                })
                .collect()
        }

        #[test]
        fn orchestration_menu_folds_in_the_full_heat_verb_set_and_is_absent_without_it() {
            // §8 / lock 9 — the Heat verb set lives in the Orchestration service
            // menu, not a separate ad-hoc menu. No orchestration service ⇒ no
            // Orchestration menu at all (omitted, not dead).
            let no_heat = InfraCodeState {
                outcome: CatalogOutcome::Ready(fixture_view()),
                ..InfraCodeState::default()
            };
            assert!(
                menu(&build_service_menus(&no_heat), "Orchestration").is_none(),
                "no orchestration service ⇒ no Orchestration menu"
            );

            // Orchestration cataloged ⇒ its menu carries Drill + the full Heat set.
            let mut state = InfraCodeState {
                outcome: CatalogOutcome::Ready(heat_view()),
                ..InfraCodeState::default()
            };
            let menus = build_service_menus(&state);
            let orch = menu(&menus, "Orchestration")
                .expect("Orchestration menu")
                .clone();
            let ids = menu_action_ids(&orch);
            for want in [
                MenuAction::Drill(HEAT_SERVICE.to_string()),
                MenuAction::HeatRefresh,
                MenuAction::HeatReverse,
                MenuAction::HeatNewStack,
                MenuAction::HeatShow,
                MenuAction::HeatPreview,
                MenuAction::HeatCheck,
                MenuAction::HeatArm(HeatOp::Update),
                MenuAction::HeatArm(HeatOp::Delete),
            ] {
                assert!(
                    ids.contains(&want),
                    "the Orchestration menu is missing {want:?}"
                );
            }

            // The selection-gated verbs are disabled (context-gated, §7) — not
            // omitted — until a stack is selected.
            let show = orch
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::HeatShow => Some(i),
                    _ => None,
                })
                .expect("Show item");
            assert!(!show.enabled, "Show is disabled with no selection");

            // With a selection they enable, and the armed verbs open the confirm.
            state.heat.selected = Some(("s-1".to_string(), "mesh-net".to_string()));
            let menus2 = build_service_menus(&state);
            let orch2 = menu(&menus2, "Orchestration").expect("Orchestration menu");
            let show2 = orch2
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::HeatShow => Some(i),
                    _ => None,
                })
                .expect("Show item");
            assert!(show2.enabled, "Show enables once a stack is selected");
            apply(&mut state, MenuAction::HeatArm(HeatOp::Delete));
            assert!(
                state.heat.arming.is_some(),
                "the Delete menu verb opens the typed-arming confirm"
            );
        }

        #[test]
        fn service_menus_are_catalog_driven_and_carry_the_verb_set() {
            // The fixture catalog = compute + identity + image; compute & image are
            // drillable, identity is not (no resource collection).
            let state = InfraCodeState {
                outcome: CatalogOutcome::Ready(fixture_view()),
                ..InfraCodeState::default()
            };
            let menus = build_service_menus(&state);
            let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
            // #17 — one menu per advertised service, incl. the verb-less Identity.
            assert!(
                titles.contains(&"Compute")
                    && titles.contains(&"Image")
                    && titles.contains(&"Identity"),
                "every advertised service gets a menu (auto-grow); got {titles:?}"
            );

            // A non-compute drillable service carries just the two read verbs.
            let image = menu(&menus, "Image").expect("Image menu");
            assert_eq!(image.entries.len(), 2, "Drill + Refresh only");

            // A verb-less service (identity) gets an honest caption, not a dead
            // actionable entry (§8) — no activatable items at all.
            let identity = menu(&menus, "Identity").expect("Identity menu");
            assert!(
                matches!(identity.entries.as_slice(), [Entry::Caption(_)]),
                "a verb-less service shows one honest caption, no items"
            );

            // Compute carries the read verbs + the four armed lifecycle verbs,
            // disabled while nothing is selected (context-gated, §7 — not omitted).
            let compute = menus
                .iter()
                .find(|m| m.title == "Compute")
                .expect("Compute menu");
            let items: Vec<&Item<MenuAction>> = compute
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some(i),
                    _ => None,
                })
                .collect();
            assert_eq!(items.len(), 6, "Drill + Refresh + Start/Stop/Reboot/Delete");
            let is_lifecycle = |a: &MenuAction| {
                matches!(
                    a,
                    MenuAction::Lifecycle { .. } | MenuAction::ArmLifecycle { .. }
                )
            };
            assert_eq!(items.iter().filter(|i| is_lifecycle(&i.id)).count(), 4);
            assert!(
                items
                    .iter()
                    .filter(|i| is_lifecycle(&i.id))
                    .all(|i| !i.enabled),
                "the lifecycle verbs are disabled until exactly one instance is selected"
            );
            // Delete is a typed-armed verb (ArmLifecycle), present in the menu.
            assert!(items.iter().any(|i| i.id
                == MenuAction::ArmLifecycle {
                    verb: "instance-delete",
                    instance_id: String::new(),
                    name: String::new(),
                }));
        }

        /// A `CatalogView` over a Keystone token, no health rows.
        fn view_from(token: &str) -> super::super::CatalogView {
            super::super::CatalogView {
                catalog: mackes_mesh_types::openstack::ServiceCatalog::from_keystone_token_json(
                    token,
                )
                .expect("fixture catalog"),
                health: Vec::new(),
            }
        }

        #[test]
        fn the_bar_auto_grows_one_menu_per_advertised_service() {
            // #17 — the bar carries exactly one menu per advertised service and
            // grows as the catalog advertises more.
            let two = view_from(
                r#"{"token":{"catalog":[
                    {"type":"compute","name":"nova","endpoints":[{"interface":"public","url":"http://n:8774/v2.1","region":"R"}]},
                    {"type":"network","name":"neutron","endpoints":[{"interface":"public","url":"http://n:9696","region":"R"}]}
                ]}}"#,
            );
            let pair = InfraCodeState {
                outcome: CatalogOutcome::Ready(two),
                ..InfraCodeState::default()
            };
            let pair_menus = build_service_menus(&pair);
            let titles: Vec<&str> = pair_menus.iter().map(|m| m.title.as_str()).collect();
            assert_eq!(titles, vec!["Compute", "Network"], "one menu per service");

            // Advertise a third service → the bar grows a menu for it automatically.
            let three = view_from(
                r#"{"token":{"catalog":[
                    {"type":"compute","name":"nova","endpoints":[{"interface":"public","url":"http://n:8774/v2.1","region":"R"}]},
                    {"type":"network","name":"neutron","endpoints":[{"interface":"public","url":"http://n:9696","region":"R"}]},
                    {"type":"image","name":"glance","endpoints":[{"interface":"public","url":"http://g:9292","region":"R"}]}
                ]}}"#,
            );
            let grown = InfraCodeState {
                outcome: CatalogOutcome::Ready(three),
                ..InfraCodeState::default()
            };
            let grown_menus = build_service_menus(&grown);
            assert_eq!(
                grown_menus.len(),
                3,
                "the bar grew a menu for the new service"
            );
            assert!(
                menu(&grown_menus, "Image").is_some(),
                "the newly-advertised Image service got its own menu"
            );

            // An unknown service type still fans out its own menu (the Other bucket
            // is per-type, not a single collapsed catch-all).
            let unknown = view_from(
                r#"{"token":{"catalog":[
                    {"type":"metering","name":"ceilometer","endpoints":[{"interface":"public","url":"http://m:8777","region":"R"}]},
                    {"type":"sharev2","name":"manila","endpoints":[{"interface":"public","url":"http://s:8786","region":"R"}]}
                ]}}"#,
            );
            let exotic = InfraCodeState {
                outcome: CatalogOutcome::Ready(unknown),
                ..InfraCodeState::default()
            };
            assert_eq!(
                build_service_menus(&exotic).len(),
                2,
                "two unknown types ⇒ two distinct menus"
            );
        }

        #[test]
        fn help_menu_carries_a_real_seam_not_a_dead_entry() {
            // The Help spine (§8) — a caption + one real action; no dead entry.
            let help = build_help_menu();
            assert_eq!(help.title, "Help");
            assert_eq!(menu_action_ids(&help), vec![MenuAction::HelpAbout]);
            // The action drives a real seam — the audit/notify posture note.
            let mut state = InfraCodeState::default();
            assert!(state.note.is_none());
            apply(&mut state, MenuAction::HelpAbout);
            assert!(
                state.note.as_deref().is_some_and(|n| n.contains("audited")),
                "Help surfaces the audit + notify posture (a real seam)"
            );
        }

        #[test]
        fn the_two_toggles_track_state() {
            // The Auto-refresh + Show-URLs items are checkable and mirror state.
            let checked = |auto: bool, urls: bool| {
                let menus = build_menus(auto, urls);
                let auto_item = match &menus[0].entries[2] {
                    Entry::Item(i) => i.checked,
                    _ => panic!("Catalog[2] is the auto-refresh toggle"),
                };
                let url_item = match &menus[1].entries[0] {
                    Entry::Item(i) => i.checked,
                    _ => panic!("View[0] is the show-URLs toggle"),
                };
                (auto_item, url_item)
            };
            assert_eq!(checked(true, false), (Some(true), Some(false)));
            assert_eq!(checked(false, true), (Some(false), Some(true)));
        }

        #[test]
        fn apply_flips_the_real_seams() {
            let mut state = InfraCodeState::default();
            assert!(state.auto_refresh && !state.show_urls);
            apply(&mut state, MenuAction::ToggleAuto);
            apply(&mut state, MenuAction::ToggleUrls);
            assert!(!state.auto_refresh && state.show_urls);
            // Refresh queues an immediate request + drops any in-flight one.
            apply(&mut state, MenuAction::Refresh);
            assert!(state.forced, "Refresh queues a re-poll");
            assert!(state.pending.is_none());
        }

        #[test]
        fn status_counts_services_and_healthy_from_the_live_catalog() {
            let state = InfraCodeState {
                outcome: CatalogOutcome::Ready(fixture_view()),
                ..InfraCodeState::default()
            };
            let chips = build_status(&state);
            // The fixture catalogs three services; compute + identity probe up.
            assert!(chips.iter().any(|c| c.text == "3 services"));
            assert!(chips
                .iter()
                .any(|c| c.text == "2 healthy" && c.tone == ChipTone::Warn));
            assert!(chips.iter().any(|c| c.text == "RegionOne"));
        }

        #[test]
        fn status_reads_honestly_when_not_configured_or_unreachable() {
            let not_configured = InfraCodeState {
                outcome: CatalogOutcome::NotConfigured("no clouds.yaml on node-a".to_string()),
                ..InfraCodeState::default()
            };
            let chips = build_status(&not_configured);
            assert!(chips
                .iter()
                .any(|c| c.text == "not configured" && c.tone == ChipTone::Warn));

            let failed = InfraCodeState {
                outcome: CatalogOutcome::Failed("keystone auth failed".to_string()),
                ..InfraCodeState::default()
            };
            assert!(build_status(&failed)
                .iter()
                .any(|c| c.text == "unreachable" && c.tone == ChipTone::Danger));
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use mackes_mesh_types::openstack::{shape_health, ProbeOutcome};
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A realistic Keystone v3 token catalog — a three-interface compute service,
    /// a single-interface identity service, and an image service (mirrors the
    /// shared crate's fixture, so the surface is exercised against the real shape).
    const V3_TOKEN: &str = r#"{
      "token": {
        "catalog": [
          {
            "type": "compute", "name": "nova",
            "endpoints": [
              {"interface": "public",   "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne"},
              {"interface": "internal", "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne"},
              {"interface": "admin",    "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne"}
            ]
          },
          {
            "type": "identity", "name": "keystone",
            "endpoints": [
              {"interface": "public", "url": "http://keystone.mesh:5000/v3", "region": "RegionOne"}
            ]
          },
          {
            "type": "image", "name": "glance",
            "endpoints": [
              {"interface": "public", "url": "http://glance.mesh:9292", "region": "RegionOne"}
            ]
          }
        ]
      }
    }"#;

    /// A fixture view: the real catalog + health rows where compute + identity
    /// probe **up** and image probes **down** (2 of 3 healthy) — so the render +
    /// the status counts are exercised over a mixed-health directory.
    pub(super) fn fixture_view() -> CatalogView {
        let catalog = ServiceCatalog::from_keystone_token_json(V3_TOKEN).expect("fixture catalog");
        let up = |ty: &str, url: &str| {
            shape_health(
                ty,
                EndpointInterface::Public,
                url,
                &ProbeOutcome::Reachable {
                    http_status: 200,
                    body: String::new(),
                    elapsed_ms: 12,
                },
            )
        };
        let health = vec![
            up("compute", "http://nova.mesh:8774/v2.1"),
            up("identity", "http://keystone.mesh:5000/v3"),
            shape_health(
                "image",
                EndpointInterface::Public,
                "http://glance.mesh:9292",
                &ProbeOutcome::Unreachable {
                    elapsed_ms: 2000,
                    reason: "connection refused".to_string(),
                },
            ),
        ];
        CatalogView { catalog, health }
    }

    /// Drive one headless frame of `infra_code_panel` and tessellate it on the CPU
    /// (the DRM runner's path minus the GPU). Returns whether it drew primitives.
    fn run_panel(state: &mut InfraCodeState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| infra_code_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn the_surface_is_reachable_in_the_dock() {
        // §7 reachability: the surface is in Surface::ALL and wears the server /
        // infrastructure brand glyph (the group membership is pinned by dock.rs).
        use crate::dock::Surface;
        assert!(Surface::ALL.contains(&Surface::InfraCode));
        assert_eq!(
            Surface::InfraCode.icon_id(),
            mde_theme::brand::icons::IconId::Server
        );
    }

    #[test]
    fn overview_renders_from_a_fixture_catalog() {
        let mut state = InfraCodeState {
            outcome: CatalogOutcome::Ready(fixture_view()),
            ..InfraCodeState::default()
        };
        assert!(
            run_panel(&mut state),
            "the Overview (status band + directory) produced no draw primitives"
        );
        // Expanding the endpoint URLs still tessellates cleanly.
        state.show_urls = true;
        assert!(run_panel(&mut state), "the URL-expanded tiles drew nothing");
    }

    #[test]
    fn the_honest_not_configured_state_renders() {
        // A node with no clouds.yaml reads "not configured", never fake data (§7).
        let mut state = InfraCodeState {
            outcome: CatalogOutcome::NotConfigured("no clouds.yaml on this node".to_string()),
            ..InfraCodeState::default()
        };
        assert!(
            run_panel(&mut state),
            "the not-configured empty state produced no draw primitives"
        );
    }

    #[test]
    fn the_querying_and_failed_states_render() {
        let mut querying = InfraCodeState::default();
        assert!(matches!(querying.outcome, CatalogOutcome::Querying));
        assert!(run_panel(&mut querying), "the querying state drew nothing");

        let mut failed = InfraCodeState {
            outcome: CatalogOutcome::Failed("keystone auth failed".to_string()),
            ..InfraCodeState::default()
        };
        assert!(run_panel(&mut failed), "the failed state drew nothing");
    }

    #[test]
    fn fold_reply_maps_the_reply_tri_state_honestly() {
        // A successful reply (the real wire shape mackesd emits) folds to Ready.
        let view = fixture_view();
        let ok_body = serde_json::json!({
            "ok": true,
            "verb": "get-catalog",
            "audited": false,
            "catalog": view.catalog,
            "health": view.health,
        })
        .to_string();
        let reply: CatalogReply = serde_json::from_str(&ok_body).expect("ok reply parses");
        match fold_reply(reply) {
            CatalogOutcome::Ready(v) => {
                assert_eq!(v.catalog.services.len(), 3);
                assert_eq!(v.healthy_count(), 2);
            }
            other => panic!("an ok reply must fold to Ready, got {other:?}"),
        }

        // A gated reply → NotConfigured (the honest "no clouds.yaml").
        let gated: CatalogReply = serde_json::from_str(
            r#"{"ok":false,"verb":"get-catalog","audited":false,"gated":"no clouds.yaml on node-a"}"#,
        )
        .expect("gated reply parses");
        assert!(matches!(
            fold_reply(gated),
            CatalogOutcome::NotConfigured(r) if r.contains("clouds.yaml")
        ));

        // An error reply → Failed.
        let errored: CatalogReply = serde_json::from_str(
            r#"{"ok":false,"verb":"get-catalog","audited":false,"error":"keystone auth failed"}"#,
        )
        .expect("error reply parses");
        assert!(matches!(
            fold_reply(errored),
            CatalogOutcome::Failed(r) if r.contains("auth failed")
        ));

        // An `ok` reply with no directory is a failure, never a fabricated empty
        // catalog (§7).
        let empty: CatalogReply =
            serde_json::from_str(r#"{"ok":true,"verb":"get-catalog","audited":false}"#)
                .expect("bare ok reply parses");
        assert!(matches!(fold_reply(empty), CatalogOutcome::Failed(_)));
    }

    #[test]
    fn services_group_into_buckets_by_type() {
        assert_eq!(service_bucket("compute"), "Compute");
        assert_eq!(service_bucket("network"), "Network");
        assert_eq!(service_bucket("image"), "Image");
        assert_eq!(service_bucket("volumev3"), "Volume");
        assert_eq!(service_bucket("orchestration"), "Orchestration");
        assert_eq!(service_bucket("identity"), "Identity");
        assert_eq!(service_bucket("object-store"), "Object Storage");
        // An unknown/new service type is grouped honestly, never dropped.
        assert_eq!(service_bucket("load-balancer"), "Other");
        // Every bucket a service can map to is one of the rendered BUCKETS.
        for ty in ["compute", "network", "image", "volumev3", "dns", "weird"] {
            assert!(BUCKETS.contains(&service_bucket(ty)));
        }
    }

    #[test]
    fn health_for_prefers_the_public_interface() {
        let view = fixture_view();
        let compute = view.catalog.service("compute").expect("compute");
        let health = view.health_for(compute).expect("compute health");
        assert_eq!(health.interface, EndpointInterface::Public);
        assert_eq!(health.state, HealthState::Up);
        // A service with no health row reads unprobed (None), never a faked up.
        let mut bare = view.clone();
        bare.health.clear();
        assert!(bare.health_for(compute).is_none());
    }

    #[test]
    fn authority_extracts_host_and_port() {
        assert_eq!(authority("http://nova.mesh:8774/v2.1"), "nova.mesh:8774");
        assert_eq!(
            authority("https://keystone.mesh:5000/v3"),
            "keystone.mesh:5000"
        );
        assert_eq!(
            authority("http://user@glance.mesh:9292"),
            "glance.mesh:9292"
        );
        assert_eq!(authority("glance.mesh:9292"), "glance.mesh:9292");
    }

    // ─────────────────────────── IAC-3: Resources tab ───────────────────────────

    /// A two-row Nova compute table — the fixture the Resources tab renders.
    pub(super) fn fixture_resource_table() -> ResourceTable {
        ResourceTable::from_collection_json(
            "compute",
            "servers/detail",
            r#"{"servers":[
                {"id":"i-1","name":"web","status":"ACTIVE"},
                {"id":"i-2","name":"db","status":"SHUTOFF"}
            ]}"#,
        )
        .expect("fixture table")
    }

    /// A surface state on the Resources tab over the fixture catalog, with the
    /// compute pane populated (`ready` = its resource table landed).
    fn resources_state(ready: bool) -> InfraCodeState {
        let mut state = InfraCodeState {
            outcome: CatalogOutcome::Ready(fixture_view()),
            tab: IacTab::Resources,
            ..InfraCodeState::default()
        };
        if ready {
            state.resources.insert(
                "compute".to_string(),
                ResourcePane {
                    outcome: Some(ResourceOutcome::Ready(fixture_resource_table())),
                    ..ResourcePane::default()
                },
            );
        }
        state
    }

    #[test]
    fn the_tab_bar_switches_and_the_heat_tab_is_an_honest_empty_state() {
        // The three tabs render; the default is Overview (IAC-2 render).
        let mut state = InfraCodeState {
            outcome: CatalogOutcome::Ready(fixture_view()),
            ..InfraCodeState::default()
        };
        assert_eq!(state.tab, IacTab::Overview);
        assert!(run_panel(&mut state), "Overview drew nothing");
        // Heat is an honest forward-looking empty state (not a disabled tab, §7).
        state.tab = IacTab::Heat;
        assert!(run_panel(&mut state), "the Heat empty state drew nothing");
    }

    #[test]
    fn resources_renders_honestly_empty_with_no_reply_and_rows_with_one() {
        // Resources tab, catalog Ready, but no pane reply yet → honest "querying"
        // per service, never fabricated rows (§7).
        let mut empty = resources_state(false);
        assert!(
            run_panel(&mut empty),
            "the querying Resources tab drew nothing"
        );
        // A landed fixture list-resources reply renders the rows.
        let mut ready = resources_state(true);
        assert!(
            run_panel(&mut ready),
            "the populated Resources table drew nothing"
        );
        // Selecting a row + re-render (bulk selection is a real toggle set).
        ready
            .selected
            .insert(("compute".to_string(), "i-1".to_string()));
        assert!(
            run_panel(&mut ready),
            "the selected-row render drew nothing"
        );
    }

    #[test]
    fn resources_reads_honestly_when_the_catalog_is_absent() {
        // Until the catalog answers, the Resources tab reads the same honest
        // catalog-absent story as the Overview (never an empty table of nothing).
        let mut not_configured = InfraCodeState {
            outcome: CatalogOutcome::NotConfigured("no clouds.yaml".to_string()),
            tab: IacTab::Resources,
            ..InfraCodeState::default()
        };
        assert!(run_panel(&mut not_configured), "drew nothing");
    }

    #[test]
    fn fold_resource_reply_maps_the_reply_tri_state_honestly() {
        let table = fixture_resource_table();
        let ok_body = serde_json::json!({
            "ok": true, "verb": "list-resources", "audited": false, "resources": table,
        })
        .to_string();
        let reply: CatalogReply = serde_json::from_str(&ok_body).expect("ok reply parses");
        match fold_resource_reply(reply) {
            ResourceOutcome::Ready(t) => assert_eq!(t.rows.len(), 2),
            other => panic!("an ok reply with a table must fold to Ready, got {other:?}"),
        }
        // A gated reply → NotConfigured; an error → Failed; ok-with-no-table →
        // Failed (never a fabricated empty table).
        let gated: CatalogReply = serde_json::from_str(
            r#"{"ok":false,"verb":"list-resources","audited":false,"gated":"no clouds.yaml"}"#,
        )
        .unwrap();
        assert!(matches!(
            fold_resource_reply(gated),
            ResourceOutcome::NotConfigured(_)
        ));
        let errored: CatalogReply = serde_json::from_str(
            r#"{"ok":false,"verb":"list-resources","audited":false,"error":"HTTP 500"}"#,
        )
        .unwrap();
        assert!(matches!(
            fold_resource_reply(errored),
            ResourceOutcome::Failed(r) if r.contains("500")
        ));
        let bare: CatalogReply =
            serde_json::from_str(r#"{"ok":true,"verb":"list-resources","audited":false}"#).unwrap();
        assert!(matches!(
            fold_resource_reply(bare),
            ResourceOutcome::Failed(_)
        ));
    }

    #[test]
    fn typed_arming_blocks_an_unconfirmed_mutation() {
        // The arming gate: only an exact (trimmed) name match arms the mutation.
        assert!(armed("web", "web"));
        assert!(armed("  web ", "web"), "surrounding space is tolerated");
        assert!(!armed("we", "web"), "a partial echo does not arm");
        assert!(!armed("", "web"), "an empty echo does not arm");

        // Applying a destructive verb OPENS the typed-arming confirm — it does
        // NOT publish anything (no note, no Bus request) until the name is typed.
        let mut state = resources_state(true);
        state
            .selected
            .insert(("compute".to_string(), "i-1".to_string()));
        menubar::apply(
            &mut state,
            menubar::MenuAction::ArmLifecycle {
                verb: "instance-delete",
                instance_id: "i-1".to_string(),
                name: "web".to_string(),
            },
        );
        let arming = state
            .arming
            .as_ref()
            .expect("delete opens the arming confirm");
        assert_eq!(arming.verb, "instance-delete");
        assert_eq!(arming.target_name, "web");
        assert!(arming.typed.is_empty());
        assert!(
            state.note.is_none(),
            "an unconfirmed mutation publishes nothing (no action note)"
        );
    }

    #[test]
    fn drill_and_refresh_menu_actions_drive_their_real_seams() {
        let mut state = resources_state(false);
        state.tab = IacTab::Overview;
        // Drill switches to Resources + focuses the service (the linked view).
        menubar::apply(
            &mut state,
            menubar::MenuAction::Drill("network".to_string()),
        );
        assert_eq!(state.tab, IacTab::Resources);
        assert_eq!(state.linked_focus.as_deref(), Some("network"));
        // Refresh queues an immediate re-poll of that service's pane.
        menubar::apply(
            &mut state,
            menubar::MenuAction::RefreshResources("compute".to_string()),
        );
        assert!(state.resources.get("compute").expect("pane").forced);
    }

    #[test]
    fn single_selected_instance_is_some_only_for_exactly_one_compute_row() {
        let mut state = resources_state(true);
        assert!(state.single_selected_instance().is_none(), "none selected");
        state
            .selected
            .insert(("compute".to_string(), "i-1".to_string()));
        // Resolves the name from the compute pane's table.
        assert_eq!(
            state.single_selected_instance(),
            Some(("i-1".to_string(), "web".to_string()))
        );
        // A second compute selection makes the destructive target ambiguous → None.
        state
            .selected
            .insert(("compute".to_string(), "i-2".to_string()));
        assert!(state.single_selected_instance().is_none(), "two selected");
    }

    // ─────────────────────────── IAC-4: Heat tab ───────────────────────────

    /// A catalog view that advertises orchestration (Heat) — so the Heat tab is
    /// live — plus compute (a reverse-generate source).
    pub(super) fn heat_view() -> CatalogView {
        let catalog = ServiceCatalog::from_keystone_token_json(
            r#"{"token":{"catalog":[
                {"type":"orchestration","name":"heat","endpoints":[
                    {"interface":"public","url":"http://heat.mesh:8004/v1/p","region":"RegionOne"}
                ]},
                {"type":"compute","name":"nova","endpoints":[
                    {"interface":"public","url":"http://nova.mesh:8774/v2.1","region":"RegionOne"}
                ]}
            ]}}"#,
        )
        .expect("heat catalog");
        CatalogView {
            catalog,
            health: vec![],
        }
    }

    /// A fixture Heat stack list (the orchestration `list-resources` table).
    fn fixture_stack_table() -> ResourceTable {
        ResourceTable::from_collection_json(
            "orchestration",
            "stacks",
            r#"{"stacks":[
                {"id":"s-1","stack_name":"mesh-net","stack_status":"CREATE_COMPLETE"},
                {"id":"s-2","stack_name":"web","stack_status":"UPDATE_COMPLETE"}
            ]}"#,
        )
        .expect("fixture stacks")
    }

    /// A fixture stack detail with resources, events, outputs, and a template.
    fn fixture_stack_detail() -> HeatStackDetail {
        HeatStackDetail::from_stack_json(
            r#"{"stack":{"id":"s-1","stack_name":"mesh-net","stack_status":"CREATE_COMPLETE",
                "stack_status_reason":"Stack CREATE completed successfully",
                "outputs":[{"output_key":"net_id","output_value":"n-9","description":"the net id"}]}}"#,
        )
        .unwrap()
        .with_resources_json(
            r#"{"resources":[{"resource_name":"net","resource_type":"OS::Neutron::Net","resource_status":"CREATE_COMPLETE","physical_resource_id":"n-9"}]}"#,
        )
        .with_events_json(
            r#"{"events":[{"event_time":"2026-07-05T00:00:00Z","resource_name":"net","resource_status":"CREATE_COMPLETE","resource_status_reason":"state changed"}]}"#,
        )
        .with_template_json(r#"{"heat_template_version":"2021-04-16","resources":{}}"#)
    }

    /// A Heat-tab state over the Heat catalog with the stack list ready +,
    /// optionally, the selected stack's detail + editable buffer loaded.
    fn heat_tab_state(with_detail: bool) -> InfraCodeState {
        let mut state = InfraCodeState {
            outcome: CatalogOutcome::Ready(heat_view()),
            tab: IacTab::Heat,
            ..InfraCodeState::default()
        };
        state.resources.insert(
            "orchestration".to_string(),
            ResourcePane {
                outcome: Some(ResourceOutcome::Ready(fixture_stack_table())),
                ..ResourcePane::default()
            },
        );
        if with_detail {
            let detail = fixture_stack_detail();
            state.heat.template_buf = detail.template.clone();
            state.heat.template_for = Some("s-1".to_string());
            state.heat.selected = Some(("s-1".to_string(), "mesh-net".to_string()));
            state.heat.show_for = Some("s-1".to_string());
            state.heat.detail = Some(HeatOutcome::Ready(detail));
        }
        state
    }

    #[test]
    fn the_heat_tab_renders_honestly_empty_with_no_reply_and_a_list_with_one() {
        // No orchestration service in the catalog → an honest "no Heat", never a
        // fabricated engine (§7).
        let mut no_heat = InfraCodeState {
            outcome: CatalogOutcome::Ready(fixture_view()),
            tab: IacTab::Heat,
            ..InfraCodeState::default()
        };
        assert!(run_panel(&mut no_heat), "the no-Heat state drew nothing");
        // Heat cataloged but no stack reply yet → honest querying, no fake stacks.
        let mut querying = InfraCodeState {
            outcome: CatalogOutcome::Ready(heat_view()),
            tab: IacTab::Heat,
            ..InfraCodeState::default()
        };
        assert!(
            run_panel(&mut querying),
            "the querying Heat tab drew nothing"
        );
        // A landed stack list renders the table.
        let mut ready = heat_tab_state(false);
        assert!(run_panel(&mut ready), "the stack list drew nothing");
    }

    #[test]
    fn a_fixture_heat_show_renders_resources_events_outputs_and_template() {
        let mut state = heat_tab_state(true);
        // The fixture detail carries each section (proves the fold + the render).
        match &state.heat.detail {
            Some(HeatOutcome::Ready(d)) => {
                assert_eq!(d.resources.len(), 1);
                assert_eq!(d.events.len(), 1);
                assert_eq!(d.outputs.len(), 1);
                assert!(d.template.contains("heat_template_version"));
            }
            other => panic!("expected a ready detail, got {other:?}"),
        }
        assert!(
            run_panel(&mut state),
            "the stack detail (resources/events/outputs/template) drew nothing"
        );
    }

    #[test]
    fn the_preview_update_diff_renders_a_fixture_diff() {
        let mut state = heat_tab_state(true);
        state.heat.preview = Some(HeatOutcome::Ready(HeatPreview {
            added: vec!["new_net".to_string()],
            replaced: vec!["server".to_string()],
            unchanged: vec!["router".to_string()],
            ..HeatPreview::default()
        }));
        assert!(run_panel(&mut state), "the preview diff drew nothing");
        // A no-change diff renders honestly too.
        state.heat.preview = Some(HeatOutcome::Ready(HeatPreview::default()));
        assert!(run_panel(&mut state), "the no-change preview drew nothing");
    }

    #[test]
    fn typed_arming_blocks_an_unconfirmed_stack_delete() {
        // The arming gate is the shared exact-name match.
        assert!(armed("mesh-net", "mesh-net"));
        assert!(!armed("mesh", "mesh-net"), "a partial echo does not arm");
        // Arming a delete OPENS the confirm — it publishes nothing (no mutation
        // request) until the name is typed (#22).
        let mut state = heat_tab_state(true);
        state.arm_heat_delete();
        let arming = state
            .heat
            .arming
            .as_ref()
            .expect("delete opens the arming confirm");
        assert_eq!(arming.op, HeatOp::Delete);
        assert_eq!(arming.stack_name, "mesh-net");
        assert_eq!(arming.stack_id, "s-1");
        assert!(arming.typed.is_empty());
        assert!(
            state.heat.mutation_pending.is_none(),
            "an unconfirmed delete publishes nothing"
        );
        // The armed render still tessellates (the confirm panel).
        assert!(run_panel(&mut state), "the arming confirm drew nothing");
    }

    #[test]
    fn reverse_generate_output_renders_and_create_arms_on_the_typed_name() {
        // The reverse-generated HOT (produced mesh-side) renders as a copyable view.
        let mut state = heat_tab_state(false);
        state.heat.reverse = Some(HeatOutcome::Ready(
            "heat_template_version: 2021-04-16\nresources: {}\n".to_string(),
        ));
        assert!(run_panel(&mut state), "the reverse output drew nothing");
        // Create is typed-armed on the entered name; an empty name refuses to arm.
        state.heat.create_name = String::new();
        state.arm_heat_create();
        assert!(
            state.heat.arming.is_none(),
            "an empty name does not arm a create"
        );
        state.heat.create_name = "fresh".to_string();
        state.heat.create_template = "heat_template_version: 2021-04-16".to_string();
        state.arm_heat_create();
        let arming = state.heat.arming.as_ref().expect("create arms on a name");
        assert_eq!(arming.op, HeatOp::Create);
        assert_eq!(arming.stack_name, "fresh");
        assert!(
            state.heat.mutation_pending.is_none(),
            "arming publishes nothing"
        );
    }

    #[test]
    fn reverse_services_exclude_orchestration_itself() {
        // Reverse-generate captures raw infra (compute/…), not existing stacks.
        let state = InfraCodeState {
            outcome: CatalogOutcome::Ready(heat_view()),
            ..InfraCodeState::default()
        };
        let services = state.heat_reverse_services();
        assert!(services.iter().any(|(ty, _)| ty == "compute"));
        assert!(
            !services.iter().any(|(ty, _)| ty == "orchestration"),
            "orchestration is excluded from the reverse-generate source set"
        );
    }
}

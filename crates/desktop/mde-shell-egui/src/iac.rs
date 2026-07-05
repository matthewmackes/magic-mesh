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
    default_collection, CatalogService, EndpointInterface, HealthState, ResourceRow, ResourceTable,
    ServiceCatalog, ServiceHealth,
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
            note: None,
        }
    }
}

impl InfraCodeState {
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
        //    `list-resources` request/reply on the same non-blocking cadence.
        if self.tab == IacTab::Resources {
            self.poll_resources(now);
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
            self.resources.entry(ty.clone()).or_default();

            // Resolve a pending request — its reply, or an honest timeout.
            let pending = self
                .resources
                .get(&ty)
                .and_then(|p| p.pending.as_ref().map(|q| (q.ulid.clone(), q.sent)));
            if let Some((ulid, sent)) = pending {
                if let Some(reply) = self.read_reply(&ulid) {
                    let outcome = fold_resource_reply(reply);
                    if let Some(pane) = self.resources.get_mut(&ty) {
                        pane.outcome = Some(outcome);
                        pane.pending = None;
                        pane.settled_at = Some(now);
                    }
                } else if sent.elapsed() >= REQUEST_TIMEOUT {
                    if let Some(pane) = self.resources.get_mut(&ty) {
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
            let Some(pane) = self.resources.get(&ty) else {
                continue;
            };
            let never = pane.settled_at.is_none();
            let cadence_due = self.auto_refresh
                && pane
                    .settled_at
                    .is_none_or(|t| now.duration_since(t) >= CATALOG_REFRESH);
            if pane.pending.is_none() && (pane.forced || never || cadence_due) {
                self.send_resource_request(&ty, &collection);
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

    // A pending typed-arming confirm (a destructive mutation) + the transient
    // action note render above the tab body — honest feedback, never silent.
    render_arming(ui, state);
    render_note(ui, state);

    match state.tab {
        IacTab::Overview => match &state.outcome {
            CatalogOutcome::Ready(view) => render_overview(ui, view, state.show_urls),
            other => render_catalog_absent(ui, other),
        },
        IacTab::Resources => render_resources_tab(ui, state),
        IacTab::Heat => render_heat_tab(ui),
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

/// The **Heat** tab body — an honest forward-looking empty state (design #21 /
/// the task's sanctioned honest-empty): the native `IaC` loop lands in IAC-4,
/// not a disabled tab and not fabricated stacks (§7).
fn render_heat_tab(ui: &mut egui::Ui) {
    crate::session::empty_state(
        ui,
        "Heat orchestration",
        "Stacks, templates, preview-update diff, drift, and reverse-generate arrive in the \
         Heat tab (IAC-4).",
    );
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
/// plane. Every item is a real seam (§6): **Catalog → Refresh now** forces an
/// immediate re-poll, **Catalog → Auto-refresh** toggles the ~15 s live poll,
/// **View → Show endpoint URLs** expands the tiles' full URLs. The File/Edit/Help
/// spine is omitted (no file/clipboard/about seam here — the Instances precedent,
/// §7). IAC-3 adds the **catalog-driven per-service menus** (one per drillable
/// bucket — Drill / Refresh resources, + Compute's armed lifecycle verbs), the
/// governing-principle headline: comprehensive, yet every item maps to a landed
/// Bus seam and an absent verb is omitted, never greyed (§8). The status cluster
/// reads the live catalog.
mod menubar {
    use super::{service_bucket, Arming, CatalogOutcome, IacTab, InfraCodeState, BUCKETS, DOT};
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
    }

    /// Render the INFRA AS CODE bar and return the action picked this frame. The
    /// bar is the Catalog / View spine **plus** the catalog-driven per-service
    /// menus (the governing principle — every real control, incl. the armed
    /// lifecycle verbs, is here; a verb with no landed seam is omitted, §8).
    pub(super) fn show(ui: &mut Ui, state: &InfraCodeState) -> Option<MenuAction> {
        let mut menus = build_menus(state.auto_refresh, state.show_urls);
        menus.extend(build_service_menus(state));
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

    /// Build the catalog-driven per-service menus (design #17): one menu per
    /// service bucket that carries a drillable resource collection, each with
    /// **Drill into resources** + **Refresh resources** (real `list-resources`
    /// seams), and — for **Compute** — the armed Nova lifecycle verbs on the
    /// selected instance (Start / Stop direct; Reboot / Delete typed-armed).
    /// Every item maps to a landed Bus seam; verbs without one are omitted, never
    /// greyed (§8). Empty until the catalog is [`CatalogOutcome::Ready`].
    fn build_service_menus(state: &InfraCodeState) -> Vec<Menu<MenuAction>> {
        let CatalogOutcome::Ready(view) = &state.outcome else {
            return Vec::new();
        };
        let selected = state.single_selected_instance();
        let mut menus = Vec::new();
        for bucket in BUCKETS {
            // The first cataloged service in this bucket that has a resource table.
            let Some(svc) = view.catalog.services.iter().find(|s| {
                service_bucket(&s.service_type) == bucket
                    && default_collection(&s.service_type).is_some()
            }) else {
                continue;
            };
            let ty = svc.service_type.clone();
            let mut entries = vec![
                Entry::Item(Item::new(
                    MenuAction::Drill(ty.clone()),
                    "Drill into resources",
                )),
                Entry::Item(Item::new(
                    MenuAction::RefreshResources(ty.clone()),
                    "Refresh resources",
                )),
            ];
            if bucket == "Compute" {
                entries.push(Entry::Separator);
                // The lifecycle verbs act on the single selected instance — a
                // context-gated control, so disabled (not omitted) when the
                // selection isn't exactly one (§7/#22 single-target arming).
                let (enabled, id, name) = selected.clone().map_or_else(
                    || (false, String::new(), String::new()),
                    |(id, name)| (true, id, name),
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
            menus.push(Menu::new(bucket, entries));
        }
        menus
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
        }
    }

    #[cfg(test)]
    #[allow(clippy::panic)]
    mod tests {
        use super::super::tests::fixture_view;
        use super::super::{CatalogOutcome, InfraCodeState};
        use super::{apply, build_menus, build_service_menus, build_status, MenuAction};
        use mde_egui::menubar::{Entry, Item};
        use mde_egui::ChipTone;

        #[test]
        fn service_menus_are_catalog_driven_and_carry_the_verb_set() {
            // The fixture catalog = compute + identity + image; compute & image are
            // drillable, identity is not (it has no resource table).
            let state = InfraCodeState {
                outcome: CatalogOutcome::Ready(fixture_view()),
                ..InfraCodeState::default()
            };
            let menus = build_service_menus(&state);
            let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
            assert!(titles.contains(&"Compute") && titles.contains(&"Image"));
            assert!(
                !titles.contains(&"Identity"),
                "an undrillable service gets no menu (§8, omitted not greyed)"
            );

            // A non-compute service carries just the two read verbs.
            let image = menus
                .iter()
                .find(|m| m.title == "Image")
                .expect("Image menu");
            assert_eq!(image.entries.len(), 2, "Drill + Refresh only");

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
}

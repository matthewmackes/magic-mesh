//! The **Infra as Code (`IaC`)** surface — the `OpenStack` `IaaS` control plane.
//!
//! `docs/design/iac-workspace.md` (the 25-lock design). This unit is **IAC-2**:
//! the surface shell + the **Overview** tab. The Overview is two stacked
//! sections:
//!
//! 1. the **`OpenStack` API status band** — a rich tile per cataloged service
//!    (name/type · health dot + latency · microversion/version · region ·
//!    public/internal/admin endpoints + port); and
//! 2. the **merged service directory** — the Keystone catalog services grouped
//!    by type (Compute / Network / Image / …), rich rows.
//!
//! The Resources + Heat tabs are IAC-3 / IAC-4; this unit shows Overview only —
//! the honest "one live tab" rather than a disabled-tab or "coming soon"
//! placeholder (§7). The `Style`-tokened tab bar + the merged "Mesh services"
//! group (folded from `descriptors` / `probe_nmap` / the mackesd registries)
//! land in IAC-3; the seam is a code-level note here, never rendered copy.
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

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use mackes_mesh_types::openstack::{
    CatalogService, EndpointInterface, HealthState, ServiceCatalog, ServiceHealth,
};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

/// The QC-11 Bus read verb this surface consumes (IAC-1) — the Keystone service
/// directory + per-endpoint API health, served on `reply/<request-ulid>`.
const CATALOG_ACTION: &str = "action/cloud/get-catalog";

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

        ctx.request_repaint_after(POLL_REPAINT);
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

// ───────────────────────────────── the render ───────────────────────────────

/// Render the Infra-as-Code surface into `ui`: the shared MENUBAR-ALL bar
/// (INFRA AS CODE, Workloads accent) over the **Overview** body — the `OpenStack`
/// API status band + the merged service directory, or an honest not-configured /
/// unreachable / querying empty state when the Bus verb hasn't answered with a
/// catalog (§7).
pub fn infra_code_panel(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    // MENUBAR-ALL — the shared bar. Its items are real seams (§6, one dispatch
    // path): Catalog → Refresh now / Auto-refresh; View → Show endpoint URLs. The
    // status cluster counts the live catalog (N services · M healthy · region).
    if let Some(action) = menubar::show(ui, state) {
        menubar::apply(state, action);
    }
    ui.separator();
    ui.add_space(Style::SP_S);

    match &state.outcome {
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
                "The OpenStack API status band appears here once the mesh cloud control plane \
                 answers.",
            );
        }
        CatalogOutcome::Ready(view) => render_overview(ui, view, state.show_urls),
    }
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
/// §7). The per-service dynamic catalog menus (the Resources/Heat verb set) land
/// with those tabs in IAC-3/4. The status cluster reads the live catalog.
mod menubar {
    use super::{CatalogOutcome, InfraCodeState, DOT};
    use mde_egui::egui::Ui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};

    /// One menu action — each routes to a real Infra-as-Code seam in [`apply`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// Force an immediate catalog re-poll (`Catalog → Refresh now`).
        Refresh,
        /// Toggle the ~15 s auto-poll (`Catalog → Auto-refresh`).
        ToggleAuto,
        /// Toggle full endpoint URLs on the tiles (`View → Show endpoint URLs`).
        ToggleUrls,
    }

    /// Render the INFRA AS CODE bar and return the action picked this frame.
    pub(super) fn show(ui: &mut Ui, state: &InfraCodeState) -> Option<MenuAction> {
        let menus = build_menus(state.auto_refresh, state.show_urls);
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
        }
    }

    #[cfg(test)]
    #[allow(clippy::panic)]
    mod tests {
        use super::super::tests::fixture_view;
        use super::super::{CatalogOutcome, InfraCodeState};
        use super::{apply, build_menus, build_status, MenuAction};
        use mde_egui::menubar::Entry;
        use mde_egui::ChipTone;

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
}

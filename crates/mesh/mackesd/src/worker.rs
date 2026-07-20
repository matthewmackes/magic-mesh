//! Reconcile worker — the thread that wires `reconcile::plan_tick`
//! into the running `mackesd` daemon (Phase 12.5 wiring lock).
//!
//! The reconcile engine's pure functions live in
//! [`crate::reconcile`]; this module owns the cadence + I/O around
//! them. Every `RECONCILE_INTERVAL_S` seconds the worker:
//!
//!   1. Reads each peer's `<workgroup_root>/<peer>/mackesd/heartbeat.json`
//!      and each peer's `<workgroup_root>/<peer>/mackesd/links.json` to
//!      build an *observed* `TopologySnapshot`.
//!   2. Reads the latest applied / verified `desired_config` row
//!      from the local SQL store and deserializes its `spec_json` into
//!      a *desired* `DesiredSnapshot` (falling back to
//!      `DesiredSnapshot::default()` on a fresh peer with no rows).
//!   3. Runs `topology::calculate` over the desired snapshot, diffs
//!      against the observed snapshot, calls `reconcile::plan_tick`,
//!      and routes each [`crate::reconcile::DriftRow`] to either the
//!      audit log + repair dispatcher (`repair_now`) or the operator
//!      inbox (`inbox`).
//!   4. For the SAFE subset of `repair_now` drift — an
//!      auto-repairable *missing edge* where THIS node is one of the
//!      two endpoints — it APPLIES a bounded, idempotent repair:
//!      [`apply_safe_repairs`] forces a fresh overlay re-probe of the
//!      peer ([`crate::transport_probe::probe_rtt`]). That probe is a
//!      TCP SYN through the Nebula tunnel, which both prompts Nebula
//!      to (re)establish the underlay tunnel for the pair (the actual
//!      self-heal for a dropped/never-punched hole-punch — the
//!      "transient hiccup" the drift detector names) and re-measures
//!      reachability. It is read-only on the peer (discard port),
//!      idempotent (a probe mutates nothing), and bounded
//!      ([`DEFAULT_MAX_REPAIRS_PER_TICK`]). Every attempt is
//!      audit-logged before + after.
//!
//! What the worker deliberately leaves OBSERVE-ONLY (mackesd-03 —
//! repair only the clearly-safe subset), per the Phase 12.5 lock and
//! the 12.14+ connectivity scope:
//!
//!   * A missing edge NOT incident to this node. Re-establishing a
//!     remote A↔B adjacency needs a route push over the connectivity
//!     layer (12.14+, multi-week scope) — this node has no safe local
//!     action, so the row is logged `manual-repair-required` and left
//!     for the operator / the gated layer. Explicit, documented scope
//!     boundary, not a stub.
//!   * Every `ManualReview` (extra-edge) drift row. Tearing down an
//!     observed-but-undesired adjacency is destructive (it could cut
//!     off a legitimately-recovering peer or mask tampering), so it
//!     stays in the operator inbox for explicit approval — never
//!     auto-repaired.
//!   * It does NOT INSERT into a `pending_changes` table. The
//!     `SQLite` schema (`migrations/0001_init.sql`) treats
//!     "pending changes" as `desired_config WHERE state IN
//!     ('draft','validated')` — the column-level state machine on
//!     `desired_config` IS the pending-changes bucket. Inbox rows
//!     get logged via `tracing::warn` until the GUI inbox surface
//!     wires through (12.9.x).
//!
//! Threading model: `std::thread::spawn` + `Arc<AtomicBool>` shutdown
//! flag, matching the heartbeat worker pattern in
//! [`crate::telemetry::spawn_heartbeat_worker`]. The 30-second tick
//! is implemented as a polling sleep on the shutdown flag every
//! [`SHUTDOWN_POLL`] so SIGTERM exits the thread in at most that
//! interval instead of waiting out the full cadence.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Context;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::audit::next_hash;
use crate::events::{Event, EventKind};
use crate::reconcile::{plan_tick, DriftRow, TickPlan};
use crate::telemetry::{Heartbeat, LinkSample};
use crate::topology::{calculate, diff, DesiredSnapshot, Edge, EdgeKind, TopologySnapshot};
use crate::transport_probe::ProbeResult;
use crate::Result;

/// Reconcile cadence in seconds (Phase 12.5.1 lock — "30 s default").
pub const RECONCILE_INTERVAL_S: u64 = 30;

/// How often the worker thread checks the shutdown flag while
/// sleeping between ticks.
///
/// Smaller = faster SIGTERM response; larger = less wakeup
/// overhead. 250 ms is the sweet spot — a systemd `TimeoutStopSec=5s`
/// exit lands in well under the limit.
pub const SHUTDOWN_POLL: Duration = Duration::from_millis(250);

/// The auto-repair policy flag the worker passes into
/// `reconcile::plan_tick`. True by default per the 12.5.3 lock —
/// auto-repair is opt-out via desired-config policy, not opt-in.
///
/// This is the *default*; the live flag comes from the daemon config
/// (`/etc/mackesd/mackesd.toml` `auto_repair = false` turns the apply
/// step OFF and falls the whole reconciler back to observe-only, per
/// [`load_repair_policy`]).
pub const DEFAULT_AUTO_REPAIR: bool = true;

/// Blast-radius cap: the maximum number of *safe repair actions* the
/// worker will take in a single reconcile tick (mackesd-03 guardrail).
///
/// A mass-drift event (e.g. a lighthouse flap that drops half the
/// observed adjacencies at once) must not let the daemon fire a probe
/// storm across the whole fleet in one pass. When the number of
/// auto-repairable, incident missing edges exceeds this cap, the
/// overflow is *deferred* to the next tick (audit-logged, `warn`) —
/// the reconciler converges over several ticks instead of thrashing in
/// one. Overridable via `/etc/mackesd/mackesd.toml`
/// `max_repairs_per_tick`.
pub const DEFAULT_MAX_REPAIRS_PER_TICK: usize = 16;

/// One reconcile tick's result. Captured so `mackesd reconcile
/// --once` can serialize it to JSON and so tests can assert against
/// the value returned by [`tick`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickOutcome {
    /// Wall-clock millis when the tick started.
    pub started_at_ms: i64,
    /// Number of heartbeat files the worker observed.
    pub observed_heartbeats: usize,
    /// Number of distinct observed adjacency edges (from links.json).
    pub observed_edges: usize,
    /// Number of edges in the calculated desired topology.
    pub desired_edges: usize,
    /// Plan that came out of `reconcile::plan_tick`.
    pub plan: TickPlanJson,
    /// Tick wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// JSON-friendly view of [`crate::reconcile::TickPlan`].
///
/// The plan's own fields aren't JSON-stable (`DriftRow`'s `detector`
/// is `&'static str`); the worker normalizes them to owned strings
/// here for `--once` output and on-disk audit detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickPlanJson {
    /// Owned-string form of `plan.repair_now`.
    pub repair_now: Vec<DriftRowJson>,
    /// Owned-string form of `plan.inbox`.
    pub inbox: Vec<DriftRowJson>,
}

/// JSON-stable mirror of [`crate::reconcile::DriftRow`] — owned
/// strings only so the outcome can round-trip through serde.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftRowJson {
    /// Severity — auto-repairable or manual-review.
    pub severity: String,
    /// Detector name (always `"topology"` today).
    pub detector: String,
    /// Reason chain copied verbatim from the source `DriftRow`.
    pub reason: String,
}

impl From<&DriftRow> for DriftRowJson {
    fn from(row: &DriftRow) -> Self {
        Self {
            severity: serde_json::to_value(row.severity)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned()),
            detector: row.detector.to_owned(),
            reason: row.reason.clone(),
        }
    }
}

impl From<&TickPlan> for TickPlanJson {
    fn from(plan: &TickPlan) -> Self {
        Self {
            repair_now: plan.repair_now.iter().map(DriftRowJson::from).collect(),
            inbox: plan.inbox.iter().map(DriftRowJson::from).collect(),
        }
    }
}

/// Spawn the reconcile worker as a standalone OS thread.
///
/// The returned [`JoinHandle`] lets the caller block on a clean exit
/// after flipping `shutdown`. The thread NEVER panics — every error
/// (corrupt heartbeat JSON, missing peer dir, DB unavailable) is
/// logged via `tracing::warn` and the loop continues. A persistent
/// failure surfaces in `mackesd healthz` once 12.6/12.1.3 wiring
/// is complete; for now operators inspect `journalctl -u mackesd`.
///
/// `node_id` is the stable id of the current peer (`peer:<hostname>`
/// by convention). It's used as the `actor` field on the audit-log
/// events the worker emits.
///
/// # Panics
///
/// Panics only if `std::thread::Builder::spawn` fails (e.g. the OS
/// refuses to create another thread). On a healthy system this never
/// happens; on a system that can't spawn threads the daemon can't
/// run anyway, so panic is the correct surfacing.
#[must_use]
pub fn spawn_reconcile_worker(
    workgroup_root: PathBuf,
    node_id: String,
    db_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("mackesd-reconcile".into())
        .spawn(move || {
            info!(
                node_id = %node_id,
                workgroup_root = %workgroup_root.display(),
                db = %db_path.display(),
                interval_s = RECONCILE_INTERVAL_S,
                "reconcile worker starting",
            );
            run_loop(&workgroup_root, &node_id, &db_path, &shutdown);
            info!(node_id = %node_id, "reconcile worker exited");
        })
        .expect("spawning mackesd-reconcile thread")
}

/// Inner blocking loop — exposed so `mackesd reconcile` (no
/// `--once`) can run the loop on the foreground thread for
/// systemd's `Type=simple` unit. Same shutdown semantics as the
/// spawned variant.
pub fn run_loop(workgroup_root: &Path, node_id: &str, db_path: &Path, shutdown: &Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Relaxed) {
        match tick(workgroup_root, node_id, db_path) {
            Ok(outcome) => {
                info!(
                    observed_heartbeats = outcome.observed_heartbeats,
                    observed_edges = outcome.observed_edges,
                    desired_edges = outcome.desired_edges,
                    repair_now = outcome.plan.repair_now.len(),
                    inbox = outcome.plan.inbox.len(),
                    duration_ms = outcome.duration_ms,
                    "reconcile tick complete",
                );
            }
            Err(e) => {
                // WL-RUN-002 — the reconcile loop's real failure path.
                // Bump the process-wide `mackesd_reconcile_failures_total`
                // counter the metrics exporter renders each tick.
                crate::metrics::record_reconcile_failure();
                warn!(error = %e, "reconcile tick failed; will retry next interval");
            }
        }
        interruptible_sleep(Duration::from_secs(RECONCILE_INTERVAL_S), shutdown);
    }
}

/// Sleep up to `total`, waking every [`SHUTDOWN_POLL`] to check the
/// shutdown flag. Returns immediately when shutdown flips true.
fn interruptible_sleep(total: Duration, shutdown: &Arc<AtomicBool>) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(SHUTDOWN_POLL));
    }
}

/// Run one reconcile tick end-to-end.
///
/// Pure-by-construction except for the FS read (heartbeats / links)
/// and SQL read+append (desired snapshot in, audit event out).
/// Returns the structured outcome the caller logs or prints.
///
/// # Errors
///
/// Returns an error only when the SQL store can't open or the
/// audit-event INSERT fails. FS errors on individual peer files are
/// logged and skipped — one corrupt heartbeat doesn't poison the
/// whole tick.
pub fn tick(workgroup_root: &Path, node_id: &str, db_path: &Path) -> Result<TickOutcome> {
    tick_with(
        workgroup_root,
        node_id,
        db_path,
        &LiveEdgeProber,
        &load_repair_policy(),
    )
}

/// Run one reconcile tick with an injected [`EdgeProber`] + explicit
/// [`RepairPolicy`] — the seam the tests drive so the safe-repair path
/// is exercised deterministically (fake prober, no live network) and
/// the live `mackesd` config can't perturb a test.
///
/// [`tick`] is the production entry point: it wires in the live prober
/// ([`LiveEdgeProber`]) and the operator's on-disk policy
/// ([`load_repair_policy`]). Same shutdown / error semantics.
///
/// # Errors
///
/// See [`tick`].
pub fn tick_with(
    workgroup_root: &Path,
    node_id: &str,
    db_path: &Path,
    prober: &dyn EdgeProber,
    policy: &RepairPolicy,
) -> Result<TickOutcome> {
    let started = Instant::now();
    let started_at_ms = now_ms();

    let heartbeats = read_all_heartbeats(workgroup_root);
    let observed_edges_set = read_all_observed_edges(workgroup_root);
    debug!(
        observed_heartbeats = heartbeats.len(),
        observed_edges = observed_edges_set.edges.len(),
        "reconcile tick: observed snapshot built",
    );

    // Desired snapshot comes from the latest applied / verified
    // `desired_config` row. On a fresh store (or while no revision
    // has applied yet) this is `DesiredSnapshot::default()`.
    let mut conn = crate::store::open(db_path)
        .with_context(|| format!("opening store at {}", db_path.display()))?;
    let desired_snapshot = load_desired_snapshot(&conn)?;
    let desired_topo = calculate(&desired_snapshot);

    // VV-2.a — materialize the voice-desired.json document from
    // the snapshot's approved voice policies. Idempotent: only
    // bumps the file mtime when the serialized bytes differ, so
    // the `voice_config` worker's mtime gate fires exactly once
    // per policy change.
    materialize_voice_desired_for_tick(
        &desired_snapshot,
        node_id,
        workgroup_root,
        &crate::voice::materialize::default_desired_json_path(),
    );

    let topology_diff = diff(&desired_topo, &observed_edges_set);
    // The apply-side gate: `policy.enabled` drives `plan_tick`'s
    // routing (auto-repairable rows → `repair_now` when on, → `inbox`
    // when off) AND `apply_safe_repairs`'s short-circuit, so flipping
    // `auto_repair = false` in the daemon config falls the entire
    // reconciler back to observe-only.
    let plan = plan_tick(&topology_diff, policy.enabled);

    // WL-RUN-002 — every drift row this tick classified (auto-repair +
    // inbox) is a drift EVENT; record them for the process-wide
    // `mackesd_drift_events_total` counter the metrics exporter renders.
    crate::metrics::record_drift_events(
        u64::try_from(plan.repair_now.len() + plan.inbox.len()).unwrap_or(u64::MAX),
    );

    // mackesd-03 — APPLY the safe subset of the drift. Runs BEFORE the
    // audit transaction so the (potentially blocking) probes never sit
    // inside the SQL write lock; the per-edge outcomes then ride into
    // the audit event detail. Only the auto-repairable *missing* edges
    // are candidates; extra edges are `ManualReview` and never reach
    // here (they're in `plan.inbox`).
    let repair = apply_safe_repairs(
        prober,
        node_id,
        workgroup_root,
        &topology_diff.missing,
        policy,
    );

    // Emit one Reconcile event per repair-now row (enriched with the
    // applied repair's outcome) + log every inbox row. Audit hash
    // chain is per-row so an audit verify walks back through them
    // cleanly (12.6.3 / 12.10.3).
    let emitted = apply_repair_rows(&mut conn, node_id, &plan.repair_now, &repair.outcomes)?;
    // EFF-25 — fire the 12.6.4 alert hooks for the committed events.
    // Hooks come from /etc/mackesd/mackesd.toml `[[alert_hooks]]`
    // (fail-open load, default empty → no-op). Dispatch is post-commit
    // so a rolled-back event can never alert, and best-effort by
    // design (spawn failures warn + continue).
    if !emitted.is_empty() {
        let hooks = crate::config::daemon::load().alert_hooks();
        if !hooks.is_empty() {
            for event in &emitted {
                crate::events::dispatch_alerts(event, &hooks);
            }
        }
    }
    surface_inbox_rows(node_id, &plan.inbox);

    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(TickOutcome {
        started_at_ms,
        observed_heartbeats: heartbeats.len(),
        observed_edges: observed_edges_set.edges.len(),
        desired_edges: desired_topo.edges.len(),
        plan: TickPlanJson::from(&plan),
        duration_ms,
    })
}

/// Walk `<workgroup_root>/*/mackesd/heartbeat.json` and deserialize every
/// readable row. Unreadable / malformed files are skipped with a
/// warn-level log — the reconcile tick stays best-effort by design.
#[must_use]
pub fn read_all_heartbeats(workgroup_root: &Path) -> Vec<Heartbeat> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(e) => e,
        Err(e) => {
            // ENOENT is normal on a fresh peer — only warn for
            // anything else.
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(error = %e, root = %workgroup_root.display(), "scanning workgroup_root failed");
            }
            return out;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path().join("mackesd").join("heartbeat.json");
        if !path.is_file() {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Heartbeat>(&bytes) {
                Ok(hb) => out.push(hb),
                Err(e) => warn!(
                    error = %e,
                    file = %path.display(),
                    "skipping malformed heartbeat",
                ),
            },
            Err(e) => warn!(
                error = %e,
                file = %path.display(),
                "skipping unreadable heartbeat",
            ),
        }
    }
    out
}

/// Walk `<workgroup_root>/*/mackesd/links.json` and build the observed
/// `TopologySnapshot`.
///
/// An observed edge exists between peer A and B when A's `links.json`
/// records a `LinkSample` with `rtt_ms = Some` (i.e. the probe
/// actually reached). The kind is hard-coded to `NebulaDirect` today;
/// once the transport classifier ships (12.14+), the worker fills in
/// `NebulaLighthouseRelay` / `NebulaHttps443` from the probe metadata.
#[must_use]
pub fn read_all_observed_edges(workgroup_root: &Path) -> TopologySnapshot {
    let mut edges: BTreeSet<Edge> = BTreeSet::new();
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(e) => e,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(error = %e, root = %workgroup_root.display(), "scanning workgroup_root failed");
            }
            return TopologySnapshot::default();
        }
    };
    for entry in entries.flatten() {
        let path = entry.path().join("mackesd").join("links.json");
        if !path.is_file() {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, file = %path.display(), "skipping unreadable links file");
                continue;
            }
        };
        let samples: Vec<LinkSample> = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, file = %path.display(), "skipping malformed links file");
                continue;
            }
        };
        for s in samples {
            if s.rtt_ms.is_none() {
                continue;
            }
            let (a, b) = if s.from_id <= s.to_id {
                (s.from_id, s.to_id)
            } else {
                (s.to_id, s.from_id)
            };
            if a == b {
                continue;
            }
            edges.insert(Edge {
                a,
                b,
                kind: EdgeKind::NebulaDirect,
            });
        }
    }
    TopologySnapshot {
        edges,
        routes: std::collections::BTreeMap::default(),
    }
}

/// Read the latest applied / verified `desired_config` row and
/// deserialize its `spec_json` payload into a `DesiredSnapshot`.
///
/// Returns `DesiredSnapshot::default()` when no such row exists
/// (fresh store) so the reconciler still ticks deterministically.
///
/// # Errors
///
/// Returns an error if the SQL query itself fails (e.g. schema
/// corruption); a missing row is a normal empty-store return.
pub fn load_desired_snapshot(conn: &Connection) -> Result<DesiredSnapshot> {
    // The schema (migrations/0001_init.sql) defines `desired_config`
    // with a state column whose terminal values are `applied` and
    // `verified`. Pull the highest-revision row in either state.
    let row: Option<String> = conn
        .query_row(
            "SELECT spec_json FROM desired_config \
             WHERE state IN ('verified', 'applied') \
             ORDER BY revision_id DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    match row {
        Some(json) => {
            let snap: DesiredSnapshot = serde_json::from_str(&json)
                .context("deserializing latest desired_config.spec_json")?;
            Ok(snap)
        }
        None => Ok(DesiredSnapshot::default()),
    }
}

/// VV-2.a — call the voice materializer with best-effort
/// logging. Wrapped so the reconcile tick can stay
/// non-fatal on FS errors (writing to `/var/lib/mackesd` can
/// fail on a read-only mount, full disk, etc., and that
/// shouldn't poison the whole tick).
fn materialize_voice_desired_for_tick(
    snapshot: &DesiredSnapshot,
    node_id: &str,
    workgroup_root: &Path,
    desired_json_path: &Path,
) {
    match crate::voice::materialize::materialize_voice_desired(
        snapshot,
        node_id,
        workgroup_root,
        desired_json_path,
    ) {
        Ok(crate::voice::materialize::MaterializeOutcome::Wrote) => {
            info!(
                path = %desired_json_path.display(),
                voice_policies = snapshot.voice_policies.len(),
                "voice-desired.json materialized; voice_config will reload on next tick",
            );
        }
        Ok(crate::voice::materialize::MaterializeOutcome::Unchanged) => {
            debug!(
                path = %desired_json_path.display(),
                "voice-desired.json unchanged from previous tick",
            );
        }
        Ok(crate::voice::materialize::MaterializeOutcome::SkippedNoPolicies) => {
            debug!(
                "no voice policies in desired snapshot; deferring boot-default seed to voice_config worker",
            );
        }
        Err(e) => {
            warn!(
                error = %e,
                path = %desired_json_path.display(),
                "voice-desired materialize failed; will retry on next tick",
            );
        }
    }
}

/// mackesd-03 — the live-flag pair that gates the safe-repair apply
/// step. Sourced from the daemon config via [`load_repair_policy`];
/// tests build one by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairPolicy {
    /// Master switch. `false` → the reconciler is observe-only:
    /// `plan_tick` routes even auto-repairable drift to the inbox and
    /// [`apply_safe_repairs`] takes no action. Defaults to
    /// [`DEFAULT_AUTO_REPAIR`].
    pub enabled: bool,
    /// Blast-radius cap — the max number of repair *actions* (probes)
    /// per tick. Overflow drift is deferred to the next tick. Defaults
    /// to [`DEFAULT_MAX_REPAIRS_PER_TICK`].
    pub max_per_tick: usize,
}

impl Default for RepairPolicy {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_AUTO_REPAIR,
            max_per_tick: DEFAULT_MAX_REPAIRS_PER_TICK,
        }
    }
}

/// Load the live [`RepairPolicy`] from `/etc/mackesd/mackesd.toml`
/// (fail-open: a missing / malformed file → the locked defaults, so an
/// un-templated box behaves exactly as before this loader existed).
#[must_use]
pub fn load_repair_policy() -> RepairPolicy {
    let cfg = crate::config::daemon::load();
    RepairPolicy {
        enabled: cfg.auto_repair,
        max_per_tick: cfg.max_repairs_per_tick,
    }
}

/// Per-edge result of the safe-repair pass. Carried into the audit
/// event detail (what drift → what action → what outcome) and asserted
/// by the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairOutcome {
    /// The re-probe reached the peer — the overlay tunnel is
    /// (re)established; the drift should clear once telemetry catches
    /// up next tick.
    Reprobed {
        reachable: bool,
        rtt_ms: Option<u64>,
    },
    /// No safe local action: the missing edge is not incident to this
    /// node, or the peer's overlay IP couldn't be resolved. Left for
    /// the operator / the gated 12.14+ connectivity layer.
    ManualRepairRequired(&'static str),
    /// The per-cycle repair cap was already spent this tick — the row
    /// is deferred (audit-logged) and retried next tick.
    DeferredCapReached,
    /// Auto-repair is disabled by config — observe-only.
    Disabled,
}

impl RepairOutcome {
    /// Stable snake_case action token for the audit event detail.
    fn action(&self) -> &'static str {
        match self {
            RepairOutcome::Reprobed { .. } => "reprobe_overlay",
            RepairOutcome::ManualRepairRequired(_) => "manual_repair_required",
            RepairOutcome::DeferredCapReached => "deferred_cap_reached",
            RepairOutcome::Disabled => "observe_only_disabled",
        }
    }
}

/// The probe seam. Production uses [`LiveEdgeProber`]
/// ([`crate::transport_probe::probe_rtt`]); tests inject a fake so the
/// safe-repair path runs deterministically with no live network.
pub trait EdgeProber {
    /// Probe `overlay_ip` once (a TCP SYN through the Nebula tunnel).
    fn probe_overlay(&self, overlay_ip: &str) -> ProbeResult;
}

/// Production prober — a real overlay RTT probe.
pub struct LiveEdgeProber;

impl EdgeProber for LiveEdgeProber {
    fn probe_overlay(&self, overlay_ip: &str) -> ProbeResult {
        crate::transport_probe::probe_rtt(overlay_ip)
    }
}

/// mackesd-03 — APPLY the safe subset of the reconcile drift.
///
/// The ONLY drift category we auto-repair is an auto-repairable
/// **missing edge** (a desired adjacency absent from observed
/// telemetry) where THIS node (`node_id`) is one of the two endpoints.
/// The repair is a fresh overlay re-probe of the OTHER endpoint: a TCP
/// SYN through the Nebula tunnel both prompts Nebula to (re)establish
/// the underlay tunnel for the pair (the real self-heal for a dropped
/// hole-punch — the "transient network hiccup" the drift detector
/// names) and re-measures reachability. It is:
///
///   * **Safe** — read-only on the peer (the SYN hits the discard
///     port; a RST is a *successful* measurement), never restarts a
///     service or pushes config.
///   * **Idempotent** — a probe mutates no persistent state, so
///     re-running the reconcile loop can't double-apply anything; once
///     the tunnel is back the edge stops being drift and no further
///     probe fires.
///   * **Bounded** — at most `policy.max_per_tick` probes per tick;
///     overflow is deferred (`warn` + audit) so a mass-drift event
///     can't trigger a fleet-wide probe storm in one pass.
///
/// Every other case is left OBSERVE-ONLY and reported so the operator
/// (or the gated 12.14+ connectivity layer) can act:
///   * a missing edge not incident to this node → no safe local action;
///   * a peer whose overlay IP can't be resolved (no bundle yet);
///   * `policy.enabled == false` → the whole step is a no-op.
///
/// `missing` is `topology_diff.missing` — the detector's output is
/// unchanged; this only adds the guardrailed apply step for the safe
/// rows. Returns a per-edge [`RepairOutcome`] list (BTreeSet order, so
/// it aligns index-for-index with `plan.repair_now`).
#[must_use]
pub fn apply_safe_repairs(
    prober: &dyn EdgeProber,
    node_id: &str,
    workgroup_root: &Path,
    missing: &BTreeSet<Edge>,
    policy: &RepairPolicy,
) -> RepairReport {
    let mut report = RepairReport::default();

    // Config-off fallback: observe-only, fire nothing. (`plan_tick`
    // has already routed these rows to the inbox; we simply record the
    // Disabled outcome so the audit detail is honest.)
    if !policy.enabled {
        for edge in missing {
            report
                .outcomes
                .push((edge.clone(), RepairOutcome::Disabled));
        }
        return report;
    }

    for edge in missing {
        // Only edges incident to THIS node have a safe local repair.
        let Some(peer) = incident_peer(node_id, edge) else {
            report.outcomes.push((
                edge.clone(),
                RepairOutcome::ManualRepairRequired(
                    "missing edge not incident to this node; route push gated on 12.14+ connectivity layer",
                ),
            ));
            continue;
        };

        // Blast-radius cap: stop taking *actions* past the cap; defer
        // the rest to the next tick.
        if report.attempted >= policy.max_per_tick {
            report.cap_reached = true;
            report
                .outcomes
                .push((edge.clone(), RepairOutcome::DeferredCapReached));
            continue;
        }

        // Resolve the peer's overlay IP from its replicated bundle.
        let Some(overlay_ip) = resolve_overlay_ip(workgroup_root, peer) else {
            report.outcomes.push((
                edge.clone(),
                RepairOutcome::ManualRepairRequired(
                    "peer overlay IP unresolved (no nebula-bundle.json yet)",
                ),
            ));
            continue;
        };

        // AUDIT — before.
        info!(
            actor = %node_id,
            edge = %format!("{} ↔ {}", edge.a, edge.b),
            peer = %peer,
            overlay_ip = %overlay_ip,
            "auto-repair: re-probing overlay path to (re)establish adjacency",
        );

        let probe = prober.probe_overlay(&overlay_ip);
        report.attempted += 1;
        let rtt_ms = probe.rtt_ms.map(|v| v.max(0.0).round() as u64);
        let outcome = RepairOutcome::Reprobed {
            reachable: probe.reachable,
            rtt_ms,
        };

        // AUDIT — after.
        info!(
            actor = %node_id,
            edge = %format!("{} ↔ {}", edge.a, edge.b),
            peer = %peer,
            overlay_ip = %overlay_ip,
            reachable = probe.reachable,
            rtt_ms = ?rtt_ms,
            "auto-repair: re-probe complete",
        );

        report.outcomes.push((edge.clone(), outcome));
    }

    if report.cap_reached {
        warn!(
            actor = %node_id,
            cap = policy.max_per_tick,
            attempted = report.attempted,
            "auto-repair per-cycle cap reached; remaining drift deferred to next tick",
        );
    }

    report
}

/// Summary of one [`apply_safe_repairs`] pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Per-edge outcome in BTreeSet order (aligns with
    /// `plan.repair_now`).
    pub outcomes: Vec<(Edge, RepairOutcome)>,
    /// How many repair actions (probes) actually fired — the number
    /// bounded by `policy.max_per_tick`.
    pub attempted: usize,
    /// True when the per-cycle cap truncated the work.
    pub cap_reached: bool,
}

/// The other endpoint of `edge` when `node_id` is one of its two ends;
/// `None` when this node isn't incident to the edge.
fn incident_peer<'a>(node_id: &str, edge: &'a Edge) -> Option<&'a str> {
    if edge.a == node_id {
        Some(edge.b.as_str())
    } else if edge.b == node_id {
        Some(edge.a.as_str())
    } else {
        None
    }
}

/// Resolve `peer`'s overlay IP from its replicated
/// `<workgroup_root>/<peer>/mackesd/nebula-bundle.json`. `None` when
/// the bundle is missing / unreadable — the caller then leaves the
/// edge observe-only rather than probing a bogus address.
fn resolve_overlay_ip(workgroup_root: &Path, peer: &str) -> Option<String> {
    let path = crate::ca::bundle::bundle_path(workgroup_root, peer);
    match crate::ca::bundle::read_bundle(&path) {
        Ok(bundle) if !bundle.overlay_ip.is_empty() => Some(bundle.overlay_ip),
        _ => None,
    }
}

/// Emit one audit-log event per `repair_now` row — enriched with the
/// applied repair's [`RepairOutcome`] (what drift → what action → what
/// outcome) — and write a `tracing::info` line. Returns the emitted
/// [`Event`]s so the caller can fire the 12.6.4 alert hooks AFTER the
/// transaction commits (EFF-25 — alerts must never fire for a
/// rolled-back event).
///
/// `outcomes` is [`apply_safe_repairs`]'s per-edge result, aligned
/// index-for-index with `rows` (both derive from the same sorted
/// `missing` set). The *safe* repair (an overlay re-probe of an
/// incident peer) has already run in [`apply_safe_repairs`]; the
/// heavier take-action (pushing routes over the Nebula overlay,
/// restarting peer services) remains gated on the connectivity layer
/// (12.14+) — an explicit, documented scope boundary, not a stub.
///
/// # Errors
///
/// Returns an error only when the SQL `INSERT INTO events` fails
/// (e.g. WAL contention beyond the busy timeout). FS / serde errors
/// are logged and the loop continues.
pub fn apply_repair_rows(
    conn: &mut Connection,
    node_id: &str,
    rows: &[DriftRow],
    outcomes: &[(Edge, RepairOutcome)],
) -> Result<Vec<Event>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    crate::store::with_transaction(conn, |tx| {
        // Bootstrap: load the most recent event's hash to chain
        // onto. Genesis case = 32 zero bytes per `audit::next_hash`.
        let prev_hash_hex: String = tx
            .query_row(
                "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default();
        let mut prev_hash = decode_hex32(&prev_hash_hex).unwrap_or([0u8; 32]);

        // `now_iso` MUST encode exactly `now_ms_val`: load_audit_rows
        // reparses created_at → epoch-millis to recompute the chain
        // hash, so a separate Utc::now() here would drift from the
        // now_ms_val baked into each row's hash and spuriously fail
        // `audit::verify`. Derive both from one instant.
        let now_ms_val = now_ms();
        let now_iso = chrono::DateTime::from_timestamp_millis(now_ms_val)
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc3339();
        let mut emitted: Vec<Event> = Vec::with_capacity(rows.len());
        for (idx, row) in rows.iter().enumerate() {
            // The safe-repair outcome for this row (aligned by index;
            // absent only if a future detector emits a repair_now row
            // with no matching missing edge — record it observe-only).
            let outcome = outcomes.get(idx).map(|(_, o)| o.clone()).unwrap_or(
                RepairOutcome::ManualRepairRequired("no aligned repair outcome for drift row"),
            );
            // Build the structured Event payload; serialize once for
            // both the audit chain and the JSON column.
            let event = Event {
                event_id: u64::try_from(now_ms_val.max(0)).unwrap_or(0)
                    + u64::try_from(idx).unwrap_or(0),
                kind: EventKind::Reconcile,
                node_id: node_id.to_owned(),
                timestamp_ms: now_ms_val,
                detail: serde_json::json!({
                    "action": outcome.action(),
                    "drift": DriftRowJson::from(row),
                    "outcome": repair_outcome_detail(&outcome),
                }),
            };
            let payload = event
                .payload_bytes()
                .context("serializing audit event payload")?;
            let hash = next_hash(&prev_hash, &payload, now_ms_val);

            let payload_str = String::from_utf8(payload).context(
                "audit payload is not valid UTF-8 (should be impossible from serde_json)",
            )?;
            tx.execute(
                "INSERT INTO events (prev_hash, hash, kind, actor, payload_json, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                (
                    encode_hex(&prev_hash),
                    encode_hex(&hash),
                    serde_json::to_string(&EventKind::Reconcile)
                        .unwrap_or_else(|_| "\"reconcile\"".to_owned())
                        .trim_matches('"')
                        .to_owned(),
                    node_id,
                    payload_str,
                    &now_iso,
                ),
            )
            .context("inserting audit event row")?;

            info!(
                actor = %node_id,
                detector = row.detector,
                reason = %row.reason,
                action = outcome.action(),
                "auto-repair drift row committed to audit log",
            );

            prev_hash = hash;
            emitted.push(event);
        }
        Ok(emitted)
    })
}

/// JSON detail block for one [`RepairOutcome`] — the "outcome" leg of
/// the audit event's `(what drift → what action → what outcome)`.
fn repair_outcome_detail(outcome: &RepairOutcome) -> serde_json::Value {
    match outcome {
        RepairOutcome::Reprobed { reachable, rtt_ms } => serde_json::json!({
            "kind": "reprobed",
            "reachable": reachable,
            "rtt_ms": rtt_ms,
        }),
        RepairOutcome::ManualRepairRequired(why) => serde_json::json!({
            "kind": "manual_repair_required",
            "reason": why,
        }),
        RepairOutcome::DeferredCapReached => serde_json::json!({
            "kind": "deferred_cap_reached",
        }),
        RepairOutcome::Disabled => serde_json::json!({
            "kind": "observe_only_disabled",
        }),
    }
}

/// Log every inbox row at `warn` level so operators see them in the
/// journal.
///
/// The "would-be insert into `pending_changes`" the Phase 12.5 lock
/// mentions is conceptual: the `SQLite` schema doesn't have a separate
/// `pending_changes` table — the `desired_config WHERE state IN
/// ('draft','validated')` projection IS the pending bucket. Once the
/// GUI inbox surface lands (12.9.x) these warnings cross-reference
/// into UI rows.
pub fn surface_inbox_rows(node_id: &str, rows: &[DriftRow]) {
    for row in rows {
        warn!(
            actor = %node_id,
            detector = row.detector,
            reason = %row.reason,
            "manual-review drift surfaced to inbox (operator approval required)",
        );
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn encode_hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn decode_hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{write_heartbeat, write_links, HealthState};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    fn make_hb(workgroup_root: &Path, id: &str) {
        let hb = Heartbeat {
            node_id: id.to_owned(),
            at_ms: 1,
            agent_version: "1.0.7".into(),
            applied_revision: None,
            health: HealthState::Healthy,
        };
        write_heartbeat(workgroup_root, &hb).unwrap();
    }

    fn make_links(workgroup_root: &Path, from: &str, to: &str, rtt: Option<u32>) {
        let samples = vec![LinkSample {
            from_id: from.to_owned(),
            to_id: to.to_owned(),
            rtt_ms: rtt,
            loss: None,
            throughput_mbps: None,
            at_ms: 1,
        }];
        write_links(workgroup_root, from, &samples).unwrap();
    }

    #[test]
    fn read_heartbeats_skips_empty_root() {
        let dir = tempfile::tempdir().unwrap();
        let hbs = read_all_heartbeats(dir.path());
        assert!(hbs.is_empty());
    }

    #[test]
    fn read_heartbeats_returns_each_valid_row() {
        let dir = tempfile::tempdir().unwrap();
        make_hb(dir.path(), "peer:a");
        make_hb(dir.path(), "peer:b");
        let hbs = read_all_heartbeats(dir.path());
        assert_eq!(hbs.len(), 2);
    }

    #[test]
    fn read_heartbeats_skips_malformed_files() {
        let dir = tempfile::tempdir().unwrap();
        make_hb(dir.path(), "peer:good");
        // Drop a bogus heartbeat for `peer:bad`.
        let bad_dir = dir.path().join("peer:bad").join("mackesd");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("heartbeat.json"), b"not valid json").unwrap();
        let hbs = read_all_heartbeats(dir.path());
        assert_eq!(hbs.len(), 1);
        assert_eq!(hbs[0].node_id, "peer:good");
    }

    #[test]
    fn observed_edges_built_from_links() {
        let dir = tempfile::tempdir().unwrap();
        make_links(dir.path(), "peer:a", "peer:b", Some(10));
        make_links(dir.path(), "peer:c", "peer:d", Some(20));
        let topo = read_all_observed_edges(dir.path());
        assert_eq!(topo.edges.len(), 2);
    }

    #[test]
    fn observed_edges_dedupe_lexicographically() {
        let dir = tempfile::tempdir().unwrap();
        // `peer:b -> peer:a` collapses to the same edge as a -> b.
        make_links(dir.path(), "peer:b", "peer:a", Some(10));
        make_links(dir.path(), "peer:a", "peer:b", Some(11));
        let topo = read_all_observed_edges(dir.path());
        assert_eq!(topo.edges.len(), 1);
        let e = topo.edges.iter().next().unwrap();
        assert_eq!(e.a, "peer:a");
        assert_eq!(e.b, "peer:b");
    }

    #[test]
    fn observed_edges_skip_unmeasured_probes() {
        let dir = tempfile::tempdir().unwrap();
        // rtt = None means the probe didn't reach — no edge.
        make_links(dir.path(), "peer:a", "peer:b", None);
        let topo = read_all_observed_edges(dir.path());
        assert!(topo.edges.is_empty());
    }

    #[test]
    fn load_desired_snapshot_default_on_empty_store() {
        let conn = crate::store::open_in_memory().unwrap();
        let snap = load_desired_snapshot(&conn).unwrap();
        assert!(snap.nodes.is_empty());
    }

    #[test]
    fn load_desired_snapshot_reads_latest_applied_row() {
        let conn = crate::store::open_in_memory().unwrap();
        let payload = serde_json::json!({
            "nodes": [
                {"id": "peer:anvil", "region": "us-east", "healthy": true, "is_host": true},
            ],
            "allow_east_west": [],
        });
        conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at, applied_at) \
             VALUES (?, ?, ?, 'applied', ?, ?)",
            (
                "tester",
                "seed",
                payload.to_string(),
                "2026-05-19T00:00:00Z",
                "2026-05-19T00:00:00Z",
            ),
        )
        .unwrap();
        let snap = load_desired_snapshot(&conn).unwrap();
        assert_eq!(snap.nodes.len(), 1);
        assert_eq!(snap.nodes[0].id, "peer:anvil");
    }

    #[test]
    fn load_desired_snapshot_ignores_draft_rows() {
        let conn = crate::store::open_in_memory().unwrap();
        let payload = serde_json::json!({
            "nodes": [
                {"id": "peer:draft", "region": "r", "healthy": true, "is_host": false},
            ],
            "allow_east_west": [],
        });
        conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at) \
             VALUES (?, ?, ?, 'draft', ?)",
            ("tester", "wip", payload.to_string(), "2026-05-19T00:00:00Z"),
        )
        .unwrap();
        let snap = load_desired_snapshot(&conn).unwrap();
        // Draft rows must NOT feed the reconciler — they haven't
        // been approved + applied yet.
        assert!(snap.nodes.is_empty());
    }

    #[test]
    fn tick_returns_outcome_against_empty_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let workgroup_root = dir.path().join("qnm-shared");
        let db_path = dir.path().join("mackesd.db");
        // Bootstrap an empty store so `tick` can open it.
        let _ = crate::store::open(&db_path).unwrap();
        let outcome = tick(&workgroup_root, "peer:test", &db_path).unwrap();
        assert_eq!(outcome.observed_heartbeats, 0);
        assert_eq!(outcome.observed_edges, 0);
        assert_eq!(outcome.desired_edges, 0);
        assert!(outcome.plan.repair_now.is_empty());
        assert!(outcome.plan.inbox.is_empty());
    }

    #[test]
    fn tick_routes_extra_observed_edge_to_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let workgroup_root = dir.path().join("qnm-shared");
        std::fs::create_dir_all(&workgroup_root).unwrap();
        let db_path = dir.path().join("mackesd.db");
        let _ = crate::store::open(&db_path).unwrap();
        // No desired config → an observed edge is "extra" → manual review.
        make_links(&workgroup_root, "peer:a", "peer:b", Some(10));
        let outcome = tick(&workgroup_root, "peer:test", &db_path).unwrap();
        assert_eq!(outcome.observed_edges, 1);
        assert_eq!(outcome.plan.repair_now.len(), 0);
        assert_eq!(outcome.plan.inbox.len(), 1);
        assert_eq!(outcome.plan.inbox[0].severity, "manual_review");
    }

    #[test]
    fn tick_routes_missing_edge_to_repair_now_and_writes_audit_row() {
        let dir = tempfile::tempdir().unwrap();
        let workgroup_root = dir.path().join("qnm-shared");
        std::fs::create_dir_all(&workgroup_root).unwrap();
        let db_path = dir.path().join("mackesd.db");
        // Seed the store with a desired config that expects peer:a ↔ peer:b.
        let conn = crate::store::open(&db_path).unwrap();
        let payload = serde_json::json!({
            "nodes": [
                {"id": "peer:a", "region": "r", "healthy": true, "is_host": true},
                {"id": "peer:b", "region": "r", "healthy": true, "is_host": false},
            ],
            "allow_east_west": [],
        });
        conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at, applied_at) \
             VALUES (?, ?, ?, 'applied', ?, ?)",
            (
                "tester",
                "seed",
                payload.to_string(),
                "2026-05-19T00:00:00Z",
                "2026-05-19T00:00:00Z",
            ),
        )
        .unwrap();
        drop(conn);
        // No observed link → desired edge is "missing" → auto-repairable.
        let outcome = tick(&workgroup_root, "peer:test", &db_path).unwrap();
        assert_eq!(outcome.desired_edges, 1);
        assert_eq!(outcome.plan.repair_now.len(), 1);
        assert_eq!(outcome.plan.repair_now[0].severity, "auto_repairable");
        // Audit row landed: events count should be 1.
        let conn2 = crate::store::open(&db_path).unwrap();
        let n: i64 = conn2
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn spawn_reconcile_worker_exits_when_shutdown_flips() {
        let dir = tempfile::tempdir().unwrap();
        let workgroup_root = dir.path().join("qnm-shared");
        let db_path = dir.path().join("mackesd.db");
        let _ = crate::store::open(&db_path).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_reconcile_worker(
            workgroup_root,
            "peer:test".into(),
            db_path,
            Arc::clone(&shutdown),
        );
        // Let one tick complete before we flip.
        std::thread::sleep(Duration::from_millis(200));
        shutdown.store(true, Ordering::Relaxed);
        // Should exit well within SHUTDOWN_POLL + a small margin.
        // We cap the join wait via a watchdog thread.
        let watchdog = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            // If the worker is still alive after 5 s we deserve to
            // see a panic in the test runner — but we don't have a
            // graceful way to kill it. Best we can do is fail.
            // (handle.join() below will block until exit.)
        });
        handle.join().unwrap();
        // Watchdog has either already slept and returned, or hasn't —
        // join it for cleanliness.
        drop(watchdog);
    }

    #[test]
    fn interruptible_sleep_returns_when_flag_flips_mid_sleep() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let s2 = Arc::clone(&shutdown);
        let flipper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            s2.store(true, Ordering::Relaxed);
        });
        let before = Instant::now();
        interruptible_sleep(Duration::from_secs(10), &shutdown);
        let elapsed = before.elapsed();
        flipper.join().unwrap();
        // Should bail out within ~SHUTDOWN_POLL + flipper delay
        // (~350 ms in practice), well under the full 10 s.
        assert!(
            elapsed < Duration::from_secs(2),
            "interruptible_sleep took {elapsed:?}; should have exited near 350 ms",
        );
    }

    #[test]
    fn hex_round_trip_through_encode_decode() {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap_or(0);
        }
        let s = encode_hex(&bytes);
        assert_eq!(s.len(), 64);
        let back = decode_hex32(&s).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn hex_decode_rejects_wrong_length() {
        assert!(decode_hex32("").is_none());
        assert!(decode_hex32("aa").is_none());
        assert!(decode_hex32(&"a".repeat(63)).is_none());
        assert!(decode_hex32(&"a".repeat(65)).is_none());
    }

    #[test]
    fn drift_row_json_round_trip() {
        let row = DriftRow {
            severity: crate::reconcile::DriftSeverity::AutoRepairable,
            detector: "topology",
            reason: "x".into(),
        };
        let json = DriftRowJson::from(&row);
        assert_eq!(json.severity, "auto_repairable");
        assert_eq!(json.detector, "topology");
        assert_eq!(json.reason, "x");
    }

    #[test]
    fn tick_plan_json_round_trip_through_serde() {
        let outcome = TickOutcome {
            started_at_ms: 0,
            observed_heartbeats: 0,
            observed_edges: 0,
            desired_edges: 0,
            plan: TickPlanJson {
                repair_now: vec![],
                inbox: vec![],
            },
            duration_ms: 0,
        };
        let s = serde_json::to_string(&outcome).unwrap();
        let back: TickOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(back.observed_heartbeats, 0);
    }

    // ---- mackesd-03: safe-repair apply step ---------------------------

    /// A deterministic [`EdgeProber`] for the repair tests — records
    /// every probe target so a test can assert the repair action ran
    /// (or didn't), with no live network.
    struct FakeProber {
        reachable: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl FakeProber {
        fn new(reachable: bool) -> Self {
            Self {
                reachable,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl EdgeProber for FakeProber {
        fn probe_overlay(&self, overlay_ip: &str) -> ProbeResult {
            self.calls.lock().unwrap().push(overlay_ip.to_owned());
            ProbeResult {
                rtt_ms: if self.reachable { Some(7.0) } else { None },
                reachable: self.reachable,
                path: "overlay",
            }
        }
    }

    fn mk_edge(a: &str, b: &str) -> Edge {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        Edge {
            a: lo.to_owned(),
            b: hi.to_owned(),
            kind: EdgeKind::NebulaDirect,
        }
    }

    fn write_test_bundle(workgroup_root: &Path, peer: &str, overlay_ip: &str) {
        let bundle = crate::ca::bundle::NebulaBundle {
            mesh_id: "mesh".into(),
            epoch: 1,
            ca_cert_pem: String::new(),
            peer_cert_pem: String::new(),
            peer_key_pem: String::new(),
            overlay_ip: overlay_ip.to_owned(),
            mesh_cidr: "10.42.0.0/16".into(),
            lighthouses: vec![],
            ca_key_pem: None,
            created_at: 0,
        };
        let path = crate::ca::bundle::bundle_path(workgroup_root, peer);
        crate::ca::bundle::write_bundle(&path, &bundle).unwrap();
    }

    // (1) A detected SAFE drift row (auto-repairable missing edge
    // incident to this node) is actually repaired: the repair action
    // runs and the post-state records a reachable re-probe.
    #[test]
    fn safe_repair_reprobes_incident_missing_edge() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_test_bundle(root, "peer:remote", "127.0.0.1");
        let mut missing = BTreeSet::new();
        missing.insert(mk_edge("peer:self", "peer:remote"));
        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: true,
            max_per_tick: 16,
        };
        let report = apply_safe_repairs(&prober, "peer:self", root, &missing, &policy);
        // The repair ACTION ran: exactly one probe, to the resolved IP.
        assert_eq!(prober.call_count(), 1);
        assert_eq!(report.attempted, 1);
        assert_eq!(prober.calls.lock().unwrap()[0], "127.0.0.1");
        // Post-state: the edge's outcome is a reachable re-probe.
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(
            report.outcomes[0].1,
            RepairOutcome::Reprobed {
                reachable: true,
                rtt_ms: Some(7),
            },
        );
    }

    // (2) Repair is idempotent: re-running the identical pass is
    // deterministic (no accumulated state), and once the edge recovers
    // (drops out of the missing set) the next reconcile pass is a
    // genuine no-op — zero probes.
    #[test]
    fn safe_repair_is_idempotent_and_no_op_once_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_test_bundle(root, "peer:remote", "127.0.0.1");
        let policy = RepairPolicy {
            enabled: true,
            max_per_tick: 16,
        };
        let mut missing = BTreeSet::new();
        missing.insert(mk_edge("peer:self", "peer:remote"));

        let prober = FakeProber::new(true);
        let r1 = apply_safe_repairs(&prober, "peer:self", root, &missing, &policy);
        assert_eq!(prober.call_count(), 1);

        // Re-run the IDENTICAL pass — a probe mutates nothing, so the
        // report is bit-for-bit the same (no double-applied state).
        let prober_again = FakeProber::new(true);
        let r1_again = apply_safe_repairs(&prober_again, "peer:self", root, &missing, &policy);
        assert_eq!(r1, r1_again);

        // Edge recovered → no longer in the missing set → the next
        // reconcile pass takes NO action.
        let empty: BTreeSet<Edge> = BTreeSet::new();
        let prober2 = FakeProber::new(true);
        let r2 = apply_safe_repairs(&prober2, "peer:self", root, &empty, &policy);
        assert_eq!(prober2.call_count(), 0);
        assert_eq!(r2.attempted, 0);
        assert!(r2.outcomes.is_empty());
    }

    // (3) The per-cycle cap bounds the number of repairs: with 5
    // incident missing edges and a cap of 2, exactly two probes fire
    // and the remaining three are deferred.
    #[test]
    fn safe_repair_cap_bounds_repairs_per_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut missing = BTreeSet::new();
        for i in 0..5 {
            let peer = format!("peer:r{i}");
            write_test_bundle(root, &peer, "127.0.0.1");
            missing.insert(mk_edge("peer:self", &peer));
        }
        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: true,
            max_per_tick: 2,
        };
        let report = apply_safe_repairs(&prober, "peer:self", root, &missing, &policy);
        assert_eq!(prober.call_count(), 2, "cap bounds probes to 2");
        assert_eq!(report.attempted, 2);
        assert!(report.cap_reached);
        let deferred = report
            .outcomes
            .iter()
            .filter(|(_, o)| matches!(o, RepairOutcome::DeferredCapReached))
            .count();
        assert_eq!(deferred, 3);
    }

    // (4) With auto-repair DISABLED via config, no repair action runs —
    // observe-only is preserved even when a safe drift row is present.
    #[test]
    fn safe_repair_disabled_takes_no_action() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_test_bundle(root, "peer:remote", "127.0.0.1");
        let mut missing = BTreeSet::new();
        missing.insert(mk_edge("peer:self", "peer:remote"));
        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: false,
            max_per_tick: 16,
        };
        let report = apply_safe_repairs(&prober, "peer:self", root, &missing, &policy);
        assert_eq!(prober.call_count(), 0);
        assert_eq!(report.attempted, 0);
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.outcomes[0].1, RepairOutcome::Disabled);
    }

    // (5a) An UNSAFE / observe-only case — a missing edge NOT incident
    // to this node — is never auto-repaired: no probe, logged
    // manual-repair-required.
    #[test]
    fn safe_repair_leaves_non_incident_missing_edge_observe_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut missing = BTreeSet::new();
        missing.insert(mk_edge("peer:x", "peer:y")); // this node is neither
        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: true,
            max_per_tick: 16,
        };
        let report = apply_safe_repairs(&prober, "peer:self", root, &missing, &policy);
        assert_eq!(prober.call_count(), 0, "no probe for a non-incident edge");
        assert!(matches!(
            report.outcomes[0].1,
            RepairOutcome::ManualRepairRequired(_)
        ));
    }

    // (5a') An incident missing edge whose peer bundle is missing has
    // no resolvable overlay IP → observe-only, no probe.
    #[test]
    fn safe_repair_unresolvable_peer_is_observe_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // No bundle written for peer:remote → IP unresolved.
        let mut missing = BTreeSet::new();
        missing.insert(mk_edge("peer:self", "peer:remote"));
        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: true,
            max_per_tick: 16,
        };
        let report = apply_safe_repairs(&prober, "peer:self", root, &missing, &policy);
        assert_eq!(prober.call_count(), 0);
        assert!(matches!(
            report.outcomes[0].1,
            RepairOutcome::ManualRepairRequired(_)
        ));
    }

    // (5b) The ManualReview (extra-edge) drift category is never
    // auto-repaired end-to-end: an observed-but-undesired adjacency
    // routes to the inbox and fires zero probes.
    #[test]
    fn tick_never_reprobes_for_extra_edge_manual_review() {
        let dir = tempfile::tempdir().unwrap();
        let workgroup_root = dir.path().join("qnm-shared");
        std::fs::create_dir_all(&workgroup_root).unwrap();
        let db_path = dir.path().join("mackesd.db");
        let _ = crate::store::open(&db_path).unwrap();
        // No desired config → an observed edge is "extra" → manual review.
        make_links(&workgroup_root, "peer:self", "peer:b", Some(10));
        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: true,
            max_per_tick: 16,
        };
        let outcome = tick_with(&workgroup_root, "peer:self", &db_path, &prober, &policy).unwrap();
        assert_eq!(outcome.plan.inbox.len(), 1, "extra edge → inbox");
        assert_eq!(outcome.plan.repair_now.len(), 0);
        assert_eq!(
            prober.call_count(),
            0,
            "manual-review drift is never probed"
        );
    }

    // End-to-end through the injectable tick seam: a safe incident
    // missing edge fires one re-probe AND the audit event records the
    // reprobe action + outcome.
    #[test]
    fn tick_with_reprobes_incident_missing_edge_and_audits_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let workgroup_root = dir.path().join("qnm-shared");
        std::fs::create_dir_all(&workgroup_root).unwrap();
        let db_path = dir.path().join("mackesd.db");
        let conn = crate::store::open(&db_path).unwrap();
        let payload = serde_json::json!({
            "nodes": [
                {"id": "peer:self", "region": "r", "healthy": true, "is_host": true},
                {"id": "peer:remote", "region": "r", "healthy": true, "is_host": false},
            ],
            "allow_east_west": [],
        });
        conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at, applied_at) \
             VALUES (?, ?, ?, 'applied', ?, ?)",
            (
                "tester",
                "seed",
                payload.to_string(),
                "2026-05-19T00:00:00Z",
                "2026-05-19T00:00:00Z",
            ),
        )
        .unwrap();
        drop(conn);
        write_test_bundle(&workgroup_root, "peer:remote", "127.0.0.1");

        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: true,
            max_per_tick: 16,
        };
        let outcome = tick_with(&workgroup_root, "peer:self", &db_path, &prober, &policy).unwrap();
        assert_eq!(outcome.desired_edges, 1);
        assert_eq!(outcome.plan.repair_now.len(), 1);
        // The safe repair fired exactly one probe...
        assert_eq!(prober.call_count(), 1);
        // ...and the audit event names the action + the reprobe outcome.
        let conn2 = crate::store::open(&db_path).unwrap();
        let payload_json: String = conn2
            .query_row(
                "SELECT payload_json FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            payload_json.contains("reprobe_overlay"),
            "audit detail must name the action: {payload_json}",
        );
        assert!(payload_json.contains("reprobed"));
    }

    // The disabled config path end-to-end: even with a desired edge
    // missing, auto_repair=false routes it to the inbox and fires no
    // probe (observe-only fallback preserved through the full tick).
    #[test]
    fn tick_with_disabled_routes_missing_edge_to_inbox_no_probe() {
        let dir = tempfile::tempdir().unwrap();
        let workgroup_root = dir.path().join("qnm-shared");
        std::fs::create_dir_all(&workgroup_root).unwrap();
        let db_path = dir.path().join("mackesd.db");
        let conn = crate::store::open(&db_path).unwrap();
        let payload = serde_json::json!({
            "nodes": [
                {"id": "peer:self", "region": "r", "healthy": true, "is_host": true},
                {"id": "peer:remote", "region": "r", "healthy": true, "is_host": false},
            ],
            "allow_east_west": [],
        });
        conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at, applied_at) \
             VALUES (?, ?, ?, 'applied', ?, ?)",
            (
                "tester",
                "seed",
                payload.to_string(),
                "2026-05-19T00:00:00Z",
                "2026-05-19T00:00:00Z",
            ),
        )
        .unwrap();
        drop(conn);
        write_test_bundle(&workgroup_root, "peer:remote", "127.0.0.1");

        let prober = FakeProber::new(true);
        let policy = RepairPolicy {
            enabled: false,
            max_per_tick: 16,
        };
        let outcome = tick_with(&workgroup_root, "peer:self", &db_path, &prober, &policy).unwrap();
        // Auto-repair off → the missing edge lands in the inbox, not
        // repair_now, and no probe fires.
        assert_eq!(outcome.plan.repair_now.len(), 0);
        assert_eq!(outcome.plan.inbox.len(), 1);
        assert_eq!(prober.call_count(), 0);
    }

    #[test]
    fn repair_policy_default_matches_locked_consts() {
        let p = RepairPolicy::default();
        assert_eq!(p.enabled, DEFAULT_AUTO_REPAIR);
        assert_eq!(p.max_per_tick, DEFAULT_MAX_REPAIRS_PER_TICK);
    }
}

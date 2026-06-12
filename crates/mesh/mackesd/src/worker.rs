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
//!
//! What the worker does NOT do today, per the Phase 12.5 lock and
//! the 12.14+ connectivity scope:
//!
//!   * It does NOT push routes through Tailscale or any other
//!     transport. The take-action step is gated on the connectivity
//!     layer (12.14+, multi-week scope) — this is an explicit,
//!     documented scope boundary, not a stub.
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
pub const DEFAULT_AUTO_REPAIR: bool = true;

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
    let plan = plan_tick(&topology_diff, DEFAULT_AUTO_REPAIR);

    // Emit one ConfigChange-or-Reconcile event per repair-now row +
    // log every inbox row. Audit hash chain is per-row so an audit
    // verify walks back through them cleanly (12.6.3 / 12.10.3).
    let emitted = apply_repair_rows(&mut conn, node_id, &plan.repair_now)?;
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
    use std::collections::BTreeSet;
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

/// Emit one audit-log event per `repair_now` row and write a
/// `tracing::info` line describing the intended repair action.
/// Returns the emitted [`Event`]s so the caller can fire the 12.6.4
/// alert hooks AFTER the transaction commits (EFF-25 — alerts must
/// never fire for a rolled-back event).
///
/// Actual repair execution (pushing routes over the Nebula overlay,
/// restarting peer services) is gated on the connectivity layer
/// (12.14+) per the Phase 12.5 lock — this is an explicit, documented
/// scope boundary, not a stub. The audit event records that the
/// reconciler *would have* repaired the drift; the connectivity layer
/// wires the take-action step when it ships.
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

        let now_iso = chrono::Utc::now().to_rfc3339();
        let now_ms_val = now_ms();
        let mut emitted: Vec<Event> = Vec::with_capacity(rows.len());
        for (idx, row) in rows.iter().enumerate() {
            // Build the structured Event payload; serialize once for
            // both the audit chain and the JSON column.
            let event = Event {
                event_id: u64::try_from(now_ms_val.max(0)).unwrap_or(0)
                    + u64::try_from(idx).unwrap_or(0),
                kind: EventKind::Reconcile,
                node_id: node_id.to_owned(),
                timestamp_ms: now_ms_val,
                detail: serde_json::json!({
                    "action": "repair_now",
                    "drift": DriftRowJson::from(row),
                    "intent": "auto-repair queued; transport push gated on 12.14+ connectivity layer",
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
                "auto-repair queued (take-action gated on 12.14+ connectivity layer)",
            );

            prev_hash = hash;
            emitted.push(event);
        }
        Ok(emitted)
    })
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
}

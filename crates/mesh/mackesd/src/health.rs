//! Health surface (Phase 12.1.3).
//!
//! `HealthReport` is the value type returned by `mackesd healthz`
//! (CLI subcommand) and `mackesd_core::healthz()` (library function
//! the panel imports for the status cluster).
//!
//! Per the 12.1.3 lock the same data surfaces in both places — the
//! CLI prints it as JSON, the library returns the typed struct.

use serde::{Deserialize, Serialize};

/// Top-level health report. Each field is independently reportable
/// so a probe failure on one doesn't poison the others.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Schema version. Bump when the shape changes; the panel uses
    /// this to fall back gracefully if a newer `mackesd` reports a
    /// shape it doesn't recognize yet.
    pub schema: u32,
    /// Is this peer currently the leader? See 12.A.5.
    pub is_leader: bool,
    /// Most recent applied revision (`r-YYYY-MM-DD-NNNN` form).
    /// `None` when the store has never accepted a deploy.
    pub applied_revision: Option<String>,
    /// Count of rows in the `nodes` table (mesh size from this peer's
    /// perspective).
    pub node_count: u32,
    /// Count of rows whose `last_heartbeat` is within the healthy
    /// threshold (per 12.3.3).
    pub healthy_nodes: u32,
    /// Count of rows whose `last_heartbeat` missed exactly one cycle.
    pub degraded_nodes: u32,
    /// Count of rows whose `last_heartbeat` missed 3+ cycles.
    pub unreachable_nodes: u32,
    /// Audit chain status. `true` = `audit::verify()` returned
    /// `Intact`. `false` = the most recent verify reported a break.
    pub audit_chain_intact: bool,
    /// Mackesd version (Cargo package version).
    pub version: String,
    /// EFF-24 — workers currently alive (live only on the Bus healthz,
    /// served in-process by the daemon; the CLI's store-only view
    /// reports 0/0). Serde-defaulted so schema 1 readers/writers
    /// interoperate.
    #[serde(default)]
    pub workers_alive: u32,
    /// EFF-24 — workers spawned this daemon lifetime.
    #[serde(default)]
    pub workers_total: u32,
    /// EFF-24 — count of ENT-6 circuit-breaker trips (a tripped
    /// worker stays down until the daemon restarts).
    #[serde(default)]
    pub breaker_tripped: u32,
    /// EFF-24 — the readiness verdict. Store view: audit chain
    /// intact. Daemon (Bus) view: that AND every worker alive AND no
    /// breaker tripped.
    #[serde(default)]
    pub ready: bool,
}

impl HealthReport {
    /// Current schema version. Bump alongside any breaking field
    /// change. Add a fallback path on the panel side before bumping
    /// so older readers degrade gracefully.
    pub const CURRENT_SCHEMA: u32 = 1;

    /// Build a default report for a fresh peer that has no data
    /// yet. Used by `mackesd healthz` on a just-installed system
    /// before the first reconcile tick.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema: Self::CURRENT_SCHEMA,
            is_leader: false,
            applied_revision: None,
            node_count: 0,
            healthy_nodes: 0,
            degraded_nodes: 0,
            unreachable_nodes: 0,
            audit_chain_intact: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            workers_alive: 0,
            workers_total: 0,
            breaker_tripped: 0,
            // A bare baseline report has verified nothing — not ready.
            ready: false,
        }
    }

    /// EFF-8 — build a LIVE report from the SQLite store: real `node_count` +
    /// healthy/degraded/unreachable buckets (from each node's recorded health)
    /// + `audit_chain_intact` (from `audit::verify`) + version. Each probe is
    /// independent — a query failure leaves that field at its `empty()` default
    /// rather than poisoning the others.
    ///
    /// NOTE (EFF-8 remainder): `is_leader` + `applied_revision` stay at their
    /// defaults (false / None) pending the leader-lease + applied-revision
    /// query plumbing (today the leader check is per-worker + partly stubbed);
    /// wiring those is the remaining half of EFF-8.
    #[must_use]
    pub fn from_store(conn: &rusqlite::Connection) -> Self {
        let mut r = Self::empty();
        if let Ok(nodes) = crate::store::list_nodes(conn) {
            r.node_count = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
            for n in &nodes {
                match n.health.as_str() {
                    "healthy" => r.healthy_nodes += 1,
                    "degraded" => r.degraded_nodes += 1,
                    _ => r.unreachable_nodes += 1,
                }
            }
        }
        if let Ok(rows) = crate::store::load_audit_rows(conn) {
            // Only a detected `Break` is unhealthy; an `Empty` chain (fresh
            // peer, nothing logged yet) is intact-by-vacuity, same as a
            // fully-verified `Intact` chain.
            r.audit_chain_intact = !matches!(
                crate::audit::verify(&rows),
                crate::audit::VerifyOutcome::Break { .. }
            );
        }
        // EFF-24 — the store-only readiness verdict (the daemon's Bus
        // healthz ANDs worker liveness on top — see ipc::shell).
        r.ready = r.audit_chain_intact;
        r
    }

    /// EFF-24 — enrich a store-derived report with live per-worker
    /// status (the daemon-side Bus healthz path). Readiness becomes:
    /// store-ready AND every spawned worker alive AND no ENT-6
    /// breaker tripped.
    #[must_use]
    pub fn with_worker_status(mut self, alive: u32, total: u32, tripped: u32) -> Self {
        self.workers_alive = alive;
        self.workers_total = total;
        self.breaker_tripped = tripped;
        self.ready = self.ready && tripped == 0 && alive == total;
        self
    }

    /// ONBOARD-6 (OB6-FIX-4) — override the mesh-size + leadership fields
    /// from the LIVE directory (QNM-Shared heartbeats) + the leader lease,
    /// instead of the store's `nodes` table. The store only holds rows the
    /// leader has enrolled; the live mesh view is the replicated directory,
    /// which is what `mackesd peers` shows — so the healthz card now matches
    /// the Inventory (was: `node_count:0` / `is_leader:false` stubs).
    #[must_use]
    pub fn with_mesh(
        mut self,
        node_count: u32,
        healthy: u32,
        degraded: u32,
        unreachable: u32,
        is_leader: bool,
    ) -> Self {
        self.node_count = node_count;
        self.healthy_nodes = healthy;
        self.degraded_nodes = degraded;
        self.unreachable_nodes = unreachable;
        self.is_leader = is_leader;
        self
    }

    /// JSON one-liner for `mackesd healthz`. Stable shape — every
    /// field always present, no schema-conditional keys.
    ///
    /// # Errors
    /// Returns `serde_json::Error` only on out-of-memory while
    /// serializing — never on schema-shape issues.
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_has_schema_1() {
        let r = HealthReport::empty();
        assert_eq!(r.schema, 1);
        assert!(!r.is_leader);
        assert!(r.applied_revision.is_none());
        assert_eq!(r.node_count, 0);
    }

    #[test]
    fn json_round_trips() {
        let r = HealthReport::empty();
        let line = r.to_json_line().expect("serialize");
        let back: HealthReport = serde_json::from_str(&line).expect("parse");
        assert_eq!(back.schema, r.schema);
        assert_eq!(back.is_leader, r.is_leader);
        assert_eq!(back.version, r.version);
    }

    #[test]
    fn version_string_matches_cargo() {
        let r = HealthReport::empty();
        assert_eq!(r.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn readiness_folds_worker_status_onto_store_health() {
        // EFF-24 — store-ready + all workers alive + no trip → ready;
        // any breaker trip or a dead worker flips it off.
        let conn = crate::store::open_in_memory().expect("in-memory store");
        let base = HealthReport::from_store(&conn);
        assert!(base.ready, "intact store view is ready");
        let ok = base.clone().with_worker_status(5, 5, 0);
        assert!(ok.ready);
        assert_eq!((ok.workers_alive, ok.workers_total), (5, 5));
        let dead_worker = base.clone().with_worker_status(4, 5, 0);
        assert!(!dead_worker.ready, "a dead worker breaks readiness");
        let tripped = base.with_worker_status(5, 5, 1);
        assert!(!tripped.ready, "a breaker trip breaks readiness");
    }

    #[test]
    fn from_store_reports_live_node_and_audit_state() {
        // EFF-8 — a migrated-but-empty store yields a real (zero-node,
        // intact-chain) report, not the hardcoded baseline-by-accident.
        let conn = crate::store::open_in_memory().expect("in-memory store");
        let r = HealthReport::from_store(&conn);
        assert_eq!(r.node_count, 0);
        assert_eq!(r.healthy_nodes, 0);
        assert!(r.audit_chain_intact, "empty audit chain verifies as intact");
        assert_eq!(r.version, env!("CARGO_PKG_VERSION"));
    }
}

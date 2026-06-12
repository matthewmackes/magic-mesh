//! EFF-9 — Prometheus textfile exporter worker.
//!
//! Before this worker, [`crate::metrics::write_textfile`] +
//! [`crate::metrics::default_textfile_dir`] existed but had zero
//! production callers — the renderer was fully built and tested yet
//! never wrote a file, so a node_exporter textfile collector pointed
//! at `/var/lib/node_exporter/textfile_collector` always found an
//! empty mesh. This worker closes that gap: on a fixed cadence it
//! snapshots the store-derivable control-plane gauges and atomically
//! writes `mackesd.prom`.
//!
//! Tick cadence: 30 s — matches the node_exporter default scrape and
//! keeps the snapshot cheap (one SQLite open + a `list_nodes` scan +
//! an audit-chain verify per tick). A failed open/write logs a warn
//! and the worker keeps ticking; the supervisor's `OnFailure` policy
//! only restarts on a returned `Err`, which this worker never does
//! for transient I/O (it would just thrash on a read-only fs).
//!
//! Scope: the snapshot covers what is reliably derivable from the
//! local store on every tick — mesh node counts bucketed by health,
//! the audit-chain-intact flag, and the applied-migration count.
//! Runtime in-process counters (drift events, reconcile failures,
//! breaker trips) need a shared process-wide registry the workers
//! increment, and cert days-remaining is its own probe (EFF-11);
//! both are follow-ups that will append to the same `mackesd.prom`
//! via additional `Counter`s here.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};
use crate::metrics::{write_textfile, Counter};

/// Default tick cadence. 30 s matches the node_exporter default
/// scrape interval — writing faster than the collector reads wastes
/// I/O, slower leaves the scrape reading a stale snapshot.
pub const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Worker handle. The SQLite handle is opened lazily inside
/// `tick_once` so a transient store-open failure doesn't pin the
/// worker to a stale connection.
pub struct MetricsExporterWorker {
    db_path: PathBuf,
    /// Directory the `mackesd.prom` snapshot is written into — the
    /// node_exporter textfile collector dir
    /// ([`crate::metrics::default_textfile_dir`]).
    textfile_dir: PathBuf,
    /// Path to the Nebula CA cert, for the EFF-11 expiry probe. When
    /// set, each tick emits `mackesd_ca_cert_days_remaining` +
    /// `mackesd_ca_cert_expiry_warning` and logs a warn under the
    /// threshold. `None` (or a missing `nebula-cert`) silently omits
    /// the cert series rather than alerting on an unknown.
    ca_cert_path: Option<PathBuf>,
    /// Override the tick cadence (default [`TICK_INTERVAL`]). Used by
    /// tests to drive the loop without 30 s waits.
    tick: Duration,
}

impl MetricsExporterWorker {
    /// Construct with production defaults: 30 s tick, CA-cert expiry
    /// probe against `ca_cert_path`.
    #[must_use]
    pub fn new(db_path: PathBuf, textfile_dir: PathBuf, ca_cert_path: Option<PathBuf>) -> Self {
        Self {
            db_path,
            textfile_dir,
            ca_cert_path,
            tick: TICK_INTERVAL,
        }
    }

    /// Override the tick cadence — used by tests to avoid 30-second
    /// wall-clock waits.
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }
}

#[async_trait::async_trait]
impl Worker for MetricsExporterWorker {
    fn name(&self) -> &'static str {
        "metrics_exporter"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut interval = tokio::time::interval(self.tick);
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = interval.tick() => {
                    let db = self.db_path.clone();
                    let dir = self.textfile_dir.clone();
                    let ca = self.ca_cert_path.clone();
                    // tick_once is sync (rusqlite + blocking file I/O +
                    // a nebula-cert subprocess) — hop onto a blocking
                    // task so it doesn't pin the tokio scheduler.
                    let _ = tokio::task::spawn_blocking(move || {
                        tick_once(&db, &dir, ca.as_deref());
                    })
                    .await;
                }
            }
        }
    }
}

/// One export pass: open the store, snapshot the counters, write the
/// `mackesd.prom` file. A failure at any stage logs a warn and
/// returns — the worker keeps ticking rather than failing out (a
/// read-only collector dir shouldn't restart-thrash the worker).
///
/// Pulled out as a free function so tests can drive a single pass
/// against a tempdir + in-memory-then-file store without owning the
/// tokio scheduler.
pub fn tick_once(
    db_path: &std::path::Path,
    textfile_dir: &std::path::Path,
    ca_cert_path: Option<&std::path::Path>,
) {
    let conn = match crate::store::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %e,
                db_path = %db_path.display(),
                "metrics-exporter: sqlite open failed; skipping tick",
            );
            return;
        }
    };
    let mut counters = snapshot_counters(&conn);
    if let Some(ca) = ca_cert_path {
        counters.extend(cert_counters(ca, now_unix()));
    }
    match write_textfile(textfile_dir, &counters, &[]) {
        Ok(path) => tracing::debug!(path = %path.display(), "metrics-exporter: wrote snapshot"),
        Err(e) => tracing::warn!(
            error = %e,
            dir = %textfile_dir.display(),
            "metrics-exporter: textfile write failed",
        ),
    }
}

/// Build the control-plane counter set from the live store. Reuses
/// [`crate::health::HealthReport::from_store`] for the node/health/
/// audit fields (single source of truth with `healthz`, EFF-8) and
/// adds the applied-migration count.
///
/// Exposed `pub` so tests assert the counter set directly without
/// going through the file write.
#[must_use]
pub fn snapshot_counters(conn: &rusqlite::Connection) -> Vec<Counter> {
    let h = crate::health::HealthReport::from_store(conn);
    let migrations =
        crate::store::applied_migration_count(conn).map_or(0u64, |n| u64::try_from(n).unwrap_or(0));

    let counter = |name: &'static str, help: &'static str, value: u64| Counter {
        name,
        help,
        value,
        labels: BTreeMap::new(),
    };

    vec![
        counter(
            "mackesd_mesh_nodes_total",
            "Total nodes in this peer's mesh view",
            u64::from(h.node_count),
        ),
        counter(
            "mackesd_mesh_nodes_healthy",
            "Nodes reporting a healthy heartbeat",
            u64::from(h.healthy_nodes),
        ),
        counter(
            "mackesd_mesh_nodes_degraded",
            "Nodes that missed one heartbeat cycle",
            u64::from(h.degraded_nodes),
        ),
        counter(
            "mackesd_mesh_nodes_unreachable",
            "Nodes that missed 3+ heartbeat cycles",
            u64::from(h.unreachable_nodes),
        ),
        counter(
            "mackesd_audit_chain_intact",
            "1 when the audit hash-chain verifies (no Break), else 0",
            u64::from(h.audit_chain_intact),
        ),
        counter(
            "mackesd_store_migrations_applied",
            "Count of applied schema migrations",
            migrations,
        ),
    ]
}

/// EFF-11 — CA-cert expiry series. Probes `ca_cert_path` for days
/// remaining and renders two counters: `mackesd_ca_cert_days_remaining`
/// (clamped at 0 — an expired cert reads as "0 days", and the metric
/// stays representable in the u64 counter value) and
/// `mackesd_ca_cert_expiry_warning` (1 when at/under
/// [`crate::ca::expiry::CERT_EXPIRY_WARN_DAYS`], else 0). Logs a warn
/// under the threshold so the cliff is visible in logs as well as the
/// scrape. Returns an empty vec when the probe can't read the cert
/// (`nebula-cert` missing / unreadable) — an unknown is not an alert.
fn cert_counters(ca_cert_path: &std::path::Path, now_unix: i64) -> Vec<Counter> {
    let Some(days) = crate::ca::expiry::ca_cert_days_remaining(ca_cert_path, now_unix) else {
        return Vec::new();
    };
    let warning = days <= crate::ca::expiry::CERT_EXPIRY_WARN_DAYS;
    if warning {
        tracing::warn!(
            days_remaining = days,
            threshold = crate::ca::expiry::CERT_EXPIRY_WARN_DAYS,
            ca_cert = %ca_cert_path.display(),
            "metrics-exporter: CA cert near expiry — rotate with `mackesd ca rotate`",
        );
    }
    vec![
        Counter {
            name: "mackesd_ca_cert_days_remaining",
            help: "Days until the Nebula CA cert expires (clamped at 0)",
            value: u64::try_from(days.max(0)).unwrap_or(0),
            labels: BTreeMap::new(),
        },
        Counter {
            name: "mackesd_ca_cert_expiry_warning",
            help: "1 when the CA cert is at/under the expiry warn threshold, else 0",
            value: u64::from(warning),
            labels: BTreeMap::new(),
        },
    ]
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{open_in_memory, upsert_node};

    fn fresh_store() -> rusqlite::Connection {
        open_in_memory().expect("in-memory store")
    }

    #[test]
    fn worker_name_matches_tier_table() {
        let w = MetricsExporterWorker::new(
            PathBuf::from("/tmp/db"),
            PathBuf::from("/tmp/tf"),
            None,
        );
        assert_eq!(w.name(), "metrics_exporter");
    }

    #[test]
    fn snapshot_on_empty_store_reports_zero_nodes_and_intact_chain() {
        let conn = fresh_store();
        let counters = snapshot_counters(&conn);
        let by_name: BTreeMap<_, _> = counters.iter().map(|c| (c.name, c.value)).collect();
        assert_eq!(by_name["mackesd_mesh_nodes_total"], 0);
        assert_eq!(by_name["mackesd_audit_chain_intact"], 1);
        // The in-memory store is migrated on open, so this is > 0.
        assert!(by_name["mackesd_store_migrations_applied"] > 0);
    }

    #[test]
    fn snapshot_buckets_nodes_by_health() {
        let conn = fresh_store();
        upsert_node(&conn, "peer:a", "a", "pk", None).expect("seed a");
        upsert_node(&conn, "peer:b", "b", "pk", None).expect("seed b");
        crate::store::set_node_health(&conn, "peer:a", "healthy").expect("health a");
        crate::store::set_node_health(&conn, "peer:b", "unreachable").expect("health b");
        let counters = snapshot_counters(&conn);
        let by_name: BTreeMap<_, _> = counters.iter().map(|c| (c.name, c.value)).collect();
        assert_eq!(by_name["mackesd_mesh_nodes_total"], 2);
        assert_eq!(by_name["mackesd_mesh_nodes_healthy"], 1);
        assert_eq!(by_name["mackesd_mesh_nodes_unreachable"], 1);
    }

    #[test]
    fn tick_once_writes_a_parseable_prom_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("mackesd.db");
        // Materialize a real file-backed store so tick_once's
        // `store::open(db_path)` succeeds.
        {
            let conn = crate::store::open(&db).expect("open file store");
            upsert_node(&conn, "peer:a", "a", "pk", None).expect("seed");
            crate::store::set_node_health(&conn, "peer:a", "healthy").expect("health");
        }
        tick_once(&db, dir.path(), None);
        let prom = std::fs::read_to_string(dir.path().join("mackesd.prom")).expect("prom written");
        assert!(prom.contains("# TYPE mackesd_mesh_nodes_total counter"));
        assert!(prom.contains("mackesd_mesh_nodes_total 1"));
        assert!(prom.contains("mackesd_mesh_nodes_healthy 1"));
    }
}

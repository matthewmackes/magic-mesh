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
//! the audit-chain-intact flag, and the applied-migration count. The
//! runtime in-process counters that need a shared process-wide registry
//! the producers increment now land too: breaker trips + worker restarts
//! ride the supervisor's `WorkerStatusMap` (test-obs-9), and reconcile
//! failures, drift events, and Bus publish errors ride
//! [`crate::metrics::process_counters`] (WL-RUN-002). Cert days-remaining
//! is its own probe (EFF-11). All append to the same `mackesd.prom` via
//! additional `Counter`s here.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};
use crate::metrics::{write_textfile, Counter, Histogram};

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
    /// AUD2-1 — shared handle to the router's `kdc2_router_decision_us`
    /// histogram ([`crate::workers::mesh_router::RouterMetrics`]). Each
    /// tick snapshots it (brief lock + clone) into the textfile's
    /// histogram section, closing the observe-but-never-export seam.
    /// `None` omits the series (tests / a daemon without the router).
    router_decision_us: Option<std::sync::Arc<std::sync::Mutex<Histogram>>>,
    /// EFF-26 — the supervisor's live worker registry (EFF-24): emits
    /// `mackesd_workers_alive/total` + `mackesd_breaker_tripped` and
    /// error-logs on any breaker trip (the headless alert).
    worker_status: Option<crate::workers::WorkerStatusMap>,
    /// EFF-26 — filesystems to report disk headroom for (the QNM
    /// replicated root + the store's filesystem). Gauge + warn under
    /// 10% free.
    disk_paths: Vec<PathBuf>,
    /// EFF-26 — the daily `state-backup.enc` path; drives the
    /// backup-staleness gauge/alert.
    backup_file: Option<PathBuf>,
    /// EFF-21/26 — whether the backup passphrase was provided at boot
    /// (env captured-then-scrubbed by run_serve, or systemd-creds; the
    /// env can no longer be read per tick BECAUSE it is scrubbed).
    backup_passphrase_set: bool,
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
            router_decision_us: None,
            worker_status: None,
            disk_paths: Vec::new(),
            backup_file: None,
            backup_passphrase_set: false,
            tick: TICK_INTERVAL,
        }
    }

    /// AUD2-1 — attach the router's shared decision-time histogram so
    /// the per-tick snapshot exports it alongside the counters.
    #[must_use]
    pub fn with_router_metrics(
        mut self,
        metrics: std::sync::Arc<std::sync::Mutex<Histogram>>,
    ) -> Self {
        self.router_decision_us = Some(metrics);
        self
    }

    /// EFF-26 — attach the supervisor's live worker registry.
    #[must_use]
    pub fn with_worker_status(mut self, map: crate::workers::WorkerStatusMap) -> Self {
        self.worker_status = Some(map);
        self
    }

    /// EFF-26 — filesystems whose headroom is reported each tick.
    #[must_use]
    pub fn with_disk_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.disk_paths = paths;
        self
    }

    /// EFF-26 — the daily backup bundle path (staleness alert).
    #[must_use]
    pub fn with_backup_file(mut self, path: PathBuf) -> Self {
        self.backup_file = Some(path);
        self
    }

    /// EFF-21/26 — record whether the backup passphrase was provided
    /// at boot (env-captured or creds; see run_serve).
    #[must_use]
    pub fn with_backup_passphrase_set(mut self, set: bool) -> Self {
        self.backup_passphrase_set = set;
        self
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
                    let inputs = TickInputs {
                        db_path: self.db_path.clone(),
                        textfile_dir: self.textfile_dir.clone(),
                        ca_cert_path: self.ca_cert_path.clone(),
                        router_decision_us: self.router_decision_us.clone(),
                        worker_status: self.worker_status.clone(),
                        disk_paths: self.disk_paths.clone(),
                        backup_file: self.backup_file.clone(),
                        backup_passphrase_set: self.backup_passphrase_set,
                    };
                    // tick_once is sync (rusqlite + blocking file I/O +
                    // subprocesses) — hop onto a blocking task so it
                    // doesn't pin the tokio scheduler.
                    let _ = tokio::task::spawn_blocking(move || {
                        tick_once(&inputs);
                    })
                    .await;
                }
            }
        }
    }
}

/// Owned per-tick inputs (built from the worker, moved into the
/// blocking task; also how tests drive a single pass).
#[derive(Default)]
pub struct TickInputs {
    /// SQLite store path.
    pub db_path: PathBuf,
    /// Textfile-collector output dir.
    pub textfile_dir: PathBuf,
    /// EFF-11 CA-cert expiry probe target.
    pub ca_cert_path: Option<PathBuf>,
    /// AUD2-1 router histogram handle.
    pub router_decision_us: Option<std::sync::Arc<std::sync::Mutex<Histogram>>>,
    /// EFF-26/EFF-24 worker registry.
    pub worker_status: Option<crate::workers::WorkerStatusMap>,
    /// EFF-26 disk-headroom targets.
    pub disk_paths: Vec<PathBuf>,
    /// EFF-26 backup bundle (staleness alert).
    pub backup_file: Option<PathBuf>,
    /// EFF-21/26 — boot-time passphrase presence (env or creds).
    pub backup_passphrase_set: bool,
}

/// One export pass: open the store, snapshot the counters, write the
/// `mackesd.prom` file. A failure at any stage logs a warn and
/// returns — the worker keeps ticking rather than failing out (a
/// read-only collector dir shouldn't restart-thrash the worker).
///
/// Pulled out as a free function so tests can drive a single pass
/// against a tempdir + in-memory-then-file store without owning the
/// tokio scheduler.
pub fn tick_once(inputs: &TickInputs) {
    let conn = match crate::store::open(&inputs.db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %e,
                db_path = %inputs.db_path.display(),
                "metrics-exporter: sqlite open failed; skipping tick",
            );
            return;
        }
    };
    let mut counters = snapshot_counters(&conn);
    if let Some(ca) = &inputs.ca_cert_path {
        counters.extend(cert_counters(ca, now_unix()));
    }
    // EFF-26/EFF-24 — worker liveness + breaker gauges; a trip is a
    // headless-visible ERROR (the alert).
    if let Some(map) = &inputs.worker_status {
        counters.extend(worker_counters(map));
    }
    // EFF-26 — disk headroom per configured filesystem.
    for path in &inputs.disk_paths {
        counters.extend(disk_counters(path));
    }
    // EFF-26 — backup posture: passphrase set? bundle fresh? The env
    // var is scrubbed at boot (EFF-21), so presence comes from the
    // boot-time flag OR'd with the systemd-creds file.
    counters.extend(backup_counters(
        inputs.backup_file.as_deref(),
        inputs.backup_passphrase_set || creds_backup_present(),
        now_unix(),
    ));
    // WL-RUN-002 — process-wide runtime counters incremented live at
    // their producer sites (reconcile-loop failures, drift events, Bus
    // publish errors). Snapshot them into the same textfile so the
    // observe-but-never-export seam the module header flagged is closed.
    counters.extend(crate::metrics::process_counters());
    // AUD2-1 — snapshot the router's decision-time histogram (brief
    // lock + clone) so the SLO instrumentation actually reaches the
    // scrape instead of being observed-and-dropped.
    let histograms: Vec<Histogram> = inputs
        .router_decision_us
        .as_ref()
        .map(|m| {
            vec![m
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()]
        })
        .unwrap_or_default();
    match write_textfile(&inputs.textfile_dir, &counters, &histograms) {
        Ok(path) => tracing::debug!(path = %path.display(), "metrics-exporter: wrote snapshot"),
        Err(e) => tracing::warn!(
            error = %e,
            dir = %inputs.textfile_dir.display(),
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

/// EFF-26/EFF-24 — worker-liveness gauges from the supervisor's
/// registry. A breaker trip error-logs every tick it persists (the
/// headless journal alert — deliberately repeated so a log pipeline's
/// time-window alerts keep firing while the condition holds).
///
/// test-obs-9 — beyond the point-in-time gauges (`mackesd_workers_alive`
/// / `_total` / `_breaker_tripped`), this also emits the two CUMULATIVE
/// per-worker FAILURE counters the supervisor increments live at its real
/// restart + trip sites (`workers/mod.rs`): `mackesd_worker_restarts_total`
/// and `mackesd_breaker_trips_total`, both labelled `{worker=…}`. These
/// monotonic totals are the scrapeable signal alerting rules `rate()` over
/// (a gauge that clears on recovery can't drive a "worker keeps dying" alert).
fn worker_counters(map: &crate::workers::WorkerStatusMap) -> Vec<Counter> {
    let (alive, total, tripped) = crate::workers::workers_ready(map);
    if tripped > 0 {
        tracing::error!(
            target: "mackesd::alert",
            tripped,
            "ALERT (crit): circuit breaker tripped — worker(s) down until mackesd restarts",
        );
    }
    let mk = |name: &'static str, help: &'static str, value: u64| Counter {
        name,
        help,
        value,
        labels: BTreeMap::new(),
    };
    let mut out = vec![
        mk(
            "mackesd_workers_alive",
            "Workers currently alive",
            u64::from(alive),
        ),
        mk(
            "mackesd_workers_total",
            "Workers spawned this daemon lifetime",
            u64::from(total),
        ),
        mk(
            "mackesd_breaker_tripped",
            "ENT-6 circuit-breaker trips (worker stays down until restart)",
            u64::from(tripped),
        ),
    ];
    // test-obs-9 — snapshot the per-worker cumulative failure totals
    // (brief lock; strings built after release).
    let per_worker: Vec<(&'static str, u32, u32)> = {
        let g = map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.values()
            .map(|w| (w.name, w.restarts, w.breaker_trips))
            .collect()
    };
    for (name, restarts, trips) in per_worker {
        let mut labels = BTreeMap::new();
        labels.insert("worker".to_owned(), name.to_owned());
        out.push(Counter {
            name: "mackesd_worker_restarts_total",
            help: "Cumulative worker restarts since daemon start",
            value: u64::from(restarts),
            labels: labels.clone(),
        });
        out.push(Counter {
            name: "mackesd_breaker_trips_total",
            help: "Cumulative circuit-breaker trips (closed->open) since daemon start",
            value: u64::from(trips),
            labels,
        });
    }
    out
}

/// EFF-26 — threshold below which free space warns.
const DISK_WARN_FREE_PCT: u64 = 10;

/// EFF-26 — free-bytes gauge for one filesystem via `df -B1` (bounded
/// by the EFF-20 timeout helper; no libc/statvfs dep). Warns under
/// [`DISK_WARN_FREE_PCT`]% free. Empty vec when `df` fails.
fn disk_counters(path: &std::path::Path) -> Vec<Counter> {
    let mut cmd = std::process::Command::new("df");
    cmd.arg("-B1").arg("--output=avail,pcent").arg(path);
    let Ok(out) =
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Second line: "<avail> <used%>".
    let Some(line) = text.lines().nth(1) else {
        return Vec::new();
    };
    let mut parts = line.split_whitespace();
    let (Some(avail), Some(pcent)) = (parts.next(), parts.next()) else {
        return Vec::new();
    };
    let Ok(avail_bytes) = avail.parse::<u64>() else {
        return Vec::new();
    };
    let used_pct: u64 = pcent.trim_end_matches('%').parse().unwrap_or(0);
    let free_pct = 100u64.saturating_sub(used_pct);
    if free_pct < DISK_WARN_FREE_PCT {
        tracing::warn!(
            target: "mackesd::alert",
            path = %path.display(),
            free_pct,
            "ALERT (warn): disk headroom low",
        );
    }
    let mut labels = BTreeMap::new();
    labels.insert("path".to_owned(), path.display().to_string());
    vec![Counter {
        name: "mackesd_disk_available_bytes",
        help: "Free bytes on the monitored filesystem",
        value: avail_bytes,
        labels,
    }]
}

/// EFF-26 — staleness threshold: the backup worker runs daily, so a
/// bundle older than 48 h means at least one missed cycle.
const BACKUP_STALE_SECS: i64 = 48 * 60 * 60;

/// EFF-26 — backup-posture gauges/alerts. Exposed for tests.
pub fn backup_counters(
    backup_file: Option<&std::path::Path>,
    passphrase_set: bool,
    now_unix: i64,
) -> Vec<Counter> {
    let mk = |name: &'static str, help: &'static str, value: u64| Counter {
        name,
        help,
        value,
        labels: BTreeMap::new(),
    };
    let mut out = vec![mk(
        "mackesd_backup_passphrase_set",
        "1 when MDE_BACKUP_PASSPHRASE is set (daily CA/state backup enabled)",
        u64::from(passphrase_set),
    )];
    if !passphrase_set {
        tracing::warn!(
            target: "mackesd::alert",
            "ALERT (warn): MDE_BACKUP_PASSPHRASE unset — the daily state backup is DISABLED",
        );
    }
    if let Some(path) = backup_file {
        if let Ok(meta) = std::fs::metadata(path) {
            let age_secs = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(i64::MAX, |d| {
                    now_unix.saturating_sub(i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
                });
            out.push(mk(
                "mackesd_backup_age_seconds",
                "Seconds since the state-backup bundle was last written",
                u64::try_from(age_secs.max(0)).unwrap_or(u64::MAX),
            ));
            if passphrase_set && age_secs > BACKUP_STALE_SECS {
                tracing::warn!(
                    target: "mackesd::alert",
                    age_hours = age_secs / 3600,
                    path = %path.display(),
                    "ALERT (warn): state backup is stale (daily cycle missed)",
                );
            }
        } else if passphrase_set {
            // Passphrase set but no bundle yet — first cycle pending or
            // the worker is failing; surface it.
            tracing::warn!(
                target: "mackesd::alert",
                path = %path.display(),
                "ALERT (warn): backup enabled but no state-backup bundle exists yet",
            );
        }
    }
    out
}

/// EFF-21/26 — true when the systemd-creds backup passphrase file
/// exists non-empty (`$CREDENTIALS_DIRECTORY/backup-passphrase`).
fn creds_backup_present() -> bool {
    std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(|d| std::path::Path::new(&d).join("backup-passphrase"))
        .and_then(|p| std::fs::metadata(p).ok())
        .is_some_and(|m| m.len() > 0)
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
        let w =
            MetricsExporterWorker::new(PathBuf::from("/tmp/db"), PathBuf::from("/tmp/tf"), None);
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
        tick_once(&TickInputs {
            db_path: db.clone(),
            textfile_dir: dir.path().to_path_buf(),
            ..TickInputs::default()
        });
        let prom = std::fs::read_to_string(dir.path().join("mackesd.prom")).expect("prom written");
        assert!(prom.contains("# TYPE mackesd_mesh_nodes_total counter"));
        assert!(prom.contains("mackesd_mesh_nodes_total 1"));
        assert!(prom.contains("mackesd_mesh_nodes_healthy 1"));
        // WL-RUN-002 — the process-wide runtime counters render via the
        // SAME exporter each tick, under their stable `mackesd_*_total`
        // names (value-agnostic: they're process globals other tests may
        // have bumped).
        assert!(prom.contains("# TYPE mackesd_reconcile_failures_total counter"));
        assert!(prom.contains("# TYPE mackesd_drift_events_total counter"));
        assert!(prom.contains("# TYPE mackesd_bus_publish_errors_total counter"));
    }

    #[test]
    fn backup_counters_report_posture_and_staleness() {
        // EFF-26 — passphrase flag + bundle age land as gauges; a
        // fresh bundle isn't stale.
        let tmp = tempfile::tempdir().expect("tmp");
        let bundle = tmp.path().join("state-backup.enc");
        std::fs::write(&bundle, b"enc").expect("write bundle");
        let now = i64::try_from(
            std::fs::metadata(&bundle)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();
        let counters = backup_counters(Some(&bundle), true, now + 60);
        let by_name: BTreeMap<_, _> = counters.iter().map(|c| (c.name, c.value)).collect();
        assert_eq!(by_name["mackesd_backup_passphrase_set"], 1);
        assert!(by_name["mackesd_backup_age_seconds"] < 3_600);

        // Unset passphrase: flag 0, no age gauge required.
        let counters = backup_counters(None, false, now);
        let by_name: BTreeMap<_, _> = counters.iter().map(|c| (c.name, c.value)).collect();
        assert_eq!(by_name["mackesd_backup_passphrase_set"], 0);
    }

    #[test]
    fn tick_once_with_worker_status_and_disk_paths_exports_gauges() {
        // EFF-26 — the full tick writes worker + disk + backup series.
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("mackesd.db");
        let _ = crate::store::open(&db).expect("open file store");
        let status = crate::workers::new_status_map();
        tick_once(&TickInputs {
            db_path: db,
            textfile_dir: dir.path().to_path_buf(),
            worker_status: Some(status),
            disk_paths: vec![dir.path().to_path_buf()],
            ..TickInputs::default()
        });
        let prom = std::fs::read_to_string(dir.path().join("mackesd.prom")).expect("written");
        assert!(prom.contains("mackesd_workers_total 0"));
        assert!(prom.contains("mackesd_breaker_tripped 0"));
        assert!(prom.contains("mackesd_backup_passphrase_set"));
        // df runs on real systems; tolerate absence (container without df)
        // by not hard-asserting the disk series — but when present it must
        // carry the path label.
        if prom.contains("mackesd_disk_available_bytes") {
            assert!(prom.contains(r#"path=""#));
        }
    }

    #[test]
    fn tick_once_exports_per_worker_failure_counters() {
        // test-obs-9 — a populated worker registry renders the two
        // cumulative FAILURE counters the supervisor maintains at its
        // real restart/trip sites, as labelled Prometheus counters with
        // stable names — and the header de-dupe holds across the many
        // per-worker series.
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("mackesd.db");
        let _ = crate::store::open(&db).expect("open file store");
        let status = crate::workers::new_status_map();
        {
            let mut g = status.lock().unwrap();
            g.insert(
                "chat",
                crate::workers::WorkerStatus {
                    name: "chat",
                    alive: true,
                    restarts: 3,
                    breaker_tripped: false,
                    breaker_trips: 1,
                    last_exit_ok: Some(false),
                },
            );
            g.insert(
                "heartbeat",
                crate::workers::WorkerStatus {
                    name: "heartbeat",
                    alive: true,
                    restarts: 0,
                    breaker_tripped: false,
                    breaker_trips: 0,
                    last_exit_ok: None,
                },
            );
        }
        tick_once(&TickInputs {
            db_path: db,
            textfile_dir: dir.path().to_path_buf(),
            worker_status: Some(status),
            ..TickInputs::default()
        });
        let prom = std::fs::read_to_string(dir.path().join("mackesd.prom")).expect("written");
        // Header emitted exactly once per metric name despite two series.
        assert_eq!(
            prom.matches("# TYPE mackesd_worker_restarts_total counter")
                .count(),
            1,
        );
        assert_eq!(
            prom.matches("# TYPE mackesd_breaker_trips_total counter")
                .count(),
            1,
        );
        // The wired totals land as labelled series.
        assert!(prom.contains(r#"mackesd_worker_restarts_total{worker="chat"} 3"#));
        assert!(prom.contains(r#"mackesd_breaker_trips_total{worker="chat"} 1"#));
        assert!(prom.contains(r#"mackesd_worker_restarts_total{worker="heartbeat"} 0"#));
        assert!(prom.contains(r#"mackesd_breaker_trips_total{worker="heartbeat"} 0"#));
    }

    #[test]
    fn tick_once_exports_the_router_histogram_when_attached() {
        // AUD2-1 — an attached router histogram lands in the textfile
        // as a full Prometheus histogram series.
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("mackesd.db");
        let _ = crate::store::open(&db).expect("open file store");
        let hist = std::sync::Arc::new(std::sync::Mutex::new(
            crate::metrics::kdc2_router_decision_us(),
        ));
        hist.lock().unwrap().observe(250.0);
        hist.lock().unwrap().observe(1_500.0);
        tick_once(&TickInputs {
            db_path: db.clone(),
            textfile_dir: dir.path().to_path_buf(),
            router_decision_us: Some(hist),
            ..TickInputs::default()
        });
        let prom = std::fs::read_to_string(dir.path().join("mackesd.prom")).expect("prom written");
        assert!(prom.contains("# TYPE kdc2_router_decision_us histogram"));
        assert!(prom.contains("kdc2_router_decision_us_count 2"));
        assert!(prom.contains(r#"kdc2_router_decision_us_bucket{le="+Inf"} 2"#));
    }
}

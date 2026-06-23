//! Shell control (health, version, worker pool status).
//!
//! E0.3.5 (EPIC-RETIRE-DBUS, 2026-06-04): served on the mesh **Bus**
//! at `action/shell/{version,healthz,workers}` via [`serve_bus`] /
//! [`build_reply`], replacing the retired `dev.mackes.MDE.Shell`
//! D-Bus `#[interface]` (mirrors the Nebula migration, E0.3.1). The
//! responder runs on its own OS thread (`Persist`/rusqlite isn't
//! `Send`) — but unlike Nebula's it needs no tokio runtime, since
//! the builders are synchronous (version is a const, healthz builds
//! `HealthReport::empty()`, workers is a lock-and-clone).
//!
//! The service carries a `ShellState` with the daemon's db_path + a
//! shared `Vec<String>` of live worker names that `run_serve`
//! populates as it spawns each supervisor child. `healthz` returns
//! the same `HealthReport` envelope as the `mackesd healthz` CLI so
//! reads stay in parity; the only live consumer today is the
//! Workbench Overview's `probe_mackesd_alive` liveness check.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// Object exposed at `/dev/mackes/MDE/Shell`.
#[derive(Debug, Clone)]
pub struct ShellService {
    state: Arc<ShellState>,
}

impl Default for ShellService {
    fn default() -> Self {
        Self {
            state: Arc::new(ShellState::default()),
        }
    }
}

/// Live state the daemon binds at registration time. Cheap to
/// share via `Arc` so the service handle stays `Clone` for
/// zbus's interface registration.
///
/// `worker_names` is a shared `Mutex<Vec<String>>` because
/// some workers spawn AFTER ShellService registers (KDC host,
/// reconcile). `run_serve` pushes to the shared vec at each
/// spawn site so Workers() always reflects what's currently
/// running, not a snapshot frozen at registration time.
#[derive(Debug, Default, Clone)]
pub struct ShellState {
    /// Sqlite store path. Healthz reads via this on every call so
    /// the report reflects current store contents, not a snapshot
    /// taken at registration time. Empty when the daemon is
    /// running without a store (only the `version` method works
    /// in that case).
    pub db_path: PathBuf,
    /// Shared roster of spawned worker names, in spawn order.
    /// The daemon writes (push) during each `sup.spawn()` call
    /// site; Workers() reads (lock + clone) on each D-Bus call.
    /// Held across awaits in the zbus method but only via
    /// momentary lock acquisition (clone-then-drop pattern), so
    /// there's no deadlock risk with the tokio scheduler.
    pub worker_names: Arc<std::sync::Mutex<Vec<String>>>,
    /// EFF-24 — the supervisor's live per-worker status registry.
    /// `None` in tests / minimal boots; when present, `healthz`
    /// reports `workers_alive`/`workers_total`/`breaker_tripped`
    /// and folds them into the `ready` verdict.
    pub worker_status: Option<crate::workers::WorkerStatusMap>,
    /// ONBOARD-6 (OB6-FIX-4) — QNM-Shared root + this node's id, so
    /// `healthz` reports `node_count`/`is_leader` from the live directory
    /// + leader lease (not the store's enrolled-nodes stub). Empty root
    /// disables the enrichment (the store-derived counts stand).
    pub workgroup_root: PathBuf,
    /// Stable node id, for the leader-lease holder check.
    pub node_id: String,
}

impl ShellService {
    /// Construct against a live `ShellState`. Used by
    /// `register_shell_on` in `run_serve`.
    #[must_use]
    pub fn new(state: ShellState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }
}

/// Action verbs served on `action/shell/<verb>` (E0.3.5).
pub const ACTION_VERBS: [&str; 3] = ["version", "healthz", "workers"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// SHELL-RPC-1 — hard cap on the mount-touching mesh enrichment in `healthz`.
/// A healthy QNM-Shared read returns in single-digit ms; anything beyond this
/// means the FUSE mount is wedged, so we drop the enrichment and keep the
/// liveness probe answering. Generous enough not to trip on a loaded mount.
pub const MESH_ENRICH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

impl ShellService {
    /// Compiled crate version (`CARGO_PKG_VERSION`).
    #[must_use]
    pub fn build_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// JSON-encoded [`crate::health::HealthReport`] — the same live
    /// report the `mackesd healthz` CLI emits (EFF-8), so panel reads
    /// stay in parity. Opens the store at `state.db_path` on each call
    /// so the report reflects current contents; falls back to the
    /// zero-node `empty()` baseline when no store is bound or the open
    /// fails (e.g. the daemon is running version-only).
    ///
    /// # Errors
    /// Returns the encode error string if the report won't serialize.
    pub fn build_healthz(&self) -> Result<String, String> {
        let report = if self.state.db_path.as_os_str().is_empty() {
            crate::health::HealthReport::empty()
        } else {
            match crate::store::open(&self.state.db_path) {
                Ok(conn) => crate::health::HealthReport::from_store(&conn),
                Err(_) => crate::health::HealthReport::empty(),
            }
        };
        // EFF-24 — fold live worker status into the daemon-side report
        // (the in-process view the CLI's store-only healthz can't see).
        let report = match &self.state.worker_status {
            Some(map) => {
                let (alive, total, tripped) = crate::workers::workers_ready(map);
                report.with_worker_status(alive, total, tripped)
            }
            None => report,
        };
        // OB6-FIX-4 — override node_count/health-buckets/is_leader from the
        // live directory + leader lease (the store nodes table is the wrong
        // source — it only holds enrolled rows, so it read 0 on every peer).
        //
        // SHELL-RPC-1 — this enrichment reads the shared QNM-Shared FUSE mount
        // (`read_peers`/lease over /mnt/mesh-storage). healthz is the daemon
        // LIVENESS probe (`probe_mackesd_alive`), and the single-threaded shell
        // responder serves version/healthz/workers in one loop — so if a wedged
        // mount blocks this read, it hangs the WHOLE responder and every verb
        // times out (the field symptom). A liveness surface must never depend on
        // the shared mount being healthy (the boot-recovery window mounts it
        // AFTER the daemon). So the mount-touching enrichment runs under a hard
        // timeout; on a stall it's skipped and the base report stands — healthz
        // still answers truthfully about the daemon itself.
        let report = if self.state.workgroup_root.as_os_str().is_empty() {
            report
        } else {
            match self.mesh_enrichment_bounded(MESH_ENRICH_TIMEOUT) {
                Some((n, healthy, degraded, unreachable, is_leader, lighthouses)) => {
                    report.with_mesh(n, healthy, degraded, unreachable, is_leader, lighthouses)
                }
                None => {
                    tracing::warn!(
                        root = %self.state.workgroup_root.display(),
                        "healthz: mesh enrichment timed out (wedged QNM-Shared mount?) \
                         — answering with the daemon-local report (SHELL-RPC-1)"
                    );
                    report
                }
            }
        };
        report
            .to_json_line()
            .map_err(|e| format!("healthz encode: {e}"))
    }

    /// Run the mount-touching mesh enrichment on a short-lived helper thread
    /// and wait at most `timeout` for it. `None` means it didn't finish in
    /// time (a wedged/slow shared mount) — the caller falls back to the
    /// daemon-local report rather than blocking the responder. SHELL-RPC-1.
    fn mesh_enrichment_bounded(
        &self,
        timeout: std::time::Duration,
    ) -> Option<(u32, u32, u32, u32, bool, u32)> {
        let root = self.state.workgroup_root.clone();
        let db_path = self.state.db_path.clone();
        let node_id = self.state.node_id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        // Detached: if the read is truly wedged the helper leaks until the
        // mount recovers — acceptable, and far better than stalling the
        // responder. We just stop waiting for it.
        std::thread::Builder::new()
            .name("healthz-mesh-enrich".into())
            .spawn(move || {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let svc = crate::ipc::directory::DirectoryService::new(&root, Some(db_path));
                let counts = svc.mesh_health_counts(&node_id, now_ms);
                let _ = tx.send(counts);
            })
            .ok()?;
        rx.recv_timeout(timeout).ok()
    }

    /// Currently-spawned worker names, in spawn order — sourced from
    /// the `ShellState::worker_names` shared roster the daemon pushes
    /// to at each `sup.spawn()`. Brief lock (clone-then-drop).
    ///
    /// # Errors
    /// Returns the lock-poison error string if the mutex is poisoned.
    pub fn build_workers(&self) -> Result<Vec<String>, String> {
        self.state
            .worker_names
            .lock()
            .map(|g| g.clone())
            .map_err(|e| format!("worker_names lock: {e}"))
    }
}

/// Action topic for verb `verb`: `action/shell/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/shell/{verb}")
}

/// Build the reply body for one `action/shell/<verb>` request.
/// `version` → the raw version string; `healthz` → the
/// `HealthReport` JSON line; `workers` → a JSON `Vec<String>`. On a
/// builder error or unknown verb the body is `{"error": "..."}` so
/// the caller surfaces a diagnostic rather than timing out.
#[must_use]
pub fn build_reply(svc: &ShellService, verb: &str) -> String {
    match verb {
        "version" => svc.build_version().to_string(),
        "healthz" => match svc.build_healthz() {
            Ok(json_line) => json_line,
            Err(e) => json!({ "error": e }).to_string(),
        },
        "workers" => match svc.build_workers() {
            Ok(names) => serde_json::to_string(&names)
                .unwrap_or_else(|e| json!({ "error": format!("encode: {e}") }).to_string()),
            Err(e) => json!({ "error": e }).to_string(),
        },
        other => json!({ "error": format!("unknown shell verb: {other}") }).to_string(),
    }
}

/// Run the Shell Bus responder loop on the current thread until
/// `should_stop()`. Unlike the Nebula responder this needs no tokio
/// runtime — the builders are synchronous; `Persist` ops are sync
/// too. `mackesd` `run_serve` spawns this on a dedicated OS thread
/// (`Persist`/rusqlite isn't `Send`).
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &ShellService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out so a test can
/// drive it without the sleep loop). For each new request on
/// `action/shell/<verb>`, writes [`build_reply`] to `reply/<ulid>`.
pub fn poll_once(persist: &Persist, svc: &ShellService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "shell responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = build_reply(svc, verb);
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "shell responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_crate() {
        let svc = ShellService::default();
        assert_eq!(svc.build_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn action_verbs_and_topic_lock() {
        assert_eq!(ACTION_VERBS, ["version", "healthz", "workers"]);
        assert_eq!(action_topic("healthz"), "action/shell/healthz");
    }

    #[test]
    fn healthz_returns_health_report_json() {
        let svc = ShellService::default();
        let json = svc.build_healthz().expect("healthz");
        // Round-trips to HealthReport with the current schema.
        let parsed: crate::health::HealthReport = serde_json::from_str(&json).expect("decode");
        assert_eq!(parsed.schema, crate::health::HealthReport::CURRENT_SCHEMA);
        assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn workers_returns_empty_for_default_state() {
        let svc = ShellService::default();
        assert!(svc.build_workers().expect("workers").is_empty());
    }

    #[test]
    fn workers_reflects_state_snapshot_in_spawn_order() {
        let names_shared = Arc::new(std::sync::Mutex::new(vec![
            "clipboard".to_string(),
            "mdns".into(),
            "app_sync".into(),
            "heartbeat".into(),
            "mesh_router".into(),
        ]));
        let state = ShellState {
            db_path: PathBuf::from("/tmp/test.sqlite"),
            worker_names: Arc::clone(&names_shared),
            worker_status: None,
            workgroup_root: PathBuf::new(),
            node_id: String::new(),
        };
        let svc = ShellService::new(state);
        assert_eq!(
            svc.build_workers().expect("workers"),
            vec![
                "clipboard".to_string(),
                "mdns".into(),
                "app_sync".into(),
                "heartbeat".into(),
                "mesh_router".into(),
            ]
        );
    }

    #[test]
    fn workers_reflects_post_registration_appends() {
        // The daemon binds ShellState BEFORE spawning every worker
        // (KDC + reconcile spawn after IPC bootstrap). The shared
        // Mutex must pick up post-bind pushes.
        let names_shared = Arc::new(std::sync::Mutex::new(vec!["clipboard".to_string()]));
        let state = ShellState {
            db_path: PathBuf::new(),
            worker_names: Arc::clone(&names_shared),
            worker_status: None,
            workgroup_root: PathBuf::new(),
            node_id: String::new(),
        };
        let svc = ShellService::new(state);
        names_shared.lock().unwrap().push("kdc_host".into());
        assert_eq!(
            svc.build_workers().expect("workers"),
            vec!["clipboard".to_string(), "kdc_host".into()]
        );
    }

    #[test]
    fn build_reply_covers_each_verb_and_unknown() {
        // E0.3.5 — the Bus responder's per-verb reply bodies.
        let svc = ShellService::default();
        assert_eq!(build_reply(&svc, "version"), env!("CARGO_PKG_VERSION"));
        let h: crate::health::HealthReport =
            serde_json::from_str(&build_reply(&svc, "healthz")).expect("healthz json");
        assert_eq!(h.schema, crate::health::HealthReport::CURRENT_SCHEMA);
        let w: Vec<String> =
            serde_json::from_str(&build_reply(&svc, "workers")).expect("workers json");
        assert!(w.is_empty());
        assert!(build_reply(&svc, "bogus").contains("unknown shell verb"));
    }

    #[test]
    fn shell_state_default_carries_empty_paths_and_workers() {
        let s = ShellState::default();
        assert_eq!(s.db_path, PathBuf::new());
        assert!(s.worker_names.lock().unwrap().is_empty());
    }

    /// SHELL-RPC-1 — healthz with a live workgroup_root answers promptly and
    /// stays valid JSON (the mount enrichment runs under MESH_ENRICH_TIMEOUT
    /// and folds in; an empty mount just yields zero mesh counts).
    #[test]
    fn healthz_with_workgroup_root_answers_within_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ShellState {
            db_path: PathBuf::new(),
            worker_names: Arc::new(std::sync::Mutex::new(vec![])),
            worker_status: None,
            workgroup_root: tmp.path().to_path_buf(),
            node_id: "node-a".into(),
        };
        let svc = ShellService::new(state);
        let start = std::time::Instant::now();
        let json = svc.build_healthz().expect("healthz");
        // Comfortably under the enrichment cap even on a slow CI disk.
        assert!(
            start.elapsed() < MESH_ENRICH_TIMEOUT + std::time::Duration::from_secs(1),
            "healthz blocked too long: {:?}",
            start.elapsed()
        );
        let parsed: crate::health::HealthReport = serde_json::from_str(&json).expect("decode");
        assert_eq!(parsed.schema, crate::health::HealthReport::CURRENT_SCHEMA);
        // Empty mount → zero peers, not leader.
        assert!(svc.mesh_enrichment_bounded(MESH_ENRICH_TIMEOUT).is_some());
    }

    /// SHELL-RPC-1 — full request→reply round-trip over a real `Persist`,
    /// exactly as `mde-bus request action/shell/<verb>` drives it: publish
    /// to the action topic, run one poll sweep, assert the reply landed on
    /// `reply/<ulid>`. Proves the responder logic answers every verb (a
    /// no-reply in the field is then a stale-binary / env issue, not code).
    #[test]
    fn poll_once_replies_to_each_verb_end_to_end() {
        use mde_bus::rpc::{publish_request, reply_topic};

        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let svc = ShellService::default();
        let mut cursors: HashMap<String, String> = HashMap::new();

        // A pre-existing request must be answered on the FIRST sweep
        // (cursor starts empty → list_since(None) sees history).
        for verb in ACTION_VERBS {
            let ulid =
                publish_request(&persist, &action_topic(verb), Priority::Default, None, None)
                    .unwrap();
            poll_once(&persist, &svc, &mut cursors);
            let reply = persist.list_since(&reply_topic(&ulid), None).unwrap();
            assert_eq!(reply.len(), 1, "verb {verb} got no reply");
            assert_eq!(
                reply[0].body.as_deref(),
                Some(build_reply(&svc, verb).as_str())
            );
        }

        // A request arriving AFTER the cursor advanced must also be answered
        // (the steady-state path: cursor=Some(last) → list_since sees only new).
        let ulid = publish_request(
            &persist,
            &action_topic("version"),
            Priority::Default,
            None,
            None,
        )
        .unwrap();
        poll_once(&persist, &svc, &mut cursors);
        let reply = persist.list_since(&reply_topic(&ulid), None).unwrap();
        assert_eq!(reply.len(), 1, "steady-state version request got no reply");
        assert_eq!(reply[0].body.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    }
}

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

impl ShellService {
    /// Compiled crate version (`CARGO_PKG_VERSION`).
    #[must_use]
    pub fn build_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// JSON-encoded [`crate::health::HealthReport`] — the same
    /// `HealthReport::empty()` shape the `mackesd healthz` CLI emits,
    /// so panel reads stay in parity. When the CLI's healthz grows a
    /// live-probe path both surfaces inherit it.
    ///
    /// # Errors
    /// Returns the encode error string if the report won't serialize.
    pub fn build_healthz(&self) -> Result<String, String> {
        crate::health::HealthReport::empty()
            .to_json_line()
            .map_err(|e| format!("healthz encode: {e}"))
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
}

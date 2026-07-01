//! NF-Bundle-0 (v2.5) — Nebula status surface.
//!
//! E0.3.1 (EPIC-RETIRE-DBUS, 2026-06-03): the read-projection
//! verbs migrated off the retired `dev.mackes.MDE.Nebula.Status`
//! D-Bus interface onto the mesh **Bus** action/reply pattern
//! (`action/nebula/<verb>`), mirroring the session lifecycle
//! migration (`crates/shell/mde-session/src/session.rs`) and the
//! cert-authority Bus responder (`workers::cert_authority`). The
//! Bus responder ([`serve_bus`] / [`poll_once`]) is spawned from
//! `mackesd` `run_serve` on its own OS thread (rusqlite isn't
//! `Send`, so it runs a current-thread runtime off the main
//! multi-thread executor — same constraint as `mde-session`).
//!
//! The desktop-surface consumers (applets / workbench panels /
//! mde-files / wizard) publish a request + await the reply:
//!
//!   * `action/nebula/status` → JSON [`StatusSnapshot`] covering
//!     active transport, peer-cert epoch, lighthouse role, peer
//!     count, mesh-id.
//!   * `action/nebula/list-peers` → JSON `Vec<PeerRow>` of paired
//!     peers + per-peer overlay IP + cert fingerprint + reachable.
//!   * `action/nebula/self-node` → JSON [`SelfNodeSnapshot`]
//!     {overlay_ip, role, cert_epoch, cert_expires_at, mesh_id}.
//!   * `action/nebula/regen-certs` → JSON { ok, message } — the CA
//!     epoch-bump WRITE (E0.3.1.b); runs [`NebulaStatusService::
//!     regen_certs_inner`], which shells `nebula-cert`.
//!
//! The pure async builders ([`NebulaStatusService::
//! build_status_snapshot`] etc.) are unchanged — the responder
//! reuses them verbatim, then `serde_json`-encodes the result
//! into the reply body, matching the exact wire shape the
//! pre-migration D-Bus methods produced (so consumers parse the
//! same JSON).
//!
//! The `dev.mackes.MDE.Nebula.Status` D-Bus interface is now FULLY
//! RETIRED (E0.3.1.b): reads + the `RegenCerts` write serve on
//! `action/nebula/*`; the three signals (`PeerStateChanged` /
//! `TransportChanged` / `EnrollmentCompleted`) fan out as
//! fire-and-forget rows on the [`NEBULA_EVENT_TOPIC`] Bus event
//! topic via [`spawn_signal_dispatcher`] (a dedicated thread +
//! `Persist`, since rusqlite isn't `Send`); the `Enroll` D-Bus
//! method was removed as dead (the `mackesd enroll` CLI +
//! CSR-watcher path drive enrollment). No `#[interface]` block and
//! no object-server registration remain, so nebula.rs drops out of
//! the lint-dbus-shape allowlist. Workers still emit via the
//! unchanged [`NebulaSignal`] enum + mpsc; only the dispatcher's
//! tail (D-Bus signal → Bus event row) changed.
//!
//! Reads come from the live SQLite tables (`nebula_ca` +
//! `nebula_peer_certs` from migration 0011, `nodes` from the
//! existing reconcile worker) + the on-disk role.host marker
//! file the NF-3.4 supervisor maintains. No new schema; this
//! is a pure read-projection surface.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::sync::Arc;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

/// Poll cadence for the `action/nebula/<verb>` topics. Control
/// surface (not on a human's interactive path), so 400 ms keeps
/// index-read churn low while staying well under the 30 s RPC
/// timeout — matches `rpc::CONTROL_POLL_INTERVAL` +
/// `workers::cert_authority::DEFAULT_POLL_INTERVAL`.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// The read-projection verbs served on `action/nebula/<verb>`.
/// Locked to the `action/<domain>/<verb>` Q96 convention.
pub const ACTION_VERBS: [&str; 5] = [
    "status",
    "self-node",
    "list-peers",
    "regen-certs",
    "published-services",
];

/// Bus event topic the signal dispatcher publishes to (E0.3.1.b).
/// The retired `dev.mackes.MDE.Nebula.Status` D-Bus interface +
/// its object-path/bus-name consts are gone — reads + the
/// RegenCerts write serve on `action/nebula/*`, and the three
/// signals fan out here as fire-and-forget event rows. Consumers
/// (GUI overview surfaces) `list_since` this topic from a per-reader
/// cursor. The literal is the Bus contract event readers key on;
/// the `event_topic_locks` test pins it.
pub const NEBULA_EVENT_TOPIC: &str = "event/nebula/signals";

/// JSON wire shape for the Status() reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusSnapshot {
    /// True when this peer is acting as a lighthouse.
    pub is_lighthouse: bool,
    /// Active CA epoch the local node's cert was signed under.
    /// 0 when no CA exists yet.
    pub ca_epoch: i64,
    /// Number of paired peers (excluding self) the local
    /// nodes table knows about.
    pub peer_count: usize,
    /// Mesh-id this peer belongs to. Empty when no CA exists.
    pub mesh_id: String,
    /// Last known active transport name (one of
    /// "nebula_direct" / "nebula_lighthouse_relay" /
    /// "nebula_https443" / "kdc_tls" / "offline"). Stays
    /// `"offline"` until any worker writes a value.
    pub active_transport: String,
}

/// JSON wire shape for one row of the ListPeers() reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerRow {
    /// Stable node-id.
    pub node_id: String,
    /// Display name (host's hostname at enrollment time).
    pub name: String,
    /// Overlay IP allocated to this peer (e.g. "10.42.0.5").
    /// Empty when no peer cert exists yet.
    pub overlay_ip: String,
    /// First 8 chars of the peer's cert fingerprint.
    /// Empty when no cert exists.
    pub fingerprint: String,
    /// Cert epoch.
    pub cert_epoch: i64,
    /// Unix-epoch seconds when the cert expires.
    pub cert_expires_at: i64,
    /// "online" / "idle" / "offline" — sourced from the
    /// nodes table's health column.
    pub reachable: String,
}

/// JSON wire shape for SelfNode().
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfNodeSnapshot {
    /// Stable node-id of the local peer.
    pub node_id: String,
    /// Hostname.
    pub host: String,
    /// "host" | "peer" — derived from the role.host marker
    /// file at /var/lib/mackesd/nebula/role.host.
    pub role: String,
    /// Active CA epoch.
    pub cert_epoch: i64,
    /// Unix-epoch seconds when the local peer's cert expires.
    pub cert_expires_at: i64,
    /// Overlay IP.
    pub overlay_ip: String,
    /// Mesh-id.
    pub mesh_id: String,
}

/// Default location of the role.host marker the supervisor
/// writes when this peer wins the leader-election lease.
pub const DEFAULT_ROLE_HOST_MARKER: &str = "/var/lib/mackesd/nebula/role.host";

/// Cross-thread events workers hand to the signal dispatcher so
/// the matching `dev.mackes.MDE.Nebula.Status.*` D-Bus signals
/// fan out to every subscribed consumer (Workbench Overview,
/// applets, mde-files). The daemon's worker→IPC plumbing follows
/// the same signal-enum pattern as the meshfs worker (MESHFS-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NebulaSignal {
    /// A peer's reachability flipped. Fired by the
    /// `health_reconciler` worker when the SQLite `nodes.health`
    /// row changes (e.g. unknown→healthy on first heartbeat,
    /// healthy→degraded after one missed cycle).
    PeerStateChanged {
        /// Stable node id whose health column flipped.
        node_id: String,
        /// New reachable string, matching the `PeerRow.reachable`
        /// mapping ("online" / "idle" / "offline").
        reachable: String,
    },
    /// The mesh's active transport rotated. Fired by
    /// `mesh_router` when its scorer picks a different primary
    /// transport. OV-7.b emission lands when KDC2-1.9 wires
    /// `detect_switch` into `tick_once`; today only the
    /// dispatcher infrastructure exists and the signal helper is
    /// callable by any future emitter.
    TransportChanged {
        /// New active-transport name (`nebula_direct`,
        /// `nebula_https443`, `kdc_tls`, etc.).
        active_transport: String,
    },
    /// A peer finished enrollment into the mesh. Fired from
    /// `Enroll()` on the local peer's enrollment success AND
    /// from `nebula_csr_watcher` on the leader's successful
    /// `sign_pending_csr` (the remote-peer path).
    EnrollmentCompleted {
        /// Stable node id of the peer that just enrolled.
        node_id: String,
    },
}

/// Best-effort cross-thread sender handed to workers once IPC
/// registration completes. Cloning is cheap (UnboundedSender is
/// an Arc internally). `emit` is fire-and-forget — a full /
/// closed channel drops the event silently. The worker's own
/// tracing log already carries the event payload so forensics
/// don't depend on the signal landing.
#[derive(Debug, Clone)]
pub struct NebulaSignalSender {
    tx: tokio::sync::mpsc::UnboundedSender<NebulaSignal>,
}

impl NebulaSignalSender {
    /// Emit a signal. Returns immediately.
    pub fn emit(&self, signal: NebulaSignal) {
        let _ = self.tx.send(signal);
    }
}

/// Shared slot workers hold so the signal sender can be wired
/// AFTER the worker has already spawned. The dispatcher
/// `spawn_signal_dispatcher` fills the slot once IPC registration
/// completes; workers spawned earlier in `run_serve()` pick up
/// the sender on their next tick via `slot.get()`. Avoids
/// reordering the entire startup sequence around D-Bus readiness.
pub type SignalSenderSlot = Arc<std::sync::OnceLock<NebulaSignalSender>>;

/// Construct a fresh, empty signal-sender slot. Workers receive
/// a clone of the same `Arc` and read it lock-free per tick.
#[must_use]
pub fn new_signal_sender_slot() -> SignalSenderSlot {
    Arc::new(std::sync::OnceLock::new())
}

/// Service state. Cheap to clone (every field is an Arc /
/// String).
#[derive(Debug, Clone)]
pub struct NebulaStatusService {
    store: Arc<Mutex<rusqlite::Connection>>,
    node_id: String,
    host: String,
    role_marker_path: std::path::PathBuf,
    /// NF-2.5 wire-up (v2.5) — mesh_id passed at
    /// construction so RegenCerts() knows which mesh's CA
    /// to rotate. Defaults to "mesh-<node_id>" when the
    /// supervisor hasn't set the MDE_MESH_ID env var.
    mesh_id: String,
    /// NF-3.6 (v2.5) — QNM-Shared root the Enroll() method
    /// hands to `nebula_enroll::enroll_with_token`. Defaults
    /// to `~/QNM-Shared` (via
    /// `mackesd_core::default_qnm_shared_root`) when the
    /// caller doesn't override.
    workgroup_root: std::path::PathBuf,
    /// CA cert / key / peer-cert dir the RegenCerts() rotation
    /// writes through. Default to the canonical system paths in
    /// production; tests redirect all three under one tempdir via
    /// [`with_ca_dir`](Self::with_ca_dir) so the rotation never
    /// touches `/var/lib/mackesd` (which made the regen-certs tests
    /// non-hermetic — they passed only on a box with no `nebula-cert`
    /// AND a clean CA dir, and failed wherever a real CA already sat).
    ca_crt_path: std::path::PathBuf,
    ca_key_path: std::path::PathBuf,
    peer_cert_dir: std::path::PathBuf,
}

impl NebulaStatusService {
    /// Construct rooted at the live SQLite store + the local
    /// peer's identity.
    #[must_use]
    pub fn new(
        store: Arc<Mutex<rusqlite::Connection>>,
        node_id: impl Into<String>,
        host: impl Into<String>,
    ) -> Self {
        let nid: String = node_id.into();
        let default_mesh = format!("mesh-{nid}");
        Self {
            store,
            node_id: nid,
            host: host.into(),
            role_marker_path: std::path::PathBuf::from(DEFAULT_ROLE_HOST_MARKER),
            mesh_id: std::env::var("MDE_MESH_ID").unwrap_or(default_mesh),
            workgroup_root: crate::default_qnm_shared_root(),
            ca_crt_path: std::path::PathBuf::from(crate::ca::DEFAULT_CA_CERT_PATH),
            ca_key_path: std::path::PathBuf::from(crate::ca::DEFAULT_CA_KEY_PATH),
            peer_cert_dir: std::path::PathBuf::from(crate::ca::epoch::DEFAULT_PEER_CERT_DIR),
        }
    }

    /// Redirect the CA cert/key + peer-cert dir under one directory —
    /// used by tests so the RegenCerts() rotation writes into a tempdir
    /// instead of `/var/lib/mackesd/nebula-ca`. `ca.crt` + `ca.key` land
    /// directly in `dir`; per-peer certs under `dir/peers`.
    #[must_use]
    pub fn with_ca_dir(mut self, dir: &std::path::Path) -> Self {
        self.ca_crt_path = dir.join("ca.crt");
        self.ca_key_path = dir.join("ca.key");
        self.peer_cert_dir = dir.join("peers");
        self
    }

    /// Override the mesh_id — used by tests that need a
    /// deterministic value.
    #[must_use]
    pub fn with_mesh_id(mut self, mesh_id: impl Into<String>) -> Self {
        self.mesh_id = mesh_id.into();
        self
    }

    /// Override the marker path — used by tests that can't
    /// touch /var.
    #[must_use]
    pub fn with_role_marker(mut self, path: std::path::PathBuf) -> Self {
        self.role_marker_path = path;
        self
    }

    /// Override the QNM-Shared root — used by Enroll() to find
    /// the per-peer pending-enroll + bundle paths. Tests
    /// redirect into a tempdir.
    #[must_use]
    pub fn with_workgroup_root(mut self, path: std::path::PathBuf) -> Self {
        self.workgroup_root = path;
        self
    }

    /// Pure helper — builds a [`StatusSnapshot`] from the
    /// live SQLite state. Pulled out for direct testing
    /// without spinning up zbus.
    pub async fn build_status_snapshot(&self) -> Result<StatusSnapshot, String> {
        let conn = self.store.lock().await;
        let is_lighthouse = self.role_marker_path.exists();
        let (ca_epoch, mesh_id) = current_ca_row(&conn).unwrap_or_default();
        let peer_count = count_peers_excluding(&conn, &self.node_id);
        Ok(StatusSnapshot {
            is_lighthouse,
            ca_epoch,
            peer_count,
            mesh_id,
            active_transport: "offline".to_string(),
        })
    }

    /// Pure helper — builds the [`PeerRow`] list from the
    /// live SQLite state.
    pub async fn build_peer_list(&self) -> Result<Vec<PeerRow>, String> {
        // SUBAUDIT-A2 — read the replicated directory (the real roster), not the
        // empty sqlite `nodes` table, so the mde-files mesh overview + any
        // ListPeers() consumer sees the live mesh (was always 0 peers). Cert
        // details still come from the local cert store per peer.
        let conn = self.store.lock().await;
        let dir = crate::ipc::directory::DirectoryService::new(&self.workgroup_root, None);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0u64, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let built = dir.build_directory(now);
        let rows = built["peers"].as_array().cloned().unwrap_or_default();
        let mut out = Vec::new();
        for p in &rows {
            let name = p["hostname"].as_str().unwrap_or("").to_string();
            // Exclude self (it surfaces via self-node).
            if name == self.host {
                continue;
            }
            let node_id = p["node_id"].as_str().unwrap_or(&name).to_string();
            // Prefer the directory's overlay_ip; fall back to the local cert.
            let (cert_ip, fingerprint, cert_epoch, cert_expires_at) =
                peer_cert_for(&conn, &node_id).unwrap_or_default();
            let overlay_ip = p["overlay_ip"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map_or(cert_ip, str::to_string);
            out.push(PeerRow {
                node_id,
                name,
                overlay_ip,
                fingerprint,
                cert_epoch,
                cert_expires_at,
                reachable: match p["health"].as_str().unwrap_or("") {
                    "healthy" => "online".to_string(),
                    "degraded" => "idle".to_string(),
                    _ => "offline".to_string(),
                },
            });
        }
        Ok(out)
    }

    /// Pure async core of the D-Bus `Enroll(token)` method —
    /// testable without a SignalEmitter. The public surface
    /// (`enroll`) wraps this and fires `EnrollmentCompleted`.
    pub async fn enroll_inner(&self, token: String) -> zbus::fdo::Result<String> {
        let workgroup_root = self.workgroup_root.clone();
        let node_id = self.node_id.clone();
        let display_name = self.host.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            crate::nebula_enroll::enroll_with_token(
                &workgroup_root,
                &node_id,
                &display_name,
                &token,
            )
        })
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("enroll task: {e}")))?;
        match outcome {
            Ok(o) => Ok(format!(
                "enrolled into mesh '{}' as {} (overlay {}) after {} s.",
                o.mesh_id,
                self.node_id,
                o.overlay_ip,
                o.waited.as_secs(),
            )),
            Err(e) => Err(zbus::fdo::Error::Failed(e.to_string())),
        }
    }

    /// Pure helper — builds the [`SelfNodeSnapshot`] from
    /// the live SQLite state + role marker.
    pub async fn build_self_node(&self) -> Result<SelfNodeSnapshot, String> {
        let conn = self.store.lock().await;
        let role = if self.role_marker_path.exists() {
            "host".to_string()
        } else {
            "peer".to_string()
        };
        let (ca_epoch, db_mesh_id) = current_ca_row(&conn).unwrap_or_default();
        let (cert_ip, _fingerprint, cert_epoch, cert_expires_at) =
            peer_cert_for(&conn, &self.node_id).unwrap_or_default();

        // AUDIT-MESH-2 — on a peer the local cert store has no self-CA row and no
        // self-cert (the host issues those), so `overlay_ip` and `mesh_id` come
        // back blank and Mesh Control renders empty. Mirror `build_peer_list`:
        // fall back to the replicated directory's self-record for the overlay IP,
        // and to the daemon's configured `mesh_id` for the mesh name.
        let overlay_ip = if cert_ip.is_empty() {
            let dir = crate::ipc::directory::DirectoryService::new(&self.workgroup_root, None);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0u64, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            let built = dir.build_directory(now);
            built["peers"]
                .as_array()
                .and_then(|peers| {
                    peers.iter().find_map(|p| {
                        (p["hostname"].as_str() == Some(self.host.as_str()))
                            .then(|| p["overlay_ip"].as_str().unwrap_or("").to_string())
                            .filter(|s| !s.is_empty())
                    })
                })
                .unwrap_or(cert_ip)
        } else {
            cert_ip
        };
        let mesh_id = if db_mesh_id.is_empty() {
            self.mesh_id.clone()
        } else {
            db_mesh_id
        };

        Ok(SelfNodeSnapshot {
            node_id: self.node_id.clone(),
            host: self.host.clone(),
            role,
            cert_epoch: cert_epoch.max(ca_epoch),
            cert_expires_at,
            overlay_ip,
            mesh_id,
        })
    }

    /// E0.3.1.b — the CA-epoch bump logic, callable from the Bus
    /// responder (`action/nebula/regen-certs`). Extracted from the
    /// retired `#[interface] regen_certs` D-Bus method verbatim;
    /// returns the human-readable status string on success (incl.
    /// the BinaryMissing "skipped" hint, which is still a success
    /// — nothing was rotated but the operator gets a clear next
    /// step), or an error string the responder wraps in
    /// `{ "ok": false, "message": ... }`.
    ///
    /// WRITE verb: shells `nebula-cert` subprocesses, so it can run
    /// for seconds and briefly blocks the single-threaded responder
    /// from answering reads — acceptable for a rare admin rotation.
    pub async fn regen_certs_inner(&self) -> Result<String, String> {
        use crate::ca::epoch;
        use crate::ca::{CaError, SubprocessBackend};
        let mesh_id = self.mesh_id.clone();
        let mut conn = self.store.lock().await;
        match epoch::bump_epoch_into(
            &SubprocessBackend,
            &mut *conn,
            &mesh_id,
            Some(&self.ca_crt_path),
            Some(&self.ca_key_path),
            &self.peer_cert_dir,
        ) {
            Ok(o) => Ok(format!(
                "CA rotated to epoch {} (retired {}); {} peer certs re-signed.",
                o.new_epoch,
                o.retired_epoch
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                o.re_signed,
            )),
            Err(CaError::BinaryMissing) => Ok("CA rotation skipped: nebula-cert not on PATH. \
                 Install the Fedora `nebula` package + retry."
                .to_string()),
            Err(e) => Err(format!("rotation: {e}")),
        }
    }
}

// ----- signal dispatcher (E0.3.1.b) ----------------------------------
//
// The `dev.mackes.MDE.Nebula.Status` D-Bus interface is fully
// retired: no `#[interface]` block, no object-server registration,
// no register helpers. Reads + the `RegenCerts` write serve on
// `action/nebula/*` (above); the three signals fan out on the
// `NEBULA_EVENT_TOPIC` Bus event topic via the dispatcher below.
// Workers still emit `NebulaSignal`s through the unchanged mpsc;
// only the dispatcher's tail changed (D-Bus signal → Bus row).

/// Serialize a [`NebulaSignal`] to the JSON event body the
/// dispatcher writes to [`NEBULA_EVENT_TOPIC`]. The `kind` tag lets
/// one topic carry all three variants; downstream decoders key on
/// the `kind` strings, so the round-trip test below locks them.
#[must_use]
pub fn signal_event_body(signal: &NebulaSignal) -> String {
    match signal {
        NebulaSignal::PeerStateChanged { node_id, reachable } => {
            json!({ "kind": "peer-state-changed", "node_id": node_id, "reachable": reachable })
                .to_string()
        }
        NebulaSignal::TransportChanged { active_transport } => {
            json!({ "kind": "transport-changed", "active_transport": active_transport }).to_string()
        }
        NebulaSignal::EnrollmentCompleted { node_id } => {
            json!({ "kind": "enrollment-completed", "node_id": node_id }).to_string()
        }
    }
}

/// Spawn the Nebula signal-dispatch loop. Drains [`NebulaSignal`]
/// events from the worker-facing mpsc and writes each as a row on
/// the [`NEBULA_EVENT_TOPIC`] Bus event topic, replacing the old
/// `dev.mackes.MDE.Nebula.Status.*` D-Bus signal emission
/// (E0.3.1.b). Subscribers (Workbench Overview) `list_since` the
/// topic from their own cursor.
///
/// Runs on a dedicated OS thread with its own current-thread
/// runtime holding one `Persist` — rusqlite isn't `Send`, so it
/// can't ride the main multi-thread executor (same constraint +
/// structure as [`serve_bus`]). Fills `slot` with the sender so
/// workers spawned earlier in `run_serve()` pick it up on their
/// next tick; also returns it (the slot's clone keeps the channel
/// open for the daemon's lifetime, so callers may drop the return).
pub fn spawn_signal_dispatcher(slot: &SignalSenderSlot) -> NebulaSignalSender {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<NebulaSignal>();
    if let Err(e) = std::thread::Builder::new()
        .name("nebula-signal-dispatcher".into())
        .spawn(move || {
            let Some(bus_dir) = mde_bus::default_data_dir() else {
                tracing::warn!("nebula dispatcher: no Bus data dir; signals unavailable");
                return;
            };
            let persist = match Persist::open(bus_dir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("nebula dispatcher: opening Bus store: {e}");
                    return;
                }
            };
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("nebula dispatcher: runtime build failed: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                while let Some(signal) = rx.recv().await {
                    let body = signal_event_body(&signal);
                    if let Err(e) =
                        persist.write(NEBULA_EVENT_TOPIC, Priority::Default, None, Some(&body))
                    {
                        tracing::warn!(error = %e, "nebula dispatcher: event write failed");
                    }
                }
            });
        })
    {
        tracing::error!("nebula dispatcher: thread spawn failed: {e}");
    }
    let sender = NebulaSignalSender { tx };
    // Fill the shared slot for workers that spawned before this call.
    // `set` returns Err if already filled — a programmer error
    // (called twice), surfaced via tracing rather than overwriting.
    if slot.set(sender.clone()).is_err() {
        tracing::warn!(
            "nebula signal-sender slot already filled; \
             ignoring duplicate spawn_signal_dispatcher call",
        );
    }
    sender
}

// ----- Bus responder (E0.3.1) ----------------------------------------

/// Action topic for verb `verb`: `action/nebula/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/nebula/{verb}")
}

/// Build the reply body for one `action/nebula/<verb>` request.
///
/// Mirrors the exact JSON the retired D-Bus methods produced:
/// `status` → [`StatusSnapshot`], `self-node` → [`SelfNodeSnapshot`],
/// `list-peers` → `Vec<PeerRow>`. On a builder error or an unknown
/// verb the body is `{"error": "..."}` so the caller can surface a
/// diagnostic rather than time out (the `serde_json::from_str` on
/// the consumer side simply fails to decode the error envelope and
/// falls back to its empty/default rendering, exactly as it did
/// when the daemon was unreachable).
///
/// Split out from [`poll_once`] so a unit test can drive it
/// without a `Persist` round-trip.
pub async fn build_reply(svc: &NebulaStatusService, verb: &str, body: Option<&str>) -> String {
    match verb {
        "status" => match svc.build_status_snapshot().await {
            Ok(snap) => serde_json::to_string(&snap)
                .unwrap_or_else(|e| json!({ "error": format!("encode: {e}") }).to_string()),
            Err(e) => json!({ "error": e }).to_string(),
        },
        "self-node" => match svc.build_self_node().await {
            Ok(s) => serde_json::to_string(&s)
                .unwrap_or_else(|e| json!({ "error": format!("encode: {e}") }).to_string()),
            Err(e) => json!({ "error": e }).to_string(),
        },
        "list-peers" => match svc.build_peer_list().await {
            Ok(peers) => serde_json::to_string(&peers)
                .unwrap_or_else(|e| json!({ "error": format!("encode: {e}") }).to_string()),
            Err(e) => json!({ "error": e }).to_string(),
        },
        // WRITE verb (E0.3.1.b): rotates the CA epoch as a side effect.
        // Replies `{ "ok": bool, "message": str }` so the caller
        // (`mesh_control::run_rotate_ca`) gets an unambiguous
        // success flag + a human string for its `last_op` banner.
        "regen-certs" => {
            // SEC-2 — the rotation gate guards the Bus door too: the
            // request body must carry the operator passphrase.
            let phrase = body
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| {
                    v.get("passphrase")
                        .and_then(|p| p.as_str().map(str::to_string))
                })
                .unwrap_or_default();
            let check = crate::ca::rotation_gate::verify(&svc.workgroup_root, &phrase);
            if let Some(msg) = crate::ca::rotation_gate::refusal_message(check) {
                return json!({ "ok": false, "message": msg }).to_string();
            }
            let (ok, message) = match svc.regen_certs_inner().await {
                Ok(msg) => (true, msg),
                Err(e) => (false, e),
            };
            json!({ "ok": ok, "message": message }).to_string()
        }
        // RETIRE-PY.7 — the Service-Publishing panel's summary (was a
        // `python3 -c mackes.mesh_nebula` shell-out). Pure read: the 7 canonical
        // services × this peer's overlay IP.
        "published-services" => build_published_services(),
        other => json!({ "error": format!("unknown nebula verb: {other}") }).to_string(),
    }
}

/// The canonical Nebula-published services: `(id, display, default-port, proto)`.
/// Mirrors the v1.x `mackes.mesh_nebula.CANONICAL_SERVICES` tuple.
const CANONICAL_SERVICES: [(&str, &str, u16, &str); 7] = [
    ("ssh", "SSH", 22, "tcp"),
    ("nats", "NATS broker", 4222, "tcp"),
    ("fs", "Mesh FS (SSHFS)", 22, "tcp"),
    ("media", "Media library", 8080, "tcp"),
    ("sync", "rsync", 873, "tcp"),
    ("wol", "Wake-on-LAN relay", 9, "udp"),
    ("av", "Audio/video transport", 5004, "udp"),
];

/// Build the published-services summary JSON (one row per canonical service ×
/// the current overlay IP; `is_publishable` = an overlay IP exists). Replaces
/// the python `published_services_summary()` — same JSON list-of-rows shape the
/// workbench `service_publishing` panel's `parse_summary` already expects.
fn build_published_services() -> String {
    let overlay = crate::voip_rtt::own_nebula_ip();
    let rows: Vec<serde_json::Value> = CANONICAL_SERVICES
        .iter()
        .map(|(id, name, port, proto)| {
            json!({
                "id": id,
                "name": name,
                "port": port,
                "proto": proto,
                "overlay_ip": overlay,
                "is_publishable": overlay.is_some(),
            })
        })
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
}

/// Run the Bus responder loop on the current thread, building a
/// local current-thread tokio runtime for the async builders
/// (`Persist`/rusqlite isn't `Send`, so this runs off the main
/// multi-thread executor — same constraint + structure as
/// `mde-session`'s `serve_bus`). Loops until `should_stop()`.
///
/// `mackesd` `run_serve` spawns this on a dedicated `std::thread`
/// so the responder is runtime-reachable at boot.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &NebulaStatusService, should_stop: F) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("nebula responder: runtime build failed: {e}");
            return;
        }
    };
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &rt, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out so a test
/// can drive it without the sleep loop). For each new request on
/// `action/nebula/<verb>`, runs the matching builder on `rt` and
/// writes the JSON reply to `reply/<ulid>`.
pub fn poll_once(
    persist: &Persist,
    svc: &NebulaStatusService,
    rt: &tokio::runtime::Runtime,
    cursors: &mut HashMap<String, String>,
) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "nebula responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = rt.block_on(build_reply(svc, verb, msg.body.as_deref()));
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "nebula responder: reply write failed");
            }
        }
    }
}

// ----- private SQL helpers -------------------------------------------

fn current_ca_row(conn: &rusqlite::Connection) -> Option<(i64, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT epoch, mesh_id FROM nebula_ca \
             WHERE retired_at IS NULL \
             ORDER BY epoch DESC LIMIT 1",
        )
        .ok()?;
    stmt.query_row([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .ok()
}

fn count_peers_excluding(conn: &rusqlite::Connection, local: &str) -> usize {
    let mut stmt = match conn.prepare("SELECT COUNT(*) FROM nodes WHERE node_id != ?1") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    stmt.query_row([local], |r| r.get::<_, i64>(0))
        .map(|n| n as usize)
        .unwrap_or(0)
}

fn peer_cert_for(conn: &rusqlite::Connection, node_id: &str) -> Option<(String, String, i64, i64)> {
    let mut stmt = conn
        .prepare(
            "SELECT overlay_ip, cert_pem, epoch, expires_at \
             FROM nebula_peer_certs \
             WHERE node_id = ?1 AND revoked_at IS NULL \
             ORDER BY epoch DESC LIMIT 1",
        )
        .ok()?;
    stmt.query_row([node_id], |r| {
        let cert_pem: String = r.get(1)?;
        Ok((
            r.get::<_, String>(0)?,
            fingerprint(&cert_pem),
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })
    .ok()
}

/// Pure helper — derive an 8-char "fingerprint" from a PEM
/// blob. Today we use the first 8 alphanumeric chars of the
/// base64 body so the value is stable + readable in the UI;
/// when `nebula-cert print` ships a real fingerprint in
/// JSON, swap this for the real call.
#[must_use]
pub fn fingerprint(cert_pem: &str) -> String {
    let body: String = cert_pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(|l| l.chars())
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_store() -> Arc<Mutex<rusqlite::Connection>> {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn event_topic_locks() {
        // E0.3.1.b — the signal fan-out topic; every event consumer
        // reads this literal, so it must never drift.
        assert_eq!(NEBULA_EVENT_TOPIC, "event/nebula/signals");
    }

    #[test]
    fn signal_event_body_round_trips_each_variant() {
        // The dispatcher serializes each NebulaSignal to this JSON;
        // downstream event consumers parse it by the `kind` tag.
        let peer = signal_event_body(&NebulaSignal::PeerStateChanged {
            node_id: "peer:pine".into(),
            reachable: "online".into(),
        });
        assert!(peer.contains("\"kind\":\"peer-state-changed\""));
        assert!(peer.contains("peer:pine"));
        let transport = signal_event_body(&NebulaSignal::TransportChanged {
            active_transport: "nebula_direct".into(),
        });
        assert!(transport.contains("\"kind\":\"transport-changed\""));
        assert!(transport.contains("nebula_direct"));
        let enroll = signal_event_body(&NebulaSignal::EnrollmentCompleted {
            node_id: "peer:birch".into(),
        });
        assert!(enroll.contains("\"kind\":\"enrollment-completed\""));
        assert!(enroll.contains("peer:birch"));
    }

    #[test]
    fn fingerprint_extracts_first_8_alnum() {
        let pem = "-----BEGIN CERT-----\n\
                   abcd-EFGH+1234ZZZZ\n\
                   -----END CERT-----\n";
        // 'abcdEFGH' = first 8 alphanumeric after stripping
        // delimiters + non-alnum chars.
        assert_eq!(fingerprint(pem), "abcdEFGH");
    }

    #[test]
    fn fingerprint_handles_empty_pem() {
        assert_eq!(fingerprint(""), "");
        assert_eq!(fingerprint("-----BEGIN-----\n-----END-----\n"), "");
    }

    #[tokio::test]
    async fn status_on_empty_store_reports_offline_zero_peers() {
        let svc = NebulaStatusService::new(fresh_store(), "peer:local", "host")
            .with_role_marker("/nonexistent/marker".into());
        let s = svc.build_status_snapshot().await.expect("status");
        assert!(!s.is_lighthouse);
        assert_eq!(s.ca_epoch, 0);
        assert_eq!(s.peer_count, 0);
        assert_eq!(s.mesh_id, "");
        assert_eq!(s.active_transport, "offline");
    }

    #[tokio::test]
    async fn status_reports_is_lighthouse_when_marker_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("role.host");
        std::fs::write(&marker, "role:host\n").expect("write");
        let svc =
            NebulaStatusService::new(fresh_store(), "peer:local", "host").with_role_marker(marker);
        let s = svc.build_status_snapshot().await.expect("status");
        assert!(s.is_lighthouse);
    }

    #[tokio::test]
    async fn status_reports_ca_epoch_and_mesh_after_mint() {
        let store = fresh_store();
        {
            let conn = store.lock().await;
            conn.execute(
                "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, retired_at) \
                 VALUES ('m1', 0, 'pem', NULL)",
                [],
            )
            .expect("insert ca");
        }
        let svc = NebulaStatusService::new(store, "peer:local", "host")
            .with_role_marker("/nonexistent/marker".into());
        let s = svc.build_status_snapshot().await.expect("status");
        assert_eq!(s.ca_epoch, 0);
        assert_eq!(s.mesh_id, "m1");
    }

    #[tokio::test]
    async fn list_peers_excludes_local_and_emits_overlay_ip() {
        let store = fresh_store();
        // SUBAUDIT-A2 — build_peer_list reads the replicated directory now, not
        // the sqlite nodes table. Seed two peer records; self ("host") is
        // excluded, anvil surfaces with its overlay IP.
        let tmp = tempfile::tempdir().expect("tempdir");
        let peers_dir = mackes_mesh_types::peers::peers_dir(tmp.path());
        std::fs::create_dir_all(&peers_dir).unwrap();
        for (host, ip) in [("host", "10.42.0.1"), ("anvil", "10.42.0.5")] {
            let mut rec =
                mackes_mesh_types::peers::PeerRecord::now(host, Some("v10".into()), "healthy");
            rec.overlay_ip = Some(ip.to_string());
            std::fs::write(
                peers_dir.join(format!("{host}.json")),
                serde_json::to_string(&rec).unwrap(),
            )
            .unwrap();
        }
        let svc = NebulaStatusService::new(store, "peer:local", "host")
            .with_workgroup_root(tmp.path().to_path_buf())
            .with_role_marker("/nonexistent/marker".into());
        let peers = svc.build_peer_list().await.expect("peers");
        assert_eq!(peers.len(), 1, "self excluded, anvil present");
        assert_eq!(peers[0].name, "anvil");
        assert_eq!(peers[0].overlay_ip, "10.42.0.5");
        assert_eq!(peers[0].reachable, "online");
    }

    #[tokio::test]
    async fn self_node_role_flips_with_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("role.host");
        let store = fresh_store();
        let svc = NebulaStatusService::new(Arc::clone(&store), "peer:local", "host")
            .with_role_marker(marker.clone());
        let s = svc.build_self_node().await.expect("self");
        assert_eq!(s.role, "peer");
        std::fs::write(&marker, "role:host\n").expect("write");
        let s2 = svc.build_self_node().await.expect("self after promote");
        assert_eq!(s2.role, "host");
    }

    #[tokio::test]
    async fn self_node_falls_back_to_directory_and_mesh_id_on_a_peer() {
        // AUDIT-MESH-2 — on a peer the local cert store has no self-cert and no
        // CA row, so overlay_ip + mesh_id used to come back blank (Mesh Control
        // empty). They must now fall back to the replicated directory self-record
        // (overlay_ip) and the daemon's configured mesh_id.
        let tmp = tempfile::tempdir().expect("tempdir");
        let peers_dir = mackes_mesh_types::peers::peers_dir(tmp.path());
        std::fs::create_dir_all(&peers_dir).unwrap();
        let mut rec =
            mackes_mesh_types::peers::PeerRecord::now("birch", Some("v10".into()), "healthy");
        rec.overlay_ip = Some("10.42.0.9".to_string());
        std::fs::write(
            peers_dir.join("birch.json"),
            serde_json::to_string(&rec).unwrap(),
        )
        .unwrap();
        let svc = NebulaStatusService::new(fresh_store(), "peer:birch", "birch")
            .with_workgroup_root(tmp.path().to_path_buf())
            .with_mesh_id("mesh-prod")
            .with_role_marker("/nonexistent/marker".into());
        let s = svc.build_self_node().await.expect("self");
        assert_eq!(s.role, "peer");
        assert_eq!(
            s.overlay_ip, "10.42.0.9",
            "overlay_ip falls back to the directory self-record"
        );
        assert_eq!(
            s.mesh_id, "mesh-prod",
            "mesh_id falls back to the configured daemon mesh_id"
        );
    }

    #[tokio::test]
    async fn regen_certs_is_passphrase_gated_sec2() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let svc = NebulaStatusService::new(fresh_store(), "peer:local", "host")
            .with_workgroup_root(tmp.path().to_path_buf())
            .with_ca_dir(tmp.path());
        // Unset gate → fail-closed refusal naming set-passphrase.
        let r: serde_json::Value =
            serde_json::from_str(&build_reply(&svc, "regen-certs", None).await).unwrap();
        assert_eq!(r["ok"], false);
        assert!(r["message"].as_str().unwrap().contains("set-passphrase"));
        // Armed gate + wrong phrase → refusal.
        crate::ca::rotation_gate::set_passphrase(tmp.path(), "correct horse").unwrap();
        let r: serde_json::Value = serde_json::from_str(
            &build_reply(&svc, "regen-certs", Some(r#"{"passphrase":"nope"}"#)).await,
        )
        .unwrap();
        assert_eq!(r["ok"], false);
        assert!(r["message"].as_str().unwrap().contains("wrong passphrase"));
        // Right phrase → the gate opens (rotation itself then reports
        // its own outcome — binary-missing hint on dev boxes).
        let r: serde_json::Value = serde_json::from_str(
            &build_reply(
                &svc,
                "regen-certs",
                Some(r#"{"passphrase":"correct horse"}"#),
            )
            .await,
        )
        .unwrap();
        let msg = r["message"].as_str().unwrap();
        assert!(
            msg.contains("nebula-cert not on PATH") || msg.contains("CA rotated"),
            "gate must open on the right phrase: {msg}"
        );
    }

    #[tokio::test]
    async fn regen_certs_handles_binary_missing_gracefully() {
        // Hermetic: redirect the CA dir into a tempdir so the rotation
        // never touches /var/lib/mackesd. Outcome depends only on whether
        // `nebula-cert` is on PATH — installed → "CA rotated to epoch 0"
        // (into the empty tempdir, no leftover to refuse); absent → the
        // "not on PATH" hint. Both are valid; neither depends on host CA
        // state, so this no longer flakes on a box with a real CA present.
        let tmp = tempfile::tempdir().expect("tempdir");
        let svc =
            NebulaStatusService::new(fresh_store(), "peer:local", "host").with_ca_dir(tmp.path());
        let msg = svc.regen_certs_inner().await.expect("ok");
        assert!(
            msg.contains("nebula-cert not on PATH") || msg.contains("CA rotated to epoch"),
            "unexpected regen-certs reply: {msg}",
        );
    }

    // ---- NF-3.6 Enroll D-Bus method ----------------------

    #[tokio::test]
    async fn enroll_rejects_invalid_token_with_actionable_error() {
        // Garbage tokens fall through to EnrollError::InvalidToken
        // which we surface as zbus::fdo::Error::Failed.
        let tmp = tempfile::tempdir().expect("tempdir");
        let svc = NebulaStatusService::new(fresh_store(), "peer:local", "anvil")
            .with_workgroup_root(tmp.path().to_path_buf());
        let err = svc
            .enroll_inner("not a valid token".to_string())
            .await
            .expect_err("invalid token");
        let s = err.to_string();
        assert!(s.contains("invalid join token"), "msg: {s}");
        assert!(s.contains("mesh:"), "msg: {s}");
    }

    #[tokio::test]
    async fn enroll_with_valid_token_publishes_csr_then_times_out() {
        // Valid token + a tempdir QNM-Shared root + no lighthouse
        // signing on the other end → publish-CSR succeeds, then
        // wait_for_signed_bundle times out per the default
        // ENROLL_WAIT_TIMEOUT.
        //
        // Skip the actual 30 s wait — this test would block CI.
        // Just confirm the CSR file lands by triggering enroll
        // and then aborting via a short-lived spawn (we don't
        // await it). Real timeout is covered in nebula_enroll
        // tests.
        //
        // We just check the synchronous "what would happen" by
        // calling the underlying publish path directly — Enroll's
        // wrapper is thin.
        use crate::enrollment::build_identity;
        use crate::nebula_enroll::{
            build_pending, parse_join_token, pending_enroll_path, publish_enrollment_request,
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let identity = build_identity();
        let token = parse_join_token("mesh:test@10.0.0.5:4242#bearer").unwrap();
        let pending = build_pending(&identity, "peer:local", "anvil", token);
        let p = publish_enrollment_request(tmp.path(), "peer:local", &pending).expect("publish");
        assert_eq!(p, pending_enroll_path(tmp.path(), "peer:local"));
        assert!(p.exists());
    }

    #[tokio::test]
    async fn with_workgroup_root_overrides_default() {
        let custom = std::path::PathBuf::from("/tmp/custom-qnm-test");
        let svc = NebulaStatusService::new(fresh_store(), "peer:local", "anvil")
            .with_workgroup_root(custom.clone());
        assert_eq!(svc.workgroup_root, custom);
    }

    // ---- E0.3.1 Bus responder (action/reply) -----------------

    #[test]
    fn action_topic_is_canonical_three_segments() {
        // rpc::publish_request rejects any topic outside the
        // `action/` namespace; lock the `action/nebula/<verb>`
        // three-segment shape so a future rename doesn't drift
        // off the convention the consumers publish to.
        for verb in ACTION_VERBS {
            let topic = action_topic(verb);
            assert!(
                topic.starts_with("action/"),
                "topic {topic:?} must be in action/ namespace"
            );
            let parts: Vec<&str> = topic.split('/').collect();
            assert_eq!(
                parts.len(),
                3,
                "topic {topic:?} must be action/<domain>/<verb>"
            );
            assert_eq!(parts[0], "action");
            assert_eq!(parts[1], "nebula");
            assert_eq!(parts[2], verb);
        }
    }

    #[tokio::test]
    async fn build_reply_unknown_verb_yields_error_envelope() {
        let svc = NebulaStatusService::new(fresh_store(), "peer:local", "host")
            .with_role_marker("/nonexistent/marker".into());
        let body = build_reply(&svc, "frobnicate", None).await;
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert!(v["error"].as_str().unwrap().contains("unknown nebula verb"));
    }

    #[test]
    fn list_peers_round_trips_through_a_temp_persist() {
        // Required E0.3.1 acceptance: a temp Persist + an
        // in-memory NebulaStatusService, publish
        // action/nebula/list-peers, run one responder poll, and
        // assert the reply.body deserializes to Vec<PeerRow>.
        //
        // The responder builds its own current-thread runtime (as
        // it does in production via serve_bus) since the SQLite
        // guard held across the builder's await is !Send; this
        // test therefore must NOT itself be a #[tokio::test].
        use mde_bus::hooks::config::Priority;
        use mde_bus::persist::Persist;
        use mde_bus::rpc::{publish_request, reply_topic};

        let tmp = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist open");

        // In-memory store seeded with one peer (excluding self) so
        // build_peer_list returns a non-empty Vec<PeerRow>.
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        let store = Arc::new(Mutex::new(conn));
        // SUBAUDIT-A2 — seed the replicated directory (build_peer_list's source).
        let tmp = tempfile::tempdir().expect("tempdir");
        let peers_dir = mackes_mesh_types::peers::peers_dir(tmp.path());
        std::fs::create_dir_all(&peers_dir).unwrap();
        for (host, ip) in [("host", "10.42.0.1"), ("anvil", "10.42.0.7")] {
            let mut rec =
                mackes_mesh_types::peers::PeerRecord::now(host, Some("v10".into()), "healthy");
            rec.overlay_ip = Some(ip.to_string());
            std::fs::write(
                peers_dir.join(format!("{host}.json")),
                serde_json::to_string(&rec).unwrap(),
            )
            .unwrap();
        }
        let svc = NebulaStatusService::new(store, "peer:local", "host")
            .with_workgroup_root(tmp.path().to_path_buf())
            .with_role_marker("/nonexistent/marker".into());

        // Consumer side: publish the request to the action topic.
        let ulid = publish_request(
            &persist,
            &action_topic("list-peers"),
            Priority::Default,
            None,
            None,
        )
        .expect("publish request");

        // Responder side: one poll sweep writes the reply.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        let mut cursors: HashMap<String, String> = HashMap::new();
        poll_once(&persist, &svc, &rt, &mut cursors);

        // The reply landed on reply/<ulid> and deserializes to
        // Vec<PeerRow> with the seeded peer.
        let replies = persist
            .list_since(&reply_topic(&ulid), None)
            .expect("list replies");
        assert_eq!(replies.len(), 1, "exactly one reply on reply/<ulid>");
        let body = replies[0].body.as_deref().expect("reply body");
        let peers: Vec<PeerRow> = serde_json::from_str(body).expect("decode Vec<PeerRow>");
        assert_eq!(peers.len(), 1, "self excluded, one peer remains");
        assert_eq!(peers[0].name, "anvil");
        assert_eq!(peers[0].overlay_ip, "10.42.0.7");
        assert_eq!(peers[0].reachable, "online");

        // A second poll with the advanced cursor writes no new
        // reply (the request was already drained).
        poll_once(&persist, &svc, &rt, &mut cursors);
        let replies2 = persist
            .list_since(&reply_topic(&ulid), None)
            .expect("list replies again");
        assert_eq!(replies2.len(), 1, "cursor advanced; no duplicate reply");
    }
}

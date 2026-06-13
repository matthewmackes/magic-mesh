//! NF-3.6.c (v2.5) — Nebula CSR auto-signer worker.
//!
//! Polls `QNM-Shared/*/mackesd/pending-enroll.json` every 30 s.
//! For each CSR without a matching `nebula-bundle.json`, calls
//! [`crate::nebula_enroll::sign_pending_csr`] to mint the cert
//! + write the bundle, replacing the manual `mackesd ca sign-csr`
//! operator step for the common case.
//!
//! Skip conditions (each is honest — no signing happens):
//!
//!   * No active CA for `mesh_id` — sign_pending_csr returns
//!     `SignFailed` which the worker logs at warn level + skips
//!     until the operator runs `mackesd ca mint`.
//!   * Bundle already exists alongside the CSR — assume signed
//!     by an earlier tick OR by a manual `sign-csr` call. The
//!     mtime check covers re-enrollment: when the CSR is newer
//!     than the bundle, re-sign.
//!   * nebula-cert binary missing — `SignFailed` again; the
//!     worker doesn't crash, just logs each tick.
//!
//! Multi-lighthouse note: this worker runs on every lighthouse
//! AND every peer (since the worker pool is uniform across the
//! mesh). Non-lighthouse nodes never have an active CA, so
//! sign_pending_csr returns SignFailed + the worker skips —
//! a no-op on peer-role boxes. Multi-lighthouse signing races
//! resolve via the underlying `nebula_peer_certs` partial unique
//! index on (overlay_ip, epoch) — at most one signer wins.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::{ShutdownToken, Worker};
use crate::ca::bundle::bundle_path;
use crate::ca::NebulaCertBackend;
use crate::ipc::nebula::{NebulaSignal, SignalSenderSlot};
use crate::nebula_enroll::{pending_enroll_path, SignCsrPaths};

/// Default tick cadence. Slower than the heartbeat (5 s) because
/// enrollment is a low-frequency operator event; the operator
/// doesn't typically join a peer every 5 seconds. 30 s gives
/// the lighthouse a tight feedback loop while not hammering the
/// SQLite WAL.
pub const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Worker handle. Cheap to construct; the heavy lifting (sqlite
/// open + nebula-cert subprocess) happens in `run()`.
pub struct NebulaCsrWatcher {
    workgroup_root: PathBuf,
    db_path: PathBuf,
    mesh_id: String,
    paths: SignCsrPaths,
    /// Lighthouse external address baked into each issued bundle's
    /// roster entry. Mirrors the CLI's --lighthouse-addr flag.
    lighthouse_addr: String,
    /// Self node_id, used as the lighthouse entry's node_id.
    local_node_id: String,
    /// Cert lifetime in days for each issued peer cert. 365 by
    /// default.
    /// Override the tick cadence (default [`TICK_INTERVAL`]). Used
    /// by tests to keep the loop short.
    tick: Duration,
    /// Cert backend. Defaults to `SubprocessBackend` (shells out
    /// to nebula-cert). Tests inject `MockBackend` via
    /// [`Self::with_backend`].
    backend: Arc<dyn NebulaCertBackend>,
    /// OV-7.c leader-side emission slot. Filled by the IPC
    /// bootstrap once `spawn_signal_dispatcher` lands. When set,
    /// every successful `sign_pending_csr` fires
    /// `NebulaSignal::EnrollmentCompleted{node_id}` so any
    /// Workbench Overview / applets on the leader's peer re-probe
    /// without waiting for the next reconcile tick. Empty slot =
    /// silent emission (peer-role boxes, pre-IPC startup); the
    /// signing path still runs.
    signal_slot: Option<SignalSenderSlot>,
}

impl NebulaCsrWatcher {
    /// Construct with production defaults: 30 s tick,
    /// SignCsrPaths::production_defaults, 365-day certs,
    /// SubprocessBackend (shells out to nebula-cert).
    #[must_use]
    pub fn new(
        workgroup_root: PathBuf,
        db_path: PathBuf,
        mesh_id: String,
        local_node_id: String,
        lighthouse_addr: String,
    ) -> Self {
        Self {
            workgroup_root,
            db_path,
            mesh_id,
            paths: SignCsrPaths::production_defaults(),
            lighthouse_addr,
            local_node_id,
            tick: TICK_INTERVAL,
            backend: Arc::new(crate::ca::SubprocessBackend),
            signal_slot: None,
        }
    }

    /// OV-7.c (v2.6) — attach the shared signal-sender slot so
    /// successful CSR signings fire `EnrollmentCompleted` for
    /// every Workbench / applet subscriber. Wired in
    /// `run_serve` after `spawn_signal_dispatcher` returns.
    #[must_use]
    pub fn with_signal_slot(mut self, slot: SignalSenderSlot) -> Self {
        self.signal_slot = Some(slot);
        self
    }

    /// Override the SignCsrPaths used per tick. Tests need this
    /// to redirect ca_crt / ca_key / scratch_dir into a tempdir.
    #[must_use]
    pub fn with_paths(mut self, paths: SignCsrPaths) -> Self {
        self.paths = paths;
        self
    }

    /// Override the tick cadence — used by tests to avoid
    /// 30-second wall-clock waits.
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Override the cert lifetime in days.
    #[must_use]
    /// Override the cert backend — used by tests to inject
    /// `MockBackend` so the worker can run without nebula-cert
    /// on PATH.

    pub fn with_backend(mut self, backend: Arc<dyn NebulaCertBackend>) -> Self {
        self.backend = backend;
        self
    }
}

#[async_trait::async_trait]
impl Worker for NebulaCsrWatcher {
    fn name(&self) -> &'static str {
        "nebula-csr-watcher"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.tick) => {
                    self.tick_once();
                }
            }
        }
        Ok(())
    }
}

impl NebulaCsrWatcher {
    /// One scan pass. Pulled out for direct testing without
    /// the tokio scheduler.
    pub fn tick_once(&self) {
        let peers = match discover_pending_peers(&self.workgroup_root) {
            Ok(ps) => ps,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    workgroup_root = %self.workgroup_root.display(),
                    "nebula-csr-watcher: scan failed (QNM-Shared may not be mounted)",
                );
                return;
            }
        };
        if peers.is_empty() {
            return;
        }
        // Open a fresh SQLite handle per tick — cheap (microseconds)
        // for a local file + avoids holding a connection across the
        // 30 s sleep that would block the WAL writer.
        let conn = match crate::store::open(&self.db_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %self.db_path.display(),
                    "nebula-csr-watcher: sqlite open failed; skipping tick",
                );
                return;
            }
        };
        for peer_id in peers {
            if !needs_signing(&self.workgroup_root, &peer_id).unwrap_or(true) {
                continue;
            }
            let lighthouses = vec![crate::ca::bundle::LighthouseEntry {
                node_id: self.local_node_id.clone(),
                // Conventional first-host address — matches the
                // CLI default. Operators on multi-IP setups can
                // override at the CLI for one-off bundles or
                // edit the bundle file directly.
                overlay_ip: "10.42.0.1".to_string(),
                external_addr: self.lighthouse_addr.clone(),
            }];
            // TUNE-11: the watcher NEVER auto-overrides the cap.
            // Operator overrides only flow through the explicit
            // `mackesd ca sign-csr --override-cap` CLI path.
            match crate::nebula_enroll::sign_pending_csr(
                &*self.backend,
                &conn,
                &self.workgroup_root,
                &peer_id,
                &self.mesh_id,
                &self.paths,
                lighthouses,
                false,
            ) {
                Ok(outcome) => {
                    tracing::info!(
                        peer_id = %peer_id,
                        mesh_id = %self.mesh_id,
                        epoch = outcome.epoch,
                        overlay_ip = %outcome.overlay_ip,
                        "nebula-csr-watcher: signed peer cert + wrote bundle",
                    );
                    // OV-7.c leader-side emission — fire
                    // EnrollmentCompleted so any subscriber on the
                    // leader's peer (Workbench Overview / applets)
                    // re-probes capability status immediately
                    // rather than waiting for the next reconcile
                    // tick. Empty slot (peer-role box, pre-IPC
                    // startup) → silent no-op; the SQL + bundle
                    // write already landed.
                    if let Some(slot) = &self.signal_slot {
                        if let Some(sender) = slot.get() {
                            sender.emit(NebulaSignal::EnrollmentCompleted {
                                node_id: peer_id.clone(),
                            });
                        }
                    }
                }
                Err(e) => {
                    // Common case on peer-role boxes: no active
                    // CA. Log at debug to avoid spamming the
                    // journal on every non-lighthouse box.
                    tracing::debug!(
                        peer_id = %peer_id,
                        error = %e,
                        "nebula-csr-watcher: sign skipped",
                    );
                }
            }
        }
    }
}

/// Pure helper — scan `workgroup_root` for `*/mackesd/pending-enroll.json`
/// entries and return the peer_id slugs (the directory name under
/// workgroup_root). Empty Vec on a missing root.
///
/// # Errors
///
/// Surfaces I/O errors from reading workgroup_root. Permission denied
/// counts as "no peers" (worker logs at debug + moves on).
pub fn discover_pending_peers(workgroup_root: &Path) -> std::io::Result<Vec<String>> {
    if !workgroup_root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(workgroup_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let csr = path.join("mackesd").join("pending-enroll.json");
        if csr.exists() {
            if let Some(peer) = path.file_name().and_then(|s| s.to_str()) {
                out.push(peer.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Pure helper — decide whether a peer's CSR needs signing. The
/// rule: sign if no bundle exists, OR if the CSR is newer than the
/// existing bundle (operator-initiated re-enroll). Idempotent: a
/// CSR with a fresh matching bundle stays untouched.
///
/// Returns `Ok(true)` on signing-needed; `Ok(false)` on
/// already-signed; `Err` only on filesystem errors that would
/// make a decision unsafe (the caller treats Err as "try anyway"
/// per the unwrap_or(true) in tick_once — better to attempt a
/// race-condition-safe sign than silently skip).
pub fn needs_signing(workgroup_root: &Path, peer_id: &str) -> std::io::Result<bool> {
    let csr = pending_enroll_path(workgroup_root, peer_id);
    let bundle = bundle_path(workgroup_root, peer_id);
    if !bundle.exists() {
        return Ok(true);
    }
    let csr_mtime = csr.metadata()?.modified()?;
    let bundle_mtime = bundle.metadata()?.modified()?;
    // Strict greater-than — same-mtime files (rare; same-second
    // writes) are treated as already-signed. Re-enroll always
    // generates a fresh CSR with a newer timestamp.
    Ok(csr_mtime > bundle_mtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::{mint, MockBackend};
    use crate::nebula_enroll::{build_pending, parse_join_token, publish_enrollment_request};
    use tempfile::tempdir;

    fn fresh_store() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        conn
    }

    fn make_test_ca(tmp_dir: &Path, conn: &rusqlite::Connection) -> (PathBuf, PathBuf) {
        let ca_crt = tmp_dir.join("ca.crt");
        let ca_key = tmp_dir.join("ca.key");
        mint::mint_ca(
            &MockBackend,
            conn,
            "test-mesh",
            Some(&ca_crt),
            Some(&ca_key),
        )
        .expect("mint");
        (ca_crt, ca_key)
    }

    fn place_csr(workgroup_root: &Path, peer_id: &str) {
        let identity = crate::enrollment::build_identity();
        let token = parse_join_token("mesh:test-mesh@10.0.0.5:4242#bearer").unwrap();
        // ENT-1 — the watcher signs only issued bearers now.
        crate::bearer_ledger::record_issued(workgroup_root, &token.bearer).expect("seed bearer");
        let pending = build_pending(&identity, peer_id, "anvil", token);
        publish_enrollment_request(workgroup_root, peer_id, &pending).expect("publish");
    }

    #[test]
    fn discover_returns_empty_for_missing_root() {
        let p = PathBuf::from("/this/path/definitely/does/not/exist");
        assert!(discover_pending_peers(&p).unwrap().is_empty());
    }

    #[test]
    fn discover_returns_empty_for_empty_root() {
        let tmp = tempdir().unwrap();
        assert!(discover_pending_peers(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn discover_finds_csrs_one_per_peer_dir() {
        let tmp = tempdir().unwrap();
        place_csr(tmp.path(), "peer:anvil");
        place_csr(tmp.path(), "peer:birch");
        place_csr(tmp.path(), "peer:oak");
        // A peer dir without a CSR (mid-enrollment heartbeat
        // only) should NOT appear in the output.
        let no_csr_dir = tmp.path().join("peer:cedar").join("mackesd");
        std::fs::create_dir_all(&no_csr_dir).unwrap();
        std::fs::write(no_csr_dir.join("heartbeat.json"), "{}").unwrap();
        let peers = discover_pending_peers(tmp.path()).unwrap();
        assert_eq!(peers, vec!["peer:anvil", "peer:birch", "peer:oak"]);
    }

    #[test]
    fn discover_skips_non_directory_entries() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("stray.txt"), "hi").unwrap();
        place_csr(tmp.path(), "peer:anvil");
        let peers = discover_pending_peers(tmp.path()).unwrap();
        assert_eq!(peers, vec!["peer:anvil"]);
    }

    #[test]
    fn needs_signing_returns_true_when_no_bundle() {
        let tmp = tempdir().unwrap();
        place_csr(tmp.path(), "peer:anvil");
        assert!(needs_signing(tmp.path(), "peer:anvil").unwrap());
    }

    #[test]
    fn needs_signing_returns_false_when_bundle_is_newer() {
        let tmp = tempdir().unwrap();
        place_csr(tmp.path(), "peer:anvil");
        // Place a bundle dated AFTER the CSR.
        let bp = bundle_path(tmp.path(), "peer:anvil");
        std::fs::create_dir_all(bp.parent().unwrap()).unwrap();
        // Write the bundle slightly after the CSR by sleeping a
        // few ms. (Linux fs mtime resolution is nanoseconds on
        // most filesystems, so even 5 ms is enough.)
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&bp, b"{}").unwrap();
        assert!(!needs_signing(tmp.path(), "peer:anvil").unwrap());
    }

    #[test]
    fn needs_signing_returns_true_when_csr_is_newer_than_bundle() {
        let tmp = tempdir().unwrap();
        // Place bundle first.
        let bp = bundle_path(tmp.path(), "peer:anvil");
        std::fs::create_dir_all(bp.parent().unwrap()).unwrap();
        std::fs::write(&bp, b"{}").unwrap();
        std::thread::sleep(Duration::from_millis(10));
        place_csr(tmp.path(), "peer:anvil");
        // CSR is newer → re-sign.
        assert!(needs_signing(tmp.path(), "peer:anvil").unwrap());
    }

    #[test]
    fn tick_once_signs_pending_peers() {
        let tmp = tempdir().unwrap();
        let db = tmp.path().join("mded.db");
        // Initialize store via mint_ca (writes the CA cert/key).
        let conn = rusqlite::Connection::open(&db).expect("open db");
        crate::store::migrate(&conn).expect("migrate");
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        drop(conn); // release the lock so the worker can reopen
        place_csr(tmp.path(), "peer:anvil");
        let worker = NebulaCsrWatcher::new(
            tmp.path().to_path_buf(),
            db.clone(),
            "test-mesh".to_string(),
            "peer:lh".to_string(),
            "lh.example:4242".to_string(),
        )
        .with_paths(SignCsrPaths {
            ca_crt: ca_crt.clone(),
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        })
        .with_backend(Arc::new(MockBackend));
        worker.tick_once();
        let bp = bundle_path(tmp.path(), "peer:anvil");
        assert!(bp.exists(), "expected bundle at {}", bp.display());
        // Second tick is a no-op (bundle exists, CSR not newer).
        worker.tick_once();
        let conn = rusqlite::Connection::open(&db).unwrap();
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = ?1",
                ["peer:anvil"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            row_count, 1,
            "expected exactly one cert row (idempotent tick)"
        );
    }

    #[test]
    fn tick_once_is_noop_on_empty_workgroup_root() {
        let tmp = tempdir().unwrap();
        let db = tmp.path().join("mded.db");
        let conn = rusqlite::Connection::open(&db).expect("open db");
        crate::store::migrate(&conn).expect("migrate");
        drop(conn);
        let worker = NebulaCsrWatcher::new(
            tmp.path().to_path_buf(),
            db,
            "test-mesh".to_string(),
            "peer:lh".to_string(),
            "lh.example:4242".to_string(),
        );
        // Should not panic, should not log loudly.
        worker.tick_once();
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown() {
        let tmp = tempdir().unwrap();
        let mut w = NebulaCsrWatcher::new(
            tmp.path().to_path_buf(),
            tmp.path().join("mded.db"),
            "test-mesh".to_string(),
            "peer:lh".to_string(),
            "lh.example:4242".to_string(),
        )
        .with_tick(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(2), w.run(token)).await;
        assert!(result.is_ok());
    }

    #[test]
    fn worker_name_is_kebab_case() {
        let w = NebulaCsrWatcher::new(
            PathBuf::from("/tmp/x"),
            PathBuf::from("/tmp/x.db"),
            "m".into(),
            "p".into(),
            "p:1".into(),
        );
        assert_eq!(w.name(), "nebula-csr-watcher");
    }
}

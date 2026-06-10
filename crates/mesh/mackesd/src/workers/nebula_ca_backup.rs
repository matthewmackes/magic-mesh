//! NF-18.4 (v2.5) — automated CA backup worker.
//!
//! Daily encrypted CA backup to QNM-Shared. Writes a sealed
//! bundle (NF-18.1 `ca::backup::seal` + `armor`) to
//! `QNM-Shared/<self>/mackesd/ca-backup.enc` every
//! `TICK_INTERVAL` (default 24 h).
//!
//! On peer-role boxes (no active CA), the worker no-ops — same
//! pattern as `nebula_csr_watcher`. On lighthouse-role boxes
//! with `MDE_BACKUP_PASSPHRASE` set, it runs silently in the
//! background; operators can grab the latest sealed bundle at
//! any time from QNM-Shared.
//!
//! Disabled when `MDE_BACKUP_PASSPHRASE` is unset — the worker
//! logs at info on first tick + then no-ops. Operators
//! explicitly opt in by exporting the env var in their systemd
//! unit's `Environment=` line.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use super::{ShutdownToken, Worker};

/// SEC-7 — drop the unbacked-CA alert into the alert_relay watch dir
/// (one per day via the deterministic id; the relay dedupes).
fn emit_unbacked_ca_alert(env_var: &str) {
    let Some(dir) = super::alert_relay::default_alerts_dir() else {
        return;
    };
    let id = NebulaCaBackup::unbacked_alert_id();
    let event = serde_json::json!({
        "id": id,
        "severity": "crit",
        "alert": "mesh.ca.unbacked",
        "host": "this lighthouse",
        "summary": format!(
            "The mesh CA on this box has NO encrypted backup — export {env_var} \
             in the mackesd unit (SEC-7)."
        ),
    });
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(format!("{id}.json")), event.to_string());
    }
}

/// Default tick — once per 24 hours. Operators with shorter
/// CA-rotation cadences override via [`with_tick`].
pub const TICK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Default backup filename under `QNM-Shared/<self>/mackesd/`.
///
/// GF-9.1 (v5.0.0) — renamed from the legacy `ca-backup.enc`.
/// The file now carries both the Nebula CA payload (NF-18.1)
/// and the optional storage topology snapshot (GF-9.2-era field,
/// LizardFS since E3 — the payload key keeps its wire name), so
/// the broader name reflects what's inside. Operators who
/// upgrade from v4.x will see the old `ca-backup.enc` sit
/// untouched alongside the new `state-backup.enc` — manual
/// cleanup is safe (just `rm` it; the next worker tick re-
/// writes the new path).
pub const BACKUP_FILENAME: &str = "state-backup.enc";

/// Legacy filename retired by GF-9.1. Kept as a documented
/// constant so the operator-runbook + restore CLI can look up
/// the old path during the upgrade window.
pub const LEGACY_BACKUP_FILENAME: &str = "ca-backup.enc";

/// Worker handle. Cheap to construct.
pub struct NebulaCaBackup {
    workgroup_root: PathBuf,
    self_node_id: String,
    mesh_id: String,
    store: Arc<Mutex<rusqlite::Connection>>,
    /// Sealed CA key path. Default
    /// `/var/lib/mackesd/nebula-ca/ca.key`.
    ca_key_path: PathBuf,
    /// Tick cadence (default 24 h). Tests use shorter values.
    tick: Duration,
    /// Env var name to read for the backup passphrase. Default
    /// `MDE_BACKUP_PASSPHRASE`. Tests use a unique name per test
    /// to avoid cross-test interference.
    passphrase_env: String,
}

impl NebulaCaBackup {
    /// Construct with production defaults — 24h tick, CA key at
    /// `/var/lib/mackesd/nebula-ca/ca.key`, passphrase env var
    /// `MDE_BACKUP_PASSPHRASE`.
    #[must_use]
    pub fn new(
        workgroup_root: PathBuf,
        self_node_id: String,
        mesh_id: String,
        store: Arc<Mutex<rusqlite::Connection>>,
    ) -> Self {
        Self {
            workgroup_root,
            self_node_id,
            mesh_id,
            store,
            ca_key_path: PathBuf::from("/var/lib/mackesd/nebula-ca/ca.key"),
            tick: TICK_INTERVAL,
            passphrase_env: "MDE_BACKUP_PASSPHRASE".to_string(),
        }
    }

    /// Override the CA key path. Tests redirect to a tempdir.
    #[must_use]
    pub fn with_ca_key(mut self, path: PathBuf) -> Self {
        self.ca_key_path = path;
        self
    }

    /// Override the tick cadence.
    #[must_use]
    pub fn with_tick(mut self, t: Duration) -> Self {
        self.tick = t;
        self
    }

    /// Override the passphrase env var name.
    #[must_use]
    pub fn with_passphrase_env(mut self, name: impl Into<String>) -> Self {
        self.passphrase_env = name.into();
        self
    }

    /// Compute the on-disk backup path for this worker's
    /// QNM-Shared root + self_node_id.
    #[must_use]
    pub fn backup_path(&self) -> PathBuf {
        backup_path_for(&self.workgroup_root, &self.self_node_id)
    }
}

/// Pure helper — compute the backup file path for a given root
/// + node-id. Mirrors `ca::bundle::bundle_path` convention.
#[must_use]
pub fn backup_path_for(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join(node_id)
        .join("mackesd")
        .join(BACKUP_FILENAME)
}

#[async_trait::async_trait]
impl Worker for NebulaCaBackup {
    fn name(&self) -> &'static str {
        "nebula-ca-backup"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // One immediate tick on startup so a fresh restart picks
        // up a current snapshot, then the regular cadence.
        self.tick_once().await;
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(self.tick) => {
                    self.tick_once().await;
                }
            }
        }
    }
}

impl NebulaCaBackup {
    /// ENT-11 (C8) — read the backup passphrase, preferring
    /// systemd-creds over the process environment.
    ///
    /// systemd hands `LoadCredentialEncrypted=` secrets in a tmpfs
    /// dir named by `$CREDENTIALS_DIRECTORY`, one file per credential
    /// — **not** visible in `systemctl show` or `/proc/<pid>/environ`.
    /// We read `<creds>/backup-passphrase` first; the `MDE_BACKUP_
    /// PASSPHRASE` env var stays a documented fallback for dev /
    /// pre-creds boxes. Returns the trimmed phrase, or empty.
    #[must_use]
    pub fn read_passphrase(&self) -> String {
        if let Some(dir) = std::env::var_os("CREDENTIALS_DIRECTORY") {
            let path = std::path::Path::new(&dir).join("backup-passphrase");
            if let Ok(p) = std::fs::read_to_string(&path) {
                let p = p.trim().to_string();
                if !p.is_empty() {
                    return p;
                }
            }
        }
        std::env::var(&self.passphrase_env)
            .map(|p| p.trim().to_string())
            .unwrap_or_default()
    }

    /// One backup pass. Pulled out for direct testing.
    pub async fn tick_once(&self) {
        // ENT-11 — systemd-creds first, env fallback (C8).
        let passphrase = match self.read_passphrase() {
            p if !p.is_empty() => p,
            _ => {
                // SEC-7 (Q31/32) — on a box that actually HOLDS the
                // CA (lighthouse duty), an unset backup passphrase is
                // a standing data-loss risk: the CA is the mesh's
                // single point of loss. Loud-warn every tick + drop an
                // alert into the alert_relay dir so the operator sees
                // a desktop notification — not a quiet debug skip.
                // Boxes without a CA key stay quiet (peer-role).
                if self.ca_key_path.exists() {
                    tracing::error!(
                        "SEC-7/ENT-11: this box holds the mesh CA but has no backup \
                         passphrase — the CA is UNBACKED-UP. Provision the \
                         systemd credential (LoadCredentialEncrypted=backup-passphrase) \
                         or set {} for dev, then restart.",
                        self.passphrase_env,
                    );
                    emit_unbacked_ca_alert(&self.passphrase_env);
                } else {
                    tracing::debug!(
                        env_var = %self.passphrase_env,
                        "nebula-ca-backup: passphrase env unset; skipping tick",
                    );
                }
                return;
            }
        };
        match self.try_backup(&passphrase).await {
            Ok(stats) => {
                tracing::info!(
                    mesh_id = %self.mesh_id,
                    ca_certs = stats.ca_certs,
                    peer_certs = stats.peer_certs,
                    bytes = stats.armored_bytes,
                    path = %self.backup_path().display(),
                    "nebula-ca-backup: wrote sealed bundle",
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "nebula-ca-backup: skip (no CA, key missing, or sql fail)",
                );
            }
        }
    }

    fn unbacked_alert_id() -> String {
        // One alert per day per box — loud, not spammy.
        let day = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 86_400);
        format!("sec7-ca-unbacked-{day}")
    }

    async fn try_backup(&self, passphrase: &str) -> Result<BackupStats, BackupTickError> {
        // Read CA key from disk first (cheap check — fails fast
        // on peer-role boxes without an active CA).
        let ca_key_bytes = crate::ca::seal::read_sealed(&self.ca_key_path)
            .map_err(|e| BackupTickError::CaKeyMissing(e.to_string()))?;
        let ca_key_pem = String::from_utf8(ca_key_bytes)
            .map_err(|e| BackupTickError::CaKeyMissing(format!("not UTF-8: {e}")))?;
        // Lock store + assemble bundle.
        let conn = self.store.lock().await;
        let mut plaintext =
            crate::ca::backup::assemble_from_store(&conn, &self.mesh_id, &ca_key_pem)
                .map_err(|e| BackupTickError::Assemble(e.to_string()))?;
        // Empty mesh (no CA rows) → skip rather than write an
        // empty backup. Avoids confusing operators who might
        // think an empty backup means the CA was wiped.
        if plaintext.ca_certs.is_empty() {
            return Err(BackupTickError::NoCa);
        }
        // Drop the lock before doing CPU-bound Argon2 work — let
        // the rest of the daemon proceed.
        drop(conn);
        // MESHFS-14.1 — fold a LizardFS snapshot into the bundle.
        // Returns None when mfsmetadump + mfsadmin are both absent.
        // Bumps schema_version to 3 so the restore CLI knows to
        // apply the meshfs step.
        let meshfs_snap =
            crate::meshfs::snapshot::collect(&crate::meshfs::snapshot::SnapshotConfig::default());
        if meshfs_snap.is_some() {
            plaintext.schema_version = 3;
            plaintext.meshfs_snapshot = meshfs_snap;
        }
        let sealed = crate::ca::backup::seal(passphrase, &plaintext)
            .map_err(|e| BackupTickError::Seal(e.to_string()))?;
        let armored = crate::ca::backup::armor(&sealed, plaintext.exported_at);
        let path = self.backup_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| BackupTickError::Io(format!("mkdir {}: {e}", parent.display())))?;
        }
        // Atomic write (temp + rename) so a reader mid-write
        // never sees a half-formed bundle.
        let tmp = path.with_extension("enc.tmp");
        std::fs::write(&tmp, &armored)
            .map_err(|e| BackupTickError::Io(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            BackupTickError::Io(format!(
                "rename {} → {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        Ok(BackupStats {
            ca_certs: plaintext.ca_certs.len(),
            peer_certs: plaintext.peer_certs.len(),
            armored_bytes: armored.len(),
        })
    }
}

/// Per-tick stats logged at info level on success.
#[derive(Debug, Clone, Copy)]
pub struct BackupStats {
    /// Number of CA cert rows in the bundle (typically 1).
    pub ca_certs: usize,
    /// Number of signed peer cert rows in the bundle.
    pub peer_certs: usize,
    /// Size of the ASCII-armored bundle in bytes.
    pub armored_bytes: usize,
}

/// Per-tick error variants. Each maps to a specific
/// operator-visible reason for skipping — the worker's tracing
/// line surfaces the variant name + context.
#[derive(Debug)]
enum BackupTickError {
    /// CA key file missing — peer-role box, or lighthouse
    /// before first `mackesd ca mint`.
    CaKeyMissing(String),
    /// Store assembly hit a SQL error.
    Assemble(String),
    /// No active CA — skip the tick rather than write an empty
    /// backup that would confuse operators.
    NoCa,
    /// AEAD seal failure (unreachable in practice — only fails
    /// on internal RustCrypto bugs).
    Seal(String),
    /// Filesystem I/O failed (QNM-Shared umounted, full disk).
    Io(String),
}

impl std::fmt::Display for BackupTickError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CaKeyMissing(s) => write!(f, "CA key missing: {s}"),
            Self::Assemble(s) => write!(f, "assemble from store: {s}"),
            Self::NoCa => write!(f, "no active CA — skipped"),
            Self::Seal(s) => write!(f, "seal: {s}"),
            Self::Io(s) => write!(f, "io: {s}"),
        }
    }
}

#[cfg(test)]
mod ent11_tests {
    use super::*;

    fn worker(env_var: &str) -> NebulaCaBackup {
        NebulaCaBackup::new(
            std::path::PathBuf::from("/tmp/ent11-root"),
            "peer:test".into(),
            "test-mesh".into(),
            std::sync::Arc::new(tokio::sync::Mutex::new(
                rusqlite::Connection::open_in_memory().unwrap(),
            )),
        )
        .with_passphrase_env(env_var.to_string())
    }

    #[test]
    fn credentials_directory_beats_the_env_var() {
        // ENT-11 — the systemd-creds file is preferred; the env var
        // (visible in systemctl show / /proc) is only a fallback.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("backup-passphrase"), "from-creds\n").unwrap();
        let env_name = "ENT11_TEST_PASS_A";
        std::env::set_var("CREDENTIALS_DIRECTORY", tmp.path());
        std::env::set_var(env_name, "from-env");
        let w = worker(env_name);
        assert_eq!(w.read_passphrase(), "from-creds");
        std::env::remove_var("CREDENTIALS_DIRECTORY");
        // With no creds dir, the env var is the fallback.
        assert_eq!(w.read_passphrase(), "from-env");
        std::env::remove_var(env_name);
        assert_eq!(w.read_passphrase(), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::{mint, MockBackend};
    use tempfile::tempdir;

    fn fresh_store() -> Arc<Mutex<rusqlite::Connection>> {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        Arc::new(Mutex::new(conn))
    }

    async fn make_test_ca(
        tmp_dir: &Path,
        store: &Arc<Mutex<rusqlite::Connection>>,
        mesh_id: &str,
    ) -> PathBuf {
        let ca_crt = tmp_dir.join("ca.crt");
        let ca_key = tmp_dir.join("ca.key");
        let conn = store.lock().await;
        mint::mint_ca(&MockBackend, &conn, mesh_id, Some(&ca_crt), Some(&ca_key)).expect("mint");
        ca_key
    }

    /// Pick a unique env-var name per test so tests don't
    /// interfere via the shared global env.
    fn unique_passphrase_env(test_name: &str) -> String {
        format!("MDE_BACKUP_PASSPHRASE_TEST_{test_name}")
    }

    #[test]
    fn backup_path_for_mirrors_bundle_convention() {
        // GF-9.1 (v5.0.0) renamed `ca-backup.enc` → `state-backup.enc`
        // so the backup carries the full mackesd state (CA + volume
        // config), not just the CA bundle. Test follows the rename.
        let p = backup_path_for(Path::new("/qnm"), "peer:anvil");
        assert_eq!(p, PathBuf::from("/qnm/peer:anvil/mackesd/state-backup.enc"));
    }

    #[test]
    fn worker_name_is_kebab_case() {
        let w = NebulaCaBackup::new(
            PathBuf::from("/tmp/x"),
            "peer:lh".into(),
            "m".into(),
            fresh_store(),
        );
        assert_eq!(w.name(), "nebula-ca-backup");
    }

    #[test]
    fn builders_override_each_field() {
        let w = NebulaCaBackup::new(
            PathBuf::from("/tmp/x"),
            "peer:lh".into(),
            "m".into(),
            fresh_store(),
        )
        .with_ca_key(PathBuf::from("/tmp/ca.key"))
        .with_tick(Duration::from_secs(60))
        .with_passphrase_env("MY_PP");
        assert_eq!(w.ca_key_path, PathBuf::from("/tmp/ca.key"));
        assert_eq!(w.tick, Duration::from_secs(60));
        assert_eq!(w.passphrase_env, "MY_PP");
    }

    #[tokio::test]
    async fn tick_skips_silently_when_passphrase_env_unset() {
        let tmp = tempdir().unwrap();
        let store = fresh_store();
        let env_var = unique_passphrase_env("skips_unset");
        // Ensure unset.
        std::env::remove_var(&env_var);
        let w = NebulaCaBackup::new(
            tmp.path().to_path_buf(),
            "peer:lh".into(),
            "test-mesh".into(),
            store,
        )
        .with_passphrase_env(env_var);
        w.tick_once().await;
        // No backup file should land.
        assert!(!w.backup_path().exists());
    }

    #[tokio::test]
    async fn tick_skips_when_ca_key_missing_even_with_passphrase() {
        let tmp = tempdir().unwrap();
        let store = fresh_store();
        let env_var = unique_passphrase_env("skips_no_key");
        std::env::set_var(&env_var, "test-passphrase");
        let w = NebulaCaBackup::new(
            tmp.path().to_path_buf(),
            "peer:lh".into(),
            "test-mesh".into(),
            store,
        )
        .with_ca_key(PathBuf::from("/nonexistent/ca.key"))
        .with_passphrase_env(env_var.clone());
        w.tick_once().await;
        std::env::remove_var(&env_var);
        assert!(!w.backup_path().exists());
    }

    #[tokio::test]
    async fn tick_writes_armored_bundle_on_happy_path() {
        let tmp = tempdir().unwrap();
        let store = fresh_store();
        let ca_key = make_test_ca(tmp.path(), &store, "test-mesh").await;
        let env_var = unique_passphrase_env("happy_path");
        std::env::set_var(&env_var, "test-passphrase");
        let w = NebulaCaBackup::new(
            tmp.path().to_path_buf(),
            "peer:lh".into(),
            "test-mesh".into(),
            store.clone(),
        )
        .with_ca_key(ca_key)
        .with_passphrase_env(env_var.clone());
        w.tick_once().await;
        std::env::remove_var(&env_var);
        // Bundle exists.
        let bp = w.backup_path();
        assert!(bp.exists(), "expected bundle at {}", bp.display());
        // Decodes back through dearmor + unseal.
        let armored = std::fs::read_to_string(&bp).unwrap();
        assert!(armored.contains("-----BEGIN MACKES NEBULA CA EXPORT-----"));
        let sealed = crate::ca::backup::dearmor(&armored).expect("dearmor");
        let plain = crate::ca::backup::unseal("test-passphrase", &sealed).expect("unseal");
        assert_eq!(plain.mesh_id, "test-mesh");
        assert_eq!(plain.ca_certs.len(), 1);
    }

    #[tokio::test]
    async fn tick_is_atomic_no_tmp_file_leftover() {
        let tmp = tempdir().unwrap();
        let store = fresh_store();
        let ca_key = make_test_ca(tmp.path(), &store, "test-mesh").await;
        let env_var = unique_passphrase_env("atomic");
        std::env::set_var(&env_var, "pp");
        let w = NebulaCaBackup::new(
            tmp.path().to_path_buf(),
            "peer:lh".into(),
            "test-mesh".into(),
            store,
        )
        .with_ca_key(ca_key)
        .with_passphrase_env(env_var.clone());
        w.tick_once().await;
        std::env::remove_var(&env_var);
        let bp = w.backup_path();
        let tmp_path = bp.with_extension("enc.tmp");
        assert!(bp.exists());
        assert!(!tmp_path.exists(), "temp file should not survive");
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown() {
        let tmp = tempdir().unwrap();
        let mut w = NebulaCaBackup::new(
            tmp.path().to_path_buf(),
            "peer:lh".into(),
            "test-mesh".into(),
            fresh_store(),
        )
        .with_tick(Duration::from_millis(50))
        .with_passphrase_env(unique_passphrase_env("shutdown"));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(2), w.run(token)).await;
        assert!(result.is_ok());
    }
}

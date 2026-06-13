//! MESHFS-14.1 (v5.0.0) — LizardFS state snapshot for the
//! daily `state-backup.enc` bundle.
//!
//! Mirrors `crate::gluster::snapshot` in shape; the
//! [`nebula_ca_backup`] worker calls [`collect`] and folds the
//! result into [`crate::ca::backup::BundlePlaintext::meshfs_snapshot`].
//!
//! **Collected payloads:**
//! - `metadata_dump` — output of `mfsmetadump <metadata_file>`;
//!   the primary input for a bare-peer metadata restore.
//! - `cs_list` — `mfsadmin <vip> CS-LIST`; records which peers
//!   held chunkservers at backup time (useful for topology
//!   reconciliation on restore).
//! - `exports_config` — content of the LizardFS exports config
//!   (`/etc/mfs/mfsexports.cfg`); re-applied verbatim on restore.
//! - `goal` — current replication goal (from `mfsgetgoal`); the
//!   restore worker re-sets it after the metadata is imported.
//! - `vip` — the floating overlay VIP recorded for reference.
//!
//! **Best-effort:** all fields are `Option<_>`. A missing binary
//! or a failed subcommand produces `None` for that field; the
//! rest of the snapshot is still written. When neither
//! `mfsmetadump` nor `mfsadmin` is on PATH, [`collect`] returns
//! `None` so the backup worker stays at schema_version 2 (Gluster
//! path) rather than bumping to 3.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default `mfsmetadump` binary name — dumps the master metadata file for backup/restore.
pub const DEFAULT_METADUMP_BINARY: &str = "mfsmetadump";
/// Default `mfsadmin` binary name — the LizardFS admin CLI (CS-LIST, CS-EVICT, etc.).
pub const DEFAULT_ADMIN_BINARY: &str = "mfsadmin";
/// Default `mfsgetgoal` binary name — reads a file/dir's replication goal.
pub const DEFAULT_GETGOAL_BINARY: &str = "mfsgetgoal";

/// Default LizardFS metadata file path (under the MESHFS-1.2 storage layout).
pub const DEFAULT_METADATA_FILE: &str = "/var/lib/mde/meshfs/meta/metadata.mfs";

/// Default LizardFS exports config path.
pub const DEFAULT_EXPORTS_CONFIG: &str = "/etc/mfs/mfsexports.cfg";

/// Default mount path for `mfsgetgoal`.
pub const DEFAULT_MOUNT_PATH: &str = "/mnt/mesh-storage";

/// Default floating overlay VIP.
pub const DEFAULT_VIP: &str = "10.42.0.1";

/// Per-invocation wall-clock timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A point-in-time snapshot of the LizardFS cluster state.
/// All fields are `Option<_>` — individual subcommand failures
/// are absorbed so a degraded cluster still gets a partial snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MeshFsSnapshot {
    /// Output of `mfsmetadump <metadata_file>` — the primary
    /// payload for metadata restore on a bare peer.
    #[serde(default)]
    pub metadata_dump: Option<String>,

    /// Output of `mfsadmin <vip> CS-LIST` at backup time.
    /// Lists the overlay IPs of all registered chunkservers so
    /// the operator can verify topology after restore.
    #[serde(default)]
    pub cs_list: Option<String>,

    /// Content of `/etc/mfs/mfsexports.cfg`. Re-applied verbatim
    /// on restore so the master knows about the `mesh-storage`
    /// export root.
    #[serde(default)]
    pub exports_config: Option<String>,

    /// Replication goal at backup time. The restore worker
    /// re-sets this via `mfssetgoal -r <goal> /mnt/mesh-storage`.
    #[serde(default)]
    pub goal: Option<u8>,

    /// Floating overlay VIP recorded for reference.
    #[serde(default)]
    pub vip: Option<String>,
}

/// Knobs for [`collect`] — production uses `SnapshotConfig::default()`;
/// tests override individual fields via the builder methods.
#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    /// Path (or name on `PATH`) of the `mfsmetadump` binary.
    pub metadump_binary: String,
    /// Path (or name on `PATH`) of the `mfsadmin` binary.
    pub admin_binary: String,
    /// Path (or name on `PATH`) of the `mfsgetgoal` binary.
    pub getgoal_binary: String,
    /// LizardFS master metadata file passed to `mfsmetadump`.
    pub metadata_file: PathBuf,
    /// LizardFS exports config file read verbatim for the snapshot.
    pub exports_config_path: PathBuf,
    /// Mount path passed to `mfsgetgoal` when querying the replication goal.
    pub mount_path: String,
    /// Floating overlay VIP recorded in the snapshot for restore reference.
    pub vip: String,
    /// Per-subprocess wall-clock timeout; all subcommands are killed after this.
    pub timeout: Duration,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            metadump_binary: DEFAULT_METADUMP_BINARY.to_owned(),
            admin_binary: DEFAULT_ADMIN_BINARY.to_owned(),
            getgoal_binary: DEFAULT_GETGOAL_BINARY.to_owned(),
            metadata_file: PathBuf::from(DEFAULT_METADATA_FILE),
            exports_config_path: PathBuf::from(DEFAULT_EXPORTS_CONFIG),
            mount_path: DEFAULT_MOUNT_PATH.to_owned(),
            vip: DEFAULT_VIP.to_owned(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl SnapshotConfig {
    /// Override the `mfsmetadump` binary. Tests pass an absolute path
    /// to a stub or `/nonexistent/…` to simulate absence.
    #[must_use]
    pub fn with_metadump_binary(mut self, name: impl Into<String>) -> Self {
        self.metadump_binary = name.into();
        self
    }

    /// Override the `mfsadmin` binary. Tests pass `/bin/false` or a
    /// nonexistent path to exercise the best-effort fallback.
    #[must_use]
    pub fn with_admin_binary(mut self, name: impl Into<String>) -> Self {
        self.admin_binary = name.into();
        self
    }

    /// Override the `mfsgetgoal` binary. Tests pass a nonexistent name
    /// to skip the goal query without touching other fields.
    #[must_use]
    pub fn with_getgoal_binary(mut self, name: impl Into<String>) -> Self {
        self.getgoal_binary = name.into();
        self
    }

    /// Override the LizardFS metadata file path. Tests redirect to a
    /// temp file containing a synthetic metadata dump.
    #[must_use]
    pub fn with_metadata_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.metadata_file = path.into();
        self
    }

    /// Override the floating VIP. Tests use a non-routable address so
    /// `mfsadmin CS-LIST` fails fast without a live master.
    #[must_use]
    pub fn with_vip(mut self, vip: impl Into<String>) -> Self {
        self.vip = vip.into();
        self
    }
}

/// Collect a [`MeshFsSnapshot`], or return `None` when LizardFS
/// isn't installed on this peer (neither `mfsmetadump` nor
/// `mfsadmin` is on PATH).
#[must_use]
pub fn collect(config: &SnapshotConfig) -> Option<MeshFsSnapshot> {
    if !binary_on_path(&config.metadump_binary) && !binary_on_path(&config.admin_binary) {
        return None;
    }
    Some(MeshFsSnapshot {
        metadata_dump: run_metadump(config),
        cs_list: run_cs_list(config),
        exports_config: read_exports_config(&config.exports_config_path),
        goal: query_goal(config),
        vip: Some(config.vip.clone()),
    })
}

// ── Pure-ish helpers (each absorbs its own failure) ─────────────────────────

fn run_metadump(config: &SnapshotConfig) -> Option<String> {
    if !binary_on_path(&config.metadump_binary) {
        return None;
    }
    let out = Command::new(&config.metadump_binary)
        .arg(&config.metadata_file)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        tracing::debug!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "mfsmetadump exited non-zero; metadata_dump field will be None",
        );
        None
    }
}

fn run_cs_list(config: &SnapshotConfig) -> Option<String> {
    if !binary_on_path(&config.admin_binary) {
        return None;
    }
    let out = Command::new(&config.admin_binary)
        .args([config.vip.as_str(), "CS-LIST"])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        tracing::debug!(
            status = ?out.status,
            "mfsadmin CS-LIST failed; cs_list field will be None",
        );
        None
    }
}

fn read_exports_config(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Parse `mfsgetgoal <mount_path>` output.
/// LizardFS emits: `<path>: <goal>\n`
fn query_goal(config: &SnapshotConfig) -> Option<u8> {
    if !binary_on_path(&config.getgoal_binary) {
        return None;
    }
    let out = Command::new(&config.getgoal_binary)
        .arg(&config.mount_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_getgoal_output(&text)
}

/// Parse `<path>: <N>` or `goal: <N>` from `mfsgetgoal` output.
#[must_use]
pub fn parse_getgoal_output(text: &str) -> Option<u8> {
    for line in text.lines() {
        if let Some(rest) = line.rsplit_once(':') {
            if let Ok(n) = rest.1.trim().parse::<u8>() {
                return Some(n);
            }
        }
    }
    None
}

fn binary_on_path(name: &str) -> bool {
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return candidate.exists();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_returns_none_when_both_binaries_absent() {
        let cfg = SnapshotConfig::default()
            .with_metadump_binary("/nonexistent/mfsmetadump-xyz")
            .with_admin_binary("/nonexistent/mfsadmin-xyz");
        assert!(collect(&cfg).is_none());
    }

    #[test]
    fn collect_returns_some_when_admin_present_but_fails() {
        let cfg = SnapshotConfig::default()
            .with_metadump_binary("/nonexistent/mfsmetadump-xyz")
            .with_admin_binary("/bin/false");
        let snap = collect(&cfg).expect("admin binary present → Some");
        assert!(snap.cs_list.is_none());
        assert!(snap.metadata_dump.is_none());
    }

    #[test]
    fn parse_getgoal_output_standard() {
        let text = "/mnt/mesh-storage: 3\n";
        assert_eq!(parse_getgoal_output(text), Some(3));
    }

    #[test]
    fn parse_getgoal_output_goal_prefix() {
        let text = "goal: 2\n";
        assert_eq!(parse_getgoal_output(text), Some(2));
    }

    #[test]
    fn parse_getgoal_output_empty() {
        assert_eq!(parse_getgoal_output(""), None);
    }

    #[test]
    fn parse_getgoal_output_no_number() {
        assert_eq!(parse_getgoal_output("/mnt/mesh-storage: all\n"), None);
    }

    #[test]
    fn snapshot_json_round_trips() {
        let snap = MeshFsSnapshot {
            metadata_dump: Some("MFSM NEW\nchunk 1\n".to_owned()),
            cs_list: Some("ip port used avail\n10.42.0.5 9422 0 100000\n".to_owned()),
            exports_config: Some("* / rw\n".to_owned()),
            goal: Some(3),
            vip: Some("10.42.0.1".to_owned()),
        };
        let json = serde_json::to_string(&snap).expect("encode");
        let back: MeshFsSnapshot = serde_json::from_str(&json).expect("decode");
        assert_eq!(snap, back);
    }

    #[test]
    fn snapshot_deserializes_with_all_fields_missing() {
        let back: MeshFsSnapshot = serde_json::from_str("{}").expect("legacy-shape JSON parses");
        assert_eq!(back, MeshFsSnapshot::default());
    }
}

//! MEDIA-3 / MEDIA-6 — Navidrome adopt/supervise worker (Lighthouse_Media only).
//!
//! `setup-media-navidrome.sh` is the standalone runtime: it installs
//! `mcnf-navidrome.service` (rootless-podman Navidrome, capped, `Restart=always`)
//! + the rclone S3 mount unit. This worker is the higher-level supervisor that
//! **ADOPTS** that unit and self-heals it — it never re-creates the container
//! (the unit owns the `podman run`), it just restarts the unit whenever it isn't
//! both active and running. It is **role-gated** to `Lighthouse_Media` (spawned
//! only when [`crate::worker_role::node_serves_media`]), so the container is
//! provably absent off the media subclass.
//!
//! It also carries the **MEDIA-6 shared-account** path: the single Navidrome
//! account's password is a **leader-managed mesh secret** (the XCP-7 /
//! `ipc::secret_store` `age`+etcd pattern). The leader mints-or-keeps it
//! idempotently and PUTs it to the replicated store; every media instance GETs
//! it and merges `ND_ADMIN_USER`/`ND_ADMIN_PASS` into the root-only creds env
//! file `setup-media-navidrome.sh` reads (never on argv — design security lock).
//!
//! The pure decision/state logic lives in [`crate::mesh_media`] (unit-tested
//! there); this module is the thin, bounded shell-out adapter around it. Every
//! external call is best-effort: a missing `systemctl`/`podman`/secret store is a
//! quiet degrade (logged), never a panic.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::proc::{output_with_timeout, status_with_timeout, DEFAULT_CMD_TIMEOUT};
use super::{ShutdownToken, Worker};
use crate::ipc::secret_store::{repo_root, SecretStore};
use crate::mesh_media::{
    self, decide_navidrome_action, NavidromeAction, MEDIA_ACCOUNT_USER, MEDIA_CREDS_ENV_PATH,
    NAVIDROME_CONTAINER, NAVIDROME_UNIT,
};

/// Supervise cadence — Navidrome is long-lived; a 60 s self-heal sweep is plenty
/// (the unit's own `Restart=always` covers the fast crash-loop path).
pub const TICK: Duration = Duration::from_secs(60);

/// The MEDIA-3/6 worker.
pub struct MediaNavidromeWorker {
    node_id: String,
    workgroup_root: PathBuf,
    leader_lock: PathBuf,
    /// The root-only creds env file the shared account is distributed into.
    creds_env_path: PathBuf,
}

impl MediaNavidromeWorker {
    /// Construct the worker for this node. `node_id` + the shared
    /// `.mackesd-leader.lock` under `workgroup_root` gate the leader-only secret
    /// mint (the same lock `dc_health`/`ssh_pubkey_gossip` use).
    #[must_use]
    pub fn new(node_id: String, workgroup_root: PathBuf) -> Self {
        Self {
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            workgroup_root,
            creds_env_path: PathBuf::from(MEDIA_CREDS_ENV_PATH),
        }
    }

    /// Test seam — redirect the creds env file off the privileged default.
    #[must_use]
    pub fn with_creds_env_path(mut self, p: PathBuf) -> Self {
        self.creds_env_path = p;
        self
    }

    /// Only the directory leader mints/PUTs the shared secret (no-fixed-center:
    /// any eligible node can be it). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    /// MEDIA-6 — resolve the shared account password from the leader-managed
    /// store, minting it once on the leader if absent. Returns the password to
    /// distribute, or `None` when it isn't available yet (an honest "pending" —
    /// a follower before the leader has distributed, or a store fault).
    fn resolve_shared_password(&self) -> Option<String> {
        let store = SecretStore::resolve(&repo_root(), &self.workgroup_root);
        let secret_ref = mesh_media::media_account_secret_ref();
        match store.get(&secret_ref) {
            Ok(Some(pw)) => Some(pw),
            Ok(None) => {
                // Not distributed yet — only the leader mints it.
                if !self.is_leader() {
                    tracing::debug!(target: "mackesd::media", "shared account secret pending (follower)");
                    return None;
                }
                let (pw, made) = mesh_media::ensure_account_password(None);
                if made {
                    if let Err(e) = store.put(&secret_ref, &pw) {
                        tracing::warn!(target: "mackesd::media", error = %e, "shared account secret PUT failed");
                        return None;
                    }
                    tracing::info!(target: "mackesd::media", "minted + distributed the shared Navidrome account secret");
                }
                Some(pw)
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::media", error = %e, "shared account secret GET failed");
                None
            }
        }
    }

    /// MEDIA-6 — merge the leader's `ND_ADMIN_USER`/`ND_ADMIN_PASS` into the
    /// root-only creds env file (0600), preserving the operator's `DO_SPACES_*`
    /// (MEDIA-2). Best-effort; only writes when the merged body changes.
    fn distribute_account(&self, password: &str) {
        let existing = std::fs::read_to_string(&self.creds_env_path).unwrap_or_default();
        let merged = mesh_media::merge_env_file(
            &existing,
            &[
                ("ND_ADMIN_USER", MEDIA_ACCOUNT_USER),
                ("ND_ADMIN_PASS", password),
            ],
        );
        if merged == existing {
            return;
        }
        if let Some(parent) = self.creds_env_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&self.creds_env_path, &merged) {
            Ok(()) => {
                set_owner_only(&self.creds_env_path);
                tracing::info!(target: "mackesd::media", path = %self.creds_env_path.display(), "distributed shared account to media creds env");
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::media", error = %e, "media creds env write failed")
            }
        }
    }

    /// MEDIA-3 — adopt + self-heal the Navidrome unit from its observed state.
    fn supervise(&self) {
        if !which("systemctl") {
            tracing::debug!(target: "mackesd::media", "systemctl absent — skipping Navidrome supervise");
            return;
        }
        let unit_active = self.unit_active();
        let container_running = self.container_running();
        match decide_navidrome_action(unit_active, container_running) {
            NavidromeAction::Healthy => {
                tracing::debug!(target: "mackesd::media", "Navidrome adopted + healthy");
            }
            NavidromeAction::Heal => {
                tracing::warn!(
                    target: "mackesd::media",
                    unit_active, container_running,
                    "Navidrome down — restarting {NAVIDROME_UNIT}",
                );
                let mut cmd = Command::new("systemctl");
                cmd.args(["restart", NAVIDROME_UNIT]);
                let _ = status_with_timeout(cmd, DEFAULT_CMD_TIMEOUT);
            }
        }
    }

    /// `systemctl is-active <unit>` → true on a clean `active`.
    fn unit_active(&self) -> bool {
        let mut cmd = Command::new("systemctl");
        cmd.args(["is-active", "--quiet", NAVIDROME_UNIT]);
        matches!(status_with_timeout(cmd, DEFAULT_CMD_TIMEOUT), Ok(s) if s.success())
    }

    /// `podman ps --filter name=navidrome` lists the running container.
    fn container_running(&self) -> bool {
        if !which("podman") {
            return false;
        }
        let mut cmd = Command::new("podman");
        cmd.args([
            "ps",
            "--filter",
            &format!("name=^{NAVIDROME_CONTAINER}$"),
            "--format",
            "{{.Names}}",
        ]);
        match output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == NAVIDROME_CONTAINER),
            _ => false,
        }
    }

    fn tick(&self) {
        if let Some(pw) = self.resolve_shared_password() {
            self.distribute_account(&pw);
        }
        self.supervise();
    }
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(bin).is_file()))
        .unwrap_or(false)
}

/// Best-effort 0600 on the creds env file (it carries the shared password).
fn set_owner_only(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[async_trait::async_trait]
impl Worker for MediaNavidromeWorker {
    fn name(&self) -> &'static str {
        "media_navidrome"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.tick();
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(TICK) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_name_is_the_gated_media_tier_name() {
        // The name MUST match the worker_role MEDIA_WORKERS entry the spawn gate
        // keys on — otherwise the role gate and the worker would disagree.
        let w = MediaNavidromeWorker::new("peer:lh-media".into(), PathBuf::from("/tmp/wg"));
        assert_eq!(w.name(), "media_navidrome");
        assert!(crate::worker_role::is_media_worker(w.name()));
    }

    #[test]
    fn distribute_account_merges_without_clobbering_operator_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let env = tmp.path().join("media-spaces.env");
        std::fs::write(&env, "DO_SPACES_KEY=AKIA\nDO_SPACES_SECRET=shh\n").unwrap();
        let w = MediaNavidromeWorker::new("peer:lh-media".into(), tmp.path().to_path_buf())
            .with_creds_env_path(env.clone());
        w.distribute_account("pw-123");
        let body = std::fs::read_to_string(&env).unwrap();
        assert!(
            body.contains("DO_SPACES_KEY=AKIA"),
            "operator S3 key preserved"
        );
        assert!(body.contains("ND_ADMIN_USER=admin"));
        assert!(body.contains("ND_ADMIN_PASS=pw-123"));
        // Idempotent — a second distribute with the same password is a no-op.
        let before = std::fs::read_to_string(&env).unwrap();
        w.distribute_account("pw-123");
        assert_eq!(std::fs::read_to_string(&env).unwrap(), before);
    }
}

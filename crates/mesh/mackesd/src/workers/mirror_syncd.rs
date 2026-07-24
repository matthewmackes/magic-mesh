#![cfg(feature = "async-services")]
//! PLANES-24 W63 — the scheduled one-puller mirror sync worker.
//!
//! Ties the `mirrors` building blocks into a periodic daemon:
//!
//!   * **Every node, every tick** writes its dnf `.repo` so it self-serves
//!     from the local `file://` mount (upstream as fallback, W62) — idempotent
//!     and cheap, so even a node that joined after the last pull is pointed at
//!     the replicated mirror immediately.
//!   * **Only the leader** runs the actual pull
//!     ([`crate::mirrors::sync_mirror`]: `dnf reposync` → `createrepo_c` →
//!     stamp `.last-sync`). Syncthing replicates the result to every other node,
//!     so the fleet mirrors GitHub exactly once per tick — the "one-puller"
//!     contract (W63). Leadership is proxied by the role-host marker, the same
//!     signal [`super::netdata_aggregator`] uses.
//!
//! A bad tick (network down, `dnf`/`createrepo_c` missing) is logged and
//! swallowed; the next tick retries.

use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};
use crate::mirrors::{self, MirrorSyncRunner, SubprocessSync};

/// Default sync cadence. Hourly — `dnf reposync` only fetches deltas, so a
/// frequent tick is cheap, and a fresh node's `.repo` is written promptly.
pub const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_secs(3600);

/// The role-host marker whose existence proxies "this node is the leader"
/// (the single puller). Matches the other workers' convention.
pub const DEFAULT_ROLE_HOST_MARKER: &str = "/var/lib/mackesd/nebula/role.host";

/// The scheduled mirror-sync worker.
pub struct MirrorSyncd {
    workgroup_root: PathBuf,
    role_marker_path: PathBuf,
    node_id: String,
    leadership_endpoints: Vec<String>,
    repo_dir: PathBuf,
    tick_interval: Duration,
    runner: Box<dyn MirrorSyncRunner + Send + Sync>,
}

impl MirrorSyncd {
    /// Construct rooted at the replicated workgroup root. Defaults to the
    /// system role-marker + `/etc/yum.repos.d` + the real subprocess runner.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            role_marker_path: PathBuf::from(DEFAULT_ROLE_HOST_MARKER),
            node_id: local_node_id(),
            leadership_endpoints: crate::substrate::etcd::default_endpoints(),
            repo_dir: PathBuf::from(mirrors::DEFAULT_REPO_DIR),
            tick_interval: DEFAULT_SYNC_INTERVAL,
            runner: Box::new(SubprocessSync),
        }
    }

    /// Override the role-host marker — used by tests to simulate leadership.
    #[must_use]
    pub fn with_role_marker_path(mut self, p: PathBuf) -> Self {
        self.role_marker_path = p;
        self
    }

    /// Override the local node identity — used by isolated authority tests.
    #[must_use]
    pub(crate) fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = node_id.into();
        self
    }

    /// Override the coordination-plane authority — used by isolated tests.
    #[must_use]
    pub(crate) fn with_leadership_endpoints(mut self, endpoints: Vec<String>) -> Self {
        self.leadership_endpoints = endpoints;
        self
    }

    /// Override where `.repo` files land — used by tests (off `/etc`).
    #[must_use]
    pub fn with_repo_dir(mut self, p: PathBuf) -> Self {
        self.repo_dir = p;
        self
    }

    /// Override the tick cadence — used by tests.
    #[must_use]
    pub fn with_tick_interval(mut self, d: Duration) -> Self {
        self.tick_interval = d;
        self
    }

    /// Override the sync runner — used by tests to avoid shelling `dnf`.
    #[must_use]
    pub fn with_runner(mut self, r: Box<dyn MirrorSyncRunner + Send + Sync>) -> Self {
        self.runner = r;
        self
    }

    /// One sweep: write every enabled mirror's `.repo` (self-serve), and —
    /// when leader — pull each one. Per-mirror errors are logged + swallowed.
    async fn tick(&mut self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let is_leader = super::nebula_supervisor::role_marker_is_current_leader(
            &self.role_marker_path,
            &self.workgroup_root,
            &self.node_id,
            &self.leadership_endpoints,
        )
        .await;
        for m in mirrors::load_mirrors(&self.workgroup_root)
            .iter()
            .filter(|m| m.enabled)
        {
            // Every node self-serves from its local mount.
            if let Err(e) = mirrors::write_dnf_repo(m, &self.workgroup_root, &self.repo_dir) {
                tracing::warn!(mirror = %m.name, error = %e, "mirror-syncd: .repo write failed");
            }
            // Single puller: only the leader fetches from upstream.
            if is_leader {
                match mirrors::sync_mirror(&*self.runner, m, &self.workgroup_root, now_ms) {
                    Ok(r) => tracing::info!(
                        mirror = %r.name, rpms = r.rpm_count, at_ms = r.synced_at_ms,
                        "mirror-syncd: pulled + indexed"
                    ),
                    Err(e) => {
                        tracing::warn!(mirror = %m.name, error = %e, "mirror-syncd: sync failed");
                    }
                }
            }
        }
    }
}

fn local_node_id() -> String {
    if let Ok(node_id) = std::env::var("MACKESD_NODE_ID") {
        if !node_id.trim().is_empty() {
            return node_id;
        }
    }
    std::env::var("HOSTNAME")
        .ok()
        .filter(|hostname| !hostname.trim().is_empty())
        .map(|hostname| format!("peer:{hostname}"))
        .unwrap_or_else(|| "peer:unknown".to_owned())
}

#[async_trait::async_trait]
impl Worker for MirrorSyncd {
    fn name(&self) -> &'static str {
        "mirror_syncd"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(self.tick_interval) => {
                    self.tick().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirrors::count_rpms;
    use std::path::Path;

    /// Mock runner: records that a pull happened by dropping a fake RPM,
    /// no `dnf`/`createrepo_c` needed.
    struct MockRunner;
    impl MirrorSyncRunner for MockRunner {
        fn reposync(&self, _name: &str, _upstream: &str, dest: &Path) -> Result<u32, String> {
            std::fs::write(dest.join("pkg-1.0.rpm"), b"rpm").unwrap();
            Ok(count_rpms(dest))
        }
        fn createrepo(&self, dir: &Path) -> Result<(), String> {
            std::fs::create_dir_all(dir.join("repodata")).unwrap();
            Ok(())
        }
    }

    fn worker(root: &Path, marker: PathBuf) -> MirrorSyncd {
        MirrorSyncd::new(root.to_path_buf())
            .with_role_marker_path(marker)
            .with_node_id("peer:test")
            .with_leadership_endpoints(vec![])
            .with_repo_dir(root.join("yum.repos.d"))
            .with_runner(Box::new(MockRunner))
    }

    #[tokio::test]
    async fn leader_tick_pulls_and_self_serves() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("role.host");
        crate::leader::force_take(&tmp.path().join(".mackesd-leader.lock"), "peer:test").unwrap();
        crate::workers::nebula_supervisor::write_role_marker(&marker, "peer:test").unwrap();
        let mut w = worker(tmp.path(), marker);
        w.tick().await;
        let m = &mirrors::core_pack()[0];
        // Leader pulled → .last-sync stamped; and the .repo was written.
        assert!(m.last_sync_ms(tmp.path()).is_some(), "leader must pull");
        assert!(tmp
            .path()
            .join("yum.repos.d/mackes-mirror-magic-mesh.repo")
            .exists());
    }

    #[tokio::test]
    async fn non_leader_tick_self_serves_without_pulling() {
        let tmp = tempfile::tempdir().unwrap();
        // No lease and no marker → not leader.
        let mut w = worker(tmp.path(), tmp.path().join("role.host"));
        w.tick().await;
        let m = &mirrors::core_pack()[0];
        // No pull → no .last-sync; but the node still self-serves (.repo written).
        assert!(
            m.last_sync_ms(tmp.path()).is_none(),
            "non-leader must NOT pull (single-puller contract)"
        );
        assert!(tmp
            .path()
            .join("yum.repos.d/mackes-mirror-magic-mesh.repo")
            .exists());
    }

    #[tokio::test]
    async fn forged_or_legacy_marker_does_not_pull() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("role.host");
        std::fs::write(&marker, "role:host\n").unwrap();
        let mut w = worker(tmp.path(), marker);
        w.tick().await;
        assert!(mirrors::core_pack()[0].last_sync_ms(tmp.path()).is_none());
    }
}

#![cfg(feature = "async-services")]
//! PLANES-24 W63 — the scheduled one-puller mirror sync worker.
//!
//! Ties the `mirrors` building blocks into a periodic daemon:
//!
//!   * **Every node** self-serves from its local `file://` mount (upstream as
//!     fallback, W62), but a restarted worker first retracts the retained repo
//!     advertisement. It republishes only after this process either completes
//!     an authoritative leader sync or observes a strictly forward generation
//!     replicated from the current puller.
//!   * **Only the leader** runs the actual pull
//!     ([`crate::mirrors::sync_mirror`]: `dnf reposync` → `createrepo_c` →
//!     stamp `.last-sync`). Syncthing replicates the result to every other node,
//!     so the fleet mirrors GitHub exactly once per tick — the "one-puller"
//!     contract (W63). Leadership is proxied by the role-host marker, the same
//!     signal [`super::netdata_aggregator`] uses.
//!
//! A bad tick (network down, `dnf`/`createrepo_c` missing) is logged and
//! swallowed; the next tick retries.

use std::collections::{BTreeMap, BTreeSet};
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
    startup_prepared: bool,
    retained_generations: BTreeMap<String, Option<u64>>,
    ready_mirrors: BTreeSet<String>,
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
            startup_prepared: false,
            retained_generations: BTreeMap::new(),
            ready_mirrors: BTreeSet::new(),
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

    /// Retract prior-process repo advertisements and remember their generations.
    /// A failure leaves startup pending so the next sweep retries rather than
    /// treating retained state as current.
    fn prepare_startup(&mut self) -> bool {
        if self.startup_prepared {
            return true;
        }

        let mirrors = mirrors::load_mirrors(&self.workgroup_root);
        let mut retained_generations = BTreeMap::new();
        for mirror in &mirrors {
            retained_generations.insert(
                mirror.name.clone(),
                mirror.last_sync_ms(&self.workgroup_root),
            );
            let repo_path = self
                .repo_dir
                .join(format!("mackes-mirror-{}.repo", mirror.name));
            if let Err(error) = std::fs::remove_file(&repo_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        mirror = %mirror.name,
                        path = %repo_path.display(),
                        error = %error,
                        "mirror-syncd: retained repo retraction failed; startup remains pending"
                    );
                    return false;
                }
            }
        }
        self.retained_generations = retained_generations;
        self.ready_mirrors.clear();
        self.startup_prepared = true;
        true
    }

    /// One sweep: when leader, pull each enabled mirror; otherwise wait for a
    /// strictly forward replicated sync generation. Only then publish the local
    /// `.repo` advertisement. Per-mirror errors are logged + swallowed.
    async fn tick(&mut self) {
        if !self.prepare_startup() {
            return;
        }
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
            let current_generation = m.last_sync_ms(&self.workgroup_root);
            let retained_generation = self
                .retained_generations
                .entry(m.name.clone())
                .or_insert(current_generation);
            let was_ready = self.ready_mirrors.contains(&m.name);
            let mut admitted_generation = None;
            let mut generation_verified = was_ready
                && matches!(
                    (*retained_generation, current_generation),
                    (Some(admitted), Some(current)) if current >= admitted
                );

            // Single puller: only the current authoritative leader fetches from
            // upstream. A successful pull is sufficient current-process proof.
            if is_leader {
                match mirrors::sync_mirror(&*self.runner, m, &self.workgroup_root, now_ms) {
                    Ok(r) => {
                        generation_verified = true;
                        admitted_generation = Some(r.synced_at_ms);
                        tracing::info!(
                            mirror = %r.name, rpms = r.rpm_count, at_ms = r.synced_at_ms,
                            "mirror-syncd: pulled + indexed"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(mirror = %m.name, error = %e, "mirror-syncd: sync failed");
                    }
                }
            } else if !generation_verified {
                generation_verified = match (*retained_generation, current_generation) {
                    (None, Some(_)) => true,
                    (Some(previous), Some(current)) => current > previous,
                    _ => false,
                };
                admitted_generation = generation_verified.then_some(current_generation).flatten();
            }

            if generation_verified {
                if let Err(e) = mirrors::write_dnf_repo(m, &self.workgroup_root, &self.repo_dir) {
                    tracing::warn!(mirror = %m.name, error = %e, "mirror-syncd: .repo write failed");
                } else {
                    if let Some(generation) = admitted_generation.or(current_generation) {
                        *retained_generation = Some(retained_generation.map_or(
                            generation,
                            |previous| previous.max(generation),
                        ));
                    }
                    self.ready_mirrors.insert(m.name.clone());
                }
            } else if was_ready {
                self.ready_mirrors.remove(&m.name);
                let repo_path = self
                    .repo_dir
                    .join(format!("mackes-mirror-{}.repo", m.name));
                if let Err(error) = std::fs::remove_file(&repo_path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            mirror = %m.name,
                            path = %repo_path.display(),
                            error = %error,
                            "mirror-syncd: regressed mirror generation could not be retracted"
                        );
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
        let mut interval = tokio::time::interval(self.tick_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.wait() => return Ok(()),
                _ = interval.tick() => {
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

    struct FailingRunner;
    impl MirrorSyncRunner for FailingRunner {
        fn reposync(&self, _name: &str, _upstream: &str, _dest: &Path) -> Result<u32, String> {
            Err("injected upstream failure".into())
        }
        fn createrepo(&self, _dir: &Path) -> Result<(), String> {
            panic!("createrepo must not run after reposync failure")
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
        // Retained or absent state is not advertised by a restarted process.
        assert!(
            m.last_sync_ms(tmp.path()).is_none(),
            "non-leader must NOT pull (single-puller contract)"
        );
        assert!(!tmp
            .path()
            .join("yum.repos.d/mackes-mirror-magic-mesh.repo")
            .exists());

        // A strictly forward generation replicated from the live puller makes
        // the local mirror eligible for self-service.
        std::fs::create_dir_all(m.local_dir(tmp.path())).unwrap();
        std::fs::write(m.local_dir(tmp.path()).join(".last-sync"), "2").unwrap();
        w.tick().await;
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

    #[tokio::test]
    async fn restart_does_not_republish_retained_generation_after_sync_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("role.host");
        let mirror = &mirrors::core_pack()[0];
        std::fs::create_dir_all(mirror.local_dir(tmp.path())).unwrap();
        std::fs::write(mirror.local_dir(tmp.path()).join(".last-sync"), "1").unwrap();
        mirrors::write_dnf_repo(mirror, tmp.path(), &tmp.path().join("yum.repos.d"))
            .expect("retained repo advertisement");
        crate::leader::force_take(&tmp.path().join(".mackesd-leader.lock"), "peer:test").unwrap();
        crate::workers::nebula_supervisor::write_role_marker(&marker, "peer:test").unwrap();

        let mut w = worker(tmp.path(), marker).with_runner(Box::new(FailingRunner));
        w.tick().await;

        assert_eq!(mirror.last_sync_ms(tmp.path()), Some(1));
        assert!(
            !tmp.path()
                .join("yum.repos.d/mackes-mirror-magic-mesh.repo")
                .exists(),
            "a failed current-process sync must not republish retained readiness"
        );

        w.runner = Box::new(MockRunner);
        w.tick().await;
        assert!(tmp
            .path()
            .join("yum.repos.d/mackes-mirror-magic-mesh.repo")
            .exists());
        assert_ne!(mirror.last_sync_ms(tmp.path()), Some(1));
    }

    #[tokio::test]
    async fn replicated_generation_rollback_retracts_repo_until_corrected_forward_return() {
        let tmp = tempfile::tempdir().unwrap();
        let mirror = &mirrors::core_pack()[0];
        std::fs::create_dir_all(mirror.local_dir(tmp.path())).unwrap();
        let mut w = worker(tmp.path(), tmp.path().join("role.host"));

        w.tick().await;
        std::fs::write(mirror.local_dir(tmp.path()).join(".last-sync"), "2").unwrap();
        w.tick().await;
        let repo = tmp
            .path()
            .join("yum.repos.d/mackes-mirror-magic-mesh.repo");
        assert!(repo.exists(), "forward peer generation must be admitted");

        std::fs::write(mirror.local_dir(tmp.path()).join(".last-sync"), "1").unwrap();
        w.tick().await;
        assert!(
            !repo.exists(),
            "a rolled-back replicated generation must retract the advertised mirror"
        );

        std::fs::write(mirror.local_dir(tmp.path()).join(".last-sync"), "3").unwrap();
        w.tick().await;
        assert!(
            repo.exists(),
            "a strictly corrected-forward peer generation must restore the mirror"
        );
    }
}

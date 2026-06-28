//! DATACENTER-20 — the passive `dc_promote` promotion-tracker worker.
//!
//! A read-only companion to [`super::dc_auditor`] / [`super::dc_jobs`]: where
//! those observe the `action/dc/*` request lanes, this worker tracks the
//! **version running at each promotion stage** (Build → Eagle → DO) and
//! publishes one record per stage to `event/dc/promote/<stage>`, so the
//! Workbench Datacenter plane can render the promotion matrix. It touches no
//! action handlers — a pure publisher of build/promotion state.
//!
//! Design (mirrors `dc_auditor`): the *brain* ([`DcPromote`]) is a pure,
//! deduped sieve — fed `(stage, version)` it returns a [`PromoteRecord`] ONLY
//! when that stage's version changed since the last publish, so a re-poll of an
//! unchanged matrix never re-publishes. The worker is thin I/O around it: each
//! tick it determines the farm/build version (newest RPM in
//! `$HOME/mcnf-release-artifacts/*.rpm`, falling back to `git describe`), and
//! emits the `build` stage plus best-effort `eagle` + `do` stage records whose
//! version is the honest `"unknown"` (those hosts aren't reachable from here yet
//! — we never fabricate a version). It is **leader-gated** so a multi-node mesh
//! publishes the matrix once.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 30 s (the build/promotion matrix changes slowly).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// The honest placeholder for a stage whose live version we can't observe yet.
pub const UNKNOWN_VERSION: &str = "unknown";

/// Bus topic a promotion record for `stage` is published to:
/// `event/dc/promote/<stage>`.
#[must_use]
pub fn promote_topic(stage: &str) -> String {
    format!("event/dc/promote/{stage}")
}

/// Parse the `magic-mesh-<version>` token out of an RPM filename, e.g.
/// `"magic-mesh-11.0.1-1.x86_64.rpm"` → `Some("11.0.1-1")`. Returns `None` for
/// any name that isn't a `magic-mesh-*` RPM. The version is everything between
/// the `magic-mesh-` prefix and the trailing `.<arch>.rpm` (arch + extension).
#[must_use]
pub fn rpm_version_from_filename(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".rpm")?;
    let rest = stem.strip_prefix("magic-mesh-")?;
    // Strip the trailing `.<arch>` (e.g. `.x86_64`, `.noarch`) so we keep just
    // the `<version>-<release>` token.
    let version = match rest.rsplit_once('.') {
        Some((v, _arch)) => v,
        None => rest,
    };
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

/// One promotion-stage record the tracker decided to emit (a stage's version or
/// status changed since the last publish).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PromoteRecord {
    /// The promotion stage: `"build"`, `"eagle"`, or `"do"`.
    pub stage: String,
    /// The version running at this stage (`"unknown"` when not yet observable).
    pub version: String,
    /// The stage status: `"ready"` (a real version) or `"pending"`.
    pub status: &'static str,
}

impl PromoteRecord {
    /// Bus topic this record publishes to: `event/dc/promote/<stage>`.
    #[must_use]
    pub fn topic(&self) -> String {
        promote_topic(&self.stage)
    }

    /// JSON body for `mde-bus publish`.
    #[must_use]
    pub fn body(&self) -> String {
        serde_json::json!({
            "stage": self.stage,
            "version": self.version,
            "status": self.status,
        })
        .to_string()
    }
}

/// Pure promotion core: tracks the last-published `(version, status)` per stage
/// and returns ONLY the changes (or first sight). A re-poll that observes the same
/// version+status for a stage emits nothing, so the Bus never sees a duplicate for
/// an unchanged matrix cell. Also dedups the DATACENTER-20 auto-promote
/// eligibility hint ([`observe_auto`](Self::observe_auto)).
#[derive(Default)]
pub struct DcPromote {
    last: BTreeMap<String, (String, &'static str)>,
    /// Last-published auto-promote eligibility `(build→eagle, eagle→do, armed)`.
    last_auto: Option<(bool, bool, bool)>,
}

impl DcPromote {
    /// Fresh sieve with no observed stages.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one stage's current `version` + `status`. Returns a
    /// [`PromoteRecord`] when the pair differs from the last one published for
    /// this stage (or on first sight), and `None` when unchanged. Advances
    /// internal state on a change.
    pub fn observe(
        &mut self,
        stage: &str,
        version: &str,
        status: &'static str,
    ) -> Option<PromoteRecord> {
        if self.last.get(stage) == Some(&(version.to_string(), status)) {
            return None;
        }
        self.last
            .insert(stage.to_string(), (version.to_string(), status));
        Some(PromoteRecord {
            stage: stage.to_string(),
            version: version.to_string(),
            status,
        })
    }

    /// Observe the auto-promote eligibility triple. Returns `Some(triple)` only
    /// when it changed since the last publish (so the `event/dc/promote/auto` lane
    /// doesn't spam an unchanged hint), `None` otherwise.
    pub fn observe_auto(
        &mut self,
        build_to_eagle: bool,
        eagle_to_do: bool,
        armed: bool,
    ) -> Option<(bool, bool, bool)> {
        let t = (build_to_eagle, eagle_to_do, armed);
        if self.last_auto == Some(t) {
            return None;
        }
        self.last_auto = Some(t);
        Some(t)
    }
}

/// Parse `rpm -q magic-mesh` output into the installed `<version>-<release>`
/// token. PURE. `"magic-mesh-11.0.15-1.x86_64"` → `Some("11.0.15-1")`; the
/// `"package magic-mesh is not installed"` line or any non-`magic-mesh-*` output →
/// `None` (honest: never a fabricated version).
#[must_use]
pub fn parse_installed_version(rpm_q: &str) -> Option<String> {
    let line = rpm_q.lines().next()?.trim();
    if line.is_empty() || line.contains("not installed") {
        return None;
    }
    let rest = line.strip_prefix("magic-mesh-")?;
    // Strip the trailing `.<arch>` (e.g. `.x86_64`, `.noarch`).
    let version = rest.rsplit_once('.').map_or(rest, |(v, _arch)| v);
    (!version.is_empty()).then(|| version.to_string())
}

/// Whether a promotion from `from_version` to `to_version` is *available*. PURE.
/// True only when the upstream stage holds a real (non-`unknown`, non-empty)
/// version and the downstream stage is not already at it. Used for both the
/// Build→Eagle and Eagle→DO hops (the DO hop is additionally prod-arm-gated by the
/// caller).
#[must_use]
pub fn auto_promote_eligible(from_version: Option<&str>, to_version: Option<&str>) -> bool {
    match from_version {
        Some(f) if !f.is_empty() && f != UNKNOWN_VERSION => to_version != Some(f),
        _ => false,
    }
}

// ---- thin I/O: determine the build version, emit the matrix via the Bus ----

/// Publish one promotion record onto the Bus (best-effort, fire-and-reap — same
/// lane shape as the dc_auditor's records).
fn publish(rec: &PromoteRecord) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &rec.topic(), "--body-flag", &rec.body()]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// The newest `magic-mesh-*.rpm` version under `dir`, parsed from the filename.
/// "Newest" = the highest version string by lexical order over the parsed
/// version tokens (release artifacts are monotonic). `None` when the dir is
/// absent/empty or holds no parseable `magic-mesh-*` RPM.
fn newest_rpm_version(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut versions: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(v) = rpm_version_from_filename(&name) {
            versions.push(v);
        }
    }
    versions.into_iter().max()
}

/// `git -C <repo> describe --tags --always` as a fallback build-version source.
/// `None` when git fails or the repo path doesn't resolve.
fn git_describe(repo: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["describe", "--tags", "--always"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Resolve the current farm/build version: newest release RPM, else
/// `git describe`, else `None` (caller substitutes the honest `"unknown"`).
fn resolve_build_version(home: Option<&Path>, repo: Option<&Path>) -> Option<String> {
    if let Some(home) = home {
        let artifacts = home.join("mcnf-release-artifacts");
        if let Some(v) = newest_rpm_version(&artifacts) {
            return Some(v);
        }
    }
    repo.and_then(git_describe)
}

/// Whether a host token (env-supplied overlay IP / hostname) is safe to pass to
/// ssh — `[A-Za-z0-9.-]`, non-empty, ≤ 64 chars. PURE.
fn valid_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Read the live installed `magic-mesh` version from `host` over the mesh key
/// (`rpm -q magic-mesh`), parsed by [`parse_installed_version`]. `None` on any
/// SSH/parse failure (degrades to the honest `"unknown"`).
fn read_remote_version(key: &str, host: &str) -> Option<String> {
    let o = std::process::Command::new("ssh")
        .args([
            "-i",
            key,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &format!("root@{host}"),
            "rpm -q magic-mesh",
        ])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    parse_installed_version(&String::from_utf8_lossy(&o.stdout))
}

/// Resolve a promotion stage's live version from the first host in `env_var`
/// (comma-separated; the lighthouses share a version). `None` when the env is
/// unset/empty, the host is malformed, or the read fails — the caller substitutes
/// the honest `"unknown"`.
fn resolve_stage_version(env_var: &str, key: &str) -> Option<String> {
    let val = std::env::var(env_var).ok()?;
    let host = val.split(',').map(str::trim).find(|s| !s.is_empty())?;
    if !valid_host(host) {
        return None;
    }
    read_remote_version(key, host)
}

/// Publish the auto-promote eligibility hint onto `event/dc/promote/auto`.
fn publish_auto(build_to_eagle: bool, eagle_to_do: bool, armed: bool) {
    let body = serde_json::json!({
        "build_to_eagle": build_to_eagle,
        "eagle_to_do": eagle_to_do,
        "armed": armed,
    })
    .to_string();
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", "event/dc/promote/auto", "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// One poll pass: resolve the build version + the **live** eagle/lighthouse
/// versions (DATACENTER-20 — `MCNF_EAGLE_HOST` / `MCNF_DO_HOSTS` over the mesh key,
/// replacing the hardcoded `"unknown"`), feed all three stages through the dedup
/// core, then publish the prod-arm-gated auto-promote eligibility hint. A stage
/// whose host isn't configured/reachable stays the honest `"unknown"`/`pending`.
fn poll_and_publish(
    core: &mut DcPromote,
    home: Option<&Path>,
    repo: Option<&Path>,
    workgroup_root: &Path,
) {
    let build_version =
        resolve_build_version(home, repo).unwrap_or_else(|| UNKNOWN_VERSION.to_string());
    if let Some(rec) = core.observe("build", &build_version, "ready") {
        publish(&rec);
    }

    // Live stage version reads over the mesh key.
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let eagle_version = resolve_stage_version("MCNF_EAGLE_HOST", &key);
    let do_version = resolve_stage_version("MCNF_DO_HOSTS", &key);
    let eagle_status = if eagle_version.is_some() {
        "ready"
    } else {
        "pending"
    };
    if let Some(rec) = core.observe(
        "eagle",
        eagle_version.as_deref().unwrap_or(UNKNOWN_VERSION),
        eagle_status,
    ) {
        publish(&rec);
    }
    let do_status = if do_version.is_some() {
        "ready"
    } else {
        "pending"
    };
    if let Some(rec) = core.observe(
        "do",
        do_version.as_deref().unwrap_or(UNKNOWN_VERSION),
        do_status,
    ) {
        publish(&rec);
    }

    // DATACENTER-20 — auto-promote eligibility. Build→Eagle is available when the
    // build is newer than eagle; the Eagle→DO hop is additionally gated by the
    // promote prod-arm switch (armed = auto, disarmed = held). Surfaced as a
    // deduped Bus hint; the actual deploy is enacted via `promote-now` / the
    // promotion machinery.
    let armed = crate::ipc::dc_common::read_arm(
        &crate::ipc::dc_common::dc_state_dir(workgroup_root),
        "promote",
    );
    let build_to_eagle = auto_promote_eligible(Some(&build_version), eagle_version.as_deref());
    let eagle_to_do =
        armed && auto_promote_eligible(eagle_version.as_deref(), do_version.as_deref());
    if let Some((bve, edo, a)) = core.observe_auto(build_to_eagle, eagle_to_do, armed) {
        publish_auto(bve, edo, a);
    }
}

fn default_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// The supervised worker. Leader-gated (only the elected node publishes the
/// matrix, so a multi-node mesh doesn't multi-publish) and best-effort.
pub struct DcPromoteWorker {
    core: DcPromote,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    workgroup_root: PathBuf,
    home_override: Option<PathBuf>,
    repo: PathBuf,
}

impl DcPromoteWorker {
    /// Construct with production defaults (30 s tick, the shared leader lock
    /// under `workgroup_root`, the `$HOME` artifacts dir + the in-tree repo for
    /// the `git describe` fallback).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            core: DcPromote::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            workgroup_root,
            node_id,
            home_override: None,
            repo: PathBuf::from("."),
        }
    }

    /// Override the `$HOME` directory (the artifacts dir is resolved under it).
    /// Used in tests.
    #[must_use]
    pub fn with_home(mut self, p: PathBuf) -> Self {
        self.home_override = Some(p);
        self
    }

    /// Override the git repo path used for the `git describe` fallback. Used in
    /// tests.
    #[must_use]
    pub fn with_repo(mut self, p: PathBuf) -> Self {
        self.repo = p;
        self
    }

    /// Only the directory leader publishes (no-fixed-center: any eligible node
    /// can be it, the elected one publishes). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }
}

#[async_trait::async_trait]
impl Worker for DcPromoteWorker {
    fn name(&self) -> &'static str {
        "dc_promote"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let home = self.home_override.clone().or_else(default_home);
        let repo = self.repo.clone();
        let workgroup_root = self.workgroup_root.clone();
        loop {
            if self.is_leader() {
                poll_and_publish(
                    &mut self.core,
                    home.as_deref(),
                    Some(repo.as_path()),
                    &workgroup_root,
                );
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_topic_formats_under_event_dc_promote() {
        assert_eq!(promote_topic("build"), "event/dc/promote/build");
        assert_eq!(promote_topic("eagle"), "event/dc/promote/eagle");
        assert_eq!(promote_topic("do"), "event/dc/promote/do");
    }

    #[test]
    fn rpm_version_from_filename_parses_valid_and_rejects_others() {
        // A canonical release RPM → just the `<version>-<release>` token.
        assert_eq!(
            rpm_version_from_filename("magic-mesh-11.0.1-1.x86_64.rpm").as_deref(),
            Some("11.0.1-1")
        );
        // noarch arch suffix also stripped.
        assert_eq!(
            rpm_version_from_filename("magic-mesh-11.0-2.noarch.rpm").as_deref(),
            Some("11.0-2")
        );
        // A non-magic-mesh RPM → None.
        assert_eq!(rpm_version_from_filename("some-other-1.0.x86_64.rpm"), None);
        // Not an RPM at all → None.
        assert_eq!(
            rpm_version_from_filename("magic-mesh-11.0.1-1.tar.gz"),
            None
        );
        // The prefix with no version body → None.
        assert_eq!(rpm_version_from_filename("magic-mesh-.x86_64.rpm"), None);
    }

    #[test]
    fn observe_emits_on_change_and_dedups_unchanged() {
        let mut p = DcPromote::new();
        // First sight of a stage → a record on the right topic.
        let rec = p
            .observe("build", "11.0.1-1", "ready")
            .expect("first sight emits");
        assert_eq!(rec.stage, "build");
        assert_eq!(rec.version, "11.0.1-1");
        assert_eq!(rec.status, "ready");
        assert_eq!(rec.topic(), "event/dc/promote/build");
        let body = rec.body();
        assert!(body.contains(r#""stage":"build""#));
        assert!(body.contains(r#""version":"11.0.1-1""#));
        assert!(body.contains(r#""status":"ready""#));
        // Same (version,status) → no re-emit.
        assert!(p.observe("build", "11.0.1-1", "ready").is_none());
        // A new build version → a fresh record.
        let rec2 = p
            .observe("build", "11.0.2-1", "ready")
            .expect("version change emits");
        assert_eq!(rec2.version, "11.0.2-1");
    }

    #[test]
    fn observe_tracks_stages_independently() {
        let mut p = DcPromote::new();
        // Each stage gets its own first-sight record.
        assert!(p.observe("build", "11.0.1-1", "ready").is_some());
        assert!(p.observe("eagle", UNKNOWN_VERSION, "pending").is_some());
        assert!(p.observe("do", UNKNOWN_VERSION, "pending").is_some());
        // None re-emit unchanged.
        assert!(p.observe("build", "11.0.1-1", "ready").is_none());
        assert!(p.observe("eagle", UNKNOWN_VERSION, "pending").is_none());
        assert!(p.observe("do", UNKNOWN_VERSION, "pending").is_none());
        // The eagle stage filling in a real version is one independent change.
        let rec = p
            .observe("eagle", "11.0.0-1", "ready")
            .expect("eagle stage transition emits");
        assert_eq!(rec.stage, "eagle");
        assert_eq!(rec.version, "11.0.0-1");
    }

    #[test]
    fn newest_rpm_version_picks_highest_and_skips_non_matching() {
        let dir = std::env::temp_dir().join(format!("dc_promote_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "magic-mesh-11.0.1-1.x86_64.rpm",
            "magic-mesh-11.0.2-1.x86_64.rpm",
            "unrelated-3.2.1.x86_64.rpm",
            "README.txt",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(newest_rpm_version(&dir).as_deref(), Some("11.0.2-1"));
        // An absent dir → None.
        assert_eq!(newest_rpm_version(&dir.join("does-not-exist")), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- DATACENTER-20: live version reads + auto-promote ----------------------

    #[test]
    fn parse_installed_version_reads_rpm_q() {
        assert_eq!(
            parse_installed_version("magic-mesh-11.0.15-1.x86_64").as_deref(),
            Some("11.0.15-1")
        );
        assert_eq!(
            parse_installed_version("magic-mesh-11.0-2.noarch\n").as_deref(),
            Some("11.0-2")
        );
        // not installed / wrong package / empty → None (honest)
        assert_eq!(
            parse_installed_version("package magic-mesh is not installed"),
            None
        );
        assert_eq!(parse_installed_version("some-other-1.0.x86_64"), None);
        assert_eq!(parse_installed_version(""), None);
    }

    #[test]
    fn auto_promote_eligible_on_version_drift() {
        // Build ahead of eagle → eligible.
        assert!(auto_promote_eligible(Some("11.0.2-1"), Some("11.0.1-1")));
        // Build == eagle → not eligible.
        assert!(!auto_promote_eligible(Some("11.0.2-1"), Some("11.0.2-1")));
        // Eagle not yet observed (None) but build real → eligible (something to do).
        assert!(auto_promote_eligible(Some("11.0.2-1"), None));
        // Upstream unknown/empty → never eligible (no real version to push).
        assert!(!auto_promote_eligible(
            Some(UNKNOWN_VERSION),
            Some("11.0.1-1")
        ));
        assert!(!auto_promote_eligible(Some(""), None));
        assert!(!auto_promote_eligible(None, Some("11.0.1-1")));
    }

    #[test]
    fn observe_auto_dedups_unchanged_triple() {
        let mut p = DcPromote::new();
        // First sight → emits.
        assert_eq!(
            p.observe_auto(true, false, false),
            Some((true, false, false))
        );
        // Same triple → no re-emit.
        assert!(p.observe_auto(true, false, false).is_none());
        // A change (armed flips) → emits.
        assert_eq!(p.observe_auto(true, true, true), Some((true, true, true)));
    }
}

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
//! emits the `build` stage plus the downstream `eagle` + `do` stage records. It
//! is **leader-gated** so a multi-node mesh publishes the matrix once.
//!
//! DATACENTER-20 (auto-promote half): the worker no longer only OBSERVES
//! versions — it also reads the farm's L1–L3 internal-test verdicts off the Bus
//! (`event/test/{install,feature,stability}`, the same topics
//! `panels/build_farm.rs` renders) and, via the pure [`decide_promote`] core,
//! ADVANCES the pipeline: when Build + the test tiers are GREEN it marks the
//! `eagle` stage `ready` (auto-advanced) and then gates the `do` stage on the
//! **prod-arm** master switch — armed ⇒ `armed` (auto-promote to DO), disarmed
//! ⇒ `queued` (held until the operator arms it). The arm state is a small
//! persisted file (mirroring the Workbench's saved-views config) that the
//! Datacenter panel's prod-arm toggle writes and this worker reads, so the gate
//! survives a restart and is shared between the GUI and the leader.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 30 s (the build/promotion matrix changes slowly).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// The honest placeholder for a stage whose live version we can't observe yet.
pub const UNKNOWN_VERSION: &str = "unknown";

/// Stage status: a real, observed version that is GREEN and serving.
pub const STATUS_READY: &str = "ready";
/// Stage status: not yet advanced (upstream not green, or no version yet).
pub const STATUS_PENDING: &str = "pending";
/// Stage status: auto-advanced off a green upstream (eagle once Build is green).
pub const STATUS_AUTO: &str = "auto";
/// Stage status (DO only): green + prod-arm ARMED ⇒ auto-promotes to DO.
pub const STATUS_ARMED: &str = "armed";
/// Stage status (DO only): green but prod-arm DISARMED ⇒ the promotion is HELD.
pub const STATUS_QUEUED: &str = "queued";

/// The three internal test tiers (L1/L2/L3) the farm orchestrator publishes to
/// `event/test/{install,feature,stability}` — the green signal the auto-promote
/// reads. Mirrors `panels/build_farm.rs::TEST_TIERS` (kept here so the mesh-side
/// worker doesn't reach across the mesh/desktop boundary into the GUI crate).
pub const TEST_TIERS: [&str; 3] = ["install", "feature", "stability"];

/// Bus topic the L1–L3 test-tier verdict for `tier` is published to:
/// `event/test/<tier>`.
#[must_use]
pub fn test_tier_topic(tier: &str) -> String {
    format!("event/test/{tier}")
}

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
/// and returns a record ONLY when a stage's pair changed (or on first sight). A
/// re-poll that observes the same version+status for a stage emits nothing, so
/// the Bus never sees a duplicate for an unchanged matrix cell.
#[derive(Default)]
pub struct DcPromote {
    last: BTreeMap<String, (String, &'static str)>,
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
}

// ---- DATACENTER-20 auto-promote: pure green-detection + gate decision ----

/// One internal-test tier's verdict as the farm orchestrator last published it
/// to `event/test/<tier>`. The auto-promote only advances on `Green`; anything
/// else (failed, still running, never run) HOLDS the pipeline — fail-safe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TierVerdict {
    /// The tier passed (`outcome:"pass"` / `overall:"green"`).
    Green,
    /// The tier failed (any non-pass, non-empty verdict).
    Red,
    /// Published but with no recognised verdict yet (running / unknown).
    Running,
    /// No `event/test/<tier>` body on the Bus at all.
    NoRuns,
}

/// Map an `event/test/<tier>` body's verdict to a [`TierVerdict`]. The body
/// carries `outcome` (pass|fail) or `overall` (green|RED) — mirrors
/// `panels/build_farm.rs::parse_tier_outcome` so the GUI badge and the
/// auto-promote agree on what "green" means. Pure. A body absent from the Bus is
/// handled by the caller as [`TierVerdict::NoRuns`].
#[must_use]
pub fn parse_tier_verdict(body: &str) -> TierVerdict {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return TierVerdict::Running;
    };
    let raw = v
        .get("outcome")
        .or_else(|| v.get("overall"))
        .and_then(|o| o.as_str())
        .unwrap_or("");
    match raw {
        "pass" | "green" => TierVerdict::Green,
        "" => TierVerdict::Running,
        // Any other recognised-or-not verdict is a non-pass → fail-safe Red.
        _ => TierVerdict::Red,
    }
}

/// Whether the Build→Eagle gate is GREEN: a real (non-`unknown`) build version
/// IS present AND every test tier reported [`TierVerdict::Green`]. A missing
/// tier ([`TierVerdict::NoRuns`]), a failure, or an unresolved build version all
/// HOLD the promotion — the gate fails closed. Pure + testable.
#[must_use]
pub fn build_eagle_green(build_version: &str, tiers: &[TierVerdict]) -> bool {
    if build_version.is_empty() || build_version == UNKNOWN_VERSION {
        return false;
    }
    !tiers.is_empty() && tiers.iter().all(|t| *t == TierVerdict::Green)
}

/// The per-stage status the auto-promote decides for the downstream stages,
/// given the green gate + the prod-arm switch. The `build` stage's status is
/// always [`STATUS_READY`] off its own resolved version; this decides `eagle`
/// and `do`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PromoteDecision {
    /// The `eagle` stage status: [`STATUS_AUTO`] when Build is green (the
    /// promotion auto-advances to Eagle), else [`STATUS_PENDING`].
    pub eagle: &'static str,
    /// The `do` stage status: gated on prod-arm. Green + armed ⇒ [`STATUS_ARMED`]
    /// (auto-promote to DO); green + disarmed ⇒ [`STATUS_QUEUED`] (held); not
    /// green ⇒ [`STATUS_PENDING`].
    pub do_stage: &'static str,
}

/// The auto-promote decision: given whether the Build→Eagle gate is green and
/// whether the prod-arm master switch is armed, decide the `eagle` + `do` stage
/// statuses. PURE + fails closed:
/// - not green ⇒ eagle `pending`, do `pending` (nothing advances);
/// - green ⇒ eagle `auto` (advances to Eagle), and the DO step is gated:
///   - armed ⇒ do `armed` (auto-promote to DO on green),
///   - disarmed ⇒ do `queued` (the promotion is HELD until the operator arms it).
///
/// This mirrors the DATACENTER-15 prod-arm gate (`prod_arm_allows`): a prod (DO)
/// promotion never auto-fires unless explicitly armed.
#[must_use]
pub fn decide_promote(green: bool, prod_armed: bool) -> PromoteDecision {
    if !green {
        return PromoteDecision {
            eagle: STATUS_PENDING,
            do_stage: STATUS_PENDING,
        };
    }
    PromoteDecision {
        eagle: STATUS_AUTO,
        do_stage: if prod_armed {
            STATUS_ARMED
        } else {
            STATUS_QUEUED
        },
    }
}

// ---- DATACENTER-20 prod-arm: persisted master switch shared with the GUI ----

/// The local config-file path the **prod-arm master switch** persists to. Mirrors
/// the Workbench's saved-views convention (`$XDG_CONFIG_HOME/mde/…`, falling back
/// to `$HOME/.config/mde/…`) so the Datacenter panel's prod-arm toggle and this
/// leader-side worker read/write the SAME file — the gate survives a restart and
/// is shared between the GUI and the worker. `None` only in a degenerate headless
/// env where neither var is set (then the gate is session-only, defaulting
/// disarmed = fail-closed).
#[must_use]
pub fn prod_arm_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("mde").join("dc-prod-arm.json"))
}

/// Read the persisted prod-arm state from `path`. The file is a tiny
/// `{"armed":true|false}` record. A missing/empty/corrupt file ⇒ `false`
/// (disarmed) — the gate fails closed, so a fresh install or a damaged config
/// never silently auto-promotes to prod. Pure over the filesystem + testable.
#[must_use]
pub fn load_prod_arm(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("armed").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Persist the prod-arm state to `path`, creating the `mde/` config dir if
/// needed. Returns the error text on failure (the caller surfaces it). Mirrors
/// `save_saved_views`.
pub fn save_prod_arm(path: &Path, armed: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::json!({ "armed": armed }).to_string();
    std::fs::write(path, json.as_bytes()).map_err(|e| e.to_string())
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

/// Read the latest L1–L3 test-tier verdict off the Bus — one [`TierVerdict`] per
/// [`TEST_TIERS`] entry, in L1→L3 order. A tier with no `event/test/<tier>`
/// message yet is [`TierVerdict::NoRuns`]; the last message's body decides the
/// rest. Mirrors `panels/build_farm.rs::project_tiers`' "latest body wins"
/// idiom (`list_since(topic, None)` returns the topic's messages ascending, so
/// the last is newest).
fn read_tier_verdicts(persist: &Persist) -> Vec<TierVerdict> {
    TEST_TIERS
        .iter()
        .map(|tier| {
            let topic = test_tier_topic(tier);
            match persist.list_since(&topic, None) {
                Ok(msgs) => match msgs.last() {
                    Some(msg) => parse_tier_verdict(msg.body.as_deref().unwrap_or("")),
                    None => TierVerdict::NoRuns,
                },
                Err(e) => {
                    tracing::debug!(topic = %topic, error = %e, "dc_promote: tier list_since failed");
                    TierVerdict::NoRuns
                }
            }
        })
        .collect()
}

/// One poll pass: resolve the build version, read the L1–L3 green signal + the
/// prod-arm switch, and feed the three stages through the dedup core, publishing
/// the records that survive.
///
/// The `build` stage carries the resolved version (or `"unknown"`) as `ready`.
/// The `eagle` + `do` stages are AUTO-ADVANCED by [`decide_promote`]: when
/// Build + the test tiers are green, `eagle` flips to `auto` and the `do` step
/// is gated on the prod-arm switch (`armed` ⇒ auto-promote, `queued` ⇒ held).
/// Not green ⇒ both stay `pending`. The downstream stages carry the build
/// version they would promote (so the strip shows what's being advanced) when
/// the gate is green, and the honest `"unknown"` while pending.
fn poll_and_publish(
    core: &mut DcPromote,
    home: Option<&Path>,
    repo: Option<&Path>,
    tiers: &[TierVerdict],
    prod_armed: bool,
) {
    let build_version =
        resolve_build_version(home, repo).unwrap_or_else(|| UNKNOWN_VERSION.to_string());
    if let Some(rec) = core.observe("build", &build_version, STATUS_READY) {
        publish(&rec);
    }

    let green = build_eagle_green(&build_version, tiers);
    let decision = decide_promote(green, prod_armed);
    // A green gate promotes the resolved build version downstream; while pending
    // the downstream stages have no honest version to show yet.
    let downstream_version = if green {
        build_version.as_str()
    } else {
        UNKNOWN_VERSION
    };
    if let Some(rec) = core.observe("eagle", downstream_version, decision.eagle) {
        publish(&rec);
    }
    if let Some(rec) = core.observe("do", downstream_version, decision.do_stage) {
        publish(&rec);
    }
}

fn default_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// The default Bus persist root (where `event/test/*` lives). Mirrors
/// `dc_auditor::default_bus_root`.
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The supervised worker. Leader-gated (only the elected node publishes the
/// matrix, so a multi-node mesh doesn't multi-publish) and best-effort.
pub struct DcPromoteWorker {
    core: DcPromote,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    home_override: Option<PathBuf>,
    repo: PathBuf,
    bus_root_override: Option<PathBuf>,
    prod_arm_override: Option<PathBuf>,
}

impl DcPromoteWorker {
    /// Construct with production defaults (30 s tick, the shared leader lock
    /// under `workgroup_root`, the `$HOME` artifacts dir + the in-tree repo for
    /// the `git describe` fallback, the default Bus root for the L1–L3 green
    /// signal, and the default prod-arm config path).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            core: DcPromote::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            home_override: None,
            repo: PathBuf::from("."),
            bus_root_override: None,
            prod_arm_override: None,
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

    /// Override the Bus root directory (where the `event/test/*` green signal is
    /// read from). Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the prod-arm config-file path. Used in tests.
    #[must_use]
    pub fn with_prod_arm_path(mut self, p: PathBuf) -> Self {
        self.prod_arm_override = Some(p);
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
        // The Bus persist carries the L1–L3 green signal; if it can't be opened
        // we still publish the matrix, just with the gate held (no green ⇒
        // pending), so the worker never wedges on a missing Bus root.
        let persist = self
            .bus_root_override
            .clone()
            .or_else(default_bus_root)
            .and_then(|root| match Persist::open(root) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::debug!(error = %e, "dc_promote: persist open failed; green signal unavailable");
                    None
                }
            });
        let prod_arm_path = self.prod_arm_override.clone().or_else(prod_arm_path);
        loop {
            if self.is_leader() {
                // Read the green signal + the operator's prod-arm switch fresh
                // each tick (the panel may have toggled the arm; a tier may have
                // gone green) — both fail closed when unavailable.
                let tiers = persist.as_ref().map(read_tier_verdicts).unwrap_or_default();
                let prod_armed = prod_arm_path.as_deref().map(load_prod_arm).unwrap_or(false);
                poll_and_publish(
                    &mut self.core,
                    home.as_deref(),
                    Some(repo.as_path()),
                    &tiers,
                    prod_armed,
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

    // ---- DATACENTER-20 auto-promote: green detection + the gate decision ----

    #[test]
    fn parse_tier_verdict_maps_pass_fail_running() {
        // The farm's L1 form: {"outcome":"pass"} → green.
        assert_eq!(
            parse_tier_verdict(r#"{"outcome":"pass"}"#),
            TierVerdict::Green
        );
        // The L3 form: {"overall":"green"} → green.
        assert_eq!(
            parse_tier_verdict(r#"{"overall":"green"}"#),
            TierVerdict::Green
        );
        // A failure (either spelling) → red.
        assert_eq!(
            parse_tier_verdict(r#"{"outcome":"fail"}"#),
            TierVerdict::Red
        );
        assert_eq!(parse_tier_verdict(r#"{"overall":"RED"}"#), TierVerdict::Red);
        // Empty verdict (published but not decided) → running.
        assert_eq!(
            parse_tier_verdict(r#"{"outcome":""}"#),
            TierVerdict::Running
        );
        // No verdict field at all → running (empty default).
        assert_eq!(parse_tier_verdict(r#"{"jobid":"x"}"#), TierVerdict::Running);
        // Unparseable body → running (fail-safe, never green).
        assert_eq!(parse_tier_verdict("not json"), TierVerdict::Running);
    }

    #[test]
    fn build_eagle_green_requires_real_version_and_all_tiers_green() {
        let all_green = [TierVerdict::Green; 3];
        // Real version + all tiers green → GREEN.
        assert!(build_eagle_green("11.0.2-1", &all_green));
        // An unresolved/unknown build version holds, even with all tiers green.
        assert!(!build_eagle_green(UNKNOWN_VERSION, &all_green));
        assert!(!build_eagle_green("", &all_green));
        // Any non-green tier holds (fail-safe).
        assert!(!build_eagle_green(
            "11.0.2-1",
            &[TierVerdict::Green, TierVerdict::Red, TierVerdict::Green]
        ));
        assert!(!build_eagle_green(
            "11.0.2-1",
            &[TierVerdict::Green, TierVerdict::Running, TierVerdict::Green]
        ));
        // A tier that never ran holds.
        assert!(!build_eagle_green(
            "11.0.2-1",
            &[TierVerdict::Green, TierVerdict::Green, TierVerdict::NoRuns]
        ));
        // No tiers at all holds (nothing to vouch for green).
        assert!(!build_eagle_green("11.0.2-1", &[]));
    }

    #[test]
    fn decide_promote_holds_when_not_green() {
        // Not green: nothing advances regardless of the arm state.
        let d = decide_promote(false, false);
        assert_eq!(d.eagle, STATUS_PENDING);
        assert_eq!(d.do_stage, STATUS_PENDING);
        let d = decide_promote(false, true);
        assert_eq!(d.eagle, STATUS_PENDING);
        assert_eq!(d.do_stage, STATUS_PENDING);
    }

    #[test]
    fn decide_promote_auto_advances_eagle_and_gates_do_on_prod_arm() {
        // Green + DISARMED: eagle auto-advances, the DO step is QUEUED (held).
        let d = decide_promote(true, false);
        assert_eq!(d.eagle, STATUS_AUTO);
        assert_eq!(d.do_stage, STATUS_QUEUED);
        // Green + ARMED: eagle auto-advances, the DO step is ARMED (auto-promote).
        let d = decide_promote(true, true);
        assert_eq!(d.eagle, STATUS_AUTO);
        assert_eq!(d.do_stage, STATUS_ARMED);
    }

    #[test]
    fn prod_arm_round_trips_through_the_persisted_file_and_fails_closed() {
        let dir = std::env::temp_dir().join(format!("dc_promote_arm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dc-prod-arm.json");
        // A missing file → disarmed (fails closed).
        assert!(!load_prod_arm(&path));
        // Arm → persists → reads back armed.
        save_prod_arm(&path, true).expect("save armed");
        assert!(load_prod_arm(&path));
        // Disarm → persists → reads back disarmed.
        save_prod_arm(&path, false).expect("save disarmed");
        assert!(!load_prod_arm(&path));
        // A corrupt body → disarmed (fails closed, never auto-promotes to prod).
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(!load_prod_arm(&path));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn poll_publishes_auto_and_queued_states_via_the_dedup_core() {
        // A pure exercise of the decision → record projection (no Bus I/O): feed
        // the eagle/do stages exactly as `poll_and_publish` would when green +
        // disarmed, and confirm the records carry the new statuses + the promoted
        // build version. (The publish() side is fire-and-reap I/O, out of scope.)
        let mut core = DcPromote::new();
        let green = build_eagle_green("11.0.2-1", &[TierVerdict::Green; 3]);
        assert!(green);
        let d = decide_promote(green, false);
        let rec_eagle = core
            .observe("eagle", "11.0.2-1", d.eagle)
            .expect("eagle emits");
        assert_eq!(rec_eagle.status, STATUS_AUTO);
        assert_eq!(rec_eagle.version, "11.0.2-1");
        let rec_do = core
            .observe("do", "11.0.2-1", d.do_stage)
            .expect("do emits");
        assert_eq!(rec_do.status, STATUS_QUEUED);
        // Re-decide armed: the DO stage flips to armed (one independent change).
        let d2 = decide_promote(green, true);
        let rec_do2 = core
            .observe("do", "11.0.2-1", d2.do_stage)
            .expect("do arm transition emits");
        assert_eq!(rec_do2.status, STATUS_ARMED);
        // And its body carries the new status string for the panel projection.
        assert!(rec_do2.body().contains(r#""status":"armed""#));
    }
}

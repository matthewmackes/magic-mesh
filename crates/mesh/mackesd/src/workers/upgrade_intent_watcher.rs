//! INST-11 + INST-12 + INST-13 (v2.7) — fleet upgrade-barrier worker.
//!
//! Runs on **every** peer. Drives the `mde-update --coordinate
//! <version>` cycle to completion without operator intervention:
//!
//!   * **INST-11 (watch + upgrade).** Each 5 s tick enumerates
//!     `<mesh-home>/upgrade-intent/*.json`. For each intent this peer
//!     hasn't responded to, shell `dnf upgrade -y mde-core [mde-desktop]`
//!     and record the outcome — `ready` on success, `ready_failed` on a
//!     dnf failure (so the quorum count doesn't stall on one broken
//!     repo).
//!   * **INST-12 (quorum + grace barrier).** Once enough peers have
//!     responded *and* the grace window has passed, shell
//!     `mde-install --yes --profile=<installed-profile>` to apply the
//!     new bits, then mark this peer `complete`. Stragglers that come
//!     online after the barrier already fired self-heal on their next
//!     tick.
//!   * **INST-13 (leader cleanup).** The current leader deletes intent
//!     files once every reachable peer is `complete` and a +24 h
//!     grace-after-grace has elapsed, so the dir doesn't accumulate and
//!     a re-coordinate of the same version works after a rollback.
//!
//! **Schema tolerance.** `mde-update --coordinate` (INST-10) writes a
//! minimal intent (`target_version` + `initiated_at_ms` + an empty
//! `ready` array). This worker operates on the file as a
//! [`serde_json::Value`] and *normalizes* the three ack maps (`ready` /
//! `ready_failed` / `complete`) to objects on first write — so it
//! interoperates with the minimal writer without a cross-crate schema
//! refactor and preserves any fields it doesn't own.
//!
//! Test surface: every decision is a pure function over a
//! `serde_json::Value` (`pending_intents`, `should_act`, `mark_ready`,
//! `mark_ready_failed`, `mark_complete`, `barrier_should_fire`,
//! `peers_still_pending`, `intents_to_clean`); the worker body is a thin
//! shell-out + file-lock layer over them.

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fs2::FileExt;
use serde_json::{json, Value};

use super::{ShutdownToken, Worker};

/// Tick cadence — five seconds, matching `gluster_worker` /
/// `nebula_supervisor`.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Default grace window before the barrier may fire (4 h), used when an
/// intent file carries no explicit `grace_seconds` (the minimal INST-10
/// writer omits it).
pub const DEFAULT_GRACE_SECONDS: u64 = 14_400;

/// Extra grace after the barrier grace before the leader deletes a
/// fully-complete intent (+24 h), giving late stragglers a window.
pub const CLEANUP_EXTRA_GRACE_SECONDS: u64 = 86_400;

/// A peer-record older than this is treated as unreachable for the
/// cleanup quorum (so a permanently-gone peer doesn't pin an intent
/// file forever). Twelve hours.
pub const PEER_UNREACHABLE_MS: u64 = 12 * 60 * 60 * 1000;

/// Base RPM upgraded on every peer (renamed `mde` → `mde-core` 2026-05-29).
pub const BASE_PACKAGE: &str = "mde-core";
/// Desktop subpackage — only upgraded when already installed.
pub const DESKTOP_PACKAGE: &str = "mde-desktop";

// ───────────────────────── pure helpers ─────────────────────────

/// Hostnames present as keys of the object at `field` (an `[]` array or
/// a missing field — the minimal INST-10 shape — reads as the empty
/// set, exactly right: no peer has acked yet).
#[must_use]
fn ack_hosts(intent: &Value, field: &str) -> BTreeSet<String> {
    match intent.get(field) {
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

/// Barrier issue time in epoch seconds: prefer an explicit `issued_at`
/// (seconds), else derive from the INST-10 `initiated_at_ms`.
#[must_use]
fn issued_at_s(intent: &Value) -> u64 {
    if let Some(s) = intent.get("issued_at").and_then(Value::as_u64) {
        return s;
    }
    intent
        .get("initiated_at_ms")
        .and_then(Value::as_u64)
        .map_or(0, |ms| ms / 1000)
}

/// Grace window in seconds for this intent (explicit or the default).
#[must_use]
fn grace_seconds(intent: &Value) -> u64 {
    intent
        .get("grace_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_GRACE_SECONDS)
}

/// Ensure the root is an object and the three ack fields are objects
/// (converting the minimal INST-10 `ready: []` array to `{}`), returning
/// an owned, writable copy.
#[must_use]
fn normalize(intent: &Value) -> Value {
    let mut v = match intent {
        Value::Object(_) => intent.clone(),
        _ => json!({}),
    };
    for field in ["ready", "ready_failed", "complete"] {
        if !matches!(v.get(field), Some(Value::Object(_))) {
            v[field] = json!({});
        }
    }
    v
}

/// All intent files in `dir`, sorted. Missing dir → empty.
#[must_use]
pub fn pending_intents(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// Should `hostname` run its `dnf upgrade` half for this intent? True
/// only when it hasn't already responded (not in `ready`, `ready_failed`,
/// or `complete`) — excluding `ready_failed` keeps a broken repo from
/// re-running dnf every tick.
#[must_use]
pub fn should_act(intent: &Value, hostname: &str) -> bool {
    !ack_hosts(intent, "ready").contains(hostname)
        && !ack_hosts(intent, "ready_failed").contains(hostname)
        && !ack_hosts(intent, "complete").contains(hostname)
}

/// Record `hostname`'s successful upgrade in `ready`.
#[must_use]
pub fn mark_ready(intent: &Value, hostname: &str, version: &str, now_s: u64) -> Value {
    let mut v = normalize(intent);
    v["ready"][hostname] = json!({ "at": now_s, "rpm_version": version });
    v
}

/// Record `hostname`'s failed upgrade in `ready_failed` (counts toward
/// "responded" so the barrier doesn't stall).
#[must_use]
pub fn mark_ready_failed(intent: &Value, hostname: &str, error: &str, now_s: u64) -> Value {
    let mut v = normalize(intent);
    v["ready_failed"][hostname] = json!({ "at": now_s, "error": error });
    v
}

/// Record `hostname` as having applied the new bits (`complete`).
#[must_use]
pub fn mark_complete(intent: &Value, hostname: &str, now_s: u64) -> Value {
    let mut v = normalize(intent);
    v["complete"][hostname] = json!({ "at": now_s });
    v
}

/// Should the barrier fire for `hostname` now (run `mde-install --yes`)?
///
/// Fires when this peer is `ready` and not yet `complete`, AND either:
///   * the barrier already fired on some peer (`complete` non-empty) —
///     the straggler self-heal case; or
///   * enough peers responded (`ready` + `ready_failed` ≥
///     `max(1, peer_count - 1)`) and the grace window has elapsed.
#[must_use]
pub fn barrier_should_fire(intent: &Value, peer_count: usize, now_s: u64, hostname: &str) -> bool {
    let ready = ack_hosts(intent, "ready");
    let complete = ack_hosts(intent, "complete");
    if !ready.contains(hostname) || complete.contains(hostname) {
        return false;
    }
    if !complete.is_empty() {
        return true; // straggler: barrier already fired elsewhere.
    }
    let responded = ready.len() + ack_hosts(intent, "ready_failed").len();
    let quorum = responded >= std::cmp::max(1, peer_count.saturating_sub(1));
    let grace_passed = now_s.saturating_sub(issued_at_s(intent)) >= grace_seconds(intent);
    quorum && grace_passed
}

/// Peers in `all_peers` that have not yet marked `complete`.
#[must_use]
pub fn peers_still_pending(intent: &Value, all_peers: &BTreeSet<String>) -> Vec<String> {
    let complete = ack_hosts(intent, "complete");
    all_peers.difference(&complete).cloned().collect()
}

/// Intent file paths the leader may delete: every reachable peer is
/// `complete` and the +24 h grace-after-grace has elapsed.
#[must_use]
pub fn intents_to_clean(
    intents: &[(PathBuf, Value)],
    all_peers: &BTreeSet<String>,
    unreachable: &BTreeSet<String>,
    now_s: u64,
) -> Vec<PathBuf> {
    let required = all_peers.difference(unreachable).count();
    intents
        .iter()
        .filter(|(_, v)| {
            let complete = ack_hosts(v, "complete").len();
            let aged = now_s.saturating_sub(issued_at_s(v))
                >= grace_seconds(v) + CLEANUP_EXTRA_GRACE_SECONDS;
            complete >= required && aged
        })
        .map(|(p, _)| p.clone())
        .collect()
}

// ───────────────────────── worker body ─────────────────────────

/// The upgrade-barrier worker. One per peer; spawned in `run_serve`.
pub struct UpgradeIntentWatcher {
    tick: Duration,
    mesh_home: PathBuf,
    hostname: String,
    node_id: String,
    leader_lock: PathBuf,
    dnf_binary: String,
    install_binary: String,
}

impl UpgradeIntentWatcher {
    /// Construct with production defaults: mesh-home from
    /// `$MDE_MESH_HOME`/`~/.mde-mesh`, this host's name, the standard
    /// leader lock, and the real `dnf` / `mde-install` binaries.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            tick: DEFAULT_TICK_INTERVAL,
            mesh_home: mackes_mesh_types::peers::default_mesh_home(),
            hostname: local_hostname(),
            node_id,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            dnf_binary: "dnf".to_string(),
            install_binary: "mde-install".to_string(),
        }
    }

    fn intent_dir(&self) -> PathBuf {
        self.mesh_home.join("upgrade-intent")
    }

    /// Peer roster from the GFS peers dir (PEERVER convergence files).
    /// Returns `(all_hostnames, unreachable_hostnames, peer_count)`.
    fn roster(&self) -> (BTreeSet<String>, BTreeSet<String>, usize) {
        let dir = mackes_mesh_types::peers::peers_dir(&self.mesh_home);
        let recs = mackes_mesh_types::peers::read_peers(&dir);
        let all: BTreeSet<String> = recs.iter().map(|r| r.hostname.clone()).collect();
        let unreachable: BTreeSet<String> = recs
            .iter()
            .filter(|r| r.is_stale(PEER_UNREACHABLE_MS))
            .map(|r| r.hostname.clone())
            .collect();
        let count = all.len().max(1);
        (all, unreachable, count)
    }

    fn am_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    /// One tick. Silent no-op when the upgrade-intent dir doesn't exist
    /// (no coordinate in flight, or mesh-home not mounted).
    fn tick_once(&self) {
        let dir = self.intent_dir();
        if !dir.is_dir() {
            return;
        }
        let now_s = now_s();
        let (all_peers, unreachable, peer_count) = self.roster();

        for path in pending_intents(&dir) {
            let Ok(intent) = read_value(&path) else {
                continue;
            };

            // INST-11 — upgrade half.
            if should_act(&intent, &self.hostname) {
                match self.run_dnf_upgrade() {
                    Ok(version) => {
                        let host = self.hostname.clone();
                        let _ = locked_update(&path, |v| mark_ready(v, &host, &version, now_s));
                    }
                    Err(err) => {
                        let host = self.hostname.clone();
                        let _ = locked_update(&path, |v| mark_ready_failed(v, &host, &err, now_s));
                    }
                }
                continue; // re-evaluate the barrier on the next tick.
            }

            // INST-12 — barrier half (re-read so a sibling's mark is seen).
            let Ok(fresh) = read_value(&path) else {
                continue;
            };
            if barrier_should_fire(&fresh, peer_count, now_s, &self.hostname) {
                if self.run_mde_install().is_ok() {
                    let host = self.hostname.clone();
                    let _ = locked_update(&path, |v| mark_complete(v, &host, now_s));
                }
            }
        }

        // INST-13 — leader-only cleanup.
        if self.am_leader() {
            let intents: Vec<(PathBuf, Value)> = pending_intents(&dir)
                .into_iter()
                .filter_map(|p| read_value(&p).ok().map(|v| (p, v)))
                .collect();
            for path in intents_to_clean(&intents, &all_peers, &unreachable, now_s) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// `dnf upgrade -y mde-core [mde-desktop]`; on success return the
    /// installed base version. `mde-desktop` is only upgraded when
    /// already present so a headless peer doesn't pull the desktop.
    fn run_dnf_upgrade(&self) -> Result<String, String> {
        let mut pkgs = vec![BASE_PACKAGE];
        if rpm_installed(DESKTOP_PACKAGE) {
            pkgs.push(DESKTOP_PACKAGE);
        }
        let mut cmd = Command::new(&self.dnf_binary);
        cmd.arg("upgrade").arg("-y").args(&pkgs);
        match cmd.status() {
            Ok(s) if s.success() => Ok(rpm_version(BASE_PACKAGE)),
            Ok(s) => Err(format!("dnf upgrade exit {}", s.code().unwrap_or(-1))),
            Err(e) => Err(format!("dnf spawn failed: {e}")),
        }
    }

    /// `mde-install --yes --profile=<installed-profile>` to apply the
    /// new bits. The profile is read from the marker the last install
    /// wrote; absent → `full` (the most-capable safe default).
    fn run_mde_install(&self) -> Result<(), String> {
        let profile = installed_profile().unwrap_or_else(|| "full".to_string());
        let status = Command::new(&self.install_binary)
            .arg("--yes")
            .arg(format!("--profile={profile}"))
            .status()
            .map_err(|e| format!("mde-install spawn failed: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("mde-install exit {}", status.code().unwrap_or(-1)))
        }
    }
}

#[async_trait::async_trait]
impl Worker for UpgradeIntentWatcher {
    fn name(&self) -> &'static str {
        "upgrade_intent_watcher"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        self.tick_once();
        loop {
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick) => self.tick_once(),
            }
        }
    }
}

// ───────────────────────── shell-out helpers ─────────────────────────

fn read_value(path: &Path) -> std::io::Result<Value> {
    let s = std::fs::read_to_string(path)?;
    serde_json::from_str(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Exclusive-locked read-modify-write of an intent file: lock, read the
/// current contents (so a sibling peer's concurrent mark isn't lost),
/// apply `f`, truncate + rewrite, unlock. Lock contention / IO errors
/// are returned so the caller retries next tick.
fn locked_update<F>(path: &Path, f: F) -> std::io::Result<()>
where
    F: FnOnce(&Value) -> Value,
{
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    file.lock_exclusive()?;
    let result = (|| {
        let mut s = String::new();
        file.read_to_string(&mut s)?;
        let current: Value = serde_json::from_str(&s)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let next = f(&current);
        let json = serde_json::to_string_pretty(&next)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(json.as_bytes())?;
        file.flush()
    })();
    let _ = FileExt::unlock(&file);
    result
}

fn rpm_installed(pkg: &str) -> bool {
    Command::new("rpm")
        .args(["-q", pkg])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn rpm_version(pkg: &str) -> String {
    Command::new("rpm")
        .args(["-q", "--queryformat", "%{VERSION}", pkg])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Profile from the installed-profile marker `mde-install` writes.
fn installed_profile() -> Option<String> {
    std::fs::read_to_string("/var/lib/mde/installed-profile")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn local_hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn minimal_intent(ts_ms: u64) -> Value {
        // The exact shape INST-10's `mde-update --coordinate` writes.
        json!({
            "target_version": "2.7.1",
            "initiated_by": "anvil",
            "initiated_at_ms": ts_ms,
            "ready": [],
        })
    }

    #[test]
    fn pending_intents_lists_sorted_json_only() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("2.7.1.json"), "{}").unwrap();
        fs::write(dir.path().join("2.7.0.json"), "{}").unwrap();
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let got = pending_intents(dir.path());
        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("2.7.0.json"));
        assert!(got[1].ends_with("2.7.1.json"));
    }

    #[test]
    fn should_act_true_until_responded() {
        let intent = minimal_intent(0);
        assert!(should_act(&intent, "forge"));
        let acked = mark_ready(&intent, "forge", "2.7.1", 10);
        assert!(!should_act(&acked, "forge"));
        let failed = mark_ready_failed(&intent, "forge", "repo down", 10);
        assert!(!should_act(&failed, "forge"));
    }

    #[test]
    fn mark_ready_normalizes_empty_array_to_object() {
        let intent = minimal_intent(0);
        let acked = mark_ready(&intent, "forge", "2.7.1", 99);
        assert!(acked["ready"].is_object());
        assert_eq!(acked["ready"]["forge"]["rpm_version"], "2.7.1");
        assert_eq!(acked["ready"]["forge"]["at"], 99);
        // Untouched fields preserved.
        assert_eq!(acked["target_version"], "2.7.1");
        assert_eq!(acked["initiated_by"], "anvil");
    }

    #[test]
    fn barrier_waits_for_quorum_and_grace() {
        // issued at t=0s (initiated_at_ms=0); default grace 14400s.
        let mut intent = minimal_intent(0);
        intent = mark_ready(&intent, "forge", "2.7.1", 1);
        // 2 peers → quorum needs max(1, 2-1)=1 responded → met.
        // But grace not yet passed (now=10s < 14400s).
        assert!(!barrier_should_fire(&intent, 2, 10, "forge"));
        // After grace passes, fires for the ready peer.
        assert!(barrier_should_fire(&intent, 2, 14_401, "forge"));
        // Not for a peer that isn't ready.
        assert!(!barrier_should_fire(&intent, 2, 14_401, "ghost"));
    }

    #[test]
    fn barrier_quorum_counts_failures_as_responded() {
        let mut intent = minimal_intent(0);
        intent = mark_ready(&intent, "forge", "2.7.1", 1);
        intent = mark_ready_failed(&intent, "anvil", "repo down", 1);
        // 3 peers → quorum needs max(1, 3-1)=2 responded. ready(1) +
        // ready_failed(1) = 2 → met; grace passed → fires for forge.
        assert!(barrier_should_fire(&intent, 3, 14_401, "forge"));
    }

    #[test]
    fn straggler_fires_after_barrier_regardless_of_grace() {
        let mut intent = minimal_intent(0);
        intent = mark_ready(&intent, "late", "2.7.1", 1);
        // Some other peer already completed → straggler fires now even
        // though grace hasn't passed and quorum math is irrelevant.
        intent = mark_complete(&intent, "forge", 2);
        assert!(barrier_should_fire(&intent, 5, 10, "late"));
        // Not once this peer itself is complete.
        let done = mark_complete(&intent, "late", 3);
        assert!(!barrier_should_fire(&done, 5, 10, "late"));
    }

    #[test]
    fn cleanup_requires_all_reachable_complete_and_aged() {
        let all: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| (*s).to_string()).collect();
        let none = BTreeSet::new();
        let mut intent = minimal_intent(0);
        intent = mark_complete(&intent, "a", 1);
        intent = mark_complete(&intent, "b", 1);
        let path = PathBuf::from("/x/2.7.1.json");
        let aged = grace_seconds(&intent) + CLEANUP_EXTRA_GRACE_SECONDS + 1;
        // c not complete → nothing to clean.
        assert!(intents_to_clean(&[(path.clone(), intent.clone())], &all, &none, aged).is_empty());
        // c complete → eligible once aged.
        intent = mark_complete(&intent, "c", 1);
        assert_eq!(
            intents_to_clean(&[(path.clone(), intent.clone())], &all, &none, aged),
            vec![path.clone()]
        );
        // Not yet aged → keep.
        assert!(intents_to_clean(&[(path.clone(), intent.clone())], &all, &none, 100).is_empty());
        // c unreachable → a+b complete satisfies the reduced quorum.
        let mut intent2 = minimal_intent(0);
        intent2 = mark_complete(&intent2, "a", 1);
        intent2 = mark_complete(&intent2, "b", 1);
        let unreach: BTreeSet<String> = ["c"].iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            intents_to_clean(&[(path.clone(), intent2)], &all, &unreach, aged),
            vec![path]
        );
    }

    #[test]
    fn peers_still_pending_excludes_complete() {
        let all: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| (*s).to_string()).collect();
        let intent = mark_complete(&minimal_intent(0), "b", 1);
        let mut pending = peers_still_pending(&intent, &all);
        pending.sort();
        assert_eq!(pending, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn locked_update_preserves_concurrent_marks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("2.7.1.json");
        fs::write(&path, serde_json::to_string(&minimal_intent(0)).unwrap()).unwrap();
        locked_update(&path, |v| mark_ready(v, "forge", "2.7.1", 5)).unwrap();
        locked_update(&path, |v| mark_ready(v, "anvil", "2.7.1", 6)).unwrap();
        let back = read_value(&path).unwrap();
        assert_eq!(back["ready"]["forge"]["rpm_version"], "2.7.1");
        assert_eq!(back["ready"]["anvil"]["at"], 6);
    }
}

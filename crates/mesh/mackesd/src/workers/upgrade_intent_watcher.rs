//! INST-11 + INST-12 + INST-13 (v2.7) — fleet upgrade-barrier worker.
//!
//! Runs on **every** peer. Drives the signed `mde-update --coordinate
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
//! **Schema tolerance.** The pure state-machine helpers continue to accept the
//! historical minimal intent shape (`target_version` + `initiated_at_ms` + an
//! empty `ready` array), and normalize the three ack maps (`ready` /
//! `ready_failed` / `complete`) to objects on first write. The production file
//! path additionally requires a root-issued, exact-body HMAC v1 capability, so
//! unsigned legacy bytes remain inert rather than becoming package authority.
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
use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmSigner, CloudArmedToken};
use serde_json::{json, Value};

use super::{ShutdownToken, Worker};
use crate::workers::proc::{output_with_timeout, status_with_timeout};
use crate::ipc::action_auth::{
    production_action_signer, ActionAuthorizer, MutationContext, ACTION_SCHEMA_VERSION,
    MAX_AUTH_TTL_MS,
};

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

/// Hard deadline for either privileged package operation. The shared process
/// runner also isolates each invocation in its own process group, so a wedged
/// package hook cannot pin this worker or leave helper descendants behind.
pub const UPGRADE_COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Base RPM upgraded on every peer (renamed `mde` → `mde-core` 2026-05-29).
pub const BASE_PACKAGE: &str = "mde-core";
/// Desktop subpackage — only upgraded when already installed.
pub const DESKTOP_PACKAGE: &str = "mde-desktop";

/// Closed capability context for every root-issued replicated upgrade intent.
pub const UPGRADE_INTENT_AUTH_VERB: &str = "upgrade-intent";
const UPGRADE_INTENT_AUTH_NODE: &str = "fleet";

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
/// The intent's `target_version` (the INST-10 minimal field), or `""`.
#[must_use]
fn target_version(intent: &Value) -> String {
    intent
        .get("target_version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

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

/// PLANES-7 (INST-10 in-tree equivalent) — publish a coordinated-upgrade
/// **intent** that every peer's watcher fleet-processes (quorum + grace
/// barrier). This is the best-practice update-now path: a typed intent on
/// the replicated volume, not a raw GUI-side dnf. `target_version` is a
/// coordination label (the watcher upgrades to repo-latest; the label
/// names/dedups the intent), so `"latest"` or a release tag both work.
/// Re-coordinating the same label overwrites the file with an empty
/// ready-set, so every peer upgrades again.
///
/// # Errors
/// IO, credential, or serialization failure writing the intent file.
pub fn write_intent(
    mesh_home: &Path,
    target_version: &str,
    now_ms: u64,
) -> std::io::Result<PathBuf> {
    write_intent_with_artifact_digest(mesh_home, target_version, now_ms, None)
}

/// Publish an intent bound to the exact artifact admitted by lifecycle
/// authority.  An intent without this receipt remains readable for migration
/// and diagnostics, but the watcher will never invoke a package manager for
/// it.
pub fn write_intent_with_artifact_digest(
    mesh_home: &Path,
    target_version: &str,
    now_ms: u64,
    artifact_digest: Option<&str>,
) -> std::io::Result<PathBuf> {
    write_intent_signed(mesh_home, target_version, now_ms, None, artifact_digest)
}

/// Publish an intent carrying the complete authority-admitted artifact
/// selection. The JSON is parsed and validated before it is signed, so the
/// watcher never receives an opaque operator label in place of a receipt.
pub fn write_intent_with_selection_json(
    mesh_home: &Path,
    target_version: &str,
    now_ms: u64,
    selection_json: &str,
) -> std::io::Result<PathBuf> {
    let selection: mackes_mesh_types::lifecycle::LifecycleArtifactSelectionV1 =
        serde_json::from_str(selection_json).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
        })?;
    selection.validate().map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid artifact selection: {error:?}"))
    })?;
    let selection_value = serde_json::to_value(selection).map_err(std::io::Error::other)?;
    write_intent_signed(
        mesh_home,
        target_version,
        now_ms,
        Some(selection_value),
        None,
    )
}

fn write_intent_signed(
    mesh_home: &Path,
    target_version: &str,
    now_ms: u64,
    selection: Option<Value>,
    artifact_digest: Option<&str>,
) -> std::io::Result<PathBuf> {
    let signer = production_action_signer()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
    let auth_now_ms = i64::try_from(now_ms).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "upgrade intent timestamp is outside the capability range",
        )
    })?;
    write_intent_with_signer(
        mesh_home,
        target_version,
        now_ms,
        &signer,
        auth_now_ms,
        &fresh_nonce(),
        selection,
        artifact_digest,
    )
}

/// Testable signing seam for the direct CLI authority path. Production callers
/// use [`write_intent`], which loads the root-only systemd credential first.
fn write_intent_with_signer(
    mesh_home: &Path,
    target_version: &str,
    initiated_at_ms: u64,
    signer: &CloudArmSigner,
    auth_now_ms: i64,
    nonce: &str,
        selection: Option<Value>,
        artifact_digest: Option<&str>,
) -> std::io::Result<PathBuf> {
    let dir = mesh_home.join("upgrade-intent");
    std::fs::create_dir_all(&dir)?;
    let safe: String = target_version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = if safe.is_empty() { "latest" } else { &safe };
    let path = dir.join(format!("{stem}.json"));
    let body = json!({
        "schema_version": ACTION_SCHEMA_VERSION,
        "target_version": target_version,
        "initiated_at_ms": initiated_at_ms,
        "ready": {},
    });
    let mut body = body;
    if let Some(digest) = artifact_digest {
        body["artifact_digest"] = Value::String(digest.to_string());
    }
    if let Some(selection) = selection {
        body["artifact_selection"] = selection;
    }
    let text = sign_intent_document(&path, body, signer, auth_now_ms, nonce)?;
    write_intent_file(&path, &text)?;
    Ok(path)
}

fn admitted_artifact_digest(intent: &Value) -> Option<String> {
    if let Some(selection) = intent.get("artifact_selection") {
        let parsed: Result<mackes_mesh_types::lifecycle::LifecycleArtifactSelectionV1, _> =
            serde_json::from_value(selection.clone());
        if let Ok(selection) = parsed {
            if selection.validate().is_ok() {
                return Some(selection.artifact_digest_hex);
            }
        }
        return None;
    }
    let digest = intent.get("artifact_digest").and_then(Value::as_str)?;
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(digest.to_string())
}

/// Build the stable capability target from the exact replicated filename. A
/// valid intent copied to a different filename therefore fails provenance
/// verification instead of being adopted by the watcher.
fn intent_auth_target(path: &Path) -> std::io::Result<String> {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| {
            name.len() > 5
                && std::path::Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "upgrade intent path has no safe JSON filename",
            )
        })?;
    Ok(format!("upgrade-intent:{name}"))
}

/// Validate the signed document's shape before any state-machine decision.
fn validate_intent_document(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "upgrade intent is not a JSON object".to_string())?;
    let target_version = object
        .get("target_version")
        .and_then(Value::as_str)
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| "upgrade intent has no target_version".to_string())?;
    if target_version.len() > 256 {
        return Err("upgrade intent target_version is too long".to_string());
    }
    if object
        .get("initiated_at_ms")
        .and_then(Value::as_u64)
        .is_none()
    {
        return Err("upgrade intent has no initiated_at_ms".to_string());
    }
    for field in ["ready", "ready_failed", "complete"] {
        if let Some(value) = object.get(field) {
            if !value.is_object() && !value.is_array() {
                return Err(format!("upgrade intent field `{field}` is not a map"));
            }
        }
    }
    Ok(())
}

fn sign_intent_document(
    path: &Path,
    mut document: Value,
    signer: &CloudArmSigner,
    auth_now_ms: i64,
    nonce: &str,
) -> std::io::Result<String> {
    let object = document.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "upgrade intent must be a JSON object",
        )
    })?;
    object.insert(
        "schema_version".to_string(),
        Value::from(ACTION_SCHEMA_VERSION),
    );
    object.remove("armed_token");
    validate_intent_document(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let unsigned = serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?;
    let target = intent_auth_target(path)?;
    let token = CloudArmedToken::mint(
        signer,
        nonce,
        auth_now_ms.saturating_add(MAX_AUTH_TTL_MS),
        UPGRADE_INTENT_AUTH_VERB,
        UPGRADE_INTENT_AUTH_NODE,
        &target,
        &cloud_request_digest(&unsigned).map_err(std::io::Error::other)?,
    )
    .encode();
    document["armed_token"] = Value::String(token);
    serde_json::to_string_pretty(&document).map_err(std::io::Error::other)
}

fn write_intent_file(path: &Path, text: &str) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?;
    let mut file: std::fs::File = fd.into();
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

fn fresh_nonce() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn wall_now_ms_i64() -> std::io::Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "system clock is before the Unix epoch",
            )
        })
        .and_then(|duration| {
            i64::try_from(duration.as_millis()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "system clock is outside the capability range",
                )
            })
        })
}

fn verify_intent_body(
    authorizer: &ActionAuthorizer,
    path: &Path,
    raw: &str,
) -> Result<Value, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|_| "upgrade intent is not valid JSON".to_string())?;
    validate_intent_document(&value)?;
    let target = intent_auth_target(path).map_err(|error| error.to_string())?;
    authorizer.verify_exact_body(
        raw,
        MutationContext {
            verb: UPGRADE_INTENT_AUTH_VERB,
            node: UPGRADE_INTENT_AUTH_NODE,
            target: &target,
        },
    )?;
    Ok(value)
}

fn authorize_intent_body(
    authorizer: &ActionAuthorizer,
    path: &Path,
    raw: &str,
) -> Result<(), String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|_| "upgrade intent is not valid JSON".to_string())?;
    validate_intent_document(&value)?;
    let target = intent_auth_target(path).map_err(|error| error.to_string())?;
    authorizer.authorize(
        raw,
        MutationContext {
            verb: UPGRADE_INTENT_AUTH_VERB,
            node: UPGRADE_INTENT_AUTH_NODE,
            target: &target,
        },
    )
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

/// OBS-7 — build the MON-3 `AlertEvent` JSON for one upgrade-state
/// transition. Pure so the shape + the deterministic id are unit-tested.
/// The id keys on (version, state, host) so re-emitting the same
/// transition is idempotent (alert_relay de-dupes by id).
#[must_use]
pub fn upgrade_alert_event(
    host: &str,
    version: &str,
    state: &str,
    severity: &str,
    summary: &str,
    now_s: u64,
) -> Value {
    let safe = |s: &str| s.replace(['/', '.', ' ', ':'], "-");
    json!({
        "id": format!("upgrade-{}-{}-{}", safe(version), safe(state), safe(host)),
        "ts": now_s,
        "severity": severity,
        "category": "upgrade.transition",
        "alert": format!("upgrade_{state}"),
        "host": host,
        "summary": summary,
        "value": version,
        "threshold": "",
        "chart_url": "",
        "fired_by": "upgrade_intent_watcher",
        "seen_by": [],
    })
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
    command_timeout: Duration,
    /// Verifier for the root-issued, exact-body intent capability.
    authorizer: ActionAuthorizer,
    /// Root signer used to issue the next exact-body capability after this
    /// worker appends its authenticated state transition. Missing credentials
    /// disable the execution path entirely.
    signer: Option<CloudArmSigner>,
    /// Deterministic capability clock used only by the in-process test seam.
    /// Production state updates always call [`wall_now_ms_i64`].
    #[cfg(test)]
    test_now_ms: Option<i64>,
    /// OBS-7 — where upgrade-state-transition alerts are dropped for
    /// `alert_relay` to surface (the MON-3 alerts dir). `None` skips the
    /// emit (a test that doesn't assert on alerts).
    alerts_dir: Option<PathBuf>,
}

impl UpgradeIntentWatcher {
    /// Construct with production defaults: mesh-home from
    /// `$MDE_MESH_HOME`/`~/.mde-mesh`, this host's name, the standard
    /// leader lock, and the real `dnf` / `mde-install` binaries.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        let signer = match production_action_signer() {
            Ok(signer) => Some(signer),
            Err(error) => {
                tracing::error!(
                    target: "mackesd::upgrade_intent_watcher",
                    %error,
                    "root upgrade-intent signing unavailable; upgrade execution is disabled"
                );
                None
            }
        };
        Self {
            tick: DEFAULT_TICK_INTERVAL,
            mesh_home: mackes_mesh_types::peers::default_mesh_home(),
            hostname: local_hostname(),
            node_id,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            dnf_binary: "dnf".to_string(),
            install_binary: "mde-install".to_string(),
            command_timeout: UPGRADE_COMMAND_TIMEOUT,
            authorizer: ActionAuthorizer::production(),
            signer,
            #[cfg(test)]
            test_now_ms: None,
            alerts_dir: crate::workers::alert_relay::default_alerts_dir(),
        }
    }

    /// Inject deterministic HMAC authority for hostile/valid watcher tests.
    #[cfg(test)]
    #[must_use]
    fn with_test_authority(mut self, key: &[u8], auth_root: PathBuf, now_ms: i64) -> Self {
        self.authorizer = ActionAuthorizer::for_test(key, auth_root, now_ms);
        self.signer = Some(CloudArmSigner::new(key.to_vec()).expect("test signer key"));
        self.test_now_ms = Some(now_ms);
        self
    }

    fn auth_now_ms(&self) -> std::io::Result<i64> {
        #[cfg(test)]
        if let Some(now_ms) = self.test_now_ms {
            return Ok(now_ms);
        }
        wall_now_ms_i64()
    }

    /// OBS-7 test seam — point the alert emit at a scratch dir.
    #[must_use]
    pub fn with_alerts_dir(mut self, dir: PathBuf) -> Self {
        self.alerts_dir = Some(dir);
        self
    }

    fn intent_dir(&self) -> PathBuf {
        self.mesh_home.join("upgrade-intent")
    }

    /// OBS-7 — drop an upgrade-state-transition alert into the alerts dir
    /// (best-effort; `alert_relay` surfaces it via the Bus FDO path). The
    /// id is deterministic per (version, state, host) so a re-emitted
    /// transition de-dupes rather than re-toasting.
    fn emit_upgrade_alert(&self, version: &str, state: &str, severity: &str, summary: &str) {
        let Some(dir) = &self.alerts_dir else {
            return;
        };
        let event = upgrade_alert_event(&self.hostname, version, state, severity, summary, now_s());
        let _ = std::fs::create_dir_all(dir);
        let id = event["id"].as_str().unwrap_or("upgrade").to_string();
        let path = dir.join(format!("{id}.json"));
        let tmp = dir.join(format!(".{id}.json.tmp"));
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&event).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
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
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }

    /// One tick. Silent no-op when the upgrade-intent dir doesn't exist
    /// (no coordinate in flight, or mesh-home not mounted).
    fn tick_once(&self) {
        // A watcher without both halves of the root authority must never turn
        // replicated bytes into a package-manager invocation.
        let Some(signer) = self.signer.as_ref() else {
            return;
        };
        let dir = self.intent_dir();
        if !dir.is_dir() {
            return;
        }
        let Ok(state_auth_now_ms) = self.auth_now_ms() else {
            return;
        };
        let now_s = now_s();
        let (all_peers, unreachable, peer_count) = self.roster();

        for path in pending_intents(&dir) {
            let Ok((raw, intent)) = read_authorized_intent(&path, &self.authorizer) else {
                continue;
            };

            // INST-11 — upgrade half.
            if should_act(&intent, &self.hostname) {
                if admitted_artifact_digest(&intent).is_none() {
                    self.emit_upgrade_alert(
                        &target_version(&intent),
                        "blocked",
                        "warn",
                        "upgrade intent has no admitted artifact digest; package execution withheld",
                    );
                    continue;
                }
                if authorize_intent_body(&self.authorizer, &path, &raw).is_err() {
                    continue;
                }
                match self.run_dnf_upgrade() {
                    Ok(version) => {
                        let host = self.hostname.clone();
                        let initiated_at_ms = intent["initiated_at_ms"].as_u64().unwrap_or(0);
                        let intent_target_version = target_version(&intent);
                        let _ = locked_authorized_update(
                            &path,
                            &self.authorizer,
                            signer,
                            &intent_target_version,
                            initiated_at_ms,
                            state_auth_now_ms,
                            |v| mark_ready(v, &host, &version, now_s),
                        );
                        self.emit_upgrade_alert(
                            &version,
                            "ready",
                            "info",
                            &format!("staged {version}; awaiting the fleet upgrade barrier"),
                        );
                    }
                    Err(err) => {
                        let host = self.hostname.clone();
                        let initiated_at_ms = intent["initiated_at_ms"].as_u64().unwrap_or(0);
                        let intent_target_version = target_version(&intent);
                        let _ = locked_authorized_update(
                            &path,
                            &self.authorizer,
                            signer,
                            &intent_target_version,
                            initiated_at_ms,
                            state_auth_now_ms,
                            |v| mark_ready_failed(v, &host, &err, now_s),
                        );
                        self.emit_upgrade_alert(
                            &target_version(&intent),
                            "failed",
                            "crit",
                            &format!("dnf upgrade failed: {err}"),
                        );
                    }
                }
                continue; // re-evaluate the barrier on the next tick.
            }

            // INST-12 — barrier half (re-read so a sibling's mark is seen).
            let Ok((fresh_raw, fresh)) = read_authorized_intent(&path, &self.authorizer) else {
                continue;
            };
            if barrier_should_fire(&fresh, peer_count, now_s, &self.hostname) {
                if admitted_artifact_digest(&fresh).is_none() {
                    continue;
                }
                if authorize_intent_body(&self.authorizer, &path, &fresh_raw).is_err() {
                    continue;
                }
                if self.run_mde_install().is_ok() {
                    let host = self.hostname.clone();
                    let initiated_at_ms = fresh["initiated_at_ms"].as_u64().unwrap_or(0);
                    let target = target_version(&fresh);
                    let _ = locked_authorized_update(
                        &path,
                        &self.authorizer,
                        signer,
                        &target,
                        initiated_at_ms,
                        state_auth_now_ms,
                        |v| mark_complete(v, &host, now_s),
                    );
                    self.emit_upgrade_alert(
                        &target_version(&fresh),
                        "complete",
                        "info",
                        "upgrade applied; node is on the new version",
                    );
                }
            }
        }

        // INST-13 — leader-only cleanup.
        if self.am_leader() {
            let intents: Vec<(PathBuf, Value)> = pending_intents(&dir)
                .into_iter()
                .filter_map(|p| {
                    read_authorized_intent(&p, &self.authorizer)
                        .ok()
                        .map(|(_, v)| (p, v))
                })
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
        if rpm_installed(DESKTOP_PACKAGE, self.command_timeout) {
            pkgs.push(DESKTOP_PACKAGE);
        }
        let mut cmd = Command::new(&self.dnf_binary);
        cmd.arg("upgrade").arg("-y").args(&pkgs);
        match status_with_timeout(cmd, self.command_timeout) {
            Ok(s) if s.success() => Ok(rpm_version(BASE_PACKAGE, self.command_timeout)),
            Ok(s) => Err(format!("dnf upgrade exit {}", s.code().unwrap_or(-1))),
            Err(e) => Err(format!("dnf upgrade failed: {e}")),
        }
    }

    /// `mde-install --yes --profile=<installed-profile>` to apply the
    /// new bits. The profile is read from the marker the last install
    /// wrote; absent → `full` (the most-capable safe default).
    fn run_mde_install(&self) -> Result<(), String> {
        let profile = installed_profile().unwrap_or_else(|| "full".to_string());
        let mut command = Command::new(&self.install_binary);
        command
            .arg("--yes")
            .arg(format!("--profile={profile}"));
        let status = status_with_timeout(command, self.command_timeout)
            .map_err(|e| format!("mde-install failed: {e}"))?;
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

fn open_intent_file(path: &Path, flags: rustix::fs::OFlags) -> std::io::Result<std::fs::File> {
    use rustix::fs::Mode;

    let fd = rustix::fs::open(path, flags | rustix::fs::OFlags::NOFOLLOW, Mode::empty())?;
    Ok(fd.into())
}

fn read_raw(path: &Path) -> std::io::Result<String> {
    use rustix::fs::OFlags;

    let mut file = open_intent_file(path, OFlags::RDONLY | OFlags::CLOEXEC)?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    Ok(raw)
}

#[cfg(test)]
fn read_value(path: &Path) -> std::io::Result<Value> {
    let s = read_raw(path)?;
    serde_json::from_str(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn read_authorized_intent(
    path: &Path,
    authorizer: &ActionAuthorizer,
) -> std::io::Result<(String, Value)> {
    let raw = read_raw(path)?;
    let value = verify_intent_body(authorizer, path, &raw)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
    Ok((raw, value))
}

/// Test-only legacy lock fixture for the pure file-lock test below. Production
/// state transitions use [`locked_authorized_update`] exclusively.
#[cfg(test)]
fn locked_update<F>(path: &Path, f: F) -> std::io::Result<()>
where
    F: FnOnce(&Value) -> Value,
{
    let mut file = open_intent_file(path, rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC)?;
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

fn locked_authorized_update<F>(
    path: &Path,
    authorizer: &ActionAuthorizer,
    signer: &CloudArmSigner,
    expected_target_version: &str,
    expected_initiated_at_ms: u64,
    auth_now_ms: i64,
    f: F,
) -> std::io::Result<()>
where
    F: FnOnce(&Value) -> Value,
{
    let mut file = open_intent_file(path, rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC)?;
    file.lock_exclusive()?;
    let result = (|| {
        let mut raw = String::new();
        file.read_to_string(&mut raw)?;
        let current = verify_intent_body(authorizer, path, &raw)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
        if target_version(&current) != expected_target_version
            || current["initiated_at_ms"].as_u64() != Some(expected_initiated_at_ms)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "upgrade intent changed while the package operation was running",
            ));
        }
        let next = f(&current);
        let text = sign_intent_document(path, next, signer, auth_now_ms, &fresh_nonce())?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        file.sync_all()
    })();
    let _ = FileExt::unlock(&file);
    result
}

fn rpm_installed(pkg: &str, timeout: Duration) -> bool {
    let mut command = Command::new("rpm");
    command.args(["-q", pkg]);
    output_with_timeout(command, timeout)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn rpm_version(pkg: &str, timeout: Duration) -> String {
    let mut command = Command::new("rpm");
    command.args(["-q", "--queryformat", "%{VERSION}", pkg]);
    output_with_timeout(command, timeout)
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

    const TEST_KEY: &[u8] = b"upgrade-intent-test-key";
    const TEST_NOW_MS: i64 = 1_700_000_000_000;

    fn minimal_intent(ts_ms: u64) -> Value {
        // The exact shape INST-10's `mde-update --coordinate` writes.
        json!({
            "target_version": "2.7.1",
            "initiated_by": "anvil",
            "initiated_at_ms": ts_ms,
            "ready": [],
        })
    }

    fn test_signer() -> CloudArmSigner {
        CloudArmSigner::new(TEST_KEY.to_vec()).unwrap()
    }

    #[test]
    fn write_intent_publishes_a_watcher_readable_intent() {
        // PLANES-7 — the in-tree coordinate writer lands an intent that
        // pending_intents finds, carrying the minimal watcher schema.
        let home = tempdir().unwrap();
        let signer = test_signer();
        let path = write_intent_with_signer(
            home.path(),
            "latest",
            4242,
            &signer,
            TEST_NOW_MS,
            "write-intent-valid-nonce",
            None,
            None,
        )
        .unwrap();
        assert!(path.ends_with("upgrade-intent/latest.json"));
        let found = pending_intents(&home.path().join("upgrade-intent"));
        assert_eq!(found.len(), 1);
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["target_version"], "latest");
        assert_eq!(v["initiated_at_ms"], 4242);
        assert!(v["ready"].is_object());
        // A peer hasn't acked it yet → it should act.
        assert!(should_act(&v, "anvil"));
    }

    #[test]
    fn typed_artifact_selection_is_required_for_typed_execution() {
        let home = tempdir().unwrap();
        let signer = test_signer();
        let selection = json!({
            "schema_version": 1,
            "selection_id": "selection-1",
            "target_id": "upgrade-coordinator",
            "channel": "stable",
            "artifact_digest_hex": "a".repeat(64),
            "source_revision": "revision-1",
            "signed": true,
            "unverified_build": false,
            "generation": 4242
        });
        let path = write_intent_with_signer(
            home.path(),
            "stable-1",
            4242,
            &signer,
            TEST_NOW_MS,
            "typed-selection-nonce",
            Some(selection.clone()),
            None,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let digest = "a".repeat(64);
        assert_eq!(admitted_artifact_digest(&value).as_deref(), Some(digest.as_str()));

        let mut invalid = value;
        invalid["artifact_selection"]["signed"] = Value::Bool(false);
        assert!(admitted_artifact_digest(&invalid).is_none());
    }

    #[test]
    fn write_intent_sanitizes_the_filename() {
        let home = tempdir().unwrap();
        let signer = test_signer();
        let path = write_intent_with_signer(
            home.path(),
            "v2.7.1/weird name",
            1,
            &signer,
            TEST_NOW_MS,
            "write-intent-sanitize-nonce",
            None,
            None,
        )
        .unwrap();
        // Path-unsafe chars collapse to '-'; the label is preserved inside.
        assert!(path.ends_with("upgrade-intent/v2.7.1-weird-name.json"));
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["target_version"], "v2.7.1/weird name");
    }

    #[test]
    fn signed_intent_is_readable_without_spending_its_nonce() {
        let home = tempdir().unwrap();
        let signer = test_signer();
        let path = write_intent_with_signer(
            home.path(),
            "latest",
            4242,
            &signer,
            TEST_NOW_MS,
            "read-only-poll-nonce",
            None,
            None,
        )
        .unwrap();
        let gate = ActionAuthorizer::for_test(TEST_KEY, home.path().join("auth"), TEST_NOW_MS);

        // Status/plan-style reads can poll repeatedly; verification alone does
        // not consume the capability.
        assert!(read_authorized_intent(&path, &gate).is_ok());
        assert!(read_authorized_intent(&path, &gate).is_ok());

        let raw = read_raw(&path).unwrap();
        assert!(authorize_intent_body(&gate, &path, &raw).is_ok());
        assert!(authorize_intent_body(&gate, &path, &raw)
            .unwrap_err()
            .contains("already used"));
    }

    #[test]
    fn unsigned_tampered_and_relocated_intents_fail_closed() {
        let home = tempdir().unwrap();
        let dir = home.path().join("upgrade-intent");
        fs::create_dir_all(&dir).unwrap();
        let gate = ActionAuthorizer::for_test(TEST_KEY, home.path().join("auth"), TEST_NOW_MS);

        let unsigned_path = dir.join("unsigned.json");
        fs::write(
            &unsigned_path,
            serde_json::to_string(&minimal_intent(4242)).unwrap(),
        )
        .unwrap();
        assert!(read_authorized_intent(&unsigned_path, &gate).is_err());

        let signer = test_signer();
        let signed_path = write_intent_with_signer(
            home.path(),
            "latest",
            4242,
            &signer,
            TEST_NOW_MS,
            "tamper-source-nonce",
            None,
            None,
        )
        .unwrap();
        let raw = read_raw(&signed_path).unwrap();
        let mut tampered: Value = serde_json::from_str(&raw).unwrap();
        tampered["ready"] = json!({"forge": {"at": 1, "rpm_version": "forged"}});
        fs::write(
            &signed_path,
            serde_json::to_string_pretty(&tampered).unwrap(),
        )
        .unwrap();
        assert!(read_authorized_intent(&signed_path, &gate).is_err());

        // The HMAC target includes the filename, so a valid body cannot be
        // adopted merely by moving it to another replicated intent path.
        fs::write(&signed_path, raw).unwrap();
        let relocated = dir.join("other.json");
        fs::copy(&signed_path, &relocated).unwrap();
        assert!(read_authorized_intent(&relocated, &gate).is_err());
    }

    #[cfg(unix)]
    fn executable_marker(path: &Path, marker: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(
            path,
            format!("#!/bin/sh\nprintf x > '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn only_a_valid_intent_can_reach_the_package_manager_seams() {
        let home = tempdir().unwrap();
        let mesh_home = home.path().join("mesh");
        let intent_dir = mesh_home.join("upgrade-intent");
        fs::create_dir_all(&intent_dir).unwrap();
        let dnf_marker = home.path().join("dnf-ran");
        let install_marker = home.path().join("install-ran");
        let dnf = home.path().join("dnf");
        let install = home.path().join("mde-install");
        executable_marker(&dnf, &dnf_marker);
        executable_marker(&install, &install_marker);

        let signer = test_signer();
        let now_ms = wall_now_ms_i64().unwrap();
        let valid_path = write_intent_with_signer(
            &mesh_home,
            "valid",
            now_ms as u64,
            &signer,
            now_ms,
            "valid-execution-nonce",
            None,
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        )
        .unwrap();

        // A valid intent is allowed through the exact-body gate and reaches
        // dnf; its authenticated ready transition is written back as a new
        // signed document.
        let mut valid_watcher = UpgradeIntentWatcher::new(
            home.path().join("workgroup-valid"),
            "node-valid".to_string(),
        )
        .with_test_authority(TEST_KEY, home.path().join("auth-valid"), now_ms)
        .with_alerts_dir(home.path().join("alerts-valid"));
        valid_watcher.mesh_home = mesh_home.clone();
        valid_watcher.hostname = "forge".to_string();
        valid_watcher.dnf_binary = dnf.display().to_string();
        valid_watcher.install_binary = install.display().to_string();
        valid_watcher.tick_once();
        assert!(dnf_marker.exists(), "valid intent must reach dnf");
        let (_, updated) = read_authorized_intent(&valid_path, &valid_watcher.authorizer)
            .expect("valid dnf transition is re-signed");
        assert!(updated["ready"]["forge"].is_object());

        // The hostile files live in a separate directory so this assertion is
        // independent of the valid intent's state transition above.
        let hostile_home = home.path().join("hostile-mesh");
        let hostile_dir = hostile_home.join("upgrade-intent");
        fs::create_dir_all(&hostile_dir).unwrap();
        let unsigned_path = hostile_dir.join("unsigned.json");
        fs::write(
            &unsigned_path,
            serde_json::to_string(&minimal_intent(1)).unwrap(),
        )
        .unwrap();
        let signed_path = hostile_dir.join("tampered.json");
        let signed = sign_intent_document(
            &signed_path,
            json!({
                "target_version": "tampered",
                "initiated_at_ms": 1,
                "grace_seconds": 0,
                "ready": {"forge": {"at": 1, "rpm_version": "real"}},
            }),
            &signer,
            now_ms,
            "tamper-execution-nonce",
        )
        .unwrap();
        let mut tampered: Value = serde_json::from_str(&signed).unwrap();
        tampered["ready"]["forge"]["rpm_version"] = Value::String("forged".to_string());
        fs::write(
            &signed_path,
            serde_json::to_string_pretty(&tampered).unwrap(),
        )
        .unwrap();

        let mut hostile_watcher = UpgradeIntentWatcher::new(
            home.path().join("workgroup-hostile"),
            "node-hostile".to_string(),
        )
        .with_test_authority(TEST_KEY, home.path().join("auth-hostile"), now_ms)
        .with_alerts_dir(home.path().join("alerts-hostile"));
        hostile_watcher.mesh_home = hostile_home;
        hostile_watcher.hostname = "forge".to_string();
        hostile_watcher.dnf_binary = dnf.display().to_string();
        hostile_watcher.install_binary = install.display().to_string();
        hostile_watcher.tick_once();
        assert!(
            !install_marker.exists(),
            "tampered quorum state must not reach mde-install"
        );
    }

    #[test]
    #[cfg(unix)]
    fn hostile_installer_cannot_pin_the_upgrade_worker_past_its_process_budget() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        let home = tempdir().unwrap();
        let installer = home.path().join("hostile-mde-install");
        fs::write(&installer, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&installer, fs::Permissions::from_mode(0o755)).unwrap();

        let mut watcher =
            UpgradeIntentWatcher::new(home.path().join("workgroup"), "node-test".to_string());
        watcher.install_binary = installer.display().to_string();
        watcher.command_timeout = Duration::from_millis(50);

        let started = Instant::now();
        let error = watcher
            .run_mde_install()
            .expect_err("a hostile installer must exceed the process budget");
        assert!(
            error.contains("timed out") || error.contains("timeout"),
            "the bounded process seam must report its timeout: {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the hostile installer pinned the worker past its process budget"
        );
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
    fn upgrade_alert_event_has_a_deterministic_dedupe_id() {
        let a = upgrade_alert_event("anvil", "2.7.1", "ready", "info", "staged", 100);
        // The id is stable per (version, state, host) so a re-emit dedupes.
        assert_eq!(a["id"], "upgrade-2-7-1-ready-anvil");
        assert_eq!(a["severity"], "info");
        assert_eq!(a["alert"], "upgrade_ready");
        assert_eq!(a["category"], "upgrade.transition");
        assert_eq!(a["host"], "anvil");
        // Same transition → same id (idempotent surfacing).
        let again = upgrade_alert_event("anvil", "2.7.1", "ready", "info", "x", 999);
        assert_eq!(a["id"], again["id"]);
        // A different state → a different id.
        let done = upgrade_alert_event("anvil", "2.7.1", "complete", "info", "y", 100);
        assert_ne!(a["id"], done["id"]);
    }

    #[test]
    fn emit_upgrade_alert_writes_a_relayable_event_file() {
        let tmp = tempfile::tempdir().unwrap();
        let w = UpgradeIntentWatcher::new(tmp.path().to_path_buf(), "peer:anvil".into())
            .with_alerts_dir(tmp.path().to_path_buf());
        w.emit_upgrade_alert("2.7.1", "ready", "info", "staged");
        // alert_relay can parse what we wrote.
        let files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect();
        assert_eq!(files.len(), 1, "one alert event written");
        let body = std::fs::read_to_string(files[0].path()).unwrap();
        let parsed: crate::workers::alert_relay::AlertEventPartial =
            serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.alert, "upgrade_ready");
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

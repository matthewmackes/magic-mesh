//! DATACENTER (action layer) — `action/dc/vm-power` + `action/dc/vm-snapshot`
//! + `action/dc/vm-clone` + `action/dc/vm-delete` → Xen VM control.
//!
//! The action side of the DATACENTER epic: the worker
//! ([`crate::workers::datacenter_orchestrator`]) PUBLISHES VM state; this
//! responder lets the Workbench plane ACT on it. Same dedicated-OS-thread,
//! `action/<domain>/<verb>` Bus-RPC shape as the route-trace responder
//! ([`crate::ipc::route`]) — the reads/exec are synchronous SSH calls.
//!
//! `vm-power` request body `{ "uuid", "op": "start"|"shutdown"|"reboot", "dom0" }`:
//!   * `op` maps to an `xe` verb (`start`→`vm-start`, …);
//!   * `uuid` is validated to be hex+`-` only (no command injection);
//!   * `dom0` MUST be in the configured allowed set
//!     ([`crate::workers::datacenter_orchestrator::xen_dom0s`]) before any SSH.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-snapshot` request body `{ "uuid", "dom0" }`:
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * snapshots the VM via `xe vm-snapshot uuid=<uuid> new-name-label=…`.
//! Reply `{"ok":true,"snapshot":"<new-snapshot-uuid>"}` on success (the new
//! snapshot uuid `xe` prints on stdout), `{"error":"<message>"}` on failure.
//!
//! `vm-clone` request body `{ "uuid", "dom0", "name"? }`:
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * an optional `name` is sanitized to `[A-Za-z0-9._-]` only; absent, the
//!     clone defaults to name-label `dc-clone-<first 8 chars of uuid>`;
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * clones the VM via `xe vm-clone uuid=<uuid> new-name-label=…`.
//! Reply `{"ok":true,"clone":"<new-vm-uuid>"}` on success (the new uuid `xe`
//! prints on stdout), `{"error":"<message>"}` on failure.
//!
//! `vm-delete` request body `{ "uuid", "dom0", "confirm": true }`:
//!   * `confirm` MUST be the boolean `true` — a destructive guard checked first;
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * deletes the VM via `xe vm-uninstall uuid=<uuid> force=true`.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-console` request body `{ "uuid", "dom0" }` (read-only):
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * reads the XAPI console object's `location` (the connection URL the noVNC
//!     viewer uses) via `xe console-list vm-uuid=<uuid> params=location --minimal`.
//! Reply `{"ok":true,"location":"<console URL>"}` on success; if the VM has no
//! console (halted / not running) the trimmed output is empty →
//! `{"error":"no console (vm not running?)"}`; `{"error":"<message>"}` on failure.
//!
//! `do-regions` request body ignored/empty (read-only):
//!   * runs `doctl compute region list --context <ctx> -o json` locally, where
//!     `<ctx>` is `MCNF_DOCTL_CONTEXT` (default `mackes`, the authed context);
//!   * parses the JSON array (each entry `{"slug","name","available"}`).
//! Reply `{"ok":true,"regions":[{"slug","name","available"}, …]}` on success,
//! `{"error":"doctl region list failed"}` if doctl is missing/failed.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The VM power-control responder.
///
/// Rooted at the shared workgroup root (carried for parity with the other action
/// services; the allowed-dom0 set + ssh key come from the orchestrator's
/// env-driven config).
///
/// DATACENTER-6 (op-lock half): the service also carries an in-flight op-lock —
/// a shared set of the resource keys currently being mutated (the VM `uuid` for
/// the `vm-*` mutating verbs). [`build_reply`] try-inserts the key before
/// dispatching a mutating verb and rejects a second concurrent mutation on the
/// same resource with a clear `busy` reason; a [`OpLockGuard`] removes the key
/// when the op completes (RAII). `Clone` shares the same lock (the spawn in
/// `bin/mackesd.rs` clones the service into the responder thread), so two
/// in-flight requests — even across `Clone`d handles — see one set.
#[derive(Debug, Clone)]
pub struct DatacenterService {
    // Carried for parity with the other action services and the
    // `new(workgroup_root)` spawn contract; the allowed-dom0 set + ssh key are
    // read from the orchestrator's env config, so this isn't read here yet.
    #[allow(dead_code)]
    workgroup_root: PathBuf,
    /// In-flight resource keys currently being mutated. `Arc<Mutex<…>>` so a
    /// `Clone` of the service (the responder-thread handle) shares ONE set, and
    /// so concurrent `build_reply` calls serialize on insert/remove.
    in_flight: Arc<Mutex<BTreeSet<String>>>,
}

impl DatacenterService {
    /// Build the service rooted at the shared workgroup root, with an empty
    /// in-flight op-lock set.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            in_flight: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// Try to claim `key` in the in-flight set. Returns a [`OpLockGuard`] (which
    /// releases the key on drop) when the key was free, or `None` when a mutation
    /// on the same resource is already in flight — the caller turns that into the
    /// `busy` reject. A poisoned lock is recovered (the set is plain data; a panic
    /// mid-mutation cannot leave it inconsistent), so the op-lock never wedges the
    /// responder.
    #[must_use]
    fn try_lock(&self, key: String) -> Option<OpLockGuard<'_>> {
        let mut set = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if set.insert(key.clone()) {
            Some(OpLockGuard {
                in_flight: &self.in_flight,
                key,
            })
        } else {
            None
        }
    }
}

/// RAII release for one claimed in-flight resource key: dropping it removes the
/// key from the service's in-flight set, so a panic or early return in
/// [`build_reply`] still frees the lock (the resource never gets stuck `busy`).
struct OpLockGuard<'a> {
    in_flight: &'a Arc<Mutex<BTreeSet<String>>>,
    key: String,
}

impl Drop for OpLockGuard<'_> {
    fn drop(&mut self) {
        let mut set = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set.remove(&self.key);
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 6] = [
    "vm-power",
    "vm-snapshot",
    "vm-clone",
    "vm-delete",
    "vm-console",
    "do-regions",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Build the remote `xe` argument string for a VM power op. PURE.
///
/// Maps `op` → the `xe` verb (`start`→`vm-start`, `shutdown`→`vm-shutdown`,
/// `reboot`→`vm-reboot`; any other `op` is an error) and validates `uuid` is
/// non-empty and contains only `[0-9a-fA-F-]` — this is the command-injection
/// guard, since the result is interpolated into a remote shell `xe …` string.
/// Returns e.g. `"vm-start uuid=<uuid>"`.
///
/// # Errors
/// Returns `Err` for an unknown `op`, an empty `uuid`, or a `uuid` containing any
/// character that is not an ASCII hex digit or `-`.
pub fn vm_power_command(uuid: &str, op: &str) -> Result<String, String> {
    let verb = match op {
        "start" => "vm-start",
        "shutdown" => "vm-shutdown",
        "reboot" => "vm-reboot",
        other => return Err(format!("unknown op: {other}")),
    };
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!("{verb} uuid={uuid}"))
}

/// Build the remote `xe` argument string for a VM snapshot. PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`], since the result is
/// interpolated into a remote shell `xe …` string. The new snapshot is given a
/// stable name-label `dc-snap-<first 8 chars of uuid>`. Returns e.g.
/// `"vm-snapshot uuid=<uuid> new-name-label=dc-snap-abcd1234"`.
///
/// # Errors
/// Returns `Err` for an empty `uuid`, or a `uuid` containing any character that
/// is not an ASCII hex digit or `-`.
pub fn vm_snapshot_command(uuid: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    let short: String = uuid.chars().take(8).collect();
    Ok(format!(
        "vm-snapshot uuid={uuid} new-name-label=dc-snap-{short}"
    ))
}

/// Build the remote `xe` argument string for a VM clone. PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`], since the result is
/// interpolated into a remote shell `xe …` string. The new clone's name-label is
/// either the caller-supplied `name` (sanitized to `[A-Za-z0-9._-]` only — any
/// other character is rejected) or, when absent, the stable default
/// `dc-clone-<first 8 chars of uuid>`. Returns e.g.
/// `"vm-clone uuid=<uuid> new-name-label=dc-clone-abcd1234"`.
///
/// # Errors
/// Returns `Err` for an empty `uuid`, a `uuid` containing any character that is
/// not an ASCII hex digit or `-`, or a supplied `name` that is empty or contains
/// any character outside `[A-Za-z0-9._-]`.
pub fn vm_clone_command(uuid: &str, name: Option<&str>) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    let label = match name {
        Some(n) => {
            if n.is_empty() {
                return Err("empty name".into());
            }
            if !n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err("name contains invalid characters".into());
            }
            n.to_string()
        }
        None => {
            let short: String = uuid.chars().take(8).collect();
            format!("dc-clone-{short}")
        }
    };
    Ok(format!("vm-clone uuid={uuid} new-name-label={label}"))
}

/// Run a remote `xe` command on a dom0 over SSH, returning the process result.
/// Mirrors the exact ssh arg style of `ssh_xe` in the orchestrator.
fn ssh_xe_status(key: &str, dom0: &str, remote: &str) -> std::io::Result<std::process::Output> {
    std::process::Command::new("ssh")
        .args([
            "-i",
            key,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &format!("root@{dom0}"),
            remote,
        ])
        .output()
}

/// The resource key a mutating `verb` op-locks, or `None` for a read-only verb
/// that needs no lock. PURE (used by [`build_reply`]'s op-lock and unit-testable
/// on its own).
///
/// DATACENTER-6 (op-lock half): a second concurrent mutation on the same resource
/// is rejected, so the lock is keyed on the resource the verb targets — every
/// mutating verb THIS responder dispatches targets a single VM, so the key is the
/// VM `uuid`, namespaced `vm:<uuid>`:
/// * `vm-power` / `vm-snapshot` / `vm-clone` / `vm-delete` → `vm:<uuid>`;
/// * the read-only verbs `do-regions` and `vm-console` return `None` — they read,
///   never mutate, so concurrent reads are allowed.
///
/// (The `host-power` host op runs on its own separate responder
/// [`crate::ipc::host_ops`] and is not dispatched here, so it is intentionally
/// not in this key space — the `vm:` namespace prefix leaves room for a future
/// `host:<dom0>` key without collision.)
///
/// A verb whose body is missing/unparseable, or whose `uuid` is empty, also
/// returns `None`: there is no resource to lock, and the per-verb handler will
/// produce the real validation error. The key is NOT validated for injection here
/// (that is the per-verb command builder's job); it is only ever used as a set
/// member, never interpolated into a shell command.
#[must_use]
pub fn lock_key(verb: &str, req_body: Option<&str>) -> Option<String> {
    // Only the mutating vm-* verbs lock; read-only verbs hold no lock.
    match verb {
        "vm-power" | "vm-snapshot" | "vm-clone" | "vm-delete" => {}
        _ => return None,
    }
    let uuid = serde_json::from_str::<serde_json::Value>(req_body?)
        .ok()?
        .get("uuid")
        .and_then(|v| v.as_str())
        .map(str::to_string)?;
    if uuid.is_empty() {
        return None;
    }
    Some(format!("vm:{uuid}"))
}

/// Build the reply for one `action/dc/<verb>` request, dispatching on `verb`.
///
/// DATACENTER-6 (op-lock half): before dispatching a *mutating* verb, the resource
/// key ([`lock_key`]) is claimed in the service's in-flight set. If a mutation on
/// the same resource is already in flight, this returns the clear `busy` reject
/// WITHOUT running the op; otherwise a [`OpLockGuard`] holds the key for the
/// duration of the (synchronous) dispatch and releases it on return (RAII).
/// Read-only verbs ([`lock_key`] → `None`) take no lock and never reject.
#[must_use]
pub fn build_reply(svc: &DatacenterService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    // Op-lock: claim the resource for the duration of a mutating dispatch. The
    // guard is dropped at the end of this function (after the reply is built),
    // releasing the key. Read-only verbs (lock_key → None) are unguarded.
    let _guard = match lock_key(verb, req_body) {
        Some(key) => match svc.try_lock(key.clone()) {
            Some(g) => Some(g),
            None => {
                return err(format!(
                    "resource {key} busy: a {verb} is already in flight"
                ));
            }
        },
        None => None,
    };
    match verb {
        "vm-power" => vm_power_reply(req_body),
        "vm-snapshot" => vm_snapshot_reply(req_body),
        "vm-clone" => vm_clone_reply(req_body),
        "vm-delete" => vm_delete_reply(req_body),
        "vm-console" => vm_console_reply(req_body),
        "do-regions" => do_regions_reply(),
        _ => err("unknown dc verb".into()),
    }
}

/// Handle a `vm-power` request body: parse, allow-list the dom0, then run the
/// mapped `xe` power verb over SSH.
fn vm_power_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-power: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-power: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_power_command(uuid, op) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => json!({ "ok": true }).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-snapshot` request body: parse, allow-list the dom0, then run
/// `xe vm-snapshot …` over SSH. On success `xe` prints the new snapshot uuid on
/// stdout, which is returned as `{"ok":true,"snapshot":"<uuid>"}`.
fn vm_snapshot_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-snapshot: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-snapshot: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_snapshot_command(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let snapshot = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "snapshot": snapshot }).to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-clone` request body: parse, allow-list the dom0, then run
/// `xe vm-clone …` over SSH. On success `xe` prints the new clone's uuid on
/// stdout, which is returned as `{"ok":true,"clone":"<uuid>"}`.
fn vm_clone_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-clone: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-clone: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let name = req.get("name").and_then(serde_json::Value::as_str);

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_clone_command(uuid, name) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let clone = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "clone": clone }).to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-delete` request body: parse, REQUIRE `confirm == true`,
/// allow-list the dom0, then run `xe vm-uninstall uuid=<uuid> force=true` over
/// SSH. Reply `{"ok":true}` on success.
fn vm_delete_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-delete: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-delete: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // DESTRUCTIVE: refuse unless the caller explicitly confirms. Checked BEFORE
    // the dom0 allow-list and before building/running anything.
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return err("delete requires confirm:true".into());
    }

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_uninstall_command(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => json!({ "ok": true }).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Build the remote `xe` argument string for a VM uninstall (delete). PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`]. Returns
/// `"vm-uninstall uuid=<uuid> force=true"`.
///
/// # Errors
/// Returns `Err` for an empty `uuid`, or a `uuid` containing any character that
/// is not an ASCII hex digit or `-`.
pub fn vm_uninstall_command(uuid: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!("vm-uninstall uuid={uuid} force=true"))
}

/// Build the remote `xe` argument string for reading a VM's console location. PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`], since the result is
/// interpolated into a remote shell `xe …` string. Returns
/// `"console-list vm-uuid=<uuid> params=location --minimal"` — `--minimal` prints
/// just the console object's `location` (the connection URL the noVNC viewer uses).
///
/// # Errors
/// Returns `Err` for an empty `uuid`, or a `uuid` containing any character that is
/// not an ASCII hex digit or `-`.
pub fn vm_console_command(uuid: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!(
        "console-list vm-uuid={uuid} params=location --minimal"
    ))
}

/// Handle a `vm-console` request body: parse, allow-list the dom0, then read the
/// XAPI console `location` over SSH (read-only). On success the trimmed stdout is
/// the connection URL; an empty result means the VM has no console (halted / not
/// running), reported as `{"error":"no console (vm not running?)"}`.
fn vm_console_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-console: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-console: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything. Read-only,
    // so there is no confirm gate, but we still SSH there → keep the allow-list.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_console_command(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let location = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if location.is_empty() {
                err("no console (vm not running?)".into())
            } else {
                json!({ "ok": true, "location": location }).to_string()
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Parse a `doctl compute region list -o json` array into `(slug, name, available)`
/// triples. PURE.
///
/// Each array element is expected to be an object with string `slug`/`name` and a
/// boolean `available`. Missing string fields default to empty, a missing/non-bool
/// `available` defaults to `false`. Non-array or unparsable input yields an empty
/// vector (best-effort — the caller turns that into the doctl-failed error).
#[must_use]
pub fn parse_regions(json: &str) -> Vec<(String, String, bool)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|r| {
            let slug = r
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = r
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let available = r
                .get("available")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            (slug, name, available)
        })
        .collect()
}

/// Handle a `do-regions` request: run `doctl compute region list` (read-only) and
/// reply with the parsed regions. The doctl context is `MCNF_DOCTL_CONTEXT`
/// (default `mackes`). Best-effort: doctl missing/failed → the doctl-failed error.
fn do_regions_reply() -> String {
    let err = |m: &str| json!({ "error": m }).to_string();
    let context = std::env::var("MCNF_DOCTL_CONTEXT").unwrap_or_else(|_| "mackes".to_string());
    let output = std::process::Command::new("doctl")
        .args([
            "compute",
            "region",
            "list",
            "--context",
            &context,
            "-o",
            "json",
        ])
        .output();
    let Ok(out) = output else {
        return err("doctl region list failed");
    };
    if !out.status.success() {
        return err("doctl region list failed");
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let regions: Vec<serde_json::Value> = parse_regions(&stdout)
        .into_iter()
        .map(
            |(slug, name, available)| json!({ "slug": slug, "name": name, "available": available }),
        )
        .collect();
    json!({ "ok": true, "regions": regions }).to_string()
}

/// Run the datacenter Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DatacenterService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(
    persist: &Persist,
    svc: &DatacenterService,
    cursors: &mut HashMap<String, String>,
) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "dc responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("vm-power"), "action/dc/vm-power");
        assert_eq!(action_topic("vm-snapshot"), "action/dc/vm-snapshot");
        assert_eq!(action_topic("vm-clone"), "action/dc/vm-clone");
        assert_eq!(action_topic("vm-delete"), "action/dc/vm-delete");
        assert_eq!(action_topic("vm-console"), "action/dc/vm-console");
        assert_eq!(action_topic("do-regions"), "action/dc/do-regions");
        assert!(ACTION_VERBS.contains(&"vm-power"));
        assert!(ACTION_VERBS.contains(&"vm-snapshot"));
        assert!(ACTION_VERBS.contains(&"vm-clone"));
        assert!(ACTION_VERBS.contains(&"vm-delete"));
        assert!(ACTION_VERBS.contains(&"vm-console"));
        assert!(ACTION_VERBS.contains(&"do-regions"));
    }

    #[test]
    fn parse_regions_parses_doctl_json() {
        let json = r#"[
            {"slug":"nyc3","name":"New York 3","available":true,"sizes":["s-1vcpu-1gb"]},
            {"slug":"ams2","name":"Amsterdam 2","available":false}
        ]"#;
        let regions = parse_regions(json);
        assert_eq!(
            regions,
            vec![
                ("nyc3".to_string(), "New York 3".to_string(), true),
                ("ams2".to_string(), "Amsterdam 2".to_string(), false),
            ]
        );
    }

    #[test]
    fn parse_regions_garbage_is_empty() {
        assert!(parse_regions("not json at all").is_empty());
        // valid JSON but not an array
        assert!(parse_regions(r#"{"slug":"nyc3"}"#).is_empty());
        // empty array
        assert!(parse_regions("[]").is_empty());
    }

    #[test]
    fn vm_power_command_maps_each_valid_op() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_power_command(uuid, "start").unwrap(),
            format!("vm-start uuid={uuid}")
        );
        assert_eq!(
            vm_power_command(uuid, "shutdown").unwrap(),
            format!("vm-shutdown uuid={uuid}")
        );
        assert_eq!(
            vm_power_command(uuid, "reboot").unwrap(),
            format!("vm-reboot uuid={uuid}")
        );
    }

    #[test]
    fn vm_power_command_unknown_op_errors() {
        assert!(vm_power_command("abcd-1234", "destroy").is_err());
        assert!(vm_power_command("abcd-1234", "").is_err());
    }

    #[test]
    fn vm_power_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_power_command("", "start").is_err());
        // a `;` to chain a second command
        assert!(vm_power_command("abcd;rm -rf /", "start").is_err());
        // a space (would split into extra args)
        assert!(vm_power_command("abcd 1234", "start").is_err());
        // backtick / non-hex
        assert!(vm_power_command("abcd`whoami`", "start").is_err());
        assert!(vm_power_command("ghij", "start").is_err());
    }

    #[test]
    fn vm_snapshot_command_builds_labelled_snapshot() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_snapshot_command(uuid).unwrap(),
            // name-label uses the first 8 chars of the uuid
            format!("vm-snapshot uuid={uuid} new-name-label=dc-snap-abcd1234")
        );
        // a uuid shorter than 8 chars uses whatever is there (still hex+dash)
        assert_eq!(
            vm_snapshot_command("ab-12").unwrap(),
            "vm-snapshot uuid=ab-12 new-name-label=dc-snap-ab-12"
        );
    }

    #[test]
    fn vm_snapshot_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_snapshot_command("").is_err());
        // a `;` to chain a second command
        assert!(vm_snapshot_command("abcd;rm -rf /").is_err());
        // a space (would split into extra args)
        assert!(vm_snapshot_command("abcd 1234").is_err());
        // backtick / command substitution
        assert!(vm_snapshot_command("abcd`whoami`").is_err());
        // non-hex letters
        assert!(vm_snapshot_command("ghij").is_err());
        // a `=` that could inject an extra xe arg
        assert!(vm_snapshot_command("abcd=evil").is_err());
    }

    #[test]
    fn vm_clone_command_default_label_uses_uuid_prefix() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_clone_command(uuid, None).unwrap(),
            // default name-label uses the first 8 chars of the uuid
            format!("vm-clone uuid={uuid} new-name-label=dc-clone-abcd1234")
        );
        // a uuid shorter than 8 chars uses whatever is there (still hex+dash)
        assert_eq!(
            vm_clone_command("ab-12", None).unwrap(),
            "vm-clone uuid=ab-12 new-name-label=dc-clone-ab-12"
        );
    }

    #[test]
    fn vm_clone_command_uses_sanitized_supplied_name() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_clone_command(uuid, Some("my.vm_clone-1")).unwrap(),
            format!("vm-clone uuid={uuid} new-name-label=my.vm_clone-1")
        );
    }

    #[test]
    fn vm_clone_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_clone_command("", None).is_err());
        // a `;` to chain a second command
        assert!(vm_clone_command("abcd;rm -rf /", None).is_err());
        // a space (would split into extra args)
        assert!(vm_clone_command("abcd 1234", None).is_err());
        // backtick / command substitution
        assert!(vm_clone_command("abcd`whoami`", None).is_err());
        // non-hex letters
        assert!(vm_clone_command("ghij", None).is_err());
    }

    #[test]
    fn vm_clone_command_rejects_unsafe_name() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        // empty name
        assert!(vm_clone_command(uuid, Some("")).is_err());
        // space would split into extra xe args
        assert!(vm_clone_command(uuid, Some("evil name")).is_err());
        // `;` chains a command
        assert!(vm_clone_command(uuid, Some("a;rm -rf /")).is_err());
        // `=` could inject an extra xe arg
        assert!(vm_clone_command(uuid, Some("a=b")).is_err());
        // backtick / command substitution
        assert!(vm_clone_command(uuid, Some("a`whoami`")).is_err());
    }

    #[test]
    fn vm_uninstall_command_builds_forced_uninstall() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_uninstall_command(uuid).unwrap(),
            format!("vm-uninstall uuid={uuid} force=true")
        );
    }

    #[test]
    fn vm_uninstall_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_uninstall_command("").is_err());
        // a `;` to chain a second command
        assert!(vm_uninstall_command("abcd;rm -rf /").is_err());
        // a space (would split into extra args)
        assert!(vm_uninstall_command("abcd 1234").is_err());
        // backtick / command substitution
        assert!(vm_uninstall_command("abcd`whoami`").is_err());
        // non-hex letters
        assert!(vm_uninstall_command("ghij").is_err());
    }

    #[test]
    fn vm_console_command_builds_location_query() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_console_command(uuid).unwrap(),
            format!("console-list vm-uuid={uuid} params=location --minimal")
        );
        // a uuid shorter than 8 chars still works (hex+dash only)
        assert_eq!(
            vm_console_command("ab-12").unwrap(),
            "console-list vm-uuid=ab-12 params=location --minimal"
        );
    }

    #[test]
    fn vm_console_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_console_command("").is_err());
        // a `;` to chain a second command
        assert!(vm_console_command("abcd;rm -rf /").is_err());
        // a space (would split into extra args)
        assert!(vm_console_command("abcd 1234").is_err());
        // backtick / command substitution
        assert!(vm_console_command("abcd`whoami`").is_err());
        // non-hex letters
        assert!(vm_console_command("ghij").is_err());
        // a `=` that could inject an extra xe arg
        assert!(vm_console_command("abcd=evil").is_err());
    }

    #[test]
    fn vm_console_dom0_not_in_allowed_set_is_rejected() {
        // Read-only, but still SSHes → the dom0 allow-list guard applies. With
        // MCNF_XEN_DOM0S unset the allowed set is empty, so the dom0 is rejected
        // before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-console", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_delete_requires_confirm_true() {
        // The confirm gate is checked BEFORE the dom0 allow-list, so even with an
        // empty allowed set the missing/false confirm is what we observe.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // confirm missing
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
        // confirm false
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": false
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
        // confirm as a non-bool ("true" string) does not satisfy the gate
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": "true"
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
    }

    #[test]
    fn vm_delete_confirmed_then_checks_dom0_allow_list() {
        // With confirm:true the gate passes and the dom0 allow-list is the next
        // guard — with MCNF_XEN_DOM0S unset the allowed set is empty, so the
        // (unlisted) dom0 is rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": true
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_clone_dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-clone", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "vm-power", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-snapshot", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-clone", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-delete", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-console", None).contains("missing request body"));
    }

    #[test]
    fn dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "op": "start",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-power", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_snapshot_dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-snapshot", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    // ---- DATACENTER-6: per-resource op-lock ----

    #[test]
    fn lock_key_maps_mutating_verbs_to_namespaced_resource() {
        let body = json!({ "uuid": "abcd-1234", "op": "start", "dom0": "10.0.0.1" }).to_string();
        // every vm-* mutating verb keys on the vm uuid (namespaced)
        for verb in ["vm-power", "vm-snapshot", "vm-clone", "vm-delete"] {
            assert_eq!(
                lock_key(verb, Some(&body)),
                Some("vm:abcd-1234".to_string()),
                "verb {verb} should lock on the vm uuid"
            );
        }
    }

    #[test]
    fn lock_key_read_only_and_unlockable_return_none() {
        let body = json!({ "uuid": "abcd-1234", "dom0": "10.0.0.1" }).to_string();
        // read-only verbs take no lock
        assert_eq!(lock_key("vm-console", Some(&body)), None);
        assert_eq!(lock_key("do-regions", Some(&body)), None);
        // unknown verb → no lock
        assert_eq!(lock_key("bogus", Some(&body)), None);
        // mutating verb but nothing to lock on → no lock (the handler emits the
        // real validation error instead of us inventing an empty key)
        assert_eq!(lock_key("vm-power", None), None);
        assert_eq!(lock_key("vm-power", Some("not json")), None);
        assert_eq!(lock_key("vm-power", Some(r#"{"op":"start"}"#)), None);
        assert_eq!(lock_key("vm-power", Some(r#"{"uuid":""}"#)), None);
    }

    #[test]
    fn second_concurrent_mutation_on_same_uuid_is_busy_rejected() {
        // Two concurrent vm-power on the SAME uuid: model the first being still
        // in flight by holding its op-lock guard, then issue the second through
        // build_reply. The second must be rejected with the clear busy reason,
        // and crucially WITHOUT reaching the dom0 allow-list (the lock is the
        // first gate). A different uuid is unaffected.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";

        // First op still running: hold the guard for vm:<uuid>.
        let held = s
            .try_lock(format!("vm:{uuid}"))
            .expect("first claim succeeds on a free resource");

        // Second vm-power on the same uuid → busy-reject, NOT the dom0 error.
        let body = json!({ "uuid": uuid, "op": "start", "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "vm-power", Some(&body));
        assert!(
            r.contains(&format!("resource vm:{uuid} busy")),
            "expected busy reject, got: {r}"
        );
        assert!(
            r.contains("a vm-power is already in flight"),
            "expected the clear reason, got: {r}"
        );
        assert!(
            !r.contains("dom0 not in allowed set"),
            "lock must gate BEFORE the dom0 check: {r}"
        );

        // A DIFFERENT uuid is not locked → it proceeds to the next gate (the
        // empty dom0 allow-list), proving the lock is per-resource.
        let other = json!({
            "uuid": "ffff0000-1111-2222-3333-444455556666",
            "op": "start",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r2 = build_reply(&s, "vm-power", Some(&other));
        assert!(r2.contains("dom0 not in allowed set"), "{r2}");

        // Release the first op; the same uuid is now claimable again.
        drop(held);
        assert!(
            s.try_lock(format!("vm:{uuid}")).is_some(),
            "the resource is free again after the first op completes"
        );
    }

    #[test]
    fn op_lock_releases_after_a_completed_dispatch() {
        // build_reply's guard is dropped when it returns, so back-to-back (not
        // overlapping) mutations on the same uuid both run — the lock only blocks
        // CONCURRENT ones. With an empty dom0 set both hit the allow-list error,
        // proving neither was spuriously busy-rejected.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "op": "start",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r1 = build_reply(&s, "vm-power", Some(&body));
        assert!(r1.contains("dom0 not in allowed set"), "{r1}");
        let r2 = build_reply(&s, "vm-power", Some(&body));
        assert!(
            r2.contains("dom0 not in allowed set"),
            "lock must have released after the first call: {r2}"
        );
    }

    #[test]
    fn op_lock_is_shared_across_cloned_handles() {
        // The responder thread gets a Clone of the service; the op-lock must be
        // shared (Arc), so a resource claimed on one handle is busy on its clone.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        let _held = s.try_lock(format!("vm:{uuid}")).expect("claim on original");
        let clone = s.clone();
        let body = json!({ "uuid": uuid, "op": "reboot", "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&clone, "vm-power", Some(&body));
        assert!(
            r.contains(&format!("resource vm:{uuid} busy")),
            "a clone must see the same in-flight set: {r}"
        );
    }

    #[test]
    fn read_only_verb_is_never_busy_rejected() {
        // vm-console is read-only: even with the same uuid "in flight" it is not
        // gated by the lock (concurrent reads are fine). It falls through to its
        // own dom0 allow-list check.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        let _held = s.try_lock(format!("vm:{uuid}")).expect("claim the uuid");
        let body = json!({ "uuid": uuid, "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "vm-console", Some(&body));
        assert!(
            !r.contains("busy"),
            "read-only verb must not be locked: {r}"
        );
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }
}

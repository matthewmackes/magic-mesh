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
    // The shared workgroup root, used by `vm-create` as the repo root the relative
    // `infra/tofu/xen-xapi` dir is resolved against (DATACENTER-11, Tofu-backed
    // create — same convention as the tofu responder). The allowed-dom0 set + ssh
    // key still come from the orchestrator's env config.
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
///
/// DATACENTER-19 adds `do-guided-lighthouse` (the droplet→bootstrap→found/join→DNS
/// orchestration) and DATACENTER-20 adds `promote-arm` (the Build→Eagle→DO `do`
/// step gate) + `promote-now` (trigger a stage promotion) to the DO surface.
pub const ACTION_VERBS: [&str; 14] = [
    "vm-power",
    "vm-snapshot",
    "vm-clone",
    "vm-delete",
    "vm-console",
    "do-regions",
    // DATACENTER-11 — full VM lifecycle: suspend/resume + live-migrate + bulk +
    // golden-template create (Tofu-backed).
    "vm-suspend",
    "vm-resume",
    "vm-migrate",
    "vm-create",
    "vm-bulk",
    "do-guided-lighthouse",
    "promote-arm",
    "promote-now",
];

/// Whether `verb` MUTATES infrastructure (so it is RBAC-gated to `operator` and
/// op-lock-eligible). The read-only verbs (`vm-console`, `do-regions`) return
/// `false`; everything else — including an unknown verb, which is rejected later
/// — is treated as mutating. PURE.
#[must_use]
pub fn is_mutating(verb: &str) -> bool {
    !matches!(verb, "vm-console" | "do-regions")
}

/// True iff `dom0` is in the configured allowed set
/// ([`crate::workers::datacenter_orchestrator::xen_dom0s`]). The SECURITY guard
/// every responder applies before SSHing a host.
#[must_use]
fn dom0_allowed(dom0: &str) -> bool {
    crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
}

/// Validate a VM `uuid` is non-empty and hex+`-` only — the command-injection
/// guard shared by the new lifecycle command builders (same rule as
/// [`vm_power_command`]). PURE.
///
/// # Errors
/// Returns `Err` for an empty `uuid` or any character that is not an ASCII hex
/// digit or `-`.
fn validate_vm_uuid(uuid: &str) -> Result<(), String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(())
}

/// Run `xe <cmd>` on an already-allow-listed `dom0` and map the result to the
/// standard `{"ok":true}` / `{"error":...}` reply. Shared by the simple mutating
/// VM verbs (suspend/resume/migrate) so their exec + error mapping can't drift.
fn run_xe_ok(dom0: &str, cmd: &str) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
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
    // Only the single-VM mutating verbs lock on their `uuid`; read-only verbs and
    // the multi-target verbs (vm-bulk has many uuids, vm-create has none yet) hold
    // no per-uuid lock here.
    match verb {
        "vm-power" | "vm-snapshot" | "vm-clone" | "vm-delete" | "vm-suspend" | "vm-resume"
        | "vm-migrate" => {}
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
    // RBAC (design §9): a mutating verb requires the caller's mesh principal to map
    // to `operator`; a `viewer` is rejected BEFORE the op-lock / dom0 allow-list /
    // any SSH. Read-only verbs are always allowed. Checked first so a viewer's
    // write never touches the substrate; a denial is also audited (DATACENTER-7).
    if let Err(m) = crate::ipc::dc_rbac::authorize(req_body, is_mutating(verb)) {
        crate::ipc::dc_rbac::audit_denial(verb, req_body, &m);
        return err(m);
    }
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
        "vm-suspend" => vm_suspend_reply(req_body),
        "vm-resume" => vm_resume_reply(req_body),
        "vm-migrate" => vm_migrate_reply(req_body),
        "vm-create" => vm_create_reply(svc, req_body),
        "vm-bulk" => vm_bulk_reply(req_body),
        "do-guided-lighthouse" => do_guided_lighthouse_reply(svc, req_body),
        "promote-arm" => promote_arm_reply(svc, req_body),
        "promote-now" => promote_now_reply(svc, req_body),
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

// ───────────────────── DATACENTER-11 — full VM lifecycle ─────────────────────

/// Build the remote `xe` argument string for a VM suspend. PURE.
///
/// Validates `uuid` ([`validate_vm_uuid`]) then reuses the canonical argv builder
/// [`mackes_xcp::argv_suspend`] (joined to the `xe`-remote string shape this
/// responder runs). Returns `"vm-suspend uuid=<uuid>"`.
///
/// # Errors
/// Returns `Err` for an empty `uuid` or a `uuid` with non-`[0-9a-fA-F-]` chars.
pub fn vm_suspend_command(uuid: &str) -> Result<String, String> {
    validate_vm_uuid(uuid)?;
    Ok(mackes_xcp::argv_suspend(uuid).join(" "))
}

/// Build the remote `xe` argument string for a VM resume. PURE.
/// Reuses [`mackes_xcp::argv_resume`]; same `uuid` guard as [`vm_suspend_command`].
///
/// # Errors
/// Returns `Err` for an empty / non-hex `uuid`.
pub fn vm_resume_command(uuid: &str) -> Result<String, String> {
    validate_vm_uuid(uuid)?;
    Ok(mackes_xcp::argv_resume(uuid).join(" "))
}

/// Build the remote `xe` argument string for a VM live-migrate. PURE.
///
/// Validates `uuid` ([`validate_vm_uuid`]) and `target_host` (non-empty,
/// `[A-Za-z0-9._-]` only — a host name-label or uuid, never a shell metachar),
/// then reuses [`mackes_xcp::argv_migrate`]. Returns
/// `"vm-migrate uuid=<uuid> host=<target_host> live=true"`.
///
/// # Errors
/// Returns `Err` for an empty / non-hex `uuid`, or a `target_host` that is empty
/// or carries a character outside `[A-Za-z0-9._-]`.
pub fn vm_migrate_command(uuid: &str, target_host: &str) -> Result<String, String> {
    validate_vm_uuid(uuid)?;
    if target_host.is_empty() {
        return Err("empty target_host".into());
    }
    if !target_host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("target_host contains invalid characters".into());
    }
    Ok(mackes_xcp::argv_migrate(uuid, target_host).join(" "))
}

/// Parse `{uuid, dom0}`, allow-list the dom0, build `cmd`, and run it. Shared body
/// of the suspend/resume replies (identical but for the command builder).
fn vm_simple_reply(
    verb: &str,
    req_body: Option<&str>,
    build: impl Fn(&str) -> Result<String, String>,
) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err(format!("{verb}: missing request body"));
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("{verb}: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !dom0_allowed(dom0) {
        return err("dom0 not in allowed set".into());
    }
    let cmd = match build(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe_ok(dom0, &cmd)
}

/// Handle a `vm-suspend` request: parse, allow-list the dom0, `xe vm-suspend`.
fn vm_suspend_reply(req_body: Option<&str>) -> String {
    vm_simple_reply("vm-suspend", req_body, vm_suspend_command)
}

/// Handle a `vm-resume` request: parse, allow-list the dom0, `xe vm-resume`.
fn vm_resume_reply(req_body: Option<&str>) -> String {
    vm_simple_reply("vm-resume", req_body, vm_resume_command)
}

/// Handle a `vm-migrate` request `{ uuid, dom0, target_host }`: parse, allow-list
/// the dom0, then `xe vm-migrate uuid=<uuid> host=<target_host> live=true`.
fn vm_migrate_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-migrate: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-migrate: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let target_host = req
        .get("target_host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !dom0_allowed(dom0) {
        return err("dom0 not in allowed set".into());
    }
    let cmd = match vm_migrate_command(uuid, target_host) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe_ok(dom0, &cmd)
}

/// Map a `vm-bulk` `op` to the `xe` verb run per uuid. PURE.
///
/// # Errors
/// Returns `Err` for an `op` outside the supported lifecycle set.
pub fn vm_bulk_op_verb(op: &str) -> Result<&'static str, String> {
    match op {
        "start" => Ok("vm-start"),
        "shutdown" => Ok("vm-shutdown"),
        "reboot" => Ok("vm-reboot"),
        "suspend" => Ok("vm-suspend"),
        "resume" => Ok("vm-resume"),
        other => Err(format!("unknown bulk op: {other}")),
    }
}

/// Handle a `vm-bulk` request `{ uuids:[…], op, dom0 }`: run the mapped `xe` verb
/// against each uuid on the allow-listed dom0, collecting a per-uuid result so one
/// bad VM doesn't sink the batch. Reply
/// `{"ok":true,"results":[{"uuid","ok":true}|{"uuid","error":"…"}]}`.
fn vm_bulk_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-bulk: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-bulk: bad json: {e}")),
    };
    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let uuids: Vec<String> = req
        .get("uuids")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if !dom0_allowed(dom0) {
        return err("dom0 not in allowed set".into());
    }
    let verb = match vm_bulk_op_verb(op) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    if uuids.is_empty() {
        return err("vm-bulk: empty uuids".into());
    }

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(uuids.len());
    for uuid in &uuids {
        if let Err(e) = validate_vm_uuid(uuid) {
            results.push(json!({ "uuid": uuid, "error": e }));
            continue;
        }
        let remote = format!("xe {verb} uuid={uuid}");
        match ssh_xe_status(&key, dom0, &remote) {
            Ok(o) if o.status.success() => results.push(json!({ "uuid": uuid, "ok": true })),
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = stderr.trim();
                let msg = if msg.is_empty() { "xe failed" } else { msg };
                results.push(json!({ "uuid": uuid, "error": msg }));
            }
            Err(e) => results.push(json!({ "uuid": uuid, "error": format!("ssh failed: {e}") })),
        }
    }
    json!({ "ok": true, "results": results }).to_string()
}

/// Map a `vm-create` `zone` to the `xen-xapi` Tofu provider alias for the target
/// pool. PURE. The three standalone XCP-ng pools each have an aliased provider
/// (`xhs`/`kvm`/`big`, see `infra/tofu/xen-xapi/providers.tf`); `dev` defaults to
/// `xhs` (the founding pool).
///
/// # Errors
/// Returns `Err` for any `zone` outside the known pool aliases.
pub fn vm_create_provider_alias(zone: &str) -> Result<&'static str, String> {
    match zone {
        "dev" | "xhs" => Ok("xhs"),
        "kvm" => Ok("kvm"),
        "big" => Ok("big"),
        other => Err(format!("unknown zone/pool: {other}")),
    }
}

/// Derive a Tofu resource label from a VM `name`: lower-cased, every char outside
/// `[a-z0-9_]` collapsed to `_`, prefixed `dc_` so it is always a valid HCL
/// identifier (which may not start with a digit) and namespaced away from the
/// hand-written `build_*` resources. PURE.
///
/// # Errors
/// Returns `Err` for an empty `name` or one with no alphanumeric character (the
/// label would be all separators).
pub fn tofu_resource_label(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("empty name".into());
    }
    if !trimmed.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Err("name has no alphanumeric character".into());
    }
    let sanitized: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    Ok(format!("dc_{sanitized}"))
}

/// Render the `xenserver_vm` HCL resource block a `vm-create` writes into the
/// `xen-xapi` workspace (DATACENTER-11 — structural change via Tofu, no drift).
/// PURE so the rendered HCL is testable without writing a file or running tofu.
///
/// Mirrors the hand-written `build_*` resources (`infra/tofu/xen-xapi/build-vms.tf`)
/// exactly — the same `lifecycle.ignore_changes` set that lets an adopted/golden
/// clone plan clean against the early-stage 0.2.x provider — so a created VM does
/// not churn drift on the next plan. All interpolated values are pre-validated by
/// the caller ([`vm_create_reply`]).
#[must_use]
pub fn render_xenserver_vm_hcl(
    label: &str,
    name: &str,
    alias: &str,
    template: &str,
    network_uuid: &str,
    vcpus: u32,
    mem_bytes: u64,
) -> String {
    format!(
        "# DATACENTER-11 — created from the Workbench golden-template wizard.\n\
         resource \"xenserver_vm\" \"{label}\" {{\n\
         \x20\x20provider          = xenserver.{alias}\n\
         \x20\x20name_label        = \"{name}\"\n\
         \x20\x20template_name     = \"{template}\"\n\
         \x20\x20static_mem_max    = {mem_bytes}\n\
         \x20\x20vcpus             = {vcpus}\n\
         \x20\x20check_ip_timeout  = 0\n\
         \x20\x20network_interface = [{{ device = \"0\", network_uuid = \"{network_uuid}\" }}]\n\
         \x20\x20lifecycle {{\n\
         \x20\x20\x20\x20ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]\n\
         \x20\x20}}\n\
         }}\n"
    )
}

/// Handle a `vm-create` request
/// `{ template, name, zone, network_uuid, vcpus?, mem_mb? }`: render a
/// `xenserver_vm` resource, write it into the `xen-xapi` Tofu workspace, then
/// `tofu apply` so the structural change goes through IaC (no drift). Reply
/// `{"ok":true,"file":"<path>","summary":"<tofu output>"}` on success.
///
/// `vcpus` defaults to 2 and `mem_mb` to 4096 when absent; both are bounded. The
/// repo root is the service's `workgroup_root` (same convention as the tofu
/// responder).
fn vm_create_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-create: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-create: bad json: {e}")),
    };
    let name = req
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let template = req
        .get("template")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("MDE-VM-golden");
    let zone = req
        .get("zone")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("dev");
    let network_uuid = req
        .get("network_uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let vcpus = req
        .get("vcpus")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(2);
    let mem_mb = req
        .get("mem_mb")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(4096);

    // Validate every interpolated value BEFORE rendering/writing/applying.
    // name/template are name-labels → `[A-Za-z0-9._-]`; network_uuid is hex+dash;
    // the label is derived + sanitized; the pool alias is allow-listed.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || name.is_empty()
    {
        return err("name must be non-empty [A-Za-z0-9._-]".into());
    }
    if !template
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || template.is_empty()
    {
        return err("template must be non-empty [A-Za-z0-9._-]".into());
    }
    if network_uuid.is_empty()
        || !network_uuid
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return err("network_uuid must be a non-empty hex+dash uuid".into());
    }
    if !(1..=64).contains(&vcpus) {
        return err("vcpus must be 1..=64".into());
    }
    if !(512..=1_048_576).contains(&mem_mb) {
        return err("mem_mb must be 512..=1048576".into());
    }
    let alias = match vm_create_provider_alias(zone) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let label = match tofu_resource_label(name) {
        Ok(l) => l,
        Err(e) => return err(e),
    };

    let hcl = render_xenserver_vm_hcl(
        &label,
        name,
        alias,
        template,
        network_uuid,
        u32::try_from(vcpus).unwrap_or(2),
        mem_mb * 1024 * 1024,
    );

    let dir = svc.workgroup_root.join("infra/tofu/xen-xapi");
    let file = dir.join(format!("{label}.tf"));
    if let Err(e) = std::fs::write(&file, &hcl) {
        return err(format!("vm-create: writing {}: {e}", file.display()));
    }

    // Apply through IaC against the mesh-replicated state (same lane as the tofu
    // responder). `dir` is process-owned, so this is not an injection surface.
    let script = format!(
        "cd {} && source ./env.sh 2>/dev/null && tofu apply -auto-approve -no-color 2>&1 | tail -30",
        dir.display()
    );
    match std::process::Command::new("bash")
        .args(["-lc", &script])
        .output()
    {
        Ok(o) if o.status.success() => {
            let summary = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "file": file.display().to_string(), "summary": summary })
                .to_string()
        }
        Ok(o) => {
            let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
            out.push_str(&String::from_utf8_lossy(&o.stderr));
            let msg = out.trim();
            if msg.is_empty() {
                err("vm-create: tofu apply failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("vm-create: tofu exec failed: {e}")),
    }
}

// ---- DATACENTER-19: guided new-lighthouse orchestration -----------------------
//
// One orchestrated job: droplet (Tofu) → bootstrap mackesd → found/join the prod
// mesh → add the DNS record, with per-step progress on `event/dc/job/<jobid>`.
// The plan is PURE + unit-tested; the executor runs each real step on a detached
// thread and replies immediately with the job id so the long flow streams its
// progress instead of blocking the responder.

/// A doctl region slug is valid? `[a-z0-9-]`, non-empty, ≤ 16 chars. PURE.
#[must_use]
pub fn valid_region(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 16
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The deterministic droplet/host name for a guided lighthouse in `region` at
/// `ts`: `lighthouse-<region>-<ts>`. PURE.
#[must_use]
pub fn guided_droplet_name(region: &str, ts: u64) -> String {
    format!("lighthouse-{region}-{ts}")
}

/// The shell fragment that resolves the new droplet's public IPv4 by name via
/// doctl (no jq dependency — `--format`/`--no-header` + awk), failing the step if
/// no public IP has surfaced yet. PURE — `name` is whitelisted `[a-z0-9-]`.
fn ip_resolve(name: &str) -> String {
    format!(
        "IP=$(doctl compute droplet list --context ${{MCNF_DOCTL_CONTEXT:-mackes}} \
         --format Name,PublicIPv4 --no-header 2>/dev/null | awk '$1==\"{name}\"{{print $2}}'); \
         [ -n \"$IP\" ] || {{ echo 'no public IP for {name}' >&2; exit 1; }}"
    )
}

/// Build the ordered guided-lighthouse plan for `region`/`name`. PURE — each step
/// is a `(name, bash-script)` pair run in order. The droplet step is the in-repo
/// `zone1-do` Tofu apply; the bootstrap/found-join/DNS steps shell the sibling
/// `automation/lighthouse/*` scripts (referenced by path, the same pattern as the
/// DR backup) against the resolved droplet IP.
#[must_use]
pub fn guided_lighthouse_plan(region: &str, name: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "droplet",
            format!(
                "cd infra/tofu/zone1-do && source ./env.sh 2>/dev/null && \
                 tofu apply -auto-approve -no-color \
                 -var 'lighthouse_name={name}' -var 'lighthouse_region={region}' 2>&1 | tail -20"
            ),
        ),
        (
            "await-ip",
            format!(
                "for i in $(seq 1 30); do {ip}; echo \"$IP\"; exit 0; done",
                ip = ip_resolve(name)
            ),
        ),
        (
            "bootstrap",
            format!(
                "{ip}; bash automation/lighthouse/bootstrap.sh \"$IP\"",
                ip = ip_resolve(name)
            ),
        ),
        (
            "found-join",
            format!(
                "{ip}; bash automation/lighthouse/found-join.sh \"$IP\"",
                ip = ip_resolve(name)
            ),
        ),
        (
            "dns",
            format!(
                "{ip}; bash automation/lighthouse/add-dns.sh {name} \"$IP\"",
                ip = ip_resolve(name)
            ),
        ),
    ]
}

/// Publish one job-progress body onto `event/dc/job/<jobid>` (fire-and-reap, the
/// same lane shape as the dc workers).
fn publish_job(jobid: &str, body: &str) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args([
        "publish",
        &format!("event/dc/job/{jobid}"),
        "--body-flag",
        body,
    ]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Run the guided-lighthouse plan to completion on the calling (detached) thread,
/// publishing a progress event per step transition and a terminal job event. Stops
/// at the first failing step (honest: a failed Tofu apply ends the job in
/// `error`, with the step + detail on the Bus).
fn run_guided_job(jobid: String, region: String, plan: Vec<(&'static str, String)>) {
    publish_job(
        &jobid,
        &json!({ "job": jobid, "region": region, "status": "running", "step": "start" })
            .to_string(),
    );
    for (step, script) in plan {
        publish_job(
            &jobid,
            &json!({ "job": jobid, "region": region, "status": "running", "step": step })
                .to_string(),
        );
        let out = std::process::Command::new("bash")
            .args(["-lc", &script])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let detail: String = String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .chars()
                    .take(200)
                    .collect();
                publish_job(
                    &jobid,
                    &json!({ "job": jobid, "region": region, "status": "step-ok", "step": step, "detail": detail })
                        .to_string(),
                );
            }
            Ok(o) => {
                let detail: String = String::from_utf8_lossy(&o.stderr)
                    .trim()
                    .chars()
                    .take(200)
                    .collect();
                publish_job(
                    &jobid,
                    &json!({ "job": jobid, "region": region, "status": "error", "step": step, "detail": detail })
                        .to_string(),
                );
                return;
            }
            Err(e) => {
                publish_job(
                    &jobid,
                    &json!({ "job": jobid, "region": region, "status": "error", "step": step, "detail": format!("spawn failed: {e}") })
                        .to_string(),
                );
                return;
            }
        }
    }
    publish_job(
        &jobid,
        &json!({ "job": jobid, "region": region, "status": "ok", "step": "done" }).to_string(),
    );
}

/// Handle a `do-guided-lighthouse` request: RBAC + confirm + region validation,
/// then kick off the orchestrated job on a detached thread and reply immediately
/// with the job id + planned steps. Body `{ region, confirm:true, principal? }`.
fn do_guided_lighthouse_reply(_svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("do-guided-lighthouse: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("do-guided-lighthouse: bad json: {e}")),
    };
    if let Err(e) =
        crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))
    {
        return err(e);
    }
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return err("do-guided-lighthouse requires confirm:true".into());
    }
    let region = req
        .get("region")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_region(region) {
        return err("region must be a doctl slug ([a-z0-9-], ≤16 chars)".into());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = guided_droplet_name(region, ts);
    let jobid = format!("guided-lh-{name}");
    let plan = guided_lighthouse_plan(region, &name);
    let step_names: Vec<&str> = plan.iter().map(|(n, _)| *n).collect();
    // Run the long flow off the responder thread; it streams progress to
    // event/dc/job/<jobid>. A failed spawn degrades to a synchronous error.
    let (jid, reg, pl) = (jobid.clone(), region.to_string(), plan);
    if let Err(e) = std::thread::Builder::new()
        .name("dc-guided-lighthouse".into())
        .spawn(move || run_guided_job(jid, reg, pl))
    {
        return err(format!("do-guided-lighthouse: spawn failed: {e}"));
    }
    json!({ "ok": true, "job": jobid, "name": name, "steps": step_names }).to_string()
}

// ---- DATACENTER-20: promotion arm + trigger ----------------------------------

/// Whether a `promote-now` to `stage` is allowed given the promote prod-arm state.
/// PURE. Only the production `do` (DigitalOcean) step is gated; `eagle` (the
/// Build→Eagle hop) is always allowed.
///
/// # Errors
/// Returns the disarmed reason when promoting to `do` while the gate is off.
pub fn promote_now_gate(stage: &str, armed: bool) -> Result<(), String> {
    if (stage == "do" || stage == "prod") && !armed {
        return Err(
            "prod disarmed: arm promotion (action/dc/promote-arm {\"on\":true}) \
             before promoting to do"
                .into(),
        );
    }
    Ok(())
}

/// Handle a `promote-arm` request: read or set the Build→Eagle→DO **`do` step**
/// prod-arm gate. A set carries `{"on": <bool>}` (RBAC-gated + persisted); a bare
/// read omits `on`. Reply `{"ok":true,"armed":<bool>}`.
fn promote_arm_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("promote-arm: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("promote-arm: bad json: {e}")),
    };
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    if let Some(on) = req.get("on").and_then(serde_json::Value::as_bool) {
        if let Err(e) =
            crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))
        {
            return err(e);
        }
        if let Err(e) = crate::ipc::dc_common::write_arm(&state_dir, "promote", on) {
            return err(format!("promote-arm: persist failed: {e}"));
        }
        return json!({ "ok": true, "armed": on }).to_string();
    }
    json!({ "ok": true, "armed": crate::ipc::dc_common::read_arm(&state_dir, "promote") })
        .to_string()
}

/// Handle a `promote-now` request: RBAC + confirm, then (for the `do` stage) the
/// promote prod-arm gate; publish the promotion intent to `event/dc/promote/intent`
/// and shell the sibling `automation/promote/promote-now.sh <stage> <version>`
/// (referenced by path, the DR-backup pattern). Body
/// `{ stage:"eagle"|"do", version?, confirm:true, principal? }`.
fn promote_now_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("promote-now: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("promote-now: bad json: {e}")),
    };
    if let Err(e) =
        crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))
    {
        return err(e);
    }
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return err("promote-now requires confirm:true".into());
    }
    let stage = req
        .get("stage")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !matches!(stage, "eagle" | "do" | "prod") {
        return err("promote-now: stage must be eagle|do".into());
    }
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    let armed = crate::ipc::dc_common::read_arm(&state_dir, "promote");
    if let Err(e) = promote_now_gate(stage, armed) {
        return err(e);
    }
    // The version is whitelisted to a release token so it is safe to interpolate.
    let version = req
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !version.is_empty()
        && !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    {
        return err("promote-now: version contains invalid characters".into());
    }
    // Record the promotion intent on the Bus (no-fixed-center: the action records
    // intent; the promotion machinery enacts it + the dc_promote worker re-reads
    // the live versions onto event/dc/promote/*).
    publish_job(
        &format!("promote-{stage}"),
        &json!({ "kind": "promote-intent", "stage": stage, "version": version }).to_string(),
    );
    let script = format!("automation/promote/promote-now.sh {stage} {version}");
    let out = std::process::Command::new("bash")
        .args(["-lc", &script])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let detail = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "stage": stage, "version": version, "detail": detail }).to_string()
        }
        Ok(o) => {
            let mut combined = String::from_utf8_lossy(&o.stdout).into_owned();
            combined.push_str(&String::from_utf8_lossy(&o.stderr));
            let msg = combined.trim();
            if msg.is_empty() {
                err(format!("promote-now {stage} failed"))
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("promote-now exec failed: {e}")),
    }
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

    /// Test-only: call [`build_reply`] while holding the RBAC env lock so a
    /// concurrent `rbac_*` test that mutates `dc_rbac::ROLE_MAP_ENV` can't leak
    /// a stray role map into this call. EFF-18 env-race: `cargo test` runs tests
    /// in threads of ONE process, and `enforce` reads the role map from the
    /// process-global env — so a mutating-verb call here (no caller principal)
    /// would be denied if a setter test had `ROLE_MAP_ENV` set at that instant.
    /// The canonical run pins `--test-threads=1`, but routing every reader
    /// through this lock makes the suite robust even without it (the full
    /// `cargo test --workspace` integration gate runs threaded). The two
    /// `rbac_*` setter tests keep their EXPLICIT guard — they hold the lock
    /// across `set_var`/`build_reply`/`remove_var`, so they must call
    /// `build_reply` directly (re-locking through this helper would deadlock).
    fn rbac_safe_reply(s: &DatacenterService, verb: &str, body: Option<&str>) -> String {
        let _g = crate::ipc::dc_rbac::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        build_reply(s, verb, body)
    }

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
        let r = rbac_safe_reply(&s, "vm-console", Some(&body));
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
        let r = rbac_safe_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
        // confirm false
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": false
        })
        .to_string();
        let r = rbac_safe_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
        // confirm as a non-bool ("true" string) does not satisfy the gate
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": "true"
        })
        .to_string();
        let r = rbac_safe_reply(&s, "vm-delete", Some(&body));
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
        let r = rbac_safe_reply(&s, "vm-delete", Some(&body));
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
        let r = rbac_safe_reply(&s, "vm-clone", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        assert!(rbac_safe_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(rbac_safe_reply(&s, "vm-power", None).contains("missing request body"));
        assert!(rbac_safe_reply(&s, "vm-snapshot", None).contains("missing request body"));
        assert!(rbac_safe_reply(&s, "vm-clone", None).contains("missing request body"));
        assert!(rbac_safe_reply(&s, "vm-delete", None).contains("missing request body"));
        assert!(rbac_safe_reply(&s, "vm-console", None).contains("missing request body"));
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
        let r = rbac_safe_reply(&s, "vm-power", Some(&body));
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
        let r = rbac_safe_reply(&s, "vm-snapshot", Some(&body));
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
        let r = rbac_safe_reply(&s, "vm-power", Some(&body));
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
        let r2 = rbac_safe_reply(&s, "vm-power", Some(&other));
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
        let r1 = rbac_safe_reply(&s, "vm-power", Some(&body));
        assert!(r1.contains("dom0 not in allowed set"), "{r1}");
        let r2 = rbac_safe_reply(&s, "vm-power", Some(&body));
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
        let r = rbac_safe_reply(&clone, "vm-power", Some(&body));
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
        let r = rbac_safe_reply(&s, "vm-console", Some(&body));
        assert!(
            !r.contains("busy"),
            "read-only verb must not be locked: {r}"
        );
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    // ---- DATACENTER-11: full VM lifecycle ----

    #[test]
    fn lifecycle_verbs_in_action_set() {
        for v in [
            "vm-suspend",
            "vm-resume",
            "vm-migrate",
            "vm-create",
            "vm-bulk",
        ] {
            assert!(ACTION_VERBS.contains(&v), "missing verb {v}");
            assert_eq!(action_topic(v), format!("action/dc/{v}"));
        }
    }

    // ---- DATACENTER-19: guided new-lighthouse ----------------------------------

    #[test]
    fn dc19_dc20_verbs_in_lock() {
        for v in ["do-guided-lighthouse", "promote-arm", "promote-now"] {
            assert_eq!(action_topic(v), format!("action/dc/{v}"));
            assert!(ACTION_VERBS.contains(&v), "{v} missing");
        }
    }

    #[test]
    fn is_mutating_marks_only_reads_readonly() {
        assert!(!is_mutating("vm-console"));
        assert!(!is_mutating("do-regions"));
        for v in [
            "vm-power",
            "vm-suspend",
            "vm-resume",
            "vm-migrate",
            "vm-create",
            "vm-bulk",
            "vm-delete",
        ] {
            assert!(is_mutating(v), "{v} should be mutating");
        }
    }

    #[test]
    fn suspend_resume_commands_reuse_argv_builders() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_suspend_command(uuid).unwrap(),
            format!("vm-suspend uuid={uuid}")
        );
        assert_eq!(
            vm_resume_command(uuid).unwrap(),
            format!("vm-resume uuid={uuid}")
        );
        // injection guards (empty / non-hex / metachars) reject.
        assert!(vm_suspend_command("").is_err());
        assert!(vm_resume_command("abcd;rm -rf /").is_err());
        assert!(vm_suspend_command("ghij").is_err());
    }

    #[test]
    fn migrate_command_validates_uuid_and_target() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_migrate_command(uuid, "MDE-host-b").unwrap(),
            format!("vm-migrate uuid={uuid} host=MDE-host-b live=true")
        );
        // a destination uuid is fine too.
        assert!(vm_migrate_command(uuid, "1111-2222").is_ok());
        // bad uuid / empty or unsafe target rejected (no shell metachars).
        assert!(vm_migrate_command("nope!", "host").is_err());
        assert!(vm_migrate_command(uuid, "").is_err());
        assert!(vm_migrate_command(uuid, "a;reboot").is_err());
        assert!(vm_migrate_command(uuid, "a b").is_err());
    }

    #[test]
    fn bulk_op_verb_maps_and_rejects() {
        assert_eq!(vm_bulk_op_verb("start").unwrap(), "vm-start");
        assert_eq!(vm_bulk_op_verb("shutdown").unwrap(), "vm-shutdown");
        assert_eq!(vm_bulk_op_verb("reboot").unwrap(), "vm-reboot");
        assert_eq!(vm_bulk_op_verb("suspend").unwrap(), "vm-suspend");
        assert_eq!(vm_bulk_op_verb("resume").unwrap(), "vm-resume");
        assert!(vm_bulk_op_verb("destroy").is_err());
        assert!(vm_bulk_op_verb("").is_err());
    }

    #[test]
    fn lifecycle_verbs_reject_unlisted_dom0_before_ssh() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        for (verb, body) in [
            ("vm-suspend", json!({ "uuid": uuid, "dom0": "10.0.0.1" })),
            ("vm-resume", json!({ "uuid": uuid, "dom0": "10.0.0.1" })),
            (
                "vm-migrate",
                json!({ "uuid": uuid, "dom0": "10.0.0.1", "target_host": "h" }),
            ),
            (
                "vm-bulk",
                json!({ "uuids": [uuid], "op": "start", "dom0": "10.0.0.1" }),
            ),
        ] {
            let r = rbac_safe_reply(&s, verb, Some(&body.to_string()));
            assert!(r.contains("dom0 not in allowed set"), "{verb}: {r}");
        }
    }

    #[test]
    fn rbac_viewer_is_rejected_on_a_mutating_verb() {
        // A configured viewer principal is rejected BEFORE the dom0 allow-list.
        let _g = crate::ipc::dc_rbac::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // Set + clear the role map around the call (serialized by the lock above).
        std::env::set_var(crate::ipc::dc_rbac::ROLE_MAP_ENV, "bob=viewer");
        let body = json!({
            "principal": "bob", "uuid": "abcd-1234", "op": "start", "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-power", Some(&body));
        std::env::remove_var(crate::ipc::dc_rbac::ROLE_MAP_ENV);
        assert!(r.contains("rbac"), "{r}");
        assert!(r.contains("viewer"), "{r}");
        assert!(
            !r.contains("dom0 not in allowed set"),
            "rbac gates first: {r}"
        );
    }

    #[test]
    fn rbac_read_only_verb_allowed_for_viewer() {
        // vm-console is read-only → a viewer may call it (falls to the dom0 check).
        let _g = crate::ipc::dc_rbac::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        std::env::set_var(crate::ipc::dc_rbac::ROLE_MAP_ENV, "bob=viewer");
        let body =
            json!({ "principal": "bob", "uuid": "abcd-1234", "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "vm-console", Some(&body));
        std::env::remove_var(crate::ipc::dc_rbac::ROLE_MAP_ENV);
        assert!(!r.contains("rbac"), "{r}");
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn tofu_resource_label_sanitizes() {
        assert_eq!(tofu_resource_label("web1").unwrap(), "dc_web1");
        assert_eq!(tofu_resource_label("My VM.01").unwrap(), "dc_my_vm_01");
        assert_eq!(tofu_resource_label("MDE-VM-x").unwrap(), "dc_mde_vm_x");
        assert!(tofu_resource_label("").is_err());
        assert!(tofu_resource_label("___").is_err());
    }

    #[test]
    fn provider_alias_maps_zones() {
        assert_eq!(vm_create_provider_alias("dev").unwrap(), "xhs");
        assert_eq!(vm_create_provider_alias("xhs").unwrap(), "xhs");
        assert_eq!(vm_create_provider_alias("kvm").unwrap(), "kvm");
        assert_eq!(vm_create_provider_alias("big").unwrap(), "big");
        assert!(vm_create_provider_alias("prod").is_err());
    }

    #[test]
    fn render_hcl_matches_the_build_vm_shape() {
        let hcl = render_xenserver_vm_hcl(
            "dc_web1",
            "web1",
            "xhs",
            "MDE-VM-golden",
            "420c5872-dd49-af7f-fe4f-d5e2502429f8",
            4,
            17_179_869_184,
        );
        assert!(hcl.contains("resource \"xenserver_vm\" \"dc_web1\" {"));
        assert!(hcl.contains("provider          = xenserver.xhs"));
        assert!(hcl.contains("name_label        = \"web1\""));
        assert!(hcl.contains("template_name     = \"MDE-VM-golden\""));
        assert!(hcl.contains("static_mem_max    = 17179869184"));
        assert!(hcl.contains("vcpus             = 4"));
        assert!(hcl.contains("network_uuid = \"420c5872-dd49-af7f-fe4f-d5e2502429f8\""));
        assert!(hcl.contains("ignore_changes = [hard_drive, template_name"));
    }

    #[test]
    fn vm_create_validates_before_touching_tofu() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // bad name.
        let body =
            json!({ "name": "bad name!", "network_uuid": "1111", "zone": "dev" }).to_string();
        assert!(rbac_safe_reply(&s, "vm-create", Some(&body)).contains("name must be"));
        // missing/empty network_uuid.
        let body = json!({ "name": "web1", "zone": "dev" }).to_string();
        assert!(rbac_safe_reply(&s, "vm-create", Some(&body)).contains("network_uuid"));
        // bad zone.
        let body =
            json!({ "name": "web1", "network_uuid": "1111-2222", "zone": "prod" }).to_string();
        assert!(rbac_safe_reply(&s, "vm-create", Some(&body)).contains("unknown zone/pool"));
        // out-of-range vcpus.
        let body = json!({
            "name": "web1", "network_uuid": "1111-2222", "zone": "dev", "vcpus": 999
        })
        .to_string();
        assert!(rbac_safe_reply(&s, "vm-create", Some(&body)).contains("vcpus must be"));
    }

    #[test]
    fn missing_body_errors_for_new_verbs() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        for v in [
            "vm-suspend",
            "vm-resume",
            "vm-migrate",
            "vm-create",
            "vm-bulk",
        ] {
            assert!(
                rbac_safe_reply(&s, v, None).contains("missing request body"),
                "{v}"
            );
        }
    }

    #[test]
    fn valid_region_whitelists_slugs() {
        assert!(valid_region("nyc3"));
        assert!(valid_region("fra1"));
        assert!(valid_region("sfo3"));
        // injection / uppercase / empty / too long rejected
        assert!(!valid_region("nyc3; rm -rf /"));
        assert!(!valid_region("NYC3"));
        assert!(!valid_region(""));
        assert!(!valid_region("a-very-long-region-slug"));
    }

    #[test]
    fn guided_name_and_plan_shape() {
        let name = guided_droplet_name("nyc3", 1_700_000_000);
        assert_eq!(name, "lighthouse-nyc3-1700000000");
        let plan = guided_lighthouse_plan("nyc3", &name);
        let names: Vec<&str> = plan.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec!["droplet", "await-ip", "bootstrap", "found-join", "dns"]
        );
        // The droplet step is the in-repo zone1-do Tofu apply, parameterized.
        assert!(plan[0].1.contains("infra/tofu/zone1-do"));
        assert!(plan[0].1.contains("tofu apply -auto-approve"));
        assert!(plan[0].1.contains("lighthouse_region=nyc3"));
        // The IP-dependent steps resolve the IP via doctl and shell the sibling
        // automation scripts.
        assert!(plan[1].1.contains("doctl compute droplet list"));
        assert!(plan[2].1.contains("automation/lighthouse/bootstrap.sh"));
        assert!(plan[3].1.contains("automation/lighthouse/found-join.sh"));
        assert!(plan[4].1.contains("automation/lighthouse/add-dns.sh"));
    }

    #[test]
    fn do_guided_lighthouse_gates_before_spawn() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // missing body
        assert!(rbac_safe_reply(&s, "do-guided-lighthouse", None).contains("missing request body"));
        // confirm required (valid region but no confirm → no spawn)
        let body = json!({ "region": "nyc3" }).to_string();
        let r = rbac_safe_reply(&s, "do-guided-lighthouse", Some(&body));
        assert!(r.contains("requires confirm:true"), "{r}");
        // bad region with confirm → rejected before any spawn
        let body = json!({ "region": "nyc3; rm -rf /", "confirm": true }).to_string();
        let r = rbac_safe_reply(&s, "do-guided-lighthouse", Some(&body));
        assert!(r.contains("region must be a doctl slug"), "{r}");
    }

    // ---- DATACENTER-20: promote arm + trigger ----------------------------------

    #[test]
    fn promote_now_gate_only_gates_do() {
        // Build→Eagle is always allowed.
        assert!(promote_now_gate("eagle", false).is_ok());
        assert!(promote_now_gate("eagle", true).is_ok());
        // The DO step is gated.
        let e = promote_now_gate("do", false).unwrap_err();
        assert!(e.contains("prod disarmed"), "{e}");
        assert!(promote_now_gate("do", true).is_ok());
        assert!(promote_now_gate("prod", true).is_ok());
    }

    #[test]
    fn promote_arm_read_reports_state() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let r = rbac_safe_reply(&s, "promote-arm", Some("{}"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v.get("armed").and_then(|a| a.as_bool()).is_some(), "{r}");
    }

    #[test]
    fn promote_now_gates_before_exec() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // missing body
        assert!(rbac_safe_reply(&s, "promote-now", None).contains("missing request body"));
        // confirm required (returns before any publish/shell)
        let body = json!({ "stage": "eagle" }).to_string();
        let r = rbac_safe_reply(&s, "promote-now", Some(&body));
        assert!(r.contains("requires confirm:true"), "{r}");
        // bad stage with confirm → rejected before any publish/shell
        let body = json!({ "stage": "moon", "confirm": true }).to_string();
        let r = rbac_safe_reply(&s, "promote-now", Some(&body));
        assert!(r.contains("stage must be eagle|do"), "{r}");
    }
}

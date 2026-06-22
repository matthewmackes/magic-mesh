//! DATACENTER (action layer) — `action/dc/vm-power` + `action/dc/vm-snapshot`
//! → Xen VM control.
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

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The VM power-control responder — rooted at the shared workgroup root (carried
/// for parity with the other action services; the allowed-dom0 set + ssh key come
/// from the orchestrator's env-driven config).
#[derive(Debug, Clone)]
pub struct DatacenterService {
    // Carried for parity with the other action services and the
    // `new(workgroup_root)` spawn contract; the allowed-dom0 set + ssh key are
    // read from the orchestrator's env config, so this isn't read here yet.
    #[allow(dead_code)]
    workgroup_root: PathBuf,
}

impl DatacenterService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 2] = ["vm-power", "vm-snapshot"];

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

/// Build the reply for one `action/dc/<verb>` request, dispatching on `verb`.
#[must_use]
pub fn build_reply(_svc: &DatacenterService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    match verb {
        "vm-power" => vm_power_reply(req_body),
        "vm-snapshot" => vm_snapshot_reply(req_body),
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
        assert!(ACTION_VERBS.contains(&"vm-power"));
        assert!(ACTION_VERBS.contains(&"vm-snapshot"));
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
    fn unknown_verb_and_missing_body_error() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "vm-power", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-snapshot", None).contains("missing request body"));
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
}

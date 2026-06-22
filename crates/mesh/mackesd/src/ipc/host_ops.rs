//! DATACENTER (action layer) — `action/dc/host-power` → Xen host (dom0)
//! maintenance + reboot control.
//!
//! Companion to the VM power responder ([`crate::ipc::datacenter`]): where that
//! acts on a guest VM, this acts on the host (pool member) itself. Same
//! dedicated-OS-thread, `action/dc/<verb>` Bus-RPC shape; the reads/exec are
//! synchronous SSH calls over the mesh key.
//!
//! Request body `{ "dom0", "op": "maintenance-on"|"maintenance-off"|"reboot" }`:
//!   * `op` maps to a sequence of `xe` host verbs
//!     ([`host_power_commands`]) — `maintenance-on`→`host-disable`,
//!     `maintenance-off`→`host-enable`, `reboot`→`host-disable`+`host-reboot`
//!     (XAPI requires the host be disabled before it will reboot it);
//!   * `dom0` MUST be in the configured allowed set
//!     ([`crate::workers::datacenter_orchestrator::xen_dom0s`]) before any SSH.
//! The host UUID is resolved remotely (`xe host-list params=uuid --minimal`),
//! then each verb runs as `xe <verb> host=<uuid>` in sequence.
//! Reply `{"ok":true}` when every step succeeds, `{"error":"<message>"}` otherwise.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The host power-control responder — rooted at the shared workgroup root (carried
/// for parity with the other action services; the allowed-dom0 set + ssh key come
/// from the orchestrator's env-driven config).
#[derive(Debug, Clone)]
pub struct HostOpsService {
    // Carried for parity with the other action services and the
    // `new(workgroup_root)` spawn contract; the allowed-dom0 set + ssh key are
    // read from the orchestrator's env config, so this isn't read here yet.
    #[allow(dead_code)]
    workgroup_root: PathBuf,
}

impl HostOpsService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 1] = ["host-power"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Map a host power `op` to the ordered list of `xe` host verbs it runs. PURE.
///
/// * `maintenance-on`  → `["host-disable"]` (enters maintenance mode);
/// * `maintenance-off` → `["host-enable"]` (leaves maintenance mode);
/// * `reboot`          → `["host-disable", "host-reboot"]` — XAPI refuses to
///   reboot an enabled host, so it must be disabled first.
///
/// Each returned verb is later run as `xe <verb> host=<uuid>`.
///
/// # Errors
/// Returns `Err` for any `op` outside the three above.
pub fn host_power_commands(op: &str) -> Result<Vec<String>, String> {
    match op {
        "maintenance-on" => Ok(vec!["host-disable".to_string()]),
        "maintenance-off" => Ok(vec!["host-enable".to_string()]),
        "reboot" => Ok(vec!["host-disable".to_string(), "host-reboot".to_string()]),
        other => Err(format!("unknown op: {other}")),
    }
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

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(_svc: &HostOpsService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    if verb != "host-power" {
        return err("unknown dc verb".into());
    }
    let Some(body) = req_body else {
        return err("host-power: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("host-power: bad json: {e}")),
    };
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let op = req
        .get("op")
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

    let verbs = match host_power_commands(op) {
        Ok(v) => v,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();

    // Resolve the host's UUID remotely. `--minimal` prints just the value.
    let uuid = match ssh_xe_status(&key, dom0, "xe host-list params=uuid --minimal") {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            // `--minimal` yields a comma-separated list for multiple hosts; on a
            // single-host pool member it's one uuid. Take the first.
            out.trim()
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            return if msg.is_empty() {
                err("host-list failed".into())
            } else {
                err(msg.to_string())
            };
        }
        Err(e) => return err(format!("ssh failed: {e}")),
    };
    if uuid.is_empty() {
        return err("host uuid not found".into());
    }
    // The remote uuid is XAPI-generated; guard anyway before interpolation.
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return err("host uuid contains invalid characters".into());
    }

    // Run each verb in sequence; stop at the first failure.
    for v in &verbs {
        let remote = format!("xe {v} host={uuid}");
        match ssh_xe_status(&key, dom0, &remote) {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = stderr.trim();
                return if msg.is_empty() {
                    err(format!("{v} failed"))
                } else {
                    err(msg.to_string())
                };
            }
            Err(e) => return err(format!("ssh failed: {e}")),
        }
    }
    json!({ "ok": true }).to_string()
}

/// Run the host-ops Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &HostOpsService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &HostOpsService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "host-ops responder: list_since failed");
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "host-ops responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("host-power"), "action/dc/host-power");
        assert!(ACTION_VERBS.contains(&"host-power"));
    }

    #[test]
    fn host_power_commands_maps_each_valid_op() {
        assert_eq!(
            host_power_commands("maintenance-on").unwrap(),
            vec!["host-disable".to_string()]
        );
        assert_eq!(
            host_power_commands("maintenance-off").unwrap(),
            vec!["host-enable".to_string()]
        );
        assert_eq!(
            host_power_commands("reboot").unwrap(),
            vec!["host-disable".to_string(), "host-reboot".to_string()]
        );
    }

    #[test]
    fn host_power_commands_unknown_op_errors() {
        assert!(host_power_commands("destroy").is_err());
        assert!(host_power_commands("").is_err());
        assert!(host_power_commands("shutdown").is_err());
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "host-power", None).contains("missing request body"));
    }

    #[test]
    fn dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "dom0": "10.0.0.1",
            "op": "maintenance-on"
        })
        .to_string();
        let r = build_reply(&s, "host-power", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }
}

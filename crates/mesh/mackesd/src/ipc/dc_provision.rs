//! DATACENTER-21 (action layer) — `action/dc/testmesh-spin` +
//! `action/dc/testmesh-teardown` → ephemeral test-mesh lifecycle.
//!
//! The action side of the test-mesh provisioning flow: a thin, hermetic wrapper
//! over `automation/testbed/farm-testbed.sh` (the BUILD-PLATFORM-3 snapshot-reset
//! VM-pool harness), so the Workbench can spin / tear down an N-node test mesh
//! from the golden template with one click. Same dedicated-OS-thread,
//! `action/dc/<verb>` Bus-RPC shape as the other dc responders; the
//! clone/boot/destroy are synchronous (long, like `tofu-apply`) — a low-frequency
//! operator action.
//!
//! `testmesh-spin` request body `{ "n": <1..=10> }`:
//!   * `n` is the node count, bounded to the testbed's reserved IP range
//!     (`172.20.0.60..69`, [`MAX_TESTMESH_NODES`] nodes); anything else is
//!     rejected without spawning;
//!   * runs `farm-testbed.sh up <n>`, which clones `n` fresh VMs from the golden
//!     template, boots them on static IPs, and prints `name ip` per node;
//!   * reply `{"ok":true,"nodes":[{"name","ip"}, …]}` parsed from that output.
//!
//! `testmesh-teardown` request body `{ "confirm": true, "id"? }` (destructive):
//!   * `confirm` MUST be `true` — the destructive guard, checked first;
//!   * runs `farm-testbed.sh down`, which destroys EVERY `mcnf-test-*` VM + its
//!     disks (the testbed is a single hermetic pool, so `id` is accepted for
//!     forward-compat but teardown is whole-pool today);
//!   * reply `{"ok":true}` on success.
//!
//! (farm-scale — adjusting the build-VM count via Tofu — is NOT served here: the
//! build farm is three statically-declared, state-adopted VMs (one per standalone
//! pool, `infra/tofu/xen-xapi/build-vms.tf`), so scaling needs a `for_each`
//! refactor of that imported state, which is the DATACENTER-2 infra domain.)

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The test-mesh provisioning responder — rooted at the shared workgroup root
/// (carried for parity with the other action services; the testbed config comes
/// from `farm-testbed.sh`'s own env).
#[derive(Debug, Clone)]
pub struct DcProvisionService {
    #[allow(dead_code)]
    workgroup_root: PathBuf,
}

impl DcProvisionService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 2] = ["testmesh-spin", "testmesh-teardown"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Upper bound on a test-mesh node count — the testbed reserves IPs
/// `172.20.0.60..69`, so at most 10 nodes spin at once.
pub const MAX_TESTMESH_NODES: u64 = 10;

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Validate + extract the node count `n` from a `testmesh-spin` body. PURE.
///
/// `n` must be present, an integer in `1..=`[`MAX_TESTMESH_NODES`]. Returns the
/// count or a human-readable rejection — so an out-of-range / missing count never
/// reaches the spawn.
///
/// # Errors
/// Returns `Err` when `n` is missing, not a positive integer, or above the cap.
pub fn parse_node_count(req: &serde_json::Value) -> Result<u64, String> {
    let n = req
        .get("n")
        .and_then(serde_json::Value::as_u64)
        .ok_or("testmesh-spin: `n` must be a positive integer")?;
    if n == 0 {
        return Err("testmesh-spin: `n` must be at least 1".into());
    }
    if n > MAX_TESTMESH_NODES {
        return Err(format!(
            "testmesh-spin: `n` must be <= {MAX_TESTMESH_NODES} (testbed IP range)"
        ));
    }
    Ok(n)
}

/// Parse `farm-testbed.sh up` stdout (`name ip` per line) into `(name, ip)`
/// pairs. PURE. Blank lines + malformed lines (not exactly two fields) are
/// skipped, so the testbed's stderr log noise can never poison the node list.
#[must_use]
pub fn parse_testmesh_nodes(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let name = it.next()?;
            let ip = it.next()?;
            // Exactly two fields — a third means it isn't a clean "name ip" row.
            if it.next().is_some() || name.is_empty() || ip.is_empty() {
                return None;
            }
            Some((name.to_string(), ip.to_string()))
        })
        .collect()
}

/// Run `farm-testbed.sh up <n>` (clones+boots `n` test VMs) and return the parsed
/// node list. The script narrates progress on stderr; the `name ip` rows are on
/// stdout.
fn testmesh_spin(req_body: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let Some(body) = req_body else {
        return Err("testmesh-spin: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("testmesh-spin: bad json: {e}"))?;
    let n = parse_node_count(&req)?;

    let o = std::process::Command::new("bash")
        .arg("automation/testbed/farm-testbed.sh")
        .arg("up")
        .arg(n.to_string())
        .output()
        .map_err(|e| format!("testmesh-spin: spawn failed: {e}"))?;

    if !o.status.success() {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        return Err(if msg.is_empty() {
            "testmesh-spin: farm-testbed up failed".into()
        } else {
            msg.to_string()
        });
    }
    let nodes = parse_testmesh_nodes(&String::from_utf8_lossy(&o.stdout));
    if nodes.is_empty() {
        return Err("testmesh-spin: no test nodes came up".into());
    }
    Ok(nodes)
}

/// Run `farm-testbed.sh down` (destroys ALL `mcnf-test-*` VMs + disks).
/// `confirm:true`-gated. `id` is accepted for forward-compat but teardown is
/// whole-pool today (the testbed is one hermetic pool).
fn testmesh_teardown(req_body: Option<&str>) -> Result<(), String> {
    let Some(body) = req_body else {
        return Err("testmesh-teardown: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("testmesh-teardown: bad json: {e}"))?;
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("testmesh-teardown requires confirm:true".into());
    }

    let o = std::process::Command::new("bash")
        .arg("automation/testbed/farm-testbed.sh")
        .arg("down")
        .output()
        .map_err(|e| format!("testmesh-teardown: spawn failed: {e}"))?;

    if o.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        Err(if msg.is_empty() {
            "testmesh-teardown: farm-testbed down failed".into()
        } else {
            msg.to_string()
        })
    }
}

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(_svc: &DcProvisionService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    // DATACENTER-7 (RBAC): both verbs mutate (spin clones VMs, teardown destroys
    // them) → gate the mesh principal BEFORE dispatch; deny + audit a viewer.
    if let Err(reason) = crate::ipc::dc_rbac::enforce(verb, req_body) {
        crate::ipc::dc_rbac::audit_denial(verb, req_body, &reason);
        return err(reason);
    }
    match verb {
        "testmesh-spin" => match testmesh_spin(req_body) {
            Ok(nodes) => {
                let nodes: Vec<serde_json::Value> = nodes
                    .into_iter()
                    .map(|(name, ip)| json!({ "name": name, "ip": ip }))
                    .collect();
                json!({ "ok": true, "nodes": nodes }).to_string()
            }
            Err(m) => err(m),
        },
        "testmesh-teardown" => match testmesh_teardown(req_body) {
            Ok(()) => json!({ "ok": true }).to_string(),
            Err(m) => err(m),
        },
        _ => err("unknown dc verb".into()),
    }
}

/// Run the provisioning Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DcProvisionService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(
    persist: &Persist,
    svc: &DcProvisionService,
    cursors: &mut HashMap<String, String>,
) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc-provision responder: list_since failed");
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "dc-provision responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("testmesh-spin"), "action/dc/testmesh-spin");
        assert_eq!(
            action_topic("testmesh-teardown"),
            "action/dc/testmesh-teardown"
        );
        assert!(ACTION_VERBS.contains(&"testmesh-spin"));
        assert!(ACTION_VERBS.contains(&"testmesh-teardown"));
    }

    #[test]
    fn parse_node_count_bounds_the_range() {
        assert_eq!(parse_node_count(&json!({ "n": 1 })).unwrap(), 1);
        assert_eq!(
            parse_node_count(&json!({ "n": MAX_TESTMESH_NODES })).unwrap(),
            MAX_TESTMESH_NODES
        );
        // missing
        assert!(parse_node_count(&json!({})).is_err());
        // zero
        assert!(parse_node_count(&json!({ "n": 0 })).is_err());
        // over the cap
        assert!(parse_node_count(&json!({ "n": MAX_TESTMESH_NODES + 1 })).is_err());
        // non-integer / negative
        assert!(parse_node_count(&json!({ "n": "three" })).is_err());
        assert!(parse_node_count(&json!({ "n": -2 })).is_err());
    }

    #[test]
    fn parse_testmesh_nodes_extracts_clean_rows_only() {
        // Real shape: the script's stderr log lines never reach stdout, but be
        // defensive — only clean two-field rows become nodes.
        let out = "mcnf-test-0 172.20.0.60\n\
                   mcnf-test-1 172.20.0.61\n\
                   \n\
                   garbage line with three fields here\n\
                   mcnf-test-2 172.20.0.62\n";
        let nodes = parse_testmesh_nodes(out);
        assert_eq!(
            nodes,
            vec![
                ("mcnf-test-0".to_string(), "172.20.0.60".to_string()),
                ("mcnf-test-1".to_string(), "172.20.0.61".to_string()),
                ("mcnf-test-2".to_string(), "172.20.0.62".to_string()),
            ]
        );
        assert!(parse_testmesh_nodes("").is_empty());
    }

    #[test]
    fn testmesh_teardown_requires_confirm_true() {
        let s = DcProvisionService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "testmesh-teardown", Some(&json!({}).to_string()));
        assert!(r.contains("testmesh-teardown requires confirm:true"), "{r}");
        let r = build_reply(
            &s,
            "testmesh-teardown",
            Some(&json!({ "confirm": false }).to_string()),
        );
        assert!(r.contains("testmesh-teardown requires confirm:true"), "{r}");
    }

    #[test]
    fn testmesh_spin_rejects_bad_count_before_spawn() {
        let s = DcProvisionService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "testmesh-spin", Some(&json!({ "n": 0 }).to_string()));
        assert!(r.contains("at least 1"), "{r}");
        let r = build_reply(&s, "testmesh-spin", Some(&json!({ "n": 99 }).to_string()));
        assert!(r.contains("must be <="), "{r}");
        let r = build_reply(&s, "testmesh-spin", Some(&json!({}).to_string()));
        assert!(r.contains("positive integer"), "{r}");
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = DcProvisionService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "testmesh-spin", None).contains("missing request body"));
        assert!(build_reply(&s, "testmesh-teardown", None).contains("missing request body"));
    }
}

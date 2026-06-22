//! DC-15 (action layer) — `action/dc/tofu-plan` → read-only OpenTofu plan.
//!
//! The plan side of the DATACENTER infra surface: the Workbench plane asks for a
//! `tofu plan` of an allow-listed workspace and gets the trimmed plan text back.
//! Same dedicated-OS-thread, `action/dc/<verb>` Bus-RPC shape as the VM
//! power-control responder ([`crate::ipc::datacenter`]) — the exec is a
//! synchronous `bash -lc` call.
//!
//! Request body `{ "workspace": "xen-xapi" | "zone1-do" }`:
//!   * `workspace` is allow-listed by [`tofu_workspace_dir`] (no path traversal /
//!     injection) → the relative dir `infra/tofu/<workspace>`;
//!   * the plan runs in that dir with the workspace's `env.sh` sourced.
//! Reply `{"ok":true,"summary":"<text>"}` on success, `{"error":"<msg>"}` on
//! failure. This is **plan only** — read-only; it never runs `apply`.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The Tofu-plan responder — rooted at the shared workgroup root, which is the
/// repo root the relative `infra/tofu/<ws>` dir is resolved against.
#[derive(Debug, Clone)]
pub struct TofuService {
    workgroup_root: PathBuf,
}

impl TofuService {
    /// Build the service rooted at the shared workgroup root (the repo root).
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 1] = ["tofu-plan"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Resolve a request `workspace` to its relative tofu dir. PURE.
///
/// Allow-lists `ws` ∈ {`xen-xapi`, `zone1-do`} → `infra/tofu/<ws>`. Any other
/// value (including anything with path separators / traversal / injection
/// characters) is rejected, so the caller can never escape the tofu tree.
///
/// # Errors
/// Returns `Err` for any `ws` not in the allow-list.
pub fn tofu_workspace_dir(ws: &str) -> Result<String, String> {
    match ws {
        "xen-xapi" | "zone1-do" => Ok(format!("infra/tofu/{ws}")),
        other => Err(format!("unknown tofu workspace: {other}")),
    }
}

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(svc: &TofuService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    if verb != "tofu-plan" {
        return err("unknown dc verb".into());
    }
    let Some(body) = req_body else {
        return err("tofu-plan: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("tofu-plan: bad json: {e}")),
    };
    let ws = req
        .get("workspace")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dir = match tofu_workspace_dir(ws) {
        Ok(d) => d,
        Err(e) => return err(e),
    };

    let repo = svc.workgroup_root.display();
    // Read-only: `tofu plan` (never apply), in the workspace dir with its env
    // sourced. `dir` and `repo` are allow-listed / process-owned, so this is not
    // an injection surface.
    let script = format!(
        "cd {repo}/{dir} && source ./env.sh 2>/dev/null && tofu plan -no-color 2>&1 | tail -25"
    );
    match std::process::Command::new("bash")
        .args(["-lc", &script])
        .output()
    {
        Ok(o) if o.status.success() => {
            let summary = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "summary": summary }).to_string()
        }
        Ok(o) => {
            let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
            out.push_str(&String::from_utf8_lossy(&o.stderr));
            let msg = out.trim();
            if msg.is_empty() {
                err("tofu plan failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("tofu plan exec failed: {e}")),
    }
}

/// Run the tofu Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &TofuService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &TofuService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "tofu responder: list_since failed");
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "tofu responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("tofu-plan"), "action/dc/tofu-plan");
        assert!(ACTION_VERBS.contains(&"tofu-plan"));
    }

    #[test]
    fn tofu_workspace_dir_allows_xen_and_zone1() {
        assert_eq!(
            tofu_workspace_dir("xen-xapi").unwrap(),
            "infra/tofu/xen-xapi"
        );
        assert_eq!(
            tofu_workspace_dir("zone1-do").unwrap(),
            "infra/tofu/zone1-do"
        );
    }

    #[test]
    fn tofu_workspace_dir_rejects_unknown_and_traversal() {
        assert!(tofu_workspace_dir("prod").is_err());
        assert!(tofu_workspace_dir("../../etc").is_err());
        assert!(tofu_workspace_dir("xen-xapi; rm -rf /").is_err());
        assert!(tofu_workspace_dir("").is_err());
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = TofuService::new(PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "tofu-plan", None).contains("missing request body"));
    }

    #[test]
    fn unknown_workspace_is_rejected() {
        let s = TofuService::new(PathBuf::from("/tmp"));
        let body = json!({ "workspace": "prod" }).to_string();
        let r = build_reply(&s, "tofu-plan", Some(&body));
        assert!(r.contains("unknown tofu workspace"), "{r}");
    }
}

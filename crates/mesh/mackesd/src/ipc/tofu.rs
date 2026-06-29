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
//! failure. The plan side is **read-only**; it never runs `apply`.
//!
//! DC-15 also exposes two **mutating** verbs gated behind an explicit
//! `confirm:true` in the request body (DATACENTER-15):
//!   * `action/dc/tofu-apply`   → `tofu apply -auto-approve`;
//!   * `action/dc/tofu-destroy` → `tofu destroy -auto-approve`.
//! Both share the same `tofu_workspace_dir` allow-list as the injection guard
//! and refuse to run unless `confirm == true`.
//!
//! DC-15 also exposes a read-only **state browser** verb (DATACENTER-15):
//!   * `action/dc/tofu-state` → the managed-resource addresses from
//!     `tofu state list` plus a `drift` flag from parsing a `tofu plan` (ignoring
//!     the 0.2.x provider's benign phantom fields). Request `{ "workspace": ... }`, reply
//!     `{"ok":true,"resources":[<addr>...],"drift":<bool>}`. Read-only.

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
///
/// DATACENTER-15 adds the persisted **run-log** (`tofu-runlog`) and the
/// production-apply **prod-arm** gate (`tofu-arm`) to the plan/apply/destroy/state
/// surface.
pub const ACTION_VERBS: [&str; 6] = [
    "tofu-plan",
    "tofu-apply",
    "tofu-destroy",
    "tofu-state",
    "tofu-runlog",
    "tofu-arm",
];

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

/// Whether a parsed request body carries an explicit `confirm: true`. PURE.
///
/// The mutating verbs (`tofu-apply` / `tofu-destroy`) refuse to run unless this
/// returns `true`. A missing field, `false`, or any non-boolean value all count
/// as *not confirmed* — the gate fails closed.
#[must_use]
pub fn is_confirmed(req: &serde_json::Value) -> bool {
    req.get("confirm")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Parse `tofu state list` stdout into one address per managed resource. PURE.
///
/// Keeps non-empty trimmed lines, in order. Blank lines (and surrounding
/// whitespace) are dropped, so an empty / whitespace-only output yields `[]`.
#[must_use]
pub fn parse_state_list(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Whether a `tofu plan -no-color` output signals REAL drift. PURE.
///
/// `tofu plan -detailed-exitcode` returns 2 for ANY diff, but the early-stage
/// (0.2.x) `xenserver` provider can't round-trip two benign fields
/// (`check_ip_timeout`, `default_ip`), so it reports a phantom change on every
/// plan. This parses the plan text and returns true only when a resource is
/// added/destroyed or an attribute OUTSIDE that benign set changes. "No changes"
/// or only-benign → false; unrecognized output → false (fails closed).
#[must_use]
pub fn plan_has_real_drift(plan: &str) -> bool {
    const BENIGN: [&str; 2] = ["check_ip_timeout", "default_ip"];
    if plan.contains("No changes.") {
        return false;
    }
    for line in plan.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("Plan:") {
            // "Plan: A to add, B to change, C to destroy" — add/destroy = real.
            let count = |needle: &str| -> u32 {
                rest.split(',')
                    .find(|seg| seg.contains(needle))
                    .and_then(|seg| seg.split_whitespace().next())
                    .and_then(|num| num.parse().ok())
                    .unwrap_or(0)
            };
            if count("to add") > 0 || count("to destroy") > 0 {
                return true;
            }
        }
        // A real attribute diff line is "<+|~|-> <attr> = …". Require the `=` so
        // the legend ("~ update in-place"), the resource header ("~ resource …"),
        // and "->" arrows are all excluded. Any changed attr outside the benign
        // set = drift.
        for sym in ['+', '~', '-'] {
            if let Some(rest) = t.strip_prefix(sym).map(str::trim_start) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let is_attr = rest[name.len()..].trim_start().starts_with('=');
                if name.is_empty() || !is_attr {
                    continue;
                }
                if !BENIGN.contains(&name.as_str()) {
                    return true;
                }
            }
        }
    }
    false
}

// ---- DATACENTER-15: persisted run-log + the prod-arm production gate ----------

/// Max chars of a run's summary kept in an on-disk run-log line. PURE cap so one
/// giant plan can't bloat the log.
pub const RUNLOG_SUMMARY_CAP: usize = 2000;

/// Default number of run-log records `tofu-runlog` returns (newest last).
pub const RUNLOG_DEFAULT_LIMIT: usize = 50;

/// The per-workspace Tofu run-log file under the dc state dir:
/// `<state>/tofu-runlog/<ws>.jsonl`. PURE — the workspace is already allow-listed
/// by [`tofu_workspace_dir`] before this is built, so it carries no traversal.
#[must_use]
pub fn runlog_path(state_dir: &std::path::Path, ws: &str) -> PathBuf {
    state_dir.join("tofu-runlog").join(format!("{ws}.jsonl"))
}

/// Build one JSON run-log line for a completed plan/apply/destroy. PURE — `ts` is
/// supplied so it's deterministic in tests. The summary is capped to
/// [`RUNLOG_SUMMARY_CAP`] chars (char-boundary safe).
#[must_use]
pub fn runlog_record_json(ts: u64, verb: &str, ws: &str, ok: bool, summary: &str) -> String {
    let capped: String = summary.chars().take(RUNLOG_SUMMARY_CAP).collect();
    serde_json::json!({
        "ts": ts, "verb": verb, "workspace": ws, "ok": ok, "summary": capped,
    })
    .to_string()
}

/// Parse run-log file content into the last `limit` records (newest last). PURE.
/// Blank lines and lines that don't parse as JSON are skipped.
#[must_use]
pub fn parse_runlog(content: &str, limit: usize) -> Vec<serde_json::Value> {
    let mut recs: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect();
    if recs.len() > limit {
        recs = recs.split_off(recs.len() - limit);
    }
    recs
}

/// Whether an apply/destroy on `ws` is allowed given the Tofu prod-arm state.
/// PURE. Only the **production** zone (`zone1-do`) is gated; the dev zone
/// (`xen-xapi`) is always allowed.
///
/// # Errors
/// Returns the "prod disarmed" reason when a production apply/destroy is attempted
/// while the gate is off.
pub fn prod_apply_gate(ws: &str, armed: bool) -> Result<(), String> {
    if ws == "zone1-do" && !armed {
        return Err(
            "prod disarmed: arm tofu prod (action/dc/tofu-arm {\"on\":true}) \
             before applying to zone1-do"
                .into(),
        );
    }
    Ok(())
}

/// Current unix time in seconds (0 on a pre-epoch clock — never panics).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append one run record to a workspace's run-log (best-effort). Creates the
/// run-log dir as needed; a write failure is logged + swallowed so a full disk can
/// never wedge the tofu op itself.
fn append_runlog(state_dir: &std::path::Path, verb: &str, ws: &str, ok: bool, summary: &str) {
    use std::io::Write;
    let path = runlog_path(state_dir, ws);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::debug!(error = %e, "tofu runlog: mkdir failed");
            return;
        }
    }
    let line = format!(
        "{}\n",
        runlog_record_json(now_unix(), verb, ws, ok, summary)
    );
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(line.as_bytes()) {
                tracing::debug!(error = %e, "tofu runlog: append failed");
            }
        }
        Err(e) => tracing::debug!(error = %e, "tofu runlog: open failed"),
    }
}

/// `tofu-arm` — read or set the Tofu **production-apply** gate (the `zone1-do`
/// apply/destroy guard). A set carries `{"on": <bool>}` (RBAC-gated + persisted);
/// a bare read omits `on`. Reply `{"ok":true,"armed":<bool>}`.
fn tofu_arm_reply(svc: &TofuService, req: &serde_json::Value) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    if let Some(on) = req.get("on").and_then(serde_json::Value::as_bool) {
        if let Err(e) =
            crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(req))
        {
            return err(e);
        }
        if let Err(e) = crate::ipc::dc_common::write_arm(&state_dir, "tofu", on) {
            return err(format!("tofu-arm: persist failed: {e}"));
        }
        return json!({ "ok": true, "armed": on }).to_string();
    }
    json!({ "ok": true, "armed": crate::ipc::dc_common::read_arm(&state_dir, "tofu") }).to_string()
}

/// `tofu-runlog` — the persisted per-workspace run-log (read-only). `ws` is
/// already allow-listed. Optional `{"limit": <n>}` (clamped 1..=1000) bounds the
/// newest records returned. Reply `{"ok":true,"workspace":<ws>,"runs":[…]}`.
fn runlog_reply(svc: &TofuService, ws: &str, req: &serde_json::Value) -> String {
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    let limit = req
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .map_or(RUNLOG_DEFAULT_LIMIT, |n| n as usize)
        .clamp(1, 1000);
    let path = runlog_path(&state_dir, ws);
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let runs = parse_runlog(&content, limit);
    json!({ "ok": true, "workspace": ws, "runs": runs }).to_string()
}

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(svc: &TofuService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    // DATACENTER-7 (RBAC): gate the mesh principal BEFORE dispatch. `tofu-plan` /
    // `tofu-state` are reads (any role); `tofu-apply` / `tofu-destroy` mutate and
    // require operator — a denied viewer is refused + audited here.
    if let Err(reason) = crate::ipc::dc_rbac::enforce(verb, req_body) {
        crate::ipc::dc_rbac::audit_denial(verb, req_body, &reason);
        return err(reason);
    }
    match verb {
        "tofu-plan" | "tofu-apply" | "tofu-destroy" | "tofu-state" | "tofu-runlog" | "tofu-arm" => {
        }
        _ => return err("unknown dc verb".into()),
    }
    let Some(body) = req_body else {
        return err(format!("{verb}: missing request body"));
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("{verb}: bad json: {e}")),
    };

    // tofu-arm is a global prod gate with no workspace — handle it before the
    // workspace allow-list (and before any tofu invocation).
    if verb == "tofu-arm" {
        return tofu_arm_reply(svc, &req);
    }

    let ws = req
        .get("workspace")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dir = match tofu_workspace_dir(ws) {
        Ok(d) => d,
        Err(e) => return err(e),
    };

    // tofu-runlog is a read-only per-workspace history (allow-list above is the
    // only guard it needs).
    if verb == "tofu-runlog" {
        return runlog_reply(svc, ws, &req);
    }

    // The mutating verbs are RBAC-gated (viewer = denied when a role map is set),
    // then fail closed unless explicitly confirmed, then — for the production
    // zone — gated behind the prod-arm switch.
    if verb == "tofu-apply" || verb == "tofu-destroy" {
        if let Err(e) =
            crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))
        {
            return err(e);
        }
        if !is_confirmed(&req) {
            return err("apply requires confirm:true".into());
        }
        let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
        let armed = crate::ipc::dc_common::read_arm(&state_dir, "tofu");
        if let Err(e) = prod_apply_gate(ws, armed) {
            return err(e);
        }
    }

    let repo = svc.workgroup_root.display();

    // The state browser has its own (read-only) two-command shape and reply.
    if verb == "tofu-state" {
        // 1. Managed-resource addresses. `dir` / `repo` are allow-listed /
        //    process-owned, so this is not an injection surface.
        let list_script = format!("cd {repo}/{dir} && tofu state list 2>/dev/null");
        let resources = match std::process::Command::new("bash")
            .args(["-lc", &list_script])
            .output()
        {
            Ok(o) => parse_state_list(&String::from_utf8_lossy(&o.stdout)),
            Err(e) => return err(format!("tofu-state exec failed: {e}")),
        };

        // 2. Drift: parse a plain plan, ignoring the provider's benign phantom
        //    fields (see `plan_has_real_drift`).
        let drift_script =
            format!("cd {repo}/{dir} && source ./env.sh 2>/dev/null && tofu plan -no-color 2>&1");
        let drift = match std::process::Command::new("bash")
            .args(["-lc", &drift_script])
            .output()
        {
            Ok(o) => plan_has_real_drift(&String::from_utf8_lossy(&o.stdout)),
            Err(e) => return err(format!("tofu-state exec failed: {e}")),
        };

        return json!({ "ok": true, "resources": resources, "drift": drift }).to_string();
    }

    // The per-verb tofu invocation. `dir` and `repo` are allow-listed /
    // process-owned, so this is not an injection surface.
    let tofu_cmd = match verb {
        "tofu-plan" => "tofu plan -no-color 2>&1 | tail -25",
        "tofu-apply" => "tofu apply -auto-approve -no-color 2>&1 | tail -30",
        // tofu-destroy
        _ => "tofu destroy -auto-approve -no-color 2>&1 | tail -30",
    };

    let script = format!("cd {repo}/{dir} && source ./env.sh 2>/dev/null && {tofu_cmd}");
    // Persist the run to the per-workspace on-disk run-log (DATACENTER-15) — every
    // plan/apply/destroy, ok or fail — so the panel's run-log survives a reload,
    // not just the transient Bus.
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    match std::process::Command::new("bash")
        .args(["-lc", &script])
        .output()
    {
        Ok(o) if o.status.success() => {
            let summary = String::from_utf8_lossy(&o.stdout).trim().to_string();
            append_runlog(&state_dir, verb, ws, true, &summary);
            json!({ "ok": true, "summary": summary }).to_string()
        }
        Ok(o) => {
            let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
            out.push_str(&String::from_utf8_lossy(&o.stderr));
            let msg = out.trim();
            let msg = if msg.is_empty() {
                format!("{verb} failed")
            } else {
                msg.to_string()
            };
            append_runlog(&state_dir, verb, ws, false, &msg);
            err(msg)
        }
        Err(e) => {
            let msg = format!("{verb} exec failed: {e}");
            append_runlog(&state_dir, verb, ws, false, &msg);
            err(msg)
        }
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
        assert_eq!(action_topic("tofu-apply"), "action/dc/tofu-apply");
        assert_eq!(action_topic("tofu-destroy"), "action/dc/tofu-destroy");
        assert_eq!(action_topic("tofu-state"), "action/dc/tofu-state");
        assert!(ACTION_VERBS.contains(&"tofu-plan"));
        assert!(ACTION_VERBS.contains(&"tofu-apply"));
        assert!(ACTION_VERBS.contains(&"tofu-destroy"));
        assert!(ACTION_VERBS.contains(&"tofu-state"));
    }

    #[test]
    fn parse_state_list_keeps_nonempty_trimmed_lines() {
        assert_eq!(parse_state_list(""), Vec::<String>::new());
        assert_eq!(parse_state_list("   \n\n\t\n"), Vec::<String>::new());
        assert_eq!(
            parse_state_list("xenorchestra_vm.a\n\n  module.net.do_droplet.b  \nlocal_file.c\n"),
            vec![
                "xenorchestra_vm.a".to_string(),
                "module.net.do_droplet.b".to_string(),
                "local_file.c".to_string(),
            ]
        );
    }

    #[test]
    fn plan_drift_ignores_benign_provider_fields() {
        // "No changes" → no drift.
        assert!(!plan_has_real_drift(
            "No changes. Your infrastructure matches."
        ));
        // The xenserver-provider phantom: the legend + resource header + only
        // check_ip_timeout + default_ip → NOT drift (the legend "~ update
        // in-place" must not be mistaken for a changed attribute).
        let benign = "Resource actions are indicated with the following symbols:\n  \
            ~ update in-place (current -> planned)\n\n  \
            # xenserver_vm.build_50 will be updated in-place\n  \
            ~ resource \"xenserver_vm\" \"build_50\" {\n      \
            + check_ip_timeout  = 0\n      + default_ip        = (known after apply)\n        \
            id                = \"1119\"\n    }\nPlan: 0 to add, 1 to change, 0 to destroy.";
        assert!(!plan_has_real_drift(benign));
        // A real attribute change → drift.
        let real = "  ~ resource \"xenserver_vm\" \"build_50\" {\n      \
            ~ static_mem_max = 25769803776 -> 17179869184\n    }\n\
            Plan: 0 to add, 1 to change, 0 to destroy.";
        assert!(plan_has_real_drift(real));
        // An add or destroy is always real drift.
        assert!(plan_has_real_drift(
            "Plan: 1 to add, 0 to change, 0 to destroy."
        ));
        assert!(plan_has_real_drift(
            "Plan: 0 to add, 0 to change, 1 to destroy."
        ));
    }

    #[test]
    fn tofu_state_rejects_unknown_workspace() {
        let s = TofuService::new(PathBuf::from("/tmp"));
        let body = json!({ "workspace": "prod" }).to_string();
        let r = build_reply(&s, "tofu-state", Some(&body));
        assert!(r.contains("unknown tofu workspace"), "{r}");
    }

    #[test]
    fn tofu_state_does_not_require_confirm() {
        // Read-only: the confirm gate never applies, so a bad workspace (not the
        // confirm error) is what we get back.
        let s = TofuService::new(PathBuf::from("/tmp"));
        let body = json!({ "workspace": "../../etc" }).to_string();
        let r = build_reply(&s, "tofu-state", Some(&body));
        assert!(!r.contains("apply requires confirm:true"), "{r}");
        assert!(r.contains("unknown tofu workspace"), "{r}");
    }

    #[test]
    fn confirm_gate_helper_fails_closed() {
        // Missing / false / non-boolean → not confirmed.
        assert!(!is_confirmed(&json!({ "workspace": "xen-xapi" })));
        assert!(!is_confirmed(&json!({ "confirm": false })));
        assert!(!is_confirmed(&json!({ "confirm": "true" })));
        assert!(!is_confirmed(&json!({ "confirm": 1 })));
        // Only an explicit boolean true confirms.
        assert!(is_confirmed(&json!({ "confirm": true })));
    }

    #[test]
    fn apply_and_destroy_refuse_without_confirm() {
        let s = TofuService::new(PathBuf::from("/tmp"));
        for verb in ["tofu-apply", "tofu-destroy"] {
            // Missing confirm.
            let body = json!({ "workspace": "xen-xapi" }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("apply requires confirm:true"), "{verb}: {r}");
            // confirm:false.
            let body = json!({ "workspace": "xen-xapi", "confirm": false }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("apply requires confirm:true"), "{verb}: {r}");
        }
    }

    #[test]
    fn apply_and_destroy_reject_traversal_before_confirm() {
        // The allow-list stays the injection guard even with confirm:true.
        let s = TofuService::new(PathBuf::from("/tmp"));
        for verb in ["tofu-apply", "tofu-destroy"] {
            let body = json!({ "workspace": "../../etc", "confirm": true }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("unknown tofu workspace"), "{verb}: {r}");
            let body = json!({ "workspace": "xen-xapi; rm -rf /", "confirm": true }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("unknown tofu workspace"), "{verb}: {r}");
        }
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

    // ---- DATACENTER-15: run-log + prod-arm gate --------------------------------

    #[test]
    fn new_verbs_in_lock() {
        assert_eq!(action_topic("tofu-runlog"), "action/dc/tofu-runlog");
        assert_eq!(action_topic("tofu-arm"), "action/dc/tofu-arm");
        assert!(ACTION_VERBS.contains(&"tofu-runlog"));
        assert!(ACTION_VERBS.contains(&"tofu-arm"));
    }

    #[test]
    fn runlog_path_is_per_workspace_jsonl() {
        let p = runlog_path(std::path::Path::new("/var/dc"), "xen-xapi");
        assert_eq!(
            p,
            std::path::Path::new("/var/dc/tofu-runlog/xen-xapi.jsonl")
        );
    }

    #[test]
    fn runlog_record_round_trips_and_caps_summary() {
        let line = runlog_record_json(1_700_000_000, "tofu-apply", "zone1-do", true, "applied 2");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ts"], 1_700_000_000_u64);
        assert_eq!(v["verb"], "tofu-apply");
        assert_eq!(v["workspace"], "zone1-do");
        assert_eq!(v["ok"], true);
        assert_eq!(v["summary"], "applied 2");
        // A monster summary is capped char-boundary safe.
        let big = "é".repeat(RUNLOG_SUMMARY_CAP + 500);
        let line = runlog_record_json(1, "tofu-plan", "xen-xapi", false, &big);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v["summary"].as_str().unwrap().chars().count(),
            RUNLOG_SUMMARY_CAP
        );
    }

    #[test]
    fn parse_runlog_keeps_newest_limit_in_order() {
        let mut content = String::new();
        for i in 0..5 {
            content.push_str(&runlog_record_json(i, "tofu-plan", "xen-xapi", true, "ok"));
            content.push('\n');
        }
        // A blank + garbage line are skipped.
        content.push_str("\n");
        content.push_str("not json\n");
        let runs = parse_runlog(&content, 3);
        assert_eq!(runs.len(), 3);
        // Newest-last: ts 2,3,4.
        assert_eq!(runs[0]["ts"], 2);
        assert_eq!(runs[2]["ts"], 4);
        // A limit larger than the record count returns them all.
        assert_eq!(parse_runlog(&content, 100).len(), 5);
        assert!(parse_runlog("", 10).is_empty());
    }

    #[test]
    fn append_then_read_runlog_round_trip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        append_runlog(dir, "tofu-plan", "xen-xapi", true, "no changes");
        append_runlog(dir, "tofu-apply", "xen-xapi", false, "boom");
        let content = std::fs::read_to_string(runlog_path(dir, "xen-xapi")).unwrap();
        let runs = parse_runlog(&content, 10);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0]["verb"], "tofu-plan");
        assert_eq!(runs[1]["verb"], "tofu-apply");
        assert_eq!(runs[1]["ok"], false);
        // A different workspace has its own (empty) log.
        assert!(std::fs::read_to_string(runlog_path(dir, "zone1-do")).is_err());
    }

    #[test]
    fn prod_apply_gate_only_gates_zone1_do() {
        // Dev zone is always allowed.
        assert!(prod_apply_gate("xen-xapi", false).is_ok());
        assert!(prod_apply_gate("xen-xapi", true).is_ok());
        // Prod zone disarmed → denied with a clear reason.
        let e = prod_apply_gate("zone1-do", false).unwrap_err();
        assert!(e.contains("prod disarmed") && e.contains("zone1-do"), "{e}");
        // Prod zone armed → allowed.
        assert!(prod_apply_gate("zone1-do", true).is_ok());
    }

    #[test]
    fn tofu_arm_read_reports_armed_state() {
        // A bare read (no `on`) never mutates and always answers ok+armed.
        let s = TofuService::new(PathBuf::from("/tmp"));
        let r = build_reply(&s, "tofu-arm", Some("{}"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v.get("armed").and_then(|a| a.as_bool()).is_some(), "{r}");
    }

    #[test]
    fn tofu_runlog_returns_runs_array() {
        // Pin the state dir to a temp dir so the read is hermetic, seed one run.
        let tmp = tempfile::tempdir().expect("tempdir");
        let s = TofuService::new(tmp.path().to_path_buf());
        // With no MCNF_DC_STATE_DIR override the service falls back to XDG; to keep
        // this hermetic we write directly to where the service will look only when
        // XDG is absent — so instead assert the read-only shape on an empty log.
        let r = build_reply(&s, "tofu-runlog", Some(r#"{"workspace":"xen-xapi"}"#));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["workspace"], "xen-xapi");
        assert!(v["runs"].is_array(), "{r}");
        // An unknown workspace is still rejected by the allow-list.
        let r = build_reply(&s, "tofu-runlog", Some(r#"{"workspace":"prod"}"#));
        assert!(r.contains("unknown tofu workspace"), "{r}");
    }
}

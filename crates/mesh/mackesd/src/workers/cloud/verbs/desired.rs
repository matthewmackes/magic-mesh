//! Workloads U4 — the `set-desired` + `plan` verb handlers.
//!
//! These fill the honest not-yet-wired skeletons U2 left in [`super`]:
//!
//! - **`set-desired`** ([`handle_set_desired`]) — persists a placement node's
//!   desired-state workload doc(s) under `<state_root>/mcnf/cloud/desired/<node>/…`
//!   (the local realization of the `/mcnf/cloud/desired/<node>/<name>` key), via the
//!   [`super::super::reconcile`] per-node store. It is an authenticated declarative
//!   mutation: changing desired state does not apply live infrastructure immediately,
//!   but it still requires an exact-body-bound, single-use capability.
//! - **`plan`** ([`handle_plan`]) — renders the node's desired slice into tfvars
//!   ([`super::super::render`]) and shells `tofu plan -json` through the injectable
//!   [`CloudRunner`] seam, returning the pending-change [`PlanCounts`] the surface
//!   previews before an armed apply. A READ, served for THIS node's slice.
//!
//! Honest by construction (§7): a plan the backend can't run is an honest `gated`,
//! never a fabricated all-zero in-sync plan; a `set-desired` with no writable spec
//! is an honest `error`, never a silent success.

use serde::Deserialize;

use mackes_mesh_types::cloud::{CloudReply, WorkloadSpec};

use super::super::reconcile;
use super::super::runner::default_libvirt_uri;
use super::super::CloudWorker;

/// The `set-desired` request body: one `spec`, a `specs` batch, and/or a `remove`
/// list of workload names. Every field is optional so the handler enforces what it
/// requires (at least one writable spec or one removal).
#[derive(Debug, Default, Deserialize)]
struct SetDesiredBody {
    /// The placement node this request targets (falls back to each spec's `node`).
    #[serde(default)]
    node: String,
    /// A single workload to declare.
    #[serde(default)]
    spec: Option<WorkloadSpec>,
    /// A batch of workloads to declare.
    #[serde(default)]
    specs: Vec<WorkloadSpec>,
    /// Workload names to retract from this node's desired state.
    #[serde(default)]
    remove: Vec<String>,
}

/// Stable semantic target for a desired-state request. The capability also binds
/// the complete canonical request digest, so this label is operator context, not
/// the only anti-substitution control.
pub(super) fn authorization_target(body_str: &str) -> Result<String, String> {
    let body: SetDesiredBody = serde_json::from_str(body_str.trim())
        .map_err(|e| format!("malformed set-desired request: {e}"))?;
    let mut names = Vec::new();
    if let Some(spec) = body.spec {
        names.push(spec.name.trim().to_string());
    }
    names.extend(
        body.specs
            .into_iter()
            .map(|spec| spec.name.trim().to_string()),
    );
    names.extend(body.remove.into_iter().map(|name| name.trim().to_string()));
    names.retain(|name| !name.is_empty());
    names.sort();
    names.dedup();
    Ok(if names.len() == 1 {
        format!("desired:{}", names[0])
    } else {
        "desired-state".to_string()
    })
}

/// The `plan` request body — the placement node whose slice to plan.
#[derive(Debug, Default, Deserialize)]
struct PlanBody {
    /// The explicit placement node whose local desired slice is planned.
    #[serde(default)]
    node: String,
}

/// Persist a node's desired-state workload doc(s) and echo the accepted specs.
///
/// Writes each declared [`WorkloadSpec`] under its placement node's desired dir and
/// retracts any `remove` names, then answers with the accepted specs in
/// [`CloudReply::desired`]. Honest rejection (§7): a body carrying neither a writable
/// spec nor a removal, or a spec missing its `name`/`node`, is an `error` — never a
/// silent no-op success.
#[must_use]
pub(crate) fn handle_set_desired(w: &CloudWorker, verb_name: &str, body_str: &str) -> CloudReply {
    let body: SetDesiredBody = match serde_json::from_str(body_str.trim()) {
        Ok(b) => b,
        Err(e) => return reject(verb_name, format!("malformed set-desired request: {e}")),
    };

    // Collect the specs to write (single + batch), resolving each spec's placement
    // node from the request `node` when the spec left it blank.
    let mut to_write: Vec<WorkloadSpec> = Vec::new();
    if let Some(s) = body.spec {
        to_write.push(s);
    }
    to_write.extend(body.specs);

    let request_node = body.node.trim();
    for spec in &mut to_write {
        if spec.node.trim().is_empty() {
            spec.node = request_node.to_string();
        }
    }

    if to_write.is_empty() && body.remove.is_empty() {
        return reject(
            verb_name,
            "set-desired requires a `spec`/`specs` to declare or a `remove` list".to_string(),
        );
    }

    // Validate the complete batch before the first write. The outer placement is
    // the sole routing authority: a nested spec may omit its node and inherit it,
    // but it may never smuggle a second node into one authorized request.
    if let Err(e) = super::super::path_key::segment("node", request_node) {
        return reject(verb_name, e);
    }
    for spec in &to_write {
        if spec.node != request_node {
            return reject(
                verb_name,
                format!(
                    "workload `{}` targets node `{}` but request placement is `{request_node}`",
                    spec.name, spec.node
                ),
            );
        }
        if let Err(e) = super::super::path_key::segment("name", &spec.name) {
            return reject(verb_name, e);
        }
    }
    for name in &body.remove {
        if let Err(e) = super::super::path_key::segment("name", name) {
            return reject(verb_name, e);
        }
    }

    // Write each declared spec.
    let mut accepted: Vec<WorkloadSpec> = Vec::new();
    for spec in to_write {
        if let Err(e) = reconcile::write_desired_doc(&w.state_root, &spec) {
            return fail(verb_name, format!("persist desired doc failed: {e}"));
        }
        accepted.push(spec);
    }

    // Retract each removal (idempotent — an absent workload is not an error).
    for name in &body.remove {
        if let Err(e) = reconcile::remove_desired_doc(&w.state_root, request_node, name) {
            return fail(verb_name, format!("retract desired doc failed: {e}"));
        }
    }

    CloudReply {
        ok: true,
        verb: verb_name.to_string(),
        desired: Some(accepted),
        ..Default::default()
    }
}

/// Render this node's desired slice and return the pending-change [`PlanCounts`].
///
/// Served for THIS placement node's slice: a `plan` targeting another node is
/// honestly gated (that node holds its own desired slice + tofu state and answers
/// for itself — this node would otherwise plan an empty foreign slice against the
/// wrong state). A plan the backend can't run is `gated`, never a faked in-sync plan.
#[must_use]
pub(crate) fn handle_plan(w: &CloudWorker, verb_name: &str, body_str: &str) -> CloudReply {
    let body: PlanBody = serde_json::from_str(body_str.trim()).unwrap_or_default();
    let node = body.node.trim();
    let node = if node.is_empty() {
        w.host.as_str()
    } else {
        node
    };

    if node != w.host {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!(
                "plan for node `{node}` is served by that placement node (it holds its \
                 own desired slice + tofu state)"
            )),
            ..Default::default()
        };
    }

    match reconcile::plan_counts_for_node(
        w.runner.as_ref(),
        &w.state_root,
        node,
        &default_libvirt_uri(),
    ) {
        Ok(counts) => CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            plan: Some(counts),
            ..Default::default()
        },
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!("plan unavailable: {e}")),
            ..Default::default()
        },
    }
}

/// An honest rejection (a malformed / insufficient request).
fn reject(verb_name: &str, reason: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(reason),
        ..Default::default()
    }
}

/// An honest backend/store failure.
fn fail(verb_name: &str, reason: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(reason),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::runner::fake::{instance, FakeRunner};
    use super::super::super::CloudWorker;
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A worker rooted at `state_root` with the fake `runner`, no bus.
    fn worker(host: &str, state_root: PathBuf, runner: Arc<FakeRunner>) -> CloudWorker {
        CloudWorker::new(host.to_string(), format!("peer:{host}"), state_root)
            .with_runner(runner)
            .with_bus_root(None)
    }

    #[test]
    fn set_desired_persists_the_spec_and_echoes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(
            "eagle",
            tmp.path().to_path_buf(),
            Arc::new(FakeRunner::default()),
        );
        let body = r#"{"node":"eagle","spec":{
            "name":"web","delivery_type":"service_vm","node":"eagle",
            "vcpu":2,"memory_mb":2048,"disk_gb":20}}"#;
        let reply = handle_set_desired(&w, "set-desired", body);
        assert!(reply.ok, "err: {:?}", reply.error);
        assert_eq!(reply.desired.as_ref().unwrap().len(), 1);
        // The doc landed in the node's per-node desired slice.
        let slice = reconcile::read_desired_slice(tmp.path(), "eagle");
        assert_eq!(slice.len(), 1);
        assert_eq!(slice[0].name, "web");
    }

    #[test]
    fn set_desired_fills_a_blank_spec_node_from_the_request_node() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(
            "eagle",
            tmp.path().to_path_buf(),
            Arc::new(FakeRunner::default()),
        );
        // The spec omits `node`; the request `node` fills it in.
        let body = r#"{"node":"eagle","spec":{
            "name":"svc","delivery_type":"service_container","node":"",
            "vcpu":1,"memory_mb":512,"disk_gb":4}}"#;
        assert!(handle_set_desired(&w, "set-desired", body).ok);
        let slice = reconcile::read_desired_slice(tmp.path(), "eagle");
        assert_eq!(slice.len(), 1);
        assert_eq!(slice[0].node, "eagle");
    }

    #[test]
    fn set_desired_can_retract_a_workload() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(
            "eagle",
            tmp.path().to_path_buf(),
            Arc::new(FakeRunner::default()),
        );
        let add = r#"{"node":"eagle","spec":{
            "name":"web","delivery_type":"service_vm","node":"eagle",
            "vcpu":2,"memory_mb":2048,"disk_gb":20}}"#;
        assert!(handle_set_desired(&w, "set-desired", add).ok);
        let remove = r#"{"node":"eagle","remove":["web"]}"#;
        assert!(handle_set_desired(&w, "set-desired", remove).ok);
        assert!(reconcile::read_desired_slice(tmp.path(), "eagle").is_empty());
    }

    #[test]
    fn set_desired_rejects_a_nested_second_node_before_writing_the_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(
            "eagle",
            tmp.path().to_path_buf(),
            Arc::new(FakeRunner::default()),
        );
        let body = r#"{"node":"eagle","specs":[
            {"name":"first","delivery_type":"service_vm","node":"eagle",
             "vcpu":1,"memory_mb":512,"disk_gb":4},
            {"name":"smuggled","delivery_type":"service_vm","node":"otter",
             "vcpu":1,"memory_mb":512,"disk_gb":4}
        ]}"#;

        let reply = handle_set_desired(&w, "set-desired", body);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("request placement"));
        assert!(reconcile::read_desired_slice(tmp.path(), "eagle").is_empty());
        assert!(reconcile::read_desired_slice(tmp.path(), "otter").is_empty());
    }

    #[test]
    fn set_desired_rejects_blank_removal_before_any_write() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(
            "eagle",
            tmp.path().to_path_buf(),
            Arc::new(FakeRunner::default()),
        );
        let body = r#"{"node":"eagle","spec":{
            "name":"first","delivery_type":"service_vm","node":"eagle",
            "vcpu":1,"memory_mb":512,"disk_gb":4},"remove":[" "]}"#;

        let reply = handle_set_desired(&w, "set-desired", body);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("path-safe"));
        assert!(reconcile::read_desired_slice(tmp.path(), "eagle").is_empty());
    }

    #[test]
    fn set_desired_without_a_spec_or_removal_is_an_honest_error() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(
            "eagle",
            tmp.path().to_path_buf(),
            Arc::new(FakeRunner::default()),
        );
        let reply = handle_set_desired(&w, "set-desired", r#"{"node":"eagle"}"#);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("requires"));
    }

    #[test]
    fn set_desired_rejects_an_absolute_node_before_any_write() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        let node = outside.to_string_lossy();
        let w = worker(
            "eagle",
            tmp.path().join("state"),
            Arc::new(FakeRunner::default()),
        );
        let body = format!(
            r#"{{"node":"{node}","spec":{{
                "name":"proof","delivery_type":"service_vm","node":"{node}",
                "vcpu":1,"memory_mb":1,"disk_gb":1}}}}"#
        );

        let reply = handle_set_desired(&w, "set-desired", &body);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("path-safe"));
        assert!(!outside.exists());
    }

    #[test]
    fn set_desired_rejects_an_absolute_removal_and_preserves_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("victim.json");
        std::fs::write(&outside, "keep").unwrap();
        let attack_path = tmp.path().join("victim");
        let attack_name = attack_path.to_string_lossy();
        let w = worker(
            "eagle",
            tmp.path().join("state"),
            Arc::new(FakeRunner::default()),
        );
        let body = format!(r#"{{"node":"eagle","remove":["{attack_name}"]}}"#);

        let reply = handle_set_desired(&w, "set-desired", &body);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("path-safe"));
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "keep");
    }

    #[test]
    fn plan_renders_the_slice_and_returns_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            plan_ndjson: Some(
                r#"{"type":"change_summary","changes":{"add":2,"change":0,"remove":0}}"#.into(),
            ),
            ..Default::default()
        });
        let w = worker("eagle", tmp.path().to_path_buf(), runner.clone());
        // Declare a workload, then plan it.
        let add = r#"{"node":"eagle","spec":{
            "name":"web","delivery_type":"service_vm","node":"eagle",
            "vcpu":2,"memory_mb":2048,"disk_gb":20}}"#;
        assert!(handle_set_desired(&w, "set-desired", add).ok);
        let reply = handle_plan(&w, "plan", r#"{"node":"eagle"}"#);
        assert!(reply.ok, "gated: {:?}", reply.gated);
        assert_eq!(reply.plan.unwrap().add, 2);
        // The renderer wrote this node's tfvars through the seam.
        assert_eq!(runner.tfvars_written.lock().unwrap().len(), 1);
    }

    #[test]
    fn plan_is_gated_when_the_backend_cannot_run_it() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            plan_err: Some("tofu: command not found".into()),
            ..Default::default()
        });
        let w = worker("eagle", tmp.path().to_path_buf(), runner);
        // §7 — a plan the backend can't run is an honest gate, never a faked 0/0/0.
        let reply = handle_plan(&w, "plan", "{}");
        assert!(!reply.ok);
        assert!(reply.plan.is_none());
        assert!(reply.gated.unwrap().contains("plan unavailable"));
    }

    #[test]
    fn plan_for_another_node_is_honestly_gated() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(
            "eagle",
            tmp.path().to_path_buf(),
            Arc::new(FakeRunner::default()),
        );
        let reply = handle_plan(&w, "plan", r#"{"node":"otter"}"#);
        assert!(!reply.ok);
        assert!(reply
            .gated
            .unwrap()
            .contains("served by that placement node"));
    }
}

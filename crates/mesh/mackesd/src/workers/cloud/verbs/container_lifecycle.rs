//! WL-ARCH-007 — bounded lifecycle actions for rootless Quadlet containers.
//!
//! The container view uses the workload name as the systemd unit stem. Every
//! stem is validated before it becomes an argv value, and every invocation goes
//! through [`CloudRunner::run_tool`] so production shells and tests share the
//! exact same no-shell-interpolation path.

use mackes_mesh_types::cloud::CloudReply;

use super::super::reconcile;
use super::super::runner::{CloudRunOutcome, ToolRun};
use super::super::CloudWorker;
use super::CloudActionBody;

const SYSTEMCTL: &str = "systemctl";
const JOURNALCTL: &str = "journalctl";
const SERVICE_SUFFIX: &str = ".service";
const RECENT_LINES: &str = "200";
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const OUTPUT_TRUNCATION_MARKER: &str = "\n[output truncated]";

/// Restart one rootless Quadlet service after the shared placement + auth gates
/// have accepted the request.
pub(crate) fn handle_restart(
    w: &CloudWorker,
    verb_name: &str,
    body: &CloudActionBody,
    raw: &str,
) -> CloudReply {
    let Some(instance) = validated_instance(body, SERVICE_SUFFIX) else {
        return invalid_instance_reply(verb_name, body, SERVICE_SUFFIX);
    };
    if let Some(reply) = authorization_refusal(w, verb_name, body, instance, raw) {
        return reply;
    }

    let unit = format!("{instance}{SERVICE_SUFFIX}");
    let args = ["--user", "restart", unit.as_str()];
    match w.runner.run_tool(SYSTEMCTL, &args) {
        Err(error) => finish_mutation(
            w,
            verb_name,
            instance,
            CloudRunOutcome::failed(bounded_message(
                "systemctl unavailable: ",
                &error,
                "",
            )),
            true,
        ),
        Ok(run) => finish_mutation(
            w,
            verb_name,
            instance,
            tool_outcome(&run, "restart", &unit),
            false,
        ),
    }
}

/// Return a bounded recent journal tail. This is a placement-scoped read: it
/// deliberately does not require a mutation capability, but it never turns a
/// missing journal backend or non-zero command into fabricated log content.
pub(crate) fn handle_logs(w: &CloudWorker, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    let Some(instance) = validated_instance(body, SERVICE_SUFFIX) else {
        return invalid_instance_reply(verb_name, body, SERVICE_SUFFIX);
    };
    let unit = format!("{instance}{SERVICE_SUFFIX}");
    let args = [
        "--user",
        "--no-pager",
        "--lines",
        RECENT_LINES,
        "--unit",
        unit.as_str(),
    ];
    match w.runner.run_tool(JOURNALCTL, &args) {
        Err(error) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(bounded_message(
                "journal backend unavailable: ",
                &error,
                " — recent logs were not retrieved",
            )),
            ..Default::default()
        },
        Ok(run) if run.ok => CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            raw_log: Some(if run.stdout.trim().is_empty() {
                "(no recent journal entries)".to_string()
            } else {
                bounded_output(run.stdout.trim())
            }),
            ..Default::default()
        },
        Ok(run) => {
            let detail = tool_detail(&run);
            CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!(
                    "journalctl could not read recent logs for `{instance}`: {detail}"
                )),
                raw_log: (!detail.is_empty()).then_some(detail),
                ..Default::default()
            }
        }
    }
}

/// Stop and disable one rootless Quadlet service, then retract its desired
/// document when the existing per-node desired-state store contains one.
pub(crate) fn handle_destroy(
    w: &CloudWorker,
    verb_name: &str,
    body: &CloudActionBody,
    raw: &str,
) -> CloudReply {
    let Some(instance) = validated_instance(body, SERVICE_SUFFIX) else {
        return invalid_instance_reply(verb_name, body, SERVICE_SUFFIX);
    };
    if let Err(error) = super::super::path_key::file_stem("instance", instance, ".json") {
        return reject(verb_name, error);
    }
    let confirmed = body
        .typed_name
        .as_deref()
        .map(str::trim)
        .is_some_and(|typed| typed == instance);
    if !confirmed {
        return reject(
            verb_name,
            format!("destroy blocked: `typed_name` must equal target `{instance}`"),
        );
    }
    if let Some(reply) = authorization_refusal(w, verb_name, body, instance, raw) {
        return reply;
    }

    let unit = format!("{instance}{SERVICE_SUFFIX}");
    let args = ["--user", "disable", "--now", unit.as_str()];
    let mut outcome = match w.runner.run_tool(SYSTEMCTL, &args) {
        Err(error) => {
            return finish_mutation(
                w,
                verb_name,
                instance,
                CloudRunOutcome::failed(bounded_message(
                    "systemctl unavailable: ",
                    &error,
                    "",
                )),
                true,
            )
        }
        Ok(run) => tool_outcome(&run, "stop/disable", &unit),
    };

    if outcome.ok {
        match reconcile::remove_desired_doc(&w.state_root, body.node.trim(), instance) {
            Ok(true) => {
                outcome.summary.push_str("; desired entry retracted");
            }
            Ok(false) => {
                outcome.summary.push_str("; no desired entry was present");
            }
            Err(error) => {
                // systemd already ran, so preserve `applied: true` in the audit
                // record while refusing to claim a complete destroy.
                outcome.ok = false;
                outcome.summary = format!(
                    "{}; desired-state retraction failed: {error}",
                    outcome.summary
                );
            }
        }
    }
    finish_mutation(w, verb_name, instance, outcome, false)
}

fn validated_instance<'a>(body: &'a CloudActionBody, suffix: &str) -> Option<&'a str> {
    let instance = body
        .instance
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    super::super::path_key::file_stem("instance", instance, suffix).ok()
}

fn invalid_instance_reply(verb_name: &str, body: &CloudActionBody, suffix: &str) -> CloudReply {
    let Some(instance) = body
        .instance
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return reject(
            verb_name,
            format!("`{verb_name}` requires an `instance` field in the request body"),
        );
    };
    let error = super::super::path_key::file_stem("instance", instance, suffix)
        .expect_err("invalid_instance_reply is only called for invalid names");
    reject(verb_name, error)
}

fn authorization_refusal(
    w: &CloudWorker,
    verb_name: &str,
    body: &CloudActionBody,
    target: &str,
    raw: &str,
) -> Option<CloudReply> {
    let verdict = w.consume_armed_token(
        body.armed_token.as_deref(),
        verb_name,
        body.node.trim(),
        target,
        raw,
    );
    (!verdict.is_valid()).then(|| CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        gated: Some(format!(
            "cloud action is not authorized ({}) — nothing changed or disclosed",
            verdict.reason()
        )),
        ..Default::default()
    })
}

fn tool_outcome(run: &ToolRun, operation: &str, unit: &str) -> CloudRunOutcome {
    if run.ok {
        CloudRunOutcome::ok(format!("{operation} `{unit}` completed"), true)
    } else {
        CloudRunOutcome::failed(format!("{operation} `{unit}` failed: {}", tool_detail(run)))
    }
}

fn tool_detail(run: &ToolRun) -> String {
    let stdout = run.stdout.trim();
    let stderr = run.stderr.trim();
    let detail = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => "backend command exited unsuccessfully".to_string(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    };
    bounded_output(&detail)
}

/// Keep backend output bounded before it enters a Bus reply or audit record.
/// `journalctl --lines` limits entries, not the size of an individual entry.
fn bounded_output(value: &str) -> String {
    if value.len() <= MAX_OUTPUT_BYTES {
        return value.to_string();
    }
    let keep = MAX_OUTPUT_BYTES.saturating_sub(OUTPUT_TRUNCATION_MARKER.len());
    let mut end = keep.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = String::with_capacity(MAX_OUTPUT_BYTES);
    bounded.push_str(&value[..end]);
    bounded.push_str(OUTPUT_TRUNCATION_MARKER);
    bounded
}

fn bounded_message(prefix: &str, value: &str, suffix: &str) -> String {
    let bounded_value = bounded_output(value);
    bounded_output(&format!("{prefix}{bounded_value}{suffix}"))
}

fn finish_mutation(
    w: &CloudWorker,
    verb_name: &str,
    instance: &str,
    outcome: CloudRunOutcome,
    unavailable: bool,
) -> CloudReply {
    w.audit(verb_name, Some(instance), &outcome);
    let summary = outcome.summary.clone();
    CloudReply {
        ok: outcome.ok,
        verb: verb_name.to_string(),
        audited: true,
        gated: (!outcome.ok && unavailable).then_some(summary.clone()),
        error: (!outcome.ok && !unavailable).then_some(summary.clone()),
        raw_log: Some(summary),
        ..Default::default()
    }
}

fn reject(verb_name: &str, reason: impl Into<String>) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(reason.into()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use mackes_mesh_types::cloud::{
        CloudInstance, EndpointInterface, HealthState, LifecycleAction, ServiceHealth,
    };

    use super::super::super::gate::{ArmedToken, HmacTokenSigner};
    use super::super::super::runner::{CloudRunOutcome, CloudRunner, ToolRun};
    use super::super::super::runner::fake::FakeRunner;
    use super::super::super::{now_ms, CloudWorker};

    const KEY: &[u8] = b"test-mesh-arming-key";

    fn signer() -> HmacTokenSigner {
        HmacTokenSigner::new(KEY.to_vec())
    }

    fn worker(root: &Path, runner: Arc<FakeRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), root.to_path_buf())
            .with_runner(runner)
            .with_signer(Arc::new(signer()))
            .with_auth_root(root.join("auth"))
            .with_db_path(root.join("events.sqlite"))
            .with_bus_root(None)
    }

    struct OversizedLogRunner {
        fail: bool,
    }

    impl CloudRunner for OversizedLogRunner {
        fn probe_tool(&self, tool: &str) -> ServiceHealth {
            ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: "(test)".to_string(),
                state: HealthState::Up,
                latency_ms: Some(1),
                microversion: None,
                version_id: None,
                detail: None,
            }
        }

        fn list_instances(&self) -> Result<Vec<CloudInstance>, String> {
            Ok(Vec::new())
        }

        fn provision(&self) -> CloudRunOutcome {
            CloudRunOutcome::ok("test", true)
        }

        fn configure(&self) -> CloudRunOutcome {
            CloudRunOutcome::ok("test", true)
        }

        fn lifecycle(&self, _action: LifecycleAction, _instance: &str) -> CloudRunOutcome {
            CloudRunOutcome::ok("test", true)
        }

        fn plan_json(&self, _tfvars_json: &str) -> Result<String, String> {
            Ok(String::new())
        }

        fn run_tool(&self, _bin: &str, _args: &[&str]) -> Result<ToolRun, String> {
            if self.fail {
                return Err(format!("é{}", "x".repeat(super::MAX_OUTPUT_BYTES + 1)));
            }
            Ok(ToolRun {
                ok: true,
                stdout: format!("é{}", "x".repeat(super::MAX_OUTPUT_BYTES + 1)),
                stderr: String::new(),
            })
        }
    }

    fn armed_body(verb: &str, mut body: serde_json::Value, target: &str) -> String {
        let unsigned = body.to_string();
        let token = ArmedToken::mint(
            &signer(),
            &format!("nonce-{}", now_ms()),
            now_ms().saturating_add(super::super::super::MAX_AUTH_TTL_MS),
            verb,
            "me",
            target,
            &mackes_mesh_types::cloud::cloud_request_digest(&unsigned).unwrap(),
        )
        .encode();
        body["armed_token"] = serde_json::Value::String(token);
        body.to_string()
    }

    #[test]
    fn unsafe_instance_is_rejected_before_auth_or_runner() {
        let runner = Arc::new(FakeRunner::default());
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(tmp.path(), runner.clone());
        for verb in ["container-restart", "container-logs", "container-destroy"] {
            let reply = w.handle(
                verb,
                r#"{"schema_version":1,"node":"me","instance":"../escape","typed_name":"../escape"}"#,
            );
            assert!(!reply.ok, "{verb} must reject traversal");
            assert!(reply
                .error
                .as_deref()
                .is_some_and(|e| e.contains("path-safe")));
        }
        assert!(runner.tool_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn restart_requires_auth_and_then_invokes_systemctl_with_literal_argv() {
        let runner = Arc::new(FakeRunner::default());
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(tmp.path(), runner.clone());
        let unsigned = serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "instance": "web",
        });
        let gated = w.handle("container-restart", &unsigned.to_string());
        assert!(!gated.ok);
        assert!(gated.gated.is_some());
        assert!(runner.tool_calls.lock().unwrap().is_empty());

        let raw = armed_body("container-restart", unsigned, "web");
        let reply = w.handle("container-restart", &raw);
        assert!(
            reply.ok,
            "error: {:?}, gated: {:?}",
            reply.error, reply.gated
        );
        assert!(reply.audited);
        assert_eq!(
            runner.tool_calls.lock().unwrap().as_slice(),
            &[(
                "systemctl".to_string(),
                vec!["--user".into(), "restart".into(), "web.service".into()]
            )]
        );
    }

    #[test]
    fn logs_are_placement_scoped_and_unavailable_backend_is_honest() {
        let runner = Arc::new(FakeRunner {
            tool_absent: true,
            ..Default::default()
        });
        let tmp = tempfile::tempdir().unwrap();
        let w = worker(tmp.path(), runner.clone());
        let reply = w.handle("container-logs", r#"{"node":"me","instance":"web"}"#);
        assert!(!reply.ok);
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("unavailable")));
        assert_eq!(
            runner.tool_calls.lock().unwrap().as_slice(),
            &[(
                "journalctl".to_string(),
                vec![
                    "--user".into(),
                    "--no-pager".into(),
                    "--lines".into(),
                    "200".into(),
                    "--unit".into(),
                    "web.service".into(),
                ]
            )]
        );
    }

    #[test]
    fn logs_bound_oversized_utf8_backend_output() {
        let tmp = tempfile::tempdir().unwrap();
        let w = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(OversizedLogRunner { fail: false }))
            .with_signer(Arc::new(signer()))
            .with_auth_root(tmp.path().join("auth"))
            .with_db_path(tmp.path().join("events.sqlite"))
            .with_bus_root(None);

        let reply = w.handle("container-logs", r#"{"node":"me","instance":"web"}"#);
        let output = reply.raw_log.expect("successful log reply must include output");
        assert!(reply.ok);
        assert!(output.len() <= super::MAX_OUTPUT_BYTES);
        assert!(output.ends_with(super::OUTPUT_TRUNCATION_MARKER));
        assert!(output.is_char_boundary(output.len()));
    }

    #[test]
    fn unavailable_backend_error_is_bounded_before_reply() {
        let tmp = tempfile::tempdir().unwrap();
        let w = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(OversizedLogRunner { fail: true }))
            .with_signer(Arc::new(signer()))
            .with_auth_root(tmp.path().join("auth"))
            .with_db_path(tmp.path().join("events.sqlite"))
            .with_bus_root(None);

        let reply = w.handle("container-logs", r#"{"node":"me","instance":"web"}"#);
        let detail = reply
            .gated
            .expect("unavailable backend must explain the failed read");
        assert!(!reply.ok);
        assert!(detail.len() <= super::MAX_OUTPUT_BYTES);
        assert!(detail.ends_with(super::OUTPUT_TRUNCATION_MARKER));
    }

    #[test]
    fn destroy_requires_typed_confirmation_and_retracts_desired_entry_after_stop() {
        let runner = Arc::new(FakeRunner::default());
        let tmp = tempfile::tempdir().unwrap();
        let desired = tmp.path().join("mcnf/cloud/desired/me");
        std::fs::create_dir_all(&desired).unwrap();
        std::fs::write(desired.join("web.json"), b"{}\n").unwrap();
        let w = worker(tmp.path(), runner.clone());
        let unsigned = serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "instance": "web",
            "typed_name": "web",
        });
        let raw = armed_body("container-destroy", unsigned, "web");
        let reply = w.handle("container-destroy", &raw);
        assert!(
            reply.ok,
            "error: {:?}, gated: {:?}",
            reply.error, reply.gated
        );
        assert!(reply.audited);
        assert!(!desired.join("web.json").exists());
        assert_eq!(
            runner.tool_calls.lock().unwrap().as_slice(),
            &[(
                "systemctl".to_string(),
                vec![
                    "--user".into(),
                    "disable".into(),
                    "--now".into(),
                    "web.service".into(),
                ]
            )]
        );
    }

    #[test]
    fn destroy_backend_failure_does_not_retract_desired_entry_or_claim_success() {
        let runner = Arc::new(FakeRunner {
            tool_fail: true,
            ..Default::default()
        });
        let tmp = tempfile::tempdir().unwrap();
        let desired = tmp.path().join("mcnf/cloud/desired/me");
        std::fs::create_dir_all(&desired).unwrap();
        std::fs::write(desired.join("web.json"), b"{}\n").unwrap();
        let w = worker(tmp.path(), runner);
        let raw = armed_body(
            "container-destroy",
            serde_json::json!({
                "schema_version": 1,
                "node": "me",
                "instance": "web",
                "typed_name": "web",
            }),
            "web",
        );
        let reply = w.handle("container-destroy", &raw);
        assert!(!reply.ok);
        assert!(reply.error.is_some());
        assert!(desired.join("web.json").exists());
    }
}

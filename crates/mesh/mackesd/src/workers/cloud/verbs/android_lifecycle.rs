//! Retired Android outer-VM lifecycle boundary.
//!
//! Android inventory, guest launch, and VDI-source contracts remain live, but
//! outer-VM lifecycle must be delegated to the typed Workload operation lane.
//! Until that delegation is wired, every valid legacy request fails closed
//! before state persistence, guest cleanup, or a backend effect.

use mackes_mesh_types::android_apps::AospStarterApp;
use mackes_mesh_types::cloud::CloudReply;
use serde::Deserialize;

use super::super::CloudWorker;

const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Start,
    Stop,
    Cancel,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u16,
    node: String,
    workload_id: String,
    request_id: String,
    expected_generation: u64,
    operation: Operation,
    #[serde(default)]
    app: Option<AospStarterApp>,
    #[serde(default)]
    armed_token: Option<String>,
    #[serde(default)]
    typed_name: Option<String>,
}

pub(super) fn authorization_target(raw: &str) -> Result<String, String> {
    Ok(parse(raw)?.workload_id)
}

pub(super) fn handle(_worker: &CloudWorker, verb: &str, raw: &str) -> CloudReply {
    if let Err(error) = parse(raw) {
        return failure(verb, error);
    }
    failure(
        verb,
        "Android outer-VM lifecycle is retired on the cloud lane; typed Workload delegation is not yet available, so nothing changed",
    )
}

fn parse(raw: &str) -> Result<Request, String> {
    if raw.len() > super::MAX_CLOUD_ACTION_BODY_BYTES {
        return Err("Android lifecycle request exceeds the action bound".to_owned());
    }
    let request: Request = serde_json::from_str(raw)
        .map_err(|error| format!("invalid Android lifecycle request: {error}"))?;
    if request.schema_version != SCHEMA_VERSION {
        return Err("unsupported Android lifecycle schema".to_owned());
    }
    super::super::path_key::segment("node", &request.node)?;
    super::super::path_key::segment("workload_id", &request.workload_id)?;
    super::super::path_key::segment("request_id", &request.request_id)?;
    if request.operation == Operation::Start && request.app.is_none() {
        return Err("start requires an approved `app`".to_owned());
    }
    if matches!(request.operation, Operation::Stop | Operation::Cancel) && request.app.is_some() {
        return Err("stop/cancel must not carry an app".to_owned());
    }
    Ok(request)
}

fn failure(verb: &str, error: impl Into<String>) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb.to_owned(),
        error: Some(error.into()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_lifecycle_request_fails_closed_without_state_or_backend_effect() {
        let temporary = tempfile::tempdir().expect("temporary state root");
        let runner =
            std::sync::Arc::new(crate::workers::cloud::runner::fake::FakeRunner::default());
        let worker = CloudWorker::new(
            "node-a".into(),
            "test".into(),
            temporary.path().to_path_buf(),
        )
        .with_runner(runner.clone());
        let reply = handle(
            &worker,
            "android-lifecycle",
            r#"{"schema_version":1,"node":"node-a","workload_id":"android-one","request_id":"stop-1","expected_generation":0,"operation":"stop"}"#,
        );
        assert!(!reply.ok);
        assert!(reply.error.as_deref().is_some_and(|error| {
            error.contains("typed Workload delegation") && error.contains("nothing changed")
        }));
        assert!(runner.calls.lock().expect("runner calls").is_empty());
        assert!(!temporary
            .path()
            .join("mcnf/cloud/android-lifecycle")
            .exists());
    }
}

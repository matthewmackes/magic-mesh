//! Retired legacy `container-deploy` cloud verb.
//!
//! Container lifecycle is owned by the typed Workload authority.  This legacy
//! verb previously accepted arbitrary OCI references and drove a separate
//! Quadlet/Ansible deployment path, which could bypass Workload admission,
//! catalog approval, and the single actuator.  Keep the dispatch target so
//! older callers receive an explicit refusal instead of silently reaching a
//! competing container authority.

use mackes_mesh_types::cloud::CloudReply;

use super::super::CloudWorker;

const RETIRED_MESSAGE: &str =
    "container-deploy is retired; submit a typed Workload operation using the approved catalog image";

/// Refuse the retired adapter before parsing, authenticating, staging, or
/// invoking any external deployment tool.
pub(crate) fn handle(_w: &CloudWorker, verb_name: &str, _raw: &str) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        gated: Some(RETIRED_MESSAGE.to_string()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::super::runner::fake::FakeRunner;
    use super::super::super::CloudWorker;
    use super::{handle, RETIRED_MESSAGE};

    #[test]
    fn retired_deploy_refuses_before_staging_or_runner_activity() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let worker = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_bus_root(None);

        let reply = handle(
            &worker,
            "container-deploy",
            r#"{"schema_version":1,"name":"web","image":"attacker/image:latest"}"#,
        );

        assert!(!reply.ok);
        assert_eq!(reply.verb, "container-deploy");
        assert_eq!(reply.gated.as_deref(), Some(RETIRED_MESSAGE));
        assert!(reply.error.is_none());
        assert!(!tmp.path().join("quadlets").exists());
        assert!(runner.tool_calls.lock().unwrap().is_empty());
    }
}

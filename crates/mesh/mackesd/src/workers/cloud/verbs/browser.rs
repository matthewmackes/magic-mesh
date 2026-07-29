//! WL-ARCH-008 — the typed `browser-provision` Workloads handler.
//!
//! Browser is delivered as a normal [`DeliveryType::DesktopVm`] workload. This
//! verb only declares the stable desired state; it never claims that Chromium or
//! a VM is already available. The separately armed `provision` action renders the
//! complete node slice and asks the configured provider to realize it. Provider
//! or hypervisor failures therefore remain explicit in that existing apply reply.
//!
//! The request requires both an explicit placement node and an explicit workload
//! name. Both are validated before the authorization token is consumed and before
//! the desired-state directory is touched.

use mackes_mesh_types::cloud::{BrowserVmProfile, CloudReply, WorkloadSpec};

use super::super::reconcile;
use super::super::CloudWorker;
use super::CloudActionBody;

/// Handle one `action/cloud/browser-provision` request.
pub(super) fn handle(w: &CloudWorker, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    build_reply(&w.state_root, verb_name, body)
}

/// Validate the typed target used by the armed-token capability.
///
/// Returning the original validated slices keeps the capability target and the
/// persisted filename byte-identical; trimming here would permit an ambiguous
/// request to authorize one spelling and persist another.
pub(super) fn authorization_target(body: &CloudActionBody) -> Result<&str, String> {
    let (_, name) = validated_target(body)?;
    Ok(name)
}

/// Build the baseline Browser VM desired-state spec.
#[must_use]
pub(super) fn browser_spec(node: &str, name: &str) -> WorkloadSpec {
    BrowserVmProfile::default().workload_spec(node, name)
}

fn build_reply(
    state_root: &std::path::Path,
    verb_name: &str,
    body: &CloudActionBody,
) -> CloudReply {
    let (node, name) = match validated_target(body) {
        Ok(target) => target,
        Err(error) => return reject(verb_name, error),
    };
    let spec = browser_spec(node, name);

    match reconcile::write_desired_doc(state_root, &spec) {
        Ok(()) => CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            desired: Some(vec![spec]),
            ..Default::default()
        },
        Err(error) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!(
                "browser-provision built the Desktop VM desired state for `{name}` on `{node}` \
                 but could not persist it: {error}"
            )),
            desired: Some(vec![spec]),
            ..Default::default()
        },
    }
}

fn validated_target(body: &CloudActionBody) -> Result<(&str, &str), String> {
    if body.node.is_empty() {
        return Err("`browser-provision` requires an explicit placement `node`".to_string());
    }
    let node = super::super::path_key::segment("node", &body.node)?;

    let name = body
        .name
        .as_deref()
        .ok_or_else(|| "`browser-provision` requires an explicit workload `name`".to_string())?;
    let name = super::super::path_key::file_stem("name", name, ".json")?;
    Ok((node, name))
}

fn reject(verb_name: &str, reason: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(reason),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::cloud::reconcile;
    use mackes_mesh_types::cloud::DeliveryType;
    use tempfile::tempdir;

    fn body(node: &str, name: Option<&str>) -> CloudActionBody {
        CloudActionBody {
            node: node.to_string(),
            name: name.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn browser_profile_is_the_baseline_desktop_vm_shape() {
        let spec = browser_spec("eagle", "browser-eagle");
        assert_eq!(spec.delivery_type, DeliveryType::DesktopVm);
        assert_eq!(spec.node, "eagle");
        assert_eq!(spec.name, "browser-eagle");
        assert_eq!(spec.vcpu, 4);
        assert_eq!(spec.memory_mb, 8192);
        assert_eq!(spec.disk_gb, 64);
        assert!(spec.image.is_none());
        assert!(!spec.network_isolation);
    }

    #[test]
    fn browser_provision_persists_the_typed_desired_spec_deterministically() {
        let tmp = tempdir().unwrap();
        let request = body("eagle", Some("browser-eagle"));
        let reply = build_reply(tmp.path(), "browser-provision", &request);
        assert!(reply.ok, "error: {:?}", reply.error);
        assert_eq!(
            reply.desired.as_ref().unwrap(),
            &[browser_spec("eagle", "browser-eagle")]
        );
        assert_eq!(
            reconcile::read_desired_slice(tmp.path(), "eagle"),
            vec![browser_spec("eagle", "browser-eagle")]
        );
    }

    #[test]
    fn browser_provision_requires_explicit_node_and_name() {
        let tmp = tempdir().unwrap();
        for request in [body("", Some("browser")), body("eagle", None)] {
            let reply = build_reply(tmp.path(), "browser-provision", &request);
            assert!(!reply.ok);
            assert!(reply.desired.is_none());
            assert!(reply.error.is_some());
        }
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn browser_provision_rejects_unsafe_targets_before_writing() {
        let tmp = tempdir().unwrap();
        for request in [
            body("eagle/other", Some("browser")),
            body("eagle", Some("../browser")),
            body("eagle", Some("browser name")),
            body("eagle ", Some("browser")),
        ] {
            let reply = build_reply(tmp.path(), "browser-provision", &request);
            assert!(!reply.ok);
            assert!(reply.desired.is_none());
        }
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn authorization_target_rejects_invalid_input_without_a_canonicalized_alias() {
        let padded = body("eagle", Some(" browser"));
        assert!(authorization_target(&padded).is_err());
        let valid = body("eagle", Some("browser-eagle"));
        assert_eq!(authorization_target(&valid), Ok("browser-eagle"));
    }

    #[test]
    fn dispatch_fails_closed_before_touching_state_without_an_armed_token() {
        let tmp = tempdir().unwrap();
        let worker = CloudWorker::new(
            "eagle".to_string(),
            "peer:eagle".to_string(),
            tmp.path().to_path_buf(),
        )
        .with_bus_root(None);
        let reply = worker.handle(
            "browser-provision",
            r#"{"schema_version":1,"node":"eagle","name":"browser-eagle"}"#,
        );
        assert!(!reply.ok);
        assert!(reply.gated.as_deref().is_some_and(|reason| {
            reason.contains("no armed token") && reason.contains("nothing changed")
        }));
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn persistence_failures_are_explicit_and_never_reported_as_success() {
        let tmp = tempdir().unwrap();
        let state_root = tmp.path().join("not-a-directory");
        std::fs::write(&state_root, "occupied").unwrap();
        let reply = build_reply(
            &state_root,
            "browser-provision",
            &body("eagle", Some("browser")),
        );
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("could not persist"));
    }
}

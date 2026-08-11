//! WL-ARCH-008 — the typed `browser-provision` Workloads handler.
//!
//! Browser is delivered as a normal [`DeliveryType::DesktopVm`] workload. This
//! verb only declares the stable desired state; it never claims that Chromium or
//! a VM is already available. The typed Workload row operation realizes the
//! declaration and reports provider or hypervisor failures through the canonical
//! Workload status projection.
//!
//! The request requires an explicit placement node, workload name, and immutable
//! guest-image digest. All are validated before the authorization token is
//! consumed and before the desired-state directory is touched.

use mackes_mesh_types::cloud::{BrowserVmProfile, CloudReply, WorkloadSpec};

use super::super::reconcile;
use super::super::CloudWorker;
use super::CloudActionBody;

const BROWSER_VM_WORKLOAD_NAME: &str = "browser-vm";

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
    let (_, name, _) = validated_target(body)?;
    Ok(name)
}

/// Build the baseline Browser VM desired-state spec.
#[must_use]
pub(super) fn browser_spec(node: &str, _name: &str, image_digest: &str) -> WorkloadSpec {
    // The Browser surface selects exactly one VM route. Do not let an internal
    // caller mint an alternate workload identity that can never be selected by
    // the runtime controller (or shadow the canonical desired-state file).
    let mut spec = BrowserVmProfile::default().workload_spec(node, BROWSER_VM_WORKLOAD_NAME);
    spec.image_digest = Some(image_digest.to_owned());
    spec
}

fn build_reply(
    state_root: &std::path::Path,
    verb_name: &str,
    body: &CloudActionBody,
) -> CloudReply {
    let (node, name, image_digest) = match validated_target(body) {
        Ok(target) => target,
        Err(error) => return reject(verb_name, error),
    };
    let spec = browser_spec(node, name, image_digest);

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

fn validated_target(body: &CloudActionBody) -> Result<(&str, &str, &str), String> {
    if body.node.is_empty() {
        return Err("`browser-provision` requires an explicit placement `node`".to_string());
    }
    let node = super::super::path_key::segment("node", &body.node)?;

    let name = body
        .name
        .as_deref()
        .ok_or_else(|| "`browser-provision` requires an explicit workload `name`".to_string())?;
    let name = super::super::path_key::file_stem("name", name, ".json")?;
    if name != BROWSER_VM_WORKLOAD_NAME {
        return Err(format!(
            "`browser-provision` workload `name` must be `{BROWSER_VM_WORKLOAD_NAME}` so Surface::Browser can select the stable route"
        ));
    }
    let image_digest = body.image_digest.as_deref().ok_or_else(|| {
        "`browser-provision` requires an immutable `image_digest` (sha256:<64-hex>)".to_string()
    })?;
    validate_image_digest(image_digest)?;
    Ok((node, name, image_digest))
}

fn validate_image_digest(digest: &str) -> Result<(), String> {
    let hex = digest.strip_prefix("sha256:").ok_or_else(|| {
        "`browser-provision` image_digest must use the sha256:<64-hex> form".to_string()
    })?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(
            "`browser-provision` image_digest must use the sha256:<64-hex> form".to_string(),
        );
    }
    Ok(())
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

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn body(node: &str, name: Option<&str>) -> CloudActionBody {
        CloudActionBody {
            node: node.to_string(),
            name: name.map(str::to_string),
            image_digest: Some(DIGEST.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn browser_profile_is_the_baseline_desktop_vm_shape() {
        let spec = browser_spec("eagle", "browser-eagle", DIGEST);
        assert_eq!(spec.delivery_type, DeliveryType::DesktopVm);
        assert_eq!(spec.node, "eagle");
        assert_eq!(spec.name, BROWSER_VM_WORKLOAD_NAME);
        assert_eq!(spec.vcpu, 4);
        assert_eq!(spec.memory_mb, 8192);
        assert_eq!(spec.disk_gb, 64);
        assert_eq!(spec.image.as_deref(), Some("browser-vm-chromium"));
        assert_eq!(spec.image_digest.as_deref(), Some(DIGEST));
        assert!(!spec.network_isolation);
    }

    #[test]
    fn browser_spec_cannot_mint_a_noncanonical_runtime_identity() {
        let spec = browser_spec("eagle", "browser-shadow", DIGEST);

        assert_eq!(spec.name, BROWSER_VM_WORKLOAD_NAME);
        assert_eq!(spec.image.as_deref(), Some("browser-vm-chromium"));
        assert_eq!(spec.node, "eagle");
    }

    #[test]
    fn browser_provision_persists_the_typed_desired_spec_deterministically() {
        let tmp = tempdir().unwrap();
        let request = body("eagle", Some(BROWSER_VM_WORKLOAD_NAME));
        let reply = build_reply(tmp.path(), "browser-provision", &request);
        assert!(reply.ok, "error: {:?}", reply.error);
        assert_eq!(
            reply.desired.as_ref().unwrap(),
            &[browser_spec("eagle", BROWSER_VM_WORKLOAD_NAME, DIGEST)]
        );
        assert_eq!(
            reconcile::read_desired_slice(tmp.path(), "eagle"),
            vec![browser_spec("eagle", BROWSER_VM_WORKLOAD_NAME, DIGEST)]
        );
    }

    #[test]
    fn browser_provision_requires_explicit_node_and_name() {
        let tmp = tempdir().unwrap();
        for request in [
            body("", Some(BROWSER_VM_WORKLOAD_NAME)),
            body("eagle", None),
        ] {
            let reply = build_reply(tmp.path(), "browser-provision", &request);
            assert!(!reply.ok);
            assert!(reply.desired.is_none());
            assert!(reply.error.is_some());
        }
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn browser_provision_rejects_an_alias_that_surface_browser_cannot_select() {
        let tmp = tempdir().unwrap();
        let reply = build_reply(
            tmp.path(),
            "browser-provision",
            &body("eagle", Some("browser-eagle")),
        );
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("must be `browser-vm`")));
        assert!(reply.desired.is_none());
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
        let valid = body("eagle", Some(BROWSER_VM_WORKLOAD_NAME));
        assert_eq!(authorization_target(&valid), Ok(BROWSER_VM_WORKLOAD_NAME));
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
            r#"{"schema_version":1,"node":"eagle","name":"browser-vm","image_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#,
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
            &body("eagle", Some(BROWSER_VM_WORKLOAD_NAME)),
        );
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("could not persist"));
    }
}

//! WL-FUNC-018 — the typed `app-provision` Workloads handler.
//!
//! This handler records one admitted guest-owned Flatpak App VM in the existing
//! per-node desired-state plane. It does not execute a catalog string, install
//! a host Flatpak, or claim that a guest is running; the normal armed
//! `provision` path remains responsible for realizing the node's complete
//! desired slice. Repeated requests converge on the same workload name and
//! session identity.

use mackes_mesh_types::app_catalog::is_valid_flatpak_app_id;
use mackes_mesh_types::cloud::{AppVmProfile, CloudReply};
use mackes_mesh_types::vdi_session::AppVmLaunchRequest;

use super::super::path_key;
use super::super::reconcile;
use super::super::CloudActionBody;
use super::super::CloudWorker;
use super::app_image;

/// Handle one `action/cloud/app-provision` request.
pub(super) fn handle(w: &CloudWorker, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    build_reply_with_runtime_bus(
        &w.state_root,
        w.bus_root.as_deref().or(Some(w.state_root.as_path())),
        verb_name,
        body,
    )
}

/// Return the exact capability target for an admitted App VM declaration.
pub(super) fn authorization_target(body: &CloudActionBody) -> Result<String, String> {
    let (node, name, request) = validated_request(body)?;
    Ok(format!(
        "app-vm:{node}:{name}:{}:{}",
        request.app_id, request.catalog_revision
    ))
}

fn build_reply(
    state_root: &std::path::Path,
    verb_name: &str,
    body: &CloudActionBody,
) -> CloudReply {
    build_reply_with_profile(
        state_root,
        None,
        verb_name,
        body,
        AppVmProfile::default(),
    )
}

fn build_reply_with_runtime_bus(
    state_root: &std::path::Path,
    runtime_bus_root: Option<&std::path::Path>,
    verb_name: &str,
    body: &CloudActionBody,
) -> CloudReply {
    build_reply_with_profile(
        state_root,
        runtime_bus_root,
        verb_name,
        body,
        AppVmProfile::default(),
    )
}

fn build_reply_with_profile(
    state_root: &std::path::Path,
    runtime_bus_root: Option<&std::path::Path>,
    verb_name: &str,
    body: &CloudActionBody,
    profile: AppVmProfile,
) -> CloudReply {
    let (node, name, request) = match validated_request(body) {
        Ok(value) => value,
        Err(error) => return reject(verb_name, error),
    };
    if let Err(error) = profile.admit(&request) {
        return reject(verb_name, format!("App VM admission rejected: {error}"));
    }
    let image_admission = app_image::check(state_root, &request.guest_profile, now_ms());
    if !image_admission.is_admitted() {
        return reject(
            verb_name,
            format!(
                "App VM image `{}` is not admitted ({})",
                app_image::APP_VM_IMAGE_NAME,
                image_admission.reason()
            ),
        );
    }
    let spec = AppVmProfile::default().workload_spec(node, name, request);

    match persist_declaration(state_root, runtime_bus_root, now_ms() as i64, &spec) {
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
                "app-provision built the App VM desired state for `{name}` on `{node}` \
                 but could not persist it: {error}"
            )),
            desired: Some(vec![spec]),
            ..Default::default()
        },
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Persist an App VM declaration without allowing replayed or stale requests to
/// replace a different workload/session under the same stable workload key.
///
/// A byte-for-byte replay is a true no-op. An update that retains the admitted
/// app, guest profile, and session identity may refresh catalog/capability/resume
/// intent. Any other replacement is rejected before the atomic writer runs.
fn persist_declaration(
    state_root: &std::path::Path,
    runtime_bus_root: Option<&std::path::Path>,
    now_ms: i64,
    spec: &mackes_mesh_types::cloud::WorkloadSpec,
) -> Result<(), String> {
    let requested_app = spec
        .app
        .as_ref()
        .ok_or_else(|| format!("workload `{}` has no typed App VM declaration", spec.name))?;
    if let Some((owner_node, owner_name)) =
        reconcile::app_session_owner(state_root, &requested_app.session_id)?
    {
        if owner_node != spec.node || owner_name != spec.name {
            return Err(format!(
                "App VM session `{}` is already owned by workload `{owner_name}` on node `{owner_node}`; refusing a second desired owner",
                requested_app.session_id
            ));
        }
    }

    let existing = reconcile::read_desired_doc_strict(state_root, &spec.node, &spec.name)?;
    if let Some(runtime_bus_root) = runtime_bus_root {
        let runtime =
            app_image::check_runtime_evidence(Some(runtime_bus_root), requested_app, now_ms);
        let runtime_required = existing.is_some() || requested_app.resume;
        if runtime_required && !runtime.is_usable() {
            return Err(format!(
                "App VM guest runtime readiness is not admitted for session `{}` ({})",
                requested_app.session_id,
                runtime.reason()
            ));
        }
        // A fresh declaration is allowed to enter desired state without a
        // guest observation: first boot is the operation that creates it. If a
        // record does exist, however, it has already claimed this session and
        // must pass the same identity/freshness/terminal checks as a resume.
        if existing.is_none()
            && !requested_app.resume
            && !matches!(runtime, app_image::AppVmRuntimeAdmission::Missing(_))
        {
            return Err(format!(
                "App VM guest runtime readiness is not admitted for new session `{}` ({})",
                requested_app.session_id,
                runtime.reason()
            ));
        }
    }
    let Some(existing) = existing else {
        return reconcile::write_desired_doc(state_root, spec);
    };

    if existing == *spec {
        return Ok(());
    }

    if existing.delivery_type != mackes_mesh_types::cloud::DeliveryType::AppVm {
        return Err(format!(
            "workload `{}` already declares delivery type `{}`; refusing App VM replay",
            spec.name,
            existing.delivery_type.as_str()
        ));
    }

    let Some(existing_app) = existing.app.as_ref() else {
        return Err(format!(
            "workload `{}` has an App VM delivery type but no typed session identity; refusing replacement",
            spec.name
        ));
    };
    let Some(requested_app) = spec.app.as_ref() else {
        return Err(format!(
            "workload `{}` has no typed App VM declaration; refusing replacement",
            spec.name
        ));
    };
    if existing_app.app_id != requested_app.app_id
        || existing_app.guest_profile != requested_app.guest_profile
        || existing_app.session_id != requested_app.session_id
    {
        return Err(format!(
            "workload `{}` is bound to App VM session `{}`; refusing stale session replay `{}`",
            spec.name, existing_app.session_id, requested_app.session_id
        ));
    }

    reconcile::write_desired_doc(state_root, spec)
}

fn validated_request(body: &CloudActionBody) -> Result<(&str, &str, AppVmLaunchRequest), String> {
    let node = path_key::segment("node", &body.node)?;
    let name = body
        .name
        .as_deref()
        .ok_or_else(|| "`app-provision` requires an explicit workload `name`".to_owned())?;
    let name = path_key::file_stem("name", name, ".json")?;
    let app_id = body
        .app_id
        .as_deref()
        .ok_or_else(|| "`app-provision` requires an explicit `app_id`".to_owned())?;
    if !is_valid_flatpak_app_id(app_id) {
        return Err("`app-provision` requires a valid reverse-DNS Flatpak `app_id`".to_owned());
    }
    let catalog_revision = body
        .catalog_revision
        .as_deref()
        .ok_or_else(|| "`app-provision` requires an explicit catalog revision".to_owned())?;
    let guest_profile = body
        .guest_profile
        .as_deref()
        .ok_or_else(|| "`app-provision` requires an explicit guest profile".to_owned())?;
    let session_id = body
        .session_id
        .as_deref()
        .ok_or_else(|| "`app-provision` requires an explicit session identity".to_owned())?;
    let request = AppVmLaunchRequest::new(
        app_id,
        catalog_revision,
        guest_profile,
        body.requested_capabilities.clone(),
        session_id,
        body.resume,
    )
    .map_err(|error| format!("invalid App VM declaration: {error}"))?;
    Ok((node, name, request))
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
    use tempfile::tempdir;

    fn admit_app_image(root: &std::path::Path) {
        let version = "2026.07.31";
        let dir = crate::image_catalog::images_dir(root)
            .join(app_image::APP_VM_IMAGE_NAME)
            .join(version);
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("disk.qcow2");
        std::fs::write(&artifact, b"app-vm-test-image").unwrap();
        use sha2::{Digest, Sha256};
        let sha = format!("{:x}", Sha256::digest(b"app-vm-test-image"));
        std::fs::write(dir.join("image.sha256"), &sha).unwrap();
        std::fs::write(
            dir.join("manifest.toml"),
            format!(
                "name = \"{}\"\nkind = \"vm\"\nversion = \"{version}\"\nprofile = \"app_vm\"\n",
                app_image::APP_VM_IMAGE_NAME
            ),
        )
        .unwrap();
        std::fs::write(
            crate::image_catalog::images_dir(root)
                .join(app_image::APP_VM_IMAGE_NAME)
                .join("PROMOTED"),
            version,
        )
        .unwrap();
        let now = now_ms();
        std::fs::write(
            dir.join("admission.json"),
            serde_json::json!({
                "schema_version": 1,
                "image_name": app_image::APP_VM_IMAGE_NAME,
                "image_version": version,
                "guest_profile": "wayland-standard",
                "sha256": sha,
                "issued_at_ms": now.saturating_sub(1000),
                "expires_at_ms": now + 60_000,
                "signature": "publisher-signature"
            })
            .to_string(),
        )
        .unwrap();
    }

    fn body(node: &str, name: &str) -> CloudActionBody {
        CloudActionBody {
            node: node.to_owned(),
            name: Some(name.to_owned()),
            app_id: Some("org.example.Writer".to_owned()),
            catalog_revision: Some("catalog-7".to_owned()),
            guest_profile: Some("wayland-standard".to_owned()),
            requested_capabilities: vec!["clipboard".to_owned()],
            session_id: Some("app-session-7".to_owned()),
            resume: true,
            ..Default::default()
        }
    }

    #[test]
    fn app_provision_persists_typed_guest_state() {
        let tmp = tempdir().unwrap();
        admit_app_image(tmp.path());
        let reply = build_reply(tmp.path(), "app-provision", &body("eagle", "writer"));
        assert!(reply.ok, "error: {:?}", reply.error);
        let spec = reply.desired.unwrap().pop().unwrap();
        assert_eq!(
            spec.delivery_type,
            mackes_mesh_types::cloud::DeliveryType::AppVm
        );
        assert!(spec.network_isolation);
        assert_eq!(spec.app.as_ref().unwrap().app_id, "org.example.Writer");
        assert_eq!(
            reconcile::read_desired_slice(tmp.path(), "eagle"),
            vec![spec]
        );
    }

    #[test]
    fn identical_app_provision_replay_is_a_noop() {
        let tmp = tempdir().unwrap();
        admit_app_image(tmp.path());
        let request = body("eagle", "writer");
        let first = build_reply(tmp.path(), "app-provision", &request);
        assert!(first.ok, "error: {:?}", first.error);
        let path = reconcile::desired_doc_path(tmp.path(), "eagle", "writer").unwrap();
        let before = std::fs::read(&path).unwrap();

        let replay = build_reply(tmp.path(), "app-provision", &request);
        assert!(replay.ok, "error: {:?}", replay.error);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn daemon_replay_refuses_to_resume_without_guest_runtime_evidence() {
        let tmp = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(tmp.path());
        let mut request = body("eagle", "writer");
        request.resume = false;
        let first = build_reply_with_runtime_bus(
            tmp.path(),
            Some(bus.path()),
            "app-provision",
            &request,
        );
        assert!(first.ok, "initial declaration should await first boot evidence");

        let replay = build_reply_with_runtime_bus(
            tmp.path(),
            Some(bus.path()),
            "app-provision",
            &request,
        );
        assert!(!replay.ok);
        assert!(replay
            .error
            .as_deref()
            .is_some_and(|error| error.contains("missing")));
        assert_eq!(reconcile::read_desired_slice(tmp.path(), "eagle").len(), 1);
    }

    #[test]
    fn stale_session_replay_is_rejected_without_overwriting_the_admitted_session() {
        let tmp = tempdir().unwrap();
        admit_app_image(tmp.path());
        let request = body("eagle", "writer");
        let first = build_reply(tmp.path(), "app-provision", &request);
        assert!(first.ok, "error: {:?}", first.error);
        let original = reconcile::read_desired_slice(tmp.path(), "eagle");

        let mut stale = request;
        stale.session_id = Some("stale-session".to_owned());
        let reply = build_reply(tmp.path(), "app-provision", &stale);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("stale session replay"));
        assert_eq!(reconcile::read_desired_slice(tmp.path(), "eagle"), original);
    }

    #[test]
    fn one_session_cannot_be_declared_under_a_second_workload_name() {
        let tmp = tempdir().unwrap();
        admit_app_image(tmp.path());
        let request = body("eagle", "writer");
        let first = build_reply(tmp.path(), "app-provision", &request);
        assert!(first.ok, "error: {:?}", first.error);

        let mut second = request;
        second.name = Some("writer-copy".to_owned());
        let reply = build_reply(tmp.path(), "app-provision", &second);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("second desired owner"));
        assert_eq!(reconcile::read_desired_slice(tmp.path(), "eagle").len(), 1);
    }

    #[test]
    fn one_session_cannot_move_to_a_second_placement_node() {
        let tmp = tempdir().unwrap();
        admit_app_image(tmp.path());
        let first = build_reply(tmp.path(), "app-provision", &body("eagle", "writer"));
        assert!(first.ok, "error: {:?}", first.error);

        let second = build_reply(tmp.path(), "app-provision", &body("otter", "writer"));
        assert!(!second.ok);
        assert!(second.error.unwrap().contains("second desired owner"));
        assert!(reconcile::read_desired_slice(tmp.path(), "otter").is_empty());
    }

    #[test]
    fn app_provision_does_not_replace_a_non_app_workload_with_the_same_name() {
        let tmp = tempdir().unwrap();
        admit_app_image(tmp.path());
        let existing = mackes_mesh_types::cloud::WorkloadSpec {
            name: "writer".to_owned(),
            delivery_type: mackes_mesh_types::cloud::DeliveryType::ServiceVm,
            node: "eagle".to_owned(),
            vcpu: 2,
            memory_mb: 2048,
            disk_gb: 20,
            image: None,
            image_digest: None,
            network_isolation: false,
            raw_hcl: None,
            app: None,
        };
        reconcile::write_desired_doc(tmp.path(), &existing).unwrap();

        let reply = build_reply(tmp.path(), "app-provision", &body("eagle", "writer"));
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("delivery type"));
        assert_eq!(
            reconcile::read_desired_slice(tmp.path(), "eagle"),
            vec![existing]
        );
    }

    #[test]
    fn app_provision_requires_every_lifecycle_identity() {
        let tmp = tempdir().unwrap();
        let mut request = body("eagle", "writer");
        request.session_id = None;
        let reply = build_reply(tmp.path(), "app-provision", &request);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("session identity"));
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn app_provision_refuses_to_write_without_admitted_image_evidence() {
        let tmp = tempdir().unwrap();
        let reply = build_reply(tmp.path(), "app-provision", &body("eagle", "writer"));
        assert!(!reply.ok);
        assert!(reply.error.as_deref().is_some_and(|error| {
            error.contains("not admitted") && error.contains("unavailable")
        }));
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn app_provision_rejects_invalid_catalog_identity_before_writing() {
        let tmp = tempdir().unwrap();
        let mut request = body("eagle", "writer");
        request.app_id = Some("/tmp/command".to_owned());
        let reply = build_reply(tmp.path(), "app-provision", &request);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("reverse-DNS"));
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn app_provision_rejects_unknown_guest_profile_before_writing() {
        let tmp = tempdir().unwrap();
        let mut request = body("eagle", "writer");
        request.guest_profile = Some("arbitrary-image".to_owned());
        let reply = build_reply(tmp.path(), "app-provision", &request);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("guest_profile"));
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn app_provision_rejects_unsupported_capability_before_image_or_persistence() {
        let tmp = tempdir().unwrap();
        let mut request = body("eagle", "writer");
        request.requested_capabilities = vec!["gpu".to_owned()];
        let reply = build_reply(tmp.path(), "app-provision", &request);
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("capability `gpu`")));
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn app_provision_rejects_unsafe_profile_resources_before_persistence() {
        let tmp = tempdir().unwrap();
        let request = body("eagle", "writer");
        let profile = AppVmProfile {
            vcpu: 0,
            ..AppVmProfile::default()
        };
        let reply = build_reply_with_profile(
            tmp.path(),
            None,
            "app-provision",
            &request,
            profile,
        );
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("vcpu 0")));
        assert!(!tmp.path().join("mcnf").exists());
    }

    #[test]
    fn authorization_target_is_stable_and_typed() {
        let target = authorization_target(&body("eagle", "writer")).unwrap();
        assert_eq!(target, "app-vm:eagle:writer:org.example.Writer:catalog-7");
    }
}

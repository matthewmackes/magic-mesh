//! WL-FUNC-018 — the typed `app-provision` Workloads handler.
//!
//! This handler records one admitted guest-owned Flatpak App VM in the existing
//! per-node desired-state plane, then emits the typed Workload `StartAndAttach`
//! operation that realizes a first boot. It does not execute a catalog string,
//! install a host Flatpak, or claim that a guest is running. Repeated requests
//! converge on the same workload, operation, and session identities.

use mackes_mesh_types::app_catalog::is_valid_flatpak_app_id;
use mackes_mesh_types::cloud::{
    cloud_request_digest, AppVmProfile, CloudArmedToken, CloudReply,
    CLOUD_ACTION_SCHEMA_VERSION,
};
use mackes_mesh_types::vdi_session::{AppVmLaunchRequest, SessionRequest};
use mackes_mesh_types::workloads::{
    WorkloadAttachmentProtocol, WorkloadBackend, WorkloadId, WorkloadOperationAction,
    WorkloadOperationRequest, WorkloadResources, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    WORKLOAD_OPERATION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use sha2::{Digest, Sha256};

use super::super::path_key;
use super::super::reconcile;
use super::super::CloudActionBody;
use super::super::CloudWorker;
use super::app_image;

const WORKLOAD_AUTH_VERB: &str = "workload-operation";
const VDI_AUTH_VERB: &str = "vdi-session-open";
const VDI_AUTH_NODE: &str = "vdi-session";
const WORKLOAD_AUTH_TTL_MS: i64 = 30_000;
const APP_VM_START_DEADLINE_MS: u64 = 5 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationPersistence {
    Created,
    HandoffRetry,
    Converged,
    Updated,
    Recovered,
}

/// Handle one `action/cloud/app-provision` request.
pub(super) fn handle(w: &CloudWorker, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    let Some(bus_root) = w.bus_root() else {
        return reject(
            verb_name,
            "App VM runtime Bus is unavailable; lifecycle admission failed closed".to_owned(),
        );
    };
    if !w.arm_capable {
        return reject(
            verb_name,
            "App VM Workload handoff is unavailable because mutation signing is not configured"
                .to_owned(),
        );
    }
    build_reply_with_profile(
        &w.state_root,
        Some(&bus_root),
        verb_name,
        body,
        AppVmProfile::default(),
        Some((w, &bus_root)),
    )
}

/// Return the exact capability target for an admitted App VM declaration.
pub(super) fn authorization_target(body: &CloudActionBody) -> Result<String, String> {
    let (node, name, _, request) = validated_request(body)?;
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
        None,
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
        None,
    )
}

fn build_reply_with_profile(
    state_root: &std::path::Path,
    runtime_bus_root: Option<&std::path::Path>,
    verb_name: &str,
    body: &CloudActionBody,
    profile: AppVmProfile,
    workload_handoff: Option<(&CloudWorker, &std::path::Path)>,
) -> CloudReply {
    let (node, name, client_peer, request) = match validated_request(body) {
        Ok(value) => value,
        Err(error) => return reject(verb_name, error),
    };
    if let Err(error) = profile.admit(&request) {
        return reject(verb_name, format!("App VM admission rejected: {error}"));
    }
    let image_admission = app_image::check(state_root, &request.guest_profile, now_ms());
    let image_version = match &image_admission {
        app_image::AppVmImageAdmission::Admitted { version } => version.clone(),
        _ => {
            return reject(
                verb_name,
                format!(
                    "App VM image `{}` is not admitted ({})",
                    app_image::APP_VM_IMAGE_NAME,
                    image_admission.reason()
                ),
            );
        }
    };
    let spec = profile.workload_spec(node, name, request);

    if let Some((worker, bus_root)) = workload_handoff {
        if let Err(error) = admit_open_app_replay(worker, bus_root, &spec, client_peer) {
            return reject(
                verb_name,
                format!(
                    "App VM session replay for `{name}` on `{node}` failed identity admission: {error}"
                ),
            );
        }
    }

    match persist_declaration(state_root, runtime_bus_root, now_ms() as i64, &spec) {
        Ok(persistence) => {
            if matches!(
                persistence,
                DeclarationPersistence::Created | DeclarationPersistence::HandoffRetry
            ) {
                if let Some((worker, bus_root)) = workload_handoff {
                    if let Err(error) = publish_start_and_attach(
                        worker,
                        bus_root,
                        &spec,
                        &image_version,
                        now_ms(),
                    ) {
                        return CloudReply {
                            ok: false,
                            verb: verb_name.to_string(),
                            error: Some(format!(
                                "app-provision persisted App VM `{name}` on `{node}` but could not publish its Workload StartAndAttach handoff: {error}"
                            )),
                            desired: Some(vec![spec]),
                            ..Default::default()
                        };
                    }
                }
            }
            if let Some((worker, bus_root)) = workload_handoff {
                if let Err(error) = publish_open_app(worker, bus_root, &spec, client_peer, now_ms())
                {
                    return CloudReply {
                        ok: false,
                        verb: verb_name.to_string(),
                        error: Some(format!(
                            "app-provision persisted App VM `{name}` on `{node}` but could not publish its VDI OpenApp handoff: {error}"
                        )),
                        desired: Some(vec![spec]),
                        ..Default::default()
                    };
                }
            }
            CloudReply {
                ok: true,
                verb: verb_name.to_string(),
                desired: Some(vec![spec]),
                ..Default::default()
            }
        }
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
/// app, catalog revision, guest profile, and session identity may refresh
/// capability/resume intent. Any other replacement is rejected before the
/// atomic writer runs.
fn persist_declaration(
    state_root: &std::path::Path,
    runtime_bus_root: Option<&std::path::Path>,
    now_ms: i64,
    spec: &mackes_mesh_types::cloud::WorkloadSpec,
) -> Result<DeclarationPersistence, String> {
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
    let mut persistence = if existing.is_some() {
        DeclarationPersistence::Updated
    } else {
        DeclarationPersistence::Created
    };
    if let Some(runtime_bus_root) = runtime_bus_root {
        let runtime = app_image::check_runtime_evidence(
            Some(runtime_bus_root),
            requested_app,
            &spec.name,
            now_ms,
        );
        if existing.as_ref().is_some_and(|current| current == spec) {
            if matches!(&runtime, app_image::AppVmRuntimeAdmission::Missing(_)) {
                return Ok(DeclarationPersistence::HandoffRetry);
            }
            if !runtime.is_usable() {
                return Err(format!(
                    "App VM guest runtime readiness is not admitted for session `{}` ({})",
                    requested_app.session_id,
                    runtime.reason()
                ));
            }
            return Ok(DeclarationPersistence::Converged);
        } else if existing.is_some() {
            if !runtime.is_usable() {
                return Err(format!(
                    "App VM guest runtime readiness is not admitted for session `{}` ({})",
                    requested_app.session_id,
                    runtime.reason()
                ));
            }
        } else {
            // Front Door deliberately sends one idempotent "place or resume"
            // intent. Missing evidence means there is nothing to resume, so the
            // first declaration must be allowed to cold boot. Existing usable
            // evidence may recover desired state only when resume was requested;
            // stale, terminal, malformed, or cross-VM evidence always refuses.
            let cold_start = matches!(
                &runtime,
                app_image::AppVmRuntimeAdmission::Missing(_)
            );
            let admitted_recovery = requested_app.resume && runtime.is_usable();
            if !cold_start && !admitted_recovery {
                return Err(format!(
                    "App VM guest runtime readiness is not admitted for new session `{}` ({})",
                    requested_app.session_id,
                    runtime.reason()
                ));
            }
            if admitted_recovery {
                persistence = DeclarationPersistence::Recovered;
            }
        }
    } else if existing.as_ref().is_some_and(|current| current == spec) {
        return Ok(DeclarationPersistence::Converged);
    }
    let Some(existing) = existing else {
        reconcile::write_desired_doc(state_root, spec)?;
        return Ok(persistence);
    };

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
        || existing_app.catalog_revision != requested_app.catalog_revision
        || existing_app.guest_profile != requested_app.guest_profile
        || existing_app.session_id != requested_app.session_id
    {
        return Err(format!(
            "workload `{}` is bound to App VM session `{}` at catalog revision `{}`; refusing conflicting session/revision replay `{}` at `{}`",
            spec.name,
            existing_app.session_id,
            existing_app.catalog_revision,
            requested_app.session_id,
            requested_app.catalog_revision
        ));
    }

    reconcile::write_desired_doc(state_root, spec)?;
    Ok(DeclarationPersistence::Updated)
}

fn publish_start_and_attach(
    worker: &CloudWorker,
    bus_root: &std::path::Path,
    spec: &mackes_mesh_types::cloud::WorkloadSpec,
    image_version: &str,
    now_ms: u64,
) -> Result<(), String> {
    if spec.node != worker.host {
        return Err(format!(
            "App VM placement `{}` does not match local Cloud placement `{}`",
            spec.node, worker.host
        ));
    }
    let workload_id = WorkloadId::new(spec.name.clone())
        .map_err(|error| format!("invalid App VM Workload identity: {error}"))?;
    let request_id = workload_request_id(spec)?;
    let mut request = WorkloadOperationRequest {
        schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
        request_id,
        workload_id,
        backend: WorkloadBackend::LibvirtVirtqemud,
        resources: WorkloadResources {
            vcpu: spec.vcpu,
            memory_mb: spec.memory_mb,
            disk_gb: spec.disk_gb,
        },
        image_ref: Some(format!(
            "{}:{image_version}",
            app_image::APP_VM_IMAGE_NAME
        )),
        target_node: worker.workload_node_id.clone(),
        expected_generation: 0,
        action: WorkloadOperationAction::StartAndAttach,
        target_request_id: None,
        deadline_at_ms: now_ms.saturating_add(APP_VM_START_DEADLINE_MS),
        preferred_attachment: Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
        armed_token: None,
    };
    request
        .validate(now_ms)
        .map_err(|error| format!("invalid App VM Workload handoff: {error}"))?;
    let unsigned = serde_json::to_string(&request)
        .map_err(|error| format!("encode App VM Workload handoff: {error}"))?;
    let request_sha256 = cloud_request_digest(&unsigned)
        .map_err(|error| format!("digest App VM Workload handoff: {error}"))?;
    let token_now = i64::try_from(now_ms)
        .map_err(|_| "App VM Workload handoff clock exceeds token range".to_owned())?;
    let target = format!("workload:{}", request.workload_id.as_str());
    let mut token = CloudArmedToken {
        nonce: format!("app-vm-{:016x}", rand::random::<u64>()),
        expires_at_ms: token_now.saturating_add(WORKLOAD_AUTH_TTL_MS),
        verb: WORKLOAD_AUTH_VERB.to_owned(),
        node: request.target_node.clone(),
        target,
        request_sha256,
        signature: String::new(),
    };
    token.signature = worker.signer.sign_payload(&token.signing_payload());
    request.armed_token = Some(token.encode());
    let body = serde_json::to_string(&request)
        .map_err(|error| format!("encode armed App VM Workload handoff: {error}"))?;
    Persist::open(bus_root.to_path_buf())
        .map_err(|error| format!("open Workload Bus {}: {error}", bus_root.display()))?
        .write(
            WORKLOAD_OPERATION_TOPIC,
            Priority::Default,
            Some("App VM StartAndAttach"),
            Some(&body),
        )
        .map_err(|error| format!("publish Workload StartAndAttach: {error}"))?;
    Ok(())
}

fn workload_request_id(
    spec: &mackes_mesh_types::cloud::WorkloadSpec,
) -> Result<String, String> {
    let app = spec
        .app
        .as_ref()
        .ok_or_else(|| format!("workload `{}` has no typed App VM declaration", spec.name))?;
    let mut digest = Sha256::new();
    for identity in [&spec.node, &spec.name, &app.app_id, &app.session_id] {
        digest.update((identity.len() as u64).to_be_bytes());
        digest.update(identity.as_bytes());
    }
    Ok(format!("appvm-start-{:x}", digest.finalize()))
}

fn publish_open_app(
    worker: &CloudWorker,
    bus_root: &std::path::Path,
    spec: &mackes_mesh_types::cloud::WorkloadSpec,
    client_peer: &str,
    now_ms: u64,
) -> Result<(), String> {
    let request = open_app_request(worker, spec, client_peer)?;
    let app = spec
        .app
        .as_ref()
        .ok_or_else(|| format!("workload `{}` has no typed App VM declaration", spec.name))?;
    let mut document = serde_json::to_value(&request)
        .map_err(|error| format!("encode App VM OpenApp handoff: {error}"))?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| "App VM OpenApp handoff is not a JSON object".to_owned())?;
    object.insert(
        "schema_version".to_owned(),
        serde_json::Value::from(CLOUD_ACTION_SCHEMA_VERSION),
    );
    let unsigned = serde_json::to_string(&document)
        .map_err(|error| format!("encode App VM OpenApp authorization body: {error}"))?;
    let request_sha256 = cloud_request_digest(&unsigned)
        .map_err(|error| format!("digest App VM OpenApp handoff: {error}"))?;
    let token_now = i64::try_from(now_ms)
        .map_err(|_| "App VM OpenApp handoff clock exceeds token range".to_owned())?;
    let mut token = CloudArmedToken {
        nonce: format!("app-open-{:016x}", rand::random::<u64>()),
        expires_at_ms: token_now.saturating_add(WORKLOAD_AUTH_TTL_MS),
        verb: VDI_AUTH_VERB.to_owned(),
        node: VDI_AUTH_NODE.to_owned(),
        target: format!("session:{}", app.session_id),
        request_sha256,
        signature: String::new(),
    };
    token.signature = worker.signer.sign_payload(&token.signing_payload());
    document
        .as_object_mut()
        .ok_or_else(|| "App VM OpenApp handoff stopped being a JSON object".to_owned())?
        .insert(
            "armed_token".to_owned(),
            serde_json::Value::String(token.encode()),
        );
    let body = serde_json::to_string(&document)
        .map_err(|error| format!("encode armed App VM OpenApp handoff: {error}"))?;
    Persist::open(bus_root.to_path_buf())
        .map_err(|error| format!("open VDI Bus {}: {error}", bus_root.display()))?
        .write(
            crate::workers::session_broker::ACTION_TOPIC,
            Priority::Default,
            Some("App VM OpenApp"),
            Some(&body),
        )
        .map_err(|error| format!("publish VDI OpenApp: {error}"))?;
    Ok(())
}

fn open_app_request(
    worker: &CloudWorker,
    spec: &mackes_mesh_types::cloud::WorkloadSpec,
    client_peer: &str,
) -> Result<SessionRequest, String> {
    if spec.node != worker.host {
        return Err(format!(
            "App VM placement `{}` does not match local Cloud placement `{}`",
            spec.node, worker.host
        ));
    }
    WorkloadId::new(client_peer)
        .map_err(|error| format!("invalid App VM client peer identity: {error}"))?;
    let app = spec
        .app
        .as_ref()
        .ok_or_else(|| format!("workload `{}` has no typed App VM declaration", spec.name))?;
    Ok(SessionRequest::OpenApp {
        id: app.session_id.clone(),
        serving_peer: worker.workload_node_id.clone(),
        vm_id: spec.name.clone(),
        client_peer: client_peer.to_owned(),
        app_id: app.app_id.clone(),
        catalog_revision: app.catalog_revision.clone(),
        guest_profile: app.guest_profile.clone(),
        requested_capabilities: app.requested_capabilities.clone(),
        resume: app.resume,
    })
}

/// Rebind prevention must survive a broker/daemon restart, so the durable
/// action log is consulted before either lifecycle handoff is emitted.  A
/// matching session may refresh capabilities or resume intent, but its serving
/// VM, app/catalog/profile identity, and initiating client are immutable.
fn admit_open_app_replay(
    worker: &CloudWorker,
    bus_root: &std::path::Path,
    spec: &mackes_mesh_types::cloud::WorkloadSpec,
    client_peer: &str,
) -> Result<(), String> {
    let requested = open_app_request(worker, spec, client_peer)?;
    let SessionRequest::OpenApp {
        id: requested_id,
        serving_peer: requested_serving_peer,
        vm_id: requested_vm_id,
        client_peer: requested_client_peer,
        app_id: requested_app_id,
        catalog_revision: requested_catalog_revision,
        guest_profile: requested_guest_profile,
        ..
    } = &requested
    else {
        unreachable!("open_app_request always returns OpenApp")
    };
    let persist = Persist::open(bus_root.to_path_buf())
        .map_err(|error| format!("open VDI Bus {}: {error}", bus_root.display()))?;
    let messages = persist
        .list_since(crate::workers::session_broker::ACTION_TOPIC, None)
        .map_err(|error| format!("read durable App VM session identity: {error}"))?;
    for message in messages {
        let Some(body) = message.body.as_deref() else {
            continue;
        };
        if !crate::ipc::body_within_cap(Some(body)) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            continue;
        };
        if value.get("op").and_then(serde_json::Value::as_str) != Some("open_app")
            || value.get("id").and_then(serde_json::Value::as_str) != Some(requested_id)
        {
            continue;
        }
        let prior: SessionRequest = serde_json::from_value(value).map_err(|_| {
            format!(
                "durable OpenApp identity for session `{requested_id}` is malformed; refusing replay"
            )
        })?;
        let SessionRequest::OpenApp {
            serving_peer,
            vm_id,
            client_peer,
            app_id,
            catalog_revision,
            guest_profile,
            ..
        } = prior
        else {
            unreachable!("matching open_app tag decoded as OpenApp")
        };
        if serving_peer != *requested_serving_peer
            || vm_id != *requested_vm_id
            || client_peer != *requested_client_peer
            || app_id != *requested_app_id
            || catalog_revision != *requested_catalog_revision
            || guest_profile != *requested_guest_profile
        {
            return Err(format!(
                "session `{requested_id}` is durably bound to client `{client_peer}` and VM `{vm_id}`; refusing substituted client/identity replay"
            ));
        }
    }
    Ok(())
}

fn validated_request(
    body: &CloudActionBody,
) -> Result<(&str, &str, &str, AppVmLaunchRequest), String> {
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
    let client_peer = body
        .client_peer
        .as_deref()
        .ok_or_else(|| "`app-provision` requires an explicit initiating client peer".to_owned())?;
    WorkloadId::new(client_peer)
        .map_err(|error| format!("invalid App VM initiating client peer: {error}"))?;
    let request = AppVmLaunchRequest::new(
        app_id,
        catalog_revision,
        guest_profile,
        body.requested_capabilities.clone(),
        session_id,
        body.resume,
    )
    .map_err(|error| format!("invalid App VM declaration: {error}"))?;
    request
        .validate_admitted()
        .map_err(|error| format!("App VM declaration failed admission: {error}"))?;
    Ok((node, name, client_peer, request))
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
    use crate::workers::cloud::{reconcile, verify_token, HmacTokenSigner, TokenVerdict};
    use std::sync::Arc;
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
            client_peer: Some("peer:seat-a".to_owned()),
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
    fn app_provision_rejects_capability_outside_guest_policy_before_persisting() {
        let tmp = tempdir().unwrap();
        admit_app_image(tmp.path());
        let mut request = body("eagle", "writer");
        request.requested_capabilities = vec!["host_socket".to_owned()];

        let reply = build_reply(tmp.path(), "app-provision", &request);

        assert!(!reply.ok, "unsupported capability was admitted: {reply:?}");
        assert!(
            reply
                .error
                .as_deref()
                .is_some_and(|error| error.contains("failed admission")),
            "unexpected rejection: {reply:?}"
        );
        assert!(reconcile::read_desired_slice(tmp.path(), "eagle").is_empty());
    }

    #[test]
    fn front_door_resume_intent_cold_starts_without_prior_guest_evidence() {
        let state = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(state.path());
        let worker = CloudWorker::new(
            "eagle".to_owned(),
            "peer:eagle".to_owned(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(Arc::new(HmacTokenSigner::new(b"app-vm-test-key".to_vec())));

        // Front Door publishes one idempotent place-or-resume intent. On the
        // first launch there cannot yet be guest evidence; the production
        // handler must create the declaration instead of deadlocking on proof
        // that only that first boot can produce.
        let request = body("eagle", "writer");
        assert!(request.resume);
        let reply = handle(&worker, "app-provision", &request);

        assert!(reply.ok, "first App VM launch failed: {:?}", reply.error);
        let desired = reconcile::read_desired_slice(state.path(), "eagle");
        assert_eq!(desired.len(), 1);
        assert_eq!(
            desired[0].app.as_ref().map(|app| app.session_id.as_str()),
            Some("app-session-7")
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
    fn exact_daemon_replay_without_guest_evidence_is_handoff_retry() {
        let tmp = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(tmp.path());
        let mut request = body("eagle", "writer");
        request.resume = false;
        let first =
            build_reply_with_runtime_bus(tmp.path(), Some(bus.path()), "app-provision", &request);
        assert!(
            first.ok,
            "initial declaration should await first boot evidence"
        );

        let replay =
            build_reply_with_runtime_bus(tmp.path(), Some(bus.path()), "app-provision", &request);
        assert!(replay.ok, "handoff retry failed: {:?}", replay.error);
        assert_eq!(reconcile::read_desired_slice(tmp.path(), "eagle").len(), 1);
    }

    #[test]
    fn production_app_provision_publishes_identity_bound_workload_and_open_app() {
        let state = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(state.path());
        let signer = Arc::new(HmacTokenSigner::new(b"app-vm-handoff-key".to_vec()));
        let worker = CloudWorker::new(
            "eagle".to_owned(),
            "peer:eagle".to_owned(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(signer.clone());

        let mut missing_client = body("eagle", "writer");
        missing_client.client_peer = None;
        let refused = handle(&worker, "app-provision", &missing_client);
        assert!(!refused.ok, "missing client peer reached lifecycle handoff");
        assert!(refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("initiating client peer")));

        let first = handle(&worker, "app-provision", &body("eagle", "writer"));
        assert!(first.ok, "first handoff failed: {:?}", first.error);
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let first_messages = persist.list_since(WORKLOAD_OPERATION_TOPIC, None).unwrap();
        assert_eq!(first_messages.len(), 1);
        let first_body = first_messages[0].body.as_deref().unwrap();
        let first_request = WorkloadOperationRequest::from_json(first_body, now_ms()).unwrap();
        assert_eq!(first_request.workload_id.as_str(), "writer");
        assert_eq!(first_request.target_node, "peer:eagle");
        assert_eq!(first_request.backend, WorkloadBackend::LibvirtVirtqemud);
        assert_eq!(
            first_request.action,
            WorkloadOperationAction::StartAndAttach
        );
        assert_eq!(
            first_request.preferred_attachment,
            Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf)
        );
        assert_eq!(
            first_request.resources,
            WorkloadResources {
                vcpu: 2,
                memory_mb: 4096,
                disk_gb: 32,
            }
        );
        assert_eq!(
            first_request.image_ref.as_deref(),
            Some("app-vm-wayland-standard:2026.07.31")
        );
        assert_eq!(first_request.expected_generation, 0);
        assert_eq!(
            verify_token(
                first_request.armed_token.as_deref(),
                WORKLOAD_AUTH_VERB,
                "peer:eagle",
                "workload:writer",
                first_body,
                now_ms() as i64,
                signer.as_ref(),
            ),
            TokenVerdict::Valid
        );
        let first_open_messages = persist
            .list_since(crate::workers::session_broker::ACTION_TOPIC, None)
            .unwrap();
        assert_eq!(first_open_messages.len(), 1);
        let first_open_body = first_open_messages[0].body.as_deref().unwrap();
        let first_open: SessionRequest = serde_json::from_str(first_open_body).unwrap();
        assert_eq!(
            first_open,
            SessionRequest::OpenApp {
                id: "app-session-7".to_owned(),
                serving_peer: "peer:eagle".to_owned(),
                vm_id: "writer".to_owned(),
                client_peer: "peer:seat-a".to_owned(),
                app_id: "org.example.Writer".to_owned(),
                catalog_revision: "catalog-7".to_owned(),
                guest_profile: "wayland-standard".to_owned(),
                requested_capabilities: vec!["clipboard".to_owned()],
                resume: true,
            }
        );
        let first_open_value: serde_json::Value = serde_json::from_str(first_open_body).unwrap();
        let first_open_token = first_open_value["armed_token"].as_str().unwrap();
        assert_eq!(
            verify_token(
                Some(first_open_token),
                VDI_AUTH_VERB,
                VDI_AUTH_NODE,
                "session:app-session-7",
                first_open_body,
                now_ms() as i64,
                signer.as_ref(),
            ),
            TokenVerdict::Valid
        );

        let replay = handle(&worker, "app-provision", &body("eagle", "writer"));
        assert!(replay.ok, "handoff retry failed: {:?}", replay.error);
        let messages = persist.list_since(WORKLOAD_OPERATION_TOPIC, None).unwrap();
        assert_eq!(messages.len(), 2);
        let replay_request = WorkloadOperationRequest::from_json(
            messages[1].body.as_deref().unwrap(),
            now_ms(),
        )
        .unwrap();
        assert_eq!(replay_request.request_id, first_request.request_id);
        assert_ne!(replay_request.armed_token, first_request.armed_token);
        let open_messages = persist
            .list_since(crate::workers::session_broker::ACTION_TOPIC, None)
            .unwrap();
        assert_eq!(open_messages.len(), 2);
        let replay_open_body = open_messages[1].body.as_deref().unwrap();
        let replay_open: SessionRequest = serde_json::from_str(replay_open_body).unwrap();
        assert_eq!(replay_open, first_open);
        let replay_open_value: serde_json::Value =
            serde_json::from_str(replay_open_body).unwrap();
        assert_ne!(
            replay_open_value["armed_token"].as_str(),
            Some(first_open_token)
        );
        let mut roster = std::collections::BTreeMap::new();
        crate::workers::session_broker::apply_request(&mut roster, first_open, now_ms()).unwrap();
        crate::workers::session_broker::apply_request(&mut roster, replay_open, now_ms()).unwrap();
        assert_eq!(roster.len(), 1);
        assert_eq!(roster["app-session-7"].client_peer, "peer:seat-a");
    }

    #[test]
    fn daemon_restart_cannot_rebind_app_session_to_substituted_client_peer() {
        let state = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(state.path());
        let signer = Arc::new(HmacTokenSigner::new(b"app-vm-restart-key".to_vec()));
        let worker = CloudWorker::new(
            "eagle".to_owned(),
            "peer:eagle".to_owned(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(signer.clone());
        let first = handle(&worker, "app-provision", &body("eagle", "writer"));
        assert!(first.ok, "initial handoff failed: {:?}", first.error);

        // Reconstructing the worker models loss of all daemon-local session
        // state while retaining the authoritative desired state and Bus log.
        let restarted = CloudWorker::new(
            "eagle".to_owned(),
            "peer:eagle".to_owned(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(signer);
        let mut substituted = body("eagle", "writer");
        substituted.client_peer = Some("peer:seat-b".to_owned());
        let replay = handle(&restarted, "app-provision", &substituted);
        assert!(!replay.ok, "restart rebound the durable App VM session");
        assert!(replay.error.as_deref().is_some_and(|error| {
            error.contains("durably bound to client `peer:seat-a`")
                && error.contains("substituted client/identity replay")
        }));

        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        assert_eq!(
            persist.list_since(WORKLOAD_OPERATION_TOPIC, None).unwrap().len(),
            1,
            "rejected client substitution emitted another workload operation"
        );
        assert_eq!(
            persist
                .list_since(crate::workers::session_broker::ACTION_TOPIC, None)
                .unwrap()
                .len(),
            1,
            "rejected client substitution emitted another OpenApp"
        );
    }

    #[test]
    fn unavailable_guest_cannot_refresh_existing_app_vm_desired_state() {
        let tmp = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(tmp.path());
        let mut initial = body("eagle", "writer");
        initial.resume = false;
        let first =
            build_reply_with_runtime_bus(tmp.path(), Some(bus.path()), "app-provision", &initial);
        assert!(first.ok, "initial declaration failed: {:?}", first.error);
        let desired_path = reconcile::desired_doc_path(tmp.path(), "eagle", "writer").unwrap();
        let before = std::fs::read(&desired_path).unwrap();

        let evidence = mackes_mesh_types::vdi_session::AppVmRuntimeEvidence {
            session_id: "app-session-7".to_owned(),
            vm_id: "writer".to_owned(),
            app_id: "org.example.Writer".to_owned(),
            generation: 7,
            state: mackes_mesh_types::vdi_session::AppVmRuntimeState::Unavailable,
            reason: Some("guest transport unavailable".to_owned()),
        };
        mde_bus::persist::Persist::open(bus.path().to_path_buf())
            .unwrap()
            .write(
                mackes_mesh_types::vdi_session::APP_VM_RUNTIME_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&serde_json::to_string(&evidence).unwrap()),
            )
            .unwrap();

        let mut hostile_resume = initial;
        hostile_resume.resume = true;
        hostile_resume.catalog_revision = Some("catalog-8".to_owned());
        let replay = build_reply_with_runtime_bus(
            tmp.path(),
            Some(bus.path()),
            "app-provision",
            &hostile_resume,
        );

        assert!(!replay.ok, "unavailable guest was admitted for resume");
        assert!(replay
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Unavailable")));
        assert_eq!(
            std::fs::read(desired_path).unwrap(),
            before,
            "rejected unavailable evidence mutated Workloads desired state"
        );
    }

    #[test]
    fn runtime_evidence_from_another_vm_cannot_refresh_desired_state() {
        let tmp = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(tmp.path());
        let mut initial = body("eagle", "writer");
        initial.resume = false;
        let first =
            build_reply_with_runtime_bus(tmp.path(), Some(bus.path()), "app-provision", &initial);
        assert!(first.ok, "initial declaration failed: {:?}", first.error);
        let desired_path = reconcile::desired_doc_path(tmp.path(), "eagle", "writer").unwrap();
        let before = std::fs::read(&desired_path).unwrap();

        let evidence = mackes_mesh_types::vdi_session::AppVmRuntimeEvidence {
            session_id: "app-session-7".to_owned(),
            vm_id: "different-app-vm".to_owned(),
            app_id: "org.example.Writer".to_owned(),
            generation: 8,
            state: mackes_mesh_types::vdi_session::AppVmRuntimeState::Connected,
            reason: None,
        };
        mde_bus::persist::Persist::open(bus.path().to_path_buf())
            .unwrap()
            .write(
                mackes_mesh_types::vdi_session::APP_VM_RUNTIME_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&serde_json::to_string(&evidence).unwrap()),
            )
            .unwrap();

        let mut hostile_resume = initial;
        hostile_resume.resume = true;
        hostile_resume.catalog_revision = Some("catalog-8".to_owned());
        let replay = build_reply_with_runtime_bus(
            tmp.path(),
            Some(bus.path()),
            "app-provision",
            &hostile_resume,
        );

        assert!(!replay.ok, "another VM's evidence authorized resume");
        assert!(replay
            .error
            .as_deref()
            .is_some_and(|error| error.contains("different-app-vm") && error.contains("writer")));
        assert_eq!(
            std::fs::read(desired_path).unwrap(),
            before,
            "rejected cross-VM evidence mutated Workloads desired state"
        );
    }

    #[test]
    fn active_app_session_cannot_publish_a_substituted_catalog_revision() {
        let state = tempdir().unwrap();
        let bus = tempdir().unwrap();
        admit_app_image(state.path());
        let worker = CloudWorker::new(
            "eagle".to_owned(),
            "peer:eagle".to_owned(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(Arc::new(HmacTokenSigner::new(
            b"app-vm-catalog-authority-key".to_vec(),
        )));
        let initial = body("eagle", "writer");
        let first = handle(&worker, "app-provision", &initial);
        assert!(first.ok, "initial App VM handoff failed: {:?}", first.error);
        let desired_path = reconcile::desired_doc_path(state.path(), "eagle", "writer").unwrap();
        let desired_before = std::fs::read(&desired_path).unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();

        let evidence = mackes_mesh_types::vdi_session::AppVmRuntimeEvidence {
            session_id: "app-session-7".to_owned(),
            vm_id: "writer".to_owned(),
            app_id: "org.example.Writer".to_owned(),
            generation: 7,
            state: mackes_mesh_types::vdi_session::AppVmRuntimeState::Connected,
            reason: None,
        };
        persist
            .write(
                mackes_mesh_types::vdi_session::APP_VM_RUNTIME_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&evidence).unwrap()),
            )
            .unwrap();

        let mut substituted = initial;
        substituted.catalog_revision = Some("catalog-8".to_owned());
        let replay = handle(&worker, "app-provision", &substituted);

        assert!(!replay.ok, "substituted catalog revision was published");
        assert!(replay.error.as_deref().is_some_and(|error| {
            error.contains("catalog revision `catalog-7`") && error.contains("`catalog-8`")
        }));
        assert_eq!(std::fs::read(desired_path).unwrap(), desired_before);
        assert_eq!(
            persist.list_since(WORKLOAD_OPERATION_TOPIC, None).unwrap().len(),
            1,
            "conflicting retry published another Workload action"
        );
        assert_eq!(
            persist
                .list_since(crate::workers::session_broker::ACTION_TOPIC, None)
                .unwrap()
                .len(),
            1,
            "conflicting retry published an OpenApp reply"
        );
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
            storage_pool: mackes_mesh_types::cloud::StoragePool::default(),
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
            None,
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

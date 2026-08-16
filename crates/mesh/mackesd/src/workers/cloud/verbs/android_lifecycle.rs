//! Android outer-VM lifecycle delegation.
//!
//! The governed Android resource action reaches this handler only after the
//! Cloud placement and capability gates. `Start` is delegated to the sole typed
//! Workload operation lane; the Cloud worker never contacts libvirt directly.
//! Cancellation names the exact prior Workload operation and remains bound to
//! its workload generation. Retry, guest app launch, and VDI presentation remain
//! explicit refusal boundaries until their exact typed contracts exist.

use mackes_mesh_types::android_apps::{AndroidRuntimeCatalog, AospStarterApp};
use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmedToken, CloudReply, DeliveryType};
use mackes_mesh_types::workloads::{
    WorkloadBackend, WorkloadId, WorkloadOperationAction, WorkloadOperationRequest,
    WorkloadResources, WORKLOAD_CONTRACT_SCHEMA_VERSION, WORKLOAD_OPERATION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::Deserialize;

use super::super::{reconcile, CloudWorker};

const SCHEMA_VERSION: u16 = 1;
const WORKLOAD_AUTH_VERB: &str = "workload-operation";
const WORKLOAD_DEADLINE_EXTENSION_MS: u64 = 10 * 60 * 1_000;

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
    target_request_id: Option<String>,
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

pub(super) fn handle(worker: &CloudWorker, verb: &str, raw: &str) -> CloudReply {
    let request = match parse(raw) {
        Ok(request) => request,
        Err(error) => return failure(verb, error),
    };
    let result = match request.operation {
        Operation::Start | Operation::Stop | Operation::Cancel => {
            publish_workload_operation(worker, &request)
        }
        Operation::Retry => return failure(
            verb,
            "this Android lifecycle operation is not yet delegated to Workloads; nothing changed",
        ),
    };
    match result {
        Ok(spec) => CloudReply {
            ok: true,
            verb: verb.to_owned(),
            desired: Some(vec![spec]),
            ..Default::default()
        },
        Err(error) => failure(verb, error),
    }
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
    if request.operation == Operation::Stop && request.expected_generation == 0 {
        return Err("stop requires a non-zero expected workload generation".to_owned());
    }
    if request.operation == Operation::Cancel && request.expected_generation == 0 {
        return Err("cancel requires a non-zero expected workload generation".to_owned());
    }
    if request.operation == Operation::Start && request.expected_generation != 0 {
        return Err("start requires a zero expected workload generation".to_owned());
    }
    if matches!(request.operation, Operation::Stop | Operation::Cancel) && request.app.is_some() {
        return Err("stop/cancel must not carry an app".to_owned());
    }
    match request.operation {
        Operation::Cancel => {
            let target = request
                .target_request_id
                .as_deref()
                .ok_or_else(|| "cancel requires the exact prior Workload request ID".to_owned())?;
            super::super::path_key::segment("target_request_id", target)?;
            if target == request.request_id {
                return Err("cancel cannot target its own request ID".to_owned());
            }
        }
        _ if request.target_request_id.is_some() => {
            return Err("only cancel accepts a target_request_id".to_owned())
        }
        _ => {}
    }
    if request.typed_name.is_some() {
        return Err("Android lifecycle does not accept a legacy `typed_name`".to_owned());
    }
    Ok(request)
}

fn publish_workload_operation(
    worker: &CloudWorker,
    source: &Request,
) -> Result<mackes_mesh_types::cloud::WorkloadSpec, String> {
    let now = now_ms();
    let catalog = if source.operation == Operation::Start {
        Some(
            crate::workers::android_catalog::load_admitted_catalog(&worker.host, now).map_err(
                |error| {
                    format!(
                        "Android workload `{}` is quarantined: current signed release provenance is unavailable: {error}",
                        source.workload_id
                    )
                },
            )?,
        )
    } else {
        None
    };
    publish_workload_operation_with_catalog(worker, source, catalog.as_ref(), now)
}

#[cfg(test)]
fn publish_workload_operation_against_catalog(
    worker: &CloudWorker,
    source: &Request,
    catalog: &AndroidRuntimeCatalog,
    now: u64,
) -> Result<mackes_mesh_types::cloud::WorkloadSpec, String> {
    publish_workload_operation_with_catalog(worker, source, Some(catalog), now)
}

fn publish_workload_operation_with_catalog(
    worker: &CloudWorker,
    source: &Request,
    catalog: Option<&AndroidRuntimeCatalog>,
    now: u64,
) -> Result<mackes_mesh_types::cloud::WorkloadSpec, String> {
    if source.node != worker.host {
        return Err(format!(
            "Android placement `{}` does not match local Cloud placement `{}`",
            source.node, worker.host
        ));
    }
    if !worker.arm_capable {
        return Err(
            "Android Workload delegation is unavailable because mutation signing is not configured"
                .to_owned(),
        );
    }
    let bus_root = worker
        .bus_root()
        .ok_or_else(|| "Android Workload Bus is unavailable; lifecycle failed closed".to_owned())?;
    let spec =
        reconcile::read_desired_doc_strict(&worker.state_root, &source.node, &source.workload_id)
            .map_err(|error| format!("read Android Workload declaration: {error}"))?
            .ok_or_else(|| {
                format!(
                    "Android workload `{}` has no admitted desired-state declaration",
                    source.workload_id
                )
            })?;
    if spec.delivery_type != DeliveryType::AndroidVm {
        return Err(format!(
            "workload `{}` is not an admitted Android VM declaration",
            source.workload_id
        ));
    }
    if source.operation == Operation::Start {
        let catalog = catalog.ok_or_else(|| {
            "Android Start requires current signed release provenance; nothing changed".to_owned()
        })?;
        let image = &catalog.payload.image_manifest;
        let package_manifest = super::super::load_android_package_manifest(
            &worker.state_root,
            &source.workload_id,
        )
        .ok_or_else(|| {
            format!(
                "Android workload `{}` is quarantined: its admitted package provenance is missing or invalid",
                source.workload_id
            )
        })?;
        if spec.image.as_deref() != Some(image.image_id.as_str())
            || spec.image_digest.as_deref() != Some(image.image_digest.as_str())
            || package_manifest != catalog.payload.package_manifest
        {
            return Err(format!(
                "Android workload `{}` is quarantined: desired image/package provenance does not match the current signed release catalog",
                source.workload_id
            ));
        }
    }

    // The already-verified Cloud capability carries the stable operation
    // deadline chosen by the governed resource-action producer. Extending that
    // deadline gives the typed reconciler a bounded convergence window while
    // keeping every semantic Workload field identical across token rotation.
    let source_token = source
        .armed_token
        .as_deref()
        .and_then(CloudArmedToken::parse)
        .ok_or_else(|| "Android lifecycle capability is unavailable after admission".to_owned())?;
    let source_expiry = u64::try_from(source_token.expires_at_ms)
        .map_err(|_| "Android lifecycle capability has an invalid expiry".to_owned())?;
    let deadline_at_ms = source_expiry.saturating_add(WORKLOAD_DEADLINE_EXTENSION_MS);
    let (action, action_label, nonce_label) = match source.operation {
        Operation::Start => (WorkloadOperationAction::Start, "Start", "start"),
        Operation::Stop => (WorkloadOperationAction::Stop, "Stop", "stop"),
        Operation::Cancel => (WorkloadOperationAction::Cancel, "Cancel", "cancel"),
        Operation::Retry => {
            return Err("unsupported Android lifecycle delegation; nothing changed".to_owned())
        }
    };

    let workload_id = WorkloadId::new(source.workload_id.clone())
        .map_err(|error| format!("invalid Android Workload identity: {error}"))?;
    let mut operation = WorkloadOperationRequest {
        schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
        request_id: source.request_id.clone(),
        workload_id,
        backend: WorkloadBackend::LibvirtVirtqemud,
        resources: WorkloadResources {
            vcpu: spec.vcpu,
            memory_mb: spec.memory_mb,
            disk_gb: spec.disk_gb,
        },
        // Android's signed release package is admitted by the Cuttlefish
        // provider. The generic Workload image catalog does not own it.
        image_ref: None,
        target_node: worker.workload_node_id.clone(),
        expected_generation: source.expected_generation,
        action,
        target_request_id: source.target_request_id.clone(),
        deadline_at_ms,
        preferred_attachment: None,
        armed_token: None,
    };
    operation
        .validate(now)
        .map_err(|error| format!("invalid Android Workload {action_label} handoff: {error}"))?;
    let unsigned = serde_json::to_string(&operation)
        .map_err(|error| format!("encode Android Workload {action_label} handoff: {error}"))?;
    let request_sha256 = cloud_request_digest(&unsigned)
        .map_err(|error| format!("digest Android Workload {action_label} handoff: {error}"))?;
    let target = format!("workload:{}", operation.workload_id.as_str());
    let mut token = CloudArmedToken {
        nonce: format!("android-{nonce_label}-{:016x}", rand::random::<u64>()),
        expires_at_ms: source_token.expires_at_ms,
        verb: WORKLOAD_AUTH_VERB.to_owned(),
        node: operation.target_node.clone(),
        target,
        request_sha256,
        signature: String::new(),
    };
    token.signature = worker.signer.sign_payload(&token.signing_payload());
    operation.armed_token = Some(token.encode());
    let body = serde_json::to_string(&operation).map_err(|error| {
        format!("encode armed Android Workload {action_label} handoff: {error}")
    })?;
    Persist::open(bus_root.clone())
        .map_err(|error| format!("open Android Workload Bus {}: {error}", bus_root.display()))?
        .write(
            WORKLOAD_OPERATION_TOPIC,
            Priority::Default,
            Some(&format!("Android outer-VM {action_label}")),
            Some(&body),
        )
        .map_err(|error| format!("publish Android Workload {action_label}: {error}"))?;
    Ok(spec)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
    use crate::workers::cloud::{verify_token, HmacTokenSigner, TokenVerdict};
    use mackes_mesh_types::android_apps::{
        AndroidAppCapability, AndroidAppPermission, AndroidCatalogAppPolicy,
        AndroidCatalogGuestReadiness, AndroidCatalogPayload, AndroidImageManifest,
        AndroidImagePackage, AndroidImagePackageManifest, AndroidImageProvenance,
        AndroidPackageVersion, AndroidResourceClass, AndroidResourceProfile, AospStarterCatalog,
        ANDROID_RUNTIME_CATALOG_SCHEMA_VERSION,
    };
    use std::fs;
    use std::sync::Arc;

    fn current_catalog(now: u64) -> AndroidRuntimeCatalog {
        let image_manifest = AndroidImageManifest::new(
            "android_vm-golden",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "aosp-source-2026-08",
            "starter-catalog-v1",
            now.saturating_sub(2_000),
            now.saturating_sub(1_000),
            AospStarterCatalog::v1(),
        )
        .expect("valid image manifest");
        let image_provenance =
            AndroidImageProvenance::from_manifest(&image_manifest).expect("image provenance");
        let package_manifest = AndroidImagePackageManifest::new(
            image_provenance,
            AospStarterApp::ALL
                .into_iter()
                .map(|app| {
                    AndroidImagePackage::for_app(
                        app,
                        AndroidPackageVersion::new("2026.08.11", 1).expect("package version"),
                    )
                })
                .collect(),
        )
        .expect("package manifest");
        let app_policies = AospStarterApp::ALL
            .into_iter()
            .map(|app| AndroidCatalogAppPolicy {
                app,
                permissions: vec![AndroidAppPermission::Network],
                capabilities: vec![AndroidAppCapability::VdiDisplay],
                resources: AndroidResourceProfile {
                    class: AndroidResourceClass::Standard,
                    vcpus: 4,
                    memory_mib: 8_192,
                    disk_mib: 80 * 1_024,
                },
                guest_readiness: AndroidCatalogGuestReadiness::BootedInventoryAndLauncherReady,
            })
            .collect();
        AndroidRuntimeCatalog {
            payload: AndroidCatalogPayload {
                schema_version: ANDROID_RUNTIME_CATALOG_SCHEMA_VERSION,
                catalog_id: "android-production".into(),
                revision: 8,
                issued_at_unix_ms: now.saturating_sub(1_000),
                expires_at_unix_ms: now.saturating_add(60_000),
                image_manifest,
                package_manifest,
                app_policies,
            },
        }
        .admit(now)
        .expect("current runtime catalog")
    }

    #[test]
    fn restart_quarantines_legacy_provenance_and_preserves_exact_current_start() {
        let state = tempfile::tempdir().expect("temporary state root");
        let bus = tempfile::tempdir().expect("temporary Bus root");
        let signer = Arc::new(HmacTokenSigner::new(b"android-workload-test-key".to_vec()));
        let now = now_ms();
        let catalog = current_catalog(now);
        let legacy = super::super::android::android_spec("node-a", "android-one");
        reconcile::write_desired_doc(state.path(), &legacy).expect("legacy Android declaration");
        let worker = CloudWorker::new(
            "node-a".into(),
            "peer:node-a".into(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(signer.clone());
        let expires_at_ms = i64::try_from(now).unwrap() + 20_000;

        let body = |nonce: &str| {
            let mut document = serde_json::json!({
                "schema_version": 1,
                "node": "node-a",
                "workload_id": "android-one",
                "request_id": "android-start-1",
                "expected_generation": 0,
                "operation": "start",
                "app": "browser",
                "armed_token": null,
                "typed_name": null
            });
            let unsigned = document.to_string();
            let digest = cloud_request_digest(&unsigned).expect("Cloud request digest");
            let token = CloudArmedToken::mint(
                signer.as_ref(),
                nonce,
                expires_at_ms,
                "android-lifecycle",
                "node-a",
                "android-one",
                &digest,
            );
            document["armed_token"] = serde_json::Value::String(token.encode());
            document.to_string()
        };

        // A daemon restart must not turn a pre-provenance desired row into a
        // recoverable-ready workload or publish any backend effect.
        let legacy_request = parse(&body("android-cloud-legacy")).expect("legacy Start request");
        let refusal =
            publish_workload_operation_against_catalog(&worker, &legacy_request, &catalog, now)
                .expect_err("legacy desired row must remain quarantined after restart");
        assert!(
            refusal.contains("quarantined"),
            "unexpected refusal: {refusal}"
        );
        let persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        assert!(persist
            .list_since(WORKLOAD_OPERATION_TOPIC, None)
            .expect("read empty Workload operations")
            .is_empty());

        let current = super::super::android::android_spec_from_manifest(
            "node-a",
            "android-one",
            catalog.payload.image_manifest.clone(),
        )
        .expect("current Android declaration");
        reconcile::write_desired_doc(state.path(), &current).expect("replace legacy declaration");
        let manifest_parent = state.path().join("mcnf/cloud/android-manifests");
        fs::create_dir_all(&manifest_parent).expect("manifest directory");
        fs::write(
            manifest_parent.join("android-one.json"),
            serde_json::to_vec(&catalog.payload.package_manifest).expect("manifest JSON"),
        )
        .expect("current package manifest");

        let first_request = parse(&body("android-cloud-1")).expect("first Start request");
        publish_workload_operation_against_catalog(&worker, &first_request, &catalog, now)
            .expect("first current Android handoff");
        let replay_request = parse(&body("android-cloud-2")).expect("replayed Start request");
        publish_workload_operation_against_catalog(&worker, &replay_request, &catalog, now)
            .expect("idempotent current Android handoff");

        let messages = persist
            .list_since(WORKLOAD_OPERATION_TOPIC, None)
            .expect("read Workload operations");
        assert_eq!(messages.len(), 2);
        let first_body = messages[0].body.as_deref().expect("first Workload body");
        let replay_body = messages[1].body.as_deref().expect("replay Workload body");
        let mut first = WorkloadOperationRequest::from_json(first_body, now_ms())
            .expect("first typed Workload request");
        let mut replay = WorkloadOperationRequest::from_json(replay_body, now_ms())
            .expect("replayed typed Workload request");
        assert_eq!(first.action, WorkloadOperationAction::Start);
        assert_eq!(first.workload_id.as_str(), "android-one");
        assert_eq!(first.target_node, "peer:node-a");
        assert_eq!(first.expected_generation, 0);
        assert_eq!(
            first.resources,
            WorkloadResources {
                vcpu: 4,
                memory_mb: 8_192,
                disk_gb: 80,
            }
        );
        assert_eq!(
            verify_token(
                first.armed_token.as_deref(),
                WORKLOAD_AUTH_VERB,
                "peer:node-a",
                "workload:android-one",
                first_body,
                now_ms() as i64,
                signer.as_ref(),
            ),
            TokenVerdict::Valid
        );
        assert_ne!(first.armed_token, replay.armed_token);
        first.armed_token = None;
        replay.armed_token = None;
        assert_eq!(first, replay, "only the delivery capability may rotate");
    }

    #[test]
    fn typed_stop_and_cancel_bind_workload_generation_and_exact_target() {
        let state = tempfile::tempdir().expect("temporary state root");
        let bus = tempfile::tempdir().expect("temporary Bus root");
        let signer = Arc::new(HmacTokenSigner::new(b"android-stop-test-key".to_vec()));
        let now = now_ms();
        let catalog = current_catalog(now);
        let current = super::super::android::android_spec_from_manifest(
            "node-a",
            "android-one",
            catalog.payload.image_manifest.clone(),
        )
        .expect("current Android declaration");
        reconcile::write_desired_doc(state.path(), &current).expect("Android declaration");
        let manifest_parent = state.path().join("mcnf/cloud/android-manifests");
        fs::create_dir_all(&manifest_parent).expect("manifest directory");
        fs::write(
            manifest_parent.join("android-one.json"),
            serde_json::to_vec(&catalog.payload.package_manifest).expect("manifest JSON"),
        )
        .expect("current package manifest");
        let worker = CloudWorker::new(
            "node-a".into(),
            "peer:node-a".into(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(signer.clone());
        let expires_at_ms = i64::try_from(now).unwrap() + 20_000;

        let body = |workload_id: &str, operation: &str, nonce: &str| {
            let mut document = serde_json::json!({
                "schema_version": 1,
                "node": "node-a",
                "workload_id": workload_id,
                "request_id": "android-stop-1",
                "expected_generation": 41,
                "operation": operation,
                "target_request_id": if operation == "cancel" {
                    Some("android-start-1")
                } else {
                    None
                },
                "app": null,
                "armed_token": null,
                "typed_name": null
            });
            let unsigned = document.to_string();
            let digest = cloud_request_digest(&unsigned).expect("Cloud request digest");
            let token = CloudArmedToken::mint(
                signer.as_ref(),
                nonce,
                expires_at_ms,
                "android-lifecycle",
                "node-a",
                workload_id,
                &digest,
            );
            document["armed_token"] = serde_json::Value::String(token.encode());
            document.to_string()
        };

        let cancel =
            parse(&body("android-one", "cancel", "android-cancel-1")).expect("exact-target Cancel");
        publish_workload_operation_against_catalog(&worker, &cancel, &catalog, now)
            .expect("typed Cancel handoff");

        let mut missing_target: serde_json::Value =
            serde_json::from_str(&body("android-one", "cancel", "android-cancel-missing"))
                .expect("Cancel JSON");
        missing_target["target_request_id"] = serde_json::Value::Null;
        assert!(parse(&missing_target.to_string())
            .expect_err("implicit cancellation target must fail closed")
            .contains("exact prior Workload request ID"));
        let mut self_target: serde_json::Value =
            serde_json::from_str(&body("android-one", "cancel", "android-cancel-self"))
                .expect("Cancel JSON");
        self_target["target_request_id"] = self_target["request_id"].clone();
        assert!(parse(&self_target.to_string())
            .expect_err("self cancellation must fail closed")
            .contains("cannot target its own request ID"));

        let cross_workload =
            parse(&body("android-two", "stop", "android-stop-cross")).expect("cross-workload Stop");
        let refusal =
            publish_workload_operation_against_catalog(&worker, &cross_workload, &catalog, now)
                .expect_err("another workload must not be inferred or retargeted");
        assert!(refusal.contains("has no admitted desired-state declaration"));

        let first_request =
            parse(&body("android-one", "stop", "android-stop-a")).expect("first Stop request");
        publish_workload_operation_against_catalog(&worker, &first_request, &catalog, now)
            .expect("first typed Stop handoff");
        let replay_request =
            parse(&body("android-one", "stop", "android-stop-b")).expect("replayed Stop request");
        publish_workload_operation_against_catalog(&worker, &replay_request, &catalog, now)
            .expect("idempotent typed Stop handoff");

        let persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        let messages = persist
            .list_since(WORKLOAD_OPERATION_TOPIC, None)
            .expect("read Workload operations");
        assert_eq!(messages.len(), 3);
        let cancel_body = messages[0].body.as_deref().expect("Cancel body");
        let cancellation = WorkloadOperationRequest::from_json(cancel_body, now_ms())
            .expect("typed Workload Cancel request");
        assert_eq!(cancellation.action, WorkloadOperationAction::Cancel);
        assert_eq!(
            cancellation.target_request_id.as_deref(),
            Some("android-start-1")
        );
        assert_eq!(cancellation.expected_generation, 41);
        let first_body = messages[1].body.as_deref().expect("first Stop body");
        let replay_body = messages[2].body.as_deref().expect("replayed Stop body");
        let mut first = WorkloadOperationRequest::from_json(first_body, now_ms())
            .expect("first typed Stop request");
        let mut replay = WorkloadOperationRequest::from_json(replay_body, now_ms())
            .expect("replayed typed Stop request");
        assert_eq!(first.action, WorkloadOperationAction::Stop);
        assert_eq!(first.request_id, "android-stop-1");
        assert_eq!(first.workload_id.as_str(), "android-one");
        assert_eq!(first.expected_generation, 41);
        assert_eq!(first.target_node, "peer:node-a");
        assert!(first.target_request_id.is_none());
        assert_eq!(
            verify_token(
                first.armed_token.as_deref(),
                WORKLOAD_AUTH_VERB,
                "peer:node-a",
                "workload:android-one",
                first_body,
                now_ms() as i64,
                signer.as_ref(),
            ),
            TokenVerdict::Valid
        );
        assert_ne!(first.armed_token, replay.armed_token);
        first.armed_token = None;
        replay.armed_token = None;
        assert_eq!(first, replay, "only the delivery capability may rotate");
    }

    #[test]
    fn stop_remains_available_when_release_catalog_is_unavailable() {
        let state = tempfile::tempdir().expect("temporary state root");
        let bus = tempfile::tempdir().expect("temporary Bus root");
        let signer = Arc::new(HmacTokenSigner::new(
            b"android-cleanup-without-catalog-test-key".to_vec(),
        ));
        let now = now_ms();
        let catalog = current_catalog(now);
        let current = super::super::android::android_spec_from_manifest(
            "node-a",
            "android-one",
            catalog.payload.image_manifest,
        )
        .expect("current Android declaration");
        reconcile::write_desired_doc(state.path(), &current).expect("Android declaration");
        let worker = CloudWorker::new(
            "node-a".into(),
            "peer:node-a".into(),
            state.path().to_path_buf(),
        )
        .with_bus_root(Some(bus.path().to_path_buf()))
        .with_signer(signer.clone());

        let mut document = serde_json::json!({
            "schema_version": 1,
            "node": "node-a",
            "workload_id": "android-one",
            "request_id": "android-cleanup-1",
            "expected_generation": 73,
            "operation": "stop",
            "app": null,
            "armed_token": null,
            "typed_name": null
        });
        let unsigned = document.to_string();
        let digest = cloud_request_digest(&unsigned).expect("Cloud request digest");
        let token = CloudArmedToken::mint(
            signer.as_ref(),
            "android-cleanup-without-catalog",
            i64::try_from(now).unwrap() + 20_000,
            "android-lifecycle",
            "node-a",
            "android-one",
            &digest,
        );
        document["armed_token"] = serde_json::Value::String(token.encode());
        let request = parse(&document.to_string()).expect("generation-bound Stop request");

        // No catalog or package-manifest state is installed. Cleanup
        // must still reach the sole Workloads actuator for the exact admitted
        // Android desired row; requiring launch provenance here would strand a
        // running outer VM precisely when the provider/catalog has failed.
        publish_workload_operation(&worker, &request)
            .expect("cleanup remains available without release catalog state");

        let persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        let messages = persist
            .list_since(WORKLOAD_OPERATION_TOPIC, None)
            .expect("read Workload operations");
        assert_eq!(messages.len(), 1);
        let body = messages[0].body.as_deref().expect("Stop body");
        let operation = WorkloadOperationRequest::from_json(body, now_ms())
            .expect("typed Workload Stop request");
        assert_eq!(operation.action, WorkloadOperationAction::Stop);
        assert_eq!(operation.workload_id.as_str(), "android-one");
        assert_eq!(operation.expected_generation, 73);
        assert_eq!(operation.target_node, "peer:node-a");
    }

    #[test]
    fn stop_rejects_unbound_generation_before_any_backend_effect() {
        let document = serde_json::json!({
            "schema_version": 1,
            "node": "node-a",
            "workload_id": "android-one",
            "request_id": "android-stop-zero",
            "expected_generation": 0,
            "operation": "stop",
            "app": null,
            "armed_token": null,
            "typed_name": null
        });

        let refusal = parse(&document.to_string()).expect_err("zero-generation Stop must refuse");
        assert!(refusal.contains("non-zero expected workload generation"));
    }

    #[test]
    fn start_rejects_a_nonzero_expected_generation_before_any_backend_effect() {
        let raw = serde_json::json!({
            "schema_version": 1,
            "node": "node-a",
            "workload_id": "android-one",
            "request_id": "android-start-stale",
            "expected_generation": 41,
            "operation": "start",
            "app": "browser"
        })
        .to_string();

        let refusal = parse(&raw).expect_err("stale Start generation must be refused");

        assert!(refusal.contains("start requires a zero expected workload generation"));
    }
}

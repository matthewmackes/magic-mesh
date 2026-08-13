//! WL-ARCH-010 — the shell's typed Workload operation client.
//!
//! This module is intentionally small: the shell authors one bounded request,
//! arms it with the same exact-body capability used by the existing root-shell
//! mutation seam, and writes only `action/workload/operation`. It never shells
//! libvirt, systemd, QEMU, or Podman and it never writes a legacy lifecycle
//! topic. The node-local compute worker owns admission, journaling, execution,
//! and the `state/workloads/<node>` projection.

use std::path::Path;

use mackes_mesh_types::workloads::{
    reject_duplicate_json_keys, workload_state_topic, WorkloadAttachmentProtocol, WorkloadBackend,
    WorkloadOperationAction, WorkloadOperationReply, WorkloadOperationRequest,
    WorkloadOperationStatus, WorkloadProfile, WorkloadResources, WorkloadStateSnapshot,
    WORKLOAD_CONTRACT_SCHEMA_VERSION, WORKLOAD_OPERATION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

/// Build a short-lived, target-bound Workload operation request.
pub(crate) fn request(
    workload_id: &str,
    target_node: &str,
    backend: WorkloadBackend,
    resources: WorkloadResources,
    action: WorkloadOperationAction,
    preferred_attachment: Option<WorkloadAttachmentProtocol>,
    expected_generation: u64,
    now_ms: u64,
) -> Result<WorkloadOperationRequest, String> {
    request_with_image(
        workload_id,
        target_node,
        backend,
        resources,
        action,
        preferred_attachment,
        expected_generation,
        None,
        now_ms,
    )
}

/// Build a Workload operation with an exact approved catalog image reference.
pub(crate) fn request_with_image(
    workload_id: &str,
    target_node: &str,
    backend: WorkloadBackend,
    resources: WorkloadResources,
    action: WorkloadOperationAction,
    preferred_attachment: Option<WorkloadAttachmentProtocol>,
    expected_generation: u64,
    image_ref: Option<&str>,
    now_ms: u64,
) -> Result<WorkloadOperationRequest, String> {
    request_with_image_and_target(
        workload_id,
        target_node,
        backend,
        resources,
        action,
        preferred_attachment,
        expected_generation,
        image_ref,
        None,
        now_ms,
    )
}

/// Build a typed cancellation request for one exact journaled operation.
pub(crate) fn cancel_request(
    workload_id: &str,
    target_node: &str,
    backend: WorkloadBackend,
    resources: WorkloadResources,
    expected_generation: u64,
    target_request_id: &str,
    now_ms: u64,
) -> Result<WorkloadOperationRequest, String> {
    request_with_image_and_target(
        workload_id,
        target_node,
        backend,
        resources,
        WorkloadOperationAction::Cancel,
        None,
        expected_generation,
        None,
        Some(target_request_id),
        now_ms,
    )
}

fn request_with_image_and_target(
    workload_id: &str,
    target_node: &str,
    backend: WorkloadBackend,
    resources: WorkloadResources,
    action: WorkloadOperationAction,
    preferred_attachment: Option<WorkloadAttachmentProtocol>,
    expected_generation: u64,
    image_ref: Option<&str>,
    target_request_id: Option<&str>,
    now_ms: u64,
) -> Result<WorkloadOperationRequest, String> {
    let workload_id = mackes_mesh_types::workloads::WorkloadId::new(workload_id.trim())
        .map_err(|error| error.to_string())?;
    let request_id = format!("workload-op-{}", uuid::Uuid::new_v4().simple());
    let mut request = WorkloadOperationRequest {
        schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
        request_id,
        workload_id,
        backend,
        resources,
        image_ref: image_ref
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        target_node: target_node.trim().to_string(),
        expected_generation,
        action,
        target_request_id: target_request_id.map(str::to_owned),
        deadline_at_ms: now_ms.saturating_add(20_000),
        preferred_attachment,
        armed_token: None,
    };
    request
        .validate(now_ms)
        .map_err(|error| format!("workload request is invalid: {error}"))?;
    let unsigned = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    let target = format!("workload:{}", request.workload_id.as_str());
    let authorized = crate::iac::authorize_root_mutation_body(
        &unsigned,
        "workload-operation",
        request.target_node.as_str(),
        &target,
    )?;
    request = WorkloadOperationRequest::from_json(&authorized, now_ms)
        .map_err(|error| format!("authorized workload request was rejected: {error}"))?;
    Ok(request)
}

/// Persist one already-authorized operation to the sole Workload action lane.
pub(crate) fn publish(root: &Path, request: &WorkloadOperationRequest) -> Result<String, String> {
    let body = serde_json::to_string(request).map_err(|error| error.to_string())?;
    let message = Persist::open(root.to_path_buf())
        .and_then(|persist| {
            persist.write(
                WORKLOAD_OPERATION_TOPIC,
                Priority::Default,
                Some("Workload operation"),
                Some(&body),
            )
        })
        .map_err(|error| format!("could not publish Workload operation: {error}"))?;
    Ok(message.ulid)
}

/// Decode the authoritative node-local Workload projection.
pub(crate) fn read_state(persist: &Persist, node: &str) -> Option<WorkloadStateSnapshot> {
    let body = persist.read_latest(&workload_state_topic(node)).ok()??;
    reject_duplicate_json_keys(body.body.as_deref()?).ok()?;
    let snapshot: WorkloadStateSnapshot = serde_json::from_str(body.body.as_deref()?).ok()?;
    snapshot.validate(current_ms()).ok()?;
    if snapshot.node != node {
        return None;
    }
    Some(snapshot)
}

/// Find one operation in a validated node projection.
pub(crate) fn read_status(
    persist: &Persist,
    node: &str,
    workload_id: &str,
) -> Option<WorkloadOperationStatus> {
    read_state(persist, node)?
        .workloads
        .into_iter()
        .find(|status| status.workload_id.as_str() == workload_id)
}

/// Read only the generation produced by one exact operation request.
///
/// A workload can retain an older terminal status while a new operation is in
/// flight. Matching only its workload id would let that stale generation
/// falsely complete the new action in the shell.
pub(crate) fn read_status_for_request(
    persist: &Persist,
    node: &str,
    workload_id: &str,
    request_id: &str,
) -> Option<WorkloadOperationStatus> {
    read_status(persist, node, workload_id).filter(|status| status.request_id == request_id)
}

/// Decode the correlated Workload RPC reply and enforce its closed shape.
pub(crate) fn read_reply(
    persist: &Persist,
    message_ulid: &str,
    request_id: &str,
) -> Option<WorkloadOperationReply> {
    let message = persist
        .list_since_limit(&reply_topic(message_ulid), None, 1)
        .ok()?
        .into_iter()
        .next()?;
    let body = message.body.as_deref()?;
    reject_duplicate_json_keys(body).ok()?;
    let reply: WorkloadOperationReply = serde_json::from_str(body).ok()?;
    if reply.schema_version != WORKLOAD_CONTRACT_SCHEMA_VERSION
        || reply.request_id != request_id
        || reply.accepted != reply.status.is_some()
        || reply.accepted == reply.error_code.is_some()
        || reply.status.as_ref().is_some_and(|status| {
            status.request_id != request_id || status.validate(current_ms()).is_err()
        })
    {
        return None;
    }
    Some(reply)
}

fn current_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::workloads::{
        WorkloadOperationErrorCode, WorkloadOperationPhase, WorkloadPowerState, WorkloadReadiness,
        WorkloadRuntimeSignals,
    };

    fn terminal_status(request_id: &str) -> WorkloadOperationStatus {
        WorkloadOperationStatus {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: request_id.to_string(),
            workload_id: mackes_mesh_types::workloads::WorkloadId::new("browser-seat15")
                .expect("workload id"),
            backend: WorkloadBackend::LibvirtVirtqemud,
            resources: WorkloadProfile::Small.resources(),
            image_ref: None,
            generation: 7,
            phase: WorkloadOperationPhase::Completed,
            power: WorkloadPowerState::Running,
            readiness: WorkloadReadiness::Ready,
            signals: WorkloadRuntimeSignals::default(),
            retryable: false,
            attempt: 1,
            next_retry_at_ms: 0,
            reason: None,
            remediation: None,
            attachment: None,
        }
    }

    #[test]
    fn request_is_typed_and_capability_bound() {
        let request = request(
            "browser-seat15",
            "seat15",
            WorkloadBackend::LibvirtVirtqemud,
            WorkloadProfile::Small.resources(),
            WorkloadOperationAction::StartAndAttach,
            Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
            0,
            current_ms(),
        )
        .expect("request");
        assert!(request.armed_token.is_some());
        assert!(request.request_id.starts_with("workload-op-"));
        assert_eq!(request.target_node, "seat15");
    }

    #[test]
    fn image_reference_stays_catalog_scoped_and_capability_bound() {
        let request = request_with_image(
            "vm:seat15:fedora",
            "seat15",
            WorkloadBackend::LibvirtVirtqemud,
            WorkloadProfile::Small.resources(),
            WorkloadOperationAction::StartAndAttach,
            Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
            0,
            Some("fedora:1.0"),
            current_ms(),
        )
        .expect("request");
        assert_eq!(request.image_ref.as_deref(), Some("fedora:1.0"));
        assert!(request.armed_token.is_some());
    }

    #[test]
    fn cancel_request_names_one_exact_target_and_is_capability_bound() {
        let request = cancel_request(
            "browser-seat15",
            "seat15",
            WorkloadBackend::LibvirtVirtqemud,
            WorkloadProfile::Small.resources(),
            7,
            "workload-op-running",
            current_ms(),
        )
        .expect("cancel request");
        assert_eq!(request.action, WorkloadOperationAction::Cancel);
        assert_eq!(request.expected_generation, 7);
        assert_eq!(
            request.target_request_id.as_deref(),
            Some("workload-op-running")
        );
        assert!(request.armed_token.is_some());
    }

    #[test]
    fn publish_uses_only_the_workload_action_lane() {
        assert_eq!(WORKLOAD_OPERATION_TOPIC, "action/workload/operation");
    }

    #[test]
    fn status_resolution_requires_the_exact_request_generation() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let snapshot = WorkloadStateSnapshot {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            node: "seat15".into(),
            observed_at_ms: current_ms(),
            workloads: vec![terminal_status("workload-op-previous")],
        };
        persist
            .write(
                &workload_state_topic("seat15"),
                Priority::Default,
                Some("retained terminal projection"),
                Some(&serde_json::to_string(&snapshot).expect("snapshot body")),
            )
            .expect("persist snapshot");

        assert!(
            read_status_for_request(&persist, "seat15", "browser-seat15", "workload-op-current")
                .is_none(),
            "an older terminal generation must not complete a newer shell request"
        );
        assert!(read_status_for_request(
            &persist,
            "seat15",
            "browser-seat15",
            "workload-op-previous"
        )
        .is_some());
    }

    #[test]
    fn workload_reply_is_correlated_and_closed_before_ui_use() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let reply = WorkloadOperationReply {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: "workload-op-current".into(),
            accepted: false,
            status: None,
            error_code: Some(WorkloadOperationErrorCode::StaleGeneration),
        };
        persist
            .write(
                &reply_topic("bus-message-1"),
                Priority::Default,
                Some("typed Workload refusal"),
                Some(&serde_json::to_string(&reply).expect("reply body")),
            )
            .expect("persist reply");

        assert_eq!(
            read_reply(&persist, "bus-message-1", "workload-op-current")
                .and_then(|reply| reply.error_code),
            Some(WorkloadOperationErrorCode::StaleGeneration)
        );
        assert!(
            read_reply(&persist, "bus-message-1", "workload-op-other").is_none(),
            "a reply for another idempotency key must not settle this request"
        );
    }

    #[test]
    fn read_state_rejects_duplicate_top_level_keys_before_projection_decode() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let observed_at_ms = current_ms();
        // Without the duplicate-key guard, serde_json accepts this object and
        // its second `node` value would become the apparent UI authority.
        let body = format!(
            r#"{{"schema_version":{},"node":"seat15","observed_at_ms":{},"workloads":[],"node":"attacker"}}"#,
            WORKLOAD_CONTRACT_SCHEMA_VERSION, observed_at_ms
        );
        persist
            .write(
                &workload_state_topic("seat15"),
                Priority::Default,
                Some("hostile duplicate projection"),
                Some(&body),
            )
            .expect("persist hostile projection");

        assert!(
            read_state(&persist, "seat15").is_none(),
            "duplicate persisted JSON must never become shell projection state"
        );
    }

    #[test]
    fn foreign_node_projection_cannot_authorize_local_workload_presentation() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let snapshot = WorkloadStateSnapshot {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            node: "attacker".into(),
            observed_at_ms: current_ms(),
            workloads: Vec::new(),
        };
        let body = serde_json::to_string(&snapshot).expect("serialize hostile projection");
        persist
            .write(
                &workload_state_topic("seat15"),
                Priority::Default,
                Some("foreign Workload projection"),
                Some(&body),
            )
            .expect("persist hostile projection");

        assert!(
            read_state(&persist, "seat15").is_none(),
            "a valid foreign-node projection must not become local readiness or presentation authority"
        );
    }
}

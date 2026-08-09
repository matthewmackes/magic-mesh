//! WL-FUNC-019 S4 — typed authority routing for universal-resource actions.
//!
//! The resource action topic is an authorization boundary, not an execution
//! language. A request selects an exact catalog/card/action tuple and carries
//! one closed downstream authority request. The router revalidates the current
//! catalog, consumes the exact-body capability, and republishes only to a fixed
//! Workload, VDI, clipboard, or Android-provider topic. Callers cannot supply a
//! command, path, URL, executable, or topic.

use std::path::PathBuf;
use std::sync::Arc;

use mackes_mesh_types::android_apps::AospStarterApp;
use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmSigner, CloudArmedToken};
use mackes_mesh_types::resources::{
    resource_publisher_attestation_topic, ActionAvailabilityStatus, AuthStatus, HealthStatus,
    ResourceActionTarget, ResourceActionVerb, ResourceCatalog, ResourceClass,
    ResourcePublisherAttestation, RESOURCE_CATALOG_TOPIC,
};
use mackes_mesh_types::vdi_clipboard::{
    vdi_clipboard_session_topic, VdiClipboardLeaseV2, VdiClipboardMessageV2,
    VDI_CLIPBOARD_GUEST_TO_HOST_TOPIC_PREFIX, VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
    VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX,
};
use mackes_mesh_types::vdi_session::{DesktopSessionProfile, SessionRequest};
use mackes_mesh_types::workloads::{
    WorkloadOperationAction, WorkloadOperationRequest, WORKLOAD_OPERATION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use crate::ipc::action_auth::{
    production_action_signer, ActionAuthorizer, MutationContext, MAX_AUTH_TTL_MS,
};
use crate::ipc::secret_store::SecretStore;

/// Sole resource-action ingress. No caller-controlled topic suffix is accepted.
pub const RESOURCE_ACTION_TOPIC: &str = "action/resources/invoke";
/// Maximum hostile request rows consumed in one service-aggregator poll.
const MAX_ACTIONS_PER_TICK: usize = 32;
/// Resource action bodies remain comfortably below the global Bus ceiling.
const MAX_ACTION_BODY_BYTES: usize = 64 * 1024;
const SCHEMA_VERSION: u16 = 1;
const CLOUD_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClipboardDirection {
    HostToGuest,
    GuestToHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AndroidOperation {
    Start,
    Cancel,
    Retry,
}

/// Closed wire mirror of the existing Android lifecycle authority request.
/// The authority itself owns parsing and mutation; this type only prevents the
/// resource router from accepting arbitrary provider JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidLifecycleRequest {
    schema_version: u16,
    node: String,
    workload_id: String,
    request_id: String,
    expected_generation: u64,
    operation: AndroidOperation,
    app: Option<AospStarterApp>,
    armed_token: Option<String>,
    typed_name: Option<String>,
}

/// Strict subset of VDI authority operations a resource card may initiate.
/// Runtime-state and disconnect events remain session-broker owned. `Close` is
/// admitted only as an exact cancellation of a previously selected session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum StrictSessionRequest {
    Open {
        id: String,
        serving_peer: String,
        vm_id: String,
        client_peer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<DesktopSessionProfile>,
    },
    OpenApp {
        id: String,
        serving_peer: String,
        vm_id: String,
        client_peer: String,
        app_id: String,
        catalog_revision: String,
        guest_profile: String,
        requested_capabilities: Vec<String>,
        resume: bool,
    },
    Close {
        id: String,
    },
}

impl StrictSessionRequest {
    fn shared(&self) -> SessionRequest {
        match self {
            Self::Open {
                id,
                serving_peer,
                vm_id,
                client_peer,
                profile,
            } => SessionRequest::Open {
                id: id.clone(),
                serving_peer: serving_peer.clone(),
                vm_id: vm_id.clone(),
                client_peer: client_peer.clone(),
                profile: profile.clone(),
            },
            Self::OpenApp {
                id,
                serving_peer,
                vm_id,
                client_peer,
                app_id,
                catalog_revision,
                guest_profile,
                requested_capabilities,
                resume,
            } => SessionRequest::OpenApp {
                id: id.clone(),
                serving_peer: serving_peer.clone(),
                vm_id: vm_id.clone(),
                client_peer: client_peer.clone(),
                app_id: app_id.clone(),
                catalog_revision: catalog_revision.clone(),
                guest_profile: guest_profile.clone(),
                requested_capabilities: requested_capabilities.clone(),
                resume: *resume,
            },
            Self::Close { id } => SessionRequest::Close { id: id.clone() },
        }
    }
}

/// Existing typed authority requests admitted by the universal router.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "authority",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum TypedAuthorityRequest {
    Workload(WorkloadOperationRequest),
    Vdi(StrictSessionRequest),
    Clipboard {
        direction: ClipboardDirection,
        lease: VdiClipboardLeaseV2,
        message: VdiClipboardMessageV2,
    },
    /// Resource-authority cancellation request for Clipboard V2. Clipboard V2
    /// has no downstream cancellation wire operation, so this shape can only
    /// produce a signed fail-closed completion and is never dispatched.
    ClipboardCancellation {
        direction: ClipboardDirection,
        session_id: String,
        generation: u64,
        lease_id: String,
        message_sequence: u64,
        target_request_id: String,
    },
    AndroidProvider(AndroidLifecycleRequest),
}

/// Exact signed resource selection plus one closed authority request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceActionInvocation {
    schema_version: u16,
    request_id: String,
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    action_id: String,
    verb: ResourceActionVerb,
    target: ResourceActionTarget,
    expected_generation: u64,
    cancellation_id: String,
    /// Exact previously admitted operation selected by a cancellation.  This
    /// is absent for ordinary actions; cancellation never means "whatever is
    /// currently running" and cannot be supplied through an untyped target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancels_request_id: Option<String>,
    issued_at_ms: u64,
    deadline_at_ms: u64,
    authority_request: TypedAuthorityRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vdi_open_receipt: Option<VdiAuthorityCompletionReply>,
    /// Explicit, short-lived operator approval for an immutable catalog action.
    /// The outer root-mutation signature covers this exact binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_approval: Option<LocalApprovalBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    armed_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalApprovalBinding {
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    action_id: String,
    target: ResourceActionTarget,
    approved_at_ms: u64,
    expires_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RefusalCode {
    Malformed,
    Unauthorized,
    StaleCatalog,
    StaleCard,
    Unavailable,
    CapabilityMismatch,
    TargetMismatch,
    AuthorityUnavailable,
    CancellationUnsupported,
}

/// Closed decoder hint for the authority-owned response lane.  Callers never
/// supply this value or its topic; both are selected by the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DownstreamReplyKind {
    WorkloadOperation,
    VdiAuthorityCompletion,
    ClipboardReceipt,
    CloudOperation,
}

/// Authenticated selection echoed only after the exact invocation is admitted.
/// A consumer can therefore reject a reply accidentally associated with a
/// different card action, generation, or cancellation target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceActionReplyBinding {
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    action_id: String,
    verb: ResourceActionVerb,
    target: ResourceActionTarget,
    expected_generation: u64,
    cancellation_id: String,
    cancels_request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VdiCompletionOutcome {
    /// The exact signed request is durable on the session authority's action
    /// lane. This does not fabricate a connected or closed runtime state.
    DispatchAccepted,
}

/// Resource-authority-owned completion for VDI dispatch. The HMAC covers the
/// exact admitted selection and downstream body identity, allowing consumers
/// to reject cross-action or cross-request reply substitution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VdiAuthorityCompletionReply {
    schema_version: u16,
    request_id: String,
    session_id: String,
    serving_peer: String,
    outcome: VdiCompletionOutcome,
    completed_at_ms: u64,
    downstream_message_id: String,
    downstream_request_digest: String,
    authority_verb: String,
    authority_node: String,
    authority_target: String,
    binding: ResourceActionReplyBinding,
    authority_signature: String,
}

impl VdiAuthorityCompletionReply {
    fn signing_payload(&self) -> Result<String, RefusalCode> {
        let mut unsigned = self.clone();
        unsigned.authority_signature.clear();
        serde_json::to_string(&unsigned).map_err(|_| RefusalCode::Malformed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CancellationAuthority {
    ClipboardTransfer,
    AndroidLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CancellationCompletionOutcome {
    DispatchAccepted,
    UnsupportedCancellation,
}

/// Signed resource-authority completion for one exact cancellation request.
/// The invocation digest covers the closed authority request as well as the
/// selected card/action binding, preventing reply substitution between lanes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancellationAuthorityCompletionReply {
    schema_version: u16,
    request_id: String,
    authority: CancellationAuthority,
    outcome: CancellationCompletionOutcome,
    completed_at_ms: u64,
    invocation_digest: String,
    downstream_message_id: Option<String>,
    downstream_request_digest: Option<String>,
    authority_verb: String,
    authority_node: String,
    authority_target: String,
    binding: ResourceActionReplyBinding,
    authority_signature: String,
}

impl CancellationAuthorityCompletionReply {
    fn signing_payload(&self) -> Result<String, RefusalCode> {
        let mut unsigned = self.clone();
        unsigned.authority_signature.clear();
        serde_json::to_string(&unsigned).map_err(|_| RefusalCode::Malformed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct ResourceActionReply {
    schema_version: u16,
    request_id: String,
    accepted: bool,
    downstream_topic: Option<String>,
    downstream_reply_topic: Option<String>,
    downstream_reply_kind: Option<DownstreamReplyKind>,
    binding: Option<ResourceActionReplyBinding>,
    cancellation_completion: Option<CancellationAuthorityCompletionReply>,
    refusal: Option<RefusalCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedAction {
    topic: String,
    body: String,
    reply_topic: Option<String>,
    reply_kind: Option<DownstreamReplyKind>,
    verb: &'static str,
    node: String,
    target: String,
    vdi_completion: Option<PlannedVdiCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedVdiCompletion {
    session_id: String,
    serving_peer: String,
}

/// Resource action worker embedded in the existing service aggregator.
pub struct ResourceActionWorker {
    bus_root: Option<PathBuf>,
    cursor: Option<String>,
    authorizer: Arc<ActionAuthorizer>,
    signer: Option<CloudArmSigner>,
    publisher_store: SecretStore,
}

impl ResourceActionWorker {
    /// Construct the fail-closed production router.
    #[must_use]
    pub fn production(bus_root: Option<PathBuf>, publisher_store: SecretStore) -> Self {
        let signer = production_action_signer()
            .map_err(|error| {
                tracing::error!(
                    target: "mackesd::resource_actions",
                    %error,
                    "resource downstream signing unavailable; actions fail closed"
                );
                error
            })
            .ok();
        Self {
            bus_root,
            cursor: None,
            authorizer: Arc::new(ActionAuthorizer::production()),
            signer,
            publisher_store,
        }
    }

    /// Keep the action lane on the same persisted Bus root as catalog output.
    pub fn set_bus_root(&mut self, bus_root: Option<PathBuf>) {
        self.bus_root = bus_root;
        self.cursor = None;
    }

    /// Drain a bounded action page and publish one typed reply per input row.
    pub fn tick(&mut self, now_ms: u64) {
        let Some(root) = self.bus_root.as_deref() else {
            return;
        };
        let Ok(persist) = Persist::open(root.to_path_buf()) else {
            return;
        };
        let Ok(messages) = persist.list_since_limit(
            RESOURCE_ACTION_TOPIC,
            self.cursor.as_deref(),
            MAX_ACTIONS_PER_TICK,
        ) else {
            return;
        };
        for message in messages {
            self.cursor = Some(message.ulid.clone());
            let body = message.body.as_deref().unwrap_or_default();
            let reply = self.handle(&persist, body, now_ms);
            let serialized = serde_json::to_string(&reply).unwrap_or_else(|_| {
                r#"{"schema_version":1,"request_id":"invalid","accepted":false,"downstream_topic":null,"downstream_reply_topic":null,"downstream_reply_kind":null,"binding":null,"refusal":"malformed"}"#.to_owned()
            });
            let _ = persist.write(
                &mde_bus::rpc::reply_topic(&message.ulid),
                Priority::Default,
                None,
                Some(&serialized),
            );
        }
    }

    fn handle(&self, persist: &Persist, body: &str, now_ms: u64) -> ResourceActionReply {
        let request_id = safe_request_id(body);
        if body.len() > MAX_ACTION_BODY_BYTES {
            return refused(request_id, RefusalCode::Malformed);
        }
        let invocation: ResourceActionInvocation = match serde_json::from_str(body) {
            Ok(invocation) => invocation,
            Err(_) => return refused(request_id, RefusalCode::Malformed),
        };
        let context_target = format!("{}:{}", invocation.resource_id, invocation.action_id);
        let context_verb = resource_auth_verb(&invocation);
        if self
            .authorizer
            .authorize(
                body,
                MutationContext {
                    verb: context_verb,
                    node: "resource-authority",
                    target: &context_target,
                },
            )
            .is_err()
        {
            return refused(invocation.request_id, RefusalCode::Unauthorized);
        }
        let signer = match self.signer.as_ref() {
            Some(signer) => signer,
            None => return refused(invocation.request_id, RefusalCode::AuthorityUnavailable),
        };
        let planned = if invocation.vdi_open_receipt.is_some() {
            match plan_receipt_bound_vdi_close(&invocation, signer, now_ms) {
                Ok(planned) => planned,
                Err(code) => return refused(invocation.request_id, code),
            }
        } else {
            let Some(catalog) = persist
                .read_latest(RESOURCE_CATALOG_TOPIC)
                .ok()
                .flatten()
                .and_then(|message| message.body)
                .and_then(|body| ResourceCatalog::from_json(&body).ok())
            else {
                return refused(invocation.request_id, RefusalCode::AuthorityUnavailable);
            };
            let attestation = persist
                .read_latest(&resource_publisher_attestation_topic(&catalog.publisher))
                .ok()
                .flatten()
                .and_then(|message| message.body)
                .and_then(|body| serde_json::from_str::<ResourcePublisherAttestation>(&body).ok());
            let publisher_key = self
                .publisher_store
                .get(super::RESOURCE_PUBLISHER_KEY_REF)
                .ok()
                .flatten();
            if attestation
                .as_ref()
                .zip(publisher_key.as_ref())
                .is_none_or(|(attestation, key)| {
                    catalog
                        .validate_publisher_attestation(attestation, key.as_bytes(), now_ms)
                        .is_err()
                })
            {
                return refused(invocation.request_id, RefusalCode::Unauthorized);
            }
            match plan(&catalog, &invocation, signer, now_ms) {
                Ok(planned) => planned,
                Err(RefusalCode::CancellationUnsupported) => {
                    return unsupported_clipboard_cancellation_reply(&invocation, signer, now_ms)
                        .unwrap_or_else(|code| refused(invocation.request_id.clone(), code));
                }
                Err(code) => return refused(invocation.request_id, code),
            }
        };
        let downstream =
            match persist.write(&planned.topic, Priority::Default, None, Some(&planned.body)) {
                Ok(message) => message,
                Err(_) => return refused(invocation.request_id, RefusalCode::AuthorityUnavailable),
            };
        let generated_reply_topic = mde_bus::rpc::reply_topic(&downstream.ulid);
        if let Some(completion) = planned.vdi_completion.as_ref() {
            match vdi_completion_reply(
                &invocation,
                &planned,
                completion,
                &downstream.ulid,
                signer,
                now_ms,
            )
            .and_then(|completion| {
                serde_json::to_string(&completion).map_err(|_| RefusalCode::Malformed)
            }) {
                Ok(body) => {
                    if let Err(error) =
                        persist.write(&generated_reply_topic, Priority::Default, None, Some(&body))
                    {
                        tracing::warn!(
                            target: "mackesd::resource_actions",
                            request_id = %invocation.request_id,
                            %error,
                            "VDI dispatch completed but its typed authority reply could not be persisted"
                        );
                    }
                }
                Err(code) => {
                    tracing::warn!(
                        target: "mackesd::resource_actions",
                        request_id = %invocation.request_id,
                        ?code,
                        "VDI dispatch completed but its typed authority reply could not be encoded"
                    );
                }
            }
        }
        tracing::debug!(
            target: "mackesd::resource_actions",
            authority_verb = planned.verb,
            authority_node = %planned.node,
            authority_target = %planned.target,
            "routed typed universal-resource action"
        );
        let cancellation_completion = if matches!(
            &invocation.authority_request,
            TypedAuthorityRequest::AndroidProvider(AndroidLifecycleRequest {
                operation: AndroidOperation::Cancel,
                ..
            })
        ) {
            cancellation_completion_reply(
                &invocation,
                CancellationAuthority::AndroidLifecycle,
                CancellationCompletionOutcome::DispatchAccepted,
                Some((&planned, &downstream.ulid)),
                planned.verb,
                &planned.node,
                &planned.target,
                signer,
                now_ms,
            )
            .ok()
        } else {
            None
        };
        ResourceActionReply {
            schema_version: SCHEMA_VERSION,
            request_id: invocation.request_id.clone(),
            accepted: true,
            downstream_topic: Some(planned.topic),
            downstream_reply_topic: planned.reply_topic.map(|topic| {
                if topic.is_empty() {
                    generated_reply_topic
                } else {
                    topic
                }
            }),
            downstream_reply_kind: planned.reply_kind,
            binding: Some(reply_binding(&invocation)),
            cancellation_completion,
            refusal: None,
        }
    }
}

fn plan(
    catalog: &ResourceCatalog,
    invocation: &ResourceActionInvocation,
    signer: &CloudArmSigner,
    now_ms: u64,
) -> Result<PlannedAction, RefusalCode> {
    catalog.validate().map_err(|_| RefusalCode::StaleCatalog)?;
    if invocation.schema_version != SCHEMA_VERSION
        || !safe_id(&invocation.request_id)
        || !safe_id(&invocation.cancellation_id)
        || invocation
            .cancels_request_id
            .as_deref()
            .is_some_and(|request_id| !safe_id(request_id))
        || invocation.catalog_revision != catalog.revision
        || invocation.catalog_content_digest != catalog.computed_content_digest()
        || invocation.issued_at_ms == 0
        || invocation.issued_at_ms > now_ms
        || invocation.deadline_at_ms <= now_ms
        || invocation.deadline_at_ms.saturating_sub(now_ms)
            > u64::try_from(MAX_AUTH_TTL_MS).unwrap_or(30_000)
    {
        return Err(RefusalCode::StaleCatalog);
    }
    let card = catalog
        .cards
        .iter()
        .find(|card| card.resource_id() == invocation.resource_id)
        .ok_or(RefusalCode::TargetMismatch)?;
    if card.is_expired(now_ms) || invocation.deadline_at_ms > card.expires_at_ms {
        return Err(RefusalCode::StaleCard);
    }
    if !matches!(
        card.health.status,
        HealthStatus::Available | HealthStatus::Degraded
    ) {
        return Err(RefusalCode::Unavailable);
    }
    let action = card
        .actions
        .iter()
        .find(|action| action.action_id == invocation.action_id)
        .ok_or(RefusalCode::CapabilityMismatch)?;
    if action.verb != invocation.verb || action.target != invocation.target {
        return Err(RefusalCode::TargetMismatch);
    }
    let locally_approved = local_approval_admits(card, action, invocation, now_ms);
    if (!matches!(
        card.auth.status,
        AuthStatus::NotRequired | AuthStatus::Authorized
    ) || action.availability.status != ActionAvailabilityStatus::Ready)
        && !locally_approved
    {
        return Err(RefusalCode::Unavailable);
    }
    if action.expires_at_ms <= now_ms || invocation.deadline_at_ms > action.expires_at_ms {
        return Err(RefusalCode::Unavailable);
    }
    if action.availability.status == ActionAvailabilityStatus::Ready
        && invocation.local_approval.is_some()
    {
        return Err(RefusalCode::CapabilityMismatch);
    }

    match &invocation.authority_request {
        TypedAuthorityRequest::Workload(request) => {
            plan_workload(card, invocation, request.clone(), signer)
        }
        TypedAuthorityRequest::Vdi(request) => plan_vdi(card, invocation, request, signer),
        TypedAuthorityRequest::Clipboard {
            direction,
            lease,
            message,
        } => plan_clipboard(card, invocation, *direction, lease, message, now_ms),
        TypedAuthorityRequest::ClipboardCancellation {
            direction,
            session_id,
            generation,
            lease_id,
            message_sequence,
            target_request_id,
        } => plan_clipboard_cancellation(
            card,
            invocation,
            *direction,
            session_id,
            *generation,
            lease_id,
            *message_sequence,
            target_request_id,
        ),
        TypedAuthorityRequest::AndroidProvider(request) => {
            plan_android(card, invocation, request.clone(), signer)
        }
    }
}

fn plan_receipt_bound_vdi_close(
    invocation: &ResourceActionInvocation,
    signer: &CloudArmSigner,
    now_ms: u64,
) -> Result<PlannedAction, RefusalCode> {
    let receipt = invocation
        .vdi_open_receipt
        .as_ref()
        .ok_or(RefusalCode::CapabilityMismatch)?;
    let TypedAuthorityRequest::Vdi(StrictSessionRequest::Close { id }) =
        &invocation.authority_request
    else {
        return Err(RefusalCode::CapabilityMismatch);
    };
    if invocation.schema_version != SCHEMA_VERSION
        || invocation.verb != ResourceActionVerb::Connect
        || invocation.local_approval.is_some()
        || invocation.armed_token.is_some()
        || !safe_id(&invocation.request_id)
        || !safe_id(&invocation.cancellation_id)
        || invocation.cancels_request_id.as_deref() != Some(id.as_str())
        || !safe_id(id)
        || id == &invocation.request_id
        || invocation.issued_at_ms == 0
        || invocation.issued_at_ms > now_ms
        || invocation.deadline_at_ms <= now_ms
        || invocation.deadline_at_ms.saturating_sub(now_ms)
            > u64::try_from(MAX_AUTH_TTL_MS).unwrap_or(30_000)
    {
        return Err(RefusalCode::StaleCatalog);
    }
    let original = &receipt.binding;
    if receipt.schema_version != SCHEMA_VERSION
        || receipt.request_id != *id
        || receipt.session_id != *id
        || receipt.outcome != VdiCompletionOutcome::DispatchAccepted
        || receipt.completed_at_ms == 0
        || receipt.completed_at_ms > now_ms
        || !safe_id(&receipt.downstream_message_id)
        || receipt.downstream_request_digest.len() != 64
        || !receipt
            .downstream_request_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || receipt.authority_verb != "vdi-session-open"
        || receipt.authority_node != "vdi-session"
        || receipt.authority_target != format!("session:{id}")
        || !safe_id(&receipt.serving_peer)
        || original.catalog_revision != invocation.catalog_revision
        || original.catalog_content_digest != invocation.catalog_content_digest
        || original.resource_id != invocation.resource_id
        || original.action_id != invocation.action_id
        || original.verb != invocation.verb
        || original.target != invocation.target
        || original.expected_generation != invocation.expected_generation
        || original.cancels_request_id.is_some()
        || !safe_id(&original.cancellation_id)
        || !signer.verify_payload(&receipt.signing_payload()?, &receipt.authority_signature)
    {
        return Err(RefusalCode::Unauthorized);
    }

    let shared = SessionRequest::Close { id: id.clone() };
    let mut document = serde_json::to_value(shared).map_err(|_| RefusalCode::Malformed)?;
    let object = document.as_object_mut().ok_or(RefusalCode::Malformed)?;
    object.insert(
        "schema_version".into(),
        serde_json::Value::from(CLOUD_SCHEMA_VERSION),
    );
    object.insert(
        "resource_request_id".into(),
        serde_json::Value::String(invocation.request_id.clone()),
    );
    object.insert(
        "resource_id".into(),
        serde_json::Value::String(invocation.resource_id.clone()),
    );
    object.insert(
        "resource_action_id".into(),
        serde_json::Value::String(invocation.action_id.clone()),
    );
    object.insert(
        "resource_action_verb".into(),
        serde_json::to_value(invocation.verb).map_err(|_| RefusalCode::Malformed)?,
    );
    object.insert(
        "resource_expected_generation".into(),
        serde_json::Value::from(invocation.expected_generation),
    );
    object.insert(
        "resource_catalog_revision".into(),
        serde_json::Value::String(invocation.catalog_revision.clone()),
    );
    object.insert(
        "resource_catalog_content_digest".into(),
        serde_json::Value::String(invocation.catalog_content_digest.clone()),
    );
    object.insert(
        "resource_cancels_request_id".into(),
        serde_json::Value::String(id.clone()),
    );
    let target = format!("session:{id}");
    let body = arm_document(
        signer,
        document,
        &invocation.cancellation_id,
        invocation.deadline_at_ms,
        "vdi-session-close",
        "vdi-session",
        &target,
    )?;
    Ok(PlannedAction {
        topic: crate::workers::session_broker::ACTION_TOPIC.to_owned(),
        body,
        reply_topic: Some(String::new()),
        reply_kind: Some(DownstreamReplyKind::VdiAuthorityCompletion),
        verb: "vdi-session-close",
        node: receipt.serving_peer.clone(),
        target,
        vdi_completion: Some(PlannedVdiCompletion {
            session_id: id.clone(),
            serving_peer: receipt.serving_peer.clone(),
        }),
    })
}

fn local_approval_admits(
    card: &mackes_mesh_types::resources::ResourceCard,
    action: &mackes_mesh_types::resources::ResourceAction,
    invocation: &ResourceActionInvocation,
    now_ms: u64,
) -> bool {
    let Some(approval) = invocation.local_approval.as_ref() else {
        return false;
    };
    card.identity.class == ResourceClass::Desktop
        && card.auth.status == AuthStatus::Required
        && card.auth.accepted_methods == [mackes_mesh_types::resources::AuthMethod::LocalApproval]
        && card.auth.active_method.is_none()
        && action.verb == ResourceActionVerb::Connect
        && invocation.cancels_request_id.is_none()
        && action.availability.status == ActionAvailabilityStatus::RequiresApproval
        && approval.catalog_revision == invocation.catalog_revision
        && approval.catalog_content_digest == invocation.catalog_content_digest
        && approval.resource_id == invocation.resource_id
        && approval.action_id == invocation.action_id
        && approval.target == invocation.target
        && approval.approved_at_ms != 0
        && approval.approved_at_ms <= invocation.issued_at_ms
        && invocation
            .issued_at_ms
            .saturating_sub(approval.approved_at_ms)
            <= u64::try_from(MAX_AUTH_TTL_MS).unwrap_or(30_000)
        && approval.approved_at_ms <= now_ms
        && approval.expires_at_ms >= invocation.deadline_at_ms
        && approval.expires_at_ms <= action.expires_at_ms
}

fn plan_workload(
    card: &mackes_mesh_types::resources::ResourceCard,
    invocation: &ResourceActionInvocation,
    mut request: WorkloadOperationRequest,
    signer: &CloudArmSigner,
) -> Result<PlannedAction, RefusalCode> {
    let cancellation_target = invocation.cancels_request_id.as_deref();
    let request_cancellation_target = request.target_request_id.as_deref();
    let is_cancellation = request.action == WorkloadOperationAction::Cancel;
    if !matches!(
        card.identity.class,
        ResourceClass::VirtualMachine | ResourceClass::Container | ResourceClass::CloudWorkload
    ) || request.request_id != invocation.request_id
        || request.expected_generation != invocation.expected_generation
        || request.deadline_at_ms != invocation.deadline_at_ms
        || request.armed_token.is_some()
        || if is_cancellation {
            cancellation_target.is_none()
                || cancellation_target != request_cancellation_target
                || cancellation_target == Some(request.request_id.as_str())
        } else {
            cancellation_target.is_some() || request_cancellation_target.is_some()
        }
        || card.identity.canonical_key
            != format!(
                "workload/{}/{}",
                request.target_node,
                request.workload_id.as_str()
            )
    {
        return Err(RefusalCode::TargetMismatch);
    }
    let expected = match invocation.verb {
        ResourceActionVerb::Connect => [
            WorkloadOperationAction::Open,
            WorkloadOperationAction::StartAndAttach,
        ]
        .as_slice(),
        ResourceActionVerb::Launch => [
            WorkloadOperationAction::StartAndAttach,
            WorkloadOperationAction::Open,
        ]
        .as_slice(),
        ResourceActionVerb::Start => [WorkloadOperationAction::Start].as_slice(),
        ResourceActionVerb::Resume => [WorkloadOperationAction::Resume].as_slice(),
        _ => return Err(RefusalCode::CapabilityMismatch),
    };
    if !is_cancellation && !expected.contains(&request.action) {
        return Err(RefusalCode::CapabilityMismatch);
    }
    let target = format!("workload:{}", request.workload_id.as_str());
    request.armed_token = Some(arm(
        signer,
        &request,
        &invocation.cancellation_id,
        invocation.deadline_at_ms,
        "workload-operation",
        &request.target_node,
        &target,
    )?);
    Ok(PlannedAction {
        topic: WORKLOAD_OPERATION_TOPIC.to_owned(),
        body: serde_json::to_string(&request).map_err(|_| RefusalCode::Malformed)?,
        reply_topic: Some(String::new()),
        reply_kind: Some(DownstreamReplyKind::WorkloadOperation),
        verb: "workload-operation",
        node: request.target_node,
        target,
        vdi_completion: None,
    })
}

fn plan_vdi(
    card: &mackes_mesh_types::resources::ResourceCard,
    invocation: &ResourceActionInvocation,
    request: &StrictSessionRequest,
    signer: &CloudArmSigner,
) -> Result<PlannedAction, RefusalCode> {
    let (id, serving_peer, canonical_key, verb, is_cancellation) = match request {
        StrictSessionRequest::Open {
            id,
            serving_peer,
            vm_id,
            ..
        } if invocation.verb == ResourceActionVerb::Connect
            && invocation.cancels_request_id.is_none() =>
        {
            (
                id.clone(),
                serving_peer.clone(),
                if invocation.local_approval.is_some() {
                    card.identity.canonical_key.clone()
                } else {
                    format!("vdi/{serving_peer}/{vm_id}")
                },
                "vdi-session-open",
                false,
            )
        }
        StrictSessionRequest::OpenApp {
            id,
            serving_peer,
            app_id,
            resume,
            ..
        } if invocation.verb == ResourceActionVerb::Launch
            && !resume
            && invocation.cancels_request_id.is_none() =>
        {
            (
                id.clone(),
                serving_peer.clone(),
                format!("app-vm/{serving_peer}/{app_id}"),
                "vdi-session-open",
                false,
            )
        }
        StrictSessionRequest::Close { id } => {
            let cancellation_target = invocation
                .cancels_request_id
                .as_deref()
                .ok_or(RefusalCode::TargetMismatch)?;
            if id != cancellation_target || id == &invocation.request_id {
                return Err(RefusalCode::TargetMismatch);
            }
            let (serving_peer, canonical_key) = admitted_vdi_card_route(card)?;
            (
                id.clone(),
                serving_peer,
                canonical_key,
                "vdi-session-close",
                true,
            )
        }
        _ => return Err(RefusalCode::CapabilityMismatch),
    };
    if (!is_cancellation && id != invocation.request_id)
        || card.identity.canonical_key != canonical_key
        || !matches!(
            card.identity.class,
            ResourceClass::Desktop | ResourceClass::Application
        )
    {
        return Err(RefusalCode::TargetMismatch);
    }
    if invocation.local_approval.is_some()
        && !approved_external_vdi_route(card, invocation, request)
    {
        return Err(RefusalCode::TargetMismatch);
    }
    let shared = request.shared();
    if shared.browser_transport().is_err() {
        return Err(RefusalCode::CapabilityMismatch);
    }
    if let SessionRequest::OpenApp {
        app_id,
        catalog_revision,
        guest_profile,
        requested_capabilities,
        ..
    } = &shared
    {
        mackes_mesh_types::vdi_session::AppVmLaunchRequest::new(
            app_id.clone(),
            catalog_revision.clone(),
            guest_profile.clone(),
            requested_capabilities.clone(),
            id.clone(),
            false,
        )
        .and_then(|request| request.validate_admitted().map(|_| request))
        .map_err(|_| RefusalCode::CapabilityMismatch)?;
    }
    let mut document = serde_json::to_value(&shared).map_err(|_| RefusalCode::Malformed)?;
    let object = document.as_object_mut().ok_or(RefusalCode::Malformed)?;
    object.insert(
        "schema_version".into(),
        serde_json::Value::from(CLOUD_SCHEMA_VERSION),
    );
    object.insert(
        "resource_request_id".into(),
        serde_json::Value::String(invocation.request_id.clone()),
    );
    object.insert(
        "resource_id".into(),
        serde_json::Value::String(invocation.resource_id.clone()),
    );
    object.insert(
        "resource_action_id".into(),
        serde_json::Value::String(invocation.action_id.clone()),
    );
    object.insert(
        "resource_action_verb".into(),
        serde_json::to_value(invocation.verb).map_err(|_| RefusalCode::Malformed)?,
    );
    object.insert(
        "resource_expected_generation".into(),
        serde_json::Value::from(invocation.expected_generation),
    );
    object.insert(
        "resource_catalog_revision".into(),
        serde_json::Value::String(invocation.catalog_revision.clone()),
    );
    object.insert(
        "resource_catalog_content_digest".into(),
        serde_json::Value::String(invocation.catalog_content_digest.clone()),
    );
    if let Some(cancels_request_id) = invocation.cancels_request_id.as_ref() {
        object.insert(
            "resource_cancels_request_id".into(),
            serde_json::Value::String(cancels_request_id.clone()),
        );
    }
    let target = format!("session:{id}");
    let body = arm_document(
        signer,
        document,
        &invocation.cancellation_id,
        invocation.deadline_at_ms,
        verb,
        "vdi-session",
        &target,
    )?;
    Ok(PlannedAction {
        topic: crate::workers::session_broker::ACTION_TOPIC.to_owned(),
        body,
        reply_topic: Some(String::new()),
        reply_kind: Some(DownstreamReplyKind::VdiAuthorityCompletion),
        verb,
        node: serving_peer.clone(),
        target,
        vdi_completion: Some(PlannedVdiCompletion {
            session_id: id,
            serving_peer,
        }),
    })
}

fn approved_external_vdi_route(
    card: &mackes_mesh_types::resources::ResourceCard,
    invocation: &ResourceActionInvocation,
    request: &StrictSessionRequest,
) -> bool {
    let route_identity_matches = match request {
        StrictSessionRequest::Open {
            serving_peer,
            vm_id,
            profile,
            ..
        } => {
            profile.is_none()
                && vm_id == &card.identity.canonical_key
                && admitted_external_vdi_card_route(card, invocation)
                    .is_ok_and(|(host, _)| host == *serving_peer)
        }
        StrictSessionRequest::Close { .. } => false,
        StrictSessionRequest::OpenApp { .. } => false,
    };
    if !route_identity_matches {
        return false;
    }
    true
}

fn admitted_external_vdi_card_route(
    card: &mackes_mesh_types::resources::ResourceCard,
    invocation: &ResourceActionInvocation,
) -> Result<(String, String), RefusalCode> {
    let ResourceActionTarget::TransportClient {
        transport_fingerprint,
        capability_fingerprint,
    } = &invocation.target
    else {
        return Err(RefusalCode::TargetMismatch);
    };
    let Some(transport) = card
        .transports
        .iter()
        .find(|candidate| &candidate.fingerprint == transport_fingerprint)
    else {
        return Err(RefusalCode::TargetMismatch);
    };
    if transport.client_capability_fingerprint.as_ref() != Some(capability_fingerprint) {
        return Err(RefusalCode::TargetMismatch);
    }
    let mackes_mesh_types::resources::TransportEndpoint::Network { host, .. } = &transport.endpoint
    else {
        return Err(RefusalCode::TargetMismatch);
    };
    Ok((host.clone(), card.identity.canonical_key.clone()))
}

fn admitted_vdi_card_route(
    card: &mackes_mesh_types::resources::ResourceCard,
) -> Result<(String, String), RefusalCode> {
    let expected_prefix = match card.identity.class {
        ResourceClass::Desktop => "vdi",
        ResourceClass::Application => "app-vm",
        _ => return Err(RefusalCode::TargetMismatch),
    };
    let mut parts = card.identity.canonical_key.split('/');
    let (Some(prefix), Some(serving_peer), Some(target), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(RefusalCode::TargetMismatch);
    };
    if prefix != expected_prefix || !safe_id(serving_peer) || !safe_id(target) {
        return Err(RefusalCode::TargetMismatch);
    }
    Ok((serving_peer.to_owned(), card.identity.canonical_key.clone()))
}

fn vdi_completion_reply(
    invocation: &ResourceActionInvocation,
    planned: &PlannedAction,
    completion: &PlannedVdiCompletion,
    downstream_message_id: &str,
    signer: &CloudArmSigner,
    now_ms: u64,
) -> Result<VdiAuthorityCompletionReply, RefusalCode> {
    let mut reply = VdiAuthorityCompletionReply {
        schema_version: SCHEMA_VERSION,
        request_id: invocation.request_id.clone(),
        session_id: completion.session_id.clone(),
        serving_peer: completion.serving_peer.clone(),
        outcome: VdiCompletionOutcome::DispatchAccepted,
        completed_at_ms: now_ms,
        downstream_message_id: downstream_message_id.to_owned(),
        downstream_request_digest: cloud_request_digest(&planned.body)
            .map_err(|_| RefusalCode::Malformed)?,
        authority_verb: planned.verb.to_owned(),
        authority_node: "vdi-session".to_owned(),
        authority_target: planned.target.clone(),
        binding: reply_binding(invocation),
        authority_signature: String::new(),
    };
    reply.authority_signature = signer.sign_payload(&reply.signing_payload()?);
    if reply.authority_signature.is_empty() {
        return Err(RefusalCode::AuthorityUnavailable);
    }
    Ok(reply)
}

fn plan_clipboard(
    card: &mackes_mesh_types::resources::ResourceCard,
    invocation: &ResourceActionInvocation,
    direction: ClipboardDirection,
    lease: &VdiClipboardLeaseV2,
    message: &VdiClipboardMessageV2,
    now_ms: u64,
) -> Result<PlannedAction, RefusalCode> {
    if invocation.verb != ResourceActionVerb::Transfer
        || invocation.cancels_request_id.is_some()
        || invocation.expected_generation != message.generation
        || card.identity.canonical_key != format!("vdi-session/{}", message.session_id)
        || !matches!(
            card.identity.class,
            ResourceClass::Desktop | ResourceClass::FileShare
        )
        || message.admit(lease, None, now_ms).is_err()
    {
        return Err(RefusalCode::TargetMismatch);
    }
    let prefix = match direction {
        ClipboardDirection::HostToGuest => VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
        ClipboardDirection::GuestToHost => VDI_CLIPBOARD_GUEST_TO_HOST_TOPIC_PREFIX,
    };
    let topic = vdi_clipboard_session_topic(prefix, &message.session_id)
        .map_err(|_| RefusalCode::TargetMismatch)?;
    let receipt =
        vdi_clipboard_session_topic(VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX, &message.session_id)
            .map_err(|_| RefusalCode::TargetMismatch)?;
    Ok(PlannedAction {
        topic,
        body: serde_json::to_string(message).map_err(|_| RefusalCode::Malformed)?,
        reply_topic: Some(receipt),
        reply_kind: Some(DownstreamReplyKind::ClipboardReceipt),
        verb: "vdi-clipboard-transfer",
        node: message.session_id.clone(),
        target: format!("session:{}", message.session_id),
        vdi_completion: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn plan_clipboard_cancellation(
    card: &mackes_mesh_types::resources::ResourceCard,
    invocation: &ResourceActionInvocation,
    _direction: ClipboardDirection,
    session_id: &str,
    generation: u64,
    lease_id: &str,
    message_sequence: u64,
    target_request_id: &str,
) -> Result<PlannedAction, RefusalCode> {
    let cancellation_target = invocation
        .cancels_request_id
        .as_deref()
        .ok_or(RefusalCode::TargetMismatch)?;
    if invocation.verb != ResourceActionVerb::Transfer
        || invocation.expected_generation != generation
        || card.identity.canonical_key != format!("vdi-session/{session_id}")
        || !matches!(
            card.identity.class,
            ResourceClass::Desktop | ResourceClass::FileShare
        )
        || !safe_id(session_id)
        || !safe_id(lease_id)
        || message_sequence == 0
        || !safe_id(target_request_id)
        || target_request_id != cancellation_target
        || target_request_id == invocation.request_id
    {
        return Err(RefusalCode::TargetMismatch);
    }

    // Clipboard V2 exposes message, lease, and receipt contracts only. Never
    // turn cancellation into a synthetic message or caller-selected topic.
    Err(RefusalCode::CancellationUnsupported)
}

fn plan_android(
    card: &mackes_mesh_types::resources::ResourceCard,
    invocation: &ResourceActionInvocation,
    mut request: AndroidLifecycleRequest,
    signer: &CloudArmSigner,
) -> Result<PlannedAction, RefusalCode> {
    let cancellation_target = invocation.cancels_request_id.as_deref();
    let is_cancellation = request.operation == AndroidOperation::Cancel;
    let mut key = card.identity.canonical_key.split('/');
    let (Some(prefix), Some(card_node), Some(card_workload), Some(card_package), None) =
        (key.next(), key.next(), key.next(), key.next(), key.next())
    else {
        return Err(RefusalCode::TargetMismatch);
    };
    if !matches!(
        invocation.verb,
        ResourceActionVerb::Start | ResourceActionVerb::Launch
    ) || !matches!(
        request.operation,
        AndroidOperation::Start | AndroidOperation::Cancel
    ) || request.schema_version != CLOUD_SCHEMA_VERSION
        || request.request_id != invocation.request_id
        || request.expected_generation != invocation.expected_generation
        || request.armed_token.is_some()
        || request.typed_name.is_some()
        || card.identity.class != ResourceClass::Application
        || prefix != "android-app"
        || card_node != request.node
        || card_workload != request.workload_id
        || (is_cancellation
            && (request.app.is_some()
                || cancellation_target.is_none()
                || cancellation_target == Some(request.request_id.as_str())))
        || (!is_cancellation
            && (!request
                .app
                .is_some_and(|app| app.package_id().as_str() == card_package)
                || cancellation_target.is_some()))
    {
        return Err(RefusalCode::TargetMismatch);
    }
    let target = request.workload_id.clone();
    request.armed_token = Some(arm(
        signer,
        &request,
        &invocation.cancellation_id,
        invocation.deadline_at_ms,
        "android-lifecycle",
        &request.node,
        &target,
    )?);
    Ok(PlannedAction {
        topic: "action/cloud/android-lifecycle".to_owned(),
        body: serde_json::to_string(&request).map_err(|_| RefusalCode::Malformed)?,
        reply_topic: Some(String::new()),
        reply_kind: Some(DownstreamReplyKind::CloudOperation),
        verb: "android-lifecycle",
        node: request.node,
        target,
        vdi_completion: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn cancellation_completion_reply(
    invocation: &ResourceActionInvocation,
    authority: CancellationAuthority,
    outcome: CancellationCompletionOutcome,
    downstream: Option<(&PlannedAction, &str)>,
    authority_verb: &str,
    authority_node: &str,
    authority_target: &str,
    signer: &CloudArmSigner,
    now_ms: u64,
) -> Result<CancellationAuthorityCompletionReply, RefusalCode> {
    let invocation_body = serde_json::to_string(invocation).map_err(|_| RefusalCode::Malformed)?;
    let (downstream_message_id, downstream_request_digest) = match downstream {
        Some((planned, message_id)) => (
            Some(message_id.to_owned()),
            Some(cloud_request_digest(&planned.body).map_err(|_| RefusalCode::Malformed)?),
        ),
        None => (None, None),
    };
    let mut completion = CancellationAuthorityCompletionReply {
        schema_version: SCHEMA_VERSION,
        request_id: invocation.request_id.clone(),
        authority,
        outcome,
        completed_at_ms: now_ms,
        invocation_digest: cloud_request_digest(&invocation_body)
            .map_err(|_| RefusalCode::Malformed)?,
        downstream_message_id,
        downstream_request_digest,
        authority_verb: authority_verb.to_owned(),
        authority_node: authority_node.to_owned(),
        authority_target: authority_target.to_owned(),
        binding: reply_binding(invocation),
        authority_signature: String::new(),
    };
    completion.authority_signature = signer.sign_payload(&completion.signing_payload()?);
    if completion.authority_signature.is_empty() {
        return Err(RefusalCode::AuthorityUnavailable);
    }
    Ok(completion)
}

fn unsupported_clipboard_cancellation_reply(
    invocation: &ResourceActionInvocation,
    signer: &CloudArmSigner,
    now_ms: u64,
) -> Result<ResourceActionReply, RefusalCode> {
    let TypedAuthorityRequest::ClipboardCancellation {
        direction,
        session_id,
        generation,
        lease_id,
        message_sequence,
        target_request_id,
    } = &invocation.authority_request
    else {
        return Err(RefusalCode::Malformed);
    };
    let direction = match direction {
        ClipboardDirection::HostToGuest => "host-to-guest",
        ClipboardDirection::GuestToHost => "guest-to-host",
    };
    let authority_target = format!(
        "{direction}:{session_id}:{generation}:{lease_id}:{message_sequence}:{target_request_id}"
    );
    let completion = cancellation_completion_reply(
        invocation,
        CancellationAuthority::ClipboardTransfer,
        CancellationCompletionOutcome::UnsupportedCancellation,
        None,
        "vdi-clipboard-cancel",
        session_id,
        &authority_target,
        signer,
        now_ms,
    )?;
    Ok(ResourceActionReply {
        schema_version: SCHEMA_VERSION,
        request_id: invocation.request_id.clone(),
        accepted: false,
        downstream_topic: None,
        downstream_reply_topic: None,
        downstream_reply_kind: None,
        binding: Some(reply_binding(invocation)),
        cancellation_completion: Some(completion),
        refusal: Some(RefusalCode::CancellationUnsupported),
    })
}

fn arm<T: Serialize>(
    signer: &CloudArmSigner,
    request: &T,
    nonce: &str,
    deadline_at_ms: u64,
    verb: &str,
    node: &str,
    target: &str,
) -> Result<String, RefusalCode> {
    let document = serde_json::to_value(request).map_err(|_| RefusalCode::Malformed)?;
    let unsigned = serde_json::to_string(&document).map_err(|_| RefusalCode::Malformed)?;
    let digest = cloud_request_digest(&unsigned).map_err(|_| RefusalCode::Malformed)?;
    let expires = i64::try_from(deadline_at_ms).map_err(|_| RefusalCode::Malformed)?;
    Ok(CloudArmedToken::mint(signer, nonce, expires, verb, node, target, &digest).encode())
}

fn arm_document(
    signer: &CloudArmSigner,
    mut document: serde_json::Value,
    nonce: &str,
    deadline_at_ms: u64,
    verb: &str,
    node: &str,
    target: &str,
) -> Result<String, RefusalCode> {
    let unsigned = document.to_string();
    let digest = cloud_request_digest(&unsigned).map_err(|_| RefusalCode::Malformed)?;
    let expires = i64::try_from(deadline_at_ms).map_err(|_| RefusalCode::Malformed)?;
    let token = CloudArmedToken::mint(signer, nonce, expires, verb, node, target, &digest).encode();
    document
        .as_object_mut()
        .ok_or(RefusalCode::Malformed)?
        .insert("armed_token".into(), serde_json::Value::String(token));
    Ok(document.to_string())
}

fn resource_auth_verb(invocation: &ResourceActionInvocation) -> &'static str {
    if matches!(
        &invocation.authority_request,
        TypedAuthorityRequest::Workload(WorkloadOperationRequest {
            action: WorkloadOperationAction::Cancel,
            ..
        }) | TypedAuthorityRequest::Vdi(StrictSessionRequest::Close { .. })
            | TypedAuthorityRequest::ClipboardCancellation { .. }
            | TypedAuthorityRequest::AndroidProvider(AndroidLifecycleRequest {
                operation: AndroidOperation::Cancel,
                ..
            })
    ) {
        return "resource-action-cancel";
    }
    match invocation.verb {
        ResourceActionVerb::Connect => "resource-action-connect",
        ResourceActionVerb::Launch => "resource-action-launch",
        ResourceActionVerb::Start => "resource-action-start",
        ResourceActionVerb::Resume => "resource-action-resume",
        ResourceActionVerb::Transfer => "resource-action-transfer",
        _ => "resource-action-unsupported",
    }
}

fn reply_binding(invocation: &ResourceActionInvocation) -> ResourceActionReplyBinding {
    ResourceActionReplyBinding {
        catalog_revision: invocation.catalog_revision.clone(),
        catalog_content_digest: invocation.catalog_content_digest.clone(),
        resource_id: invocation.resource_id.clone(),
        action_id: invocation.action_id.clone(),
        verb: invocation.verb,
        target: invocation.target.clone(),
        expected_generation: invocation.expected_generation,
        cancellation_id: invocation.cancellation_id.clone(),
        cancels_request_id: invocation.cancels_request_id.clone(),
    }
}

fn refused(request_id: String, refusal: RefusalCode) -> ResourceActionReply {
    ResourceActionReply {
        schema_version: SCHEMA_VERSION,
        request_id,
        accepted: false,
        downstream_topic: None,
        downstream_reply_topic: None,
        downstream_reply_kind: None,
        binding: None,
        cancellation_completion: None,
        refusal: Some(refusal),
    }
}

fn safe_request_id(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .filter(|id| safe_id(id))
        .unwrap_or_else(|| "invalid".to_owned())
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.is_ascii()
        && value.trim() == value
        && !value.contains("..")
        && !value.contains(['/', '\\'])
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '@')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::authorize_test_body;
    use mackes_mesh_types::resources::{
        ActionAvailability, AuthMethod, AuthState, ClientBoundary, ClientCapability,
        ClientCapabilityLimits, ClientFeature, DiscoverySource, FailureCode, FailureReason,
        HealthState, IdentityAuthority, ProvenanceTrust, ResourceAction, ResourceCard,
        ResourceIdentity, ResourceOperatingRole, ResourceScope, SourceProvenance,
        TransportCandidate, TransportEndpoint, TransportProtocol, RESOURCE_CONTRACT_VERSION,
        RESOURCE_PUBLISHER_ATTESTATION_KEY_ID,
    };
    use mackes_mesh_types::vdi_clipboard::{
        ClipboardEnvelopeV2, VdiClipboardDisclosureV2, VdiClipboardText,
        VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
    };
    use mackes_mesh_types::workloads::{
        WorkloadBackend, WorkloadId, WorkloadResources, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    };

    const NOW: u64 = 1_800_000_000_000;
    const ACTION_KEY: &[u8] = b"resource-action-ingress-test-key";
    const PUBLISHER_KEY: &[u8] = b"resource-publisher-test-key";

    fn signer() -> CloudArmSigner {
        CloudArmSigner::new(vec![0x5a; 32]).expect("test signer")
    }

    fn catalog(status: HealthStatus) -> ResourceCatalog {
        let identity = ResourceIdentity::new(
            ResourceClass::VirtualMachine,
            IdentityAuthority::Mesh,
            "workload/node-a/vm-a",
            vec![],
        )
        .expect("identity");
        let card = ResourceCard {
            schema_version: RESOURCE_CONTRACT_VERSION,
            identity,
            display_name: "VM A".into(),
            summary: Some("Typed workload".into()),
            first_seen_at_ms: NOW - 1_000,
            last_seen_at_ms: NOW,
            expires_at_ms: NOW + 60_000,
            health: HealthState {
                schema_version: RESOURCE_CONTRACT_VERSION,
                status,
                observed_at_ms: NOW,
                expires_at_ms: NOW + 60_000,
                latency_ms: None,
                failure: (status == HealthStatus::Unavailable).then(|| FailureReason {
                    code: FailureCode::Unreachable,
                    message: "resource is unavailable".into(),
                }),
            },
            auth: AuthState {
                schema_version: RESOURCE_CONTRACT_VERSION,
                status: AuthStatus::NotRequired,
                accepted_methods: vec![],
                active_method: None,
                credential_ref: None,
                updated_at_ms: NOW,
                expires_at_ms: None,
                failure: None,
            },
            provenance: vec![SourceProvenance {
                schema_version: RESOURCE_CONTRACT_VERSION,
                source: DiscoverySource::ProviderRegistry,
                source_id: "workload/node-a/vm-a".into(),
                scope: ResourceScope::Mesh,
                trust: ProvenanceTrust::AuthenticatedMesh,
                interface: None,
                observed_at_ms: NOW,
                expires_at_ms: NOW + 60_000,
            }],
            transports: vec![],
            client_capabilities: vec![],
            actions: vec![ResourceAction {
                schema_version: RESOURCE_CONTRACT_VERSION,
                action_id: "start".into(),
                verb: ResourceActionVerb::Start,
                target: ResourceActionTarget::Resource,
                availability: ActionAvailability {
                    status: ActionAvailabilityStatus::Ready,
                    failure: None,
                },
                issued_at_ms: NOW,
                expires_at_ms: NOW + 60_000,
            }],
            operating_roles: vec![ResourceOperatingRole::Loader],
            service: None,
        };
        let mut catalog = ResourceCatalog {
            schema_version: RESOURCE_CONTRACT_VERSION,
            revision: "revision-7".into(),
            publisher: "node-a".into(),
            generated_at_ms: NOW,
            content_digest: None,
            cards: vec![card],
        };
        catalog.content_digest = Some(catalog.computed_content_digest());
        catalog.validate().expect("catalog");
        catalog
    }

    fn workload() -> WorkloadOperationRequest {
        WorkloadOperationRequest {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: "request-1".into(),
            workload_id: WorkloadId::new("vm-a").expect("workload id"),
            backend: WorkloadBackend::LibvirtVirtqemud,
            resources: WorkloadResources {
                vcpu: 2,
                memory_mb: 4_096,
                disk_gb: 32,
            },
            image_ref: None,
            target_node: "node-a".into(),
            expected_generation: 7,
            action: WorkloadOperationAction::Start,
            target_request_id: None,
            deadline_at_ms: NOW + 20_000,
            preferred_attachment: None,
            armed_token: None,
        }
    }

    fn invocation(catalog: &ResourceCatalog) -> ResourceActionInvocation {
        ResourceActionInvocation {
            schema_version: SCHEMA_VERSION,
            request_id: "request-1".into(),
            catalog_revision: catalog.revision.clone(),
            catalog_content_digest: catalog.computed_content_digest(),
            resource_id: catalog.cards[0].resource_id().into(),
            action_id: "start".into(),
            verb: ResourceActionVerb::Start,
            target: ResourceActionTarget::Resource,
            expected_generation: 7,
            cancellation_id: "cancel-request-1".into(),
            cancels_request_id: None,
            issued_at_ms: NOW,
            deadline_at_ms: NOW + 20_000,
            authority_request: TypedAuthorityRequest::Workload(workload()),
            vdi_open_receipt: None,
            local_approval: None,
            armed_token: None,
        }
    }

    fn vdi_catalog() -> ResourceCatalog {
        let mut catalog = catalog(HealthStatus::Available);
        let capability = ClientCapability::new(
            "construct.ironrdp",
            "12.1.6",
            TransportProtocol::Rdp,
            "10.7",
            ClientBoundary::ShellNative,
            vec![AuthMethod::MeshIdentity],
            vec![
                ClientFeature::Display,
                ClientFeature::KeyboardInput,
                ClientFeature::PointerInput,
                ClientFeature::Reconnect,
            ],
            ClientCapabilityLimits {
                max_width: Some(3_840),
                max_height: Some(2_160),
                max_fps: Some(60),
                max_audio_channels: None,
                max_parallel_sessions: 1,
            },
            vec![ResourceActionVerb::Connect],
        )
        .expect("VDI client capability");
        let transport = TransportCandidate::new(
            TransportProtocol::Rdp,
            TransportEndpoint::Network {
                host: "desktop-a.node-a.mesh".into(),
                port: 3_389,
                base_path: None,
            },
            ResourceScope::Mesh,
            10,
            NOW,
            NOW + 60_000,
            catalog.cards[0].health.clone(),
            Some(capability.fingerprint.clone()),
        )
        .expect("VDI transport");
        catalog.cards[0].identity = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "vdi/node-a/desktop-a",
            vec![],
        )
        .expect("VDI identity");
        catalog.cards[0].actions[0].action_id = "connect".into();
        catalog.cards[0].actions[0].verb = ResourceActionVerb::Connect;
        catalog.cards[0].actions[0].target = ResourceActionTarget::TransportClient {
            transport_fingerprint: transport.fingerprint.clone(),
            capability_fingerprint: capability.fingerprint.clone(),
        };
        catalog.cards[0].transports = vec![transport];
        catalog.cards[0].client_capabilities = vec![capability];
        catalog.cards[0].provenance[0].source_id = "vdi/node-a/desktop-a".into();
        catalog.content_digest = None;
        catalog.content_digest = Some(catalog.computed_content_digest());
        catalog.validate().expect("VDI catalog");
        catalog
    }

    fn vdi_invocation(catalog: &ResourceCatalog) -> ResourceActionInvocation {
        let mut invocation = invocation(catalog);
        invocation.resource_id = catalog.cards[0].resource_id().into();
        invocation.action_id = "connect".into();
        invocation.verb = ResourceActionVerb::Connect;
        invocation.target = catalog.cards[0].actions[0].target.clone();
        invocation.authority_request = TypedAuthorityRequest::Vdi(StrictSessionRequest::Open {
            id: invocation.request_id.clone(),
            serving_peer: "node-a".into(),
            vm_id: "desktop-a".into(),
            client_peer: "seat-a".into(),
            profile: None,
        });
        invocation
    }

    fn approval_gated_vdi() -> (ResourceCatalog, ResourceActionInvocation) {
        let mut catalog = vdi_catalog();
        let capability = ClientCapability::new(
            "construct.mde-vdi-rdp",
            "1",
            TransportProtocol::Rdp,
            "1",
            ClientBoundary::ShellNative,
            vec![AuthMethod::MeshIdentity, AuthMethod::LocalApproval],
            vec![
                ClientFeature::Display,
                ClientFeature::KeyboardInput,
                ClientFeature::PointerInput,
            ],
            ClientCapabilityLimits {
                max_width: Some(3_840),
                max_height: Some(2_160),
                max_fps: Some(60),
                max_audio_channels: None,
                max_parallel_sessions: 1,
            },
            vec![ResourceActionVerb::Connect],
        )
        .expect("external RDP capability");
        let transport = TransportCandidate::new(
            TransportProtocol::Rdp,
            TransportEndpoint::Network {
                host: "172.20.146.54".into(),
                port: 3_389,
                base_path: None,
            },
            ResourceScope::TrustedLan,
            0,
            NOW,
            NOW + 60_000,
            catalog.cards[0].health.clone(),
            Some(capability.fingerprint.clone()),
        )
        .expect("external RDP transport");
        catalog.cards[0].identity = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Device,
            "mdns:172.20.146.54:3389:rdp",
            vec![],
        )
        .expect("external desktop identity");
        catalog.cards[0].auth = AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::Required,
            accepted_methods: vec![AuthMethod::LocalApproval],
            active_method: None,
            credential_ref: None,
            updated_at_ms: NOW,
            expires_at_ms: None,
            failure: None,
        };
        catalog.cards[0].actions[0].target = ResourceActionTarget::TransportClient {
            transport_fingerprint: transport.fingerprint.clone(),
            capability_fingerprint: capability.fingerprint.clone(),
        };
        catalog.cards[0].actions[0].availability = ActionAvailability {
            status: ActionAvailabilityStatus::RequiresApproval,
            failure: Some(FailureReason {
                code: FailureCode::ApprovalRequired,
                message: "desktop source requires local approval".into(),
            }),
        };
        catalog.cards[0].transports = vec![transport];
        catalog.cards[0].client_capabilities = vec![capability];
        catalog.cards[0].provenance[0].source_id = "mdns:172.20.146.54:3389:rdp".into();
        catalog.content_digest = None;
        catalog.content_digest = Some(catalog.computed_content_digest());
        catalog.validate().expect("approval-gated catalog");

        let mut invocation = vdi_invocation(&catalog);
        invocation.catalog_content_digest = catalog.computed_content_digest();
        invocation.resource_id = catalog.cards[0].resource_id().into();
        invocation.target = catalog.cards[0].actions[0].target.clone();
        invocation.authority_request = TypedAuthorityRequest::Vdi(StrictSessionRequest::Open {
            id: invocation.request_id.clone(),
            serving_peer: "172.20.146.54".into(),
            vm_id: catalog.cards[0].identity.canonical_key.clone(),
            client_peer: "seat-a".into(),
            profile: None,
        });
        invocation.local_approval = Some(LocalApprovalBinding {
            catalog_revision: invocation.catalog_revision.clone(),
            catalog_content_digest: invocation.catalog_content_digest.clone(),
            resource_id: invocation.resource_id.clone(),
            action_id: invocation.action_id.clone(),
            target: invocation.target.clone(),
            approved_at_ms: NOW,
            expires_at_ms: NOW + 20_000,
        });
        (catalog, invocation)
    }

    fn vdi_cancellation(catalog: &ResourceCatalog) -> ResourceActionInvocation {
        let mut invocation = vdi_invocation(catalog);
        invocation.request_id = "cancel-vdi-request-1".into();
        invocation.cancellation_id = "cancel-vdi-capability-1".into();
        invocation.cancels_request_id = Some("request-1".into());
        invocation.authority_request = TypedAuthorityRequest::Vdi(StrictSessionRequest::Close {
            id: "request-1".into(),
        });
        invocation
    }

    fn android_catalog() -> ResourceCatalog {
        let mut catalog = catalog(HealthStatus::Available);
        let package = AospStarterApp::Browser.package_id();
        catalog.cards[0].identity = ResourceIdentity::new(
            ResourceClass::Application,
            IdentityAuthority::Mesh,
            format!("android-app/node-a/android-vm-a/{}", package.as_str()),
            vec![],
        )
        .expect("Android identity");
        catalog.cards[0].provenance[0].source_id = "android-app/node-a/android-vm-a".into();
        catalog.content_digest = None;
        catalog.content_digest = Some(catalog.computed_content_digest());
        catalog.validate().expect("Android catalog");
        catalog
    }

    fn clipboard_catalog() -> ResourceCatalog {
        let mut catalog = vdi_catalog();
        catalog.cards[0].identity = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "vdi-session/session-a",
            vec![],
        )
        .expect("clipboard session identity");
        catalog.cards[0].provenance[0].source_id = "vdi-session/session-a".into();
        let capability_fingerprint = {
            let capability = &mut catalog.cards[0].client_capabilities[0];
            capability.safe_actions.push(ResourceActionVerb::Transfer);
            capability.fingerprint = capability.computed_fingerprint();
            capability.fingerprint.clone()
        };
        let transport_fingerprint = {
            let transport = &mut catalog.cards[0].transports[0];
            transport.client_capability_fingerprint = Some(capability_fingerprint.clone());
            transport.fingerprint = transport.computed_fingerprint();
            transport.fingerprint.clone()
        };
        catalog.cards[0].actions[0].action_id = "transfer".into();
        catalog.cards[0].actions[0].verb = ResourceActionVerb::Transfer;
        catalog.cards[0].actions[0].target = ResourceActionTarget::TransportClient {
            transport_fingerprint,
            capability_fingerprint,
        };
        catalog.content_digest = None;
        catalog.content_digest = Some(catalog.computed_content_digest());
        catalog.validate().expect("clipboard catalog");
        catalog
    }

    fn android_cancellation(catalog: &ResourceCatalog) -> ResourceActionInvocation {
        let mut invocation = invocation(catalog);
        invocation.request_id = "cancel-android-request-1".into();
        invocation.cancellation_id = "cancel-android-capability-1".into();
        invocation.cancels_request_id = Some("start-android-request-1".into());
        invocation.authority_request =
            TypedAuthorityRequest::AndroidProvider(AndroidLifecycleRequest {
                schema_version: CLOUD_SCHEMA_VERSION,
                node: "node-a".into(),
                workload_id: "android-vm-a".into(),
                request_id: invocation.request_id.clone(),
                expected_generation: invocation.expected_generation,
                operation: AndroidOperation::Cancel,
                app: None,
                armed_token: None,
                typed_name: None,
            });
        invocation
    }

    fn clipboard_cancellation(catalog: &ResourceCatalog) -> ResourceActionInvocation {
        let mut invocation = invocation(catalog);
        invocation.request_id = "cancel-clipboard-request-1".into();
        invocation.cancellation_id = "cancel-clipboard-capability-1".into();
        invocation.cancels_request_id = Some("transfer-request-1".into());
        invocation.resource_id = catalog.cards[0].resource_id().into();
        invocation.action_id = "transfer".into();
        invocation.verb = ResourceActionVerb::Transfer;
        invocation.target = catalog.cards[0].actions[0].target.clone();
        invocation.authority_request = TypedAuthorityRequest::ClipboardCancellation {
            direction: ClipboardDirection::HostToGuest,
            session_id: "session-a".into(),
            generation: invocation.expected_generation,
            lease_id: "lease-a".into(),
            message_sequence: 1,
            target_request_id: "transfer-request-1".into(),
        };
        invocation
    }

    fn direct_card(class: ResourceClass, canonical_key: &str) -> ResourceCard {
        let mut card = catalog(HealthStatus::Available).cards.remove(0);
        card.identity =
            ResourceIdentity::new(class, IdentityAuthority::Mesh, canonical_key, vec![])
                .expect("direct fixture identity");
        card
    }

    fn clipboard_transfer() -> (VdiClipboardLeaseV2, VdiClipboardMessageV2) {
        let lease = VdiClipboardLeaseV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "session-a".into(),
            generation: 7,
            lease_id: "lease-a".into(),
            issued_at_ms: NOW,
            expires_at_ms: NOW + 30_000,
            permitted_mime_offers: vec!["text/plain;charset=utf-8".into()],
        };
        let envelope = ClipboardEnvelopeV2::new_inline_text(
            "node-a",
            "seat-a",
            "session-a",
            1,
            NOW,
            vec!["text/plain;charset=utf-8".into()],
            "hello",
            VdiClipboardText::new("hello").expect("bounded clipboard text"),
            NOW + 20_000,
        )
        .expect("clipboard envelope");
        let message = VdiClipboardMessageV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: lease.session_id.clone(),
            generation: lease.generation,
            lease_id: lease.lease_id.clone(),
            lease_expires_at_ms: lease.expires_at_ms,
            message_sequence: 1,
            selected_mime: "text/plain;charset=utf-8".into(),
            disclosure: VdiClipboardDisclosureV2::Shareable,
            envelope,
        };
        (lease, message)
    }

    fn local_publisher_store(root: &std::path::Path) -> SecretStore {
        let key_path = root.join("mesh-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .expect("write test age key");
        SecretStore::LocalAead {
            dir: root.join("sealed"),
            key_path,
        }
    }

    fn persisted_worker_fixture(
        catalog: &ResourceCatalog,
        authorization_state: &str,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        Persist,
        ResourceActionWorker,
    ) {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let secrets = tempfile::tempdir().expect("secret tempdir");
        let store = local_publisher_store(secrets.path());
        store
            .put(
                super::super::RESOURCE_PUBLISHER_KEY_REF,
                std::str::from_utf8(PUBLISHER_KEY).expect("publisher key text"),
            )
            .expect("store publisher key");
        let attestation = ResourcePublisherAttestation::mint(
            catalog,
            RESOURCE_PUBLISHER_ATTESTATION_KEY_ID,
            PUBLISHER_KEY,
            NOW,
            NOW + 30_000,
        )
        .expect("publisher attestation");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persisted Bus");
        persist
            .write(
                RESOURCE_CATALOG_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(catalog).expect("catalog JSON")),
            )
            .expect("publish catalog");
        persist
            .write(
                &resource_publisher_attestation_topic(&catalog.publisher),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&attestation).expect("attestation JSON")),
            )
            .expect("publish attestation");
        let worker = ResourceActionWorker {
            bus_root: Some(bus.path().to_path_buf()),
            cursor: None,
            authorizer: Arc::new(ActionAuthorizer::for_test(
                ACTION_KEY,
                bus.path().join(authorization_state),
                i64::try_from(NOW + 1).expect("test time"),
            )),
            signer: Some(signer()),
            publisher_store: store,
        };
        (bus, secrets, persist, worker)
    }

    fn signed_ingress(invocation: &ResourceActionInvocation, nonce: &str) -> String {
        let unsigned = serde_json::to_string(invocation).expect("unsigned invocation");
        let target = format!("{}:{}", invocation.resource_id, invocation.action_id);
        authorize_test_body(
            ACTION_KEY,
            &unsigned,
            MutationContext {
                verb: resource_auth_verb(invocation),
                node: "resource-authority",
                target: &target,
            },
            nonce,
            i64::try_from(invocation.deadline_at_ms).expect("deadline"),
        )
    }

    #[test]
    fn exact_workload_action_routes_only_to_workload_authority() {
        let catalog = catalog(HealthStatus::Available);
        let planned =
            plan(&catalog, &invocation(&catalog), &signer(), NOW + 1).expect("typed route");
        assert_eq!(planned.topic, WORKLOAD_OPERATION_TOPIC);
        assert_eq!(planned.verb, "workload-operation");
        assert_eq!(planned.node, "node-a");
        assert_eq!(planned.target, "workload:vm-a");
        let request: WorkloadOperationRequest =
            serde_json::from_str(&planned.body).expect("typed workload body");
        assert!(request.armed_token.is_some());
        request.validate(NOW + 1).expect("downstream request");
    }

    #[test]
    fn exact_workload_cancellation_is_bound_to_the_selected_operation() {
        let catalog = catalog(HealthStatus::Available);
        let mut invocation = invocation(&catalog);
        invocation.request_id = "cancel-operation-1".into();
        invocation.cancellation_id = "cancel-capability-1".into();
        invocation.cancels_request_id = Some("request-1".into());
        if let TypedAuthorityRequest::Workload(request) = &mut invocation.authority_request {
            request.request_id = invocation.request_id.clone();
            request.action = WorkloadOperationAction::Cancel;
            request.target_request_id = invocation.cancels_request_id.clone();
        }

        assert_eq!(resource_auth_verb(&invocation), "resource-action-cancel");
        let planned = plan(&catalog, &invocation, &signer(), NOW + 1)
            .expect("exact typed cancellation route");
        assert_eq!(planned.topic, WORKLOAD_OPERATION_TOPIC);
        assert_eq!(
            planned.reply_kind,
            Some(DownstreamReplyKind::WorkloadOperation)
        );
        let downstream: WorkloadOperationRequest =
            serde_json::from_str(&planned.body).expect("typed cancellation body");
        assert_eq!(downstream.action, WorkloadOperationAction::Cancel);
        assert_eq!(downstream.request_id, invocation.request_id);
        assert_eq!(
            downstream.target_request_id.as_deref(),
            invocation.cancels_request_id.as_deref()
        );
        assert_eq!(downstream.target_node, "node-a");
        assert_eq!(downstream.expected_generation, 7);
        downstream.validate(NOW + 1).expect("admitted cancellation");
        let token = CloudArmedToken::parse(
            downstream
                .armed_token
                .as_deref()
                .expect("router-owned cancellation token"),
        )
        .expect("typed armed token");
        assert_eq!(token.nonce, invocation.cancellation_id);
        assert_eq!(token.verb, "workload-operation");
        assert_eq!(token.node, "node-a");
        assert_eq!(token.target, "workload:vm-a");
    }

    #[test]
    fn cancellation_substitution_and_implicit_targets_fail_closed() {
        let catalog = catalog(HealthStatus::Available);
        let cancellation = || {
            let mut invocation = invocation(&catalog);
            invocation.request_id = "cancel-operation-1".into();
            invocation.cancellation_id = "cancel-capability-1".into();
            invocation.cancels_request_id = Some("request-1".into());
            if let TypedAuthorityRequest::Workload(request) = &mut invocation.authority_request {
                request.request_id = invocation.request_id.clone();
                request.action = WorkloadOperationAction::Cancel;
                request.target_request_id = invocation.cancels_request_id.clone();
            }
            invocation
        };

        let mut missing_target = cancellation();
        missing_target.cancels_request_id = None;
        assert_eq!(
            plan(&catalog, &missing_target, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut substituted_operation = cancellation();
        if let TypedAuthorityRequest::Workload(request) =
            &mut substituted_operation.authority_request
        {
            request.target_request_id = Some("request-from-other-action".into());
        }
        assert_eq!(
            plan(&catalog, &substituted_operation, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut substituted_action = cancellation();
        substituted_action.action_id = "resume".into();
        assert_eq!(
            plan(&catalog, &substituted_action, &signer(), NOW + 1),
            Err(RefusalCode::CapabilityMismatch)
        );

        let mut substituted_generation = cancellation();
        if let TypedAuthorityRequest::Workload(request) =
            &mut substituted_generation.authority_request
        {
            request.expected_generation = 8;
        }
        assert_eq!(
            plan(&catalog, &substituted_generation, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut ordinary_action = invocation(&catalog);
        ordinary_action.cancels_request_id = Some("request-from-other-action".into());
        assert_eq!(
            plan(&catalog, &ordinary_action, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );
    }

    #[test]
    fn exact_vdi_connect_routes_only_to_session_authority() {
        let catalog = vdi_catalog();
        let invocation = vdi_invocation(&catalog);

        let planned =
            plan(&catalog, &invocation, &signer(), NOW + 1).expect("typed VDI connect route");
        assert_eq!(planned.topic, crate::workers::session_broker::ACTION_TOPIC);
        assert_eq!(planned.verb, "vdi-session-open");
        assert_eq!(planned.node, "node-a");
        assert_eq!(planned.target, "session:request-1");
        assert_eq!(
            planned.reply_kind,
            Some(DownstreamReplyKind::VdiAuthorityCompletion)
        );
        assert_eq!(planned.reply_topic, Some(String::new()));
        let body: serde_json::Value = serde_json::from_str(&planned.body).expect("VDI body");
        assert_eq!(body["op"], "open");
        assert_eq!(body["vm_id"], "desktop-a");
        assert_eq!(body["resource_request_id"], invocation.request_id);
        assert_eq!(body["resource_id"], invocation.resource_id);
        assert_eq!(body["resource_action_id"], invocation.action_id);
        assert_eq!(body["resource_action_verb"], "connect");
        assert_eq!(body["resource_expected_generation"], 7);
        assert_eq!(body["resource_catalog_revision"], catalog.revision);
        assert_eq!(
            body["resource_catalog_content_digest"],
            catalog.computed_content_digest()
        );
        assert!(body["armed_token"].as_str().is_some());
        serde_json::from_value::<SessionRequest>(body).expect("session authority wire");
    }

    #[test]
    fn approval_gated_vdi_requires_and_preserves_the_exact_local_binding() {
        let (catalog, invocation) = approval_gated_vdi();
        let authority_signer = signer();
        let planned = plan(&catalog, &invocation, &authority_signer, NOW + 1)
            .expect("exact locally approved RDP route");
        assert_eq!(planned.topic, crate::workers::session_broker::ACTION_TOPIC);
        assert_eq!(planned.node, "172.20.146.54");

        let mut absent = invocation.clone();
        absent.local_approval = None;
        assert_eq!(
            plan(&catalog, &absent, &signer(), NOW + 1),
            Err(RefusalCode::Unavailable)
        );

        let mut substituted = invocation;
        substituted
            .local_approval
            .as_mut()
            .expect("approval")
            .action_id = "other-action".into();
        assert_eq!(
            plan(&catalog, &substituted, &signer(), NOW + 1),
            Err(RefusalCode::Unavailable)
        );

        let (_, open) = approval_gated_vdi();
        let completion = planned.vdi_completion.as_ref().expect("VDI completion");
        let receipt = vdi_completion_reply(
            &open,
            &planned,
            completion,
            "01OPENMESSAGE",
            &authority_signer,
            NOW + 1,
        )
        .expect("signed Open completion");
        let mut close = open.clone();
        close.request_id = "resource-rdp-close-1".into();
        close.cancellation_id = "cancel-resource-rdp-close-1".into();
        close.cancels_request_id = Some(open.request_id.clone());
        close.issued_at_ms = NOW + 2;
        close.deadline_at_ms = NOW + 20_000;
        close.authority_request = TypedAuthorityRequest::Vdi(StrictSessionRequest::Close {
            id: open.request_id.clone(),
        });
        close.local_approval = None;
        close.vdi_open_receipt = Some(receipt.clone());
        let planned_close = plan_receipt_bound_vdi_close(&close, &authority_signer, NOW + 3)
            .expect("signed-receipt RDP close route without a current catalog");
        assert_eq!(
            planned_close.topic,
            crate::workers::session_broker::ACTION_TOPIC
        );
        assert_eq!(planned_close.verb, "vdi-session-close");
        assert_eq!(planned_close.target, format!("session:{}", open.request_id));
        assert_eq!(planned_close.node, "172.20.146.54");

        let mut unsigned_close = close.clone();
        unsigned_close.vdi_open_receipt = None;
        assert_eq!(
            plan(&catalog, &unsigned_close, &authority_signer, NOW + 3),
            Err(RefusalCode::Unavailable),
            "ordinary external VDI Close must not be weakened"
        );

        let mut wrong_target = close.clone();
        wrong_target.cancels_request_id = Some("other-open".into());
        assert_eq!(
            plan_receipt_bound_vdi_close(&wrong_target, &authority_signer, NOW + 3),
            Err(RefusalCode::StaleCatalog)
        );

        let mut substituted_binding = close.clone();
        substituted_binding.action_id = "other-action".into();
        assert_eq!(
            plan_receipt_bound_vdi_close(&substituted_binding, &authority_signer, NOW + 3),
            Err(RefusalCode::Unauthorized)
        );

        let mut forged_receipt = close;
        forged_receipt
            .vdi_open_receipt
            .as_mut()
            .expect("receipt")
            .authority_signature = "forged".into();
        assert_eq!(
            plan_receipt_bound_vdi_close(&forged_receipt, &authority_signer, NOW + 3),
            Err(RefusalCode::Unauthorized)
        );
    }

    #[test]
    fn exact_vdi_cancellation_routes_a_bound_close_and_signed_completion() {
        let catalog = vdi_catalog();
        let invocation = vdi_cancellation(&catalog);
        assert_eq!(resource_auth_verb(&invocation), "resource-action-cancel");

        let planned =
            plan(&catalog, &invocation, &signer(), NOW + 1).expect("typed VDI cancellation route");
        assert_eq!(planned.topic, crate::workers::session_broker::ACTION_TOPIC);
        assert_eq!(planned.verb, "vdi-session-close");
        assert_eq!(planned.node, "node-a");
        assert_eq!(planned.target, "session:request-1");
        assert_eq!(
            planned.reply_kind,
            Some(DownstreamReplyKind::VdiAuthorityCompletion)
        );
        let body: serde_json::Value = serde_json::from_str(&planned.body).expect("VDI close body");
        assert_eq!(body["op"], "close");
        assert_eq!(body["id"], "request-1");
        assert_eq!(body["resource_request_id"], invocation.request_id);
        assert_eq!(body["resource_cancels_request_id"], "request-1");
        assert_eq!(body["resource_expected_generation"], 7);
        let token = CloudArmedToken::parse(
            body["armed_token"]
                .as_str()
                .expect("router-owned VDI close token"),
        )
        .expect("typed VDI close token");
        assert_eq!(token.nonce, invocation.cancellation_id);
        assert_eq!(token.verb, "vdi-session-close");
        assert_eq!(token.node, "vdi-session");
        assert_eq!(token.target, "session:request-1");
        serde_json::from_value::<SessionRequest>(body).expect("session close authority wire");

        let signer = signer();
        let completion = vdi_completion_reply(
            &invocation,
            &planned,
            planned.vdi_completion.as_ref().expect("completion plan"),
            "01JVDIDOWNSTREAM0000000000",
            &signer,
            NOW + 1,
        )
        .expect("signed VDI completion");
        assert_eq!(completion.request_id, invocation.request_id);
        assert_eq!(completion.session_id, "request-1");
        assert_eq!(completion.serving_peer, "node-a");
        assert_eq!(completion.outcome, VdiCompletionOutcome::DispatchAccepted);
        assert_eq!(completion.binding, reply_binding(&invocation));
        assert!(signer.verify_payload(
            &completion.signing_payload().expect("completion payload"),
            &completion.authority_signature,
        ));

        let mut substituted = completion.clone();
        substituted.binding.expected_generation = 8;
        assert!(!signer.verify_payload(
            &substituted.signing_payload().expect("substituted payload"),
            &substituted.authority_signature,
        ));
        let mut substituted = completion;
        substituted.binding.action_id = "launch".into();
        assert!(!signer.verify_payload(
            &substituted
                .signing_payload()
                .expect("substituted action payload"),
            &substituted.authority_signature,
        ));
    }

    #[test]
    fn vdi_cancellation_rejects_cross_request_target_and_action_substitution() {
        let catalog = vdi_catalog();

        let mut implicit = vdi_cancellation(&catalog);
        implicit.cancels_request_id = None;
        assert_eq!(
            plan(&catalog, &implicit, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut substituted_request = vdi_cancellation(&catalog);
        substituted_request.cancels_request_id = Some("other-session".into());
        assert_eq!(
            plan(&catalog, &substituted_request, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut substituted_close = vdi_cancellation(&catalog);
        substituted_close.authority_request =
            TypedAuthorityRequest::Vdi(StrictSessionRequest::Close {
                id: "other-session".into(),
            });
        assert_eq!(
            plan(&catalog, &substituted_close, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut substituted_action = vdi_cancellation(&catalog);
        substituted_action.action_id = "launch".into();
        assert_eq!(
            plan(&catalog, &substituted_action, &signer(), NOW + 1),
            Err(RefusalCode::CapabilityMismatch)
        );

        let mut self_cancel = vdi_cancellation(&catalog);
        self_cancel.request_id = "request-1".into();
        assert_eq!(
            plan(&catalog, &self_cancel, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );
    }

    #[test]
    fn exact_clipboard_transfer_routes_only_to_negotiated_session_lane() {
        let card = direct_card(ResourceClass::Desktop, "vdi-session/session-a");
        let catalog = catalog(HealthStatus::Available);
        let mut invocation = invocation(&catalog);
        invocation.verb = ResourceActionVerb::Transfer;
        let (lease, message) = clipboard_transfer();

        let planned = plan_clipboard(
            &card,
            &invocation,
            ClipboardDirection::HostToGuest,
            &lease,
            &message,
            NOW + 1,
        )
        .expect("typed clipboard route");
        assert_eq!(
            planned.topic,
            vdi_clipboard_session_topic(
                VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
                &message.session_id,
            )
            .expect("clipboard topic")
        );
        assert_eq!(
            planned.reply_topic,
            Some(
                vdi_clipboard_session_topic(
                    VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX,
                    &message.session_id,
                )
                .expect("receipt topic")
            )
        );
        assert_eq!(
            serde_json::from_str::<VdiClipboardMessageV2>(&planned.body).expect("clipboard body"),
            message
        );
    }

    #[test]
    fn clipboard_cancellation_is_typed_signed_and_explicitly_unsupported() {
        let catalog = clipboard_catalog();
        let card = &catalog.cards[0];
        let invocation = clipboard_cancellation(&catalog);
        let TypedAuthorityRequest::ClipboardCancellation {
            direction,
            session_id,
            generation,
            lease_id,
            message_sequence,
            target_request_id,
        } = &invocation.authority_request
        else {
            panic!("clipboard cancellation fixture");
        };
        assert_eq!(resource_auth_verb(&invocation), "resource-action-cancel");
        assert_eq!(
            plan_clipboard_cancellation(
                card,
                &invocation,
                *direction,
                session_id,
                *generation,
                lease_id,
                *message_sequence,
                target_request_id,
            ),
            Err(RefusalCode::CancellationUnsupported)
        );

        let completion_signer = signer();
        let reply =
            unsupported_clipboard_cancellation_reply(&invocation, &completion_signer, NOW + 1)
                .expect("signed unsupported completion");
        assert!(!reply.accepted);
        assert_eq!(reply.refusal, Some(RefusalCode::CancellationUnsupported));
        assert!(reply.downstream_topic.is_none());
        let completion = reply
            .cancellation_completion
            .expect("typed cancellation completion");
        assert_eq!(
            completion.outcome,
            CancellationCompletionOutcome::UnsupportedCancellation
        );
        assert_eq!(completion.binding, reply_binding(&invocation));
        assert!(completion.downstream_message_id.is_none());
        assert!(completion.downstream_request_digest.is_none());
        assert!(completion_signer.verify_payload(
            &completion.signing_payload().expect("completion payload"),
            &completion.authority_signature,
        ));

        let mut substituted = completion;
        substituted.binding.expected_generation += 1;
        assert!(!completion_signer.verify_payload(
            &substituted
                .signing_payload()
                .expect("substituted completion payload"),
            &substituted.authority_signature,
        ));
    }

    #[test]
    fn clipboard_cancellation_rejects_implicit_cross_transfer_and_target_substitution() {
        let catalog = clipboard_catalog();
        let card = &catalog.cards[0];
        let assert_target_mismatch = |invocation: &ResourceActionInvocation| {
            let TypedAuthorityRequest::ClipboardCancellation {
                direction,
                session_id,
                generation,
                lease_id,
                message_sequence,
                target_request_id,
            } = &invocation.authority_request
            else {
                panic!("clipboard cancellation fixture");
            };
            assert_eq!(
                plan_clipboard_cancellation(
                    card,
                    invocation,
                    *direction,
                    session_id,
                    *generation,
                    lease_id,
                    *message_sequence,
                    target_request_id,
                ),
                Err(RefusalCode::TargetMismatch)
            );
        };

        let mut implicit = clipboard_cancellation(&catalog);
        implicit.cancels_request_id = None;
        assert_target_mismatch(&implicit);

        let mut cross_transfer = clipboard_cancellation(&catalog);
        cross_transfer.cancels_request_id = Some("other-transfer".into());
        assert_target_mismatch(&cross_transfer);

        let mut substituted_session = clipboard_cancellation(&catalog);
        if let TypedAuthorityRequest::ClipboardCancellation { session_id, .. } =
            &mut substituted_session.authority_request
        {
            *session_id = "other-session".into();
        }
        assert_target_mismatch(&substituted_session);

        let mut substituted_generation = clipboard_cancellation(&catalog);
        if let TypedAuthorityRequest::ClipboardCancellation { generation, .. } =
            &mut substituted_generation.authority_request
        {
            *generation += 1;
        }
        assert_target_mismatch(&substituted_generation);

        let mut self_cancel = clipboard_cancellation(&catalog);
        self_cancel.request_id = "transfer-request-1".into();
        assert_target_mismatch(&self_cancel);
    }

    #[test]
    fn exact_android_start_routes_only_to_provider_authority() {
        let app = AospStarterApp::Browser;
        let package = app.package_id().as_str();
        let card = direct_card(
            ResourceClass::Application,
            &format!("android-app/node-a/android-vm-a/{package}"),
        );
        let catalog = catalog(HealthStatus::Available);
        let mut invocation = invocation(&catalog);
        invocation.verb = ResourceActionVerb::Start;
        let request = AndroidLifecycleRequest {
            schema_version: CLOUD_SCHEMA_VERSION,
            node: "node-a".into(),
            workload_id: "android-vm-a".into(),
            request_id: invocation.request_id.clone(),
            expected_generation: invocation.expected_generation,
            operation: AndroidOperation::Start,
            app: Some(app),
            armed_token: None,
            typed_name: None,
        };

        let planned = plan_android(&card, &invocation, request, &signer())
            .expect("typed Android provider route");
        assert_eq!(planned.topic, "action/cloud/android-lifecycle");
        assert_eq!(planned.verb, "android-lifecycle");
        assert_eq!(planned.node, "node-a");
        assert_eq!(planned.target, "android-vm-a");
        let downstream: AndroidLifecycleRequest =
            serde_json::from_str(&planned.body).expect("Android body");
        assert_eq!(downstream.app, Some(app));
        assert!(downstream.armed_token.is_some());
    }

    #[test]
    fn exact_android_cancellation_routes_generation_bound_cancel_with_signed_completion() {
        let catalog = android_catalog();
        let invocation = android_cancellation(&catalog);
        assert_eq!(resource_auth_verb(&invocation), "resource-action-cancel");
        let planned = plan(&catalog, &invocation, &signer(), NOW + 1)
            .expect("typed Android cancellation route");
        assert_eq!(planned.topic, "action/cloud/android-lifecycle");
        let downstream: AndroidLifecycleRequest =
            serde_json::from_str(&planned.body).expect("typed Android cancel body");
        assert_eq!(downstream.operation, AndroidOperation::Cancel);
        assert_eq!(downstream.request_id, invocation.request_id);
        assert_eq!(
            downstream.expected_generation,
            invocation.expected_generation
        );
        assert!(downstream.app.is_none());
        let token = CloudArmedToken::parse(
            downstream
                .armed_token
                .as_deref()
                .expect("router-owned Android cancellation token"),
        )
        .expect("typed Android cancellation token");
        assert_eq!(token.nonce, invocation.cancellation_id);
        assert_eq!(token.verb, "android-lifecycle");
        assert_eq!(token.node, "node-a");
        assert_eq!(token.target, "android-vm-a");

        let completion_signer = signer();
        let completion = cancellation_completion_reply(
            &invocation,
            CancellationAuthority::AndroidLifecycle,
            CancellationCompletionOutcome::DispatchAccepted,
            Some((&planned, "downstream-android-1")),
            planned.verb,
            &planned.node,
            &planned.target,
            &completion_signer,
            NOW + 1,
        )
        .expect("signed Android dispatch completion");
        assert_eq!(completion.binding, reply_binding(&invocation));
        assert_eq!(
            completion.binding.cancels_request_id.as_deref(),
            Some("start-android-request-1")
        );
        assert!(completion_signer.verify_payload(
            &completion.signing_payload().expect("completion payload"),
            &completion.authority_signature,
        ));

        let mut substituted = completion;
        substituted.binding.action_id = "other-action".into();
        assert!(!completion_signer.verify_payload(
            &substituted
                .signing_payload()
                .expect("substituted completion payload"),
            &substituted.authority_signature,
        ));
    }

    #[test]
    fn android_cancellation_rejects_implicit_cross_request_and_authority_substitution() {
        let catalog = android_catalog();

        let mut implicit = android_cancellation(&catalog);
        implicit.cancels_request_id = None;
        assert_eq!(
            plan(&catalog, &implicit, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut self_cancel = android_cancellation(&catalog);
        self_cancel.cancels_request_id = Some(self_cancel.request_id.clone());
        assert_eq!(
            plan(&catalog, &self_cancel, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut substituted_generation = android_cancellation(&catalog);
        if let TypedAuthorityRequest::AndroidProvider(request) =
            &mut substituted_generation.authority_request
        {
            request.expected_generation += 1;
        }
        assert_eq!(
            plan(&catalog, &substituted_generation, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut substituted_workload = android_cancellation(&catalog);
        if let TypedAuthorityRequest::AndroidProvider(request) =
            &mut substituted_workload.authority_request
        {
            request.workload_id = "other-android-vm".into();
        }
        assert_eq!(
            plan(&catalog, &substituted_workload, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );

        let mut smuggled_app = android_cancellation(&catalog);
        if let TypedAuthorityRequest::AndroidProvider(request) = &mut smuggled_app.authority_request
        {
            request.app = Some(AospStarterApp::Browser);
        }
        assert_eq!(
            plan(&catalog, &smuggled_app, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );
    }

    #[test]
    fn persisted_clipboard_unsupported_completion_is_signed_and_replay_safe() {
        let catalog = clipboard_catalog();
        let invocation = clipboard_cancellation(&catalog);
        let (_bus, _secrets, persist, mut worker) =
            persisted_worker_fixture(&catalog, "action-auth-clipboard-cancel");
        let signed = signed_ingress(&invocation, "resource-clipboard-cancel-once");
        let ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish signed clipboard cancellation");

        worker.tick(NOW + 1);
        let downstream_topic =
            vdi_clipboard_session_topic(VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX, "session-a")
                .expect("fixed clipboard topic");
        assert!(persist
            .list_since(&downstream_topic, None)
            .expect("clipboard downstream rows")
            .is_empty());
        let reply_rows = persist
            .list_since(&mde_bus::rpc::reply_topic(&ingress.ulid), None)
            .expect("clipboard cancellation reply");
        assert_eq!(reply_rows.len(), 1);
        let reply: serde_json::Value = serde_json::from_str(
            reply_rows[0]
                .body
                .as_deref()
                .expect("clipboard cancellation reply body"),
        )
        .expect("typed clipboard cancellation reply");
        assert_eq!(reply["accepted"], false);
        assert_eq!(reply["refusal"], "cancellation_unsupported");
        assert!(reply["downstream_topic"].is_null());
        let completion: CancellationAuthorityCompletionReply =
            serde_json::from_value(reply["cancellation_completion"].clone())
                .expect("typed unsupported completion");
        assert_eq!(completion.request_id, invocation.request_id);
        assert_eq!(completion.binding, reply_binding(&invocation));
        assert_eq!(
            completion.outcome,
            CancellationCompletionOutcome::UnsupportedCancellation
        );
        assert!(signer().verify_payload(
            &completion.signing_payload().expect("completion payload"),
            &completion.authority_signature,
        ));

        let replay_ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish replayed clipboard cancellation");
        worker.tick(NOW + 2);
        assert!(persist
            .list_since(&downstream_topic, None)
            .expect("clipboard replay downstream rows")
            .is_empty());
        let replay_rows = persist
            .list_since(&mde_bus::rpc::reply_topic(&replay_ingress.ulid), None)
            .expect("clipboard replay refusal");
        assert_eq!(replay_rows.len(), 1);
        let replay: serde_json::Value = serde_json::from_str(
            replay_rows[0]
                .body
                .as_deref()
                .expect("clipboard replay refusal body"),
        )
        .expect("typed clipboard replay refusal");
        assert_eq!(replay["accepted"], false);
        assert_eq!(replay["refusal"], "unauthorized");
        assert!(replay["cancellation_completion"].is_null());
    }

    #[test]
    fn persisted_android_cancel_completion_is_correlated_signed_and_replay_safe() {
        let catalog = android_catalog();
        let invocation = android_cancellation(&catalog);
        let (_bus, _secrets, persist, mut worker) =
            persisted_worker_fixture(&catalog, "action-auth-android-cancel");
        let signed = signed_ingress(&invocation, "resource-android-cancel-once");
        let ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish signed Android cancellation");

        worker.tick(NOW + 1);
        let downstream = persist
            .list_since("action/cloud/android-lifecycle", None)
            .expect("Android cancellation downstream rows");
        assert_eq!(downstream.len(), 1);
        let downstream_body = downstream[0]
            .body
            .as_deref()
            .expect("Android cancellation body");
        let request: AndroidLifecycleRequest =
            serde_json::from_str(downstream_body).expect("typed Android cancellation");
        assert_eq!(request.operation, AndroidOperation::Cancel);
        assert_eq!(request.expected_generation, invocation.expected_generation);

        let reply_rows = persist
            .list_since(&mde_bus::rpc::reply_topic(&ingress.ulid), None)
            .expect("Android cancellation immediate reply");
        assert_eq!(reply_rows.len(), 1);
        let reply: serde_json::Value = serde_json::from_str(
            reply_rows[0]
                .body
                .as_deref()
                .expect("Android cancellation reply body"),
        )
        .expect("typed Android cancellation reply");
        assert_eq!(reply["accepted"], true);
        let completion: CancellationAuthorityCompletionReply =
            serde_json::from_value(reply["cancellation_completion"].clone())
                .expect("typed Android dispatch completion");
        assert_eq!(completion.binding, reply_binding(&invocation));
        assert_eq!(
            completion.downstream_message_id.as_deref(),
            Some(downstream[0].ulid.as_str())
        );
        let expected_downstream_digest =
            cloud_request_digest(downstream_body).expect("Android downstream digest");
        assert_eq!(
            completion.downstream_request_digest.as_deref(),
            Some(expected_downstream_digest.as_str())
        );
        assert!(signer().verify_payload(
            &completion.signing_payload().expect("completion payload"),
            &completion.authority_signature,
        ));

        let replay_ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish replayed Android cancellation");
        worker.tick(NOW + 2);
        assert_eq!(
            persist
                .list_since("action/cloud/android-lifecycle", None)
                .expect("Android replay downstream rows")
                .len(),
            1
        );
        let replay_rows = persist
            .list_since(&mde_bus::rpc::reply_topic(&replay_ingress.ulid), None)
            .expect("Android replay refusal");
        assert_eq!(replay_rows.len(), 1);
        let replay: serde_json::Value = serde_json::from_str(
            replay_rows[0]
                .body
                .as_deref()
                .expect("Android replay refusal body"),
        )
        .expect("typed Android replay refusal");
        assert_eq!(replay["refusal"], "unauthorized");
        assert!(replay["cancellation_completion"].is_null());
    }

    #[test]
    fn persisted_bus_signed_ingress_attestation_reply_and_cursor_are_idempotent() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let secrets = tempfile::tempdir().expect("secret tempdir");
        let store = local_publisher_store(secrets.path());
        store
            .put(
                super::super::RESOURCE_PUBLISHER_KEY_REF,
                std::str::from_utf8(PUBLISHER_KEY).expect("publisher key text"),
            )
            .expect("store publisher key");
        let catalog = catalog(HealthStatus::Available);
        let attestation = ResourcePublisherAttestation::mint(
            &catalog,
            RESOURCE_PUBLISHER_ATTESTATION_KEY_ID,
            PUBLISHER_KEY,
            NOW,
            NOW + 30_000,
        )
        .expect("publisher attestation");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persisted Bus");
        persist
            .write(
                RESOURCE_CATALOG_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&catalog).expect("catalog JSON")),
            )
            .expect("publish catalog");
        persist
            .write(
                &resource_publisher_attestation_topic(&catalog.publisher),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&attestation).expect("attestation JSON")),
            )
            .expect("publish attestation");

        let invocation = invocation(&catalog);
        let unsigned = serde_json::to_string(&invocation).expect("unsigned invocation");
        let target = format!("{}:{}", invocation.resource_id, invocation.action_id);
        let signed = authorize_test_body(
            ACTION_KEY,
            &unsigned,
            MutationContext {
                verb: resource_auth_verb(&invocation),
                node: "resource-authority",
                target: &target,
            },
            "resource-ingress-once",
            i64::try_from(invocation.deadline_at_ms).expect("deadline"),
        );
        let ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish signed invocation");

        let mut worker = ResourceActionWorker {
            bus_root: Some(bus.path().to_path_buf()),
            cursor: None,
            authorizer: Arc::new(ActionAuthorizer::for_test(
                ACTION_KEY,
                bus.path().join("action-auth"),
                i64::try_from(NOW + 1).expect("test time"),
            )),
            signer: Some(signer()),
            publisher_store: store,
        };
        worker.tick(NOW + 1);

        let downstream = persist
            .list_since(WORKLOAD_OPERATION_TOPIC, None)
            .expect("downstream rows");
        assert_eq!(downstream.len(), 1);
        let request: WorkloadOperationRequest =
            serde_json::from_str(downstream[0].body.as_deref().expect("downstream body"))
                .expect("typed workload request");
        assert!(request.armed_token.is_some());
        let replies = persist
            .list_since(&mde_bus::rpc::reply_topic(&ingress.ulid), None)
            .expect("correlated reply rows");
        assert_eq!(replies.len(), 1);
        let reply: serde_json::Value =
            serde_json::from_str(replies[0].body.as_deref().expect("reply body"))
                .expect("typed immediate reply");
        assert_eq!(reply["accepted"], true);
        assert_eq!(reply["request_id"], invocation.request_id);
        assert_eq!(reply["downstream_topic"], WORKLOAD_OPERATION_TOPIC);
        assert_eq!(reply["downstream_reply_kind"], "workload_operation");
        assert_eq!(reply["binding"]["resource_id"], invocation.resource_id);
        assert_eq!(reply["binding"]["action_id"], invocation.action_id);
        assert_eq!(reply["binding"]["verb"], "start");
        assert_eq!(
            reply["binding"]["target"],
            serde_json::to_value(&invocation.target).expect("typed resource target")
        );
        assert_eq!(reply["binding"]["expected_generation"], 7);
        assert_eq!(
            reply["binding"]["cancellation_id"],
            invocation.cancellation_id
        );
        assert_eq!(
            reply["binding"]["cancels_request_id"],
            serde_json::Value::Null
        );
        assert_eq!(
            reply["downstream_reply_topic"],
            mde_bus::rpc::reply_topic(&downstream[0].ulid)
        );

        worker.tick(NOW + 2);
        assert_eq!(
            persist
                .list_since(WORKLOAD_OPERATION_TOPIC, None)
                .expect("downstream replay check")
                .len(),
            1
        );

        let replay_ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish replayed signed invocation");
        worker.tick(NOW + 3);
        assert_eq!(
            persist
                .list_since(WORKLOAD_OPERATION_TOPIC, None)
                .expect("replayed authorization downstream check")
                .len(),
            1
        );
        let replay_replies = persist
            .list_since(&mde_bus::rpc::reply_topic(&replay_ingress.ulid), None)
            .expect("replayed authorization reply");
        assert_eq!(replay_replies.len(), 1);
        let replay_reply: serde_json::Value = serde_json::from_str(
            replay_replies[0]
                .body
                .as_deref()
                .expect("replayed authorization reply body"),
        )
        .expect("typed replay refusal");
        assert_eq!(replay_reply["accepted"], false);
        assert_eq!(replay_reply["refusal"], "unauthorized");
        assert_eq!(replay_reply["binding"], serde_json::Value::Null);
        assert_eq!(
            persist
                .list_since(&mde_bus::rpc::reply_topic(&ingress.ulid), None)
                .expect("reply replay check")
                .len(),
            1
        );
    }

    #[test]
    fn persisted_vdi_cancel_completion_is_correlated_signed_and_replay_safe() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let secrets = tempfile::tempdir().expect("secret tempdir");
        let store = local_publisher_store(secrets.path());
        store
            .put(
                super::super::RESOURCE_PUBLISHER_KEY_REF,
                std::str::from_utf8(PUBLISHER_KEY).expect("publisher key text"),
            )
            .expect("store publisher key");
        let catalog = vdi_catalog();
        let attestation = ResourcePublisherAttestation::mint(
            &catalog,
            RESOURCE_PUBLISHER_ATTESTATION_KEY_ID,
            PUBLISHER_KEY,
            NOW,
            NOW + 30_000,
        )
        .expect("publisher attestation");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persisted Bus");
        persist
            .write(
                RESOURCE_CATALOG_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&catalog).expect("catalog JSON")),
            )
            .expect("publish catalog");
        persist
            .write(
                &resource_publisher_attestation_topic(&catalog.publisher),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&attestation).expect("attestation JSON")),
            )
            .expect("publish attestation");

        let invocation = vdi_cancellation(&catalog);
        let unsigned = serde_json::to_string(&invocation).expect("unsigned cancellation");
        let target = format!("{}:{}", invocation.resource_id, invocation.action_id);
        let signed = authorize_test_body(
            ACTION_KEY,
            &unsigned,
            MutationContext {
                verb: resource_auth_verb(&invocation),
                node: "resource-authority",
                target: &target,
            },
            "resource-vdi-cancel-once",
            i64::try_from(invocation.deadline_at_ms).expect("deadline"),
        );
        let ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish signed VDI cancellation");

        let completion_signer = signer();
        let mut worker = ResourceActionWorker {
            bus_root: Some(bus.path().to_path_buf()),
            cursor: None,
            authorizer: Arc::new(ActionAuthorizer::for_test(
                ACTION_KEY,
                bus.path().join("action-auth-vdi"),
                i64::try_from(NOW + 1).expect("test time"),
            )),
            signer: Some(completion_signer.clone()),
            publisher_store: store,
        };
        worker.tick(NOW + 1);

        let downstream = persist
            .list_since(crate::workers::session_broker::ACTION_TOPIC, None)
            .expect("VDI downstream rows");
        assert_eq!(downstream.len(), 1);
        let downstream_body = downstream[0].body.as_deref().expect("VDI downstream body");
        let downstream_value: serde_json::Value =
            serde_json::from_str(downstream_body).expect("typed VDI close body");
        assert_eq!(downstream_value["op"], "close");
        assert_eq!(downstream_value["id"], "request-1");
        assert_eq!(
            downstream_value["resource_request_id"],
            invocation.request_id
        );
        assert_eq!(
            downstream_value["resource_cancels_request_id"],
            invocation
                .cancels_request_id
                .as_deref()
                .expect("cancel target")
        );

        let completion_topic = mde_bus::rpc::reply_topic(&downstream[0].ulid);
        let completion_rows = persist
            .list_since(&completion_topic, None)
            .expect("VDI completion rows");
        assert_eq!(completion_rows.len(), 1);
        let completion: VdiAuthorityCompletionReply = serde_json::from_str(
            completion_rows[0]
                .body
                .as_deref()
                .expect("VDI completion body"),
        )
        .expect("typed VDI authority completion");
        assert_eq!(completion.request_id, invocation.request_id);
        assert_eq!(completion.session_id, "request-1");
        assert_eq!(completion.serving_peer, "node-a");
        assert_eq!(completion.downstream_message_id, downstream[0].ulid);
        assert_eq!(
            completion.downstream_request_digest,
            cloud_request_digest(downstream_body).expect("downstream request digest")
        );
        assert_eq!(completion.binding, reply_binding(&invocation));
        assert!(completion_signer.verify_payload(
            &completion
                .signing_payload()
                .expect("completion signing payload"),
            &completion.authority_signature,
        ));

        let immediate_rows = persist
            .list_since(&mde_bus::rpc::reply_topic(&ingress.ulid), None)
            .expect("immediate VDI reply");
        assert_eq!(immediate_rows.len(), 1);
        let immediate: serde_json::Value = serde_json::from_str(
            immediate_rows[0]
                .body
                .as_deref()
                .expect("immediate VDI reply body"),
        )
        .expect("typed immediate VDI reply");
        assert_eq!(immediate["accepted"], true);
        assert_eq!(immediate["downstream_reply_topic"], completion_topic);
        assert_eq!(
            immediate["downstream_reply_kind"],
            "vdi_authority_completion"
        );
        assert_eq!(immediate["binding"]["cancels_request_id"], "request-1");
        assert_eq!(immediate["binding"]["expected_generation"], 7);

        let replay_ingress = persist
            .write(
                RESOURCE_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )
            .expect("publish replayed VDI cancellation");
        worker.tick(NOW + 2);
        assert_eq!(
            persist
                .list_since(crate::workers::session_broker::ACTION_TOPIC, None)
                .expect("VDI replay downstream check")
                .len(),
            1
        );
        assert_eq!(
            persist
                .list_since(&completion_topic, None)
                .expect("VDI replay completion check")
                .len(),
            1
        );
        let replay_rows = persist
            .list_since(&mde_bus::rpc::reply_topic(&replay_ingress.ulid), None)
            .expect("VDI replay refusal");
        assert_eq!(replay_rows.len(), 1);
        let replay: serde_json::Value = serde_json::from_str(
            replay_rows[0]
                .body
                .as_deref()
                .expect("VDI replay refusal body"),
        )
        .expect("typed VDI replay refusal");
        assert_eq!(replay["accepted"], false);
        assert_eq!(replay["refusal"], "unauthorized");
        assert_eq!(replay["binding"], serde_json::Value::Null);
    }

    #[test]
    fn launch_and_resume_map_only_to_closed_workload_operations() {
        for (verb, operation, action_id) in [
            (
                ResourceActionVerb::Launch,
                WorkloadOperationAction::StartAndAttach,
                "launch",
            ),
            (
                ResourceActionVerb::Resume,
                WorkloadOperationAction::Resume,
                "resume",
            ),
        ] {
            let mut catalog = catalog(HealthStatus::Available);
            catalog.cards[0].actions[0].verb = verb;
            catalog.cards[0].actions[0].action_id = action_id.into();
            catalog.content_digest = None;
            catalog.content_digest = Some(catalog.computed_content_digest());
            catalog.validate().expect("catalog");

            let mut invocation = invocation(&catalog);
            invocation.action_id = action_id.into();
            invocation.verb = verb;
            if let TypedAuthorityRequest::Workload(request) = &mut invocation.authority_request {
                request.action = operation;
            }
            let planned =
                plan(&catalog, &invocation, &signer(), NOW + 1).expect("closed workload route");
            let request: WorkloadOperationRequest =
                serde_json::from_str(&planned.body).expect("typed workload body");
            assert_eq!(request.action, operation);
        }
    }

    #[test]
    fn executable_resource_verbs_have_distinct_authorization_contexts() {
        let catalog = catalog(HealthStatus::Available);
        for (verb, expected) in [
            (ResourceActionVerb::Connect, "resource-action-connect"),
            (ResourceActionVerb::Launch, "resource-action-launch"),
            (ResourceActionVerb::Start, "resource-action-start"),
            (ResourceActionVerb::Resume, "resource-action-resume"),
            (ResourceActionVerb::Transfer, "resource-action-transfer"),
        ] {
            let mut invocation = invocation(&catalog);
            invocation.verb = verb;
            assert_eq!(resource_auth_verb(&invocation), expected);
        }
    }

    #[test]
    fn stale_card_and_unavailable_card_fail_closed() {
        let unavailable = catalog(HealthStatus::Unavailable);
        assert_eq!(
            plan(&unavailable, &invocation(&unavailable), &signer(), NOW + 1),
            Err(RefusalCode::Unavailable)
        );
        let current = catalog(HealthStatus::Available);
        assert_eq!(
            plan(&current, &invocation(&current), &signer(), NOW + 60_001),
            Err(RefusalCode::StaleCatalog)
        );
    }

    #[test]
    fn catalog_revision_action_and_target_substitution_are_rejected() {
        let catalog = catalog(HealthStatus::Available);
        let mut stale = invocation(&catalog);
        stale.catalog_revision = "revision-8".into();
        assert_eq!(
            plan(&catalog, &stale, &signer(), NOW + 1),
            Err(RefusalCode::StaleCatalog)
        );

        let mut action = invocation(&catalog);
        action.action_id = "resume".into();
        assert_eq!(
            plan(&catalog, &action, &signer(), NOW + 1),
            Err(RefusalCode::CapabilityMismatch)
        );

        let mut target = invocation(&catalog);
        if let TypedAuthorityRequest::Workload(request) = &mut target.authority_request {
            request.target_node = "node-b".into();
        }
        assert_eq!(
            plan(&catalog, &target, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );
    }

    #[test]
    fn capability_mismatch_and_caller_supplied_downstream_token_are_rejected() {
        let catalog = catalog(HealthStatus::Available);
        let mut wrong_verb = invocation(&catalog);
        if let TypedAuthorityRequest::Workload(request) = &mut wrong_verb.authority_request {
            request.action = WorkloadOperationAction::Destroy;
        }
        assert_eq!(
            plan(&catalog, &wrong_verb, &signer(), NOW + 1),
            Err(RefusalCode::CapabilityMismatch)
        );

        let mut armed = invocation(&catalog);
        if let TypedAuthorityRequest::Workload(request) = &mut armed.authority_request {
            request.armed_token = Some("caller-token".into());
        }
        assert_eq!(
            plan(&catalog, &armed, &signer(), NOW + 1),
            Err(RefusalCode::TargetMismatch)
        );
    }

    #[test]
    fn raw_command_path_url_and_topic_fields_are_not_in_the_wire_contract() {
        let catalog = catalog(HealthStatus::Available);
        let value = serde_json::to_value(invocation(&catalog)).expect("wire");
        for forbidden in ["command", "path", "url", "topic", "executable"] {
            let mut hostile = value.clone();
            hostile
                .as_object_mut()
                .expect("object")
                .insert(forbidden.into(), serde_json::Value::String("evil".into()));
            assert!(serde_json::from_value::<ResourceActionInvocation>(hostile).is_err());
        }
    }
}

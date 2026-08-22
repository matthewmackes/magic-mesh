//! WL-FUNC-011 media verifier seam.
//!
//! This module deliberately consumes the retained
//! `state/collab/call-media-readiness` board before any media provider is
//! touched. Readiness is signed collaboration state; this verifier board is the
//! separate worker-owned live-proof state. The provider registry defaults empty
//! and reports honest unavailable rows, so a one-seat or no-provider test can
//! never be mistaken for SIP/WebRTC/LiveKit media success. A registered provider
//! may claim success only by returning observed advancing frame/data deltas for
//! the ready session.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_collab_types::topics::{self, projection as proj};
use mde_collab_types::{
    ActorId, CallId, CallKind, CallMediaAdapter, CallMediaAdmission, CallMediaFrameEvidence,
    CallMediaReadiness, CallMediaRequirement, CallMediaSession, CallMediaVerification,
    CallMediaVerificationRow, CallMediaVerificationStatus, CollabCommand, SipLegDirectionV1,
    SipLegV1,
};
use mde_voice_hud::sip::{AgentCommand, AgentEvent, RegistrationState, SipAccount};

const MAX_READINESS_BODY_BYTES: usize = 256 * 1024;
const MAX_VERIFICATION_SESSIONS: usize = 256;
const MAX_VERIFICATION_ROWS: usize = 1024;
const MAX_DETAIL_BYTES: usize = 512;
// Private cache marker for a retained verification tombstone. This is not a
// Bus payload and cannot collide with the serialized verification object.
const VERIFICATION_UNAVAILABLE: &str = "\0call-media-verification-unavailable";
const SIP_COMMAND_CAPACITY: usize = 16;
const SIP_COMMAND_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);
const MAX_SIP_DIAL_TARGET_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SipProviderHealth {
    Starting,
    Ready,
    Unavailable(String),
}

/// Concrete adapter over the already-shipped SIP/RTP voice core.
///
/// The provider starts only when a governed SIP account exists.  The voice
/// core owns registration, inbound INVITE handling, RTP/G.711, PipeWire/ALSA,
/// mute state, and RFC 4733 DTMF.  This adapter deliberately exposes only the
/// commands that core can acknowledge through its bounded agent queue. An
/// outbound Collaboration call must use the explicit bounded dial command;
/// the space-scoped start command continues to fail closed without a target.
struct SipGatewayProvider {
    commands: std::sync::mpsc::SyncSender<AgentCommand>,
    health: Arc<Mutex<SipProviderHealth>>,
    active_call: Arc<Mutex<Option<mde_collab_types::CallId>>>,
    revoked_calls: Arc<Mutex<Vec<mde_collab_types::CallId>>>,
    pending_inbound: Arc<Mutex<Option<InboundCallOffer>>>,
    /// Bound inbound dialogs waiting for a `/sip` tick. The agent adapter
    /// owns this slot, so the outbound-only publish plane will not mint them.
    inbound_legs: Arc<Mutex<BTreeMap<CallId, String>>>,
    publish_plane: super::call_media::SipGatewayPlane,
}

/// Provider-observed inbound dialog identity.  This is an untrusted offer, not
/// call authority; the Collaboration worker must resolve it against signed
/// membership and bind its exact provider dialog before authoring a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InboundCallOffer {
    pub(super) identity: String,
    pub(super) provider_call_id: String,
}

impl SipGatewayProvider {
    fn activate() -> Result<Self, String> {
        let accounts = SipAccount::load_accounts()
            .ok_or_else(|| "no governed SIP account is installed".to_string())?;
        let (command_tx, command_rx) = std::sync::mpsc::sync_channel(SIP_COMMAND_CAPACITY);
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let health = Arc::new(Mutex::new(SipProviderHealth::Starting));
        let monitor_health = Arc::clone(&health);
        let active_call = Arc::new(Mutex::new(None));
        let monitor_active_call = Arc::clone(&active_call);
        let revoked_calls = Arc::new(Mutex::new(Vec::new()));
        let monitor_revoked_calls = Arc::clone(&revoked_calls);
        let pending_inbound = Arc::new(Mutex::new(None));
        let monitor_pending_inbound = Arc::clone(&pending_inbound);
        let inbound_legs = Arc::new(Mutex::new(BTreeMap::new()));
        let monitor_inbound_legs = Arc::clone(&inbound_legs);
        std::thread::Builder::new()
            .name("mcnf-collab-sip-agent".to_string())
            .spawn(move || {
                mde_voice_hud::sip::run_agent_accounts(&accounts, &event_tx, &command_rx);
            })
            .map_err(|error| format!("start SIP agent: {error}"))?;
        std::thread::Builder::new()
            .name("mcnf-collab-sip-health".to_string())
            .spawn(move || {
                while let Ok(event) = event_rx.recv() {
                    let next = match event {
                        AgentEvent::Registration(RegistrationState::Registered { .. }) => {
                            SipProviderHealth::Ready
                        }
                        AgentEvent::Registration(RegistrationState::Failed(detail)) => {
                            SipProviderHealth::Unavailable(bounded_health_detail(&detail))
                        }
                        AgentEvent::Registration(_) => SipProviderHealth::Starting,
                        AgentEvent::Incoming { from, call_id } => {
                            if let Ok(mut pending) = monitor_pending_inbound.lock() {
                                *pending = Some(InboundCallOffer {
                                    identity: from,
                                    provider_call_id: call_id,
                                });
                            }
                            continue;
                        }
                        AgentEvent::Established => continue,
                        AgentEvent::RemoteHangup => {
                            if let Ok(mut pending) = monitor_pending_inbound.lock() {
                                pending.take();
                            }
                            forget_revoked_inbound_leg(
                                &monitor_active_call,
                                &monitor_revoked_calls,
                                &monitor_inbound_legs,
                            );
                            continue;
                        }
                    };
                    if matches!(next, SipProviderHealth::Unavailable(_)) {
                        if let Ok(mut pending) = monitor_pending_inbound.lock() {
                            pending.take();
                        }
                        forget_revoked_inbound_leg(
                            &monitor_active_call,
                            &monitor_revoked_calls,
                            &monitor_inbound_legs,
                        );
                    }
                    if let Ok(mut current) = monitor_health.lock() {
                        *current = next;
                    }
                }
                if let Ok(mut pending) = monitor_pending_inbound.lock() {
                    pending.take();
                }
                forget_revoked_inbound_leg(
                    &monitor_active_call,
                    &monitor_revoked_calls,
                    &monitor_inbound_legs,
                );
                if let Ok(mut current) = monitor_health.lock() {
                    *current = SipProviderHealth::Unavailable("SIP agent stopped".to_string());
                }
            })
            .map_err(|error| format!("start SIP health monitor: {error}"))?;
        Ok(Self {
            commands: command_tx,
            health,
            active_call,
            revoked_calls,
            pending_inbound,
            inbound_legs,
            publish_plane: super::call_media::SipGatewayPlane::production(),
        })
    }

    #[cfg(test)]
    fn with_channel(
        commands: std::sync::mpsc::SyncSender<AgentCommand>,
        health: SipProviderHealth,
    ) -> Self {
        Self {
            commands,
            health: Arc::new(Mutex::new(health)),
            active_call: Arc::new(Mutex::new(None)),
            revoked_calls: Arc::new(Mutex::new(Vec::new())),
            pending_inbound: Arc::new(Mutex::new(None)),
            inbound_legs: Arc::new(Mutex::new(BTreeMap::new())),
            publish_plane: super::call_media::SipGatewayPlane::production(),
        }
    }

    fn require_ready(&self) -> Result<(), CallMediaProviderError> {
        let health =
            self.health
                .lock()
                .map_err(|_| CallMediaProviderError::ProviderUnavailable {
                    detail: "SIP provider health lock is unavailable".to_string(),
                })?;
        match &*health {
            SipProviderHealth::Ready => Ok(()),
            SipProviderHealth::Starting => Err(CallMediaProviderError::ProviderUnavailable {
                detail: "SIP provider registration is not ready".to_string(),
            }),
            SipProviderHealth::Unavailable(detail) => {
                Err(CallMediaProviderError::ProviderUnavailable {
                    detail: detail.clone(),
                })
            }
        }
    }

    fn send_acknowledged<T>(
        &self,
        build: impl FnOnce(std::sync::mpsc::SyncSender<Result<T, String>>) -> AgentCommand,
    ) -> Result<T, CallMediaProviderError> {
        let (completion_tx, completion_rx) = std::sync::mpsc::sync_channel(1);
        self.commands
            .try_send(build(completion_tx))
            .map_err(|error| CallMediaProviderError::ProviderUnavailable {
                detail: match error {
                    std::sync::mpsc::TrySendError::Full(_) => {
                        "SIP provider command queue is full".to_string()
                    }
                    std::sync::mpsc::TrySendError::Disconnected(_) => {
                        "SIP provider command queue is disconnected".to_string()
                    }
                },
            })?;
        completion_rx
            .recv_timeout(SIP_COMMAND_ACK_TIMEOUT)
            .map_err(|error| CallMediaProviderError::ProviderUnavailable {
                detail: format!("SIP provider acknowledgement unavailable: {error}"),
            })?
            .map_err(|detail| CallMediaProviderError::ExecutionRefused {
                detail: bounded_health_detail(&detail),
            })
    }

    fn gateway_is_ready(&self) -> bool {
        self.health
            .lock()
            .map(|health| matches!(*health, SipProviderHealth::Ready))
            .unwrap_or(false)
    }

    fn retain_inbound_leg(&self, call: CallId, offer: &InboundCallOffer) {
        let Some(e164) = inbound_offer_e164(&offer.identity) else {
            return;
        };
        if let Ok(mut legs) = self.inbound_legs.lock() {
            legs.insert(call, e164);
        }
    }

    fn forget_inbound_leg(&self, call: CallId) {
        if let Ok(mut legs) = self.inbound_legs.lock() {
            legs.remove(&call);
        }
    }

    fn publish_inbound_sip_legs(
        &self,
        persist: &Persist,
        last_published: &mut BTreeMap<String, String>,
    ) {
        let Ok(legs) = self.inbound_legs.lock() else {
            return;
        };
        if legs.is_empty() {
            return;
        }
        let Some(readiness) = read_retained_readiness(persist) else {
            return;
        };
        let gateway_available = self.gateway_is_ready();
        for (call, e164) in legs.iter() {
            let Some(session) = readiness
                .sessions
                .iter()
                .find(|session| session.call == *call)
            else {
                continue;
            };
            if !session
                .connected_participants
                .iter()
                .any(|actor| actor == &readiness.local_actor)
            {
                continue;
            }
            let Ok(document) = SipLegV1::new(
                *call,
                readiness.local_actor.clone(),
                SipLegDirectionV1::Inbound,
                e164,
                gateway_available,
                false,
            ) else {
                continue;
            };
            if document.bridged || document.direction != SipLegDirectionV1::Inbound {
                continue;
            }
            let topic = document.topic();
            let Ok(body) = serde_json::to_string(&document) else {
                continue;
            };
            if last_published.get(&topic).map(String::as_str) == Some(body.as_str()) {
                continue;
            }
            if persist
                .write(&topic, Priority::Default, None, Some(&body))
                .is_err()
            {
                continue;
            }
            last_published.insert(topic, body);
        }
    }
}

fn inbound_offer_e164(identity: &str) -> Option<String> {
    let candidate = identity.trim();
    if candidate.is_empty() {
        return None;
    }
    // Fail closed on SIP URIs, display names, and non-E.164 identities.
    // Never invent a PSTN number for an inbound offer.
    SipLegV1::new(
        CallId::new(),
        ActorId::new("local"),
        SipLegDirectionV1::Inbound,
        candidate,
        false,
        false,
    )
    .ok()
    .map(|document| document.e164)
}

fn read_retained_readiness(persist: &Persist) -> Option<CallMediaReadiness> {
    let topic = topics::state_topic(proj::CALL_MEDIA_READINESS);
    let msg = persist.read_latest(&topic).ok()??;
    let body = msg.body.as_deref()?;
    if body.len() > MAX_READINESS_BODY_BYTES {
        return None;
    }
    serde_json::from_str(body).ok()
}

fn revoke_active_call(
    active_call: &Mutex<Option<mde_collab_types::CallId>>,
    revoked_calls: &Mutex<Vec<mde_collab_types::CallId>>,
) -> Option<CallId> {
    let call = active_call.lock().ok().and_then(|mut active| active.take());
    if let Some(call) = call {
        if let Ok(mut revoked) = revoked_calls.lock() {
            if !revoked.contains(&call) {
                revoked.push(call);
            }
        }
    }
    call
}

fn forget_revoked_inbound_leg(
    active_call: &Mutex<Option<mde_collab_types::CallId>>,
    revoked_calls: &Mutex<Vec<mde_collab_types::CallId>>,
    inbound_legs: &Mutex<BTreeMap<CallId, String>>,
) {
    if let Some(call) = revoke_active_call(active_call, revoked_calls) {
        if let Ok(mut legs) = inbound_legs.lock() {
            legs.remove(&call);
        }
    }
}

fn bounded_health_detail(detail: &str) -> String {
    let mut end = detail.len().min(MAX_DETAIL_BYTES);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_string()
}

fn validate_sip_dial_target(target: &str) -> Result<(), CallMediaProviderError> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(CallMediaProviderError::ExecutionRefused {
            detail: "SIP outbound target must not be empty".to_string(),
        });
    }
    if trimmed.len() > MAX_SIP_DIAL_TARGET_BYTES {
        return Err(CallMediaProviderError::ExecutionRefused {
            detail: format!("SIP outbound target exceeds {MAX_SIP_DIAL_TARGET_BYTES}-byte limit"),
        });
    }
    if trimmed.chars().any(char::is_control) {
        return Err(CallMediaProviderError::ExecutionRefused {
            detail: "SIP outbound target contains a control character".to_string(),
        });
    }
    Ok(())
}

impl CallMediaFrameVerifier for SipGatewayProvider {
    fn execute_command(
        &self,
        command: &CollabCommand,
        adapter: CallMediaAdapter,
    ) -> Result<(), CallMediaProviderError> {
        if adapter != CallMediaAdapter::SipGateway {
            return Err(CallMediaProviderError::ExecutionRefused {
                detail: "SIP provider was selected for an incompatible adapter".to_string(),
            });
        }
        let cleanup = matches!(
            command,
            CollabCommand::DeclineCall { .. } | CollabCommand::HangUpCall { .. }
        );
        if let Err(error) = self.require_ready() {
            // Provider loss must never trap the signed collaboration call in
            // an active state. Cleanup remains locally authoritative and the
            // dead adapter receives no command.
            if cleanup {
                if let CollabCommand::DeclineCall { call } | CollabCommand::HangUpCall { call } =
                    command
                {
                    self.forget_inbound_leg(*call);
                }
                let _ = self.publish_plane.execute_command(command, adapter);
                return Ok(());
            }
            return Err(error);
        }
        let agent_command = match command {
            CollabCommand::AnswerCall { call } => {
                self.commands
                    .try_send(AgentCommand::Answer)
                    .map_err(|error| CallMediaProviderError::ProviderUnavailable {
                        detail: match error {
                            std::sync::mpsc::TrySendError::Full(_) => {
                                "SIP provider command queue is full".to_string()
                            }
                            std::sync::mpsc::TrySendError::Disconnected(_) => {
                                "SIP provider command queue is disconnected".to_string()
                            }
                        },
                    })?;
                if let Ok(mut active) = self.active_call.lock() {
                    *active = Some(*call);
                }
                return Ok(());
            }
            CollabCommand::DeclineCall { call } => {
                if let Ok(mut active) = self.active_call.lock() {
                    active.take();
                }
                self.forget_inbound_leg(*call);
                AgentCommand::Decline
            }
            CollabCommand::HangUpCall { call } => {
                if let Ok(mut active) = self.active_call.lock() {
                    active.take();
                }
                self.forget_inbound_leg(*call);
                AgentCommand::HangUp
            }
            CollabCommand::SendDtmf { digit, .. } => AgentCommand::Dtmf(*digit),
            CollabCommand::StartCall { .. } => {
                return Err(CallMediaProviderError::ExecutionRefused {
                    detail: "outbound SIP execution requires an explicit dial target".to_string(),
                });
            }
            CollabCommand::StartOutboundCall { call, target, .. } => {
                validate_sip_dial_target(target)?;
                let target = target.trim().to_owned();
                self.send_acknowledged(|completion| AgentCommand::Dial { target, completion })?;
                if let Ok(mut active) = self.active_call.lock() {
                    *active = Some(*call);
                }
                let _ = self.publish_plane.execute_command(command, adapter);
                return Ok(());
            }
            CollabCommand::SetCallMuted { muted, .. } => {
                let observed = self.send_acknowledged(|completion| AgentCommand::SetMuted {
                    muted: *muted,
                    completion,
                })?;
                if observed != *muted {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "SIP agent returned a mismatched mute state".to_string(),
                    });
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        self.commands.try_send(agent_command).map_err(|error| {
            CallMediaProviderError::ProviderUnavailable {
                detail: match error {
                    std::sync::mpsc::TrySendError::Full(_) => {
                        "SIP provider command queue is full".to_string()
                    }
                    std::sync::mpsc::TrySendError::Disconnected(_) => {
                        "SIP provider command queue is disconnected".to_string()
                    }
                },
            }
        })?;
        if cleanup {
            let _ = self.publish_plane.execute_command(command, adapter);
        }
        Ok(())
    }

    fn prove_advancing_frames(
        &self,
        _session: &CallMediaSession,
        adapter: CallMediaAdapter,
    ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
        if adapter != CallMediaAdapter::SipGateway {
            return Err(CallMediaProviderError::ExecutionRefused {
                detail: "SIP provider was selected for an incompatible adapter".to_string(),
            });
        }
        self.require_ready()?;
        Err(CallMediaProviderError::ExecutionRefused {
            detail: "SIP/RTP frame counters are unavailable; live media is not proven".to_string(),
        })
    }

    fn take_revoked_calls(&self) -> Vec<mde_collab_types::CallId> {
        self.revoked_calls
            .lock()
            .map(|mut revoked| std::mem::take(&mut *revoked))
            .unwrap_or_default()
    }

    fn pending_inbound_call(&self) -> Option<InboundCallOffer> {
        self.pending_inbound.lock().ok()?.clone()
    }

    fn bind_inbound_call(
        &self,
        offer: &InboundCallOffer,
        call: Option<mde_collab_types::CallId>,
    ) -> bool {
        let Ok(mut pending) = self.pending_inbound.lock() else {
            return false;
        };
        if pending.as_ref() != Some(offer) {
            return false;
        }
        pending.take();
        if let Some(call) = call {
            let Ok(mut active) = self.active_call.lock() else {
                return false;
            };
            if active.is_some() {
                return false;
            }
            *active = Some(call);
            self.retain_inbound_leg(call, offer);
        }
        true
    }

    fn publish_p2p_media_sessions(
        &self,
        persist: &Persist,
        last_published: &mut BTreeMap<String, String>,
    ) {
        self.publish_plane
            .publish_p2p_media_sessions(persist, last_published);
        self.publish_inbound_sip_legs(persist, last_published);
    }
}

pub(super) fn publish_retained_call_media_verification(
    persist: &Persist,
    last_published: &mut BTreeMap<String, String>,
    providers: &CallMediaProviderRegistry,
) {
    providers.publish_p2p_media_sessions(persist, last_published);
    let topic = topics::state_topic(proj::CALL_MEDIA_VERIFICATION);
    match verify_retained_call_media(persist, providers) {
        Ok(board) => match serde_json::to_string(&board) {
            Ok(body) => {
                if last_published.get(&topic).map(String::as_str) == Some(body.as_str()) {
                    return;
                }
                if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
                    tracing::debug!(target: "mackesd::collab", topic, error = %e, "collab media verification publish failed");
                    return;
                }
                last_published.insert(topic, body);
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::collab", topic, error = %e, "serialize collab media verification failed")
            }
        },
        Err(e) => {
            tracing::debug!(target: "mackesd::collab", topic, error = %e, "collab media verification unavailable");
            publish_call_media_verification_tombstone(persist, last_published, &topic);
        }
    }
}

fn publish_call_media_verification_tombstone(
    persist: &Persist,
    last_published: &mut BTreeMap<String, String>,
    topic: &str,
) {
    if last_published.get(topic).map(String::as_str) == Some(VERIFICATION_UNAVAILABLE) {
        return;
    }
    if let Err(e) = persist.write(topic, Priority::Default, None, None) {
        tracing::debug!(target: "mackesd::collab", topic, error = %e, "collab media verification tombstone publish failed");
        return;
    }
    last_published.insert(topic.to_string(), VERIFICATION_UNAVAILABLE.to_string());
}

fn verify_retained_call_media(
    persist: &Persist,
    providers: &CallMediaProviderRegistry,
) -> Result<CallMediaVerification, CallMediaVerificationError> {
    let topic = topics::state_topic(proj::CALL_MEDIA_READINESS);
    let msg = persist
        .read_latest(&topic)
        .map_err(|source| CallMediaVerificationError::ReadTopic {
            topic: topic.clone(),
            detail: source.to_string(),
        })?
        .ok_or_else(|| CallMediaVerificationError::MissingTopic {
            topic: topic.clone(),
        })?;
    let body = msg
        .body
        .as_deref()
        .ok_or_else(|| CallMediaVerificationError::MissingBody {
            topic: topic.clone(),
        })?;
    if body.len() > MAX_READINESS_BODY_BYTES {
        return Err(CallMediaVerificationError::BodyTooLarge {
            topic,
            len: body.len(),
            max: MAX_READINESS_BODY_BYTES,
        });
    }
    let readiness = serde_json::from_str::<CallMediaReadiness>(body).map_err(|source| {
        CallMediaVerificationError::DecodeReadiness {
            topic,
            detail: source.to_string(),
        }
    })?;
    verify_call_media_readiness(&readiness, providers)
}

fn verify_call_media_readiness(
    readiness: &CallMediaReadiness,
    providers: &CallMediaProviderRegistry,
) -> Result<CallMediaVerification, CallMediaVerificationError> {
    if readiness.sessions.len() > MAX_VERIFICATION_SESSIONS {
        return Err(CallMediaVerificationError::TooManySessions {
            count: readiness.sessions.len(),
            max: MAX_VERIFICATION_SESSIONS,
        });
    }

    let mut rows = Vec::new();
    for (session_index, session) in readiness.sessions.iter().enumerate() {
        validate_session_provenance(readiness, session_index, session)?;
        for &adapter in &session.candidate_adapters {
            if rows.len() >= MAX_VERIFICATION_ROWS {
                return Err(CallMediaVerificationError::TooManyRows {
                    count: rows.len() + 1,
                    max: MAX_VERIFICATION_ROWS,
                });
            }
            rows.push(verify_candidate(session, adapter, providers)?);
        }
    }

    Ok(CallMediaVerification {
        local_actor: readiness.local_actor.clone(),
        rows,
    })
}

fn validate_session_provenance(
    readiness: &CallMediaReadiness,
    session_index: usize,
    session: &CallMediaSession,
) -> Result<(), CallMediaVerificationError> {
    if readiness.sessions[..session_index]
        .iter()
        .any(|prior| prior.call == session.call)
    {
        return Err(CallMediaVerificationError::DuplicateCall { call: session.call });
    }

    let local_count = session
        .connected_participants
        .iter()
        .filter(|actor| *actor == &readiness.local_actor)
        .count();
    let has_duplicate_participant = session
        .connected_participants
        .iter()
        .enumerate()
        .any(|(index, actor)| session.connected_participants[..index].contains(actor));
    let admission_matches = match session.admission {
        CallMediaAdmission::AdapterReady => session.connected_participants.len() >= 2,
        CallMediaAdmission::WaitingForConnectedPeer => session.connected_participants.len() == 1,
    };
    if local_count != 1 || has_duplicate_participant || !admission_matches {
        return Err(CallMediaVerificationError::InvalidSessionProvenance {
            call: session.call,
            local_count,
            participant_count: session.connected_participants.len(),
            duplicate_participant: has_duplicate_participant,
            admission: session.admission,
        });
    }
    Ok(())
}

fn verify_candidate(
    session: &CallMediaSession,
    adapter: CallMediaAdapter,
    providers: &CallMediaProviderRegistry,
) -> Result<CallMediaVerificationRow, CallMediaVerificationError> {
    if !candidate_matches_session(session, adapter) {
        return row(
            session,
            adapter,
            CallMediaVerificationStatus::MediaNotProven,
            None,
            Some("candidate adapter or requirements do not match the declared call kind"),
        );
    }

    if session.admission == CallMediaAdmission::WaitingForConnectedPeer {
        return row(
            session,
            adapter,
            CallMediaVerificationStatus::WaitingForConnectedPeer,
            None,
            Some("waiting for a connected remote peer before probing media"),
        );
    }

    match providers.prove_advancing_frames(session, adapter) {
        Ok(evidence) => {
            if evidence_satisfies_requirements(&session.requirements, evidence) {
                row(
                    session,
                    adapter,
                    CallMediaVerificationStatus::LiveMediaVerified,
                    Some(evidence),
                    None,
                )
            } else {
                row(
                    session,
                    adapter,
                    CallMediaVerificationStatus::MediaNotProven,
                    None,
                    Some(missing_evidence_detail(&session.requirements, evidence).as_str()),
                )
            }
        }
        Err(CallMediaProviderError::TransportUnavailable { detail }) => row(
            session,
            adapter,
            CallMediaVerificationStatus::TransportUnavailable,
            None,
            Some(detail.as_str()),
        ),
        Err(CallMediaProviderError::ProviderUnavailable { detail }) => row(
            session,
            adapter,
            CallMediaVerificationStatus::ProviderUnavailable,
            None,
            Some(detail.as_str()),
        ),
        Err(CallMediaProviderError::ExecutionRefused { detail }) => row(
            session,
            adapter,
            CallMediaVerificationStatus::ProviderUnavailable,
            None,
            Some(detail.as_str()),
        ),
    }
}

fn candidate_matches_session(session: &CallMediaSession, adapter: CallMediaAdapter) -> bool {
    let (requirements, adapters): (&[CallMediaRequirement], &[CallMediaAdapter]) =
        match session.kind {
            CallKind::Audio => (
                &[CallMediaRequirement::Microphone],
                &[
                    CallMediaAdapter::WebRtcP2p,
                    CallMediaAdapter::LiveKitSfu,
                    CallMediaAdapter::SipGateway,
                ],
            ),
            CallKind::Video => (
                &[
                    CallMediaRequirement::Microphone,
                    CallMediaRequirement::Camera,
                ],
                &[CallMediaAdapter::WebRtcP2p, CallMediaAdapter::LiveKitSfu],
            ),
            CallKind::Screen => (
                &[
                    CallMediaRequirement::Microphone,
                    CallMediaRequirement::ScreenCapture,
                ],
                &[CallMediaAdapter::WebRtcP2p, CallMediaAdapter::LiveKitSfu],
            ),
            CallKind::CoEdit => (
                &[CallMediaRequirement::DocumentSync],
                &[CallMediaAdapter::DocumentCollab],
            ),
            CallKind::RemoteDesktop => (
                &[CallMediaRequirement::RemoteDesktopStream],
                &[CallMediaAdapter::VdiRemoteDesktop],
            ),
        };

    session.requirements == requirements
        && !session.candidate_adapters.is_empty()
        && session
            .candidate_adapters
            .iter()
            .enumerate()
            .all(|(index, candidate)| {
                adapters.contains(candidate)
                    && !session.candidate_adapters[..index].contains(candidate)
            })
        && adapters.contains(&adapter)
}

fn row(
    session: &CallMediaSession,
    adapter: CallMediaAdapter,
    status: CallMediaVerificationStatus,
    evidence: Option<CallMediaFrameEvidence>,
    detail: Option<&str>,
) -> Result<CallMediaVerificationRow, CallMediaVerificationError> {
    let detail = detail.map(bounded_detail).transpose()?;
    Ok(CallMediaVerificationRow {
        call: session.call,
        space: session.space,
        kind: session.kind,
        adapter,
        status,
        evidence,
        detail,
    })
}

fn bounded_detail(detail: &str) -> Result<String, CallMediaVerificationError> {
    if detail.len() > MAX_DETAIL_BYTES {
        return Err(CallMediaVerificationError::DetailTooLarge {
            len: detail.len(),
            max: MAX_DETAIL_BYTES,
        });
    }
    Ok(detail.to_string())
}

fn evidence_satisfies_requirements(
    requirements: &[CallMediaRequirement],
    evidence: CallMediaFrameEvidence,
) -> bool {
    requirements.iter().all(|requirement| match requirement {
        CallMediaRequirement::Microphone => evidence.audio_frames > 0,
        CallMediaRequirement::Camera => evidence.video_frames > 0,
        CallMediaRequirement::ScreenCapture => evidence.screen_frames > 0,
        CallMediaRequirement::DocumentSync => evidence.data_messages > 0,
        CallMediaRequirement::RemoteDesktopStream => evidence.screen_frames > 0,
    })
}

fn missing_evidence_detail(
    requirements: &[CallMediaRequirement],
    evidence: CallMediaFrameEvidence,
) -> String {
    let mut missing = Vec::new();
    for requirement in requirements {
        match requirement {
            CallMediaRequirement::Microphone if evidence.audio_frames == 0 => {
                missing.push("audio frames")
            }
            CallMediaRequirement::Camera if evidence.video_frames == 0 => {
                missing.push("video frames")
            }
            CallMediaRequirement::ScreenCapture if evidence.screen_frames == 0 => {
                missing.push("screen frames")
            }
            CallMediaRequirement::DocumentSync if evidence.data_messages == 0 => {
                missing.push("data messages")
            }
            CallMediaRequirement::RemoteDesktopStream if evidence.screen_frames == 0 => {
                missing.push("remote-desktop frames")
            }
            _ => {}
        }
    }
    format!(
        "media verifier did not prove advancing {}",
        missing.join(", ")
    )
}

pub(crate) trait CallMediaFrameVerifier: Send + Sync {
    /// Execute one already-authorized call command against the provider.
    ///
    /// Proof-only registrations deliberately fail closed here: observing frame
    /// counters is not authority to start, answer, mutate, or signal a session.
    /// Concrete providers must override this method before their registration
    /// can drive signed call state.
    fn execute_command(
        &self,
        _command: &CollabCommand,
        adapter: CallMediaAdapter,
    ) -> Result<(), CallMediaProviderError> {
        Err(CallMediaProviderError::ExecutionRefused {
            detail: format!("{adapter:?} provider is proof-only and cannot execute call commands"),
        })
    }

    /// Prove live media for `session` by returning frame/data deltas observed
    /// during a bounded provider-owned sampling window. Cumulative counters or
    /// desired-state readiness are not sufficient for a
    /// [`CallMediaVerificationStatus::LiveMediaVerified`] row.
    fn prove_advancing_frames(
        &self,
        session: &CallMediaSession,
        adapter: CallMediaAdapter,
    ) -> Result<CallMediaFrameEvidence, CallMediaProviderError>;

    /// Consume provider-observed call terminations that must revoke the
    /// corresponding signed Collaboration call on the next worker tick.
    fn take_revoked_calls(&self) -> Vec<mde_collab_types::CallId> {
        Vec::new()
    }

    /// Return the currently ringing provider dialog without granting it any
    /// Collaboration authority. Only the SIP adapter supplies this.
    fn pending_inbound_call(&self) -> Option<InboundCallOffer> {
        None
    }

    /// Consume exactly the offer previously returned by
    /// [`Self::pending_inbound_call`]. A mismatch means the provider dialog was
    /// replaced while signed-state admission was running and must fail stale.
    /// `None` rejects an unauthorized offer without creating a call owner.
    fn bind_inbound_call(
        &self,
        _offer: &InboundCallOffer,
        _call: Option<mde_collab_types::CallId>,
    ) -> bool {
        false
    }

    /// Whether this provider currently owns a live media leg for `call`.
    fn owns_call(&self, _call: mde_collab_types::CallId) -> bool {
        false
    }

    /// Drive the P2P offer/answer plane and publish
    /// `state/calls/media/<session>` readiness. The SIP verifier leaves this
    /// as a no-op; the WebRTC P2P worker overrides it.
    fn publish_p2p_media_sessions(
        &self,
        persist: &Persist,
        last_published: &mut BTreeMap<String, String>,
    ) {
        let _ = (persist, last_published);
    }
}

/// One bounded in-process registration table for concrete call-media proof
/// providers. At most one provider can own each adapter family; the worker's
/// production default is empty and therefore fail-honest.
#[derive(Default)]
pub(crate) struct CallMediaProviderRegistry {
    webrtc_p2p: Option<Box<dyn CallMediaFrameVerifier>>,
    livekit_sfu: Option<Box<dyn CallMediaFrameVerifier>>,
    sip_gateway: Option<Box<dyn CallMediaFrameVerifier>>,
    document_collab: Option<Box<dyn CallMediaFrameVerifier>>,
    vdi_remote_desktop: Option<Box<dyn CallMediaFrameVerifier>>,
}

impl CallMediaProviderRegistry {
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    /// Construct the production registry. A configured SIP core is admitted
    /// as the concrete audio provider. The P2P, SFU, and SIP publish planes
    /// are always registered on a Workstation so live seats tick
    /// `state/calls/media/<session>/sip` through the same registry path.
    #[must_use]
    pub(crate) fn production() -> Self {
        let mut registry = Self::empty();
        // Unit tests inject deterministic providers and must never bind the
        // developer's real SIP account merely by constructing a worker.
        if cfg!(test) && std::env::var_os("MCNF_TEST_ENABLE_REAL_SIP_PROVIDER").is_none() {
            return registry;
        }
        if !matches!(mde_role::load(), Ok(mde_role::Role::Workstation)) {
            tracing::info!(target: "mackesd::collab", "SIP Calls provider requires a pinned Workstation role");
            return registry;
        }
        match SipGatewayProvider::activate() {
            Ok(provider) => registry.sip_gateway = Some(Box::new(provider)),
            Err(detail) => {
                tracing::info!(target: "mackesd::collab", detail, "SIP Calls provider not activated")
            }
        }
        registry.webrtc_p2p = Some(Box::new(super::call_media::WebrtcP2pPlane::production()));
        registry.livekit_sfu = Some(Box::new(super::call_media::LiveKitSfuPlane::production()));
        // The SIP publish plane is the S4 `/sip` tick, matching P2P + SFU.
        // A live agent adapter already occupies this slot when a governed
        // account activated; otherwise the plane still publishes honesty.
        if registry.sip_gateway.is_none() {
            registry.sip_gateway = Some(Box::new(super::call_media::SipGatewayPlane::production()));
        }
        registry
    }

    #[cfg(test)]
    pub(crate) fn register<P>(
        &mut self,
        adapter: CallMediaAdapter,
        provider: P,
    ) -> Result<(), CallMediaProviderRegistrationError>
    where
        P: CallMediaFrameVerifier + 'static,
    {
        let slot = self.slot_mut(adapter);
        if slot.is_some() {
            return Err(CallMediaProviderRegistrationError::AlreadyRegistered { adapter });
        }
        *slot = Some(Box::new(provider));
        Ok(())
    }

    /// Admit only media-effect commands backed by a provider registered for
    /// the call kind. Production currently constructs an empty registry, so it
    /// must refuse to mint connected/muted/DTMF state instead of pretending a
    /// transport exists. Cleanup commands deliberately bypass this boundary.
    pub(crate) fn admit_command(
        &self,
        command: &CollabCommand,
        existing_kind: Option<CallKind>,
    ) -> Result<(), CallMediaCommandAdmissionError> {
        let kind = match command {
            CollabCommand::StartCall { kind, .. } => Some(*kind),
            CollabCommand::StartOutboundCall { .. } => Some(CallKind::Audio),
            CollabCommand::AnswerCall { .. }
            | CollabCommand::SendDtmf { .. }
            | CollabCommand::SetCallMuted { .. } => existing_kind,
            // Decline and hang-up are revocation/cleanup paths and remain
            // available even after every provider has disappeared.
            _ => return Ok(()),
        };
        let Some(kind) = kind else {
            // Let the core return its authoritative CallNotFound error when an
            // effect targets no active call.
            return Ok(());
        };
        if matches!(command, CollabCommand::StartOutboundCall { .. }) {
            return if self.sip_gateway.is_some() {
                Ok(())
            } else {
                Err(CallMediaCommandAdmissionError::NoProvider { kind })
            };
        }
        if self.supports(kind) {
            Ok(())
        } else {
            Err(CallMediaCommandAdmissionError::NoProvider { kind })
        }
    }

    /// Execute a call effect through exactly one deterministic provider.
    ///
    /// Start/answer/media-control commands fail closed when execution fails.
    /// Decline/hang-up remain available with no provider (local revocation must
    /// survive provider loss), but when a provider is still registered the
    /// cleanup is sent to it before the signed revocation is authored.
    pub(crate) fn execute_command(
        &self,
        command: &CollabCommand,
        existing_kind: Option<CallKind>,
    ) -> Result<(), CallMediaCommandExecutionError> {
        let (kind, cleanup) = match command {
            // A Remote Desktop start is an invitation, not consent to inject
            // input.  Persisting that signed invitation is deliberately free
            // of provider side effects; only the exact AnswerCall below may
            // activate the VDI provider for the admitted call id.
            CollabCommand::StartCall {
                kind: CallKind::RemoteDesktop,
                ..
            } => return Ok(()),
            CollabCommand::StartCall { kind, .. } => (Some(*kind), false),
            CollabCommand::StartOutboundCall { .. } => (Some(CallKind::Audio), false),
            CollabCommand::AnswerCall { .. }
            | CollabCommand::SendDtmf { .. }
            | CollabCommand::SetCallMuted { .. } => (existing_kind, false),
            CollabCommand::DeclineCall { .. } | CollabCommand::HangUpCall { .. } => {
                (existing_kind, true)
            }
            _ => return Ok(()),
        };
        let Some(kind) = kind else {
            // The collaboration core owns the authoritative CallNotFound
            // result; do not send an unattributed command to any provider.
            return Ok(());
        };
        let Some((adapter, provider)) = self.provider_for_command(command, kind) else {
            return if cleanup {
                Ok(())
            } else {
                Err(CallMediaCommandExecutionError::NoProvider { kind })
            };
        };
        provider
            .execute_command(command, adapter)
            .map_err(|source| CallMediaCommandExecutionError::ProviderFailed {
                kind,
                adapter,
                detail: bounded_provider_error(source),
            })
    }

    pub(crate) fn take_revoked_calls(&self) -> Vec<mde_collab_types::CallId> {
        let mut calls = Vec::new();
        for provider in [
            self.webrtc_p2p.as_deref(),
            self.livekit_sfu.as_deref(),
            self.sip_gateway.as_deref(),
            self.document_collab.as_deref(),
            self.vdi_remote_desktop.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            for call in provider.take_revoked_calls() {
                if !calls.contains(&call) {
                    calls.push(call);
                }
            }
        }
        calls
    }

    pub(crate) fn pending_inbound_sip_call(&self) -> Option<InboundCallOffer> {
        self.sip_gateway.as_deref()?.pending_inbound_call()
    }

    pub(crate) fn bind_inbound_sip_call(
        &self,
        offer: &InboundCallOffer,
        call: Option<mde_collab_types::CallId>,
    ) -> bool {
        self.sip_gateway
            .as_deref()
            .is_some_and(|provider| provider.bind_inbound_call(offer, call))
    }

    pub(crate) fn publish_p2p_media_sessions(
        &self,
        persist: &Persist,
        last_published: &mut BTreeMap<String, String>,
    ) {
        if let Some(plane) = self.webrtc_p2p.as_deref() {
            plane.publish_p2p_media_sessions(persist, last_published);
        }
        if let Some(plane) = self.livekit_sfu.as_deref() {
            plane.publish_p2p_media_sessions(persist, last_published);
        }
        if let Some(plane) = self.sip_gateway.as_deref() {
            plane.publish_p2p_media_sessions(persist, last_published);
        }
    }

    pub(crate) fn provider_for_command(
        &self,
        command: &CollabCommand,
        kind: CallKind,
    ) -> Option<(CallMediaAdapter, &dyn CallMediaFrameVerifier)> {
        if matches!(command, CollabCommand::StartOutboundCall { .. }) {
            return self
                .slot(CallMediaAdapter::SipGateway)
                .as_deref()
                .map(|provider| (CallMediaAdapter::SipGateway, provider));
        }
        if let Some(call) = Self::command_call_id(command) {
            if matches!(
                command,
                CollabCommand::SendDtmf { .. }
                    | CollabCommand::SetCallMuted { .. }
                    | CollabCommand::HangUpCall { .. }
                    | CollabCommand::DeclineCall { .. }
            ) {
                if self
                    .livekit_sfu
                    .as_deref()
                    .is_some_and(|provider| provider.owns_call(call))
                {
                    return self
                        .slot(CallMediaAdapter::LiveKitSfu)
                        .as_deref()
                        .map(|provider| (CallMediaAdapter::LiveKitSfu, provider));
                }
                if self
                    .webrtc_p2p
                    .as_deref()
                    .is_some_and(|provider| provider.owns_call(call))
                {
                    return self
                        .slot(CallMediaAdapter::WebRtcP2p)
                        .as_deref()
                        .map(|provider| (CallMediaAdapter::WebRtcP2p, provider));
                }
            }
        }
        self.provider_for_kind(kind)
    }

    fn command_call_id(command: &CollabCommand) -> Option<CallId> {
        match command {
            CollabCommand::StartCall { call, .. }
            | CollabCommand::StartOutboundCall { call, .. }
            | CollabCommand::AnswerCall { call }
            | CollabCommand::DeclineCall { call }
            | CollabCommand::HangUpCall { call }
            | CollabCommand::SendDtmf { call, .. }
            | CollabCommand::SetCallMuted { call, .. } => Some(*call),
            _ => None,
        }
    }

    fn provider_for_kind(
        &self,
        kind: CallKind,
    ) -> Option<(CallMediaAdapter, &dyn CallMediaFrameVerifier)> {
        let candidates: &[CallMediaAdapter] = match kind {
            CallKind::Audio => &[
                CallMediaAdapter::WebRtcP2p,
                CallMediaAdapter::LiveKitSfu,
                CallMediaAdapter::SipGateway,
            ],
            CallKind::Video | CallKind::Screen => {
                &[CallMediaAdapter::WebRtcP2p, CallMediaAdapter::LiveKitSfu]
            }
            CallKind::CoEdit => &[CallMediaAdapter::DocumentCollab],
            CallKind::RemoteDesktop => &[CallMediaAdapter::VdiRemoteDesktop],
        };
        candidates.iter().find_map(|adapter| {
            self.slot(*adapter)
                .as_deref()
                .map(|provider| (*adapter, provider))
        })
    }

    fn supports(&self, kind: CallKind) -> bool {
        match kind {
            CallKind::Audio => {
                self.webrtc_p2p.is_some()
                    || self.livekit_sfu.is_some()
                    || self.sip_gateway.is_some()
            }
            CallKind::Video | CallKind::Screen => {
                self.webrtc_p2p.is_some() || self.livekit_sfu.is_some()
            }
            CallKind::CoEdit => self.document_collab.is_some(),
            CallKind::RemoteDesktop => self.vdi_remote_desktop.is_some(),
        }
    }

    fn prove_advancing_frames(
        &self,
        session: &CallMediaSession,
        adapter: CallMediaAdapter,
    ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
        let Some(provider) = self.slot(adapter).as_deref() else {
            return Err(missing_provider_error(adapter));
        };
        provider.prove_advancing_frames(session, adapter)
    }

    #[cfg(test)]
    fn slot_mut(
        &mut self,
        adapter: CallMediaAdapter,
    ) -> &mut Option<Box<dyn CallMediaFrameVerifier>> {
        match adapter {
            CallMediaAdapter::WebRtcP2p => &mut self.webrtc_p2p,
            CallMediaAdapter::LiveKitSfu => &mut self.livekit_sfu,
            CallMediaAdapter::SipGateway => &mut self.sip_gateway,
            CallMediaAdapter::DocumentCollab => &mut self.document_collab,
            CallMediaAdapter::VdiRemoteDesktop => &mut self.vdi_remote_desktop,
        }
    }

    fn slot(&self, adapter: CallMediaAdapter) -> &Option<Box<dyn CallMediaFrameVerifier>> {
        match adapter {
            CallMediaAdapter::WebRtcP2p => &self.webrtc_p2p,
            CallMediaAdapter::LiveKitSfu => &self.livekit_sfu,
            CallMediaAdapter::SipGateway => &self.sip_gateway,
            CallMediaAdapter::DocumentCollab => &self.document_collab,
            CallMediaAdapter::VdiRemoteDesktop => &self.vdi_remote_desktop,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallMediaCommandAdmissionError {
    NoProvider { kind: CallKind },
}

impl fmt::Display for CallMediaCommandAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProvider { kind } => write!(
                formatter,
                "no admitted call-media provider is registered for {kind:?}"
            ),
        }
    }
}

impl Error for CallMediaCommandAdmissionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallMediaCommandExecutionError {
    NoProvider {
        kind: CallKind,
    },
    ProviderFailed {
        kind: CallKind,
        adapter: CallMediaAdapter,
        detail: String,
    },
}

impl fmt::Display for CallMediaCommandExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProvider { kind } => {
                write!(formatter, "no call-media provider can execute {kind:?}")
            }
            Self::ProviderFailed {
                kind,
                adapter,
                detail,
            } => write!(
                formatter,
                "{adapter:?} provider failed to execute {kind:?}: {detail}"
            ),
        }
    }
}

impl Error for CallMediaCommandExecutionError {}

fn bounded_provider_error(error: CallMediaProviderError) -> String {
    let detail = match error {
        CallMediaProviderError::TransportUnavailable { detail }
        | CallMediaProviderError::ProviderUnavailable { detail }
        | CallMediaProviderError::ExecutionRefused { detail } => detail,
    };
    if detail.len() <= MAX_DETAIL_BYTES {
        return detail;
    }
    let mut end = MAX_DETAIL_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_string()
}

fn missing_provider_error(adapter: CallMediaAdapter) -> CallMediaProviderError {
    match adapter {
        CallMediaAdapter::LiveKitSfu => CallMediaProviderError::ProviderUnavailable {
            detail: "no LiveKit SFU provider is registered on this node".to_string(),
        },
        CallMediaAdapter::SipGateway => CallMediaProviderError::ProviderUnavailable {
            detail: "no SIP gateway/provider is registered on this node".to_string(),
        },
        CallMediaAdapter::WebRtcP2p => CallMediaProviderError::TransportUnavailable {
            detail: "no WebRTC media verifier is registered on this node".to_string(),
        },
        CallMediaAdapter::DocumentCollab => CallMediaProviderError::TransportUnavailable {
            detail: "no document-collaboration media verifier is registered on this node"
                .to_string(),
        },
        CallMediaAdapter::VdiRemoteDesktop => CallMediaProviderError::TransportUnavailable {
            detail: "no VDI remote-desktop media verifier is registered on this node".to_string(),
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum CallMediaProviderRegistrationError {
    AlreadyRegistered { adapter: CallMediaAdapter },
}

#[derive(Debug)]
pub(crate) enum CallMediaProviderError {
    TransportUnavailable { detail: String },
    ProviderUnavailable { detail: String },
    ExecutionRefused { detail: String },
}

impl std::fmt::Display for CallMediaProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TransportUnavailable { detail }
            | Self::ProviderUnavailable { detail }
            | Self::ExecutionRefused { detail } => f.write_str(detail),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CallMediaVerificationError {
    MissingTopic {
        topic: String,
    },
    MissingBody {
        topic: String,
    },
    ReadTopic {
        topic: String,
        detail: String,
    },
    BodyTooLarge {
        topic: String,
        len: usize,
        max: usize,
    },
    DecodeReadiness {
        topic: String,
        detail: String,
    },
    TooManySessions {
        count: usize,
        max: usize,
    },
    TooManyRows {
        count: usize,
        max: usize,
    },
    DuplicateCall {
        call: mde_collab_types::CallId,
    },
    InvalidSessionProvenance {
        call: mde_collab_types::CallId,
        local_count: usize,
        participant_count: usize,
        duplicate_participant: bool,
        admission: CallMediaAdmission,
    },
    DetailTooLarge {
        len: usize,
        max: usize,
    },
}

impl fmt::Display for CallMediaVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTopic { topic } => write!(f, "{topic} has no retained readiness row"),
            Self::MissingBody { topic } => write!(f, "{topic} retained readiness row has no body"),
            Self::ReadTopic { topic, detail } => {
                write!(f, "failed to read {topic}: {detail}")
            }
            Self::BodyTooLarge { topic, len, max } => {
                write!(f, "{topic} body is {len} bytes, over {max}")
            }
            Self::DecodeReadiness { topic, detail } => {
                write!(f, "failed to decode {topic}: {detail}")
            }
            Self::TooManySessions { count, max } => {
                write!(f, "call media readiness has {count} sessions, over {max}")
            }
            Self::TooManyRows { count, max } => {
                write!(
                    f,
                    "call media verification would publish {count} rows, over {max}"
                )
            }
            Self::DuplicateCall { call } => {
                write!(f, "call media readiness repeats call {call}")
            }
            Self::InvalidSessionProvenance {
                call,
                local_count,
                participant_count,
                duplicate_participant,
                admission,
            } => write!(
                f,
                "call {call} has invalid local media provenance: local actor count {local_count}, participant count {participant_count}, duplicate participant {duplicate_participant}, admission {admission:?}"
            ),
            Self::DetailTooLarge { len, max } => {
                write!(
                    f,
                    "call media verification detail is {len} bytes, over {max}"
                )
            }
        }
    }
}

impl Error for CallMediaVerificationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_collab_types::{
        media_session_topic, media_sip_leg_topic, ActorId, CallId, CallKind, SipLegDirectionV1,
        SipLegV1, SpaceId,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn write_readiness(persist: &Persist, readiness: &CallMediaReadiness) {
        let body = serde_json::to_string(readiness).expect("serialize readiness");
        persist
            .write(
                &topics::state_topic(proj::CALL_MEDIA_READINESS),
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("write readiness");
    }

    fn ready_audio_session() -> CallMediaSession {
        CallMediaSession {
            call: CallId::new(),
            space: SpaceId::new(),
            kind: CallKind::Audio,
            started_unix_ms: 1_700_000_000_000,
            requirements: vec![CallMediaRequirement::Microphone],
            candidate_adapters: vec![
                CallMediaAdapter::WebRtcP2p,
                CallMediaAdapter::LiveKitSfu,
                CallMediaAdapter::SipGateway,
            ],
            admission: CallMediaAdmission::AdapterReady,
            connected_participants: vec![ActorId::new("alice"), ActorId::new("bob")],
            local_muted: false,
        }
    }

    #[test]
    fn concrete_sip_provider_is_bounded_health_checked_and_fail_closed() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let provider = SipGatewayProvider::with_channel(tx, SipProviderHealth::Ready);
        let call = CallId::new();

        provider
            .execute_command(
                &CollabCommand::AnswerCall { call },
                CallMediaAdapter::SipGateway,
            )
            .expect("healthy provider queues one real agent command");
        assert!(matches!(
            rx.recv().expect("agent command"),
            AgentCommand::Answer
        ));

        provider
            .execute_command(
                &CollabCommand::SendDtmf { call, digit: '5' },
                CallMediaAdapter::SipGateway,
            )
            .expect("healthy provider routes DTMF");
        assert!(matches!(
            rx.recv().expect("DTMF command"),
            AgentCommand::Dtmf('5')
        ));

        let outbound = provider
            .execute_command(
                &CollabCommand::StartCall {
                    space: SpaceId::new(),
                    call,
                    kind: CallKind::Audio,
                },
                CallMediaAdapter::SipGateway,
            )
            .expect_err("missing dial target must fail closed");
        assert!(matches!(
            outbound,
            CallMediaProviderError::ExecutionRefused { .. }
        ));

        let responder = std::thread::spawn(move || {
            match rx.recv().expect("dial command") {
                AgentCommand::Dial { target, completion } => {
                    assert_eq!(target, "+15551234567");
                    completion.send(Ok(())).expect("acknowledge dial");
                }
                other => panic!("unexpected agent command: {other:?}"),
            }
            match rx.recv().expect("mute command") {
                AgentCommand::SetMuted { muted, completion } => {
                    assert!(muted);
                    completion.send(Ok(true)).expect("acknowledge mute");
                }
                other => panic!("unexpected agent command: {other:?}"),
            }
        });
        provider
            .execute_command(
                &CollabCommand::StartOutboundCall {
                    space: SpaceId::new(),
                    call,
                    target: "+15551234567".into(),
                },
                CallMediaAdapter::SipGateway,
            )
            .expect("dial completes only after the agent acknowledgement");
        provider
            .execute_command(
                &CollabCommand::SetCallMuted { call, muted: true },
                CallMediaAdapter::SipGateway,
            )
            .expect("mute completes only after observed media state");
        responder.join().expect("agent responder");

        *provider.health.lock().expect("health") =
            SipProviderHealth::Unavailable("registration lost".to_string());
        provider
            .execute_command(
                &CollabCommand::HangUpCall { call },
                CallMediaAdapter::SipGateway,
            )
            .expect("provider loss must not trap signed cleanup state");
        let unavailable = provider
            .execute_command(
                &CollabCommand::SendDtmf { call, digit: '6' },
                CallMediaAdapter::SipGateway,
            )
            .expect_err("unhealthy provider must not accept media effects");
        assert!(matches!(
            unavailable,
            CallMediaProviderError::ProviderUnavailable { .. }
        ));
    }

    #[test]
    fn outbound_sip_ingress_rejects_unbounded_or_control_targets_before_dial() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let provider = SipGatewayProvider::with_channel(tx, SipProviderHealth::Ready);
        let oversized = "x".repeat(MAX_SIP_DIAL_TARGET_BYTES + 1);
        for target in ["", "   ", "peer\nforged-header", oversized.as_str()] {
            let error = provider
                .execute_command(
                    &CollabCommand::StartOutboundCall {
                        space: SpaceId::new(),
                        call: CallId::new(),
                        target: target.to_string(),
                    },
                    CallMediaAdapter::SipGateway,
                )
                .expect_err("invalid signed dial target must fail closed");
            assert!(matches!(
                error,
                CallMediaProviderError::ExecutionRefused { .. }
            ));
            assert!(matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
        }
    }

    #[test]
    fn inbound_sip_binding_rejects_replaced_and_replayed_dialogs() {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        let provider = SipGatewayProvider::with_channel(tx, SipProviderHealth::Ready);
        let stale = InboundCallOffer {
            identity: "alice@example.com".to_string(),
            provider_call_id: "old@example.com".to_string(),
        };
        let current = InboundCallOffer {
            identity: "bob@example.com".to_string(),
            provider_call_id: "new@example.com".to_string(),
        };
        *provider.pending_inbound.lock().expect("pending lock") = Some(current.clone());

        assert!(!provider.bind_inbound_call(&stale, Some(CallId::new())));
        assert_eq!(provider.pending_inbound_call(), Some(current.clone()));
        let call = CallId::new();
        assert!(provider.bind_inbound_call(&current, Some(call)));
        assert_eq!(
            *provider.active_call.lock().expect("active lock"),
            Some(call)
        );
        assert!(!provider.bind_inbound_call(&current, Some(CallId::new())));
    }

    #[test]
    fn inbound_sip_offer_mints_retained_unbridged_leg_when_agent_owns_slot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        let provider = SipGatewayProvider::with_channel(tx, SipProviderHealth::Ready);
        let call = CallId::new();
        let space = SpaceId::new();
        let offer = InboundCallOffer {
            identity: "+15551234567".to_string(),
            provider_call_id: "dlg@example.com".to_string(),
        };
        *provider.pending_inbound.lock().expect("pending lock") = Some(offer.clone());
        assert!(
            provider.bind_inbound_call(&offer, Some(call)),
            "inbound offer must bind to the minted call id"
        );

        let mut providers = empty_registry();
        providers
            .register(CallMediaAdapter::SipGateway, provider)
            .expect("register agent-owned SIP adapter");

        let mut session = ready_audio_session();
        session.call = call;
        session.space = space;
        write_readiness(
            &persist,
            &CallMediaReadiness {
                local_actor: ActorId::new("alice"),
                sessions: vec![session],
            },
        );

        let mut last_published = BTreeMap::new();
        publish_retained_call_media_verification(&persist, &mut last_published, &providers);

        let topic = media_sip_leg_topic(call);
        let msg = persist.read_latest(&topic).expect("read sip topic").expect(
            "inbound offer must mint a retained /sip document when the agent owns the slot",
        );
        let leg = SipLegV1::from_json(msg.body.as_deref().expect("sip body"))
            .expect("admit inbound sip document");
        assert_eq!(leg.session, call);
        assert_eq!(leg.direction, SipLegDirectionV1::Inbound);
        assert_eq!(leg.e164, "+15551234567");
        assert!(!leg.bridged, "inbound mint must not claim a bridged PSTN");
        assert_eq!(leg.topic(), topic);
        assert!(
            last_published.contains_key(&topic),
            "inbound sip topic must be retained in the worker publish cache"
        );
        assert!(
            persist
                .read_latest(&media_session_topic(call))
                .expect("read session topic")
                .is_none(),
            "inbound sip mint must not invent a Connected media session"
        );
    }

    #[test]
    fn registry_tick_publishes_retained_sip_leg_topic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let call = CallId::new();
        let space = SpaceId::new();

        let mut providers = empty_registry();
        providers
            .register(
                CallMediaAdapter::SipGateway,
                super::super::call_media::SipGatewayPlane::production(),
            )
            .expect("register SIP publish plane");

        providers
            .execute_command(
                &CollabCommand::StartOutboundCall {
                    space,
                    call,
                    target: "+15551234567".into(),
                },
                None,
            )
            .expect("outbound SIP is admitted by the publish plane");

        let mut session = ready_audio_session();
        session.call = call;
        session.space = space;
        write_readiness(
            &persist,
            &CallMediaReadiness {
                local_actor: ActorId::new("alice"),
                sessions: vec![session],
            },
        );

        let mut last_published = BTreeMap::new();
        publish_retained_call_media_verification(&persist, &mut last_published, &providers);

        let topic = media_sip_leg_topic(call);
        let msg = persist
            .read_latest(&topic)
            .expect("read sip topic")
            .expect("registry tick must publish the retained sip topic");
        let leg = SipLegV1::from_json(msg.body.as_deref().expect("sip body"))
            .expect("admit sip document");
        assert_eq!(leg.session, call);
        assert!(!leg.bridged, "registry tick must not invent a bridged PSTN");
        assert_eq!(leg.topic(), topic);
        assert!(
            last_published.contains_key(&topic),
            "sip topic must be retained in the worker publish cache"
        );
    }

    fn empty_registry() -> CallMediaProviderRegistry {
        CallMediaProviderRegistry::empty()
    }

    #[test]
    fn provider_admission_rejects_media_effects_but_never_blocks_cleanup() {
        let providers = empty_registry();
        let space = SpaceId::new();
        let call = CallId::new();

        for (command, existing_kind) in [
            (
                CollabCommand::StartCall {
                    space,
                    call,
                    kind: CallKind::Audio,
                },
                None,
            ),
            (CollabCommand::AnswerCall { call }, Some(CallKind::Audio)),
            (
                CollabCommand::SendDtmf { call, digit: '5' },
                Some(CallKind::Audio),
            ),
            (
                CollabCommand::SetCallMuted { call, muted: true },
                Some(CallKind::Audio),
            ),
        ] {
            assert_eq!(
                providers.admit_command(&command, existing_kind),
                Err(CallMediaCommandAdmissionError::NoProvider {
                    kind: CallKind::Audio
                })
            );
        }

        assert!(providers
            .admit_command(&CollabCommand::DeclineCall { call }, Some(CallKind::Audio))
            .is_ok());
        assert!(providers
            .admit_command(&CollabCommand::HangUpCall { call }, Some(CallKind::Audio))
            .is_ok());
    }

    #[test]
    fn provider_admission_is_scoped_to_the_registered_call_kind() {
        struct DocumentProvider;
        impl CallMediaFrameVerifier for DocumentProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                panic!("admission must not probe provider media")
            }
        }

        let mut providers = empty_registry();
        providers
            .register(CallMediaAdapter::DocumentCollab, DocumentProvider)
            .expect("register document provider");
        let space = SpaceId::new();
        let call = CallId::new();

        assert_eq!(
            providers.admit_command(
                &CollabCommand::StartCall {
                    space,
                    call,
                    kind: CallKind::Audio,
                },
                None,
            ),
            Err(CallMediaCommandAdmissionError::NoProvider {
                kind: CallKind::Audio
            })
        );
        assert!(providers
            .admit_command(
                &CollabCommand::StartCall {
                    space,
                    call,
                    kind: CallKind::CoEdit,
                },
                None,
            )
            .is_ok());
    }

    #[test]
    fn execution_is_single_provider_fail_closed_and_cleanup_survives_loss() {
        struct ExecutingProvider {
            commands: Arc<std::sync::Mutex<Vec<String>>>,
            fail: bool,
        }
        impl CallMediaFrameVerifier for ExecutingProvider {
            fn execute_command(
                &self,
                command: &CollabCommand,
                adapter: CallMediaAdapter,
            ) -> Result<(), CallMediaProviderError> {
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                self.commands
                    .lock()
                    .expect("command recorder")
                    .push(command.verb().to_string());
                if self.fail {
                    Err(CallMediaProviderError::ExecutionRefused {
                        detail: "session transport rejected command".to_string(),
                    })
                } else {
                    Ok(())
                }
            }

            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                panic!("command execution must not claim frame proof")
            }
        }

        let call = CallId::new();
        let command = CollabCommand::StartCall {
            space: SpaceId::new(),
            call,
            kind: CallKind::Audio,
        };
        let commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut providers = empty_registry();
        providers
            .register(
                CallMediaAdapter::WebRtcP2p,
                ExecutingProvider {
                    commands: Arc::clone(&commands),
                    fail: true,
                },
            )
            .expect("register executor");
        // A second compatible provider must not receive a duplicate start.
        providers
            .register(
                CallMediaAdapter::LiveKitSfu,
                ExecutingProvider {
                    commands: Arc::clone(&commands),
                    fail: false,
                },
            )
            .expect("register fallback");

        let error = providers
            .execute_command(&command, None)
            .expect_err("provider failure must refuse execution");
        assert!(matches!(
            error,
            CallMediaCommandExecutionError::ProviderFailed {
                adapter: CallMediaAdapter::WebRtcP2p,
                ..
            }
        ));
        assert_eq!(
            commands.lock().expect("commands").as_slice(),
            ["start_call"]
        );

        let absent = empty_registry();
        assert!(absent
            .execute_command(&CollabCommand::HangUpCall { call }, Some(CallKind::Audio))
            .is_ok());
        assert_eq!(
            absent.execute_command(&command, None),
            Err(CallMediaCommandExecutionError::NoProvider {
                kind: CallKind::Audio
            })
        );
    }

    #[test]
    fn remote_desktop_control_waits_for_exact_answer_and_revokes_by_call() {
        struct RecordingVdiProvider {
            commands: Arc<std::sync::Mutex<Vec<(String, CallId)>>>,
        }

        impl CallMediaFrameVerifier for RecordingVdiProvider {
            fn execute_command(
                &self,
                command: &CollabCommand,
                adapter: CallMediaAdapter,
            ) -> Result<(), CallMediaProviderError> {
                assert_eq!(adapter, CallMediaAdapter::VdiRemoteDesktop);
                let call = match command {
                    CollabCommand::AnswerCall { call } | CollabCommand::HangUpCall { call } => {
                        *call
                    }
                    other => panic!("unconsented VDI command reached provider: {other:?}"),
                };
                self.commands
                    .lock()
                    .expect("command recorder")
                    .push((command.verb().to_string(), call));
                Ok(())
            }

            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                panic!("consent routing must not claim frame proof")
            }
        }

        let space = SpaceId::new();
        let admitted_call = CallId::new();
        let substituted_call = CallId::new();
        let commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut providers = empty_registry();
        providers
            .register(
                CallMediaAdapter::VdiRemoteDesktop,
                RecordingVdiProvider {
                    commands: Arc::clone(&commands),
                },
            )
            .expect("register VDI provider");

        providers
            .execute_command(
                &CollabCommand::StartCall {
                    space,
                    call: admitted_call,
                    kind: CallKind::RemoteDesktop,
                },
                None,
            )
            .expect("signed invitation has no provider effect");
        assert!(commands.lock().expect("commands").is_empty());

        // The worker supplies existing_kind only after resolving the exact
        // signed call. A substituted id has no kind and must never reach VDI.
        providers
            .execute_command(
                &CollabCommand::AnswerCall {
                    call: substituted_call,
                },
                None,
            )
            .expect("core remains authoritative for missing calls");
        assert!(commands.lock().expect("commands").is_empty());

        providers
            .execute_command(
                &CollabCommand::AnswerCall {
                    call: admitted_call,
                },
                Some(CallKind::RemoteDesktop),
            )
            .expect("exact consent activates VDI");
        providers
            .execute_command(
                &CollabCommand::HangUpCall {
                    call: admitted_call,
                },
                Some(CallKind::RemoteDesktop),
            )
            .expect("exact hang-up revokes VDI");

        assert_eq!(
            commands.lock().expect("commands").as_slice(),
            [
                ("answer_call".to_string(), admitted_call),
                ("hang_up_call".to_string(), admitted_call),
            ]
        );
    }

    #[test]
    fn retained_verifier_reports_absent_transport_without_claiming_frames() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let readiness = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![ready_audio_session()],
        };
        write_readiness(&persist, &readiness);

        let board = verify_retained_call_media(&persist, &empty_registry()).expect("verification");

        assert_eq!(board.local_actor, ActorId::new("alice"));
        assert_eq!(board.rows.len(), 3);
        for row in &board.rows {
            assert!(row.evidence.is_none());
            assert!(row
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no ") && detail.contains("registered")));
            let expected = match row.adapter {
                CallMediaAdapter::WebRtcP2p => CallMediaVerificationStatus::TransportUnavailable,
                CallMediaAdapter::LiveKitSfu | CallMediaAdapter::SipGateway => {
                    CallMediaVerificationStatus::ProviderUnavailable
                }
                CallMediaAdapter::DocumentCollab | CallMediaAdapter::VdiRemoteDesktop => {
                    panic!("audio readiness should not nominate {:?}", row.adapter)
                }
            };
            assert_eq!(row.status, expected);
        }
    }

    #[test]
    fn publisher_uses_registered_provider_to_prove_advancing_frames() {
        struct AdvancingAudioProvider;
        impl CallMediaFrameVerifier for AdvancingAudioProvider {
            fn prove_advancing_frames(
                &self,
                session: &CallMediaSession,
                adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                assert_eq!(session.admission, CallMediaAdmission::AdapterReady);
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                Ok(CallMediaFrameEvidence {
                    audio_frames: 7,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let readiness = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![ready_audio_session()],
        };
        write_readiness(&persist, &readiness);

        let mut providers = CallMediaProviderRegistry::empty();
        providers
            .register(CallMediaAdapter::WebRtcP2p, AdvancingAudioProvider)
            .expect("register WebRTC proof provider");
        let mut last_published = BTreeMap::new();

        publish_retained_call_media_verification(&persist, &mut last_published, &providers);

        let msg = persist
            .read_latest(&topics::state_topic(proj::CALL_MEDIA_VERIFICATION))
            .expect("read verification")
            .expect("verification published");
        let board: CallMediaVerification =
            serde_json::from_str(msg.body.as_deref().expect("body")).expect("decode board");

        assert_eq!(board.rows.len(), 3);
        let webrtc = board
            .rows
            .iter()
            .find(|row| row.adapter == CallMediaAdapter::WebRtcP2p)
            .expect("WebRTC row");
        assert_eq!(
            webrtc.status,
            CallMediaVerificationStatus::LiveMediaVerified
        );
        assert_eq!(
            webrtc.evidence,
            Some(CallMediaFrameEvidence {
                audio_frames: 7,
                video_frames: 0,
                screen_frames: 0,
                data_messages: 0,
            })
        );
        assert!(webrtc.detail.is_none());

        for adapter in [CallMediaAdapter::LiveKitSfu, CallMediaAdapter::SipGateway] {
            let row = board
                .rows
                .iter()
                .find(|row| row.adapter == adapter)
                .expect("missing provider row");
            assert_eq!(row.status, CallMediaVerificationStatus::ProviderUnavailable);
            assert!(row.evidence.is_none());
            assert!(
                row.detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("registered")),
                "{adapter:?} must fail honestly when no provider is registered"
            );
        }
    }

    #[test]
    fn unchanged_readiness_is_reprobed_across_revocation_and_reconnect() {
        struct LifecycleProvider {
            probes: Arc<AtomicUsize>,
        }

        impl CallMediaFrameVerifier for LifecycleProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                match self.probes.fetch_add(1, Ordering::SeqCst) {
                    0 | 2 => Ok(CallMediaFrameEvidence {
                        audio_frames: 4,
                        video_frames: 0,
                        screen_frames: 0,
                        data_messages: 0,
                    }),
                    1 => Err(CallMediaProviderError::TransportUnavailable {
                        detail: "provider session was revoked".to_string(),
                    }),
                    probe => panic!("unexpected provider probe {probe}"),
                }
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let mut session = ready_audio_session();
        session.candidate_adapters = vec![CallMediaAdapter::WebRtcP2p];
        write_readiness(
            &persist,
            &CallMediaReadiness {
                local_actor: ActorId::new("alice"),
                sessions: vec![session],
            },
        );

        let probes = Arc::new(AtomicUsize::new(0));
        let mut providers = CallMediaProviderRegistry::empty();
        providers
            .register(
                CallMediaAdapter::WebRtcP2p,
                LifecycleProvider {
                    probes: Arc::clone(&probes),
                },
            )
            .expect("register lifecycle provider");
        let mut last_published = BTreeMap::new();

        let read_status = || {
            let msg = persist
                .read_latest(&topics::state_topic(proj::CALL_MEDIA_VERIFICATION))
                .expect("read verification")
                .expect("verification published");
            serde_json::from_str::<CallMediaVerification>(
                msg.body.as_deref().expect("verification body"),
            )
            .expect("decode verification")
            .rows
            .into_iter()
            .next()
            .expect("verification row")
        };

        publish_retained_call_media_verification(&persist, &mut last_published, &providers);
        let live = read_status();
        assert_eq!(live.status, CallMediaVerificationStatus::LiveMediaVerified);
        assert!(live.evidence.is_some());

        // The readiness body is deliberately unchanged. A provider-side
        // revocation must still replace the stale live row and clear evidence.
        publish_retained_call_media_verification(&persist, &mut last_published, &providers);
        let revoked = read_status();
        assert_eq!(
            revoked.status,
            CallMediaVerificationStatus::TransportUnavailable
        );
        assert!(revoked.evidence.is_none());
        assert_eq!(
            revoked.detail.as_deref(),
            Some("provider session was revoked")
        );

        // Reconnect also needs no synthetic call mutation: the same retained
        // readiness is sampled again and advancing frames restore live state.
        publish_retained_call_media_verification(&persist, &mut last_published, &providers);
        let reconnected = read_status();
        assert_eq!(
            reconnected.status,
            CallMediaVerificationStatus::LiveMediaVerified
        );
        assert!(reconnected.evidence.is_some());
        assert_eq!(probes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn verifier_waits_for_connected_peer_before_transport_probe() {
        struct PanicProvider;
        impl CallMediaFrameVerifier for PanicProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                panic!("waiting readiness must not touch media provider");
            }
        }

        let mut providers = CallMediaProviderRegistry::empty();
        for adapter in [
            CallMediaAdapter::WebRtcP2p,
            CallMediaAdapter::LiveKitSfu,
            CallMediaAdapter::SipGateway,
        ] {
            providers
                .register(adapter, PanicProvider)
                .expect("register panic provider");
        }

        let mut session = ready_audio_session();
        session.admission = CallMediaAdmission::WaitingForConnectedPeer;
        session.connected_participants = vec![ActorId::new("alice")];
        let readiness = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![session],
        };

        let board =
            verify_call_media_readiness(&readiness, &providers).expect("verification board");

        assert_eq!(board.rows.len(), 3);
        assert!(board.rows.iter().all(|row| {
            row.status == CallMediaVerificationStatus::WaitingForConnectedPeer
                && row.evidence.is_none()
        }));
    }

    #[test]
    fn verifier_rejects_missing_required_frames() {
        struct AudioOnlyProvider;
        impl CallMediaFrameVerifier for AudioOnlyProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                Ok(CallMediaFrameEvidence {
                    audio_frames: 12,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
        }

        let mut session = ready_audio_session();
        session.kind = CallKind::Video;
        session.requirements = vec![
            CallMediaRequirement::Microphone,
            CallMediaRequirement::Camera,
        ];
        session.candidate_adapters = vec![CallMediaAdapter::WebRtcP2p];
        let readiness = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![session],
        };
        let mut providers = CallMediaProviderRegistry::empty();
        providers
            .register(CallMediaAdapter::WebRtcP2p, AudioOnlyProvider)
            .expect("register audio provider");

        let board =
            verify_call_media_readiness(&readiness, &providers).expect("verification board");

        assert_eq!(board.rows.len(), 1);
        assert_eq!(
            board.rows[0].status,
            CallMediaVerificationStatus::MediaNotProven
        );
        assert!(board.rows[0].evidence.is_none());
        assert!(board.rows[0]
            .detail
            .as_deref()
            .expect("detail")
            .contains("video frames"));
    }

    #[test]
    fn verifier_refuses_misattributed_or_vacuous_provider_evidence() {
        struct HostileProvider;
        impl CallMediaFrameVerifier for HostileProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                panic!("invalid readiness must not consume provider evidence");
            }
        }

        let mut providers = empty_registry();
        providers
            .register(CallMediaAdapter::DocumentCollab, HostileProvider)
            .expect("register hostile document provider");
        providers
            .register(CallMediaAdapter::WebRtcP2p, HostileProvider)
            .expect("register hostile WebRTC provider");

        let mut wrong_adapter = ready_audio_session();
        wrong_adapter.candidate_adapters = vec![CallMediaAdapter::DocumentCollab];
        let mut vacuous_requirements = ready_audio_session();
        vacuous_requirements.requirements.clear();
        vacuous_requirements.candidate_adapters = vec![CallMediaAdapter::WebRtcP2p];
        let readiness = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![wrong_adapter, vacuous_requirements],
        };

        let board =
            verify_call_media_readiness(&readiness, &providers).expect("verification board");

        assert_eq!(board.rows.len(), 2);
        assert!(board.rows.iter().all(|row| {
            row.status == CallMediaVerificationStatus::MediaNotProven
                && row.evidence.is_none()
                && row
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("do not match"))
        }));
    }

    #[test]
    fn verifier_refuses_duplicate_candidate_attribution_before_provider_probe() {
        struct HostileProvider;
        impl CallMediaFrameVerifier for HostileProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                panic!("duplicate candidate readiness must not consume provider evidence");
            }
        }

        let mut session = ready_audio_session();
        session.candidate_adapters = vec![CallMediaAdapter::WebRtcP2p, CallMediaAdapter::WebRtcP2p];
        let readiness = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![session],
        };
        let mut providers = empty_registry();
        providers
            .register(CallMediaAdapter::WebRtcP2p, HostileProvider)
            .expect("register hostile WebRTC provider");

        let board = verify_call_media_readiness(&readiness, &providers)
            .expect("duplicate readiness should be represented as failed proof");

        assert_eq!(board.rows.len(), 2);
        assert!(board.rows.iter().all(|row| {
            row.status == CallMediaVerificationStatus::MediaNotProven
                && row.evidence.is_none()
                && row
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("do not match"))
        }));
    }

    #[test]
    fn corrupt_readiness_after_restart_revokes_stale_live_media_proof() {
        struct AdvancingProvider;
        impl CallMediaFrameVerifier for AdvancingProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                Ok(CallMediaFrameEvidence {
                    audio_frames: 4,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let mut session = ready_audio_session();
        session.candidate_adapters = vec![CallMediaAdapter::WebRtcP2p];
        let readiness = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![session],
        };
        write_readiness(&persist, &readiness);

        let mut providers = empty_registry();
        providers
            .register(CallMediaAdapter::WebRtcP2p, AdvancingProvider)
            .expect("register provider");
        let mut before_restart = BTreeMap::new();
        publish_retained_call_media_verification(&persist, &mut before_restart, &providers);
        let verification_topic = topics::state_topic(proj::CALL_MEDIA_VERIFICATION);
        let live = persist
            .read_latest(&verification_topic)
            .expect("read live verification")
            .expect("live verification");
        let live: CallMediaVerification =
            serde_json::from_str(live.body.as_deref().expect("live body"))
                .expect("decode live verification");
        assert_eq!(
            live.rows[0].status,
            CallMediaVerificationStatus::LiveMediaVerified
        );

        persist
            .write(
                &topics::state_topic(proj::CALL_MEDIA_READINESS),
                Priority::Default,
                None,
                Some("{corrupt"),
            )
            .expect("corrupt retained readiness");

        // A fresh cache models daemon restart. The stale retained live board
        // must still be revoked even though this process did not publish it.
        let mut after_restart = BTreeMap::new();
        publish_retained_call_media_verification(&persist, &mut after_restart, &providers);
        let revoked = persist
            .read_latest(&verification_topic)
            .expect("read revoked verification")
            .expect("verification tombstone");
        assert!(
            revoked.body.is_none(),
            "invalid readiness must tombstone stale live provider proof"
        );

        // Corrected-forward readiness must leave the tombstone state and be
        // sampled normally without requiring another daemon restart.
        write_readiness(&persist, &readiness);
        publish_retained_call_media_verification(&persist, &mut after_restart, &providers);
        let repaired = persist
            .read_latest(&verification_topic)
            .expect("read repaired verification")
            .expect("repaired verification");
        let repaired: CallMediaVerification =
            serde_json::from_str(repaired.body.as_deref().expect("repaired body"))
                .expect("decode repaired verification");
        assert_eq!(
            repaired.rows[0].status,
            CallMediaVerificationStatus::LiveMediaVerified
        );
    }

    #[test]
    fn invalid_session_provenance_revokes_live_proof_without_provider_probe() {
        struct CountingProvider {
            probes: Arc<AtomicUsize>,
        }

        impl CallMediaFrameVerifier for CountingProvider {
            fn prove_advancing_frames(
                &self,
                _session: &CallMediaSession,
                adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                self.probes.fetch_add(1, Ordering::SeqCst);
                Ok(CallMediaFrameEvidence {
                    audio_frames: 4,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let mut session = ready_audio_session();
        session.candidate_adapters = vec![CallMediaAdapter::WebRtcP2p];
        let valid = CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![session.clone()],
        };
        write_readiness(&persist, &valid);

        let probes = Arc::new(AtomicUsize::new(0));
        let mut providers = empty_registry();
        providers
            .register(
                CallMediaAdapter::WebRtcP2p,
                CountingProvider {
                    probes: Arc::clone(&probes),
                },
            )
            .expect("register provider");
        let mut last_published = BTreeMap::new();
        publish_retained_call_media_verification(&persist, &mut last_published, &providers);
        assert_eq!(probes.load(Ordering::SeqCst), 1);

        session.connected_participants = vec![ActorId::new("mallory"), ActorId::new("bob")];
        write_readiness(
            &persist,
            &CallMediaReadiness {
                local_actor: ActorId::new("alice"),
                sessions: vec![session],
            },
        );
        publish_retained_call_media_verification(&persist, &mut last_published, &providers);

        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "misattributed readiness must not reach the media provider"
        );
        let verification = persist
            .read_latest(&topics::state_topic(proj::CALL_MEDIA_VERIFICATION))
            .expect("read verification")
            .expect("verification tombstone");
        assert!(
            verification.body.is_none(),
            "misattributed readiness must revoke stale live proof"
        );
    }
}

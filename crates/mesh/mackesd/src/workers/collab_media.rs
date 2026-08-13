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

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_collab_types::topics::{self, projection as proj};
use mde_collab_types::{
    CallKind, CallMediaAdapter, CallMediaAdmission, CallMediaFrameEvidence, CallMediaReadiness,
    CallMediaRequirement, CallMediaSession, CallMediaVerification, CallMediaVerificationRow,
    CallMediaVerificationStatus, CollabCommand,
};

const MAX_READINESS_BODY_BYTES: usize = 256 * 1024;
const MAX_VERIFICATION_SESSIONS: usize = 256;
const MAX_VERIFICATION_ROWS: usize = 1024;
const MAX_DETAIL_BYTES: usize = 512;
// Private cache marker for a retained verification tombstone. This is not a
// Bus payload and cannot collide with the serialized verification object.
const VERIFICATION_UNAVAILABLE: &str = "\0call-media-verification-unavailable";

pub(super) fn publish_retained_call_media_verification(
    persist: &Persist,
    last_published: &mut BTreeMap<String, String>,
    providers: &CallMediaProviderRegistry,
) {
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
    for session in &readiness.sessions {
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
                &[
                    CallMediaAdapter::WebRtcP2p,
                    CallMediaAdapter::LiveKitSfu,
                ],
            ),
            CallKind::Screen => (
                &[
                    CallMediaRequirement::Microphone,
                    CallMediaRequirement::ScreenCapture,
                ],
                &[
                    CallMediaAdapter::WebRtcP2p,
                    CallMediaAdapter::LiveKitSfu,
                ],
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
    /// Prove live media for `session` by returning frame/data deltas observed
    /// during a bounded provider-owned sampling window. Cumulative counters or
    /// desired-state readiness are not sufficient for a
    /// [`CallMediaVerificationStatus::LiveMediaVerified`] row.
    fn prove_advancing_frames(
        &self,
        session: &CallMediaSession,
        adapter: CallMediaAdapter,
    ) -> Result<CallMediaFrameEvidence, CallMediaProviderError>;
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
        if self.supports(kind) {
            Ok(())
        } else {
            Err(CallMediaCommandAdmissionError::NoProvider { kind })
        }
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

pub(crate) enum CallMediaProviderError {
    TransportUnavailable { detail: String },
    ProviderUnavailable { detail: String },
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
    use mde_collab_types::{ActorId, CallId, CallKind, SpaceId};
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
        session.candidate_adapters = vec![
            CallMediaAdapter::WebRtcP2p,
            CallMediaAdapter::WebRtcP2p,
        ];
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
}

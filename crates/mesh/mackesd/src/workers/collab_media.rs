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
    CallMediaAdapter, CallMediaAdmission, CallMediaFrameEvidence, CallMediaReadiness,
    CallMediaRequirement, CallMediaSession, CallMediaVerification, CallMediaVerificationRow,
    CallMediaVerificationStatus,
};

const MAX_READINESS_BODY_BYTES: usize = 256 * 1024;
const MAX_VERIFICATION_SESSIONS: usize = 256;
const MAX_VERIFICATION_ROWS: usize = 1024;
const MAX_DETAIL_BYTES: usize = 512;

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
            tracing::debug!(target: "mackesd::collab", topic, error = %e, "collab media verification unavailable")
        }
    }
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
}

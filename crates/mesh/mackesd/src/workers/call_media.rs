//! WL-FUNC-024 S2/S3 — WebRTC P2P and elected LiveKit SFU media planes.
//!
//! There is no WebRTC stack in this workspace. This worker is therefore a real
//! offer/answer state machine over existing collab call signaling, a seat-audio
//! bind, and an injectable loopback frame callback. It never publishes
//! [`mde_collab_types::MediaSessionStateV1::Connected`] unless the loopback seam
//! (or a future transport) observes advancing frames. Device absence and
//! permission denial publish the typed unavailable states.
//!
//! Mute and DTMF act on the bound live leg. The collab media verifier remains a
//! separate proof sidecar; this module is the P2P plane it can sample.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_collab_types::{
    media_answer_topic, media_offer_topic, media_session_topic, media_sfu_election_topic, ActorId,
    CallId, CallKind, CallMediaAdapter, CallMediaAdmission, CallMediaFrameEvidence,
    CallMediaReadiness, CallMediaSession, CollabCommand, MediaDescriptionV1, MediaFailureReasonV1,
    MediaSessionStateV1, MediaSessionV1, MediaSignalingRoleV1, MediaTrackKind, SfuElectionV1,
    SpaceId, MEDIA_SESSION_V1_SCHEMA_VERSION,
};

use super::collab_media::{CallMediaFrameVerifier, CallMediaProviderError};

const MAX_READINESS_BODY_BYTES: usize = 256 * 1024;
const MAX_SIGNAL_BODY_BYTES: usize = 8 * 1024;
const MAX_SESSIONS: usize = 32;

/// One PCM-sized chirp frame used by the injectable loopback seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MediaPcmFrame {
    seq: u64,
    /// Tiny PCM payload. Mute sends zeros; DTMF uses a non-zero marker.
    pcm16: [i16; 8],
}

impl MediaPcmFrame {
    fn chirp(seq: u64) -> Self {
        Self {
            seq,
            pcm16: [1, 2, 3, 4, 5, 6, 7, 8],
        }
    }

    fn silence(seq: u64) -> Self {
        Self { seq, pcm16: [0; 8] }
    }

    fn dtmf(seq: u64, digit: char) -> Self {
        let mark = i16::from(u8::try_from(digit).unwrap_or(0));
        Self {
            seq,
            pcm16: [mark, 9, 9, 9, 9, 9, 9, 9],
        }
    }

    fn is_silence(self) -> bool {
        self.pcm16.iter().all(|sample| *sample == 0)
    }
}

/// Why seat audio could not be bound. These map 1:1 onto typed session states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeatAudioBindError {
    DeviceAbsent,
    PermissionDenied,
}

/// Seat capture/playback bind. Production probes ALSA nodes honestly; tests
/// inject a fixed outcome. Binding is not proof of advancing frames.
trait SeatAudioSource: Send + Sync {
    fn bind(&self) -> Result<SeatAudioBinding, SeatAudioBindError>;
}

/// A bound local audio leg mute bit. DTMF and mute act here when present.
#[derive(Debug)]
struct SeatAudioBinding {
    muted: AtomicBool,
}

impl SeatAudioBinding {
    #[must_use]
    fn new() -> Self {
        Self {
            muted: AtomicBool::new(false),
        }
    }

    fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::SeqCst);
    }
}

/// Injectable loopback/chirp seam. Production leaves this unset so the plane
/// cannot invent Connected. Tests inject a shared callback to prove frames.
trait LoopbackFrameCallback: Send + Sync {
    /// Deliver `outbound` from `from` and optionally return the peer's latest
    /// inbound frame. `None` is honest silence, not a connected proof.
    fn on_frame(
        &self,
        session: CallId,
        from: &ActorId,
        outbound: MediaPcmFrame,
    ) -> Option<MediaPcmFrame>;
}

/// Shared in-process loopback used by two-seat fixtures.
#[cfg(test)]
#[derive(Debug, Default)]
struct SharedLoopback {
    last: Mutex<BTreeMap<(CallId, String), MediaPcmFrame>>,
    dtmf: Mutex<BTreeMap<(CallId, String), Vec<char>>>,
}

#[cfg(test)]
impl SharedLoopback {
    #[must_use]
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn dtmf_received(&self, session: CallId, from: &ActorId) -> Vec<char> {
        self.dtmf
            .lock()
            .map(|guard| {
                guard
                    .get(&(session, from.as_str().to_string()))
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl LoopbackFrameCallback for SharedLoopback {
    fn on_frame(
        &self,
        session: CallId,
        from: &ActorId,
        outbound: MediaPcmFrame,
    ) -> Option<MediaPcmFrame> {
        if !outbound.is_silence() && outbound.pcm16[1] == 9 {
            if let Ok(mut dtmf) = self.dtmf.lock() {
                dtmf.entry((session, from.as_str().to_string()))
                    .or_default()
                    .push(char::from(u8::try_from(outbound.pcm16[0]).unwrap_or(0)));
            }
        }
        let mut last = self.last.lock().ok()?;
        last.insert((session, from.as_str().to_string()), outbound);
        last.iter().find_map(|((stored_session, actor), frame)| {
            (*stored_session == session && actor != from.as_str()).then_some(*frame)
        })
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct GroupSfuLoopback {
    host: Mutex<BTreeMap<CallId, String>>,
    last: Mutex<BTreeMap<(CallId, String), MediaPcmFrame>>,
    dtmf: Mutex<BTreeMap<(CallId, String), Vec<char>>>,
}

#[cfg(test)]
impl GroupSfuLoopback {
    #[must_use]
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn set_host(&self, call: CallId, host: &ActorId) {
        if let Ok(mut guard) = self.host.lock() {
            guard.insert(call, host.as_str().to_string());
        }
    }

    fn dtmf_received(&self, session: CallId, from: &ActorId) -> Vec<char> {
        self.dtmf
            .lock()
            .map(|guard| {
                guard
                    .get(&(session, from.as_str().to_string()))
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl LoopbackFrameCallback for GroupSfuLoopback {
    fn on_frame(
        &self,
        session: CallId,
        from: &ActorId,
        outbound: MediaPcmFrame,
    ) -> Option<MediaPcmFrame> {
        if !outbound.is_silence() && outbound.pcm16[1] == 9 {
            if let Ok(mut dtmf) = self.dtmf.lock() {
                dtmf.entry((session, from.as_str().to_string()))
                    .or_default()
                    .push(char::from(u8::try_from(outbound.pcm16[0]).unwrap_or(0)));
            }
        }
        let host = self.host.lock().ok()?.get(&session).cloned()?;
        let mut last = self.last.lock().ok()?;
        last.insert((session, from.as_str().to_string()), outbound);
        if from.as_str() == host {
            last.iter()
                .find_map(|((stored_session, actor), frame)| {
                    (*stored_session == session && actor.as_str() != host).then_some(*frame)
                })
                .or(Some(outbound))
        } else {
            last.get(&(session, host.clone())).copied()
        }
    }
}

/// Fixed bind outcome for tests.
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct FixedSeatAudio {
    outcome: Result<(), SeatAudioBindError>,
}

#[cfg(test)]
impl FixedSeatAudio {
    #[must_use]
    fn present() -> Arc<Self> {
        Arc::new(Self { outcome: Ok(()) })
    }

    #[must_use]
    fn absent() -> Arc<Self> {
        Arc::new(Self {
            outcome: Err(SeatAudioBindError::DeviceAbsent),
        })
    }

    #[must_use]
    fn denied() -> Arc<Self> {
        Arc::new(Self {
            outcome: Err(SeatAudioBindError::PermissionDenied),
        })
    }
}

#[cfg(test)]
impl SeatAudioSource for FixedSeatAudio {
    fn bind(&self) -> Result<SeatAudioBinding, SeatAudioBindError> {
        self.outcome.map(|()| SeatAudioBinding::new())
    }
}

/// Production seat-audio probe. Looks for an ALSA capture node; does not open a
/// WebRTC track and therefore never claims live frames by itself.
struct AlsaSeatAudio;

impl SeatAudioSource for AlsaSeatAudio {
    fn bind(&self) -> Result<SeatAudioBinding, SeatAudioBindError> {
        match probe_capture_device() {
            CaptureProbe::Absent => Err(SeatAudioBindError::DeviceAbsent),
            CaptureProbe::PermissionDenied => Err(SeatAudioBindError::PermissionDenied),
            CaptureProbe::Present => Ok(SeatAudioBinding::new()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureProbe {
    Absent,
    PermissionDenied,
    Present,
}

fn probe_capture_device() -> CaptureProbe {
    let Ok(entries) = std::fs::read_dir("/dev/snd") else {
        return CaptureProbe::Absent;
    };
    let mut saw_capture = false;
    let mut denied = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("pcm") || !name.ends_with('c') {
            continue;
        }
        saw_capture = true;
        match std::fs::File::open(entry.path()) {
            Ok(_) => return CaptureProbe::Present,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                denied = true;
            }
            Err(_) => {}
        }
    }
    if denied {
        CaptureProbe::PermissionDenied
    } else if saw_capture {
        CaptureProbe::PermissionDenied
    } else {
        CaptureProbe::Absent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalingRole {
    Offer,
    Answer,
}

struct LiveSession {
    space: SpaceId,
    local_actor: ActorId,
    remote_actor: ActorId,
    role: SignalingRole,
    offered_tracks: Vec<MediaTrackKind>,
    state: MediaSessionStateV1,
    local_muted: bool,
    audio: Option<SeatAudioBinding>,
    local_description: Option<MediaDescriptionV1>,
    remote_description: Option<MediaDescriptionV1>,
    frames_observed: u64,
}

impl LiveSession {
    fn audio_bound(&self) -> bool {
        self.audio.is_some()
    }

    fn dtmf_bound(&self) -> bool {
        self.audio.is_some()
    }

    fn document(&self, call: CallId) -> Result<MediaSessionV1, String> {
        MediaSessionV1::new(
            call,
            self.space,
            self.local_actor.clone(),
            self.remote_actor.clone(),
            CallMediaAdapter::WebRtcP2p,
            self.state.clone(),
            self.offered_tracks.clone(),
            self.local_muted,
            self.dtmf_bound(),
            self.audio_bound(),
            self.frames_observed,
            self.local_description.clone(),
            self.remote_description.clone(),
        )
        .map_err(|error| error.to_string())
    }
}

struct PlaneInner {
    local_actor: Option<ActorId>,
    sessions: BTreeMap<CallId, LiveSession>,
}

/// The one-to-one P2P media worker. Registered as the WebRTC P2P verifier so
/// mute/DTMF/start/answer execute on this live plane.
pub(crate) struct WebrtcP2pPlane {
    inner: Mutex<PlaneInner>,
    audio: Arc<dyn SeatAudioSource>,
    loopback: Option<Arc<dyn LoopbackFrameCallback>>,
    frame_counter: AtomicU64,
}

impl WebrtcP2pPlane {
    #[must_use]
    pub(crate) fn production() -> Self {
        Self::new(Arc::new(AlsaSeatAudio), None)
    }

    #[must_use]
    fn new(
        audio: Arc<dyn SeatAudioSource>,
        loopback: Option<Arc<dyn LoopbackFrameCallback>>,
    ) -> Self {
        Self {
            inner: Mutex::new(PlaneInner {
                local_actor: None,
                sessions: BTreeMap::new(),
            }),
            audio,
            loopback,
            frame_counter: AtomicU64::new(1),
        }
    }

    #[cfg(test)]
    fn with_local_actor(self, actor: ActorId) -> Self {
        if let Ok(mut inner) = self.inner.lock() {
            inner.local_actor = Some(actor);
        }
        self
    }

    fn lock_inner(&self) -> Result<std::sync::MutexGuard<'_, PlaneInner>, CallMediaProviderError> {
        self.inner
            .lock()
            .map_err(|_| CallMediaProviderError::ProviderUnavailable {
                detail: "P2P media plane lock is unavailable".to_string(),
            })
    }

    fn ensure_session(
        inner: &mut PlaneInner,
        call: CallId,
        role: SignalingRole,
    ) -> &mut LiveSession {
        inner.sessions.entry(call).or_insert_with(|| LiveSession {
            space: SpaceId::nil(),
            local_actor: inner
                .local_actor
                .clone()
                .unwrap_or_else(|| ActorId::new("local")),
            remote_actor: ActorId::new("peer"),
            role,
            offered_tracks: vec![MediaTrackKind::Audio],
            state: MediaSessionStateV1::Negotiating,
            local_muted: false,
            audio: None,
            local_description: None,
            remote_description: None,
            frames_observed: 0,
        })
    }

    fn apply_bind(session: &mut LiveSession, audio: &Arc<dyn SeatAudioSource>) {
        if session.audio.is_some()
            || matches!(
                session.state,
                MediaSessionStateV1::DeviceAbsent { .. }
                    | MediaSessionStateV1::PermissionDenied { .. }
            )
        {
            return;
        }
        match audio.bind() {
            Ok(binding) => {
                binding.set_muted(session.local_muted);
                session.audio = Some(binding);
            }
            Err(SeatAudioBindError::DeviceAbsent) => {
                session.state = MediaSessionStateV1::DeviceAbsent {
                    track: MediaTrackKind::Audio,
                };
                session.audio = None;
            }
            Err(SeatAudioBindError::PermissionDenied) => {
                session.state = MediaSessionStateV1::PermissionDenied {
                    track: MediaTrackKind::Audio,
                };
                session.audio = None;
            }
        }
    }

    fn negotiate(
        persist: &Persist,
        session: &mut LiveSession,
        call: CallId,
    ) -> Result<(), CallMediaProviderError> {
        if session.space.is_nil() {
            return Ok(());
        }
        match session.role {
            SignalingRole::Offer => {
                if session.local_description.is_none() {
                    let offer = MediaDescriptionV1::new(
                        call,
                        session.local_actor.clone(),
                        session.remote_actor.clone(),
                        MediaSignalingRoleV1::Offer,
                        session.offered_tracks.clone(),
                    )
                    .map_err(|error| {
                        CallMediaProviderError::ExecutionRefused {
                            detail: error.to_string(),
                        }
                    })?;
                    publish_json(persist, &media_offer_topic(call), &offer)?;
                    session.local_description = Some(offer);
                }
                if session.remote_description.is_none() {
                    if let Some(answer) = read_description(persist, &media_answer_topic(call))? {
                        if answer.role != MediaSignalingRoleV1::Answer
                            || answer.session != call
                            || answer.from != session.remote_actor
                            || answer.to != session.local_actor
                            || answer.tracks != session.offered_tracks
                        {
                            session.state = MediaSessionStateV1::Failed {
                                reason: MediaFailureReasonV1::InvalidSignaling,
                            };
                            return Ok(());
                        }
                        session.remote_description = Some(answer);
                    }
                }
            }
            SignalingRole::Answer => {
                if session.remote_description.is_none() {
                    if let Some(offer) = read_description(persist, &media_offer_topic(call))? {
                        if offer.role != MediaSignalingRoleV1::Offer
                            || offer.session != call
                            || offer.from != session.remote_actor
                            || offer.to != session.local_actor
                            || offer.tracks != session.offered_tracks
                        {
                            session.state = MediaSessionStateV1::Failed {
                                reason: MediaFailureReasonV1::InvalidSignaling,
                            };
                            return Ok(());
                        }
                        session.remote_description = Some(offer);
                    }
                }
                if session.remote_description.is_some() && session.local_description.is_none() {
                    let answer = MediaDescriptionV1::new(
                        call,
                        session.local_actor.clone(),
                        session.remote_actor.clone(),
                        MediaSignalingRoleV1::Answer,
                        session.offered_tracks.clone(),
                    )
                    .map_err(|error| {
                        CallMediaProviderError::ExecutionRefused {
                            detail: error.to_string(),
                        }
                    })?;
                    publish_json(persist, &media_answer_topic(call), &answer)?;
                    session.local_description = Some(answer);
                }
            }
        }
        Ok(())
    }

    fn pump_loopback(&self, session: &mut LiveSession, call: CallId) {
        if session.audio.is_none() {
            return;
        }
        if matches!(
            session.state,
            MediaSessionStateV1::DeviceAbsent { .. } | MediaSessionStateV1::PermissionDenied { .. }
        ) {
            return;
        }
        let Some(loopback) = &self.loopback else {
            if session.local_description.is_some()
                && session.remote_description.is_some()
                && !session.state.claims_live_media()
            {
                session.state = MediaSessionStateV1::Failed {
                    reason: MediaFailureReasonV1::TransportUnavailable,
                };
            }
            return;
        };
        if session.local_description.is_none() || session.remote_description.is_none() {
            return;
        }
        let seq = self.frame_counter.fetch_add(1, Ordering::SeqCst);
        let outbound = if session.local_muted {
            MediaPcmFrame::silence(seq)
        } else {
            MediaPcmFrame::chirp(seq)
        };
        if loopback
            .on_frame(call, &session.local_actor, outbound)
            .is_some()
        {
            session.frames_observed = session.frames_observed.saturating_add(1);
            if session.frames_observed > 0 && session.audio_bound() {
                session.state = MediaSessionStateV1::Connected;
            }
        }
    }

    fn tick_locked(
        &self,
        persist: &Persist,
        inner: &mut PlaneInner,
        last_published: &mut BTreeMap<String, String>,
    ) {
        let Ok(readiness) = read_readiness(persist) else {
            return;
        };
        inner.local_actor = Some(readiness.local_actor.clone());
        let live_calls: Vec<CallId> = readiness
            .sessions
            .iter()
            .filter(|session| {
                is_media_call_kind(session.kind)
                    && session.admission == CallMediaAdmission::AdapterReady
                    && session
                        .connected_participants
                        .iter()
                        .any(|actor| actor == &readiness.local_actor)
            })
            .map(|session| session.call)
            .collect();
        inner.sessions.retain(|call, _| live_calls.contains(call));

        for ready in readiness
            .sessions
            .iter()
            .filter(|session| live_calls.contains(&session.call))
            .take(MAX_SESSIONS)
        {
            let Some(remote) = unique_remote(&readiness.local_actor, &ready.connected_participants)
            else {
                if is_group_call(&ready.connected_participants) {
                    inner.sessions.remove(&ready.call);
                }
                continue;
            };
            let role = inner
                .sessions
                .get(&ready.call)
                .map(|session| session.role)
                .unwrap_or_else(|| {
                    if readiness.local_actor.as_str() <= remote.as_str() {
                        SignalingRole::Offer
                    } else {
                        SignalingRole::Answer
                    }
                });
            let session = inner
                .sessions
                .entry(ready.call)
                .or_insert_with(|| LiveSession {
                    space: ready.space,
                    local_actor: readiness.local_actor.clone(),
                    remote_actor: remote.clone(),
                    role,
                    offered_tracks: tracks_for_call_kind(ready.kind),
                    state: MediaSessionStateV1::Negotiating,
                    local_muted: ready.local_muted,
                    audio: None,
                    local_description: None,
                    remote_description: None,
                    frames_observed: 0,
                });
            session.space = ready.space;
            session.local_actor = readiness.local_actor.clone();
            session.remote_actor = remote;
            session.offered_tracks = tracks_for_call_kind(ready.kind);
            if session.audio.is_none() {
                session.local_muted = ready.local_muted;
            }
            Self::apply_bind(session, &self.audio);
            if let Err(error) = Self::negotiate(persist, session, ready.call) {
                tracing::debug!(
                    target: "mackesd::call_media",
                    error = ?error,
                    "P2P offer/answer publish failed"
                );
            }
            self.pump_loopback(session, ready.call);
            match session.document(ready.call) {
                Ok(document) => {
                    let _ = publish_media_session(persist, last_published, &document);
                }
                Err(error) => tracing::debug!(
                    target: "mackesd::call_media",
                    error = %error,
                    "refusing to publish invalid media session"
                ),
            }
        }
    }

    fn tick(&self, persist: &Persist, last_published: &mut BTreeMap<String, String>) {
        let Ok(mut inner) = self.lock_inner() else {
            return;
        };
        self.tick_locked(persist, &mut inner, last_published);
    }
}

impl CallMediaFrameVerifier for WebrtcP2pPlane {
    fn execute_command(
        &self,
        command: &CollabCommand,
        adapter: CallMediaAdapter,
    ) -> Result<(), CallMediaProviderError> {
        if adapter != CallMediaAdapter::WebRtcP2p {
            return Err(CallMediaProviderError::ExecutionRefused {
                detail: "P2P plane was selected for an incompatible adapter".to_string(),
            });
        }
        let mut inner = self.lock_inner()?;
        match command {
            CollabCommand::StartCall { call, kind, .. } => {
                let session =
                    WebrtcP2pPlane::ensure_session(&mut inner, *call, SignalingRole::Offer);
                session.offered_tracks = tracks_for_call_kind(*kind);
                session.state = MediaSessionStateV1::Negotiating;
                Ok(())
            }
            CollabCommand::AnswerCall { call } => {
                WebrtcP2pPlane::ensure_session(&mut inner, *call, SignalingRole::Answer);
                Ok(())
            }
            CollabCommand::SetCallMuted { call, muted } => {
                let Some(session) = inner.sessions.get_mut(call) else {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "mute requires a bound P2P media leg".to_string(),
                    });
                };
                let Some(audio) = session.audio.as_ref() else {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "mute requires a bound seat audio device".to_string(),
                    });
                };
                audio.set_muted(*muted);
                session.local_muted = *muted;
                Ok(())
            }
            CollabCommand::SendDtmf { call, digit } => {
                if !matches!(*digit, '0'..='9' | '*' | '#') {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "DTMF digit is not a telephone keypad token".to_string(),
                    });
                }
                let Some(session) = inner.sessions.get_mut(call) else {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "DTMF requires a bound P2P media leg".to_string(),
                    });
                };
                if session.audio.is_none() {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "DTMF requires a bound seat audio device".to_string(),
                    });
                }
                if let Some(loopback) = &self.loopback {
                    let seq = self.frame_counter.fetch_add(1, Ordering::SeqCst);
                    let _ = loopback.on_frame(
                        *call,
                        &session.local_actor,
                        MediaPcmFrame::dtmf(seq, *digit),
                    );
                }
                Ok(())
            }
            CollabCommand::DeclineCall { call } | CollabCommand::HangUpCall { call } => {
                inner.sessions.remove(call);
                Ok(())
            }
            CollabCommand::StartOutboundCall { .. } => {
                Err(CallMediaProviderError::ExecutionRefused {
                    detail: "outbound PSTN dials are owned by the SIP gateway adapter".to_string(),
                })
            }
            _ => Ok(()),
        }
    }

    fn prove_advancing_frames(
        &self,
        session: &CallMediaSession,
        adapter: CallMediaAdapter,
    ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
        if adapter != CallMediaAdapter::WebRtcP2p {
            return Err(CallMediaProviderError::ExecutionRefused {
                detail: "P2P plane cannot prove a non-WebRTC adapter".to_string(),
            });
        }
        let inner = self.lock_inner()?;
        let Some(live) = inner.sessions.get(&session.call) else {
            return Err(CallMediaProviderError::TransportUnavailable {
                detail: "no P2P media session is bound on this seat".to_string(),
            });
        };
        match &live.state {
            MediaSessionStateV1::Connected if live.frames_observed > 0 => {
                Ok(CallMediaFrameEvidence {
                    audio_frames: live.frames_observed,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
            MediaSessionStateV1::DeviceAbsent { .. } => {
                Err(CallMediaProviderError::ProviderUnavailable {
                    detail: "seat capture device is absent".to_string(),
                })
            }
            MediaSessionStateV1::PermissionDenied { .. } => {
                Err(CallMediaProviderError::ProviderUnavailable {
                    detail: "seat capture permission denied".to_string(),
                })
            }
            MediaSessionStateV1::Reconnecting { attempt } => {
                Err(CallMediaProviderError::TransportUnavailable {
                    detail: format!("P2P media plane is reconnecting (attempt {attempt})"),
                })
            }
            MediaSessionStateV1::Failed { reason } => {
                Err(CallMediaProviderError::TransportUnavailable {
                    detail: format!("P2P media plane failed ({reason:?})"),
                })
            }
            MediaSessionStateV1::Negotiating | MediaSessionStateV1::Connected => {
                Err(CallMediaProviderError::TransportUnavailable {
                    detail: "P2P offer/answer has not proven advancing frames".to_string(),
                })
            }
        }
    }

    fn publish_p2p_media_sessions(
        &self,
        persist: &Persist,
        last_published: &mut BTreeMap<String, String>,
    ) {
        self.tick(persist, last_published);
    }

    fn owns_call(&self, call: CallId) -> bool {
        self.inner
            .lock()
            .ok()
            .is_some_and(|inner| inner.sessions.contains_key(&call))
    }
}

struct SfuSession {
    space: SpaceId,
    local_actor: ActorId,
    remote_actor: ActorId,
    host: ActorId,
    role: SignalingRole,
    participants: Vec<ActorId>,
    offered_tracks: Vec<MediaTrackKind>,
    state: MediaSessionStateV1,
    local_muted: bool,
    audio: Option<SeatAudioBinding>,
    local_description: Option<MediaDescriptionV1>,
    remote_description: Option<MediaDescriptionV1>,
    frames_observed: u64,
    reconnect_attempt: u16,
}

impl SfuSession {
    fn audio_bound(&self) -> bool {
        self.audio.is_some()
    }

    fn dtmf_bound(&self) -> bool {
        self.audio.is_some()
    }

    fn document(&self, call: CallId) -> Result<MediaSessionV1, String> {
        MediaSessionV1::new(
            call,
            self.space,
            self.local_actor.clone(),
            self.remote_actor.clone(),
            CallMediaAdapter::LiveKitSfu,
            self.state.clone(),
            self.offered_tracks.clone(),
            self.local_muted,
            self.dtmf_bound(),
            self.audio_bound(),
            self.frames_observed,
            self.local_description.clone(),
            self.remote_description.clone(),
        )
        .map_err(|error| error.to_string())
    }
}

struct SfuPlaneInner {
    local_actor: Option<ActorId>,
    sessions: BTreeMap<CallId, SfuSession>,
}

/// Group-call SFU worker. Elects a host, publishes
/// `state/calls/media/<session>/sfu`, and never claims [`MediaSessionStateV1::Connected`]
/// unless the injectable mixer seam observes advancing frames.
pub(crate) struct LiveKitSfuPlane {
    inner: Mutex<SfuPlaneInner>,
    audio: Arc<dyn SeatAudioSource>,
    loopback: Option<Arc<dyn LoopbackFrameCallback>>,
    frame_counter: AtomicU64,
    sfu_healthy: Arc<AtomicBool>,
}

impl LiveKitSfuPlane {
    #[must_use]
    pub(crate) fn production() -> Self {
        Self::new(
            Arc::new(AlsaSeatAudio),
            None,
            Arc::new(AtomicBool::new(true)),
        )
    }

    #[must_use]
    fn new(
        audio: Arc<dyn SeatAudioSource>,
        loopback: Option<Arc<dyn LoopbackFrameCallback>>,
        sfu_healthy: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inner: Mutex::new(SfuPlaneInner {
                local_actor: None,
                sessions: BTreeMap::new(),
            }),
            audio,
            loopback,
            frame_counter: AtomicU64::new(1),
            sfu_healthy,
        }
    }

    #[cfg(test)]
    fn with_local_actor(self, actor: ActorId) -> Self {
        if let Ok(mut inner) = self.inner.lock() {
            inner.local_actor = Some(actor);
        }
        self
    }

    fn lock_inner(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, SfuPlaneInner>, CallMediaProviderError> {
        self.inner
            .lock()
            .map_err(|_| CallMediaProviderError::ProviderUnavailable {
                detail: "SFU media plane lock is unavailable".to_string(),
            })
    }

    fn ensure_session(
        inner: &mut SfuPlaneInner,
        call: CallId,
        role: SignalingRole,
    ) -> &mut SfuSession {
        inner.sessions.entry(call).or_insert_with(|| SfuSession {
            space: SpaceId::nil(),
            local_actor: inner
                .local_actor
                .clone()
                .unwrap_or_else(|| ActorId::new("local")),
            remote_actor: ActorId::new("remote"),
            host: ActorId::new("host"),
            role,
            participants: Vec::new(),
            offered_tracks: vec![MediaTrackKind::Audio],
            state: MediaSessionStateV1::Negotiating,
            local_muted: false,
            audio: None,
            local_description: None,
            remote_description: None,
            frames_observed: 0,
            reconnect_attempt: 0,
        })
    }

    fn apply_bind(session: &mut SfuSession, audio: &Arc<dyn SeatAudioSource>) {
        if session.audio.is_some()
            || matches!(
                session.state,
                MediaSessionStateV1::DeviceAbsent { .. }
                    | MediaSessionStateV1::PermissionDenied { .. }
            )
        {
            return;
        }
        match audio.bind() {
            Ok(binding) => {
                binding.set_muted(session.local_muted);
                session.audio = Some(binding);
            }
            Err(SeatAudioBindError::DeviceAbsent) => {
                session.state = MediaSessionStateV1::DeviceAbsent {
                    track: MediaTrackKind::Audio,
                };
                session.audio = None;
            }
            Err(SeatAudioBindError::PermissionDenied) => {
                session.state = MediaSessionStateV1::PermissionDenied {
                    track: MediaTrackKind::Audio,
                };
                session.audio = None;
            }
        }
    }

    #[allow(dead_code)]
    fn negotiate_sfu(
        persist: &Persist,
        session: &mut SfuSession,
        call: CallId,
    ) -> Result<(), CallMediaProviderError> {
        if session.space.is_nil() {
            return Ok(());
        }
        match session.role {
            SignalingRole::Offer => {
                if session.local_description.is_none() {
                    let offer = MediaDescriptionV1::new(
                        call,
                        session.local_actor.clone(),
                        session.remote_actor.clone(),
                        MediaSignalingRoleV1::Offer,
                        vec![MediaTrackKind::Audio],
                    )
                    .map_err(|error| {
                        CallMediaProviderError::ExecutionRefused {
                            detail: error.to_string(),
                        }
                    })?;
                    publish_json(persist, &media_offer_topic(call), &offer)?;
                    session.local_description = Some(offer);
                }
                if session.remote_description.is_none() {
                    if let Some(answer) = read_description(persist, &media_answer_topic(call))? {
                        if answer.role != MediaSignalingRoleV1::Answer
                            || answer.session != call
                            || answer.from != session.remote_actor
                            || answer.to != session.local_actor
                            || answer.tracks != session.offered_tracks
                        {
                            session.state = MediaSessionStateV1::Failed {
                                reason: MediaFailureReasonV1::InvalidSignaling,
                            };
                            return Ok(());
                        }
                        session.remote_description = Some(answer);
                    }
                }
            }
            SignalingRole::Answer => {
                if session.remote_description.is_none() {
                    if let Some(offer) = read_description(persist, &media_offer_topic(call))? {
                        if offer.role != MediaSignalingRoleV1::Offer
                            || offer.session != call
                            || offer.from != session.remote_actor
                            || offer.to != session.local_actor
                            || offer.tracks != session.offered_tracks
                        {
                            session.state = MediaSessionStateV1::Failed {
                                reason: MediaFailureReasonV1::InvalidSignaling,
                            };
                            return Ok(());
                        }
                        session.remote_description = Some(offer);
                    }
                }
                if session.remote_description.is_some() && session.local_description.is_none() {
                    let answer = MediaDescriptionV1::new(
                        call,
                        session.local_actor.clone(),
                        session.remote_actor.clone(),
                        MediaSignalingRoleV1::Answer,
                        vec![MediaTrackKind::Audio],
                    )
                    .map_err(|error| {
                        CallMediaProviderError::ExecutionRefused {
                            detail: error.to_string(),
                        }
                    })?;
                    publish_json(persist, &media_answer_topic(call), &answer)?;
                    session.local_description = Some(answer);
                }
            }
        }
        Ok(())
    }

    fn ensure_sfu_descriptions(
        session: &mut SfuSession,
        call: CallId,
    ) -> Result<(), CallMediaProviderError> {
        if session.local_description.is_none() {
            session.local_description = Some(
                MediaDescriptionV1::new(
                    call,
                    session.local_actor.clone(),
                    session.remote_actor.clone(),
                    MediaSignalingRoleV1::Offer,
                    session.offered_tracks.clone(),
                )
                .map_err(|error| CallMediaProviderError::ExecutionRefused {
                    detail: error.to_string(),
                })?,
            );
        }
        if session.remote_description.is_none() {
            session.remote_description = Some(
                MediaDescriptionV1::new(
                    call,
                    session.remote_actor.clone(),
                    session.local_actor.clone(),
                    MediaSignalingRoleV1::Answer,
                    session.offered_tracks.clone(),
                )
                .map_err(|error| CallMediaProviderError::ExecutionRefused {
                    detail: error.to_string(),
                })?,
            );
        }
        Ok(())
    }

    fn pump_mixer(&self, session: &mut SfuSession, call: CallId) {
        if session.audio.is_none() {
            return;
        }
        if matches!(
            session.state,
            MediaSessionStateV1::DeviceAbsent { .. }
                | MediaSessionStateV1::PermissionDenied { .. }
                | MediaSessionStateV1::Reconnecting { .. }
                | MediaSessionStateV1::Failed { .. }
        ) {
            return;
        }
        let Some(loopback) = &self.loopback else {
            if session.local_description.is_some()
                && session.remote_description.is_some()
                && !session.state.claims_live_media()
            {
                session.state = MediaSessionStateV1::Failed {
                    reason: MediaFailureReasonV1::TransportUnavailable,
                };
            }
            return;
        };
        if session.local_description.is_none() || session.remote_description.is_none() {
            return;
        }
        let seq = self.frame_counter.fetch_add(1, Ordering::SeqCst);
        let outbound = if session.local_muted {
            MediaPcmFrame::silence(seq)
        } else {
            MediaPcmFrame::chirp(seq)
        };
        if loopback
            .on_frame(call, &session.local_actor, outbound)
            .is_some()
        {
            session.frames_observed = session.frames_observed.saturating_add(1);
            if session.frames_observed > 0
                && session.audio_bound()
                && self.sfu_healthy.load(Ordering::SeqCst)
            {
                session.state = MediaSessionStateV1::Connected;
                session.reconnect_attempt = 0;
            }
        }
    }

    fn tick_locked(
        &self,
        persist: &Persist,
        inner: &mut SfuPlaneInner,
        last_published: &mut BTreeMap<String, String>,
    ) {
        let Ok(readiness) = read_readiness(persist) else {
            return;
        };
        inner.local_actor = Some(readiness.local_actor.clone());
        let live_calls: Vec<CallId> = readiness
            .sessions
            .iter()
            .filter(|session| {
                is_group_call(&session.connected_participants)
                    && is_media_call_kind(session.kind)
                    && session.admission == CallMediaAdmission::AdapterReady
                    && session
                        .connected_participants
                        .iter()
                        .any(|actor| actor == &readiness.local_actor)
            })
            .map(|session| session.call)
            .collect();
        inner.sessions.retain(|call, _| live_calls.contains(call));

        let healthy = self.sfu_healthy.load(Ordering::SeqCst);
        let preferred = preferred_lighthouse_host(&readiness.local_actor);

        for ready in readiness
            .sessions
            .iter()
            .filter(|session| live_calls.contains(&session.call))
            .take(MAX_SESSIONS)
        {
            let participants = normalized_participants(&ready.connected_participants);
            let Some(host) = SfuElectionV1::elect_host(&participants, preferred.as_ref()) else {
                continue;
            };
            let remote = if readiness.local_actor == host {
                participants
                    .iter()
                    .find(|actor| *actor != &readiness.local_actor)
                    .cloned()
                    .unwrap_or_else(|| host.clone())
            } else {
                host.clone()
            };
            let role = if readiness.local_actor.as_str() <= remote.as_str() {
                SignalingRole::Offer
            } else {
                SignalingRole::Answer
            };
            let election =
                match SfuElectionV1::new(ready.call, host.clone(), healthy, participants.clone()) {
                    Ok(election) => election,
                    Err(error) => {
                        tracing::debug!(
                            target: "mackesd::call_media",
                            error = %error,
                            "refusing invalid SFU election"
                        );
                        continue;
                    }
                };
            if let Err(error) = publish_sfu_election(persist, last_published, &election) {
                tracing::debug!(
                    target: "mackesd::call_media",
                    error = ?error,
                    "SFU election publish failed"
                );
            }

            let session = inner
                .sessions
                .entry(ready.call)
                .or_insert_with(|| SfuSession {
                    space: ready.space,
                    local_actor: readiness.local_actor.clone(),
                    remote_actor: remote.clone(),
                    host: host.clone(),
                    role,
                    participants: participants.clone(),
                    offered_tracks: tracks_for_call_kind(ready.kind),
                    state: MediaSessionStateV1::Negotiating,
                    local_muted: ready.local_muted,
                    audio: None,
                    local_description: None,
                    remote_description: None,
                    frames_observed: 0,
                    reconnect_attempt: 0,
                });
            session.space = ready.space;
            session.local_actor = readiness.local_actor.clone();
            session.remote_actor = remote;
            session.host = host;
            session.role = role;
            session.participants = participants;
            session.offered_tracks = tracks_for_call_kind(ready.kind);
            if session.audio.is_none() {
                session.local_muted = ready.local_muted;
            }

            if !healthy {
                session.reconnect_attempt = session.reconnect_attempt.saturating_add(1).max(1);
                session.state = MediaSessionStateV1::Reconnecting {
                    attempt: session.reconnect_attempt,
                };
            } else {
                Self::apply_bind(session, &self.audio);
                if let Err(error) = Self::ensure_sfu_descriptions(session, ready.call) {
                    tracing::debug!(
                        target: "mackesd::call_media",
                        error = ?error,
                        "SFU join negotiation failed"
                    );
                }
                self.pump_mixer(session, ready.call);
            }

            match session.document(ready.call) {
                Ok(document) => {
                    let _ = publish_media_session(persist, last_published, &document);
                }
                Err(error) => tracing::debug!(
                    target: "mackesd::call_media",
                    error = %error,
                    "refusing to publish invalid SFU media session"
                ),
            }
        }
    }

    fn tick(&self, persist: &Persist, last_published: &mut BTreeMap<String, String>) {
        let Ok(mut inner) = self.lock_inner() else {
            return;
        };
        self.tick_locked(persist, &mut inner, last_published);
    }
}

impl CallMediaFrameVerifier for LiveKitSfuPlane {
    fn execute_command(
        &self,
        command: &CollabCommand,
        adapter: CallMediaAdapter,
    ) -> Result<(), CallMediaProviderError> {
        if adapter != CallMediaAdapter::LiveKitSfu {
            return Err(CallMediaProviderError::ExecutionRefused {
                detail: "SFU plane was selected for an incompatible adapter".to_string(),
            });
        }
        let mut inner = self.lock_inner()?;
        match command {
            CollabCommand::StartCall { call, kind, .. } => {
                let session =
                    LiveKitSfuPlane::ensure_session(&mut inner, *call, SignalingRole::Offer);
                session.offered_tracks = tracks_for_call_kind(*kind);
                session.state = MediaSessionStateV1::Negotiating;
                Ok(())
            }
            CollabCommand::AnswerCall { call } => {
                LiveKitSfuPlane::ensure_session(&mut inner, *call, SignalingRole::Answer);
                Ok(())
            }
            CollabCommand::SetCallMuted { call, muted } => {
                let Some(session) = inner.sessions.get_mut(call) else {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "mute requires a bound SFU media leg".to_string(),
                    });
                };
                let Some(audio) = session.audio.as_ref() else {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "mute requires a bound seat audio device".to_string(),
                    });
                };
                audio.set_muted(*muted);
                session.local_muted = *muted;
                Ok(())
            }
            CollabCommand::SendDtmf { call, digit } => {
                if !matches!(*digit, '0'..='9' | '*' | '#') {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "DTMF digit is not a telephone keypad token".to_string(),
                    });
                }
                let Some(session) = inner.sessions.get_mut(call) else {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "DTMF requires a bound SFU media leg".to_string(),
                    });
                };
                if session.audio.is_none() {
                    return Err(CallMediaProviderError::ExecutionRefused {
                        detail: "DTMF requires a bound seat audio device".to_string(),
                    });
                }
                if let Some(loopback) = &self.loopback {
                    let seq = self.frame_counter.fetch_add(1, Ordering::SeqCst);
                    let _ = loopback.on_frame(
                        *call,
                        &session.local_actor,
                        MediaPcmFrame::dtmf(seq, *digit),
                    );
                }
                Ok(())
            }
            CollabCommand::DeclineCall { call } | CollabCommand::HangUpCall { call } => {
                inner.sessions.remove(call);
                Ok(())
            }
            CollabCommand::StartOutboundCall { .. } => {
                Err(CallMediaProviderError::ExecutionRefused {
                    detail: "outbound PSTN dials are owned by the SIP gateway adapter".to_string(),
                })
            }
            _ => Ok(()),
        }
    }

    fn prove_advancing_frames(
        &self,
        session: &CallMediaSession,
        adapter: CallMediaAdapter,
    ) -> Result<CallMediaFrameEvidence, CallMediaProviderError> {
        if adapter != CallMediaAdapter::LiveKitSfu {
            return Err(CallMediaProviderError::ExecutionRefused {
                detail: "SFU plane cannot prove a non-LiveKit adapter".to_string(),
            });
        }
        let inner = self.lock_inner()?;
        let Some(live) = inner.sessions.get(&session.call) else {
            return Err(CallMediaProviderError::TransportUnavailable {
                detail: "no SFU media session is bound on this seat".to_string(),
            });
        };
        match &live.state {
            MediaSessionStateV1::Connected if live.frames_observed > 0 => {
                Ok(CallMediaFrameEvidence {
                    audio_frames: live.frames_observed,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
            MediaSessionStateV1::DeviceAbsent { .. } => {
                Err(CallMediaProviderError::ProviderUnavailable {
                    detail: "seat capture device is absent".to_string(),
                })
            }
            MediaSessionStateV1::PermissionDenied { .. } => {
                Err(CallMediaProviderError::ProviderUnavailable {
                    detail: "seat capture permission denied".to_string(),
                })
            }
            MediaSessionStateV1::Reconnecting { attempt } => {
                Err(CallMediaProviderError::TransportUnavailable {
                    detail: format!("SFU media plane is reconnecting (attempt {attempt})"),
                })
            }
            MediaSessionStateV1::Failed { reason } => {
                Err(CallMediaProviderError::TransportUnavailable {
                    detail: format!("SFU media plane failed ({reason:?})"),
                })
            }
            MediaSessionStateV1::Negotiating | MediaSessionStateV1::Connected => {
                Err(CallMediaProviderError::TransportUnavailable {
                    detail: "SFU join has not proven advancing frames".to_string(),
                })
            }
        }
    }

    fn publish_p2p_media_sessions(
        &self,
        persist: &Persist,
        last_published: &mut BTreeMap<String, String>,
    ) {
        self.tick(persist, last_published);
    }

    fn owns_call(&self, call: CallId) -> bool {
        self.inner
            .lock()
            .ok()
            .is_some_and(|inner| inner.sessions.contains_key(&call))
    }
}

fn is_group_call(participants: &[ActorId]) -> bool {
    participants.len() >= 3
}

fn is_media_call_kind(kind: CallKind) -> bool {
    matches!(kind, CallKind::Audio | CallKind::Video)
}

fn tracks_for_call_kind(kind: CallKind) -> Vec<MediaTrackKind> {
    match kind {
        CallKind::Video => vec![MediaTrackKind::Audio, MediaTrackKind::Video],
        CallKind::Audio | CallKind::Screen | CallKind::CoEdit | CallKind::RemoteDesktop => {
            vec![MediaTrackKind::Audio]
        }
    }
}

fn normalized_participants(participants: &[ActorId]) -> Vec<ActorId> {
    let mut sorted = participants.to_vec();
    sorted.sort_by_key(|actor| actor.as_str().to_string());
    sorted.dedup_by(|left, right| left == right);
    sorted
}

fn preferred_lighthouse_host(local: &ActorId) -> Option<ActorId> {
    match mde_role::load() {
        Ok(mde_role::Role::Lighthouse) => Some(local.clone()),
        _ => None,
    }
}

fn unique_remote(local: &ActorId, participants: &[ActorId]) -> Option<ActorId> {
    let remotes: Vec<&ActorId> = participants
        .iter()
        .filter(|actor| *actor != local)
        .collect();
    if remotes.len() == 1 {
        Some(remotes[0].clone())
    } else {
        None
    }
}

fn read_readiness(persist: &Persist) -> Result<CallMediaReadiness, String> {
    let topic = mde_collab_types::topics::state_topic(
        mde_collab_types::topics::projection::CALL_MEDIA_READINESS,
    );
    let msg = persist
        .read_latest(&topic)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "missing call-media-readiness".to_string())?;
    let body = msg
        .body
        .as_deref()
        .ok_or_else(|| "empty call-media-readiness".to_string())?;
    if body.len() > MAX_READINESS_BODY_BYTES {
        return Err("call-media-readiness exceeds bound".to_string());
    }
    serde_json::from_str(body).map_err(|error| error.to_string())
}

fn read_description(
    persist: &Persist,
    topic: &str,
) -> Result<Option<MediaDescriptionV1>, CallMediaProviderError> {
    let Some(msg) = persist.read_latest(topic).map_err(|error| {
        CallMediaProviderError::ProviderUnavailable {
            detail: error.to_string(),
        }
    })?
    else {
        return Ok(None);
    };
    let Some(body) = msg.body.as_deref() else {
        return Ok(None);
    };
    if body.len() > MAX_SIGNAL_BODY_BYTES {
        return Err(CallMediaProviderError::ExecutionRefused {
            detail: "media signaling body exceeds bound".to_string(),
        });
    }
    match MediaDescriptionV1::from_json(body) {
        Ok(description) => Ok(Some(description)),
        Err(_) => Err(CallMediaProviderError::ExecutionRefused {
            detail: "media signaling failed typed admission".to_string(),
        }),
    }
}

fn publish_json<T: serde::Serialize>(
    persist: &Persist,
    topic: &str,
    value: &T,
) -> Result<(), CallMediaProviderError> {
    let body =
        serde_json::to_string(value).map_err(|error| CallMediaProviderError::ExecutionRefused {
            detail: error.to_string(),
        })?;
    persist
        .write(topic, Priority::Default, None, Some(&body))
        .map_err(|error| CallMediaProviderError::ProviderUnavailable {
            detail: error.to_string(),
        })?;
    Ok(())
}

fn publish_sfu_election(
    persist: &Persist,
    last_published: &mut BTreeMap<String, String>,
    election: &SfuElectionV1,
) -> Result<(), CallMediaProviderError> {
    let topic = media_sfu_election_topic(election.session);
    let body = serde_json::to_string(election).map_err(|error| {
        CallMediaProviderError::ExecutionRefused {
            detail: error.to_string(),
        }
    })?;
    if last_published.get(&topic).map(String::as_str) == Some(body.as_str()) {
        return Ok(());
    }
    persist
        .write(&topic, Priority::Default, None, Some(&body))
        .map_err(|error| CallMediaProviderError::ProviderUnavailable {
            detail: error.to_string(),
        })?;
    last_published.insert(topic, body);
    Ok(())
}

fn publish_media_session(
    persist: &Persist,
    last_published: &mut BTreeMap<String, String>,
    document: &MediaSessionV1,
) -> Result<(), CallMediaProviderError> {
    if document.schema_version != MEDIA_SESSION_V1_SCHEMA_VERSION {
        return Err(CallMediaProviderError::ExecutionRefused {
            detail: "refusing to publish a non-V1 media session".to_string(),
        });
    }
    let topic = media_session_topic(document.session);
    let body = serde_json::to_string(document).map_err(|error| {
        CallMediaProviderError::ExecutionRefused {
            detail: error.to_string(),
        }
    })?;
    if last_published.get(&topic).map(String::as_str) == Some(body.as_str()) {
        return Ok(());
    }
    persist
        .write(&topic, Priority::Default, None, Some(&body))
        .map_err(|error| CallMediaProviderError::ProviderUnavailable {
            detail: error.to_string(),
        })?;
    last_published.insert(topic, body);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_collab_types::topics::{self, projection as proj};
    use mde_collab_types::CallMediaRequirement;

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

    fn two_party_readiness(call: CallId, space: SpaceId, local: &str) -> CallMediaReadiness {
        CallMediaReadiness {
            local_actor: ActorId::new(local),
            sessions: vec![CallMediaSession {
                call,
                space,
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
            }],
        }
    }

    fn two_party_video_readiness(call: CallId, space: SpaceId, local: &str) -> CallMediaReadiness {
        let mut readiness = two_party_readiness(call, space, local);
        readiness.sessions[0].kind = CallKind::Video;
        readiness
    }

    fn three_party_readiness(call: CallId, space: SpaceId, local: &str) -> CallMediaReadiness {
        CallMediaReadiness {
            local_actor: ActorId::new(local),
            sessions: vec![CallMediaSession {
                call,
                space,
                kind: CallKind::Audio,
                started_unix_ms: 1_700_000_000_000,
                requirements: vec![CallMediaRequirement::Microphone],
                candidate_adapters: vec![
                    CallMediaAdapter::WebRtcP2p,
                    CallMediaAdapter::LiveKitSfu,
                    CallMediaAdapter::SipGateway,
                ],
                admission: CallMediaAdmission::AdapterReady,
                connected_participants: vec![
                    ActorId::new("alice"),
                    ActorId::new("bob"),
                    ActorId::new("carol"),
                ],
                local_muted: false,
            }],
        }
    }

    fn read_election(persist: &Persist, call: CallId) -> SfuElectionV1 {
        let msg = persist
            .read_latest(&media_sfu_election_topic(call))
            .expect("read election")
            .expect("election published");
        SfuElectionV1::from_json(msg.body.as_deref().expect("body")).expect("admit election")
    }

    fn tick_group(
        planes: &[(&LiveKitSfuPlane, &str)],
        persist: &Persist,
        call: CallId,
        space: SpaceId,
    ) {
        for (plane, local) in planes {
            write_readiness(persist, &three_party_readiness(call, space, local));
            let mut published = BTreeMap::new();
            plane.tick(persist, &mut published);
        }
    }

    fn read_session(persist: &Persist, call: CallId) -> MediaSessionV1 {
        let msg = persist
            .read_latest(&media_session_topic(call))
            .expect("read session")
            .expect("session published");
        MediaSessionV1::from_json(msg.body.as_deref().expect("body")).expect("admit session")
    }

    #[test]
    fn loopback_two_seat_call_proves_frames_and_applies_mute_dtmf() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let call = CallId::new();
        let space = SpaceId::new();
        let loopback = SharedLoopback::new();
        let alice = WebrtcP2pPlane::new(FixedSeatAudio::present(), Some(loopback.clone()))
            .with_local_actor(ActorId::new("alice"));
        let bob = WebrtcP2pPlane::new(FixedSeatAudio::present(), Some(loopback.clone()))
            .with_local_actor(ActorId::new("bob"));

        alice
            .execute_command(
                &CollabCommand::StartCall {
                    space,
                    call,
                    kind: CallKind::Audio,
                },
                CallMediaAdapter::WebRtcP2p,
            )
            .expect("alice start");
        bob.execute_command(
            &CollabCommand::AnswerCall { call },
            CallMediaAdapter::WebRtcP2p,
        )
        .expect("bob answer");

        write_readiness(&persist, &two_party_readiness(call, space, "alice"));
        let mut alice_pub = BTreeMap::new();
        alice.tick(&persist, &mut alice_pub);
        write_readiness(&persist, &two_party_readiness(call, space, "bob"));
        let mut bob_pub = BTreeMap::new();
        bob.tick(&persist, &mut bob_pub);
        write_readiness(&persist, &two_party_readiness(call, space, "alice"));
        alice.tick(&persist, &mut alice_pub);
        write_readiness(&persist, &two_party_readiness(call, space, "bob"));
        bob.tick(&persist, &mut bob_pub);
        write_readiness(&persist, &two_party_readiness(call, space, "alice"));
        alice.tick(&persist, &mut alice_pub);

        let alice_session = read_session(&persist, call);
        write_readiness(&persist, &two_party_readiness(call, space, "bob"));
        bob.tick(&persist, &mut bob_pub);
        let bob_session = read_session(&persist, call);

        assert_eq!(alice_session.state, MediaSessionStateV1::Connected);
        assert_eq!(bob_session.state, MediaSessionStateV1::Connected);
        assert!(alice_session.frames_observed >= 1);
        assert!(bob_session.frames_observed >= 1);

        alice
            .execute_command(
                &CollabCommand::SetCallMuted { call, muted: true },
                CallMediaAdapter::WebRtcP2p,
            )
            .expect("mute live leg");
        alice
            .execute_command(
                &CollabCommand::SendDtmf { call, digit: '5' },
                CallMediaAdapter::WebRtcP2p,
            )
            .expect("dtmf live leg");
        assert_eq!(
            loopback.dtmf_received(call, &ActorId::new("alice")),
            vec!['5']
        );
    }

    #[test]
    fn device_absence_and_permission_denial_never_publish_connected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let space = SpaceId::new();
        let absent = WebrtcP2pPlane::new(FixedSeatAudio::absent(), None)
            .with_local_actor(ActorId::new("alice"));
        let denied = WebrtcP2pPlane::new(FixedSeatAudio::denied(), None)
            .with_local_actor(ActorId::new("alice"));

        let absent_call = CallId::new();
        write_readiness(&persist, &two_party_readiness(absent_call, space, "alice"));
        let mut published = BTreeMap::new();
        absent.tick(&persist, &mut published);
        let session = read_session(&persist, absent_call);
        assert_eq!(
            session.state,
            MediaSessionStateV1::DeviceAbsent {
                track: MediaTrackKind::Audio
            }
        );
        assert!(!session.state.claims_live_media());
        assert_eq!(session.frames_observed, 0);

        let denied_call = CallId::new();
        write_readiness(&persist, &two_party_readiness(denied_call, space, "alice"));
        let mut published = BTreeMap::new();
        denied.tick(&persist, &mut published);
        let session = read_session(&persist, denied_call);
        assert_eq!(
            session.state,
            MediaSessionStateV1::PermissionDenied {
                track: MediaTrackKind::Audio
            }
        );
        assert!(!session.state.claims_live_media());

        assert!(
            matches!(
                absent.execute_command(
                    &CollabCommand::SetCallMuted {
                        call: absent_call,
                        muted: true
                    },
                    CallMediaAdapter::WebRtcP2p
                ),
                Err(CallMediaProviderError::ExecutionRefused { detail })
                    if detail.contains("bound seat audio")
            ),
            "mute must refuse when no audio leg is bound"
        );
    }

    #[test]
    fn production_plane_without_loopback_fails_honestly_after_signaling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let call = CallId::new();
        let space = SpaceId::new();
        let alice = WebrtcP2pPlane::new(FixedSeatAudio::present(), None)
            .with_local_actor(ActorId::new("alice"));
        let bob = WebrtcP2pPlane::new(FixedSeatAudio::present(), None)
            .with_local_actor(ActorId::new("bob"));
        alice
            .execute_command(
                &CollabCommand::StartCall {
                    space,
                    call,
                    kind: CallKind::Audio,
                },
                CallMediaAdapter::WebRtcP2p,
            )
            .expect("start");
        bob.execute_command(
            &CollabCommand::AnswerCall { call },
            CallMediaAdapter::WebRtcP2p,
        )
        .expect("answer");
        write_readiness(&persist, &two_party_readiness(call, space, "alice"));
        let mut alice_pub = BTreeMap::new();
        alice.tick(&persist, &mut alice_pub);
        write_readiness(&persist, &two_party_readiness(call, space, "bob"));
        let mut bob_pub = BTreeMap::new();
        bob.tick(&persist, &mut bob_pub);
        write_readiness(&persist, &two_party_readiness(call, space, "alice"));
        alice.tick(&persist, &mut alice_pub);

        let session = read_session(&persist, call);
        assert_eq!(
            session.state,
            MediaSessionStateV1::Failed {
                reason: MediaFailureReasonV1::TransportUnavailable
            }
        );
        assert!(!session.state.claims_live_media());
        assert!(session.local_description.is_some());
        assert!(session.remote_description.is_some());
    }

    #[test]
    fn video_call_carries_audio_and_camera_tracks_through_p2p_signaling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let call = CallId::new();
        let space = SpaceId::new();
        let loopback = SharedLoopback::new();
        let alice = WebrtcP2pPlane::new(FixedSeatAudio::present(), Some(loopback.clone()))
            .with_local_actor(ActorId::new("alice"));
        let bob = WebrtcP2pPlane::new(FixedSeatAudio::present(), Some(loopback))
            .with_local_actor(ActorId::new("bob"));

        alice
            .execute_command(
                &CollabCommand::StartCall {
                    space,
                    call,
                    kind: CallKind::Video,
                },
                CallMediaAdapter::WebRtcP2p,
            )
            .expect("video start");
        bob.execute_command(
            &CollabCommand::AnswerCall { call },
            CallMediaAdapter::WebRtcP2p,
        )
        .expect("video answer");

        let mut alice_pub = BTreeMap::new();
        let mut bob_pub = BTreeMap::new();
        write_readiness(&persist, &two_party_video_readiness(call, space, "alice"));
        alice.tick(&persist, &mut alice_pub);
        write_readiness(&persist, &two_party_video_readiness(call, space, "bob"));
        bob.tick(&persist, &mut bob_pub);
        write_readiness(&persist, &two_party_video_readiness(call, space, "alice"));
        alice.tick(&persist, &mut alice_pub);

        let session = read_session(&persist, call);
        assert_eq!(
            session.offered_tracks,
            vec![MediaTrackKind::Audio, MediaTrackKind::Video]
        );
        assert_eq!(
            session
                .local_description
                .as_ref()
                .expect("video offer")
                .tracks,
            vec![MediaTrackKind::Audio, MediaTrackKind::Video]
        );
        assert_eq!(
            session
                .remote_description
                .as_ref()
                .expect("video answer")
                .tracks,
            vec![MediaTrackKind::Audio, MediaTrackKind::Video]
        );
    }

    #[test]
    fn group_call_three_seats_proves_election_and_connected_audio() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let call = CallId::new();
        let space = SpaceId::new();
        let loopback = GroupSfuLoopback::new();
        loopback.set_host(call, &ActorId::new("alice"));
        let healthy = Arc::new(AtomicBool::new(true));
        let alice = LiveKitSfuPlane::new(
            FixedSeatAudio::present(),
            Some(loopback.clone()),
            healthy.clone(),
        )
        .with_local_actor(ActorId::new("alice"));
        let bob = LiveKitSfuPlane::new(
            FixedSeatAudio::present(),
            Some(loopback.clone()),
            healthy.clone(),
        )
        .with_local_actor(ActorId::new("bob"));
        let carol = LiveKitSfuPlane::new(
            FixedSeatAudio::present(),
            Some(loopback.clone()),
            healthy.clone(),
        )
        .with_local_actor(ActorId::new("carol"));

        alice
            .execute_command(
                &CollabCommand::StartCall {
                    space,
                    call,
                    kind: CallKind::Audio,
                },
                CallMediaAdapter::LiveKitSfu,
            )
            .expect("alice start");
        bob.execute_command(
            &CollabCommand::AnswerCall { call },
            CallMediaAdapter::LiveKitSfu,
        )
        .expect("bob answer");
        carol
            .execute_command(
                &CollabCommand::AnswerCall { call },
                CallMediaAdapter::LiveKitSfu,
            )
            .expect("carol answer");

        for _ in 0..4 {
            tick_group(
                &[(&alice, "alice"), (&bob, "bob"), (&carol, "carol")],
                &persist,
                call,
                space,
            );
        }

        let election = read_election(&persist, call);
        assert_eq!(election.host, ActorId::new("alice"));
        assert!(election.healthy);
        assert_eq!(election.participants.len(), 3);

        let alice_session = read_session(&persist, call);
        assert_eq!(alice_session.adapter, CallMediaAdapter::LiveKitSfu);
        assert_eq!(alice_session.state, MediaSessionStateV1::Connected);
        assert!(alice_session.frames_observed >= 1);

        bob.execute_command(
            &CollabCommand::SetCallMuted { call, muted: true },
            CallMediaAdapter::LiveKitSfu,
        )
        .expect("bob mute");
        bob.execute_command(
            &CollabCommand::SendDtmf { call, digit: '7' },
            CallMediaAdapter::LiveKitSfu,
        )
        .expect("bob dtmf");
        assert_eq!(
            loopback.dtmf_received(call, &ActorId::new("bob")),
            vec!['7']
        );
    }

    #[test]
    fn sfu_loss_mid_call_reconnects_without_fake_connected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let call = CallId::new();
        let space = SpaceId::new();
        let loopback = GroupSfuLoopback::new();
        loopback.set_host(call, &ActorId::new("alice"));
        let healthy = Arc::new(AtomicBool::new(true));
        let alice =
            LiveKitSfuPlane::new(FixedSeatAudio::present(), Some(loopback), healthy.clone())
                .with_local_actor(ActorId::new("alice"));

        write_readiness(&persist, &three_party_readiness(call, space, "alice"));
        for _ in 0..4 {
            let mut published = BTreeMap::new();
            alice.tick(&persist, &mut published);
        }
        let connected = read_session(&persist, call);
        assert_eq!(connected.state, MediaSessionStateV1::Connected);

        healthy.store(false, Ordering::SeqCst);
        write_readiness(&persist, &three_party_readiness(call, space, "alice"));
        let mut published = BTreeMap::new();
        alice.tick(&persist, &mut published);

        let reconnecting = read_session(&persist, call);
        assert_eq!(
            reconnecting.state,
            MediaSessionStateV1::Reconnecting { attempt: 1 }
        );
        assert!(!reconnecting.state.claims_live_media());

        let election = read_election(&persist, call);
        assert!(!election.healthy);
    }
}

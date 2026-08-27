//! Bounded, versioned live-media session contracts (WL-FUNC-024 S1 / S4 / S6).
//!
//! These types are the only media facts allowed on the Bus. Offer/answer,
//! track kind, mute, and session readiness travel as [`MediaSessionV1`] /
//! [`MediaDescriptionV1`] — never as untyped JSON bags. Seat capture devices
//! travel as [`MediaDeviceV1`] rows on that same session (S5 enumeration),
//! never as a hardware path or untyped inventory bag. The collab-bus sidecar
//! boards [`CallMediaReadiness`] / [`CallMediaVerification`] use the same
//! bounded `from_json` admission so `state/collab/call-media-*` cannot carry a
//! hostile bag. PSTN legs travel as [`SipLegV1`]. Mid-call honesty (peer drop,
//! SFU loss, device unplug, permission revoke) is a [`MediaFailureReasonV1`]
//! on the existing reconnecting/failed ladder, with bounded auto-reconnect
//! and a manual re-dial flag on [`MediaRecoveryV1`] (S6) — not a parallel
//! schema. The crate is still pure: no I/O, no wall clock, no media stack. A
//! [`MediaSessionStateV1::Connected`] or
//! [`CallMediaVerificationStatus::LiveMediaVerified`] value is intrinsically
//! invalid unless advancing frames were observed, so a hostile publisher
//! cannot claim a live call by omitting evidence.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::clipboard_v2::reject_duplicate_json_keys;
use crate::ids::CallId;
use crate::read_model::{
    CallMediaAdapter, CallMediaAdmission, CallMediaFrameEvidence, CallMediaReadiness,
    CallMediaRequirement, CallMediaSession, CallMediaVerification, CallMediaVerificationRow,
    CallMediaVerificationStatus,
};
use crate::value::{sha256_hex, CallKind};
use crate::{ActorId, SpaceId};

/// The only media-session schema currently admitted by this crate.
pub const MEDIA_SESSION_V1_SCHEMA_VERSION: u16 = 1;
/// Maximum encoded JSON body accepted by [`MediaSessionV1::from_json_bytes`].
pub const MAX_MEDIA_SESSION_V1_JSON_BYTES: usize = 16 * 1024;
/// Maximum encoded JSON body accepted by [`MediaDescriptionV1::from_json_bytes`].
pub const MAX_MEDIA_DESCRIPTION_V1_JSON_BYTES: usize = 8 * 1024;
/// Maximum bytes in a media actor identity token.
pub const MAX_MEDIA_ACTOR_BYTES: usize = 128;
/// Maximum tracks offered on one session (audio + camera + screen).
pub const MAX_MEDIA_TRACKS: usize = 4;
/// Maximum seat capture/playback devices retained on one session.
pub const MAX_MEDIA_DEVICES: usize = 8;
/// Maximum UTF-8 bytes in a published device label.
pub const MAX_MEDIA_DEVICE_LABEL_BYTES: usize = 64;
/// Maximum reconnect attempts retained on the wire.
pub const MAX_MEDIA_RECONNECT_ATTEMPTS: u16 = 16;
/// Prefix for the local media-readiness projection.
pub const MEDIA_STATE_PREFIX: &str = "state/calls/media/";

/// Retained local readiness topic: `state/calls/media/<session>`.
#[must_use]
pub fn media_session_topic(session: CallId) -> String {
    format!("{MEDIA_STATE_PREFIX}{session}")
}

/// Offerer signaling topic: `state/calls/media/<session>/offer`.
#[must_use]
pub fn media_offer_topic(session: CallId) -> String {
    format!("{MEDIA_STATE_PREFIX}{session}/offer")
}

/// Answerer signaling topic: `state/calls/media/<session>/answer`.
#[must_use]
pub fn media_answer_topic(session: CallId) -> String {
    format!("{MEDIA_STATE_PREFIX}{session}/answer")
}

/// Elected LiveKit SFU host for a group call: `state/calls/media/<session>/sfu`.
#[must_use]
pub fn media_sfu_election_topic(session: CallId) -> String {
    format!("{MEDIA_STATE_PREFIX}{session}/sfu")
}

/// PSTN leg bridged through the LiveKit SIP gateway:
/// `state/calls/media/<session>/sip`.
#[must_use]
pub fn media_sip_leg_topic(session: CallId) -> String {
    format!("{MEDIA_STATE_PREFIX}{session}/sip")
}

/// Maximum encoded JSON body accepted by [`SfuElectionV1::from_json_bytes`].
pub const MAX_SFU_ELECTION_V1_JSON_BYTES: usize = 8 * 1024;
/// Maximum participants retained on one SFU election document.
pub const MAX_SFU_ELECTION_PARTICIPANTS: usize = 16;

/// Maximum encoded JSON body accepted by [`SipLegV1::from_json_bytes`].
pub const MAX_SIP_LEG_V1_JSON_BYTES: usize = 8 * 1024;
/// Minimum E.164 significant digits (short codes / emergency numbers).
pub const MIN_SIP_E164_DIGITS: usize = 3;
/// Maximum E.164 significant digits (ITU-T E.164 caps the number at 15).
pub const MAX_SIP_E164_DIGITS: usize = 15;
/// Maximum encoded JSON body accepted by [`CallMediaReadiness::from_json_bytes`].
pub const MAX_CALL_MEDIA_READINESS_JSON_BYTES: usize = 256 * 1024;
/// Maximum encoded JSON body accepted by [`CallMediaVerification::from_json_bytes`].
pub const MAX_CALL_MEDIA_VERIFICATION_JSON_BYTES: usize = 256 * 1024;
/// Maximum sessions retained on one readiness board.
pub const MAX_CALL_MEDIA_SESSIONS: usize = 256;
/// Maximum verification rows retained on one verification board.
pub const MAX_CALL_MEDIA_VERIFICATION_ROWS: usize = 1024;
/// Maximum participants named on one readiness session.
pub const MAX_CALL_MEDIA_PARTICIPANTS: usize = 32;
/// Maximum UTF-8 bytes in a verification-row detail string.
pub const MAX_CALL_MEDIA_DETAIL_BYTES: usize = 512;

/// One media track a session may offer. Video and screen are named so a later
/// plane can attach them; S2 only carries audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaTrackKind {
    /// Microphone capture / playback.
    Audio,
    /// Camera capture.
    Video,
    /// Screen capture.
    Screen,
}

impl MediaTrackKind {
    /// Canonical snake-case wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Screen => "screen",
        }
    }
}

/// Which side of the offer/answer exchange authored a description.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaSignalingRoleV1 {
    /// This seat minted the offer.
    Offer,
    /// This seat minted the answer.
    Answer,
}

/// Typed reason a media session failed. Free-text reasons are forbidden so a
/// command, URL, or secret cannot ride the Bus as a "failure message".
///
/// S6 mid-call honesty uses [`Self::PeerDropped`], [`Self::SfuUnreachable`],
/// [`Self::DeviceUnplugged`], and [`Self::PermissionRevoked`] on
/// [`MediaSessionStateV1::Failed`]. Those are distinct from start-of-session
/// [`MediaSessionStateV1::DeviceAbsent`] / [`MediaSessionStateV1::PermissionDenied`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaFailureReasonV1 {
    /// No media transport (WebRTC/loopback) is bound on this seat.
    TransportUnavailable,
    /// The remote description was missing, mismatched, or hostile.
    InvalidSignaling,
    /// The remote peer left or stopped answering.
    PeerDropped,
    /// Offer/answer did not complete within the worker's bounded wait.
    NegotiationTimeout,
    /// The elected LiveKit SFU is unreachable; P2P failover may follow.
    SfuUnreachable,
    /// A previously bound capture/playback device was unplugged mid-call.
    DeviceUnplugged,
    /// Capture/playback permission was revoked after the session started.
    PermissionRevoked,
}

impl MediaFailureReasonV1 {
    /// Canonical snake-case wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TransportUnavailable => "transport_unavailable",
            Self::InvalidSignaling => "invalid_signaling",
            Self::PeerDropped => "peer_dropped",
            Self::NegotiationTimeout => "negotiation_timeout",
            Self::SfuUnreachable => "sfu_unreachable",
            Self::DeviceUnplugged => "device_unplugged",
            Self::PermissionRevoked => "permission_revoked",
        }
    }
}

/// One seat capture/playback device published on [`MediaSessionV1`].
///
/// Labels are bounded tokens, never paths, URLs, commands, or ALSA/V4L
/// device nodes. An empty inventory is the honest "not yet published" state;
/// a selected-but-not-present row is mid-call unplug honesty, not a live bind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaDeviceV1 {
    /// Track this device can serve.
    pub kind: MediaTrackKind,
    /// Operator-visible label. Bounded; no path, URI, or secret.
    pub label: String,
    /// Whether the worker currently binds this device for `kind`.
    pub selected: bool,
    /// Whether the device is still attached to the seat.
    pub present: bool,
}

impl MediaDeviceV1 {
    /// Assemble an intrinsically valid device row.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the label is empty, oversized, or
    /// carries a forbidden path/command/URI.
    pub fn new(
        kind: MediaTrackKind,
        label: impl Into<String>,
        selected: bool,
        present: bool,
    ) -> Result<Self, MediaSessionV1ValidationError> {
        let device = Self {
            kind,
            label: label.into(),
            selected,
            present,
        };
        device.validate()?;
        Ok(device)
    }

    /// Validate the intrinsic device-label contract.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), MediaSessionV1ValidationError> {
        validate_device_label(&self.label)
    }
}

/// Bounded mid-call recovery on [`MediaSessionV1`] (WL-FUNC-024 S6).
///
/// Auto-reconnect and manual re-dial are mutually exclusive: the worker is
/// either still retrying, or it has given up and the operator may redial.
/// This document never claims live media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaRecoveryV1 {
    /// Why the live leg dropped. Same closed set as [`MediaFailureReasonV1`].
    pub reason: MediaFailureReasonV1,
    /// One-based attempt count, bounded by [`MAX_MEDIA_RECONNECT_ATTEMPTS`].
    pub attempt: u16,
    /// `true` while the worker is still auto-reconnecting.
    pub auto_reconnect: bool,
    /// `true` when auto-reconnect is exhausted and a manual re-dial is offered.
    pub redial_offered: bool,
}

impl MediaRecoveryV1 {
    /// Assemble an intrinsically valid recovery document.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the attempt is out of bounds or both
    /// auto-reconnect and re-dial are claimed together.
    pub fn new(
        reason: MediaFailureReasonV1,
        attempt: u16,
        auto_reconnect: bool,
        redial_offered: bool,
    ) -> Result<Self, MediaSessionV1ValidationError> {
        let recovery = Self {
            reason,
            attempt,
            auto_reconnect,
            redial_offered,
        };
        recovery.validate()?;
        Ok(recovery)
    }

    /// Validate the intrinsic recovery contract.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), MediaSessionV1ValidationError> {
        if self.attempt == 0 || self.attempt > MAX_MEDIA_RECONNECT_ATTEMPTS {
            return Err(MediaSessionV1ValidationError::OutOfBounds {
                field: "recovery.attempt",
                max: u64::from(MAX_MEDIA_RECONNECT_ATTEMPTS),
            });
        }
        if self.auto_reconnect && self.redial_offered {
            return Err(MediaSessionV1ValidationError::InvalidField { field: "recovery" });
        }
        Ok(())
    }

    /// Recovery is never live media. Kept as a method so a caller cannot treat
    /// a reconnecting/redial document as frames.
    #[must_use]
    pub const fn claims_live_media(&self) -> bool {
        let _ = self;
        false
    }
}

/// Honest media-plane state. `connected` is only valid with observed frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum MediaSessionStateV1 {
    /// Offer/answer is in flight; no live frames have been proven.
    Negotiating,
    /// Advancing frames were observed on a bound audio leg.
    Connected,
    /// The required local capture/playback device is not present.
    DeviceAbsent {
        /// The track that has no device.
        track: MediaTrackKind,
    },
    /// The seat refused capture/playback permission for a required track.
    PermissionDenied {
        /// The track whose permission was denied.
        track: MediaTrackKind,
    },
    /// A previously live leg is trying to recover.
    Reconnecting {
        /// One-based attempt count, bounded by [`MAX_MEDIA_RECONNECT_ATTEMPTS`].
        attempt: u16,
    },
    /// The session cannot carry media; see [`MediaFailureReasonV1`].
    Failed {
        /// Stable failure class.
        reason: MediaFailureReasonV1,
    },
}

impl MediaSessionStateV1 {
    /// Whether this state claims live media. Only [`Self::Connected`] does.
    #[must_use]
    pub const fn claims_live_media(&self) -> bool {
        matches!(self, Self::Connected)
    }

    /// Whether this state is an honest unavailable outcome.
    #[must_use]
    pub const fn is_unavailable(&self) -> bool {
        matches!(
            self,
            Self::DeviceAbsent { .. }
                | Self::PermissionDenied { .. }
                | Self::Failed { .. }
                | Self::Reconnecting { .. }
        )
    }
}

/// Bounded offer or answer exchanged over collab media signaling.
///
/// This is not a WebRTC SDP blob. It is a typed mesh description: session,
/// actors, tracks, role, and a content-addressed fingerprint of those fields.
/// Paths, URLs, commands, and raw SDP text are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaDescriptionV1 {
    /// Schema discriminator; must equal [`MEDIA_SESSION_V1_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// The call this description belongs to.
    pub session: CallId,
    /// Author of this description.
    pub from: ActorId,
    /// Intended peer.
    pub to: ActorId,
    /// Offer or answer.
    pub role: MediaSignalingRoleV1,
    /// Tracks this side is offering. Audio is required for S2.
    pub tracks: Vec<MediaTrackKind>,
    /// Lower-hex SHA-256 of the canonical description bytes.
    pub fingerprint_sha256_hex: String,
}

impl MediaDescriptionV1 {
    /// Assemble an intrinsically valid offer or answer.
    ///
    /// # Errors
    ///
    /// Returns a validation error when actors, tracks, or the resulting
    /// fingerprint fail the bounded contract.
    pub fn new(
        session: CallId,
        from: ActorId,
        to: ActorId,
        role: MediaSignalingRoleV1,
        tracks: Vec<MediaTrackKind>,
    ) -> Result<Self, MediaSessionV1ValidationError> {
        let fingerprint_sha256_hex = description_fingerprint(session, &from, &to, role, &tracks);
        let description = Self {
            schema_version: MEDIA_SESSION_V1_SCHEMA_VERSION,
            session,
            from,
            to,
            role,
            tracks,
            fingerprint_sha256_hex,
        };
        description.validate()?;
        Ok(description)
    }

    /// Decode and admit a bounded JSON description body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json(body: &str) -> Result<Self, MediaSessionV1DecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON byte body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, MediaSessionV1DecodeError> {
        if body.len() > MAX_MEDIA_DESCRIPTION_V1_JSON_BYTES {
            return Err(MediaSessionV1DecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_MEDIA_DESCRIPTION_V1_JSON_BYTES,
            });
        }
        reject_duplicate_json_keys(body).map_err(MediaSessionV1DecodeError::Json)?;
        let description =
            serde_json::from_slice::<Self>(body).map_err(MediaSessionV1DecodeError::Json)?;
        description
            .validate()
            .map_err(MediaSessionV1DecodeError::Validation)?;
        Ok(description)
    }

    /// Validate the intrinsic contract.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), MediaSessionV1ValidationError> {
        if self.schema_version != MEDIA_SESSION_V1_SCHEMA_VERSION {
            return Err(MediaSessionV1ValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.session.is_nil() {
            return Err(MediaSessionV1ValidationError::NilSessionId);
        }
        validate_actor("from", &self.from)?;
        validate_actor("to", &self.to)?;
        if self.from == self.to {
            return Err(MediaSessionV1ValidationError::InvalidField { field: "to" });
        }
        validate_tracks(&self.tracks)?;
        let expected =
            description_fingerprint(self.session, &self.from, &self.to, self.role, &self.tracks);
        if self.fingerprint_sha256_hex != expected {
            return Err(MediaSessionV1ValidationError::InvalidField {
                field: "fingerprint_sha256_hex",
            });
        }
        Ok(())
    }
}

/// Deterministic LiveKit SFU host election for a group call (WL-FUNC-024 S3).
///
/// `healthy` is the elected host's observed mixer/transport health. A `false`
/// value is an honest degraded SFU, never a connected claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SfuElectionV1 {
    /// Schema discriminator; must equal [`MEDIA_SESSION_V1_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// The group call this election belongs to.
    pub session: CallId,
    /// Elected SFU host (lighthouse when present, else lexicographic min).
    pub host: ActorId,
    /// Whether the elected host currently reports a healthy mixer.
    pub healthy: bool,
    /// Connected participants the election was computed from.
    pub participants: Vec<ActorId>,
}

impl SfuElectionV1 {
    /// Assemble an intrinsically valid election document.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the session, host, or participant set
    /// fail the bounded contract.
    pub fn new(
        session: CallId,
        host: ActorId,
        healthy: bool,
        participants: Vec<ActorId>,
    ) -> Result<Self, MediaSessionV1ValidationError> {
        let election = Self {
            schema_version: MEDIA_SESSION_V1_SCHEMA_VERSION,
            session,
            host,
            healthy,
            participants,
        };
        election.validate()?;
        Ok(election)
    }

    /// Decode and admit a bounded JSON election body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json(body: &str) -> Result<Self, MediaSessionV1DecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON byte body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, MediaSessionV1DecodeError> {
        if body.len() > MAX_SFU_ELECTION_V1_JSON_BYTES {
            return Err(MediaSessionV1DecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_SFU_ELECTION_V1_JSON_BYTES,
            });
        }
        reject_duplicate_json_keys(body).map_err(MediaSessionV1DecodeError::Json)?;
        let election =
            serde_json::from_slice::<Self>(body).map_err(MediaSessionV1DecodeError::Json)?;
        election
            .validate()
            .map_err(MediaSessionV1DecodeError::Validation)?;
        Ok(election)
    }

    /// Validate the intrinsic contract.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), MediaSessionV1ValidationError> {
        if self.schema_version != MEDIA_SESSION_V1_SCHEMA_VERSION {
            return Err(MediaSessionV1ValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.session.is_nil() {
            return Err(MediaSessionV1ValidationError::NilSessionId);
        }
        validate_actor("host", &self.host)?;
        if self.participants.len() < 3 || self.participants.len() > MAX_SFU_ELECTION_PARTICIPANTS {
            return Err(MediaSessionV1ValidationError::InvalidField {
                field: "participants",
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut host_present = false;
        for actor in &self.participants {
            validate_actor("participants", actor)?;
            if !seen.insert(actor.as_str()) {
                return Err(MediaSessionV1ValidationError::InvalidField {
                    field: "participants",
                });
            }
            if actor == &self.host {
                host_present = true;
            }
        }
        if !host_present {
            return Err(MediaSessionV1ValidationError::InvalidField { field: "host" });
        }
        Ok(())
    }

    /// Elect the SFU host: `preferred` when it is in the set, else the
    /// lexicographically first participant.
    #[must_use]
    pub fn elect_host(participants: &[ActorId], preferred: Option<&ActorId>) -> Option<ActorId> {
        if participants.len() < 3 {
            return None;
        }
        if let Some(preferred) = preferred {
            if participants.iter().any(|actor| actor == preferred) {
                return Some(preferred.clone());
            }
        }
        participants
            .iter()
            .min_by_key(|actor| actor.as_str())
            .cloned()
    }
}

/// Direction of a PSTN leg on the LiveKit SIP gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SipLegDirectionV1 {
    /// The local seat originated the PSTN dial.
    Outbound,
    /// The gateway presented an inbound PSTN offer.
    Inbound,
}

impl SipLegDirectionV1 {
    /// Canonical snake-case wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Outbound => "outbound",
            Self::Inbound => "inbound",
        }
    }
}

/// PSTN leg bridged through the LiveKit SIP gateway (WL-FUNC-024 S4).
///
/// The document names the call, the local seat, the dial direction, and a
/// fail-closed E.164. Credentials, SIP URIs, passwords, and raw SDP never
/// appear on the wire. `bridged` is intrinsically invalid unless the gateway
/// is available — a hostile publisher cannot claim a live PSTN leg by omitting
/// evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SipLegV1 {
    /// Schema discriminator; must equal [`MEDIA_SESSION_V1_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// The call this PSTN leg belongs to.
    pub session: CallId,
    /// The local seat that owns the gateway path.
    pub local_actor: ActorId,
    /// Inbound offer or outbound dial.
    pub direction: SipLegDirectionV1,
    /// Canonical E.164: `+` plus [`MIN_SIP_E164_DIGITS`]..=[`MAX_SIP_E164_DIGITS`]
    /// significant digits. Never a SIP URI, `tel:`, or secret.
    pub e164: String,
    /// Whether the LiveKit SIP gateway currently reports a reachable trunk.
    pub gateway_available: bool,
    /// Whether this leg is bridged onto a live PSTN path.
    pub bridged: bool,
}

impl SipLegV1 {
    /// Assemble an intrinsically valid PSTN-leg document.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the session, actor, E.164, or
    /// bridged/gateway pairing fail the bounded contract.
    pub fn new(
        session: CallId,
        local_actor: ActorId,
        direction: SipLegDirectionV1,
        e164: impl Into<String>,
        gateway_available: bool,
        bridged: bool,
    ) -> Result<Self, SipLegV1ValidationError> {
        let document = Self {
            schema_version: MEDIA_SESSION_V1_SCHEMA_VERSION,
            session,
            local_actor,
            direction,
            e164: e164.into(),
            gateway_available,
            bridged,
        };
        document.validate()?;
        Ok(document)
    }

    /// The retained Bus topic this document belongs on.
    #[must_use]
    pub fn topic(&self) -> String {
        media_sip_leg_topic(self.session)
    }

    /// Decode and admit a bounded JSON PSTN-leg body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json(body: &str) -> Result<Self, SipLegV1DecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON byte body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, SipLegV1DecodeError> {
        if body.len() > MAX_SIP_LEG_V1_JSON_BYTES {
            return Err(SipLegV1DecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_SIP_LEG_V1_JSON_BYTES,
            });
        }
        reject_duplicate_json_keys(body).map_err(SipLegV1DecodeError::Json)?;
        let document = serde_json::from_slice::<Self>(body).map_err(SipLegV1DecodeError::Json)?;
        document
            .validate()
            .map_err(SipLegV1DecodeError::Validation)?;
        Ok(document)
    }

    /// Validate the intrinsic contract, including fail-closed E.164 and the
    /// bridged-requires-gateway honesty lock.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), SipLegV1ValidationError> {
        if self.schema_version != MEDIA_SESSION_V1_SCHEMA_VERSION {
            return Err(SipLegV1ValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.session.is_nil() {
            return Err(SipLegV1ValidationError::NilSessionId);
        }
        validate_sip_actor("local_actor", &self.local_actor)?;
        validate_e164(&self.e164)?;
        if self.bridged && !self.gateway_available {
            return Err(SipLegV1ValidationError::BridgedWithoutGateway);
        }
        Ok(())
    }
}

/// Why a PSTN-leg document failed intrinsic validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SipLegV1ValidationError {
    /// Schema discriminator is not supported.
    UnsupportedSchema {
        /// Version found on the wire.
        found: u16,
    },
    /// The session/call id was the nil sentinel.
    NilSessionId,
    /// A bounded value exceeded its maximum.
    OutOfBounds {
        /// Field that exceeded its bound.
        field: &'static str,
        /// Maximum admitted value.
        max: u64,
    },
    /// A field failed shape or pairing admission.
    InvalidField {
        /// Field that failed.
        field: &'static str,
    },
    /// A free-text or identity field carried a command, path, URL, or secret.
    ForbiddenValue {
        /// Field that contained the forbidden value.
        field: &'static str,
    },
    /// `bridged` was claimed without a reachable SIP gateway.
    BridgedWithoutGateway,
}

impl fmt::Display for SipLegV1ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => {
                write!(formatter, "unsupported SIP leg schema version {found}")
            }
            Self::NilSessionId => formatter.write_str("SIP leg session id is nil"),
            Self::OutOfBounds { field, max } => {
                write!(formatter, "SIP {field} exceeds bound {max}")
            }
            Self::InvalidField { field } => write!(formatter, "invalid SIP leg field {field}"),
            Self::ForbiddenValue { field } => {
                write!(formatter, "forbidden SIP value in {field}")
            }
            Self::BridgedWithoutGateway => {
                formatter.write_str("SIP leg cannot be bridged without an available gateway")
            }
        }
    }
}

impl std::error::Error for SipLegV1ValidationError {}

/// Why a JSON PSTN-leg body could not be decoded and admitted.
#[derive(Debug)]
pub enum SipLegV1DecodeError {
    /// The encoded body was rejected before serde allocation.
    BodyTooLarge {
        /// Number of bytes supplied.
        bytes: usize,
        /// Maximum encoded body size.
        max: usize,
    },
    /// The body was malformed JSON or had an unknown/duplicate wire field.
    Json(serde_json::Error),
    /// The body decoded but failed semantic validation.
    Validation(SipLegV1ValidationError),
}

impl fmt::Display for SipLegV1DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => {
                write!(formatter, "SIP leg body is {bytes} bytes; maximum is {max}")
            }
            Self::Json(error) => write!(formatter, "invalid SIP leg JSON: {error}"),
            Self::Validation(error) => write!(formatter, "invalid SIP leg: {error}"),
        }
    }
}

impl std::error::Error for SipLegV1DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BodyTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

/// Local worker-owned media-session readiness published on
/// [`media_session_topic`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaSessionV1 {
    /// Schema discriminator; must equal [`MEDIA_SESSION_V1_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// The call this media session belongs to.
    pub session: CallId,
    /// Collaboration space the call lives in.
    pub space: SpaceId,
    /// The local seat.
    pub local_actor: ActorId,
    /// The one remote peer for this one-to-one leg.
    pub remote_actor: ActorId,
    /// Adapter family driving the leg. S2 publishes [`CallMediaAdapter::WebRtcP2p`].
    pub adapter: CallMediaAdapter,
    /// Honest media state. Never `connected` without [`Self::frames_observed`].
    pub state: MediaSessionStateV1,
    /// Tracks this seat offered.
    pub offered_tracks: Vec<MediaTrackKind>,
    /// Local mute bit applied to the bound audio leg.
    pub local_muted: bool,
    /// Whether mute/DTMF have a live bound audio leg to act on.
    pub dtmf_bound: bool,
    /// Whether seat audio capture/playback was bound (not proof of frames).
    pub audio_bound: bool,
    /// Advancing audio frames observed on the live or loopback seam.
    pub frames_observed: u64,
    /// Seat capture/playback devices the worker published for this session.
    /// Empty is the honest "not yet enumerated" state — never invented hardware.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bound_devices: Vec<MediaDeviceV1>,
    /// Mid-call recovery (S6). Absent on connected/negotiating and on
    /// start-of-session device-absent / permission-denied. Never live media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<MediaRecoveryV1>,
    /// Local offer or answer, when minted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_description: Option<MediaDescriptionV1>,
    /// Remote offer or answer, when admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_description: Option<MediaDescriptionV1>,
}

impl MediaSessionV1 {
    /// Assemble an intrinsically valid session document.
    ///
    /// # Errors
    ///
    /// Returns a validation error when identifiers, state, or descriptions
    /// fail the bounded contract — including a `connected` state with no frames.
    pub fn new(
        session: CallId,
        space: SpaceId,
        local_actor: ActorId,
        remote_actor: ActorId,
        adapter: CallMediaAdapter,
        state: MediaSessionStateV1,
        offered_tracks: Vec<MediaTrackKind>,
        local_muted: bool,
        dtmf_bound: bool,
        audio_bound: bool,
        frames_observed: u64,
        local_description: Option<MediaDescriptionV1>,
        remote_description: Option<MediaDescriptionV1>,
    ) -> Result<Self, MediaSessionV1ValidationError> {
        let document = Self {
            schema_version: MEDIA_SESSION_V1_SCHEMA_VERSION,
            session,
            space,
            local_actor,
            remote_actor,
            adapter,
            state,
            offered_tracks,
            local_muted,
            dtmf_bound,
            audio_bound,
            frames_observed,
            bound_devices: Vec::new(),
            recovery: None,
            local_description,
            remote_description,
        };
        document.validate()?;
        Ok(document)
    }

    /// The retained Bus topic this document belongs on.
    #[must_use]
    pub fn topic(&self) -> String {
        media_session_topic(self.session)
    }

    /// Whether this document claims live media. Delegates to
    /// [`MediaSessionStateV1::claims_live_media`] so a caller cannot treat
    /// signaling `CallParticipantState::Connected` as frames.
    #[must_use]
    pub const fn claims_live_media(&self) -> bool {
        self.state.claims_live_media()
    }

    /// Mute may act only when live frames were observed and the audio sender
    /// is bound. A published session in negotiating/reconnecting/failed is not
    /// a live mute path.
    #[must_use]
    pub const fn binds_live_mute(&self) -> bool {
        self.claims_live_media() && self.audio_bound
    }

    /// DTMF may act only when live frames were observed and the DTMF sender
    /// is bound. No session, or a session without that bind, is fail-closed.
    #[must_use]
    pub const fn binds_live_dtmf(&self) -> bool {
        self.claims_live_media() && self.dtmf_bound
    }

    /// Whether this session offered `track` on a live connected leg.
    #[must_use]
    pub fn offered_live_track(&self, track: MediaTrackKind) -> bool {
        self.claims_live_media() && self.offered_tracks.contains(&track)
    }

    /// Bind a worker-published device inventory. Empty is honest (not yet
    /// enumerated). Does not invent a connected claim.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a label is hostile or two devices of
    /// the same kind are selected.
    pub fn with_bound_devices(
        mut self,
        bound_devices: Vec<MediaDeviceV1>,
    ) -> Result<Self, MediaSessionV1ValidationError> {
        self.bound_devices = bound_devices;
        self.validate()?;
        Ok(self)
    }

    /// Bind a mid-call recovery document. Connected and start-of-session
    /// unavailable states reject recovery.
    ///
    /// # Errors
    ///
    /// Returns a validation error when recovery disagrees with `state`.
    pub fn with_recovery(
        mut self,
        recovery: MediaRecoveryV1,
    ) -> Result<Self, MediaSessionV1ValidationError> {
        self.recovery = Some(recovery);
        self.validate()?;
        Ok(self)
    }

    /// The selected device for `kind`, if the worker published one.
    #[must_use]
    pub fn selected_device(&self, kind: MediaTrackKind) -> Option<&MediaDeviceV1> {
        self.bound_devices
            .iter()
            .find(|device| device.kind == kind && device.selected)
    }

    /// Published devices for `kind`, in wire order.
    #[must_use]
    pub fn devices_for(&self, kind: MediaTrackKind) -> Vec<&MediaDeviceV1> {
        self.bound_devices
            .iter()
            .filter(|device| device.kind == kind)
            .collect()
    }

    /// Operator-visible reason on the reconnecting/failed ladder, if named.
    #[must_use]
    pub const fn recovery_reason(&self) -> Option<MediaFailureReasonV1> {
        match (&self.state, &self.recovery) {
            (MediaSessionStateV1::Failed { reason }, _) => Some(*reason),
            (MediaSessionStateV1::Reconnecting { .. }, Some(recovery)) => Some(recovery.reason),
            _ => None,
        }
    }

    /// Manual re-dial is offered only after auto-reconnect is exhausted on a
    /// failed session. Reconnecting is still the worker's retry, not a redial.
    #[must_use]
    pub fn offers_redial(&self) -> bool {
        match (&self.state, &self.recovery) {
            (MediaSessionStateV1::Failed { .. }, Some(recovery)) => {
                recovery.redial_offered && !self.claims_live_media()
            }
            _ => false,
        }
    }

    /// Decode and admit a bounded JSON session body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json(body: &str) -> Result<Self, MediaSessionV1DecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON byte body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, MediaSessionV1DecodeError> {
        if body.len() > MAX_MEDIA_SESSION_V1_JSON_BYTES {
            return Err(MediaSessionV1DecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_MEDIA_SESSION_V1_JSON_BYTES,
            });
        }
        reject_duplicate_json_keys(body).map_err(MediaSessionV1DecodeError::Json)?;
        let session =
            serde_json::from_slice::<Self>(body).map_err(MediaSessionV1DecodeError::Json)?;
        session
            .validate()
            .map_err(MediaSessionV1DecodeError::Validation)?;
        Ok(session)
    }

    /// Validate the intrinsic contract, including the connected-requires-frames
    /// honesty lock.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), MediaSessionV1ValidationError> {
        if self.schema_version != MEDIA_SESSION_V1_SCHEMA_VERSION {
            return Err(MediaSessionV1ValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.session.is_nil() {
            return Err(MediaSessionV1ValidationError::NilSessionId);
        }
        if self.space.is_nil() {
            return Err(MediaSessionV1ValidationError::InvalidField { field: "space" });
        }
        validate_actor("local_actor", &self.local_actor)?;
        validate_actor("remote_actor", &self.remote_actor)?;
        if self.local_actor == self.remote_actor {
            return Err(MediaSessionV1ValidationError::InvalidField {
                field: "remote_actor",
            });
        }
        match self.adapter {
            CallMediaAdapter::WebRtcP2p
            | CallMediaAdapter::LiveKitSfu
            | CallMediaAdapter::SipGateway => {}
            CallMediaAdapter::DocumentCollab | CallMediaAdapter::VdiRemoteDesktop => {
                return Err(MediaSessionV1ValidationError::InvalidField { field: "adapter" });
            }
        }
        validate_tracks(&self.offered_tracks)?;
        if let Some(description) = &self.local_description {
            description.validate()?;
            if description.session != self.session
                || description.from != self.local_actor
                || description.to != self.remote_actor
                || description.tracks != self.offered_tracks
            {
                return Err(MediaSessionV1ValidationError::InvalidField {
                    field: "local_description",
                });
            }
        }
        if let Some(description) = &self.remote_description {
            description.validate()?;
            if description.session != self.session
                || description.from != self.remote_actor
                || description.to != self.local_actor
                || description.tracks != self.offered_tracks
            {
                return Err(MediaSessionV1ValidationError::InvalidField {
                    field: "remote_description",
                });
            }
        }
        match &self.state {
            MediaSessionStateV1::Connected => {
                if self.frames_observed == 0 || !self.audio_bound || !self.dtmf_bound {
                    return Err(MediaSessionV1ValidationError::ConnectedWithoutFrames);
                }
                if self.local_description.is_none() || self.remote_description.is_none() {
                    return Err(MediaSessionV1ValidationError::ConnectedWithoutFrames);
                }
            }
            MediaSessionStateV1::DeviceAbsent { track } => {
                if self.frames_observed != 0 {
                    return Err(MediaSessionV1ValidationError::UnavailableWithFrames);
                }
                if *track == MediaTrackKind::Audio && self.audio_bound {
                    return Err(MediaSessionV1ValidationError::InvalidField { field: "state" });
                }
            }
            MediaSessionStateV1::PermissionDenied { track } => {
                if self.frames_observed != 0 {
                    return Err(MediaSessionV1ValidationError::UnavailableWithFrames);
                }
                if *track == MediaTrackKind::Audio && self.audio_bound {
                    return Err(MediaSessionV1ValidationError::InvalidField { field: "state" });
                }
            }
            MediaSessionStateV1::Reconnecting { attempt } => {
                if *attempt == 0 || *attempt > MAX_MEDIA_RECONNECT_ATTEMPTS {
                    return Err(MediaSessionV1ValidationError::OutOfBounds {
                        field: "state.attempt",
                        max: u64::from(MAX_MEDIA_RECONNECT_ATTEMPTS),
                    });
                }
            }
            MediaSessionStateV1::Failed { .. } | MediaSessionStateV1::Negotiating => {}
        }
        validate_bound_devices(&self.bound_devices)?;
        validate_recovery_pairing(&self.state, self.recovery.as_ref())?;
        Ok(())
    }
}

/// Why a media session or description failed intrinsic validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSessionV1ValidationError {
    /// Schema discriminator is not supported.
    UnsupportedSchema {
        /// Version found on the wire.
        found: u16,
    },
    /// The session/call id was the nil sentinel.
    NilSessionId,
    /// A bounded value exceeded its maximum.
    OutOfBounds {
        /// Field that exceeded its bound.
        field: &'static str,
        /// Maximum admitted value.
        max: u64,
    },
    /// A field failed shape or pairing admission.
    InvalidField {
        /// Field that failed.
        field: &'static str,
    },
    /// A free-text or identity field carried a command, path, URL, or secret.
    ForbiddenValue {
        /// Field that contained the forbidden value.
        field: &'static str,
    },
    /// `connected` was claimed without proven frames and a bound audio leg.
    ConnectedWithoutFrames,
    /// An unavailable state claimed advancing frames.
    UnavailableWithFrames,
}

impl fmt::Display for MediaSessionV1ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => {
                write!(
                    formatter,
                    "unsupported media session schema version {found}"
                )
            }
            Self::NilSessionId => formatter.write_str("media session id is nil"),
            Self::OutOfBounds { field, max } => {
                write!(formatter, "media {field} exceeds bound {max}")
            }
            Self::InvalidField { field } => {
                write!(formatter, "invalid media session field {field}")
            }
            Self::ForbiddenValue { field } => {
                write!(formatter, "forbidden media value in {field}")
            }
            Self::ConnectedWithoutFrames => {
                formatter.write_str("media session cannot be connected without observed frames")
            }
            Self::UnavailableWithFrames => {
                formatter.write_str("unavailable media state cannot carry observed frames")
            }
        }
    }
}

impl std::error::Error for MediaSessionV1ValidationError {}

/// Why a JSON media body could not be decoded and admitted.
#[derive(Debug)]
pub enum MediaSessionV1DecodeError {
    /// The encoded body was rejected before serde allocation.
    BodyTooLarge {
        /// Number of bytes supplied.
        bytes: usize,
        /// Maximum encoded body size.
        max: usize,
    },
    /// The body was malformed JSON or had an unknown/duplicate wire field.
    Json(serde_json::Error),
    /// The body decoded but failed semantic validation.
    Validation(MediaSessionV1ValidationError),
}

impl fmt::Display for MediaSessionV1DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => {
                write!(
                    formatter,
                    "media session body is {bytes} bytes; maximum is {max}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid media session JSON: {error}"),
            Self::Validation(error) => write!(formatter, "invalid media session: {error}"),
        }
    }
}

impl std::error::Error for MediaSessionV1DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BodyTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

fn description_fingerprint(
    session: CallId,
    from: &ActorId,
    to: &ActorId,
    role: MediaSignalingRoleV1,
    tracks: &[MediaTrackKind],
) -> String {
    let role = match role {
        MediaSignalingRoleV1::Offer => "offer",
        MediaSignalingRoleV1::Answer => "answer",
    };
    let mut canonical = format!("v1|{session}|{from}|{to}|{role}");
    for track in tracks {
        canonical.push('|');
        canonical.push_str(track.as_str());
    }
    sha256_hex(canonical.as_bytes())
}

fn validate_actor(
    field: &'static str,
    actor: &ActorId,
) -> Result<(), MediaSessionV1ValidationError> {
    let value = actor.as_str();
    if value.is_empty() || value.len() > MAX_MEDIA_ACTOR_BYTES {
        return Err(MediaSessionV1ValidationError::OutOfBounds {
            field,
            max: MAX_MEDIA_ACTOR_BYTES as u64,
        });
    }
    if looks_forbidden(value)
        || value.bytes().any(|byte| {
            byte.is_ascii_control() || byte == b'/' || byte == b'\\' || byte.is_ascii_whitespace()
        })
    {
        return Err(MediaSessionV1ValidationError::ForbiddenValue { field });
    }
    Ok(())
}

fn validate_tracks(tracks: &[MediaTrackKind]) -> Result<(), MediaSessionV1ValidationError> {
    if tracks.is_empty() || tracks.len() > MAX_MEDIA_TRACKS {
        return Err(MediaSessionV1ValidationError::OutOfBounds {
            field: "tracks",
            max: MAX_MEDIA_TRACKS as u64,
        });
    }
    if !tracks.contains(&MediaTrackKind::Audio) {
        return Err(MediaSessionV1ValidationError::InvalidField { field: "tracks" });
    }
    for (index, track) in tracks.iter().enumerate() {
        if tracks[..index].contains(track) {
            return Err(MediaSessionV1ValidationError::InvalidField { field: "tracks" });
        }
    }
    Ok(())
}

fn looks_forbidden(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("://")
        || lower.contains("..")
        || lower.contains("command")
        || lower.contains("password")
        || lower.contains("secret")
        || lower.contains("token=")
}

fn validate_device_label(label: &str) -> Result<(), MediaSessionV1ValidationError> {
    if label.is_empty() || label.len() > MAX_MEDIA_DEVICE_LABEL_BYTES {
        return Err(MediaSessionV1ValidationError::OutOfBounds {
            field: "bound_devices.label",
            max: MAX_MEDIA_DEVICE_LABEL_BYTES as u64,
        });
    }
    if looks_forbidden(label)
        || label.contains('/')
        || label.contains('\\')
        || label.contains('@')
        || label.contains(':')
        || label
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(MediaSessionV1ValidationError::ForbiddenValue {
            field: "bound_devices.label",
        });
    }
    Ok(())
}

fn validate_bound_devices(devices: &[MediaDeviceV1]) -> Result<(), MediaSessionV1ValidationError> {
    if devices.len() > MAX_MEDIA_DEVICES {
        return Err(MediaSessionV1ValidationError::OutOfBounds {
            field: "bound_devices",
            max: MAX_MEDIA_DEVICES as u64,
        });
    }
    let mut selected = [false; 3];
    for device in devices {
        device.validate()?;
        if device.selected {
            let index = match device.kind {
                MediaTrackKind::Audio => 0,
                MediaTrackKind::Video => 1,
                MediaTrackKind::Screen => 2,
            };
            if selected[index] {
                return Err(MediaSessionV1ValidationError::InvalidField {
                    field: "bound_devices",
                });
            }
            selected[index] = true;
        }
    }
    Ok(())
}

fn validate_recovery_pairing(
    state: &MediaSessionStateV1,
    recovery: Option<&MediaRecoveryV1>,
) -> Result<(), MediaSessionV1ValidationError> {
    match (state, recovery) {
        (MediaSessionStateV1::Connected | MediaSessionStateV1::Negotiating, Some(_))
        | (
            MediaSessionStateV1::DeviceAbsent { .. } | MediaSessionStateV1::PermissionDenied { .. },
            Some(_),
        ) => Err(MediaSessionV1ValidationError::InvalidField { field: "recovery" }),
        (MediaSessionStateV1::Reconnecting { attempt }, Some(recovery)) => {
            recovery.validate()?;
            if recovery.attempt != *attempt || !recovery.auto_reconnect || recovery.redial_offered {
                return Err(MediaSessionV1ValidationError::InvalidField { field: "recovery" });
            }
            Ok(())
        }
        (MediaSessionStateV1::Failed { reason }, Some(recovery)) => {
            recovery.validate()?;
            if recovery.reason != *reason || recovery.auto_reconnect {
                return Err(MediaSessionV1ValidationError::InvalidField { field: "recovery" });
            }
            Ok(())
        }
        (_, None) => Ok(()),
    }
}

fn validate_sip_actor(field: &'static str, actor: &ActorId) -> Result<(), SipLegV1ValidationError> {
    let value = actor.as_str();
    if value.is_empty() || value.len() > MAX_MEDIA_ACTOR_BYTES {
        return Err(SipLegV1ValidationError::OutOfBounds {
            field,
            max: MAX_MEDIA_ACTOR_BYTES as u64,
        });
    }
    if looks_forbidden(value)
        || value.bytes().any(|byte| {
            byte.is_ascii_control() || byte == b'/' || byte == b'\\' || byte.is_ascii_whitespace()
        })
    {
        return Err(SipLegV1ValidationError::ForbiddenValue { field });
    }
    Ok(())
}

fn validate_e164(value: &str) -> Result<(), SipLegV1ValidationError> {
    if looks_forbidden(value)
        || value.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || byte == b'/'
                || byte == b'\\'
                || byte == b'@'
                || byte == b':'
                || byte == b';'
        })
    {
        return Err(SipLegV1ValidationError::ForbiddenValue { field: "e164" });
    }
    let Some(digits) = value.strip_prefix('+') else {
        return Err(SipLegV1ValidationError::InvalidField { field: "e164" });
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(SipLegV1ValidationError::InvalidField { field: "e164" });
    }
    if digits.len() < MIN_SIP_E164_DIGITS || digits.len() > MAX_SIP_E164_DIGITS {
        return Err(SipLegV1ValidationError::OutOfBounds {
            field: "e164",
            max: MAX_SIP_E164_DIGITS as u64,
        });
    }
    Ok(())
}

/// Why a collab-bus media board failed intrinsic validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallMediaBoardValidationError {
    /// The session/call id was the nil sentinel.
    NilSessionId,
    /// A bounded value exceeded its maximum.
    OutOfBounds {
        /// Field that exceeded its bound.
        field: &'static str,
        /// Maximum admitted value.
        max: u64,
    },
    /// A field failed shape or pairing admission.
    InvalidField {
        /// Field that failed.
        field: &'static str,
    },
    /// A free-text or identity field carried a command, path, URL, or secret.
    ForbiddenValue {
        /// Field that contained the forbidden value.
        field: &'static str,
    },
    /// `live_media_verified` was claimed without proven advancing frames.
    VerifiedWithoutFrames,
    /// An unproven/unavailable row claimed advancing-frame evidence.
    UnavailableWithFrames,
}

impl fmt::Display for CallMediaBoardValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NilSessionId => formatter.write_str("call media board session id is nil"),
            Self::OutOfBounds { field, max } => {
                write!(formatter, "call media {field} exceeds bound {max}")
            }
            Self::InvalidField { field } => write!(formatter, "invalid call media field {field}"),
            Self::ForbiddenValue { field } => {
                write!(formatter, "forbidden call media value in {field}")
            }
            Self::VerifiedWithoutFrames => {
                formatter.write_str("live_media_verified requires proven advancing frames")
            }
            Self::UnavailableWithFrames => {
                formatter.write_str("unproven call media cannot claim advancing frames")
            }
        }
    }
}

impl std::error::Error for CallMediaBoardValidationError {}

/// Why a JSON collab-bus media board could not be decoded and admitted.
#[derive(Debug)]
pub enum CallMediaBoardDecodeError {
    /// The encoded body was rejected before serde allocation.
    BodyTooLarge {
        /// Number of bytes supplied.
        bytes: usize,
        /// Maximum encoded body size.
        max: usize,
    },
    /// The body was malformed JSON or had an unknown/duplicate wire field.
    Json(serde_json::Error),
    /// The body decoded but failed semantic validation.
    Validation(CallMediaBoardValidationError),
}

impl fmt::Display for CallMediaBoardDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => {
                write!(
                    formatter,
                    "call media board body is {bytes} bytes; maximum is {max}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid call media board JSON: {error}"),
            Self::Validation(error) => write!(formatter, "invalid call media board: {error}"),
        }
    }
}

impl std::error::Error for CallMediaBoardDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BodyTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

impl CallMediaReadiness {
    /// Sidecar readiness is never live media. [`CallMediaAdmission::AdapterReady`]
    /// is signed-state admission for a worker attempt, not frames, mute, or DTMF.
    #[must_use]
    pub const fn claims_live_media(&self) -> bool {
        let _ = self;
        false
    }

    /// Decode and admit a bounded JSON readiness board.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json(body: &str) -> Result<Self, CallMediaBoardDecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON byte body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, CallMediaBoardDecodeError> {
        if body.len() > MAX_CALL_MEDIA_READINESS_JSON_BYTES {
            return Err(CallMediaBoardDecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_CALL_MEDIA_READINESS_JSON_BYTES,
            });
        }
        reject_duplicate_json_keys(body).map_err(CallMediaBoardDecodeError::Json)?;
        let board =
            serde_json::from_slice::<Self>(body).map_err(CallMediaBoardDecodeError::Json)?;
        board
            .validate()
            .map_err(CallMediaBoardDecodeError::Validation)?;
        Ok(board)
    }

    /// Validate the intrinsic readiness contract.
    ///
    /// Empty `sessions` is honest absence, not a live call.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), CallMediaBoardValidationError> {
        validate_board_actor("local_actor", &self.local_actor)?;
        if self.sessions.len() > MAX_CALL_MEDIA_SESSIONS {
            return Err(CallMediaBoardValidationError::OutOfBounds {
                field: "sessions",
                max: MAX_CALL_MEDIA_SESSIONS as u64,
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for session in &self.sessions {
            validate_readiness_session(&self.local_actor, session)?;
            if !seen.insert(session.call) {
                return Err(CallMediaBoardValidationError::InvalidField { field: "call" });
            }
        }
        Ok(())
    }
}

impl CallMediaVerification {
    /// Whether any admitted row proved advancing frames. This board still does
    /// not bind mute/DTMF; those require [`MediaSessionV1::binds_live_mute`] /
    /// [`MediaSessionV1::binds_live_dtmf`] on `state/calls/media/<session>`.
    #[must_use]
    pub fn claims_live_media(&self) -> bool {
        self.rows.iter().any(|row| row.status.claims_live_media())
    }

    /// Decode and admit a bounded JSON verification board.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json(body: &str) -> Result<Self, CallMediaBoardDecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON byte body.
    ///
    /// # Errors
    ///
    /// Returns a decode error for an oversized, malformed, unknown, duplicate,
    /// or intrinsically invalid body.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, CallMediaBoardDecodeError> {
        if body.len() > MAX_CALL_MEDIA_VERIFICATION_JSON_BYTES {
            return Err(CallMediaBoardDecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_CALL_MEDIA_VERIFICATION_JSON_BYTES,
            });
        }
        reject_duplicate_json_keys(body).map_err(CallMediaBoardDecodeError::Json)?;
        let board =
            serde_json::from_slice::<Self>(body).map_err(CallMediaBoardDecodeError::Json)?;
        board
            .validate()
            .map_err(CallMediaBoardDecodeError::Validation)?;
        Ok(board)
    }

    /// Validate the intrinsic verification contract, including the
    /// verified-requires-frames honesty lock.
    ///
    /// Empty `rows` is honest absence, not a live call.
    ///
    /// # Errors
    ///
    /// Returns the first field that fails admission.
    pub fn validate(&self) -> Result<(), CallMediaBoardValidationError> {
        validate_board_actor("local_actor", &self.local_actor)?;
        if self.rows.len() > MAX_CALL_MEDIA_VERIFICATION_ROWS {
            return Err(CallMediaBoardValidationError::OutOfBounds {
                field: "rows",
                max: MAX_CALL_MEDIA_VERIFICATION_ROWS as u64,
            });
        }
        for (index, row) in self.rows.iter().enumerate() {
            validate_verification_row(row)?;
            if self.rows[..index]
                .iter()
                .any(|prior| prior.call == row.call && prior.adapter == row.adapter)
            {
                return Err(CallMediaBoardValidationError::InvalidField { field: "rows" });
            }
        }
        Ok(())
    }
}

fn validate_readiness_session(
    local_actor: &ActorId,
    session: &CallMediaSession,
) -> Result<(), CallMediaBoardValidationError> {
    if session.call.is_nil() {
        return Err(CallMediaBoardValidationError::NilSessionId);
    }
    if session.space.is_nil() {
        return Err(CallMediaBoardValidationError::InvalidField { field: "space" });
    }
    let (requirements, adapters) = kind_media_contract(session.kind);
    if session.requirements != requirements {
        return Err(CallMediaBoardValidationError::InvalidField {
            field: "requirements",
        });
    }
    if session.candidate_adapters.is_empty()
        || session.candidate_adapters.len() > adapters.len()
        || !session
            .candidate_adapters
            .iter()
            .enumerate()
            .all(|(index, candidate)| {
                adapters.contains(candidate)
                    && !session.candidate_adapters[..index].contains(candidate)
            })
    {
        return Err(CallMediaBoardValidationError::InvalidField {
            field: "candidate_adapters",
        });
    }
    if session.connected_participants.is_empty()
        || session.connected_participants.len() > MAX_CALL_MEDIA_PARTICIPANTS
    {
        return Err(CallMediaBoardValidationError::OutOfBounds {
            field: "connected_participants",
            max: MAX_CALL_MEDIA_PARTICIPANTS as u64,
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut local_count = 0usize;
    for actor in &session.connected_participants {
        validate_board_actor("connected_participants", actor)?;
        if !seen.insert(actor.as_str()) {
            return Err(CallMediaBoardValidationError::InvalidField {
                field: "connected_participants",
            });
        }
        if actor == local_actor {
            local_count += 1;
        }
    }
    let admission_matches = match session.admission {
        CallMediaAdmission::AdapterReady => session.connected_participants.len() >= 2,
        CallMediaAdmission::WaitingForConnectedPeer => session.connected_participants.len() == 1,
    };
    if local_count != 1 || !admission_matches {
        return Err(CallMediaBoardValidationError::InvalidField { field: "admission" });
    }
    Ok(())
}

fn validate_verification_row(
    row: &CallMediaVerificationRow,
) -> Result<(), CallMediaBoardValidationError> {
    if row.call.is_nil() {
        return Err(CallMediaBoardValidationError::NilSessionId);
    }
    if row.space.is_nil() {
        return Err(CallMediaBoardValidationError::InvalidField { field: "space" });
    }
    let (_, adapters) = kind_media_contract(row.kind);
    if !adapters.contains(&row.adapter) {
        return Err(CallMediaBoardValidationError::InvalidField { field: "adapter" });
    }
    if let Some(detail) = &row.detail {
        if detail.len() > MAX_CALL_MEDIA_DETAIL_BYTES {
            return Err(CallMediaBoardValidationError::OutOfBounds {
                field: "detail",
                max: MAX_CALL_MEDIA_DETAIL_BYTES as u64,
            });
        }
        if looks_forbidden(detail)
            || detail.bytes().any(|byte| byte.is_ascii_control())
            || detail.contains('/')
            || detail.contains('\\')
        {
            return Err(CallMediaBoardValidationError::ForbiddenValue { field: "detail" });
        }
    }
    let satisfies = row
        .evidence
        .is_some_and(|evidence| evidence_satisfies_kind(row.kind, evidence));
    match row.status {
        CallMediaVerificationStatus::LiveMediaVerified => {
            if row.detail.is_some() {
                return Err(CallMediaBoardValidationError::InvalidField { field: "detail" });
            }
            if !satisfies {
                return Err(CallMediaBoardValidationError::VerifiedWithoutFrames);
            }
        }
        CallMediaVerificationStatus::MediaNotProven => {
            if satisfies {
                return Err(CallMediaBoardValidationError::InvalidField { field: "status" });
            }
        }
        CallMediaVerificationStatus::WaitingForConnectedPeer
        | CallMediaVerificationStatus::TransportUnavailable
        | CallMediaVerificationStatus::ProviderUnavailable => {
            if row.evidence.is_some() {
                return Err(CallMediaBoardValidationError::UnavailableWithFrames);
            }
        }
    }
    Ok(())
}

fn kind_media_contract(
    kind: CallKind,
) -> (&'static [CallMediaRequirement], &'static [CallMediaAdapter]) {
    match kind {
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
    }
}

fn evidence_satisfies_kind(kind: CallKind, evidence: CallMediaFrameEvidence) -> bool {
    match kind {
        CallKind::Audio => evidence.audio_frames > 0,
        CallKind::Video => evidence.audio_frames > 0 && evidence.video_frames > 0,
        CallKind::Screen => evidence.audio_frames > 0 && evidence.screen_frames > 0,
        CallKind::CoEdit => evidence.data_messages > 0,
        CallKind::RemoteDesktop => evidence.screen_frames > 0,
    }
}

fn validate_board_actor(
    field: &'static str,
    actor: &ActorId,
) -> Result<(), CallMediaBoardValidationError> {
    let value = actor.as_str();
    if value.is_empty() || value.len() > MAX_MEDIA_ACTOR_BYTES {
        return Err(CallMediaBoardValidationError::OutOfBounds {
            field,
            max: MAX_MEDIA_ACTOR_BYTES as u64,
        });
    }
    if looks_forbidden(value)
        || value.bytes().any(|byte| {
            byte.is_ascii_control() || byte == b'/' || byte == b'\\' || byte.is_ascii_whitespace()
        })
    {
        return Err(CallMediaBoardValidationError::ForbiddenValue { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_description(role: MediaSignalingRoleV1) -> MediaDescriptionV1 {
        MediaDescriptionV1::new(
            CallId::new(),
            ActorId::new("alice"),
            ActorId::new("bob"),
            role,
            vec![MediaTrackKind::Audio],
        )
        .expect("valid description")
    }

    fn sample_session() -> MediaSessionV1 {
        let session = CallId::new();
        let space = SpaceId::new();
        let local = ActorId::new("alice");
        let remote = ActorId::new("bob");
        let offer = MediaDescriptionV1::new(
            session,
            local.clone(),
            remote.clone(),
            MediaSignalingRoleV1::Offer,
            vec![MediaTrackKind::Audio],
        )
        .expect("offer");
        let answer = MediaDescriptionV1::new(
            session,
            remote.clone(),
            local.clone(),
            MediaSignalingRoleV1::Answer,
            vec![MediaTrackKind::Audio],
        )
        .expect("answer");
        MediaSessionV1::new(
            session,
            space,
            local,
            remote,
            CallMediaAdapter::WebRtcP2p,
            MediaSessionStateV1::Connected,
            vec![MediaTrackKind::Audio],
            false,
            true,
            true,
            4,
            Some(offer),
            Some(answer),
        )
        .expect("valid connected session")
    }

    fn unavailable(state: MediaSessionStateV1) -> MediaSessionV1 {
        MediaSessionV1::new(
            CallId::new(),
            SpaceId::new(),
            ActorId::new("alice"),
            ActorId::new("bob"),
            CallMediaAdapter::WebRtcP2p,
            state,
            vec![MediaTrackKind::Audio],
            false,
            false,
            false,
            0,
            None,
            None,
        )
        .expect("valid unavailable session")
    }

    #[test]
    fn connected_session_round_trips_and_is_admitted() {
        let session = sample_session();
        let body = serde_json::to_string(&session).expect("json");
        let decoded = MediaSessionV1::from_json(&body).expect("admit");
        assert_eq!(decoded, session);
        assert!(decoded.state.claims_live_media());
        assert!(decoded.claims_live_media());
        assert!(decoded.binds_live_mute());
        assert!(decoded.binds_live_dtmf());
        assert!(decoded.offered_live_track(MediaTrackKind::Audio));
        assert!(!decoded.offered_live_track(MediaTrackKind::Video));
        assert_eq!(decoded.topic(), media_session_topic(session.session));
    }

    #[test]
    fn negotiating_and_reconnecting_do_not_bind_mute_or_dtmf() {
        let mut negotiating = sample_session();
        negotiating.state = MediaSessionStateV1::Negotiating;
        negotiating.frames_observed = 0;
        negotiating
            .validate()
            .expect("negotiating with binds is valid");
        assert!(!negotiating.claims_live_media());
        assert!(!negotiating.binds_live_mute());
        assert!(!negotiating.binds_live_dtmf());
        assert!(!negotiating.offered_live_track(MediaTrackKind::Audio));

        let reconnecting = unavailable(MediaSessionStateV1::Reconnecting { attempt: 1 });
        assert!(!reconnecting.claims_live_media());
        assert!(!reconnecting.binds_live_mute());
        assert!(!reconnecting.binds_live_dtmf());
    }

    #[test]
    fn device_absent_permission_denied_reconnecting_and_failed_are_admitted() {
        for state in [
            MediaSessionStateV1::DeviceAbsent {
                track: MediaTrackKind::Audio,
            },
            MediaSessionStateV1::PermissionDenied {
                track: MediaTrackKind::Audio,
            },
            MediaSessionStateV1::Reconnecting { attempt: 1 },
            MediaSessionStateV1::Failed {
                reason: MediaFailureReasonV1::TransportUnavailable,
            },
        ] {
            let session = unavailable(state.clone());
            let body = serde_json::to_string(&session).expect("json");
            let decoded = MediaSessionV1::from_json(&body).expect("admit unavailable");
            assert_eq!(decoded.state, state);
            assert!(!decoded.state.claims_live_media());
            assert!(decoded.state.is_unavailable());
        }
    }

    #[test]
    fn connected_without_frames_is_rejected_on_the_wire() {
        let mut value = serde_json::to_value(sample_session()).expect("value");
        value["frames_observed"] = json!(0);
        assert!(matches!(
            MediaSessionV1::from_json(&value.to_string()),
            Err(MediaSessionV1DecodeError::Validation(
                MediaSessionV1ValidationError::ConnectedWithoutFrames
            ))
        ));
        value = serde_json::to_value(sample_session()).expect("value");
        value["audio_bound"] = json!(false);
        assert!(matches!(
            MediaSessionV1::from_json(&value.to_string()),
            Err(MediaSessionV1DecodeError::Validation(
                MediaSessionV1ValidationError::ConnectedWithoutFrames
            ))
        ));
    }

    #[test]
    fn unknown_schema_and_unknown_top_level_fields_are_rejected() {
        let mut value = serde_json::to_value(sample_session()).expect("value");
        value["schema_version"] = json!(2);
        assert!(matches!(
            MediaSessionV1::from_json(&value.to_string()),
            Err(MediaSessionV1DecodeError::Validation(
                MediaSessionV1ValidationError::UnsupportedSchema { found: 2 }
            ))
        ));

        let mut hostile = serde_json::to_value(sample_session()).expect("value");
        hostile["command"] = json!("rm -rf /");
        assert!(MediaSessionV1::from_json(&hostile.to_string()).is_err());
        let mut path = serde_json::to_value(sample_session()).expect("value");
        path["path"] = json!("/etc/passwd");
        assert!(MediaSessionV1::from_json(&path.to_string()).is_err());
        let mut secret = serde_json::to_value(sample_session()).expect("value");
        secret["password"] = json!("hunter2");
        assert!(MediaSessionV1::from_json(&secret.to_string()).is_err());
        let mut sdp = serde_json::to_value(sample_session()).expect("value");
        sdp["sdp"] = json!("v=0\r\no=evil");
        assert!(MediaSessionV1::from_json(&sdp.to_string()).is_err());
    }

    #[test]
    fn hostile_actors_and_duplicate_keys_fail_closed() {
        for actor in [
            "",
            "../escape",
            "https://evil.invalid",
            "alice/bob",
            "token=abc",
            "command-host",
        ] {
            let mut value = serde_json::to_value(sample_session()).expect("value");
            value["local_actor"] = json!(actor);
            assert!(
                MediaSessionV1::from_json(&value.to_string()).is_err(),
                "actor {actor:?} must fail"
            );
        }

        let session = sample_session();
        let mut body = serde_json::to_string(&session).expect("json");
        body.insert_str(1, "\"command\":\"rm\",\"command\":\"x\",");
        assert!(MediaSessionV1::from_json(&body).is_err());
    }

    #[test]
    fn description_fingerprint_mismatch_and_unknown_role_fail() {
        let mut value =
            serde_json::to_value(sample_description(MediaSignalingRoleV1::Offer)).expect("value");
        value["fingerprint_sha256_hex"] = json!("aa".repeat(32));
        assert!(matches!(
            MediaDescriptionV1::from_json(&value.to_string()),
            Err(MediaSessionV1DecodeError::Validation(
                MediaSessionV1ValidationError::InvalidField {
                    field: "fingerprint_sha256_hex"
                }
            ))
        ));
        value =
            serde_json::to_value(sample_description(MediaSignalingRoleV1::Offer)).expect("value");
        value["role"] = json!("pranswer");
        assert!(MediaDescriptionV1::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn session_rejects_description_with_different_track_set() {
        let mut session = sample_session();
        session.remote_description = Some(
            MediaDescriptionV1::new(
                session.session,
                session.remote_actor.clone(),
                session.local_actor.clone(),
                MediaSignalingRoleV1::Answer,
                vec![MediaTrackKind::Audio, MediaTrackKind::Video],
            )
            .expect("valid mismatched description"),
        );
        assert!(matches!(
            session.validate(),
            Err(MediaSessionV1ValidationError::InvalidField {
                field: "remote_description"
            })
        ));
    }

    #[test]
    fn oversized_bodies_are_rejected_before_decode() {
        let session_body = vec![b' '; MAX_MEDIA_SESSION_V1_JSON_BYTES + 1];
        assert!(matches!(
            MediaSessionV1::from_json_bytes(&session_body),
            Err(MediaSessionV1DecodeError::BodyTooLarge { .. })
        ));
        let description_body = vec![b' '; MAX_MEDIA_DESCRIPTION_V1_JSON_BYTES + 1];
        assert!(matches!(
            MediaDescriptionV1::from_json_bytes(&description_body),
            Err(MediaSessionV1DecodeError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn device_absent_cannot_claim_frames_or_a_bound_audio_leg() {
        let mut value = serde_json::to_value(unavailable(MediaSessionStateV1::DeviceAbsent {
            track: MediaTrackKind::Audio,
        }))
        .expect("value");
        value["frames_observed"] = json!(3);
        assert!(matches!(
            MediaSessionV1::from_json(&value.to_string()),
            Err(MediaSessionV1DecodeError::Validation(
                MediaSessionV1ValidationError::UnavailableWithFrames
            ))
        ));
        value = serde_json::to_value(unavailable(MediaSessionStateV1::PermissionDenied {
            track: MediaTrackKind::Audio,
        }))
        .expect("value");
        value["audio_bound"] = json!(true);
        assert!(MediaSessionV1::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn topic_helpers_are_session_scoped_and_not_collab_json_bags() {
        let session = CallId::new();
        assert_eq!(
            media_session_topic(session),
            format!("state/calls/media/{session}")
        );
        assert_eq!(
            media_offer_topic(session),
            format!("state/calls/media/{session}/offer")
        );
        assert_eq!(
            media_answer_topic(session),
            format!("state/calls/media/{session}/answer")
        );
        assert_eq!(
            media_sfu_election_topic(session),
            format!("state/calls/media/{session}/sfu")
        );
        assert_eq!(
            media_sip_leg_topic(session),
            format!("state/calls/media/{session}/sip")
        );
    }

    #[test]
    fn sfu_election_requires_three_distinct_participants_and_a_present_host() {
        let session = CallId::new();
        let alice = ActorId::new("alice");
        let bob = ActorId::new("bob");
        let carol = ActorId::new("carol");
        let election = SfuElectionV1::new(
            session,
            alice.clone(),
            true,
            vec![alice.clone(), bob.clone(), carol.clone()],
        )
        .expect("valid election");
        let json = serde_json::to_string(&election).expect("json");
        assert_eq!(
            SfuElectionV1::from_json(&json).expect("round-trip"),
            election
        );
        assert_eq!(
            SfuElectionV1::elect_host(&[carol.clone(), alice.clone(), bob.clone()], None).as_ref(),
            Some(&alice)
        );
        assert_eq!(
            SfuElectionV1::elect_host(&[alice.clone(), bob.clone(), carol.clone()], Some(&carol))
                .as_ref(),
            Some(&carol)
        );
        assert!(SfuElectionV1::new(session, alice.clone(), false, vec![alice, bob]).is_err());
    }

    fn sample_sip_leg() -> SipLegV1 {
        SipLegV1::new(
            CallId::new(),
            ActorId::new("alice"),
            SipLegDirectionV1::Outbound,
            "+15551234567",
            true,
            true,
        )
        .expect("valid bridged SIP leg")
    }

    #[test]
    fn sip_leg_round_trips_and_is_admitted() {
        let document = sample_sip_leg();
        let body = serde_json::to_string(&document).expect("json");
        let decoded = SipLegV1::from_json(&body).expect("admit");
        assert_eq!(decoded, document);
        assert!(decoded.bridged);
        assert_eq!(decoded.topic(), media_sip_leg_topic(document.session));
        assert_eq!(decoded.direction.as_str(), "outbound");
    }

    #[test]
    fn sip_leg_admits_inbound_and_honest_unavailable_gateway() {
        let inbound = SipLegV1::new(
            CallId::new(),
            ActorId::new("alice"),
            SipLegDirectionV1::Inbound,
            "+18005551212",
            true,
            false,
        )
        .expect("valid inbound offer");
        let body = serde_json::to_string(&inbound).expect("json");
        assert_eq!(SipLegV1::from_json(&body).expect("admit inbound"), inbound);

        let unavailable = SipLegV1::new(
            CallId::new(),
            ActorId::new("alice"),
            SipLegDirectionV1::Outbound,
            "+911",
            false,
            false,
        )
        .expect("honest unavailable");
        assert!(!unavailable.gateway_available);
        assert!(!unavailable.bridged);

        let max_digits = SipLegV1::new(
            CallId::new(),
            ActorId::new("alice"),
            SipLegDirectionV1::Outbound,
            "+155512345678901",
            true,
            false,
        )
        .expect("15-digit E.164 is the ITU-T maximum");
        assert_eq!(max_digits.e164.len(), 1 + MAX_SIP_E164_DIGITS);
    }

    #[test]
    fn sip_leg_bridged_without_gateway_is_rejected_on_the_wire() {
        let mut value = serde_json::to_value(sample_sip_leg()).expect("value");
        value["gateway_available"] = json!(false);
        assert!(matches!(
            SipLegV1::from_json(&value.to_string()),
            Err(SipLegV1DecodeError::Validation(
                SipLegV1ValidationError::BridgedWithoutGateway
            ))
        ));
    }

    #[test]
    fn sip_leg_e164_is_fail_closed() {
        for number in [
            "",
            "15551234567",
            "+1",
            "+12",
            "+1555123456789012",
            "+1-555-123-4567",
            "+1 5551234567",
            "tel:+15551234567",
            "sip:+15551234567@evil.invalid",
            "+1555secret",
            "+1555password",
            "+token=abc",
        ] {
            assert!(
                SipLegV1::new(
                    CallId::new(),
                    ActorId::new("alice"),
                    SipLegDirectionV1::Outbound,
                    number,
                    true,
                    false,
                )
                .is_err(),
                "e164 {number:?} must fail"
            );
        }
        let mut value = serde_json::to_value(sample_sip_leg()).expect("value");
        value["e164"] = json!("sip:+15551234567@gw");
        assert!(matches!(
            SipLegV1::from_json(&value.to_string()),
            Err(SipLegV1DecodeError::Validation(
                SipLegV1ValidationError::ForbiddenValue { field: "e164" }
            ))
        ));
    }

    #[test]
    fn sip_leg_unknown_schema_secrets_and_duplicate_keys_fail_closed() {
        let mut value = serde_json::to_value(sample_sip_leg()).expect("value");
        value["schema_version"] = json!(2);
        assert!(matches!(
            SipLegV1::from_json(&value.to_string()),
            Err(SipLegV1DecodeError::Validation(
                SipLegV1ValidationError::UnsupportedSchema { found: 2 }
            ))
        ));

        for (field, payload) in [
            ("command", json!("rm -rf /")),
            ("password", json!("hunter2")),
            ("sip_password", json!("hunter2")),
            ("authorization", json!("Bearer secret")),
            ("sdp", json!("v=0\r\no=evil")),
            ("path", json!("/etc/passwd")),
        ] {
            let mut hostile = serde_json::to_value(sample_sip_leg()).expect("value");
            hostile[field] = payload;
            assert!(
                SipLegV1::from_json(&hostile.to_string()).is_err(),
                "field {field} must fail"
            );
        }

        for actor in [
            "",
            "../escape",
            "https://evil.invalid",
            "alice/bob",
            "token=abc",
            "command-host",
        ] {
            let mut hostile = serde_json::to_value(sample_sip_leg()).expect("value");
            hostile["local_actor"] = json!(actor);
            assert!(
                SipLegV1::from_json(&hostile.to_string()).is_err(),
                "actor {actor:?} must fail"
            );
        }

        let document = sample_sip_leg();
        let mut body = serde_json::to_string(&document).expect("json");
        body.insert_str(1, "\"password\":\"x\",\"password\":\"y\",");
        assert!(SipLegV1::from_json(&body).is_err());
    }

    #[test]
    fn sip_leg_oversized_bodies_are_rejected_before_decode() {
        let body = vec![b' '; MAX_SIP_LEG_V1_JSON_BYTES + 1];
        assert!(matches!(
            SipLegV1::from_json_bytes(&body),
            Err(SipLegV1DecodeError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn s6_failure_reasons_admit_on_the_failed_ladder() {
        for (reason, token) in [
            (MediaFailureReasonV1::PeerDropped, "peer_dropped"),
            (MediaFailureReasonV1::SfuUnreachable, "sfu_unreachable"),
            (MediaFailureReasonV1::DeviceUnplugged, "device_unplugged"),
            (
                MediaFailureReasonV1::PermissionRevoked,
                "permission_revoked",
            ),
        ] {
            assert_eq!(reason.as_str(), token);
            let session = unavailable(MediaSessionStateV1::Failed { reason });
            let body = serde_json::to_string(&session).expect("json");
            assert!(
                body.contains(&format!("\"reason\":\"{token}\"")),
                "wire token {token} must be present"
            );
            let decoded = MediaSessionV1::from_json(&body).expect("admit s6 failure");
            assert_eq!(decoded.state, MediaSessionStateV1::Failed { reason });
            assert!(!decoded.state.claims_live_media());
            assert!(decoded.state.is_unavailable());
        }
    }

    #[test]
    fn mid_call_s6_failures_may_retain_prior_frames() {
        // DeviceAbsent / PermissionDenied reject frames. Mid-call unplug,
        // permission revoke, SFU loss, and peer drop happen after frames
        // were observed, so Failed + frames is honest.
        for reason in [
            MediaFailureReasonV1::PeerDropped,
            MediaFailureReasonV1::SfuUnreachable,
            MediaFailureReasonV1::DeviceUnplugged,
            MediaFailureReasonV1::PermissionRevoked,
        ] {
            let mut session = sample_session();
            session.state = MediaSessionStateV1::Failed { reason };
            session
                .validate()
                .unwrap_or_else(|error| panic!("{reason:?} with prior frames: {error}"));
            let body = serde_json::to_string(&session).expect("json");
            let decoded = MediaSessionV1::from_json(&body).expect("admit mid-call failure");
            assert_eq!(decoded.frames_observed, 4);
            assert_eq!(decoded.state, MediaSessionStateV1::Failed { reason });
            assert!(!decoded.state.claims_live_media());
        }
    }

    #[test]
    fn unknown_failure_reason_and_free_text_reasons_fail_closed() {
        let session = unavailable(MediaSessionStateV1::Failed {
            reason: MediaFailureReasonV1::PeerDropped,
        });
        let mut value = serde_json::to_value(&session).expect("value");
        for token in [
            "unknown_reason",
            "rm -rf /",
            "password",
            "secret",
            "https://evil.invalid",
            "sip:alice@evil.invalid",
            "",
        ] {
            value["state"]["reason"] = json!(token);
            assert!(
                MediaSessionV1::from_json(&value.to_string()).is_err(),
                "reason {token:?} must fail"
            );
        }

        value = serde_json::to_value(&session).expect("value");
        value["state"]["message"] = json!("hunter2");
        assert!(MediaSessionV1::from_json(&value.to_string()).is_err());
        value = serde_json::to_value(&session).expect("value");
        value["state"]["detail"] = json!("Authorization: Bearer secret");
        assert!(MediaSessionV1::from_json(&value.to_string()).is_err());
        value = serde_json::to_value(&session).expect("value");
        value["state"]["command"] = json!("rm -rf /");
        assert!(MediaSessionV1::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn reconnecting_attempt_is_fail_closed() {
        let mut value = serde_json::to_value(unavailable(MediaSessionStateV1::Reconnecting {
            attempt: 1,
        }))
        .expect("value");
        value["state"]["attempt"] = json!(0);
        assert!(matches!(
            MediaSessionV1::from_json(&value.to_string()),
            Err(MediaSessionV1DecodeError::Validation(
                MediaSessionV1ValidationError::OutOfBounds {
                    field: "state.attempt",
                    ..
                }
            ))
        ));
        value["state"]["attempt"] = json!(u64::from(MAX_MEDIA_RECONNECT_ATTEMPTS) + 1);
        assert!(matches!(
            MediaSessionV1::from_json(&value.to_string()),
            Err(MediaSessionV1DecodeError::Validation(
                MediaSessionV1ValidationError::OutOfBounds {
                    field: "state.attempt",
                    ..
                }
            ))
        ));
    }

    fn sample_readiness() -> CallMediaReadiness {
        CallMediaReadiness {
            local_actor: ActorId::new("alice"),
            sessions: vec![CallMediaSession {
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
            }],
        }
    }

    fn sample_verification() -> CallMediaVerification {
        let readiness = sample_readiness();
        let session = &readiness.sessions[0];
        CallMediaVerification {
            local_actor: ActorId::new("alice"),
            rows: vec![CallMediaVerificationRow {
                call: session.call,
                space: session.space,
                kind: CallKind::Audio,
                adapter: CallMediaAdapter::WebRtcP2p,
                status: CallMediaVerificationStatus::LiveMediaVerified,
                evidence: Some(CallMediaFrameEvidence {
                    audio_frames: 4,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                }),
                detail: None,
            }],
        }
    }

    #[test]
    fn empty_collab_media_boards_admit_as_honest_absence() {
        let readiness = CallMediaReadiness::from_json(
            r#"{"local_actor":"Basement-Test-Workstation","sessions":[]}"#,
        )
        .expect("empty readiness is honest absence");
        assert!(readiness.sessions.is_empty());
        assert!(
            !readiness.claims_live_media(),
            "empty readiness is honest absence, not a live call"
        );
        assert!(!CallMediaAdmission::AdapterReady.claims_live_media());
        assert!(!CallMediaAdmission::WaitingForConnectedPeer.claims_live_media());
        let verification = CallMediaVerification::from_json(
            r#"{"local_actor":"Basement-Test-Workstation","rows":[]}"#,
        )
        .expect("empty verification is honest absence");
        assert!(verification.rows.is_empty());
        assert!(!verification.claims_live_media());
    }

    #[test]
    fn readiness_and_verification_round_trip_through_typed_admission() {
        let readiness = sample_readiness();
        let decoded =
            CallMediaReadiness::from_json(&serde_json::to_string(&readiness).expect("json"))
                .expect("admit readiness");
        assert_eq!(decoded, readiness);

        let verification = sample_verification();
        let decoded =
            CallMediaVerification::from_json(&serde_json::to_string(&verification).expect("json"))
                .expect("admit verification");
        assert_eq!(decoded, verification);
        assert!(
            !readiness.claims_live_media(),
            "AdapterReady readiness must not claim live media"
        );
        assert!(
            verification.claims_live_media(),
            "admitted LiveMediaVerified with frames may claim live media on the sidecar board"
        );
        assert!(CallMediaVerificationStatus::LiveMediaVerified.claims_live_media());
        assert!(!CallMediaVerificationStatus::TransportUnavailable.claims_live_media());
        assert!(!CallMediaVerificationStatus::ProviderUnavailable.claims_live_media());
        assert!(!CallMediaVerificationStatus::MediaNotProven.claims_live_media());
        assert!(!CallMediaVerificationStatus::WaitingForConnectedPeer.claims_live_media());
    }

    #[test]
    fn live_media_verified_without_frames_is_rejected_on_the_wire() {
        let mut value = serde_json::to_value(sample_verification()).expect("value");
        value["rows"][0]["evidence"]["audio_frames"] = json!(0);
        assert!(matches!(
            CallMediaVerification::from_json(&value.to_string()),
            Err(CallMediaBoardDecodeError::Validation(
                CallMediaBoardValidationError::VerifiedWithoutFrames
            ))
        ));
        value = serde_json::to_value(sample_verification()).expect("value");
        value["rows"][0]["evidence"] = serde_json::Value::Null;
        assert!(matches!(
            CallMediaVerification::from_json(&value.to_string()),
            Err(CallMediaBoardDecodeError::Validation(
                CallMediaBoardValidationError::VerifiedWithoutFrames
            ))
        ));
    }

    #[test]
    fn transport_unavailable_cannot_claim_frame_evidence() {
        let mut value = serde_json::to_value(sample_verification()).expect("value");
        value["rows"][0]["status"] = json!("transport_unavailable");
        assert!(matches!(
            CallMediaVerification::from_json(&value.to_string()),
            Err(CallMediaBoardDecodeError::Validation(
                CallMediaBoardValidationError::UnavailableWithFrames
            ))
        ));
    }

    #[test]
    fn hostile_collab_media_boards_fail_closed() {
        let mut hostile = serde_json::to_value(sample_readiness()).expect("value");
        hostile["sdp"] = json!("v=0\r\no=evil");
        assert!(CallMediaReadiness::from_json(&hostile.to_string()).is_err());
        let mut path = serde_json::to_value(sample_readiness()).expect("value");
        path["path"] = json!("/etc/passwd");
        assert!(CallMediaReadiness::from_json(&path.to_string()).is_err());
        let mut secret = serde_json::to_value(sample_verification()).expect("value");
        secret["password"] = json!("hunter2");
        assert!(CallMediaVerification::from_json(&secret.to_string()).is_err());
        let mut command = serde_json::to_value(sample_verification()).expect("value");
        command["rows"][0]["detail"] = json!("rm -rf /");
        assert!(CallMediaVerification::from_json(&command.to_string()).is_err());

        let mut body = serde_json::to_string(&sample_readiness()).expect("json");
        body.insert_str(1, "\"command\":\"rm\",\"command\":\"x\",");
        assert!(CallMediaReadiness::from_json(&body).is_err());
    }

    #[test]
    fn adapter_ready_without_a_connected_peer_is_rejected() {
        let mut value = serde_json::to_value(sample_readiness()).expect("value");
        value["sessions"][0]["connected_participants"] = json!(["alice"]);
        assert!(matches!(
            CallMediaReadiness::from_json(&value.to_string()),
            Err(CallMediaBoardDecodeError::Validation(
                CallMediaBoardValidationError::InvalidField { field: "admission" }
            ))
        ));
    }

    #[test]
    fn video_kind_cannot_claim_audio_only_requirements() {
        let mut value = serde_json::to_value(sample_readiness()).expect("value");
        value["sessions"][0]["kind"] = json!("video");
        assert!(matches!(
            CallMediaReadiness::from_json(&value.to_string()),
            Err(CallMediaBoardDecodeError::Validation(
                CallMediaBoardValidationError::InvalidField {
                    field: "requirements"
                }
            ))
        ));
    }

    #[test]
    fn oversized_collab_media_boards_are_rejected_before_decode() {
        let readiness_body = vec![b' '; MAX_CALL_MEDIA_READINESS_JSON_BYTES + 1];
        assert!(matches!(
            CallMediaReadiness::from_json_bytes(&readiness_body),
            Err(CallMediaBoardDecodeError::BodyTooLarge { .. })
        ));
        let verification_body = vec![b' '; MAX_CALL_MEDIA_VERIFICATION_JSON_BYTES + 1];
        assert!(matches!(
            CallMediaVerification::from_json_bytes(&verification_body),
            Err(CallMediaBoardDecodeError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn published_devices_round_trip_and_select_one_per_kind() {
        let mic =
            MediaDeviceV1::new(MediaTrackKind::Audio, "Built-in-Mic", true, true).expect("mic");
        let cam =
            MediaDeviceV1::new(MediaTrackKind::Video, "FaceTime-HD", true, true).expect("cam");
        let session = sample_session()
            .with_bound_devices(vec![mic.clone(), cam.clone()])
            .expect("devices admit");
        let body = serde_json::to_string(&session).expect("json");
        let decoded = MediaSessionV1::from_json(&body).expect("admit devices");
        assert_eq!(decoded.selected_device(MediaTrackKind::Audio), Some(&mic));
        assert_eq!(decoded.selected_device(MediaTrackKind::Video), Some(&cam));
        assert!(decoded.selected_device(MediaTrackKind::Screen).is_none());
        assert_eq!(decoded.devices_for(MediaTrackKind::Audio).len(), 1);
        assert!(!body.contains("/dev/"));
        assert!(!decoded.offers_redial());
    }

    #[test]
    fn omitted_device_and_recovery_fields_still_admit() {
        let session = sample_session();
        let body = serde_json::to_string(&session).expect("json");
        assert!(
            !body.contains("bound_devices"),
            "empty inventory must omit the field"
        );
        assert!(
            !body.contains("recovery"),
            "absent recovery must omit the field"
        );
        let decoded = MediaSessionV1::from_json(&body).expect("old session still admits");
        assert!(decoded.bound_devices.is_empty());
        assert!(decoded.recovery.is_none());
    }

    #[test]
    fn hostile_device_labels_and_double_select_fail_closed() {
        for label in [
            "",
            "/dev/snd/pcmC0D0p",
            "hw:0,0",
            "https://evil.invalid",
            "alice@sip.invalid",
            "token=abc",
            "password",
            "default microphone",
            "mic\nline",
        ] {
            assert!(
                MediaDeviceV1::new(MediaTrackKind::Audio, label, true, true).is_err(),
                "label {label:?} must fail"
            );
        }
        let first = MediaDeviceV1::new(MediaTrackKind::Audio, "Mic-A", true, true).expect("a");
        let second = MediaDeviceV1::new(MediaTrackKind::Audio, "Mic-B", true, true).expect("b");
        assert!(sample_session()
            .with_bound_devices(vec![first, second])
            .is_err());
    }

    #[test]
    fn s6_recovery_names_reason_and_offers_redial_only_on_failed() {
        let recovery = MediaRecoveryV1::new(MediaFailureReasonV1::PeerDropped, 1, true, false)
            .expect("auto-reconnect");
        assert!(!recovery.claims_live_media());
        let reconnecting = unavailable(MediaSessionStateV1::Reconnecting { attempt: 1 })
            .with_recovery(recovery)
            .expect("reconnecting recovery");
        assert_eq!(
            reconnecting.recovery_reason(),
            Some(MediaFailureReasonV1::PeerDropped)
        );
        assert!(!reconnecting.offers_redial());
        assert!(!reconnecting.claims_live_media());

        let failed = unavailable(MediaSessionStateV1::Failed {
            reason: MediaFailureReasonV1::SfuUnreachable,
        })
        .with_recovery(
            MediaRecoveryV1::new(MediaFailureReasonV1::SfuUnreachable, 16, false, true)
                .expect("redial"),
        )
        .expect("failed recovery");
        assert!(failed.offers_redial());
        assert_eq!(
            failed.recovery_reason(),
            Some(MediaFailureReasonV1::SfuUnreachable)
        );
        let body = serde_json::to_string(&failed).expect("json");
        let decoded = MediaSessionV1::from_json(&body).expect("admit recovery");
        assert!(decoded.offers_redial());
        assert!(!decoded.claims_live_media());
    }

    #[test]
    fn connected_and_start_of_session_reject_recovery_and_dual_flags() {
        assert!(sample_session()
            .with_recovery(
                MediaRecoveryV1::new(MediaFailureReasonV1::PeerDropped, 1, false, true,)
                    .expect("recovery")
            )
            .is_err());
        assert!(unavailable(MediaSessionStateV1::DeviceAbsent {
            track: MediaTrackKind::Audio,
        })
        .with_recovery(
            MediaRecoveryV1::new(MediaFailureReasonV1::DeviceUnplugged, 1, false, true,)
                .expect("recovery")
        )
        .is_err());
        assert!(MediaRecoveryV1::new(MediaFailureReasonV1::PeerDropped, 1, true, true,).is_err());
        assert!(MediaRecoveryV1::new(MediaFailureReasonV1::PeerDropped, 0, true, false,).is_err());

        let mismatched = unavailable(MediaSessionStateV1::Failed {
            reason: MediaFailureReasonV1::PeerDropped,
        })
        .with_recovery(
            MediaRecoveryV1::new(MediaFailureReasonV1::SfuUnreachable, 1, false, true)
                .expect("other reason"),
        );
        assert!(mismatched.is_err());
    }
}

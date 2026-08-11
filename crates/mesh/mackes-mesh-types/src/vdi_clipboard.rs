//! VDI clipboard capability status shared by desktop backends and broker records.
//!
//! WL-FUNC-016 accepts RDP/SPICE clipboard work only when the backend either
//! drives the protocol's real clipboard channel or reports an explicit unsupported
//! state. This type is that shared status surface: it is serializable for retained
//! Bus records and also cheap for the RDP/SPICE session crates to expose directly.

use serde::{de, Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

/// Maximum encoded UTF-8 size of one VDI text clipboard value.
///
/// The limit is measured in bytes, not Unicode scalar values, because VDI
/// transport caps and serialized payload sizes are byte-based. Values are
/// accepted only when the complete UTF-8 encoding fits; they are never split
/// or silently truncated by this type.
pub const MAX_VDI_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;

/// The only rich VDI clipboard envelope schema admitted by this module.
pub const CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION: u16 = 2;
/// Maximum encoded JSON body accepted by [`ClipboardEnvelopeV2::from_json`].
pub const MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES: usize = 2 * 1024 * 1024;
/// Maximum ordered MIME offers carried by one envelope.
pub const MAX_CLIPBOARD_ENVELOPE_V2_MIME_OFFERS: usize = 32;
/// Maximum encoded length of one MIME offer.
pub const MAX_CLIPBOARD_ENVELOPE_V2_MIME_BYTES: usize = 128;
/// Maximum encoded length of the operator-safe preview.
pub const MAX_CLIPBOARD_ENVELOPE_V2_PREVIEW_BYTES: usize = 512;
/// Maximum encoded length of a node, seat, or session identity.
pub const MAX_CLIPBOARD_ENVELOPE_V2_IDENTITY_BYTES: usize = 128;
/// Maximum encoded length of an opaque Files reference.
pub const MAX_CLIPBOARD_ENVELOPE_V2_FILES_REFERENCE_BYTES: usize = 512;
/// Maximum content size represented by the envelope, including Files payloads.
///
/// The envelope carries only an opaque reference for Files payloads, but the
/// declared size still needs a finite admission ceiling before a consumer
/// schedules a transfer or allocates staging space.
pub const MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum lifetime of a rich clipboard envelope after its source timestamp.
pub const MAX_CLIPBOARD_ENVELOPE_V2_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// Why a decoded [`ClipboardEnvelopeV2`] was refused at the shared admission
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEnvelopeV2ValidationError {
    /// The envelope's schema discriminator is not the supported V2 value.
    UnsupportedSchema {
        /// Version found on the wire.
        found: u16,
    },
    /// A source identity is blank, overlong, or contains unsafe characters.
    InvalidIdentity {
        /// Identity field that failed validation.
        field: &'static str,
    },
    /// The source sequence is zero; sequences are one-based.
    ZeroSequence,
    /// A timestamp is zero.
    InvalidTimestamp,
    /// The envelope claims creation after the admission clock.
    FutureTimestamp {
        /// Admission time in Unix epoch milliseconds.
        now_ms: u64,
        /// Envelope creation time in Unix epoch milliseconds.
        timestamp_ms: u64,
    },
    /// Expiry is not after the timestamp or exceeds the bounded TTL.
    InvalidExpiry,
    /// A collection or string field exceeded its contract bound.
    CapacityExceeded {
        /// Field that exceeded its bound.
        field: &'static str,
        /// Maximum admitted size.
        max: usize,
    },
    /// A scalar byte count exceeded the content ceiling.
    ContentTooLarge {
        /// Declared content size.
        bytes: u64,
        /// Maximum admitted content size.
        max: u64,
    },
    /// A MIME offer is malformed.
    InvalidMimeOffer {
        /// Position in the ordered offer list.
        index: usize,
    },
    /// The ordered offer list repeats a MIME type.
    DuplicateMimeOffer {
        /// Position of the repeated offer.
        index: usize,
    },
    /// The preview contains a control character or is otherwise unsafe.
    InvalidPreview,
    /// The content hash is not a lower-case hexadecimal SHA-256 digest.
    InvalidContentHash,
    /// An inline payload's declared byte count differs from its UTF-8 size.
    InlineByteCountMismatch {
        /// Declared envelope byte count.
        declared: u64,
        /// Actual inline UTF-8 byte count.
        actual: u64,
    },
    /// An inline payload's content hash differs from its bytes.
    InlineContentHashMismatch,
    /// Neither an inline text payload nor a Files reference was supplied.
    MissingPayload,
    /// Both mutually exclusive payload representations were supplied.
    MultiplePayloads,
    /// The opaque Files reference is blank, unsafe, or path-shaped.
    InvalidFilesReference,
    /// The envelope has expired at the supplied admission time.
    Expired {
        /// Admission time in Unix epoch milliseconds.
        now_ms: u64,
        /// Envelope expiry in Unix epoch milliseconds.
        expires_at_ms: u64,
    },
    /// The claimed source does not match the trusted source context.
    IdentityMismatch {
        /// Source field that did not match.
        field: &'static str,
    },
    /// The sequence is not strictly newer than the source high-water mark.
    Replay {
        /// Previously admitted sequence.
        previous: u64,
        /// Sequence presented by the envelope.
        received: u64,
    },
}

impl fmt::Display for ClipboardEnvelopeV2ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => {
                write!(
                    formatter,
                    "unsupported clipboard envelope schema version {found}"
                )
            }
            Self::InvalidIdentity { field } => {
                write!(formatter, "invalid clipboard envelope identity: {field}")
            }
            Self::ZeroSequence => formatter.write_str("clipboard envelope sequence is zero"),
            Self::InvalidTimestamp => formatter.write_str("clipboard envelope timestamp is zero"),
            Self::FutureTimestamp {
                now_ms,
                timestamp_ms,
            } => write!(
                formatter,
                "clipboard envelope timestamp is {timestamp_ms}; admission time is {now_ms}"
            ),
            Self::InvalidExpiry => formatter.write_str("clipboard envelope expiry is invalid"),
            Self::CapacityExceeded { field, max } => {
                write!(formatter, "clipboard envelope {field} exceeds {max}")
            }
            Self::ContentTooLarge { bytes, max } => write!(
                formatter,
                "clipboard envelope content is {bytes} bytes; maximum is {max}"
            ),
            Self::InvalidMimeOffer { index } => {
                write!(
                    formatter,
                    "clipboard envelope MIME offer {index} is invalid"
                )
            }
            Self::DuplicateMimeOffer { index } => {
                write!(
                    formatter,
                    "clipboard envelope MIME offer {index} is duplicated"
                )
            }
            Self::InvalidPreview => formatter.write_str("clipboard envelope preview is invalid"),
            Self::InvalidContentHash => {
                formatter.write_str("clipboard envelope content hash is invalid")
            }
            Self::InlineByteCountMismatch { declared, actual } => write!(
                formatter,
                "inline clipboard byte count is {declared}; actual size is {actual}"
            ),
            Self::InlineContentHashMismatch => {
                formatter.write_str("inline clipboard content hash does not match the payload")
            }
            Self::MissingPayload => formatter.write_str("clipboard envelope has no payload"),
            Self::MultiplePayloads => {
                formatter.write_str("clipboard envelope has multiple payload representations")
            }
            Self::InvalidFilesReference => {
                formatter.write_str("clipboard envelope Files reference is invalid")
            }
            Self::Expired {
                now_ms,
                expires_at_ms,
            } => write!(
                formatter,
                "clipboard envelope expired at {expires_at_ms}; admission time is {now_ms}"
            ),
            Self::IdentityMismatch { field } => {
                write!(
                    formatter,
                    "clipboard envelope source identity mismatch: {field}"
                )
            }
            Self::Replay { previous, received } => write!(
                formatter,
                "clipboard envelope sequence {received} is not newer than {previous}"
            ),
        }
    }
}

impl std::error::Error for ClipboardEnvelopeV2ValidationError {}

/// Why a JSON clipboard envelope could not be admitted.
#[derive(Debug)]
pub enum ClipboardEnvelopeV2DecodeError {
    /// The encoded body was rejected before serde could allocate it.
    BodyTooLarge {
        /// Number of bytes supplied by the caller.
        bytes: usize,
        /// Maximum encoded body size.
        max: usize,
    },
    /// The body was malformed or contained an unknown field.
    Json(serde_json::Error),
    /// The body decoded but failed semantic contract validation.
    Validation(ClipboardEnvelopeV2ValidationError),
}

impl fmt::Display for ClipboardEnvelopeV2DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => write!(
                formatter,
                "clipboard envelope body is {bytes} bytes; maximum is {max}"
            ),
            Self::Json(error) => write!(formatter, "invalid clipboard envelope JSON: {error}"),
            Self::Validation(error) => write!(formatter, "invalid clipboard envelope: {error}"),
        }
    }
}

impl std::error::Error for ClipboardEnvelopeV2DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BodyTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

/// Versioned rich clipboard metadata shared by VDI, direct-DRM, and mesh
/// boundaries.
///
/// This is a contract and admission model, not a live clipboard protocol. It
/// carries either bounded UTF-8 text or an opaque Files reference; protocol
/// adapters remain responsible for moving bytes through their real channel.
/// Consumers should use [`Self::from_json_bytes`] for untrusted Bus bodies and
/// [`Self::admit`] before materializing a payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClipboardEnvelopeV2 {
    /// The schema discriminator; this must equal
    /// [`CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Stable node identity that originated the content.
    pub source_node: String,
    /// Stable enrolled seat identity that originated the content.
    pub source_seat: String,
    /// Session identity that scopes ordering and echo suppression.
    pub source_session: String,
    /// One-based source-local monotonic sequence number.
    pub sequence: u64,
    /// Unix epoch milliseconds when the source created this envelope.
    pub timestamp_ms: u64,
    /// Ordered richest-to-fallback MIME offers; order is semantically retained.
    pub mime_offers: Vec<String>,
    /// Bounded, control-free display preview. It is never the payload source.
    pub preview: String,
    /// Lower-case hexadecimal SHA-256 of the complete payload bytes.
    pub content_hash: String,
    /// Complete payload size in bytes, including out-of-band Files payloads.
    pub byte_count: u64,
    /// Inline UTF-8 text, including an empty value for an explicit clear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_text: Option<VdiClipboardText>,
    /// Opaque reference into the Files payload lane for large/binary content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_reference: Option<String>,
    /// Unix epoch milliseconds after which this envelope must not materialize.
    pub expires_at_ms: u64,
}

/// Strict serde-only wire helper for [`ClipboardEnvelopeV2`]. Semantic checks
/// are applied immediately after decoding, while the helper makes unknown JSON
/// keys fail closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipboardEnvelopeV2Wire {
    schema_version: u16,
    source_node: String,
    source_seat: String,
    source_session: String,
    sequence: u64,
    timestamp_ms: u64,
    mime_offers: Vec<String>,
    preview: String,
    content_hash: String,
    byte_count: u64,
    #[serde(default)]
    inline_text: Option<VdiClipboardText>,
    #[serde(default)]
    files_reference: Option<String>,
    expires_at_ms: u64,
}

impl<'de> Deserialize<'de> for ClipboardEnvelopeV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ClipboardEnvelopeV2Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(de::Error::custom)
    }
}

impl ClipboardEnvelopeV2 {
    /// Construct a V2 envelope whose payload is bounded inline text.
    pub fn new_inline_text(
        source_node: impl Into<String>,
        source_seat: impl Into<String>,
        source_session: impl Into<String>,
        sequence: u64,
        timestamp_ms: u64,
        mime_offers: Vec<String>,
        preview: impl Into<String>,
        inline_text: VdiClipboardText,
        expires_at_ms: u64,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        let content_hash = Self::content_hash_for(inline_text.as_str().as_bytes());
        let byte_count = inline_text.len_bytes() as u64;
        Self {
            schema_version: CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION,
            source_node: source_node.into(),
            source_seat: source_seat.into(),
            source_session: source_session.into(),
            sequence,
            timestamp_ms,
            mime_offers,
            preview: preview.into(),
            content_hash,
            byte_count,
            inline_text: Some(inline_text),
            files_reference: None,
            expires_at_ms,
        }
        .admitted()
    }

    /// Construct a V2 envelope whose bytes live in the opaque Files lane.
    pub fn new_files(
        source_node: impl Into<String>,
        source_seat: impl Into<String>,
        source_session: impl Into<String>,
        sequence: u64,
        timestamp_ms: u64,
        mime_offers: Vec<String>,
        preview: impl Into<String>,
        content_hash: impl Into<String>,
        byte_count: u64,
        files_reference: impl Into<String>,
        expires_at_ms: u64,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        Self {
            schema_version: CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION,
            source_node: source_node.into(),
            source_seat: source_seat.into(),
            source_session: source_session.into(),
            sequence,
            timestamp_ms,
            mime_offers,
            preview: preview.into(),
            content_hash: content_hash.into(),
            byte_count,
            inline_text: None,
            files_reference: Some(files_reference.into()),
            expires_at_ms,
        }
        .admitted()
    }

    /// Compute the canonical lower-case SHA-256 content address used by V2.
    #[must_use]
    pub fn content_hash_for(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut hash = String::with_capacity(digest.len() * 2);
        for byte in digest {
            let _ = write!(hash, "{byte:02x}");
        }
        hash
    }

    /// Decode a bounded JSON body and semantically admit it as V2.
    pub fn from_json(body: &str) -> Result<Self, ClipboardEnvelopeV2DecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode a bounded JSON byte body before any untrusted allocation can
    /// exceed the envelope cap.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, ClipboardEnvelopeV2DecodeError> {
        if body.len() > MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES {
            return Err(ClipboardEnvelopeV2DecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES,
            });
        }
        let wire = serde_json::from_slice::<ClipboardEnvelopeV2Wire>(body)
            .map_err(ClipboardEnvelopeV2DecodeError::Json)?;
        Self::from_wire(wire).map_err(ClipboardEnvelopeV2DecodeError::Validation)
    }

    /// Validate all intrinsic fields without consulting a clock or replay
    /// high-water mark.
    pub fn validate(&self) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        if self.schema_version != CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION {
            return Err(ClipboardEnvelopeV2ValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        validate_clipboard_identity("source_node", &self.source_node)?;
        validate_clipboard_identity("source_seat", &self.source_seat)?;
        validate_clipboard_identity("source_session", &self.source_session)?;
        if self.sequence == 0 {
            return Err(ClipboardEnvelopeV2ValidationError::ZeroSequence);
        }
        if self.timestamp_ms == 0 {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidTimestamp);
        }
        if self.expires_at_ms <= self.timestamp_ms
            || self.expires_at_ms - self.timestamp_ms > MAX_CLIPBOARD_ENVELOPE_V2_TTL_MS
        {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidExpiry);
        }
        if self.mime_offers.is_empty() {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidMimeOffer { index: 0 });
        }
        if self.mime_offers.len() > MAX_CLIPBOARD_ENVELOPE_V2_MIME_OFFERS {
            return Err(ClipboardEnvelopeV2ValidationError::CapacityExceeded {
                field: "mime_offers",
                max: MAX_CLIPBOARD_ENVELOPE_V2_MIME_OFFERS,
            });
        }
        let mut seen_mime = BTreeSet::new();
        for (index, offer) in self.mime_offers.iter().enumerate() {
            if !valid_clipboard_mime_offer(offer) {
                return Err(ClipboardEnvelopeV2ValidationError::InvalidMimeOffer { index });
            }
            if !seen_mime.insert(offer.to_ascii_lowercase()) {
                return Err(ClipboardEnvelopeV2ValidationError::DuplicateMimeOffer { index });
            }
        }
        if self.preview.len() > MAX_CLIPBOARD_ENVELOPE_V2_PREVIEW_BYTES
            || self.preview.chars().any(char::is_control)
        {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidPreview);
        }
        if !valid_clipboard_sha256(&self.content_hash) {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidContentHash);
        }
        if self.byte_count > MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES {
            return Err(ClipboardEnvelopeV2ValidationError::ContentTooLarge {
                bytes: self.byte_count,
                max: MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES,
            });
        }

        match (&self.inline_text, &self.files_reference) {
            (None, None) => return Err(ClipboardEnvelopeV2ValidationError::MissingPayload),
            (Some(_), Some(_)) => return Err(ClipboardEnvelopeV2ValidationError::MultiplePayloads),
            (Some(text), None) => {
                let actual = text.len_bytes() as u64;
                if actual > MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES {
                    return Err(ClipboardEnvelopeV2ValidationError::ContentTooLarge {
                        bytes: actual,
                        max: MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES,
                    });
                }
                if self.byte_count != actual {
                    return Err(
                        ClipboardEnvelopeV2ValidationError::InlineByteCountMismatch {
                            declared: self.byte_count,
                            actual,
                        },
                    );
                }
                if self.content_hash != Self::content_hash_for(text.as_str().as_bytes()) {
                    return Err(ClipboardEnvelopeV2ValidationError::InlineContentHashMismatch);
                }
                if !self.mime_offers.iter().any(|offer| {
                    offer
                        .split('/')
                        .next()
                        .is_some_and(|major| major.eq_ignore_ascii_case("text"))
                }) {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidMimeOffer { index: 0 });
                }
            }
            (None, Some(reference)) => {
                if !valid_clipboard_files_reference(reference) {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidFilesReference);
                }
            }
        }
        Ok(())
    }

    /// Validate the expiry against an admission clock.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        self.validate()?;
        if now_ms < self.timestamp_ms {
            return Err(ClipboardEnvelopeV2ValidationError::FutureTimestamp {
                now_ms,
                timestamp_ms: self.timestamp_ms,
            });
        }
        if now_ms >= self.expires_at_ms {
            return Err(ClipboardEnvelopeV2ValidationError::Expired {
                now_ms,
                expires_at_ms: self.expires_at_ms,
            });
        }
        Ok(())
    }

    /// Validate that the claimed source is exactly the trusted node/seat/session
    /// context supplied by the consuming adapter.
    pub fn validate_for_identity(
        &self,
        source_node: &str,
        source_seat: &str,
        source_session: &str,
    ) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        self.validate()?;
        validate_clipboard_identity("expected_source_node", source_node)?;
        validate_clipboard_identity("expected_source_seat", source_seat)?;
        validate_clipboard_identity("expected_source_session", source_session)?;
        self.validate_identity_only(source_node, source_seat, source_session)
    }

    /// Validate a strictly newer sequence from the same source identity.
    pub fn validate_replay_after(
        &self,
        previous: Option<&Self>,
    ) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        self.validate()?;
        let Some(previous) = previous else {
            return Ok(());
        };
        previous.validate()?;
        self.validate_identity_only(
            &previous.source_node,
            &previous.source_seat,
            &previous.source_session,
        )?;
        if self.sequence <= previous.sequence {
            return Err(ClipboardEnvelopeV2ValidationError::Replay {
                previous: previous.sequence,
                received: self.sequence,
            });
        }
        Ok(())
    }

    /// Perform the complete trusted admission check used before materializing a
    /// received envelope: intrinsic validation, expiry, source binding, and
    /// strictly increasing source-local sequence.
    pub fn admit(
        &self,
        expected_source_node: &str,
        expected_source_seat: &str,
        expected_source_session: &str,
        previous: Option<&Self>,
        now_ms: u64,
    ) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        self.validate_at(now_ms)?;
        validate_clipboard_identity("expected_source_node", expected_source_node)?;
        validate_clipboard_identity("expected_source_seat", expected_source_seat)?;
        validate_clipboard_identity("expected_source_session", expected_source_session)?;
        self.validate_identity_only(
            expected_source_node,
            expected_source_seat,
            expected_source_session,
        )?;
        if let Some(previous) = previous {
            previous.validate()?;
            self.validate_identity_only(
                &previous.source_node,
                &previous.source_seat,
                &previous.source_session,
            )?;
            if self.sequence <= previous.sequence {
                return Err(ClipboardEnvelopeV2ValidationError::Replay {
                    previous: previous.sequence,
                    received: self.sequence,
                });
            }
        }
        Ok(())
    }

    /// Consume and return only an intrinsically valid V2 envelope.
    pub fn admitted(self) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        self.validate()?;
        Ok(self)
    }

    fn from_wire(
        wire: ClipboardEnvelopeV2Wire,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        Self {
            schema_version: wire.schema_version,
            source_node: wire.source_node,
            source_seat: wire.source_seat,
            source_session: wire.source_session,
            sequence: wire.sequence,
            timestamp_ms: wire.timestamp_ms,
            mime_offers: wire.mime_offers,
            preview: wire.preview,
            content_hash: wire.content_hash,
            byte_count: wire.byte_count,
            inline_text: wire.inline_text,
            files_reference: wire.files_reference,
            expires_at_ms: wire.expires_at_ms,
        }
        .admitted()
    }

    fn validate_identity_only(
        &self,
        source_node: &str,
        source_seat: &str,
        source_session: &str,
    ) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        if self.source_node != source_node {
            return Err(ClipboardEnvelopeV2ValidationError::IdentityMismatch {
                field: "source_node",
            });
        }
        if self.source_seat != source_seat {
            return Err(ClipboardEnvelopeV2ValidationError::IdentityMismatch {
                field: "source_seat",
            });
        }
        if self.source_session != source_session {
            return Err(ClipboardEnvelopeV2ValidationError::IdentityMismatch {
                field: "source_session",
            });
        }
        Ok(())
    }
}

/// The only session-consent schema currently admitted by clipboard consumers.
pub const CLIPBOARD_SESSION_CONSENT_V1_SCHEMA_VERSION: u16 = 1;
/// Bus topic for authenticated clipboard session-consent controls.
///
/// The topic carries a bounded action envelope whose typed payload is an
/// explicit [`ClipboardSessionConsentV1`]. It never carries clipboard bytes.
pub const CLIPBOARD_SESSION_CONSENT_TOPIC: &str = "action/clipboard/session-consent";
/// Maximum encoded JSON body accepted by
/// [`ClipboardSessionConsentV1::from_json_bytes`]. The consent contract has no
/// payload collections, so its body bound remains intentionally small.
pub const MAX_CLIPBOARD_SESSION_CONSENT_JSON_BYTES: usize = 16 * 1024;
/// Maximum lifetime of a session clipboard consent after its latest update.
pub const MAX_CLIPBOARD_SESSION_CONSENT_TTL_MS: u64 = MAX_CLIPBOARD_ENVELOPE_V2_TTL_MS;

/// Why a decoded [`ClipboardSessionConsentV1`] was refused at the shared
/// admission boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardSessionConsentValidationError {
    /// The consent's schema discriminator is not the supported V1 value.
    UnsupportedSchema {
        /// Version found on the wire.
        found: u16,
    },
    /// A session/source identity is blank, overlong, or contains unsafe
    /// characters.
    InvalidIdentity {
        /// Identity field that failed validation.
        field: &'static str,
    },
    /// An issue or update timestamp is zero or out of order.
    InvalidTimestamp {
        /// Timestamp field that failed validation.
        field: &'static str,
    },
    /// Expiry is not after the update or exceeds the consent TTL bound.
    InvalidExpiry,
    /// A timestamp claims a consent update from the future of the admission
    /// clock.
    FutureTimestamp {
        /// Timestamp field that is in the future.
        field: &'static str,
        /// Admission time in Unix epoch milliseconds.
        now_ms: u64,
        /// Timestamp supplied by the consent.
        timestamp_ms: u64,
    },
    /// The consent is stale at the supplied admission time.
    Expired {
        /// Admission time in Unix epoch milliseconds.
        now_ms: u64,
        /// Consent expiry in Unix epoch milliseconds.
        expires_at_ms: u64,
    },
    /// The claimed source does not match the trusted source context.
    IdentityMismatch {
        /// Source field that did not match.
        field: &'static str,
    },
    /// The consent update is not strictly newer than the prior state.
    StaleUpdate {
        /// Previous update timestamp in Unix epoch milliseconds.
        previous: u64,
        /// Received update timestamp in Unix epoch milliseconds.
        received: u64,
    },
}

impl fmt::Display for ClipboardSessionConsentValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => {
                write!(
                    formatter,
                    "unsupported clipboard session consent schema version {found}"
                )
            }
            Self::InvalidIdentity { field } => {
                write!(
                    formatter,
                    "invalid clipboard session consent identity: {field}"
                )
            }
            Self::InvalidTimestamp { field } => {
                write!(
                    formatter,
                    "invalid clipboard session consent timestamp: {field}"
                )
            }
            Self::InvalidExpiry => {
                formatter.write_str("clipboard session consent expiry is invalid")
            }
            Self::FutureTimestamp {
                field,
                now_ms,
                timestamp_ms,
            } => write!(
                formatter,
                "clipboard session consent {field} is {timestamp_ms}; admission time is {now_ms}"
            ),
            Self::Expired {
                now_ms,
                expires_at_ms,
            } => write!(
                formatter,
                "clipboard session consent expired at {expires_at_ms}; admission time is {now_ms}"
            ),
            Self::IdentityMismatch { field } => write!(
                formatter,
                "clipboard session consent source identity mismatch: {field}"
            ),
            Self::StaleUpdate { previous, received } => write!(
                formatter,
                "clipboard session consent update {received} is not newer than {previous}"
            ),
        }
    }
}

impl std::error::Error for ClipboardSessionConsentValidationError {}

/// Why a JSON clipboard session-consent body could not be admitted.
#[derive(Debug)]
pub enum ClipboardSessionConsentDecodeError {
    /// The encoded body was rejected before serde could allocate it.
    BodyTooLarge {
        /// Number of bytes supplied by the caller.
        bytes: usize,
        /// Maximum encoded body size.
        max: usize,
    },
    /// The body was malformed or contained an unknown field.
    Json(serde_json::Error),
    /// The body decoded but failed semantic contract validation.
    Validation(ClipboardSessionConsentValidationError),
}

impl fmt::Display for ClipboardSessionConsentDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => write!(
                formatter,
                "clipboard session consent body is {bytes} bytes; maximum is {max}"
            ),
            Self::Json(error) => {
                write!(formatter, "invalid clipboard session consent JSON: {error}")
            }
            Self::Validation(error) => {
                write!(formatter, "invalid clipboard session consent: {error}")
            }
        }
    }
}

impl std::error::Error for ClipboardSessionConsentDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BodyTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

/// Per-session clipboard publishing consent.
///
/// Publishing is disabled unless an admitted record has `enabled: true`. The
/// contract deliberately contains only safe source/session identity, an
/// explicit state, and bounded timestamps. It carries no credentials, paths,
/// commands, MIME declarations, or payload bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClipboardSessionConsentV1 {
    /// The schema discriminator; this must equal
    /// [`CLIPBOARD_SESSION_CONSENT_V1_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Stable node identity that owns the consented session.
    pub source_node: String,
    /// Stable enrolled seat identity that owns the consented session.
    pub source_seat: String,
    /// Session identity to which this opt-in applies.
    pub source_session: String,
    /// Whether this session may publish clipboard updates.
    pub enabled: bool,
    /// Unix epoch milliseconds when this consent record was first issued.
    pub issued_at_ms: u64,
    /// Unix epoch milliseconds when the enabled state was last changed.
    pub updated_at_ms: u64,
    /// Unix epoch milliseconds after which publishing must stop.
    pub expires_at_ms: u64,
}

/// Strict serde-only wire helper for [`ClipboardSessionConsentV1`]. Semantic
/// checks are applied immediately after decoding, while unknown JSON keys fail
/// closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipboardSessionConsentV1Wire {
    schema_version: u16,
    source_node: String,
    source_seat: String,
    source_session: String,
    enabled: bool,
    issued_at_ms: u64,
    updated_at_ms: u64,
    expires_at_ms: u64,
}

impl<'de> Deserialize<'de> for ClipboardSessionConsentV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ClipboardSessionConsentV1Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(de::Error::custom)
    }
}

impl ClipboardSessionConsentV1 {
    /// Construct an initial consent record. The initial update time equals the
    /// issue time; later state changes should use [`Self::update`].
    pub fn new(
        source_node: impl Into<String>,
        source_seat: impl Into<String>,
        source_session: impl Into<String>,
        enabled: bool,
        issued_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<Self, ClipboardSessionConsentValidationError> {
        Self {
            schema_version: CLIPBOARD_SESSION_CONSENT_V1_SCHEMA_VERSION,
            source_node: source_node.into(),
            source_seat: source_seat.into(),
            source_session: source_session.into(),
            enabled,
            issued_at_ms,
            updated_at_ms: issued_at_ms,
            expires_at_ms,
        }
        .admitted()
    }

    /// Construct a newer consent state for the same source/session identity.
    /// Updates must advance strictly and receive a fresh bounded expiry.
    pub fn update(
        &self,
        enabled: bool,
        updated_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<Self, ClipboardSessionConsentValidationError> {
        let updated = Self {
            schema_version: CLIPBOARD_SESSION_CONSENT_V1_SCHEMA_VERSION,
            source_node: self.source_node.clone(),
            source_seat: self.source_seat.clone(),
            source_session: self.source_session.clone(),
            enabled,
            issued_at_ms: self.issued_at_ms,
            updated_at_ms,
            expires_at_ms,
        };
        updated.validate_update_after(Some(self))?;
        Ok(updated)
    }

    /// Decode a bounded JSON body and semantically admit it as V1 consent.
    pub fn from_json(body: &str) -> Result<Self, ClipboardSessionConsentDecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode a bounded JSON byte body before untrusted input can exceed the
    /// consent cap.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, ClipboardSessionConsentDecodeError> {
        if body.len() > MAX_CLIPBOARD_SESSION_CONSENT_JSON_BYTES {
            return Err(ClipboardSessionConsentDecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_CLIPBOARD_SESSION_CONSENT_JSON_BYTES,
            });
        }
        let wire = serde_json::from_slice::<ClipboardSessionConsentV1Wire>(body)
            .map_err(ClipboardSessionConsentDecodeError::Json)?;
        Self::from_wire(wire).map_err(ClipboardSessionConsentDecodeError::Validation)
    }

    /// Validate all intrinsic fields without consulting a clock or prior
    /// consent state.
    pub fn validate(&self) -> Result<(), ClipboardSessionConsentValidationError> {
        if self.schema_version != CLIPBOARD_SESSION_CONSENT_V1_SCHEMA_VERSION {
            return Err(ClipboardSessionConsentValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        validate_consent_identity("source_node", &self.source_node)?;
        validate_consent_identity("source_seat", &self.source_seat)?;
        validate_consent_identity("source_session", &self.source_session)?;
        if self.issued_at_ms == 0 {
            return Err(ClipboardSessionConsentValidationError::InvalidTimestamp {
                field: "issued_at_ms",
            });
        }
        if self.updated_at_ms < self.issued_at_ms {
            return Err(ClipboardSessionConsentValidationError::InvalidTimestamp {
                field: "updated_at_ms",
            });
        }
        if self.expires_at_ms <= self.updated_at_ms
            || self.expires_at_ms - self.updated_at_ms > MAX_CLIPBOARD_SESSION_CONSENT_TTL_MS
        {
            return Err(ClipboardSessionConsentValidationError::InvalidExpiry);
        }
        Ok(())
    }

    /// Validate freshness against an admission clock. An expired record is
    /// stale and must not authorize clipboard publication.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), ClipboardSessionConsentValidationError> {
        self.validate()?;
        if now_ms < self.updated_at_ms {
            return Err(ClipboardSessionConsentValidationError::FutureTimestamp {
                field: "updated_at_ms",
                now_ms,
                timestamp_ms: self.updated_at_ms,
            });
        }
        if now_ms >= self.expires_at_ms {
            return Err(ClipboardSessionConsentValidationError::Expired {
                now_ms,
                expires_at_ms: self.expires_at_ms,
            });
        }
        Ok(())
    }

    /// Validate that the consent is bound to the trusted node/seat/session
    /// context supplied by a daemon or transport adapter.
    pub fn validate_for_identity(
        &self,
        source_node: &str,
        source_seat: &str,
        source_session: &str,
    ) -> Result<(), ClipboardSessionConsentValidationError> {
        self.validate()?;
        validate_consent_identity("expected_source_node", source_node)?;
        validate_consent_identity("expected_source_seat", source_seat)?;
        validate_consent_identity("expected_source_session", source_session)?;
        self.validate_identity_only(source_node, source_seat, source_session)
    }

    /// Validate that this record is a strictly newer state for the same
    /// source/session as the prior record.
    pub fn validate_update_after(
        &self,
        previous: Option<&Self>,
    ) -> Result<(), ClipboardSessionConsentValidationError> {
        self.validate()?;
        let Some(previous) = previous else {
            return Ok(());
        };
        previous.validate()?;
        self.validate_identity_only(
            &previous.source_node,
            &previous.source_seat,
            &previous.source_session,
        )?;
        if self.updated_at_ms <= previous.updated_at_ms {
            return Err(ClipboardSessionConsentValidationError::StaleUpdate {
                previous: previous.updated_at_ms,
                received: self.updated_at_ms,
            });
        }
        Ok(())
    }

    /// Perform the complete trusted admission check for a consent record.
    pub fn admit(
        &self,
        expected_source_node: &str,
        expected_source_seat: &str,
        expected_source_session: &str,
        previous: Option<&Self>,
        now_ms: u64,
    ) -> Result<(), ClipboardSessionConsentValidationError> {
        self.validate_at(now_ms)?;
        self.validate_for_identity(
            expected_source_node,
            expected_source_seat,
            expected_source_session,
        )?;
        self.validate_update_after(previous)
    }

    /// Return whether this consent authorizes publication at `now_ms` after
    /// validating freshness and intrinsic state.
    pub fn allows_clipboard_at(
        &self,
        expected_source_node: &str,
        expected_source_seat: &str,
        expected_source_session: &str,
        previous: Option<&Self>,
        now_ms: u64,
    ) -> Result<bool, ClipboardSessionConsentValidationError> {
        self.admit(
            expected_source_node,
            expected_source_seat,
            expected_source_session,
            previous,
            now_ms,
        )?;
        Ok(self.enabled)
    }

    /// Whether this validated record explicitly enables publication.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Consume and return only an intrinsically valid V1 consent record.
    pub fn admitted(self) -> Result<Self, ClipboardSessionConsentValidationError> {
        self.validate()?;
        Ok(self)
    }

    fn from_wire(
        wire: ClipboardSessionConsentV1Wire,
    ) -> Result<Self, ClipboardSessionConsentValidationError> {
        Self {
            schema_version: wire.schema_version,
            source_node: wire.source_node,
            source_seat: wire.source_seat,
            source_session: wire.source_session,
            enabled: wire.enabled,
            issued_at_ms: wire.issued_at_ms,
            updated_at_ms: wire.updated_at_ms,
            expires_at_ms: wire.expires_at_ms,
        }
        .admitted()
    }

    fn validate_identity_only(
        &self,
        source_node: &str,
        source_seat: &str,
        source_session: &str,
    ) -> Result<(), ClipboardSessionConsentValidationError> {
        if self.source_node != source_node {
            return Err(ClipboardSessionConsentValidationError::IdentityMismatch {
                field: "source_node",
            });
        }
        if self.source_seat != source_seat {
            return Err(ClipboardSessionConsentValidationError::IdentityMismatch {
                field: "source_seat",
            });
        }
        if self.source_session != source_session {
            return Err(ClipboardSessionConsentValidationError::IdentityMismatch {
                field: "source_session",
            });
        }
        Ok(())
    }
}

/// Compatibility-free ergonomic name for the currently admitted consent
/// contract. Future schema versions should introduce a new explicit type.
pub type ClipboardSessionConsent = ClipboardSessionConsentV1;

fn validate_consent_identity(
    field: &'static str,
    value: &str,
) -> Result<(), ClipboardSessionConsentValidationError> {
    validate_clipboard_identity(field, value)
        .map_err(|_| ClipboardSessionConsentValidationError::InvalidIdentity { field })
}

fn validate_clipboard_identity(
    field: &'static str,
    value: &str,
) -> Result<(), ClipboardEnvelopeV2ValidationError> {
    if value.is_empty()
        || value.len() > MAX_CLIPBOARD_ENVELOPE_V2_IDENTITY_BYTES
        || value.trim() != value
        || value.bytes().any(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'_' | b'-' | b':' | b'@')
        })
    {
        return Err(ClipboardEnvelopeV2ValidationError::InvalidIdentity { field });
    }
    Ok(())
}

fn valid_clipboard_mime_offer(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_CLIPBOARD_ENVELOPE_V2_MIME_BYTES
        || value.trim() != value
        || value
            .bytes()
            .any(|byte| !byte.is_ascii() || byte.is_ascii_whitespace() || byte.is_ascii_control())
        || value.bytes().filter(|byte| *byte == b'/').count() != 1
    {
        return false;
    }
    let Some((major, remainder)) = value.split_once('/') else {
        return false;
    };
    let mut subtype_and_parameters = remainder.split(';');
    let Some(subtype) = subtype_and_parameters.next() else {
        return false;
    };
    if !valid_clipboard_mime_token(major) || !valid_clipboard_mime_token(subtype) {
        return false;
    }
    subtype_and_parameters.all(|parameter| {
        let Some((name, parameter_value)) = parameter.split_once('=') else {
            return false;
        };
        !name.is_empty()
            && !parameter_value.is_empty()
            && valid_clipboard_mime_token(name)
            && valid_clipboard_mime_token(parameter_value)
    })
}

fn valid_clipboard_mime_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'&'
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_clipboard_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_clipboard_files_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CLIPBOARD_ENVELOPE_V2_FILES_REFERENCE_BYTES
        && value.trim() == value
        && !value.contains("..")
        && !value.contains('\\')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'@' | b'/')
        })
}

/// Node-local, latest-wins handoff from an authorized daemon action to the
/// direct DRM seat. This is deliberately separate from the replicated
/// `event/clipboard/clip` history lane: a guest copy addressed to one seat must
/// not be replayed into every VDI session on that node.
pub const CLIPBOARD_MATERIALIZATION_TOPIC: &str = "state/clipboard/materialize";

/// Maximum time a seat may defer consuming a daemon materialization. A stale
/// handoff is ignored rather than pasted after a shell restart.
pub const CLIPBOARD_MATERIALIZATION_MAX_AGE_SECS: i64 = 60;

/// Schema version for the bounded VDI guest clipboard transport.
pub const VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION: u16 = 2;
/// Maximum encoded transport body admitted before JSON decoding.
pub const MAX_VDI_CLIPBOARD_TRANSPORT_V2_JSON_BYTES: usize =
    MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES + 16 * 1024;
/// Maximum lifetime of one VDI clipboard lease. Long-running sessions rotate
/// leases; they never turn one attachment token into an unbounded capability.
pub const MAX_VDI_CLIPBOARD_LEASE_TTL_MS: u64 = 5 * 60 * 1_000;
/// Maximum compressed source bytes and maximum expanded CF_DIB/CF_DIBV5 bytes
/// admitted by the RDP image adapter. This deliberately does not inherit the
/// generic 4-GiB Files-envelope ceiling: one clipboard image is materialized in
/// the seat process and must remain comfortably below its cgroup memory limit.
pub const MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Schema for the root-local, descriptor-backed Files materialization request.
pub const VDI_CLIPBOARD_FILES_MATERIALIZATION_SCHEMA_VERSION: u16 = 1;
/// Maximum JSON request or response packet on the local authority socket.
pub const MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES: usize = 16 * 1024;
/// Socket filename below the canonical shared Bus root. Payload bytes never
/// enter the Bus; successful replies carry one read-only descriptor.
pub const VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET: &str =
    "vdi-clipboard-files-materializer.sock";

/// One-use, payload-free request for the daemon Files authority to open an
/// already-admitted image representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VdiClipboardFilesMaterializationRequestV1 {
    /// Closed request schema.
    pub schema_version: u16,
    /// Fresh shell-minted one-use authorization identity.
    pub authorization_id: String,
    /// Exact live VDI session.
    pub session_id: String,
    /// Exact live attachment generation.
    pub generation: u64,
    /// Exact short-lived clipboard lease.
    pub lease_id: String,
    /// Exact lease expiry snapshot.
    pub lease_expires_at_ms: u64,
    /// Exact command sequence within the lease.
    pub message_sequence: u64,
    /// Selected image MIME representation.
    pub selected_mime: String,
    /// Digest of the complete Files object.
    pub content_hash: String,
    /// Exact Files object byte count.
    pub byte_count: u64,
    /// Opaque Files identity; never a path or URL.
    pub files_reference: String,
    /// Exact Clipboard V2 envelope expiry.
    pub envelope_expires_at_ms: u64,
}

impl VdiClipboardFilesMaterializationRequestV1 {
    /// Construct the exact payload-free request for an admitted VDI command.
    pub fn from_message(
        message: &VdiClipboardMessageV2,
        authorization_id: impl Into<String>,
    ) -> Result<Self, VdiClipboardFilesMaterializationErrorV1> {
        let files_reference = message
            .envelope
            .files_reference
            .clone()
            .ok_or(VdiClipboardFilesMaterializationErrorV1::UnsupportedPayload)?;
        let request = Self {
            schema_version: VDI_CLIPBOARD_FILES_MATERIALIZATION_SCHEMA_VERSION,
            authorization_id: authorization_id.into(),
            session_id: message.session_id.clone(),
            generation: message.generation,
            lease_id: message.lease_id.clone(),
            lease_expires_at_ms: message.lease_expires_at_ms,
            message_sequence: message.message_sequence,
            selected_mime: message.selected_mime.clone(),
            content_hash: message.envelope.content_hash.clone(),
            byte_count: message.envelope.byte_count,
            files_reference,
            envelope_expires_at_ms: message.envelope.expires_at_ms,
        };
        request.validate()?;
        Ok(request)
    }

    /// Validate intrinsic bounds before opening Bus state or Files metadata.
    pub fn validate(&self) -> Result<(), VdiClipboardFilesMaterializationErrorV1> {
        if self.schema_version != VDI_CLIPBOARD_FILES_MATERIALIZATION_SCHEMA_VERSION {
            return Err(VdiClipboardFilesMaterializationErrorV1::UnsupportedSchema);
        }
        for value in [&self.authorization_id, &self.session_id, &self.lease_id] {
            validate_clipboard_identity("materialization_identity", value)
                .map_err(|_| VdiClipboardFilesMaterializationErrorV1::InvalidIdentity)?;
        }
        if self.generation == 0
            || self.message_sequence == 0
            || self.lease_expires_at_ms == 0
            || self.envelope_expires_at_ms == 0
        {
            return Err(VdiClipboardFilesMaterializationErrorV1::InvalidIdentity);
        }
        if !matches!(
            self.selected_mime.to_ascii_lowercase().as_str(),
            "image/png" | "image/jpeg"
        ) {
            return Err(VdiClipboardFilesMaterializationErrorV1::UnsupportedMime);
        }
        if self.byte_count == 0 || self.byte_count > MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES {
            return Err(VdiClipboardFilesMaterializationErrorV1::Oversized);
        }
        if !valid_clipboard_sha256(&self.content_hash) {
            return Err(VdiClipboardFilesMaterializationErrorV1::MetadataMismatch);
        }
        if !valid_clipboard_files_reference(&self.files_reference) {
            return Err(VdiClipboardFilesMaterializationErrorV1::InvalidFilesReference);
        }
        Ok(())
    }

    /// Rebind the request to the exact current command and lease immediately
    /// before daemon-side Files resolution.
    pub fn validate_against(
        &self,
        message: &VdiClipboardMessageV2,
        lease: &VdiClipboardLeaseV2,
        now_ms: u64,
    ) -> Result<(), VdiClipboardFilesMaterializationErrorV1> {
        self.validate()?;
        message
            .admit(lease, None, now_ms)
            .map_err(|_| VdiClipboardFilesMaterializationErrorV1::LeaseMismatch)?;
        if now_ms >= self.envelope_expires_at_ms
            || self.session_id != message.session_id
            || self.generation != message.generation
            || self.lease_id != message.lease_id
            || self.lease_expires_at_ms != message.lease_expires_at_ms
            || self.message_sequence != message.message_sequence
            || !self
                .selected_mime
                .eq_ignore_ascii_case(&message.selected_mime)
            || self.content_hash != message.envelope.content_hash
            || self.byte_count != message.envelope.byte_count
            || message.envelope.files_reference.as_deref() != Some(&self.files_reference)
            || self.envelope_expires_at_ms != message.envelope.expires_at_ms
        {
            return Err(VdiClipboardFilesMaterializationErrorV1::MetadataMismatch);
        }
        Ok(())
    }
}

/// Closed refusal vocabulary for descriptor-backed image materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VdiClipboardFilesMaterializationErrorV1 {
    /// Request schema is not understood.
    UnsupportedSchema,
    /// One or more binding identities are malformed or zero.
    InvalidIdentity,
    /// The selected MIME is not an admitted PNG/JPEG representation.
    UnsupportedMime,
    /// The command does not carry one Files-backed payload.
    UnsupportedPayload,
    /// Source or response exceeds the dedicated RDP image ceiling.
    Oversized,
    /// The opaque Files identity is malformed.
    InvalidFilesReference,
    /// The lease or envelope has expired.
    Expired,
    /// Current daemon lease/command state does not match the request.
    LeaseMismatch,
    /// Digest, length, MIME, or command metadata changed.
    MetadataMismatch,
    /// Authorization or exact command was already consumed.
    Replayed,
    /// Files has no readable current generation.
    FilesUnavailable,
    /// Files explicitly denied source access.
    FilesDenied,
    /// The daemon authority or its bounded ledger is unavailable.
    AuthorityUnavailable,
}

/// Payload-free response metadata. A `Ready` response is valid only when the
/// same local packet carries exactly one read-only descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum VdiClipboardFilesMaterializationResponseV1 {
    /// One verified descriptor accompanies this response packet.
    Ready {
        /// Exact one-use authorization identity.
        authorization_id: String,
        /// Exact selected image MIME.
        selected_mime: String,
        /// Verified descriptor content digest.
        content_hash: String,
        /// Verified descriptor byte count.
        byte_count: u64,
    },
    /// No descriptor was released.
    Refused {
        /// Authorization identity copied from the request when decodable.
        authorization_id: String,
        /// Closed refusal reason.
        reason: VdiClipboardFilesMaterializationErrorV1,
    },
}
/// Typed host-to-guest command lane. Append the validated session identity with
/// [`vdi_clipboard_session_topic`].
pub const VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX: &str = "state/clipboard/vdi-v2/host-to-guest";
/// Typed guest-to-host event lane. Append the validated session identity with
/// [`vdi_clipboard_session_topic`].
pub const VDI_CLIPBOARD_GUEST_TO_HOST_TOPIC_PREFIX: &str = "event/clipboard/vdi-v2/guest-to-host";
/// Payload-free acknowledgement lane used to suppress reconnect replay.
pub const VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX: &str = "state/clipboard/vdi-v2/receipt";
/// Current short-lived capability advertised by a live VDI adapter.
pub const VDI_CLIPBOARD_LEASE_TOPIC_PREFIX: &str = "state/clipboard/vdi-v2/lease";

/// Source policy classification carried across the VDI boundary. A secret is
/// representable for a typed refusal but can never be materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VdiClipboardDisclosureV2 {
    /// The source policy permits this content to cross the guest boundary.
    Shareable,
    /// The source classified the content as secret-bearing.
    Secret,
}

/// A short-lived, payload-free capability for one live guest attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VdiClipboardLeaseV2 {
    /// Closed schema discriminator.
    pub schema_version: u16,
    /// Exact broker/session identity.
    pub session_id: String,
    /// Monotonic live-attachment generation.
    pub generation: u64,
    /// One lease identity within the attachment generation.
    pub lease_id: String,
    /// Lease issuance in Unix epoch milliseconds.
    pub issued_at_ms: u64,
    /// Exclusive lease expiry in Unix epoch milliseconds.
    pub expires_at_ms: u64,
    /// Ordered MIME types this concrete protocol adapter can really consume.
    pub permitted_mime_offers: Vec<String>,
}

impl VdiClipboardLeaseV2 {
    /// Validate identity, time bounds, and the finite protocol offer set.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), VdiClipboardTransportV2Error> {
        if self.schema_version != VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION {
            return Err(VdiClipboardTransportV2Error::UnsupportedSchema);
        }
        validate_clipboard_identity("session_id", &self.session_id)
            .map_err(|_| VdiClipboardTransportV2Error::InvalidIdentity)?;
        validate_clipboard_identity("lease_id", &self.lease_id)
            .map_err(|_| VdiClipboardTransportV2Error::InvalidIdentity)?;
        if self.generation == 0 || self.issued_at_ms == 0 {
            return Err(VdiClipboardTransportV2Error::InvalidIdentity);
        }
        if self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > MAX_VDI_CLIPBOARD_LEASE_TTL_MS
        {
            return Err(VdiClipboardTransportV2Error::InvalidLease);
        }
        if now_ms < self.issued_at_ms || now_ms >= self.expires_at_ms {
            return Err(VdiClipboardTransportV2Error::ExpiredLease);
        }
        if self.permitted_mime_offers.is_empty()
            || self.permitted_mime_offers.len() > MAX_CLIPBOARD_ENVELOPE_V2_MIME_OFFERS
        {
            return Err(VdiClipboardTransportV2Error::UnsupportedMime);
        }
        let mut seen = BTreeSet::new();
        for offer in &self.permitted_mime_offers {
            if !valid_clipboard_mime_offer(offer)
                || !seen.insert(offer.to_ascii_lowercase())
                || secret_bearing_mime(offer)
            {
                return Err(VdiClipboardTransportV2Error::UnsupportedMime);
            }
        }
        Ok(())
    }

    /// Whether this exact live lease truthfully permits `mime`.
    #[must_use]
    pub fn permits_mime(&self, mime: &str) -> bool {
        self.permitted_mime_offers
            .iter()
            .any(|permitted| permitted.eq_ignore_ascii_case(mime))
    }
}

/// One negotiated Clipboard V2 transfer bound to a live VDI lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VdiClipboardMessageV2 {
    /// Closed schema discriminator.
    pub schema_version: u16,
    /// Exact broker/session identity copied from the active lease.
    pub session_id: String,
    /// Exact attachment generation copied from the active lease.
    pub generation: u64,
    /// Exact short-lived lease identity.
    pub lease_id: String,
    /// Exact expiry snapshot; a rotated/refreshed lease invalidates old bodies.
    pub lease_expires_at_ms: u64,
    /// One-based sequence within this exact lease.
    pub message_sequence: u64,
    /// MIME representation selected from both the envelope and lease offers.
    pub selected_mime: String,
    /// Explicit source policy classification.
    pub disclosure: VdiClipboardDisclosureV2,
    /// Existing bounded Clipboard V2 payload and source ordering metadata.
    pub envelope: ClipboardEnvelopeV2,
}

impl VdiClipboardMessageV2 {
    /// Decode a bounded strict JSON body. Duplicate or unknown fields fail
    /// before the body can reach a protocol adapter.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, VdiClipboardTransportV2Error> {
        if body.len() > MAX_VDI_CLIPBOARD_TRANSPORT_V2_JSON_BYTES {
            return Err(VdiClipboardTransportV2Error::BodyTooLarge);
        }
        let text =
            std::str::from_utf8(body).map_err(|_| VdiClipboardTransportV2Error::MalformedBody)?;
        crate::workloads::reject_duplicate_json_keys(text)
            .map_err(|_| VdiClipboardTransportV2Error::MalformedBody)?;
        serde_json::from_slice(body).map_err(|_| VdiClipboardTransportV2Error::MalformedBody)
    }

    /// Admit this message only for an exact live lease and strictly newer
    /// payload-free receipt.
    #[allow(clippy::suspicious_operation_groupings)]
    pub fn admit(
        &self,
        lease: &VdiClipboardLeaseV2,
        previous: Option<&VdiClipboardReceiptV2>,
        now_ms: u64,
    ) -> Result<(), VdiClipboardTransportV2Error> {
        lease.validate_at(now_ms)?;
        if self.schema_version != VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION {
            return Err(VdiClipboardTransportV2Error::UnsupportedSchema);
        }
        if self.session_id != lease.session_id
            || self.generation != lease.generation
            || self.lease_id != lease.lease_id
            || self.lease_expires_at_ms != lease.expires_at_ms
        {
            return Err(VdiClipboardTransportV2Error::LeaseIdentityMismatch);
        }
        if self.message_sequence == 0 {
            return Err(VdiClipboardTransportV2Error::InvalidSequence);
        }
        if self.disclosure == VdiClipboardDisclosureV2::Secret
            || secret_bearing_mime(&self.selected_mime)
        {
            return Err(VdiClipboardTransportV2Error::SecretBearing);
        }
        if !lease.permits_mime(&self.selected_mime)
            || !self
                .envelope
                .mime_offers
                .iter()
                .any(|offer| offer.eq_ignore_ascii_case(&self.selected_mime))
        {
            return Err(VdiClipboardTransportV2Error::UnsupportedMime);
        }
        self.envelope
            .validate_at(now_ms)
            .map_err(VdiClipboardTransportV2Error::InvalidEnvelope)?;

        if let Some(text) = self.envelope.inline_text.as_ref() {
            if !self
                .selected_mime
                .split_once('/')
                .is_some_and(|(major, _)| major.eq_ignore_ascii_case("text"))
                || text.len_bytes() > MAX_VDI_CLIPBOARD_TEXT_BYTES
            {
                return Err(VdiClipboardTransportV2Error::UnsupportedPayload);
            }
        }

        if let Some(receipt) = previous {
            receipt.validate()?;
            if receipt.session_id != self.session_id {
                return Err(VdiClipboardTransportV2Error::LeaseIdentityMismatch);
            }
            let same_envelope_source = receipt.source_node == self.envelope.source_node
                && receipt.source_seat == self.envelope.source_seat
                && receipt.source_session == self.envelope.source_session;
            if same_envelope_source && self.envelope.sequence <= receipt.envelope_sequence {
                return Err(VdiClipboardTransportV2Error::Replay);
            }
            let same_lease = receipt.generation == self.generation
                && receipt.lease_id == self.lease_id
                && receipt.lease_expires_at_ms == self.lease_expires_at_ms;
            if same_lease && self.message_sequence <= receipt.message_sequence {
                return Err(VdiClipboardTransportV2Error::Replay);
            }
        }
        Ok(())
    }

    /// Build the payload-free receipt persisted only after protocol delivery.
    #[must_use]
    pub fn receipt(&self) -> VdiClipboardReceiptV2 {
        VdiClipboardReceiptV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: self.session_id.clone(),
            generation: self.generation,
            lease_id: self.lease_id.clone(),
            lease_expires_at_ms: self.lease_expires_at_ms,
            message_sequence: self.message_sequence,
            source_node: self.envelope.source_node.clone(),
            source_seat: self.envelope.source_seat.clone(),
            source_session: self.envelope.source_session.clone(),
            envelope_sequence: self.envelope.sequence,
            content_hash: self.envelope.content_hash.clone(),
        }
    }
}

/// Payload-free delivery cursor. Persisting this record makes an adapter
/// reconnect idempotent without retaining clipboard bytes or host paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VdiClipboardReceiptV2 {
    /// Closed schema discriminator.
    pub schema_version: u16,
    /// Exact broker/session identity.
    pub session_id: String,
    /// Exact attachment generation.
    pub generation: u64,
    /// Exact short-lived lease identity.
    pub lease_id: String,
    /// Exact lease expiry snapshot.
    pub lease_expires_at_ms: u64,
    /// Highest delivered message sequence.
    pub message_sequence: u64,
    /// Clipboard V2 source node for the cross-lease replay high-water mark.
    pub source_node: String,
    /// Clipboard V2 source seat for the cross-lease replay high-water mark.
    pub source_seat: String,
    /// Clipboard V2 source session for the cross-lease replay high-water mark.
    pub source_session: String,
    /// Highest delivered Clipboard V2 source sequence.
    pub envelope_sequence: u64,
    /// Payload digest only; clipboard bytes are never retained here.
    pub content_hash: String,
}

impl VdiClipboardReceiptV2 {
    /// Validate a receipt before using it as a replay high-water mark.
    pub fn validate(&self) -> Result<(), VdiClipboardTransportV2Error> {
        if self.schema_version != VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION {
            return Err(VdiClipboardTransportV2Error::UnsupportedSchema);
        }
        validate_clipboard_identity("session_id", &self.session_id)
            .map_err(|_| VdiClipboardTransportV2Error::InvalidIdentity)?;
        validate_clipboard_identity("lease_id", &self.lease_id)
            .map_err(|_| VdiClipboardTransportV2Error::InvalidIdentity)?;
        for value in [&self.source_node, &self.source_seat, &self.source_session] {
            validate_clipboard_identity("source", value)
                .map_err(|_| VdiClipboardTransportV2Error::InvalidIdentity)?;
        }
        if self.generation == 0
            || self.message_sequence == 0
            || self.envelope_sequence == 0
            || self.lease_expires_at_ms == 0
            || !valid_clipboard_sha256(&self.content_hash)
        {
            return Err(VdiClipboardTransportV2Error::InvalidReceipt);
        }
        Ok(())
    }
}

/// Stable, log-safe refusal vocabulary for the VDI transport boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VdiClipboardTransportV2Error {
    /// The body or lease uses an unknown schema version.
    UnsupportedSchema,
    /// The encoded body exceeded its pre-parse byte ceiling.
    BodyTooLarge,
    /// JSON, duplicate fields, or the closed wire shape was invalid.
    MalformedBody,
    /// A session, generation, or lease identity was unsafe or zero.
    InvalidIdentity,
    /// Lease issuance, expiry ordering, or lifetime was invalid.
    InvalidLease,
    /// The lease is not current at the injected admission time.
    ExpiredLease,
    /// Session, generation, lease, or expiry did not exactly match.
    LeaseIdentityMismatch,
    /// Message ordering did not start at one.
    InvalidSequence,
    /// The selected MIME was not offered by both source and adapter.
    UnsupportedMime,
    /// Source policy classified the representation as secret-bearing.
    SecretBearing,
    /// The concrete protocol cannot materialize this payload representation.
    UnsupportedPayload,
    /// The exact lease sequence was already delivered.
    Replay,
    /// A payload-free delivery receipt was malformed.
    InvalidReceipt,
    /// The nested Clipboard V2 envelope failed admission.
    InvalidEnvelope(ClipboardEnvelopeV2ValidationError),
}

impl fmt::Display for VdiClipboardTransportV2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UnsupportedSchema => "unsupported VDI clipboard transport schema",
            Self::BodyTooLarge => "VDI clipboard transport body is oversized",
            Self::MalformedBody => "VDI clipboard transport body is malformed",
            Self::InvalidIdentity => "VDI clipboard transport identity is invalid",
            Self::InvalidLease => "VDI clipboard lease bounds are invalid",
            Self::ExpiredLease => "VDI clipboard lease is not current",
            Self::LeaseIdentityMismatch => "VDI clipboard lease identity does not match",
            Self::InvalidSequence => "VDI clipboard message sequence is invalid",
            Self::UnsupportedMime => "VDI clipboard MIME is unsupported",
            Self::SecretBearing => "VDI clipboard source policy refused secret-bearing content",
            Self::UnsupportedPayload => "VDI clipboard payload is unsupported by this protocol",
            Self::Replay => "VDI clipboard message was already delivered",
            Self::InvalidReceipt => "VDI clipboard receipt is invalid",
            Self::InvalidEnvelope(_) => "VDI clipboard envelope is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for VdiClipboardTransportV2Error {}

/// Build one per-session VDI clipboard topic without allowing path-shaped
/// identities to escape the fixed namespace.
pub fn vdi_clipboard_session_topic(
    prefix: &str,
    session_id: &str,
) -> Result<String, VdiClipboardTransportV2Error> {
    if !matches!(
        prefix,
        VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX
            | VDI_CLIPBOARD_GUEST_TO_HOST_TOPIC_PREFIX
            | VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX
            | VDI_CLIPBOARD_LEASE_TOPIC_PREFIX
    ) {
        return Err(VdiClipboardTransportV2Error::InvalidIdentity);
    }
    validate_clipboard_identity("session_id", session_id)
        .map_err(|_| VdiClipboardTransportV2Error::InvalidIdentity)?;
    Ok(format!("{prefix}/{session_id}"))
}

fn secret_bearing_mime(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime.contains("password") || mime.contains("secret") || mime.contains("credential")
}

/// A validated, bounded UTF-8 text value for the VDI clipboard lane.
///
/// Construct this value at a protocol or platform boundary so every consumer
/// can rely on the same byte bound. Empty text is valid and represents a
/// clipboard clear; callers that require a non-empty clip should apply that
/// policy separately.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VdiClipboardText(String);

impl VdiClipboardText {
    /// Validate and wrap an owned UTF-8 string.
    ///
    /// # Errors
    ///
    /// Returns [`VdiClipboardTextValidationError::TooLarge`] when the encoded
    /// value exceeds [`MAX_VDI_CLIPBOARD_TEXT_BYTES`].
    pub fn new(text: impl Into<String>) -> Result<Self, VdiClipboardTextValidationError> {
        let text = text.into();
        if text.len() > MAX_VDI_CLIPBOARD_TEXT_BYTES {
            return Err(VdiClipboardTextValidationError::TooLarge {
                bytes: text.len(),
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            });
        }
        Ok(Self(text))
    }

    /// Validate and decode raw clipboard bytes as UTF-8.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, VdiClipboardTextValidationError> {
        let bytes = bytes.into();
        if bytes.len() > MAX_VDI_CLIPBOARD_TEXT_BYTES {
            return Err(VdiClipboardTextValidationError::TooLarge {
                bytes: bytes.len(),
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            });
        }
        String::from_utf8(bytes)
            .map(Self)
            .map_err(|_| VdiClipboardTextValidationError::InvalidUtf8)
    }

    /// Borrow the validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the deterministic encoded UTF-8 size in bytes.
    #[must_use]
    pub fn len_bytes(&self) -> usize {
        self.0.len()
    }

    /// Whether this value represents a clipboard clear.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<String> for VdiClipboardText {
    type Error = VdiClipboardTextValidationError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::new(text)
    }
}

impl From<VdiClipboardText> for String {
    fn from(text: VdiClipboardText) -> Self {
        text.0
    }
}

/// A bounded, target-seat clipboard handoff produced only after the daemon has
/// verified the signed VDI action. It is a transient local delivery record, not
/// a second clipboard history or authorization envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardMaterialization {
    /// The exact enrolled seat/hostname that may consume this handoff.
    pub target_seat: String,
    /// The bounded UTF-8 text; empty text is an explicit clear.
    pub text: VdiClipboardText,
    /// The already-authorized producer/session attribution.
    pub source: String,
    /// RFC3339 issuance time used for stale-handoff rejection.
    pub time: String,
}

impl ClipboardMaterialization {
    /// Build a node-local target-seat handoff from validated text.
    #[must_use]
    pub fn new(
        target_seat: impl Into<String>,
        text: VdiClipboardText,
        source: impl Into<String>,
        time: impl Into<String>,
    ) -> Self {
        Self {
            target_seat: target_seat.into(),
            text,
            source: source.into(),
            time: time.into(),
        }
    }

    /// Validate routing and attribution fields at the local handoff boundary.
    pub fn validate(&self) -> Result<(), String> {
        let target = self.target_seat.trim();
        if target.is_empty()
            || target.len() > 128
            || target.bytes().any(|byte| {
                !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'_' | b'-' | b':' | b'@')
            })
        {
            return Err("clipboard materialization target_seat is unsafe or empty".to_owned());
        }
        if self.source.trim().is_empty() {
            return Err("clipboard materialization source is empty".to_owned());
        }
        if self.time.trim().is_empty() {
            return Err("clipboard materialization time is empty".to_owned());
        }
        Ok(())
    }
}

/// Why a VDI clipboard text value was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VdiClipboardTextValidationError {
    /// The encoded value exceeds the canonical byte ceiling.
    TooLarge {
        /// The rejected encoded byte length.
        bytes: usize,
        /// The canonical maximum encoded byte length.
        max_bytes: usize,
    },
    /// Raw clipboard bytes were not valid UTF-8.
    InvalidUtf8,
}

impl std::fmt::Display for VdiClipboardTextValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes, max_bytes } => {
                write!(
                    formatter,
                    "clipboard text is {bytes} bytes; maximum is {max_bytes}"
                )
            }
            Self::InvalidUtf8 => formatter.write_str("clipboard text is not valid UTF-8"),
        }
    }
}

impl std::error::Error for VdiClipboardTextValidationError {}

/// RDP's real text clipboard channel is CLIPRDR. The current backend has not wired
/// that virtual channel, so both directions must report unsupported explicitly.
pub const RDP_CLIPBOARD_UNSUPPORTED_REASON: &str =
    "RDP CLIPRDR clipboard channel is not implemented in mde-vdi-rdp";

/// SPICE text clipboard rides the vdagent/main-channel clipboard messages. The
/// current backend has not wired that path, so both directions must report
/// unsupported explicitly.
pub const SPICE_CLIPBOARD_UNSUPPORTED_REASON: &str =
    "SPICE vdagent clipboard channel is not implemented in mde-vdi-spice";

/// The protocol-native channel backing a supported VDI clipboard lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VdiClipboardChannel {
    /// RDP CLIPRDR virtual channel.
    RdpCliprdr,
    /// SPICE vdagent clipboard messages.
    SpiceVdagent,
}

/// One directional clipboard lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VdiClipboardLaneStatus {
    /// The lane is backed by a real protocol clipboard channel.
    Supported {
        /// The protocol channel used for this direction.
        channel: VdiClipboardChannel,
    },
    /// The lane is not available and the reason is operator-visible.
    Unsupported {
        /// Human-readable reason. This must name the missing protocol path.
        reason: String,
    },
}

impl VdiClipboardLaneStatus {
    /// A directional unsupported status.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    /// Whether this lane has a real protocol channel behind it.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        matches!(self, Self::Supported { .. })
    }
}

/// Bidirectional text clipboard capability for a VDI endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VdiClipboardStatus {
    /// Host/mesh clipboard materialization into the guest.
    pub host_to_guest: VdiClipboardLaneStatus,
    /// Guest clipboard publication back to the host/mesh lane.
    pub guest_to_host: VdiClipboardLaneStatus,
}

impl VdiClipboardStatus {
    /// A bidirectional unsupported report using the same explicit reason for both
    /// lanes.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            host_to_guest: VdiClipboardLaneStatus::unsupported(reason.clone()),
            guest_to_host: VdiClipboardLaneStatus::unsupported(reason),
        }
    }

    /// Current RDP status: display/input are live, but CLIPRDR clipboard is absent.
    #[must_use]
    pub fn rdp_unsupported() -> Self {
        Self::unsupported(RDP_CLIPBOARD_UNSUPPORTED_REASON)
    }

    /// Current SPICE status: display/input are live, but vdagent clipboard is absent.
    #[must_use]
    pub fn spice_unsupported() -> Self {
        Self::unsupported(SPICE_CLIPBOARD_UNSUPPORTED_REASON)
    }

    /// Whether both directions are backed by real protocol clipboard channels.
    #[must_use]
    pub fn is_bidirectional(&self) -> bool {
        self.host_to_guest.is_supported() && self.guest_to_host.is_supported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport_lease(now_ms: u64) -> VdiClipboardLeaseV2 {
        VdiClipboardLeaseV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "vnc:oak:session-1".into(),
            generation: 7,
            lease_id: "clip-lease-7".into(),
            issued_at_ms: now_ms,
            expires_at_ms: now_ms + 60_000,
            permitted_mime_offers: vec!["text/plain;charset=utf-8".into()],
        }
    }

    fn transport_message(now_ms: u64, sequence: u64) -> VdiClipboardMessageV2 {
        let lease = transport_lease(now_ms);
        let envelope = ClipboardEnvelopeV2::new_inline_text(
            "node-a",
            "seat-a",
            "clipboard-session-a",
            sequence,
            now_ms,
            vec![
                "text/html;charset=utf-8".into(),
                "text/plain;charset=utf-8".into(),
            ],
            "hello",
            VdiClipboardText::new("hello").expect("bounded text"),
            now_ms + 30_000,
        )
        .expect("valid rich fallback envelope");
        VdiClipboardMessageV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: lease.session_id,
            generation: lease.generation,
            lease_id: lease.lease_id,
            lease_expires_at_ms: lease.expires_at_ms,
            message_sequence: sequence,
            selected_mime: "text/plain;charset=utf-8".into(),
            disclosure: VdiClipboardDisclosureV2::Shareable,
            envelope,
        }
    }

    fn image_transport(now_ms: u64) -> (VdiClipboardLeaseV2, VdiClipboardMessageV2) {
        let lease = VdiClipboardLeaseV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "rdp:oak:image-session".into(),
            generation: 9,
            lease_id: "rdp-image-lease-9".into(),
            issued_at_ms: now_ms,
            expires_at_ms: now_ms + 60_000,
            permitted_mime_offers: vec!["image/png".into(), "image/jpeg".into()],
        };
        let bytes = b"bounded png fixture";
        let envelope = ClipboardEnvelopeV2::new_files(
            "node-a",
            "seat-a",
            "clipboard-session-a",
            3,
            now_ms,
            vec!["image/png".into()],
            "image",
            ClipboardEnvelopeV2::content_hash_for(bytes),
            bytes.len() as u64,
            "files:v2:76d9deaf-80d3-4ca7-bfd3-995180ae8362",
            now_ms + 30_000,
        )
        .expect("bounded image envelope");
        let message = VdiClipboardMessageV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: lease.session_id.clone(),
            generation: lease.generation,
            lease_id: lease.lease_id.clone(),
            lease_expires_at_ms: lease.expires_at_ms,
            message_sequence: 1,
            selected_mime: "image/png".into(),
            disclosure: VdiClipboardDisclosureV2::Shareable,
            envelope,
        };
        (lease, message)
    }

    #[test]
    fn files_materialization_request_is_exact_bounded_and_lease_bound() {
        let now_ms = 1_700_000_000_000;
        let (lease, message) = image_transport(now_ms);
        let request = VdiClipboardFilesMaterializationRequestV1::from_message(
            &message,
            "31b69cf1-420f-4d10-94c7-61f671b4f313",
        )
        .expect("strict image request");
        request
            .validate_against(&message, &lease, now_ms + 1)
            .expect("exact current request");

        let mut changed = request.clone();
        changed.message_sequence += 1;
        assert_eq!(
            changed.validate_against(&message, &lease, now_ms + 1),
            Err(VdiClipboardFilesMaterializationErrorV1::MetadataMismatch)
        );
        let mut oversized = request;
        oversized.byte_count = MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES + 1;
        assert_eq!(
            oversized.validate(),
            Err(VdiClipboardFilesMaterializationErrorV1::Oversized)
        );
    }

    #[test]
    fn vdi_transport_admits_only_exact_negotiated_lease_identity() {
        let now_ms = 1_700_000_000_000;
        let lease = transport_lease(now_ms);
        let message = transport_message(now_ms, 1);
        message
            .admit(&lease, None, now_ms + 1)
            .expect("exact current negotiated message");

        let mut wrong_generation = message.clone();
        wrong_generation.generation += 1;
        assert_eq!(
            wrong_generation.admit(&lease, None, now_ms + 1),
            Err(VdiClipboardTransportV2Error::LeaseIdentityMismatch)
        );
        let mut wrong_lease = message;
        wrong_lease.lease_id = "other-lease".into();
        assert_eq!(
            wrong_lease.admit(&lease, None, now_ms + 1),
            Err(VdiClipboardTransportV2Error::LeaseIdentityMismatch)
        );
    }

    #[test]
    fn vdi_transport_receipt_suppresses_reconnect_replay_without_payload() {
        let now_ms = 1_700_000_000_000;
        let lease = transport_lease(now_ms);
        let message = transport_message(now_ms, 3);
        let receipt = message.receipt();
        receipt.validate().expect("payload-free receipt");
        let body = serde_json::to_string(&receipt).expect("receipt JSON");
        assert!(!body.contains("hello"));
        assert!(!body.contains("inline_text"));
        assert_eq!(
            message.admit(&lease, Some(&receipt), now_ms + 1),
            Err(VdiClipboardTransportV2Error::Replay)
        );

        let mut rewrapped = message.clone();
        rewrapped.message_sequence += 1;
        assert_eq!(
            rewrapped.admit(&lease, Some(&receipt), now_ms + 1),
            Err(VdiClipboardTransportV2Error::Replay),
            "a newer wrapper cannot replay the same Clipboard V2 source sequence"
        );

        let newer = transport_message(now_ms, 4);
        newer
            .admit(&lease, Some(&receipt), now_ms + 1)
            .expect("strictly newer delivery");
    }

    #[test]
    fn vdi_transport_fails_closed_for_expiry_secret_unsupported_and_oversized() {
        let now_ms = 1_700_000_000_000;
        let lease = transport_lease(now_ms);
        let message = transport_message(now_ms, 1);
        assert_eq!(
            message.admit(&lease, None, lease.expires_at_ms),
            Err(VdiClipboardTransportV2Error::ExpiredLease)
        );

        let mut secret = message.clone();
        secret.disclosure = VdiClipboardDisclosureV2::Secret;
        assert_eq!(
            secret.admit(&lease, None, now_ms + 1),
            Err(VdiClipboardTransportV2Error::SecretBearing)
        );

        let mut unsupported = message;
        unsupported.selected_mime = "text/html;charset=utf-8".into();
        assert_eq!(
            unsupported.admit(&lease, None, now_ms + 1),
            Err(VdiClipboardTransportV2Error::UnsupportedMime)
        );

        let oversized = vec![b' '; MAX_VDI_CLIPBOARD_TRANSPORT_V2_JSON_BYTES + 1];
        assert_eq!(
            VdiClipboardMessageV2::from_json_bytes(&oversized),
            Err(VdiClipboardTransportV2Error::BodyTooLarge)
        );
    }

    #[test]
    fn vdi_transport_rejects_future_dated_envelopes_before_replay_admission() {
        let now_ms = 1_700_000_000_000;
        let lease = transport_lease(now_ms);
        let mut message = transport_message(now_ms, 1);
        message.envelope.timestamp_ms = now_ms + 1;
        message.envelope.expires_at_ms = now_ms + 30_001;

        assert_eq!(
            message.admit(&lease, None, now_ms),
            Err(VdiClipboardTransportV2Error::InvalidEnvelope(
                ClipboardEnvelopeV2ValidationError::FutureTimestamp {
                    now_ms,
                    timestamp_ms: now_ms + 1,
                }
            ))
        );
    }

    #[test]
    fn vdi_transport_json_and_topics_reject_hostile_paths_and_duplicate_keys() {
        let now_ms = 1_700_000_000_000;
        let message = transport_message(now_ms, 1);
        let body = serde_json::to_string(&message).expect("message JSON");
        let decoded =
            VdiClipboardMessageV2::from_json_bytes(body.as_bytes()).expect("strict transport body");
        assert_eq!(decoded, message);

        let duplicate = body.replacen(
            "\"schema_version\":2",
            "\"schema_version\":2,\"schema_version\":2",
            1,
        );
        assert_eq!(
            VdiClipboardMessageV2::from_json_bytes(duplicate.as_bytes()),
            Err(VdiClipboardTransportV2Error::MalformedBody)
        );
        assert!(vdi_clipboard_session_topic(
            VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
            "../../secret"
        )
        .is_err());
        assert_eq!(
            vdi_clipboard_session_topic(
                VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
                "vnc:oak:session-1"
            )
            .expect("safe topic"),
            "state/clipboard/vdi-v2/host-to-guest/vnc:oak:session-1"
        );
    }

    #[test]
    fn text_value_uses_encoded_bytes_for_the_limit() {
        let prefix = "a".repeat(MAX_VDI_CLIPBOARD_TEXT_BYTES - 2);
        let text = VdiClipboardText::new(format!("{prefix}é")).expect("UTF-8 boundary is safe");

        assert_eq!(text.as_str(), format!("{prefix}é"));
        assert_eq!(text.len_bytes(), MAX_VDI_CLIPBOARD_TEXT_BYTES);
        assert!(text.as_str().is_char_boundary(text.len_bytes()));

        let exact = VdiClipboardText::new("é".repeat(MAX_VDI_CLIPBOARD_TEXT_BYTES / 2))
            .expect("exact encoded byte limit");
        assert_eq!(exact.len_bytes(), MAX_VDI_CLIPBOARD_TEXT_BYTES);
    }

    #[test]
    fn text_value_rejects_oversized_strings_without_truncation() {
        let value = "x".repeat(MAX_VDI_CLIPBOARD_TEXT_BYTES + 1);
        assert_eq!(
            VdiClipboardText::new(value),
            Err(VdiClipboardTextValidationError::TooLarge {
                bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES + 1,
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            })
        );
    }

    #[test]
    fn raw_bytes_require_valid_utf8_before_materialization() {
        assert_eq!(
            VdiClipboardText::from_bytes(vec![b'v', 0xff]),
            Err(VdiClipboardTextValidationError::InvalidUtf8)
        );
    }

    #[test]
    fn serde_round_trip_preserves_empty_and_rejects_oversized_values() {
        let empty = VdiClipboardText::new("").expect("empty clears are valid");
        let body = serde_json::to_string(&empty).expect("serialize text");
        assert_eq!(body, "\"\"");
        assert_eq!(
            serde_json::from_str::<VdiClipboardText>(&body).expect("deserialize text"),
            empty
        );

        let oversized = format!("\"{}\"", "x".repeat(MAX_VDI_CLIPBOARD_TEXT_BYTES + 1));
        assert!(serde_json::from_str::<VdiClipboardText>(&oversized).is_err());
    }

    #[test]
    fn rdp_unsupported_names_cliprdr_in_both_directions() {
        let status = VdiClipboardStatus::rdp_unsupported();
        assert!(!status.is_bidirectional());
        for lane in [&status.host_to_guest, &status.guest_to_host] {
            match lane {
                VdiClipboardLaneStatus::Unsupported { reason } => {
                    assert!(reason.contains("CLIPRDR"));
                    assert!(reason.contains("mde-vdi-rdp"));
                }
                other => panic!("expected unsupported RDP lane, got {other:?}"),
            }
        }
    }

    #[test]
    fn spice_unsupported_names_vdagent_in_both_directions() {
        let status = VdiClipboardStatus::spice_unsupported();
        assert!(!status.is_bidirectional());
        for lane in [&status.host_to_guest, &status.guest_to_host] {
            match lane {
                VdiClipboardLaneStatus::Unsupported { reason } => {
                    assert!(reason.contains("vdagent"));
                    assert!(reason.contains("mde-vdi-spice"));
                }
                other => panic!("expected unsupported SPICE lane, got {other:?}"),
            }
        }
    }

    #[test]
    fn wire_shape_is_stable_and_explicit() {
        let body = serde_json::to_string(&VdiClipboardStatus::rdp_unsupported())
            .expect("serialize status");
        assert!(body.contains(r#""host_to_guest":{"state":"unsupported""#));
        assert!(body.contains(r#""guest_to_host":{"state":"unsupported""#));
        assert!(body.contains("CLIPRDR"));

        let back: VdiClipboardStatus = serde_json::from_str(&body).expect("round-trip");
        assert_eq!(back, VdiClipboardStatus::rdp_unsupported());
    }

    fn inline_envelope(sequence: u64) -> ClipboardEnvelopeV2 {
        ClipboardEnvelopeV2::new_inline_text(
            "node-a",
            "seat-a",
            "session-a",
            sequence,
            1_700_000_000_000,
            vec!["text/html".into(), "text/plain".into()],
            "hello",
            VdiClipboardText::new("hello").expect("bounded fixture text"),
            1_700_000_060_000,
        )
        .expect("valid inline envelope fixture")
    }

    #[test]
    fn v2_inline_envelope_hashes_payload_and_preserves_offer_order() {
        let envelope = inline_envelope(1);

        assert_eq!(
            envelope.mime_offers,
            vec!["text/html".to_owned(), "text/plain".to_owned()]
        );
        assert_eq!(envelope.byte_count, 5);
        assert_eq!(
            envelope.content_hash,
            ClipboardEnvelopeV2::content_hash_for(b"hello")
        );
        assert!(envelope.validate().is_ok());

        let encoded = serde_json::to_string(&envelope).expect("encode V2 envelope");
        let decoded = ClipboardEnvelopeV2::from_json(&encoded).expect("decode V2 envelope");
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn v2_files_envelope_carries_only_an_opaque_reference() {
        let envelope = ClipboardEnvelopeV2::new_files(
            "node-a",
            "seat-a",
            "session-a",
            2,
            1_700_000_000_000,
            vec!["image/png".into(), "application/octet-stream".into()],
            "image",
            ClipboardEnvelopeV2::content_hash_for(b"png bytes"),
            9,
            "files:v2:payload-1",
            1_700_000_060_000,
        )
        .expect("valid Files envelope fixture");

        assert_eq!(envelope.inline_text, None);
        assert_eq!(
            envelope.files_reference.as_deref(),
            Some("files:v2:payload-1")
        );
        assert!(envelope.validate_at(1_700_000_000_001).is_ok());
    }

    #[test]
    fn v2_serde_rejects_unknown_fields_and_unsupported_versions() {
        let envelope = inline_envelope(1);
        let mut object = serde_json::to_value(&envelope)
            .expect("encode envelope value")
            .as_object()
            .cloned()
            .expect("envelope object");
        object.insert("future_field".into(), serde_json::json!(true));
        let unknown = serde_json::to_string(&object).expect("encode unknown field");
        assert!(serde_json::from_str::<ClipboardEnvelopeV2>(&unknown).is_err());

        let mut wrong_version = serde_json::to_value(&envelope)
            .expect("encode envelope value")
            .as_object()
            .cloned()
            .expect("envelope object");
        wrong_version.insert("schema_version".into(), serde_json::json!(1));
        let wrong_version = serde_json::to_string(&wrong_version).expect("encode old version");
        assert!(serde_json::from_str::<ClipboardEnvelopeV2>(&wrong_version).is_err());
        assert!(matches!(
            ClipboardEnvelopeV2::from_json(&wrong_version),
            Err(ClipboardEnvelopeV2DecodeError::Validation(
                ClipboardEnvelopeV2ValidationError::UnsupportedSchema { found: 1 }
            ))
        ));
    }

    #[test]
    fn v2_json_body_cap_applies_before_parsing() {
        let oversized = vec![b'x'; MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES + 1];
        assert!(matches!(
            ClipboardEnvelopeV2::from_json_bytes(&oversized),
            Err(ClipboardEnvelopeV2DecodeError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn v2_admission_rejects_replay_and_cross_source_identity() {
        let previous = inline_envelope(4);
        let replay = inline_envelope(4);
        assert!(matches!(
            replay.validate_replay_after(Some(&previous)),
            Err(ClipboardEnvelopeV2ValidationError::Replay {
                previous: 4,
                received: 4
            })
        ));

        let mut cross_source = inline_envelope(5);
        cross_source.source_seat = "seat-b".into();
        assert!(matches!(
            cross_source.validate_replay_after(Some(&previous)),
            Err(ClipboardEnvelopeV2ValidationError::IdentityMismatch {
                field: "source_seat"
            })
        ));
        assert!(matches!(
            previous.admit("node-a", "seat-b", "session-a", None, 1_700_000_000_001),
            Err(ClipboardEnvelopeV2ValidationError::IdentityMismatch {
                field: "source_seat"
            })
        ));
    }

    #[test]
    fn v2_admission_rejects_expiry_and_invalid_payload_relationships() {
        let envelope = inline_envelope(1);
        assert!(matches!(
            envelope.validate_at(envelope.expires_at_ms),
            Err(ClipboardEnvelopeV2ValidationError::Expired { .. })
        ));

        let mut count_mismatch = inline_envelope(1);
        count_mismatch.byte_count = 6;
        assert!(matches!(
            count_mismatch.validate(),
            Err(
                ClipboardEnvelopeV2ValidationError::InlineByteCountMismatch {
                    declared: 6,
                    actual: 5
                }
            )
        ));

        let mut hash_mismatch = inline_envelope(1);
        hash_mismatch.content_hash = ClipboardEnvelopeV2::content_hash_for(b"other");
        assert_eq!(
            hash_mismatch.validate(),
            Err(ClipboardEnvelopeV2ValidationError::InlineContentHashMismatch)
        );

        let mut both_payloads = inline_envelope(1);
        both_payloads.files_reference = Some("files:v2:payload-1".into());
        assert_eq!(
            both_payloads.validate(),
            Err(ClipboardEnvelopeV2ValidationError::MultiplePayloads)
        );

        let mut no_payload = inline_envelope(1);
        no_payload.inline_text = None;
        assert_eq!(
            no_payload.validate(),
            Err(ClipboardEnvelopeV2ValidationError::MissingPayload)
        );
    }

    #[test]
    fn v2_admission_rejects_unsafe_or_oversized_fields() {
        let mut bad_identity = inline_envelope(1);
        bad_identity.source_node = "node with spaces".into();
        assert!(matches!(
            bad_identity.validate(),
            Err(ClipboardEnvelopeV2ValidationError::InvalidIdentity {
                field: "source_node"
            })
        ));

        let mut bad_preview = inline_envelope(1);
        bad_preview.preview = "x".repeat(MAX_CLIPBOARD_ENVELOPE_V2_PREVIEW_BYTES + 1);
        assert_eq!(
            bad_preview.validate(),
            Err(ClipboardEnvelopeV2ValidationError::InvalidPreview)
        );

        let mut duplicate_mime = inline_envelope(1);
        duplicate_mime.mime_offers = vec!["text/plain".into(), "TEXT/PLAIN".into()];
        assert_eq!(
            duplicate_mime.validate(),
            Err(ClipboardEnvelopeV2ValidationError::DuplicateMimeOffer { index: 1 })
        );

        let bad_reference = ClipboardEnvelopeV2::new_files(
            "node-a",
            "seat-a",
            "session-a",
            1,
            1_700_000_000_000,
            vec!["application/octet-stream".into()],
            "file",
            ClipboardEnvelopeV2::content_hash_for(b"bytes"),
            5,
            "../outside-files",
            1_700_000_060_000,
        );
        assert_eq!(
            bad_reference,
            Err(ClipboardEnvelopeV2ValidationError::InvalidFilesReference)
        );

        let mut too_large = inline_envelope(1);
        too_large.byte_count = MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES + 1;
        assert!(matches!(
            too_large.validate(),
            Err(ClipboardEnvelopeV2ValidationError::ContentTooLarge { .. })
        ));
    }

    fn session_consent(enabled: bool) -> ClipboardSessionConsentV1 {
        ClipboardSessionConsentV1::new(
            "node-a",
            "seat-a",
            "session-a",
            enabled,
            1_700_000_000_000,
            1_700_000_060_000,
        )
        .expect("valid session consent fixture")
    }

    #[test]
    fn session_consent_round_trip_is_bounded_and_payload_free() {
        let consent = session_consent(false);
        assert!(!consent.is_enabled());

        let body = serde_json::to_string(&consent).expect("encode session consent");
        assert!(body.contains(r#""enabled":false"#));
        assert!(!body.contains("mime_offers"));
        assert!(!body.contains("files_reference"));
        assert!(!body.contains("command"));
        assert_eq!(
            ClipboardSessionConsentV1::from_json(&body).expect("decode consent"),
            consent
        );
    }

    #[test]
    fn session_consent_admission_binds_identity_freshness_and_state() {
        let enabled = session_consent(true);
        assert_eq!(
            enabled
                .allows_clipboard_at("node-a", "seat-a", "session-a", None, 1_700_000_000_001,)
                .expect("fresh matching consent"),
            true
        );

        let disabled = enabled
            .update(false, 1_700_000_000_002, 1_700_000_060_002)
            .expect("strictly newer disable update");
        assert_eq!(
            disabled
                .allows_clipboard_at(
                    "node-a",
                    "seat-a",
                    "session-a",
                    Some(&enabled),
                    1_700_000_000_003,
                )
                .expect("fresh matching disable consent"),
            false
        );
        assert!(matches!(
            disabled.allows_clipboard_at(
                "node-a",
                "seat-b",
                "session-a",
                Some(&enabled),
                1_700_000_000_003,
            ),
            Err(ClipboardSessionConsentValidationError::IdentityMismatch {
                field: "source_seat"
            })
        ));
        assert!(matches!(
            enabled.update(true, enabled.updated_at_ms, enabled.expires_at_ms),
            Err(ClipboardSessionConsentValidationError::StaleUpdate { .. })
        ));
    }

    #[test]
    fn session_consent_rejects_stale_future_and_malformed_state() {
        let consent = session_consent(true);
        assert!(matches!(
            consent.validate_at(consent.expires_at_ms),
            Err(ClipboardSessionConsentValidationError::Expired { .. })
        ));
        assert!(matches!(
            consent.validate_at(consent.updated_at_ms - 1),
            Err(ClipboardSessionConsentValidationError::FutureTimestamp {
                field: "updated_at_ms",
                ..
            })
        ));

        let mut unsafe_identity = consent.clone();
        unsafe_identity.source_session = "session/with/path".into();
        assert_eq!(
            unsafe_identity.validate(),
            Err(ClipboardSessionConsentValidationError::InvalidIdentity {
                field: "source_session"
            })
        );

        let mut zero_issue = consent.clone();
        zero_issue.issued_at_ms = 0;
        assert_eq!(
            zero_issue.validate(),
            Err(ClipboardSessionConsentValidationError::InvalidTimestamp {
                field: "issued_at_ms"
            })
        );

        let mut reversed_update = consent.clone();
        reversed_update.updated_at_ms = reversed_update.issued_at_ms - 1;
        assert_eq!(
            reversed_update.validate(),
            Err(ClipboardSessionConsentValidationError::InvalidTimestamp {
                field: "updated_at_ms"
            })
        );

        let mut oversized_ttl = consent;
        oversized_ttl.expires_at_ms = oversized_ttl
            .updated_at_ms
            .saturating_add(MAX_CLIPBOARD_SESSION_CONSENT_TTL_MS + 1);
        assert_eq!(
            oversized_ttl.validate(),
            Err(ClipboardSessionConsentValidationError::InvalidExpiry)
        );
    }

    #[test]
    fn session_consent_serde_rejects_unknown_and_unsupported_versions() {
        let consent = session_consent(true);
        let mut object = serde_json::to_value(&consent)
            .expect("encode consent value")
            .as_object()
            .cloned()
            .expect("consent object");
        object.insert("mime_offers".into(), serde_json::json!(["text/plain"]));
        let unknown = serde_json::to_string(&object).expect("encode unknown field");
        assert!(serde_json::from_str::<ClipboardSessionConsentV1>(&unknown).is_err());

        let mut wrong_version = serde_json::to_value(&consent)
            .expect("encode consent value")
            .as_object()
            .cloned()
            .expect("consent object");
        wrong_version.insert("schema_version".into(), serde_json::json!(2));
        let wrong_version = serde_json::to_string(&wrong_version).expect("encode old version");
        assert!(matches!(
            ClipboardSessionConsentV1::from_json(&wrong_version),
            Err(ClipboardSessionConsentDecodeError::Validation(
                ClipboardSessionConsentValidationError::UnsupportedSchema { found: 2 }
            ))
        ));
    }

    #[test]
    fn session_consent_json_body_cap_applies_before_parsing() {
        let oversized = vec![b'x'; MAX_CLIPBOARD_SESSION_CONSENT_JSON_BYTES + 1];
        assert!(matches!(
            ClipboardSessionConsentV1::from_json_bytes(&oversized),
            Err(ClipboardSessionConsentDecodeError::BodyTooLarge { .. })
        ));
    }
}

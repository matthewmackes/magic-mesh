//! Strict V2 rich-clipboard transport contracts (WL-FUNC-016/WL-FUNC-011).
//!
//! The envelope carries only bounded metadata and inline UTF-8 text. Images,
//! file lists, and other large representations are identified by an opaque
//! [`FileRefId`]; their bytes stay in the Files plane and never enter the Bus
//! envelope. There are no path, URL, command, credential, or arbitrary MIME
//! string fields. Text content is opaque user data, while all routing and
//! diagnostic metadata is validated as a safe bounded value.
//!
//! Admission is deliberately split into two checks. [`ClipboardEnvelopeV2::validate`]
//! checks the intrinsic signed contract. [`ClipboardEnvelopeV2::validate_at`]
//! additionally checks expiry and a caller-provided last sequence, which is the
//! replay boundary owned by the receiving session ledger.

use std::fmt;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{de, Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::ids::FileRefId;
use crate::value::sha256_hex;

/// The only rich-clipboard schema currently admitted by this crate.
pub const CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION: u16 = 2;
/// Maximum encoded JSON body accepted by [`ClipboardEnvelopeV2::from_json_bytes`].
pub const MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES: usize = 256 * 1024;
/// Maximum bytes in a node, seat, or other safe identity token.
pub const MAX_CLIPBOARD_ID_BYTES: usize = 128;
/// Maximum bytes in a non-content preview or diagnostic display value.
pub const MAX_CLIPBOARD_PREVIEW_BYTES: usize = 512;
/// Maximum number of ordered MIME representations in one envelope.
pub const MAX_CLIPBOARD_OFFERS: usize = 8;
/// Maximum inline UTF-8 representation size. Larger values use Files.
pub const MAX_CLIPBOARD_INLINE_TEXT_BYTES: usize = 1024 * 1024;
/// Maximum byte count represented by one Files-backed clipboard offer.
pub const MAX_CLIPBOARD_PAYLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum lifetime of a clipboard envelope after its creation timestamp.
pub const MAX_CLIPBOARD_TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// Maximum number of node identities retained by the echo guard.
pub const MAX_CLIPBOARD_ECHO_HOPS: usize = 8;

/// A bounded identity validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardIdentityValidationError {
    /// The identity was empty.
    Empty,
    /// The identity exceeded [`MAX_CLIPBOARD_ID_BYTES`].
    TooLong,
    /// The identity contained a path, URL, command delimiter, or other unsafe
    /// character. Only ASCII letters, digits, `-`, `_`, and `.` are allowed.
    InvalidCharacters,
}

impl fmt::Display for ClipboardIdentityValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("clipboard identity is empty"),
            Self::TooLong => write!(
                formatter,
                "clipboard identity exceeds {MAX_CLIPBOARD_ID_BYTES} bytes"
            ),
            Self::InvalidCharacters => {
                formatter.write_str("clipboard identity contains unsafe characters")
            }
        }
    }
}

impl std::error::Error for ClipboardIdentityValidationError {}

macro_rules! bounded_identity {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validate and construct a safe opaque identity token.
            pub fn new(
                value: impl Into<String>,
            ) -> Result<Self, ClipboardIdentityValidationError> {
                let value = value.into();
                validate_identity(&value)?;
                Ok(Self(value))
            }

            /// Borrow the validated identity token.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

bounded_identity! {
    /// The enrolled mesh node that owns a seat or signs an envelope.
    ClipboardNodeId
}

bounded_identity! {
    /// A physical/logical DRM seat identity within a node.
    ClipboardSeatId
}

/// A typed per-login clipboard session identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipboardSessionId(Uuid);

impl ClipboardSessionId {
    /// Mint a fresh session identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wrap an existing UUID, useful for deterministic replay tests.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// The nil sentinel, which is never admitted in an envelope.
    #[must_use]
    pub const fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// Whether this is the nil sentinel.
    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0.is_nil()
    }
}

impl Default for ClipboardSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ClipboardSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A stable content/event identity used for deduplication and echo prevention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipboardClipId(Uuid);

impl ClipboardClipId {
    /// Mint a fresh clipboard identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wrap an existing UUID, useful for deterministic tests.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// The nil sentinel, which is never admitted in an envelope.
    #[must_use]
    pub const fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// Whether this is the nil sentinel.
    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0.is_nil()
    }
}

impl Default for ClipboardClipId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ClipboardClipId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Typed source identity bound into the envelope signature.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardSourceV2 {
    /// Enrolled node that captured the representation.
    pub node: ClipboardNodeId,
    /// Seat on that node that captured the representation.
    pub seat: ClipboardSeatId,
}

impl ClipboardSourceV2 {
    /// Construct a source identity from validated node and seat tokens.
    #[must_use]
    pub const fn new(node: ClipboardNodeId, seat: ClipboardSeatId) -> Self {
        Self { node, seat }
    }
}

/// Typed destination identity bound into the envelope signature.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardTargetV2 {
    /// Enrolled node that is the intended recipient.
    pub node: ClipboardNodeId,
    /// Seat on that node that is the intended recipient.
    pub seat: ClipboardSeatId,
}

impl ClipboardTargetV2 {
    /// Construct a target identity from validated node and seat tokens.
    #[must_use]
    pub const fn new(node: ClipboardNodeId, seat: ClipboardSeatId) -> Self {
        Self { node, seat }
    }
}

/// The finite MIME vocabulary admitted by the rich clipboard lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardMimeKind {
    /// UTF-8 plain text.
    TextPlain,
    /// UTF-8 HTML source.
    TextHtml,
    /// UTF-8 RTF source.
    TextRtf,
    /// PNG image bytes, held by Files.
    ImagePng,
    /// JPEG image bytes, held by Files.
    ImageJpeg,
    /// A Files-backed list of objects.
    FileList,
}

impl ClipboardMimeKind {
    /// Whether this representation can be carried inline as UTF-8 text.
    #[must_use]
    pub const fn is_text(self) -> bool {
        matches!(self, Self::TextPlain | Self::TextHtml | Self::TextRtf)
    }
}

/// An explicit capability refusal for one representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardUnsupportedReason {
    /// The target does not advertise this MIME kind.
    TargetMimeUnsupported,
    /// The representation is not implemented by this transport.
    TransportUnsupported,
    /// Policy intentionally disables this representation.
    PolicyDisabled,
}

/// An explicit provider/availability failure for one representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardUnavailableReason {
    /// The Files reference cannot currently be resolved.
    FilesProviderUnavailable,
    /// The VDI or native clipboard provider is offline.
    ProviderOffline,
    /// The representation expired before materialization.
    Expired,
    /// Capability information is not fresh enough to use.
    CapabilityUnknown,
}

/// The only payload forms that may enter the bounded clipboard envelope.
///
/// In particular, this enum has no raw bytes, path, URL, command, or secret
/// variant. Text may contain arbitrary user-authored text; it is never treated
/// as an instruction or fetched as a resource by this contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum ClipboardPayloadV2 {
    /// A bounded UTF-8 representation.
    InlineText {
        /// The representation bytes, measured as UTF-8 bytes.
        text: String,
    },
    /// An opaque Files object reference; bytes remain outside the envelope.
    FilesReference {
        /// Existing Files identity, never a path or URL.
        file_ref: FileRefId,
    },
    /// The target explicitly cannot consume this MIME representation.
    Unsupported {
        /// Typed capability reason.
        reason: ClipboardUnsupportedReason,
    },
    /// The representation exists but is not presently materializable.
    Unavailable {
        /// Typed provider/availability reason.
        reason: ClipboardUnavailableReason,
    },
}

/// One ordered MIME representation in a [`ClipboardEnvelopeV2`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardMimeOfferV2 {
    /// Finite typed MIME kind; arbitrary MIME strings are not admitted.
    pub mime: ClipboardMimeKind,
    /// Total representation size in bytes, including Files-backed data.
    pub byte_count: u64,
    /// Lower-case SHA-256 of the representation bytes when available.
    #[serde(default)]
    pub content_sha256_hex: Option<String>,
    /// Bounded display-only preview; never a path, URL, command, or secret.
    #[serde(default)]
    pub preview: Option<String>,
    /// Inline text, an opaque Files reference, or an explicit state.
    pub payload: ClipboardPayloadV2,
}

impl ClipboardMimeOfferV2 {
    /// Construct and validate an inline text representation.
    pub fn inline_text(
        mime: ClipboardMimeKind,
        text: impl Into<String>,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        let text = text.into();
        let offer = Self {
            mime,
            byte_count: text.len() as u64,
            content_sha256_hex: Some(sha256_hex(text.as_bytes())),
            preview: None,
            payload: ClipboardPayloadV2::InlineText { text },
        };
        offer.validate()?;
        Ok(offer)
    }

    /// Construct and validate a Files-backed representation.
    pub fn files_reference(
        mime: ClipboardMimeKind,
        file_ref: FileRefId,
        byte_count: u64,
        content_sha256_hex: impl Into<String>,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        let offer = Self {
            mime,
            byte_count,
            content_sha256_hex: Some(content_sha256_hex.into()),
            preview: None,
            payload: ClipboardPayloadV2::FilesReference { file_ref },
        };
        offer.validate()?;
        Ok(offer)
    }

    /// Construct an explicit unsupported representation.
    #[must_use]
    pub const fn unsupported(mime: ClipboardMimeKind, reason: ClipboardUnsupportedReason) -> Self {
        Self {
            mime,
            byte_count: 0,
            content_sha256_hex: None,
            preview: None,
            payload: ClipboardPayloadV2::Unsupported { reason },
        }
    }

    /// Construct an explicit unavailable representation.
    #[must_use]
    pub const fn unavailable(mime: ClipboardMimeKind, reason: ClipboardUnavailableReason) -> Self {
        Self {
            mime,
            byte_count: 0,
            content_sha256_hex: None,
            preview: None,
            payload: ClipboardPayloadV2::Unavailable { reason },
        }
    }

    /// Validate representation bounds, digest metadata, and payload shape.
    pub fn validate(&self) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        if self.byte_count > MAX_CLIPBOARD_PAYLOAD_BYTES {
            return Err(ClipboardEnvelopeV2ValidationError::OutOfBounds {
                field: "offers.byte_count",
                max: MAX_CLIPBOARD_PAYLOAD_BYTES,
            });
        }
        if let Some(preview) = &self.preview {
            validate_metadata_text(preview, "offers.preview")?;
        }

        match &self.payload {
            ClipboardPayloadV2::InlineText { text } => {
                if !self.mime.is_text() {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidOffer {
                        field: "offers.payload.inline_text.mime",
                    });
                }
                if text.is_empty() {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidOffer {
                        field: "offers.payload.inline_text.text",
                    });
                }
                if text.len() > MAX_CLIPBOARD_INLINE_TEXT_BYTES {
                    return Err(ClipboardEnvelopeV2ValidationError::OutOfBounds {
                        field: "offers.payload.inline_text.text",
                        max: MAX_CLIPBOARD_INLINE_TEXT_BYTES as u64,
                    });
                }
                if self.byte_count != text.len() as u64 {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidOffer {
                        field: "offers.byte_count",
                    });
                }
                let digest = self.content_sha256_hex.as_deref().ok_or(
                    ClipboardEnvelopeV2ValidationError::InvalidDigest {
                        field: "offers.content_sha256_hex",
                    },
                )?;
                validate_sha256(digest, "offers.content_sha256_hex")?;
                if digest != sha256_hex(text.as_bytes()) {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidDigest {
                        field: "offers.content_sha256_hex",
                    });
                }
            }
            ClipboardPayloadV2::FilesReference { file_ref } => {
                if file_ref.is_nil() {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidOffer {
                        field: "offers.payload.files_reference.file_ref",
                    });
                }
                let digest = self.content_sha256_hex.as_deref().ok_or(
                    ClipboardEnvelopeV2ValidationError::InvalidDigest {
                        field: "offers.content_sha256_hex",
                    },
                )?;
                validate_sha256(digest, "offers.content_sha256_hex")?;
            }
            ClipboardPayloadV2::Unsupported { .. } | ClipboardPayloadV2::Unavailable { .. } => {
                if self.byte_count != 0 || self.content_sha256_hex.is_some() {
                    return Err(ClipboardEnvelopeV2ValidationError::InvalidOffer {
                        field: "offers.payload.state_metadata",
                    });
                }
            }
        }
        Ok(())
    }
}

/// The origin identity and visited-node set used to stop clipboard echoes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardEchoGuardV2 {
    /// Stable origin identity shared by relays of one clipboard update.
    pub origin: ClipboardClipId,
    /// Nodes that already handled the update, including the source node.
    pub visited_nodes: Vec<ClipboardNodeId>,
}

impl ClipboardEchoGuardV2 {
    /// Create the initial guard for a newly captured clipboard update.
    #[must_use]
    pub fn origin(clip_id: ClipboardClipId, source_node: ClipboardNodeId) -> Self {
        Self {
            origin: clip_id,
            visited_nodes: vec![source_node],
        }
    }
}

/// Signed source attribution. The roster/trust layer binds the public key to
/// `signer`; this contract proves that the key signed the complete envelope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardSignedAttributionV2 {
    /// Node identity making the attribution claim; must equal `source.node`.
    pub signer: ClipboardNodeId,
    /// Ed25519 verifying key, lower-case hex.
    pub pubkey_hex: String,
    /// Detached Ed25519 signature over the canonical envelope fields.
    pub signature_hex: String,
}

impl ClipboardSignedAttributionV2 {
    fn unsigned(signer: ClipboardNodeId) -> Self {
        Self {
            signer,
            pubkey_hex: String::new(),
            signature_hex: String::new(),
        }
    }
}

/// A versioned, signed, bounded rich-clipboard update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardEnvelopeV2 {
    /// Wire schema discriminator.
    pub schema_version: u16,
    /// Stable update identity used for deduplication and echo prevention.
    pub clip_id: ClipboardClipId,
    /// Typed source node and seat.
    pub source: ClipboardSourceV2,
    /// Typed destination node and seat.
    pub target: ClipboardTargetV2,
    /// Per-login session identity; publishing consent is scoped to it.
    pub session: ClipboardSessionId,
    /// Monotonic source/session sequence number, starting at one.
    pub sequence: u64,
    /// Caller-injected creation timestamp in Unix milliseconds.
    pub created_unix_ms: u64,
    /// Absolute expiry timestamp in Unix milliseconds.
    pub expires_unix_ms: u64,
    /// Ordered richest-first MIME representations.
    pub offers: Vec<ClipboardMimeOfferV2>,
    /// Origin/visited-node echo guard.
    pub echo_guard: ClipboardEchoGuardV2,
    /// Ed25519 attribution over every field except this signature value.
    pub attribution: ClipboardSignedAttributionV2,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipboardEnvelopeV2Wire {
    schema_version: u16,
    clip_id: ClipboardClipId,
    source: ClipboardSourceV2,
    target: ClipboardTargetV2,
    session: ClipboardSessionId,
    sequence: u64,
    created_unix_ms: u64,
    expires_unix_ms: u64,
    offers: Vec<ClipboardMimeOfferV2>,
    echo_guard: ClipboardEchoGuardV2,
    attribution: ClipboardSignedAttributionV2,
}

impl<'de> Deserialize<'de> for ClipboardEnvelopeV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ClipboardEnvelopeV2Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

impl ClipboardEnvelopeV2 {
    /// Construct an unsigned envelope. Call [`Self::signed`] before transport.
    pub fn new(
        clip_id: ClipboardClipId,
        source: ClipboardSourceV2,
        target: ClipboardTargetV2,
        session: ClipboardSessionId,
        sequence: u64,
        created_unix_ms: u64,
        expires_unix_ms: u64,
        offers: Vec<ClipboardMimeOfferV2>,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        let echo_guard = ClipboardEchoGuardV2::origin(clip_id, source.node.clone());
        let signer = source.node.clone();
        let envelope = Self {
            schema_version: CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION,
            clip_id,
            source,
            target,
            session,
            sequence,
            created_unix_ms,
            expires_unix_ms,
            offers,
            echo_guard,
            attribution: ClipboardSignedAttributionV2::unsigned(signer),
        };
        envelope.validate_unsigned()?;
        Ok(envelope)
    }

    /// Return the deterministic bytes that are signed by [`Self::sign`].
    ///
    /// The detached public key and signature are excluded; all routing,
    /// ordering, expiry, content metadata, and attribution identity fields are
    /// included in fixed order.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let offers_json = serde_json::to_string(&self.offers).unwrap_or_default();
        let echo_json = serde_json::to_string(&self.echo_guard).unwrap_or_default();
        let mut bytes = String::with_capacity(offers_json.len() + echo_json.len() + 320);
        bytes.push_str("mde-clipboard-envelope-v2\n");
        bytes.push_str(&self.schema_version.to_string());
        bytes.push('\n');
        bytes.push_str(&self.clip_id.to_string());
        bytes.push('\n');
        bytes.push_str(self.source.node.as_str());
        bytes.push('\n');
        bytes.push_str(self.source.seat.as_str());
        bytes.push('\n');
        bytes.push_str(self.target.node.as_str());
        bytes.push('\n');
        bytes.push_str(self.target.seat.as_str());
        bytes.push('\n');
        bytes.push_str(&self.session.to_string());
        bytes.push('\n');
        bytes.push_str(&self.sequence.to_string());
        bytes.push('\n');
        bytes.push_str(&self.created_unix_ms.to_string());
        bytes.push('\n');
        bytes.push_str(&self.expires_unix_ms.to_string());
        bytes.push('\n');
        bytes.push_str(&offers_json);
        bytes.push('\n');
        bytes.push_str(&echo_json);
        bytes.push('\n');
        bytes.push_str(self.attribution.signer.as_str());
        bytes.into_bytes()
    }

    /// Sign the envelope in place with the supplied Ed25519 key.
    pub fn sign(&mut self, signing_key: &SigningKey) {
        self.attribution.signer = self.source.node.clone();
        self.attribution.pubkey_hex = bytes_to_hex(signing_key.verifying_key().as_bytes());
        self.attribution.signature_hex.clear();
        let signature = signing_key.sign(&self.signing_bytes());
        self.attribution.signature_hex = bytes_to_hex(&signature.to_bytes());
    }

    /// Sign and return the envelope.
    #[must_use]
    pub fn signed(mut self, signing_key: &SigningKey) -> Self {
        self.sign(signing_key);
        self
    }

    /// Verify the detached signature and all intrinsic contract invariants.
    pub fn validate(&self) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        self.validate_fields(true)
    }

    /// Validate expiry and replay state at an injected time.
    ///
    /// `last_sequence` is the last accepted sequence for this source/session
    /// ledger. The caller must scope that ledger by the typed identities before
    /// invoking this method.
    pub fn validate_at(
        &self,
        now_unix_ms: u64,
        last_sequence: Option<u64>,
    ) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        self.validate()?;
        if now_unix_ms < self.created_unix_ms {
            return Err(ClipboardEnvelopeV2ValidationError::NotYetValid {
                now: now_unix_ms,
                created: self.created_unix_ms,
            });
        }
        if now_unix_ms >= self.expires_unix_ms {
            return Err(ClipboardEnvelopeV2ValidationError::Expired {
                now: now_unix_ms,
                expires: self.expires_unix_ms,
            });
        }
        if let Some(last_sequence) = last_sequence {
            if self.sequence <= last_sequence {
                return Err(ClipboardEnvelopeV2ValidationError::Replay {
                    sequence: self.sequence,
                    last_sequence,
                });
            }
        }
        Ok(())
    }

    /// Admit and return the envelope at an injected time.
    pub fn admitted_at(
        self,
        now_unix_ms: u64,
        last_sequence: Option<u64>,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        self.validate_at(now_unix_ms, last_sequence)?;
        Ok(self)
    }

    /// Decode and admit an intrinsically valid JSON envelope.
    pub fn from_json(body: &str) -> Result<Self, ClipboardEnvelopeV2DecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON body before it reaches a worker.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, ClipboardEnvelopeV2DecodeError> {
        if body.len() > MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES {
            return Err(ClipboardEnvelopeV2DecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES,
            });
        }
        serde_json::from_slice::<NoDuplicateJson>(body)
            .map_err(ClipboardEnvelopeV2DecodeError::Json)?;
        let wire = serde_json::from_slice::<ClipboardEnvelopeV2Wire>(body)
            .map_err(ClipboardEnvelopeV2DecodeError::Json)?;
        Self::from_wire(wire).map_err(ClipboardEnvelopeV2DecodeError::Validation)
    }

    /// Return the public key carried by a valid attribution.
    #[must_use]
    pub fn signer_key(&self) -> Option<VerifyingKey> {
        let bytes = hex_to_bytes::<32>(&self.attribution.pubkey_hex)?;
        VerifyingKey::from_bytes(&bytes).ok()
    }

    fn validate_unsigned(&self) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        self.validate_fields(false)
    }

    fn validate_fields(
        &self,
        require_signature: bool,
    ) -> Result<(), ClipboardEnvelopeV2ValidationError> {
        if self.schema_version != CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION {
            return Err(ClipboardEnvelopeV2ValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.clip_id.is_nil() {
            return Err(ClipboardEnvelopeV2ValidationError::NilClipId);
        }
        if self.session.is_nil() {
            return Err(ClipboardEnvelopeV2ValidationError::NilSessionId);
        }
        if self.sequence == 0 {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidSequence);
        }
        if self.expires_unix_ms <= self.created_unix_ms {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidTimestamp);
        }
        if self.expires_unix_ms - self.created_unix_ms > MAX_CLIPBOARD_TTL_MS {
            return Err(ClipboardEnvelopeV2ValidationError::ExpiryTooLong {
                max: MAX_CLIPBOARD_TTL_MS,
            });
        }
        if self.offers.is_empty() {
            return Err(ClipboardEnvelopeV2ValidationError::NoOffers);
        }
        if self.offers.len() > MAX_CLIPBOARD_OFFERS {
            return Err(ClipboardEnvelopeV2ValidationError::TooManyOffers {
                count: self.offers.len(),
                max: MAX_CLIPBOARD_OFFERS,
            });
        }
        let mut seen = Vec::with_capacity(self.offers.len());
        for offer in &self.offers {
            offer.validate()?;
            if seen.contains(&offer.mime) {
                return Err(ClipboardEnvelopeV2ValidationError::DuplicateMime { mime: offer.mime });
            }
            seen.push(offer.mime);
        }

        if self.echo_guard.origin != self.clip_id
            || self.echo_guard.visited_nodes.is_empty()
            || self.echo_guard.visited_nodes.len() > MAX_CLIPBOARD_ECHO_HOPS
            || !self.echo_guard.visited_nodes.contains(&self.source.node)
            || self.echo_guard.visited_nodes.contains(&self.target.node)
        {
            return Err(ClipboardEnvelopeV2ValidationError::EchoLoop);
        }
        for (index, node) in self.echo_guard.visited_nodes.iter().enumerate() {
            if self.echo_guard.visited_nodes[..index].contains(node) {
                return Err(ClipboardEnvelopeV2ValidationError::EchoLoop);
            }
        }

        if self.attribution.signer != self.source.node {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidAttribution);
        }
        if require_signature {
            if !is_lower_hex(&self.attribution.pubkey_hex, 64)
                || !is_lower_hex(&self.attribution.signature_hex, 128)
            {
                return Err(ClipboardEnvelopeV2ValidationError::MalformedSignature);
            }
            let Some(key) = self.signer_key() else {
                return Err(ClipboardEnvelopeV2ValidationError::MalformedSignature);
            };
            let Some(signature_bytes) = hex_to_bytes::<64>(&self.attribution.signature_hex) else {
                return Err(ClipboardEnvelopeV2ValidationError::MalformedSignature);
            };
            let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes);
            if key
                .verify_strict(&self.signing_bytes(), &signature)
                .is_err()
            {
                return Err(ClipboardEnvelopeV2ValidationError::InvalidSignature);
            }
        } else if !self.attribution.pubkey_hex.is_empty()
            || !self.attribution.signature_hex.is_empty()
        {
            return Err(ClipboardEnvelopeV2ValidationError::InvalidAttribution);
        }
        Ok(())
    }

    fn from_wire(
        wire: ClipboardEnvelopeV2Wire,
    ) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        Self {
            schema_version: wire.schema_version,
            clip_id: wire.clip_id,
            source: wire.source,
            target: wire.target,
            session: wire.session,
            sequence: wire.sequence,
            created_unix_ms: wire.created_unix_ms,
            expires_unix_ms: wire.expires_unix_ms,
            offers: wire.offers,
            echo_guard: wire.echo_guard,
            attribution: wire.attribution,
        }
        .admitted()
    }

    fn admitted(self) -> Result<Self, ClipboardEnvelopeV2ValidationError> {
        self.validate()?;
        Ok(self)
    }
}

/// Why an envelope failed intrinsic or time/replay admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEnvelopeV2ValidationError {
    /// The wire schema is not supported.
    UnsupportedSchema {
        /// Version found on the wire.
        found: u16,
    },
    /// The update identity was nil.
    NilClipId,
    /// The session identity was nil.
    NilSessionId,
    /// Sequence numbers start at one.
    InvalidSequence,
    /// Creation/expiry ordering is invalid.
    InvalidTimestamp,
    /// The requested lifetime exceeds the contract bound.
    ExpiryTooLong {
        /// Maximum lifetime in milliseconds.
        max: u64,
    },
    /// The envelope is not yet valid at the injected time.
    NotYetValid {
        /// Injected current time.
        now: u64,
        /// Envelope creation time.
        created: u64,
    },
    /// The envelope has reached its expiry timestamp.
    Expired {
        /// Injected current time.
        now: u64,
        /// Envelope expiry time.
        expires: u64,
    },
    /// The sequence was already accepted for the scoped source/session ledger.
    Replay {
        /// Sequence received.
        sequence: u64,
        /// Highest sequence already accepted.
        last_sequence: u64,
    },
    /// No MIME offers were supplied.
    NoOffers,
    /// The offer count exceeds the bounded wire shape.
    TooManyOffers {
        /// Number received.
        count: usize,
        /// Maximum admitted count.
        max: usize,
    },
    /// The same MIME kind appeared more than once.
    DuplicateMime {
        /// Duplicate representation.
        mime: ClipboardMimeKind,
    },
    /// A bounded field exceeded its contract limit.
    OutOfBounds {
        /// Field name.
        field: &'static str,
        /// Maximum admitted value.
        max: u64,
    },
    /// An offer's payload and metadata disagree.
    InvalidOffer {
        /// Field that failed.
        field: &'static str,
    },
    /// A digest was missing, malformed, or did not match inline bytes.
    InvalidDigest {
        /// Field that failed.
        field: &'static str,
    },
    /// A relay would send the same update back to an already visited node.
    EchoLoop,
    /// The signer identity did not match the source identity.
    InvalidAttribution,
    /// The detached signature or public key was malformed.
    MalformedSignature,
    /// The signature did not verify over the current envelope.
    InvalidSignature,
}

impl fmt::Display for ClipboardEnvelopeV2ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => {
                write!(
                    formatter,
                    "unsupported clipboard envelope V2 schema {found}"
                )
            }
            Self::NilClipId => formatter.write_str("clipboard clip id is nil"),
            Self::NilSessionId => formatter.write_str("clipboard session id is nil"),
            Self::InvalidSequence => formatter.write_str("clipboard sequence must start at one"),
            Self::InvalidTimestamp => formatter.write_str("clipboard timestamps are invalid"),
            Self::ExpiryTooLong { max } => {
                write!(formatter, "clipboard expiry exceeds {max} milliseconds")
            }
            Self::NotYetValid { now, created } => {
                write!(
                    formatter,
                    "clipboard envelope is from the future: now={now}, created={created}"
                )
            }
            Self::Expired { now, expires } => {
                write!(
                    formatter,
                    "clipboard envelope expired: now={now}, expires={expires}"
                )
            }
            Self::Replay {
                sequence,
                last_sequence,
            } => write!(
                formatter,
                "clipboard sequence {sequence} was already accepted through {last_sequence}"
            ),
            Self::NoOffers => formatter.write_str("clipboard envelope has no MIME offers"),
            Self::TooManyOffers { count, max } => {
                write!(
                    formatter,
                    "clipboard has {count} MIME offers; maximum is {max}"
                )
            }
            Self::DuplicateMime { mime } => {
                write!(formatter, "clipboard MIME offer {mime:?} is duplicated")
            }
            Self::OutOfBounds { field, max } => {
                write!(formatter, "clipboard {field} exceeds bound {max}")
            }
            Self::InvalidOffer { field } => {
                write!(formatter, "invalid clipboard offer field {field}")
            }
            Self::InvalidDigest { field } => {
                write!(formatter, "invalid clipboard digest field {field}")
            }
            Self::EchoLoop => formatter.write_str("clipboard echo guard would loop"),
            Self::InvalidAttribution => formatter.write_str("invalid clipboard attribution"),
            Self::MalformedSignature => formatter.write_str("malformed clipboard signature"),
            Self::InvalidSignature => formatter.write_str("invalid clipboard signature"),
        }
    }
}

impl std::error::Error for ClipboardEnvelopeV2ValidationError {}

/// Why a JSON clipboard body could not be decoded and admitted.
#[derive(Debug)]
pub enum ClipboardEnvelopeV2DecodeError {
    /// The body was rejected before serde allocation.
    BodyTooLarge {
        /// Number of supplied bytes.
        bytes: usize,
        /// Maximum accepted JSON body size.
        max: usize,
    },
    /// The body was malformed or contained unknown fields.
    Json(serde_json::Error),
    /// The body decoded but failed intrinsic validation.
    Validation(ClipboardEnvelopeV2ValidationError),
}

impl fmt::Display for ClipboardEnvelopeV2DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => {
                write!(
                    formatter,
                    "clipboard body is {bytes} bytes; maximum is {max}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid clipboard JSON: {error}"),
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

/// A JSON admission preflight that rejects duplicate object keys at every
/// nesting level. `serde_json` otherwise accepts duplicate fields using a
/// last-value-wins rule, which is unsafe for a signed, versioned envelope:
/// different consumers could attribute a different meaning to the same body.
struct NoDuplicateJson;

impl<'de> Deserialize<'de> for NoDuplicateJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NoDuplicateJsonVisitor)?;
        Ok(Self)
    }
}

struct NoDuplicateJsonVisitor;

impl<'de> de::Visitor<'de> for NoDuplicateJsonVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_borrowed_str<E>(self, _: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_string<E>(self, _: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(Self)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        while sequence.next_element_seed(NoDuplicateJsonSeed)?.is_some() {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        let mut seen = std::collections::BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object key {key:?} is not admitted"
                )));
            }
            map.next_value_seed(NoDuplicateJsonSeed)?;
        }
        Ok(())
    }
}

struct NoDuplicateJsonSeed;

impl<'de> de::DeserializeSeed<'de> for NoDuplicateJsonSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NoDuplicateJsonVisitor)
    }
}

fn validate_identity(value: &str) -> Result<(), ClipboardIdentityValidationError> {
    if value.is_empty() {
        return Err(ClipboardIdentityValidationError::Empty);
    }
    if value.len() > MAX_CLIPBOARD_ID_BYTES {
        return Err(ClipboardIdentityValidationError::TooLong);
    }
    if value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ClipboardIdentityValidationError::InvalidCharacters);
    }
    Ok(())
}

fn validate_metadata_text(
    value: &str,
    field: &'static str,
) -> Result<(), ClipboardEnvelopeV2ValidationError> {
    if value.is_empty() || value.len() > MAX_CLIPBOARD_PREVIEW_BYTES {
        return Err(ClipboardEnvelopeV2ValidationError::OutOfBounds {
            field,
            max: MAX_CLIPBOARD_PREVIEW_BYTES as u64,
        });
    }
    if value.chars().any(char::is_control)
        || value.chars().any(|character| {
            matches!(
                character,
                '/' | '\\' | '$' | '`' | ';' | '|' | '&' | '<' | '>'
            )
        })
        || value.contains("://")
        || contains_secret_word(value)
    {
        return Err(ClipboardEnvelopeV2ValidationError::InvalidOffer { field });
    }
    Ok(())
}

fn validate_sha256(
    value: &str,
    field: &'static str,
) -> Result<(), ClipboardEnvelopeV2ValidationError> {
    if !is_lower_hex(value, 64) {
        return Err(ClipboardEnvelopeV2ValidationError::InvalidDigest { field });
    }
    Ok(())
}

fn contains_secret_word(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "credential",
        "authorization",
        "cookie",
        "token",
        "private_key",
        "apikey",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_to_bytes<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut output = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        output[index] = u8::try_from((high << 4) | low).ok()?;
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use serde_json::json;

    use super::*;

    fn node(value: &str) -> ClipboardNodeId {
        ClipboardNodeId::new(value).expect("safe node")
    }

    fn seat(value: &str) -> ClipboardSeatId {
        ClipboardSeatId::new(value).expect("safe seat")
    }

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7_u8; 32])
    }

    fn sample_offer() -> ClipboardMimeOfferV2 {
        ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextPlain, "hello mesh")
            .expect("bounded text offer")
    }

    fn sample() -> ClipboardEnvelopeV2 {
        ClipboardEnvelopeV2::new(
            ClipboardClipId::from_uuid(Uuid::from_u128(1)),
            ClipboardSourceV2::new(node("eagle"), seat("seat-1")),
            ClipboardTargetV2::new(node("dell-15"), seat("seat-1")),
            ClipboardSessionId::from_uuid(Uuid::from_u128(2)),
            1,
            1_720_000_000_000,
            1_720_000_000_000 + 60_000,
            vec![sample_offer()],
        )
        .expect("valid unsigned envelope")
        .signed(&key())
    }

    #[test]
    fn signed_envelope_round_trips_and_verifies() {
        let envelope = sample();
        assert!(envelope.validate().is_ok());
        let json = serde_json::to_string(&envelope).expect("serialize");
        let decoded = ClipboardEnvelopeV2::from_json(&json).expect("decode and admit");
        assert_eq!(decoded, envelope);
        assert_eq!(decoded.signing_bytes(), envelope.signing_bytes());
    }

    #[test]
    fn expiry_sequence_and_echo_admission_are_real_boundaries() {
        let envelope = sample();
        assert!(envelope.validate_at(1_720_000_000_001, None).is_ok());
        assert!(matches!(
            envelope.validate_at(1_720_000_000_001, Some(1)),
            Err(ClipboardEnvelopeV2ValidationError::Replay { .. })
        ));
        assert!(matches!(
            envelope.validate_at(1_720_000_060_000, None),
            Err(ClipboardEnvelopeV2ValidationError::Expired { .. })
        ));

        let mut echo = envelope.clone();
        echo.echo_guard.visited_nodes.push(echo.target.node.clone());
        assert!(matches!(
            echo.validate(),
            Err(ClipboardEnvelopeV2ValidationError::EchoLoop)
        ));
    }

    #[test]
    fn inline_and_files_boundaries_are_enforced() {
        let exact = "x".repeat(MAX_CLIPBOARD_INLINE_TEXT_BYTES);
        assert!(ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextPlain, exact).is_ok());
        let over = "x".repeat(MAX_CLIPBOARD_INLINE_TEXT_BYTES + 1);
        assert!(matches!(
            ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextPlain, over),
            Err(ClipboardEnvelopeV2ValidationError::OutOfBounds { .. })
        ));

        let files = ClipboardMimeOfferV2::files_reference(
            ClipboardMimeKind::ImagePng,
            FileRefId::new(),
            MAX_CLIPBOARD_PAYLOAD_BYTES,
            "0".repeat(64),
        );
        assert!(files.is_ok());
        let over_files = ClipboardMimeOfferV2::files_reference(
            ClipboardMimeKind::ImagePng,
            FileRefId::new(),
            MAX_CLIPBOARD_PAYLOAD_BYTES + 1,
            "0".repeat(64),
        );
        assert!(matches!(
            over_files,
            Err(ClipboardEnvelopeV2ValidationError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn hostile_structured_inputs_are_rejected_without_command_paths_urls_or_secrets() {
        for value in [
            "",
            "../escape",
            "/etc/passwd",
            "https://example.invalid",
            "a;b",
        ] {
            assert!(ClipboardNodeId::new(value).is_err());
            assert!(ClipboardSeatId::new(value).is_err());
        }

        let mut hostile = serde_json::to_value(sample()).expect("value");
        hostile["command"] = json!("rm -rf /");
        assert!(ClipboardEnvelopeV2::from_json(&hostile.to_string()).is_err());

        let mut nested = serde_json::to_value(sample()).expect("value");
        nested["source"]["path"] = json!("/etc/passwd");
        assert!(ClipboardEnvelopeV2::from_json(&nested.to_string()).is_err());

        let mut preview = sample();
        preview.offers[0].preview = Some("https://secret.invalid".into());
        assert!(matches!(
            preview.validate(),
            Err(ClipboardEnvelopeV2ValidationError::InvalidOffer { .. })
        ));

        let mut signature = sample();
        signature.offers[0].content_sha256_hex = Some("f".repeat(64));
        assert!(matches!(
            signature.validate(),
            Err(ClipboardEnvelopeV2ValidationError::InvalidDigest { .. })
        ));
    }

    #[test]
    fn unknown_schema_too_many_offers_and_oversized_json_are_rejected() {
        let mut unknown = serde_json::to_value(sample()).expect("value");
        unknown["schema_version"] = json!(1);
        assert!(matches!(
            ClipboardEnvelopeV2::from_json(&unknown.to_string()),
            Err(ClipboardEnvelopeV2DecodeError::Validation(
                ClipboardEnvelopeV2ValidationError::UnsupportedSchema { found: 1 }
            ))
        ));

        let mut too_many = sample();
        too_many.offers = (0..=MAX_CLIPBOARD_OFFERS)
            .map(|index| {
                let mime = match index {
                    0 => ClipboardMimeKind::TextPlain,
                    1 => ClipboardMimeKind::TextHtml,
                    2 => ClipboardMimeKind::TextRtf,
                    3 => ClipboardMimeKind::ImagePng,
                    4 => ClipboardMimeKind::ImageJpeg,
                    5 => ClipboardMimeKind::FileList,
                    _ => ClipboardMimeKind::TextPlain,
                };
                ClipboardMimeOfferV2::unsupported(
                    mime,
                    ClipboardUnsupportedReason::TransportUnsupported,
                )
            })
            .collect();
        assert!(matches!(
            too_many.validate(),
            Err(ClipboardEnvelopeV2ValidationError::TooManyOffers { .. })
                | Err(ClipboardEnvelopeV2ValidationError::DuplicateMime { .. })
        ));

        let oversized = vec![b' '; MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES + 1];
        assert!(matches!(
            ClipboardEnvelopeV2::from_json_bytes(&oversized),
            Err(ClipboardEnvelopeV2DecodeError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn signed_wire_admission_rejects_ambiguous_duplicate_json_keys() {
        let json = serde_json::to_string(&sample()).expect("serialize signed envelope");
        let ambiguous = json.replacen(
            "\"sequence\":1",
            "\"sequence\":1,\"sequence\":1",
            1,
        );
        assert_ne!(ambiguous, json, "fixture must contain the signed sequence field");

        let error = ClipboardEnvelopeV2::from_json(&ambiguous)
            .expect_err("duplicate signed field must never use last-value-wins admission");
        assert!(matches!(error, ClipboardEnvelopeV2DecodeError::Json(_)));
        assert!(error.to_string().contains("duplicate JSON object key \"sequence\""));
    }

    #[test]
    fn explicit_unsupported_and_unavailable_states_carry_no_payload_metadata() {
        let mut envelope = sample();
        envelope.offers = vec![
            ClipboardMimeOfferV2::unsupported(
                ClipboardMimeKind::ImagePng,
                ClipboardUnsupportedReason::TargetMimeUnsupported,
            ),
            ClipboardMimeOfferV2::unavailable(
                ClipboardMimeKind::FileList,
                ClipboardUnavailableReason::FilesProviderUnavailable,
            ),
        ];
        envelope.sign(&key());
        assert!(envelope.validate().is_ok());

        envelope.offers[0].byte_count = 1;
        assert!(matches!(
            envelope.validate(),
            Err(ClipboardEnvelopeV2ValidationError::InvalidOffer { .. })
        ));
    }
}

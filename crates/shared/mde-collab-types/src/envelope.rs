//! [`CollabEventEnvelope`] — the versioned, Ed25519-signed unit of the log.
//!
//! Every fact in a space is one envelope: it binds an [`EventId`], the
//! [`SpaceId`] it belongs to, the [`ActorId`] that authored it, that actor's
//! [`ActorClock`] stamp, an injected creation timestamp, the
//! [`CollabEventKind`] body (with an optional content-addressed
//! [`PayloadRef`] for out-of-band bytes), and a detached Ed25519 signature over
//! the canonical [`signing_bytes`](CollabEventEnvelope::signing_bytes).
//!
//! Signing follows the exact mde-chat pattern (lock 10): the same
//! `ed25519-dalek` v2 dep, a domain-separated + field-delimited canonical byte
//! string with a stable field order, and hex-encoded pubkey + signature so the
//! envelope round-trips through JSON. The signature field is NOT part of the
//! signed bytes; every other field is, so tampering with the actor, space,
//! clock, timestamp, kind, or payload reference invalidates the signature.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::clock::ActorClock;
use crate::event::CollabEventKind;
use crate::ids::{EventId, SpaceId};
use crate::value::PayloadRef;
use crate::ActorId;

/// The current envelope schema version. Bump when the wire shape changes; the
/// version is inside the signed bytes, so a downgrade attack cannot forge it.
pub const SCHEMA_VERSION: u16 = 1;

/// Domain-separation tag for the canonical signing bytes (prevents a signature
/// minted for another context from ever verifying here).
const SIGNING_DOMAIN: &str = "mde-collab-event-v1";

/// A detached Ed25519 signature plus the signer's public key, both lower-hex.
///
/// Carrying the pubkey makes an envelope self-verifying for *well-formedness*;
/// deciding whether that key is the one the claimed [`ActorId`] is trusted to
/// use is the roster/trust layer's job, not a lone envelope's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventSignature {
    /// The signer's Ed25519 verifying key, 32 bytes hex (64 chars).
    pub pubkey_hex: String,
    /// The detached signature, 64 bytes hex (128 chars).
    pub sig_hex: String,
}

/// One versioned, signed event in a space's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollabEventEnvelope {
    /// The schema version these bytes were written under.
    pub schema_version: u16,
    /// This event's stable id.
    pub event_id: EventId,
    /// The space this event belongs to.
    pub space_id: SpaceId,
    /// The identity that authored the event (the signature proves the key; the
    /// trust layer binds the key to this identity).
    pub actor: ActorId,
    /// The author's logical-clock stamp (causal ordering key).
    pub clock: ActorClock,
    /// The injected creation time, epoch milliseconds (no wall-clock read here).
    pub created_unix_ms: i64,
    /// The event body (carries the inline typed payload).
    pub kind: CollabEventKind,
    /// A content-addressed reference to out-of-band bytes (a document snapshot,
    /// a CRDT blob, a file's bytes), when the substance is too large to inline.
    /// `None` for fully-inline events.
    #[serde(default)]
    pub payload_ref: Option<PayloadRef>,
    /// The Ed25519 signature + signer pubkey; `None` until [`sign`]ed. Not part
    /// of the signed bytes.
    #[serde(default)]
    pub signature: Option<EventSignature>,
}

impl CollabEventEnvelope {
    /// Assemble a new, **unsigned** envelope at [`SCHEMA_VERSION`]. Sign it with
    /// [`sign`](Self::sign) before it leaves the node.
    #[must_use]
    pub fn new(
        event_id: EventId,
        space_id: SpaceId,
        actor: ActorId,
        clock: ActorClock,
        created_unix_ms: i64,
        kind: CollabEventKind,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            event_id,
            space_id,
            actor,
            clock,
            created_unix_ms,
            kind,
            payload_ref: None,
            signature: None,
        }
    }

    /// Builder: attach a content-addressed payload reference.
    #[must_use]
    pub fn with_payload_ref(mut self, payload_ref: PayloadRef) -> Self {
        self.payload_ref = Some(payload_ref);
        self
    }

    /// The canonical bytes that are signed and verified.
    ///
    /// Deterministic by construction: a fixed domain tag, then every signed
    /// field in a fixed order, newline-delimited, with the `kind` and
    /// `payload_ref` rendered as canonical JSON (their structs serialize in
    /// declaration order, and any map-shaped fields use `BTreeMap`, so the bytes
    /// are stable across runs and machines). The `signature` field is
    /// deliberately excluded.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        // serde_json on these plain structs cannot fail; `unwrap_or_default`
        // keeps the `unwrap_used` lint satisfied (a corrupt/empty rendering
        // would simply fail verification rather than panic).
        let kind_json = serde_json::to_string(&self.kind).unwrap_or_default();
        let ref_json = serde_json::to_string(&self.payload_ref).unwrap_or_default();

        let mut out = String::with_capacity(kind_json.len() + ref_json.len() + 160);
        out.push_str(SIGNING_DOMAIN);
        out.push('\n');
        out.push_str(&self.schema_version.to_string());
        out.push('\n');
        out.push_str(&self.event_id.to_string());
        out.push('\n');
        out.push_str(&self.space_id.to_string());
        out.push('\n');
        out.push_str(self.actor.as_str());
        out.push('\n');
        // The clock is two integers with a fixed separator — deterministic.
        out.push_str(&self.clock.wall_ms.to_string());
        out.push(':');
        out.push_str(&self.clock.counter.to_string());
        out.push('\n');
        out.push_str(&self.created_unix_ms.to_string());
        out.push('\n');
        out.push_str(&kind_json);
        out.push('\n');
        out.push_str(&ref_json);
        out.into_bytes()
    }

    /// Sign this envelope in place with `signing_key`, stamping the pubkey +
    /// detached signature over [`signing_bytes`](Self::signing_bytes).
    pub fn sign(&mut self, signing_key: &SigningKey) {
        let sig = signing_key.sign(&self.signing_bytes());
        self.signature = Some(EventSignature {
            pubkey_hex: bytes_to_hex(signing_key.verifying_key().as_bytes()),
            sig_hex: bytes_to_hex(&sig.to_bytes()),
        });
    }

    /// Builder form of [`sign`](Self::sign): sign and return `self`.
    #[must_use]
    pub fn signed(mut self, signing_key: &SigningKey) -> Self {
        self.sign(signing_key);
        self
    }

    /// Verify the envelope's signature against the pubkey it carries.
    ///
    /// `true` only when the envelope is signed, the pubkey + signature are
    /// well-formed, and the signature matches the current canonical bytes — so
    /// any post-signing tamper of the actor, space, clock, timestamp, kind, or
    /// payload reference makes this `false`. An unsigned envelope is `false`
    /// (the strict check never silently trusts unsigned data).
    #[must_use]
    pub fn verify(&self) -> bool {
        let Some(sig) = &self.signature else {
            return false;
        };
        let (Some(pub_bytes), Some(sig_bytes)) = (
            hex_to_bytes::<32>(&sig.pubkey_hex),
            hex_to_bytes::<64>(&sig.sig_hex),
        ) else {
            return false;
        };
        let Ok(vk) = VerifyingKey::from_bytes(&pub_bytes) else {
            return false;
        };
        vk.verify_strict(
            &self.signing_bytes(),
            &ed25519_dalek::Signature::from_bytes(&sig_bytes),
        )
        .is_ok()
    }

    /// The signer's claimed verifying key, when signed + well-formed. The trust
    /// layer compares this against the key the [`actor`](Self::actor) publishes.
    #[must_use]
    pub fn signer_key(&self) -> Option<VerifyingKey> {
        let sig = self.signature.as_ref()?;
        let bytes = hex_to_bytes::<32>(&sig.pubkey_hex)?;
        VerifyingKey::from_bytes(&bytes).ok()
    }
}

/// Lower-hex encode bytes (the mackesd CA-blocklist / mde-chat convention — no
/// external hex crate for a 32/64-byte field).
fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Decode exactly `N` bytes of lower/upper hex, or `None` on any malformed input
/// (wrong length or a non-hex nibble).
fn hex_to_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0_u8; N];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = u8::try_from((chunk[0] as char).to_digit(16)?).ok()?;
        let lo = u8::try_from((chunk[1] as char).to_digit(16)?).ok()?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use super::*;
    use crate::ids::{DocumentId, ThreadId};
    use crate::value::{DocumentChange, MessageBody, PayloadRef};

    fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn sample() -> CollabEventEnvelope {
        CollabEventEnvelope::new(
            EventId::new(),
            SpaceId::new(),
            ActorId::new("eagle"),
            ActorClock::at(1_720_000_000_000, 3),
            1_720_000_000_000,
            CollabEventKind::MessagePosted {
                body: MessageBody::new("hello **mesh**"),
                thread: None,
            },
        )
    }

    #[test]
    fn envelope_round_trips_through_serde() {
        let env = sample().signed(&key());
        let json = serde_json::to_string(&env).expect("serialize");
        let back: CollabEventEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(env, back);
        assert!(back.verify(), "signature survives a JSON round-trip");
    }

    #[test]
    fn signing_bytes_are_deterministic() {
        let env = sample();
        assert_eq!(
            env.signing_bytes(),
            env.signing_bytes(),
            "same envelope, same bytes"
        );
        // Byte-identical clones sign to byte-identical canonical forms.
        let clone = env.clone();
        assert_eq!(env.signing_bytes(), clone.signing_bytes());
    }

    #[test]
    fn signing_bytes_exclude_the_signature_field() {
        let unsigned = sample();
        let before = unsigned.signing_bytes();
        let signed = unsigned.signed(&key());
        assert_eq!(
            before,
            signed.signing_bytes(),
            "signing does not change the signed bytes"
        );
    }

    #[test]
    fn unsigned_never_verifies() {
        assert!(!sample().verify());
    }

    #[test]
    fn sign_then_verify_succeeds() {
        let env = sample().signed(&key());
        assert!(env.verify());
    }

    #[test]
    fn tampering_actor_fails_verification() {
        let mut env = sample().signed(&key());
        env.actor = ActorId::new("attacker");
        assert!(!env.verify(), "forged sender must fail");
    }

    #[test]
    fn tampering_space_fails_verification() {
        let mut env = sample().signed(&key());
        env.space_id = SpaceId::new();
        assert!(!env.verify(), "moved-space forgery must fail");
    }

    #[test]
    fn tampering_kind_fails_verification() {
        let mut env = sample().signed(&key());
        env.kind = CollabEventKind::MessagePosted {
            body: MessageBody::new("tampered body"),
            thread: Some(ThreadId::new()),
        };
        assert!(!env.verify(), "altered body must fail");
    }

    #[test]
    fn tampering_payload_ref_fails_verification() {
        let mut env = CollabEventEnvelope::new(
            EventId::new(),
            SpaceId::new(),
            ActorId::new("eagle"),
            ActorClock::at(10, 0),
            10,
            CollabEventKind::DocumentUpdated {
                document: DocumentId::new(),
                change: DocumentChange {
                    payload: PayloadRef::of_bytes(b"v1"),
                    summary: None,
                },
            },
        )
        .with_payload_ref(PayloadRef::of_bytes(b"snapshot-v1"))
        .signed(&key());
        env.payload_ref = Some(PayloadRef::of_bytes(b"snapshot-v2"));
        assert!(!env.verify(), "swapped content ref must fail");
    }

    #[test]
    fn tampering_clock_or_timestamp_fails_verification() {
        let mut env = sample().signed(&key());
        env.clock = ActorClock::at(env.clock.wall_ms, env.clock.counter + 1);
        assert!(!env.verify(), "clock is signed");

        let mut env2 = sample().signed(&key());
        env2.created_unix_ms += 1;
        assert!(!env2.verify(), "timestamp is signed");
    }

    #[test]
    fn wrong_key_fails_verification() {
        let mut env = sample();
        env.sign(&key());
        // Re-point the pubkey to a different signer's key: mismatch.
        let other = key();
        if let Some(sig) = env.signature.as_mut() {
            sig.pubkey_hex = bytes_to_hex(other.verifying_key().as_bytes());
        }
        assert!(!env.verify());
    }

    #[test]
    fn hex_helpers_round_trip_and_reject_malformed() {
        let bytes = [0xde_u8, 0xad, 0xbe, 0xef];
        assert_eq!(hex_to_bytes::<4>(&bytes_to_hex(&bytes)), Some(bytes));
        assert_eq!(hex_to_bytes::<4>("dead"), None, "wrong length");
        assert_eq!(hex_to_bytes::<2>("zzzz"), None, "non-hex nibble");
    }
}

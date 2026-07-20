//! The injected signing + id-minting seams.
//!
//! The core never owns an RNG or a key: the caller hands it an [`EventSigner`]
//! (the real one wraps an `ed25519-dalek` `SigningKey`; a test one signs with a
//! deterministic key) and an [`IdSource`] (the real one mints random UUIDv4
//! ids; a test one mints a deterministic sequence). Keeping both injectable is
//! what makes the pipeline replay identically under test.

use ed25519_dalek::SigningKey;
use mde_collab_types::ids::EventId;
use mde_collab_types::CollabEventEnvelope;

/// Signs an assembled envelope in place, stamping the signer's public key + a
/// detached Ed25519 signature over the envelope's canonical bytes.
pub trait EventSigner {
    /// Sign `envelope` in place. After this the envelope's
    /// [`verify`](CollabEventEnvelope::verify) must return `true`.
    fn sign(&self, envelope: &mut CollabEventEnvelope);
}

/// The production signer: an `ed25519-dalek` v2 `SigningKey` (lock 10 — the same
/// dep + version + pattern the contracts + mde-chat sign with, never openssl).
pub struct Ed25519Signer {
    key: SigningKey,
}

impl Ed25519Signer {
    /// Wrap a caller-owned signing key.
    #[must_use]
    pub const fn new(key: SigningKey) -> Self {
        Self { key }
    }

    /// Rebuild the signer from a raw 32-byte seed (deterministic construction —
    /// used by tests and by a caller rehydrating a persisted seat key).
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(&seed),
        }
    }

    /// Borrow the underlying signing key (for a caller that also needs the
    /// verifying key to publish into the trust layer).
    #[must_use]
    pub const fn signing_key(&self) -> &SigningKey {
        &self.key
    }
}

impl EventSigner for Ed25519Signer {
    fn sign(&self, envelope: &mut CollabEventEnvelope) {
        envelope.sign(&self.key);
    }
}

/// Mints fresh [`EventId`]s. A real caller uses [`RandomIds`] (UUIDv4); a test
/// injects a deterministic sequence so a replay is byte-stable.
pub trait IdSource {
    /// Mint the next event id.
    fn next_event_id(&mut self) -> EventId;
}

/// The production id source — a fresh random UUIDv4 per event.
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomIds;

impl IdSource for RandomIds {
    fn next_event_id(&mut self) -> EventId {
        EventId::new()
    }
}

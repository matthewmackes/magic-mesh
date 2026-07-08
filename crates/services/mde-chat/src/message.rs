//! [`Message`] — one entry in a conversation, human-typed or machine-folded.
//!
//! A message carries three identity-bearing facts (lock 2/10/21): the **sender
//! host** (the hostname *is* the username, and is what the signature binds to),
//! an injected-time [`MessageId`] (a ULID minted from a caller-supplied
//! timestamp — no wall-clock in this pure model), and an optional Ed25519
//! [`Signature`]. The body is a [`MessageKind`]: the six kinds Mesh Chat carries
//! (lock 15) — human text, a re-copyable clipboard item, a system alert card, a
//! file send-to, a Call hand-off, or a Remote-Control hand-off.
//!
//! **Signing** (lock 10) is over the *canonical bytes* of (id, sender, ts,
//! kind), so tampering with the sender or the body invalidates the signature —
//! "from nyc3" is unforgeable. Verification here checks the signature against
//! the pubkey the message carries; binding that pubkey to a trusted contact is
//! the [`Roster`](crate::Roster)'s job, not a single message's.

use std::collections::BTreeMap;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::alert::Severity;

/// A typed inline action carried by a folded alert card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertActionKind {
    /// A non-destructive verb that may fire immediately.
    Safe,
    /// A destructive verb; the UI must send an armed confirmation before the
    /// worker executes it.
    Destructive,
    /// Mark this alert handled for the local seat.
    Ack,
    /// Temporarily hush this alert for the local seat.
    Snooze,
}

/// One configured action button for a folded alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertAction {
    /// Stable action id inside this alert.
    pub id: String,
    /// Button label.
    pub label: String,
    /// The Bus verb this action drives, when it drives an external verb.
    #[serde(default)]
    pub verb: Option<String>,
    /// The action semantics, including destructive arming.
    #[serde(default = "default_alert_action_kind")]
    pub kind: AlertActionKind,
}

const fn default_alert_action_kind() -> AlertActionKind {
    AlertActionKind::Safe
}

/// A stable, sortable message id: a **ULID minted from the injected message
/// timestamp** (lock 8/22 ordering).
///
/// Its lexicographic order matches time order, and its 80-bit random tail makes
/// two messages in the same millisecond distinct. Injected time keeps the model
/// pure (the worker supplies the clock).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl MessageId {
    /// Mint a fresh id whose timestamp component is `ts_unix_ms` (the injected
    /// send time) and whose tail is random, so ids are unique + time-sortable.
    #[must_use]
    pub fn mint(ts_unix_ms: i64) -> Self {
        let ms = u64::try_from(ts_unix_ms).unwrap_or(0);
        Self(Ulid::from_parts(ms, rand_u128()).to_string())
    }

    /// Wrap a caller-supplied id verbatim (e.g. a deterministic id in a test, or
    /// an id already minted upstream in the worker).
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A random 80-bit tail for a ULID, widened to the u128 the ULID API wants.
/// Only the *random* half of a ULID — the timestamp half is always the injected
/// send time — so this is entropy, never a clock read.
fn rand_u128() -> u128 {
    u128::from(rand_seed())
}

/// A tiny non-cryptographic entropy source for the ULID tail. Kept dependency
/// free in the model (the security-bearing randomness is the Ed25519 keypair,
/// which the worker supplies); collisions here only affect *ordering* ties,
/// which the signature tiebreak resolves anyway.
fn rand_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    // The default RandomState is process-seeded; hashing a fresh unit gives a
    // cheap, allocation-free, per-call-varying value with no clock/IO.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(0);
    h.finish()
}

/// The six Mesh Chat message kinds (lock 15).
///
/// Human text + machine notifications share one timeline, so a
/// [`Clipboard`](MessageKind::Clipboard) copy or an
/// [`Alert`](MessageKind::Alert) is just another message from a host's contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// A human-typed line (emoji included — it is just text).
    Text(String),
    /// A clipboard copy from the sender host (lock 3). `preview` is the short,
    /// monospace one-liner the roster shows; `full` is the exact text a
    /// one-click re-copy puts back on the local clipboard.
    Clipboard {
        /// Short single-line preview for the conversation row.
        preview: String,
        /// The full clipboard payload to re-copy verbatim.
        full: String,
    },
    /// A system alert folded from a Bus lane (lock 11) — the notification-as-a-
    /// message. Built by [`fold_alert`](crate::fold_alert).
    Alert {
        /// Color/urgency axis (drives styling + the severity mute).
        severity: Severity,
        /// The source flag from the originating topic (e.g. `security`,
        /// `firewall`) — see [`alert_flag`](crate::alert_flag).
        flag: String,
        /// The alert's remaining string fields (title/summary/host/…), ordered
        /// for a stable signature + render.
        fields: BTreeMap<String, String>,
        /// An optional inline action verb the card offers (e.g.
        /// `action/shell/goto`), `None` when the alert is informational only.
        action_verb: Option<String>,
        /// Typed, configurable inline actions for the alert.
        #[serde(default)]
        actions: Vec<AlertAction>,
    },
    /// A file offered to the conversation via the mesh transfer (lock 15, reuses
    /// `mde-files` Send-To). The bytes never live here — only the offer.
    File {
        /// Display file name.
        name: String,
        /// Size in bytes (for the row's "12.3 MB" hint + transfer progress).
        size_bytes: u64,
        /// Optional MIME type, when the sender knew it.
        mime: Option<String>,
    },
    /// A "start a Call" hand-off to `mde-voice` (lock 15) — chat is the launch
    /// point; `target_host` is the contact to dial.
    CallAction {
        /// The mesh host to place the SIP call to.
        target_host: String,
    },
    /// A "Remote Control" hand-off to `mde-vdi` (lock 15) — open that host's VDI
    /// desktop from the conversation.
    RemoteAction {
        /// The mesh host to open a remote desktop into.
        target_host: String,
    },
}

impl MessageKind {
    /// A short stable tag for the kind — handy for metrics, sounds (lock 12) and
    /// operator-facing summaries, without matching the whole payload.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Clipboard { .. } => "clipboard",
            Self::Alert { .. } => "alert",
            Self::File { .. } => "file",
            Self::CallAction { .. } => "call",
            Self::RemoteAction { .. } => "remote",
        }
    }
}

/// An Ed25519 signature over a [`Message`]'s canonical bytes, plus the signer's
/// public key.
///
/// Both are hex-encoded so the message round-trips through the JSON Syncthing
/// ring log (the same hex convention as the mackesd CA blocklist).
///
/// Carrying the pubkey makes a message *self-verifying* for well-formedness;
/// deciding whether that pubkey is the one a trusted contact publishes is the
/// roster/trust layer's call (lock 6/10), not a lone message's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// The signer's Ed25519 verifying key, 32 bytes hex (64 chars).
    pub pubkey_hex: String,
    /// The detached signature, 64 bytes hex (128 chars).
    pub sig_hex: String,
}

/// One message in a conversation (human-typed or machine-folded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Stable, time-sortable id (lock 8/22).
    pub id: MessageId,
    /// The **sender host** — the hostname *is* the username (lock 2/21) and is
    /// what the signature binds to (lock 10).
    pub sender: String,
    /// The injected send time in epoch milliseconds (no wall-clock here — the
    /// worker supplies it). The primary conversation ordering axis (lock 22).
    pub ts_unix_ms: i64,
    /// The body.
    pub kind: MessageKind,
    /// The Ed25519 signature + signer pubkey; `None` until [`sign`]ed.
    pub signature: Option<Signature>,
}

impl Message {
    /// A new, **unsigned** message from `sender` at injected time `ts_unix_ms`,
    /// with a freshly minted [`MessageId`]. Sign it with [`sign`] before it
    /// leaves the node.
    #[must_use]
    pub fn new(sender: impl Into<String>, ts_unix_ms: i64, kind: MessageKind) -> Self {
        Self {
            id: MessageId::mint(ts_unix_ms),
            sender: sender.into(),
            ts_unix_ms,
            kind,
            signature: None,
        }
    }

    /// Convenience: a plain [`Text`](MessageKind::Text) message.
    #[must_use]
    pub fn text(sender: impl Into<String>, ts_unix_ms: i64, body: impl Into<String>) -> Self {
        Self::new(sender, ts_unix_ms, MessageKind::Text(body.into()))
    }

    /// The canonical bytes signed + verified (lock 10). Domain-separated and
    /// field-delimited so the id, sender, timestamp and body all fall under the
    /// signature — tampering with any of them invalidates it. The kind is its
    /// canonical serde JSON (the `Alert` fields are a `BTreeMap`, so key order —
    /// and thus these bytes — are deterministic).
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let kind_json = serde_json::to_string(&self.kind).unwrap_or_default();
        let mut out = String::with_capacity(kind_json.len() + 96);
        out.push_str("mde-chat-msg-v1\n");
        out.push_str(self.id.as_str());
        out.push('\n');
        out.push_str(&self.sender);
        out.push('\n');
        out.push_str(&self.ts_unix_ms.to_string());
        out.push('\n');
        out.push_str(&kind_json);
        out.into_bytes()
    }

    /// Verify the message's signature against the pubkey it carries (lock 10).
    /// `true` only when the message is signed, the pubkey + signature are
    /// well-formed, and the signature matches the canonical bytes — so a message
    /// whose sender or body was altered after signing fails. An **unsigned**
    /// message is `false` (this is the strict check; the model never silently
    /// trusts an unsigned message).
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
    /// layer compares this against the contact's published key.
    #[must_use]
    pub fn signer_key(&self) -> Option<VerifyingKey> {
        let sig = self.signature.as_ref()?;
        let bytes = hex_to_bytes::<32>(&sig.pubkey_hex)?;
        VerifyingKey::from_bytes(&bytes).ok()
    }
}

/// Sign `msg` in place with `signing_key`, stamping its [`Signature`] (pubkey +
/// detached signature over [`Message::signing_bytes`], lock 10).
///
/// The worker calls this with the node's identity key before a message goes on
/// the Bus / into the Syncthing log, so every peer can prove "from `<host>`".
pub fn sign(msg: &mut Message, signing_key: &SigningKey) {
    let sig = signing_key.sign(&msg.signing_bytes());
    msg.signature = Some(Signature {
        pubkey_hex: bytes_to_hex(signing_key.verifying_key().as_bytes()),
        sig_hex: bytes_to_hex(&sig.to_bytes()),
    });
}

/// Lower-hex encode bytes (the mackesd CA-blocklist convention — no external
/// hex crate for a 32/64-byte field).
fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Decode exactly `N` bytes of lower/upper hex, or `None` on any malformed
/// input (wrong length or a non-hex nibble).
fn hex_to_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0_u8; N];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        // to_digit(16) yields 0..=15, so each nibble fits a u8 without truncation.
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

    fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn message_id_is_time_sortable_and_unique() {
        let early = MessageId::mint(1_000);
        let late = MessageId::mint(2_000);
        assert!(early < late, "later timestamp sorts after earlier");
        // Two ids in the same millisecond differ (random tail).
        assert_ne!(MessageId::mint(1_000), MessageId::mint(1_000));
    }

    #[test]
    fn every_kind_round_trips_through_serde() {
        let mut fields = BTreeMap::new();
        fields.insert("summary".to_string(), "disk low".to_string());
        let kinds = [
            MessageKind::Text("hi 👋".into()),
            MessageKind::Clipboard {
                preview: "ssh key…".into(),
                full: "ssh-ed25519 AAAA".into(),
            },
            MessageKind::Alert {
                severity: Severity::Warning,
                flag: "firewall".into(),
                fields,
                action_verb: Some("action/shell/goto".into()),
                actions: Vec::new(),
            },
            MessageKind::File {
                name: "iso.img".into(),
                size_bytes: 4096,
                mime: None,
            },
            MessageKind::CallAction {
                target_host: "nyc3".into(),
            },
            MessageKind::RemoteAction {
                target_host: "fra1".into(),
            },
        ];
        for k in kinds {
            let json = serde_json::to_string(&k).expect("serialize");
            let back: MessageKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(k, back, "round-trip {}", k.tag());
        }
    }

    #[test]
    fn message_round_trips_through_serde() {
        let mut msg = Message::text("eagle", 1_720_000_000_000, "hello mesh");
        sign(&mut msg, &key());
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg, back);
    }

    #[test]
    fn sign_then_verify_succeeds() {
        let mut msg = Message::text("eagle", 42, "authentic");
        assert!(!msg.verify(), "unsigned is never trusted");
        sign(&mut msg, &key());
        assert!(msg.verify(), "a freshly signed message verifies");
    }

    #[test]
    fn tampered_sender_fails_verify() {
        let mut msg = Message::text("eagle", 42, "authentic");
        sign(&mut msg, &key());
        // Forge the origin host — the load-bearing "from <host>" claim.
        msg.sender = "nyc3".into();
        assert!(!msg.verify(), "a forged sender must fail verify");
    }

    #[test]
    fn tampered_body_fails_verify() {
        let mut msg = Message::text("eagle", 42, "send $10");
        sign(&mut msg, &key());
        msg.kind = MessageKind::Text("send $1000".into());
        assert!(!msg.verify(), "an altered body must fail verify");
    }

    #[test]
    fn a_different_key_does_not_verify_for_this_pubkey() {
        // Sign, then swap in a *valid but wrong* signature from another key over
        // the same bytes while keeping the original pubkey: must fail.
        let mut msg = Message::text("eagle", 42, "hi");
        sign(&mut msg, &key());
        let original_pub = msg.signature.as_ref().unwrap().pubkey_hex.clone();
        let other = key();
        let forged = other.sign(&msg.signing_bytes());
        msg.signature = Some(Signature {
            pubkey_hex: original_pub,
            sig_hex: bytes_to_hex(&forged.to_bytes()),
        });
        assert!(!msg.verify(), "sig from a different key must not verify");
    }

    #[test]
    fn hex_round_trips_and_rejects_malformed() {
        let bytes = [0xde, 0xad, 0xbe, 0xef];
        assert_eq!(hex_to_bytes::<4>(&bytes_to_hex(&bytes)), Some(bytes));
        assert_eq!(hex_to_bytes::<4>("dead"), None, "wrong length");
        assert_eq!(hex_to_bytes::<2>("zzzz"), None, "non-hex nibble");
    }
}

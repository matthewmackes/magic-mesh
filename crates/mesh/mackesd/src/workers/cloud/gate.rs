//! Workloads U2 — the two load-bearing gates of the `cloud` worker.
//!
//! This module owns the pure, I/O-free gate logic the drain + verb dispatch
//! consult:
//!
//! 1. **The armed-token gate** ([`verify_token`] / [`decide`]) — replaces the
//!    retired `MDE_CLOUD_APPLY=1` env wall. A live mutation is authorized by an
//!    **armed token**: a mesh-identity-signed capability carrying a nonce + expiry,
//!    bound to the exact verb + placement node it authorizes. For THIS unit the
//!    token *verification structure* + a *signing seam* ([`TokenSigner`]) are the
//!    deliverable — the production signer ([`HmacTokenSigner`]) is a keyed-SHA256
//!    structural signer over the mesh arming key, and a later unit swaps it for the
//!    real per-node mesh-identity (Ed25519 + CA) signer WITHOUT changing the token
//!    shape or this gate. The gate is real: it rejects a missing / malformed /
//!    expired / verb-or-node-mismatched / forged token (never a fabricated pass).
//!
//! 2. **The placement gate** ([`placement_match`]) — replaces the leader gate. A
//!    mutation is performed by exactly the node it is placed on (`body.node ==
//!    self.host`); a mutation for another node is that node's to perform, and a
//!    mutation for an *unreachable* node is honestly gated (never a silent swallow).
//!    Reads are not placement-scoped — they stay local on every node.

use super::verbs::CloudVerb;

/// The minimum nonce length a well-formed armed token carries (a short/absent
/// nonce is a malformed capability, never accepted).
pub(crate) const TOKEN_NONCE_MIN_LEN: usize = 8;

/// The env seam the production signer reads the mesh arming key from. A later unit
/// replaces this shared-secret seam with the real per-node mesh-identity key; the
/// token shape + gate are unchanged by that swap. Absent / empty ⇒ this node has no
/// arming key, so token-arming is unavailable (every mutation stages honestly).
pub(crate) const ARM_KEY_ENV: &str = "MDE_CLOUD_ARM_KEY";

// ─────────────────────────── the armed token ───────────────────────────

/// An armed-token capability authorizing ONE live mutation.
///
/// Wire form (the `armed_token` string a mutation request carries):
/// `v1|<nonce>|<expires_at_ms>|<verb>|<node>|<sig>` — pipe-delimited so a
/// dotted FQDN node id never collides with the field separator. The signature
/// covers the [`ArmedToken::signing_payload`] (everything before the final `sig`),
/// so the nonce, expiry, verb, and node are all authenticated: a token cannot be
/// replayed onto a different verb or node, nor its expiry extended, without the
/// signer's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmedToken {
    /// A per-arming random nonce (single-use in a full nonce-ledger design; U2
    /// authenticates + expires it, the ledger is a later unit).
    pub nonce: String,
    /// Wall-clock expiry (ms since the Unix epoch) — the capability is refused past it.
    pub expires_at_ms: i64,
    /// The verb this token authorizes (bound — a `provision` token can't arm a `destroy`).
    pub verb: String,
    /// The placement node this token authorizes (bound — a token for node A can't
    /// arm a mutation on node B).
    pub node: String,
    /// The signer's signature over [`ArmedToken::signing_payload`].
    pub signature: String,
}

impl ArmedToken {
    /// The authenticated payload — everything the signature covers.
    #[must_use]
    pub fn signing_payload(&self) -> String {
        format!(
            "v1|{}|{}|{}|{}",
            self.nonce, self.expires_at_ms, self.verb, self.node
        )
    }

    /// The full wire token (`<payload>|<sig>`).
    #[must_use]
    pub fn encode(&self) -> String {
        format!("{}|{}", self.signing_payload(), self.signature)
    }

    /// Mint (sign) a token for `(verb, node)` valid until `expires_at_ms` — the
    /// arming seam the Workloads surface (and tests) use to author a capability.
    #[must_use]
    pub fn mint(
        signer: &dyn TokenSigner,
        nonce: &str,
        expires_at_ms: i64,
        verb: &str,
        node: &str,
    ) -> Self {
        let mut token = Self {
            nonce: nonce.to_string(),
            expires_at_ms,
            verb: verb.to_string(),
            node: node.to_string(),
            signature: String::new(),
        };
        token.signature = signer.sign_payload(&token.signing_payload());
        token
    }

    /// Parse a wire token, or `None` when the shape is not a `v1` armed token
    /// (never guessed — a malformed token is honestly refused by the gate).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.trim().split('|').collect();
        if parts.len() != 6 || parts[0] != "v1" {
            return None;
        }
        let expires_at_ms = parts[2].parse::<i64>().ok()?;
        Some(Self {
            nonce: parts[1].to_string(),
            expires_at_ms,
            verb: parts[3].to_string(),
            node: parts[4].to_string(),
            signature: parts[5].to_string(),
        })
    }
}

// ─────────────────────────── the signing seam ───────────────────────────

/// The token signing/verification seam.
///
/// U2 ships [`HmacTokenSigner`] (keyed SHA-256 over the mesh arming key). A later
/// unit implements this trait with the real per-node mesh-identity signer; the
/// gate + token shape do not change.
pub trait TokenSigner: Send + Sync {
    /// The signature this signer produces over `payload` (and accepts on verify).
    fn sign_payload(&self, payload: &str) -> String;
}

/// The production structural signer: `hex(sha256(key ‖ 0x00 ‖ payload))` keyed on
/// the mesh arming key. Symmetric (any node holding the shared mesh arming key can
/// verify any node's token); the asymmetric per-node mesh-identity signer is the
/// later-unit swap this seam exists for.
pub(crate) struct HmacTokenSigner {
    key: Vec<u8>,
}

impl HmacTokenSigner {
    /// Construct from raw key bytes.
    #[must_use]
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self { key: key.into() }
    }

    /// Read the mesh arming key from [`ARM_KEY_ENV`]. `None` ⇒ no key ⇒ this node
    /// cannot arm (every mutation stages).
    #[must_use]
    pub fn from_env() -> Option<Self> {
        std::env::var(ARM_KEY_ENV)
            .ok()
            .filter(|k| !k.is_empty())
            .map(|k| Self::new(k.into_bytes()))
    }
}

impl TokenSigner for HmacTokenSigner {
    fn sign_payload(&self, payload: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&self.key);
        h.update([0u8]);
        h.update(payload.as_bytes());
        hex_encode(&h.finalize())
    }
}

/// The no-key signer a node without a mesh arming key uses: it produces a
/// signature no client could reproduce, so every presented token fails the
/// signature check and every mutation stages honestly (the "arming unavailable"
/// capability state). Never validates a token.
pub(crate) struct NullSigner;

impl TokenSigner for NullSigner {
    fn sign_payload(&self, _payload: &str) -> String {
        // A sentinel that no real token's `signature` field ever equals.
        "\u{0}arming-unavailable\u{0}".to_string()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A length-checked constant-time-ish equality for signature bytes (avoids an
/// early-exit compare leaking the matched prefix length).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ─────────────────────────── the armed-token gate ───────────────────────────

/// The verdict of verifying an armed token against a `(verb, node, now)` context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenVerdict {
    /// A well-formed, unexpired, correctly-bound, correctly-signed token.
    Valid,
    /// No token was presented — the mutation stages (the default, safe path).
    Missing,
    /// A token was presented but is not a parseable `v1` armed token / has a stunted nonce.
    Malformed,
    /// The token's expiry is in the past.
    Expired,
    /// The token authorizes a different verb or node than this request.
    Mismatch,
    /// The signature does not verify under the signer's key (forged / wrong key).
    BadSignature,
}

impl TokenVerdict {
    /// Whether the token authorizes a live apply.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        matches!(self, Self::Valid)
    }

    /// The honest operator-facing reason a non-`Valid` verdict staged the mutation.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Valid => "armed",
            Self::Missing => "no armed token supplied",
            Self::Malformed => "armed token is malformed",
            Self::Expired => "armed token has expired",
            Self::Mismatch => "armed token does not authorize this verb/node",
            Self::BadSignature => "armed token signature did not verify",
        }
    }
}

/// Verify an armed token for `(verb, node)` at `now_ms` under `signer`.
///
/// Honest by construction (§7): every failure mode is a distinct, truthful verdict
/// and NONE of them fabricate a pass. A `Valid` verdict means the token parsed, its
/// nonce is present, it has not expired, it is bound to exactly this verb + node,
/// and its signature verifies.
#[must_use]
pub(crate) fn verify_token(
    token: Option<&str>,
    verb: &str,
    node: &str,
    now_ms: i64,
    signer: &dyn TokenSigner,
) -> TokenVerdict {
    let Some(raw) = token.map(str::trim).filter(|s| !s.is_empty()) else {
        return TokenVerdict::Missing;
    };
    let Some(parsed) = ArmedToken::parse(raw) else {
        return TokenVerdict::Malformed;
    };
    if parsed.nonce.len() < TOKEN_NONCE_MIN_LEN {
        return TokenVerdict::Malformed;
    }
    if parsed.verb != verb || parsed.node != node {
        return TokenVerdict::Mismatch;
    }
    if now_ms > parsed.expires_at_ms {
        return TokenVerdict::Expired;
    }
    let expected = signer.sign_payload(&parsed.signing_payload());
    if !ct_eq(expected.as_bytes(), parsed.signature.as_bytes()) {
        return TokenVerdict::BadSignature;
    }
    TokenVerdict::Valid
}

/// The pre-run decision for a verb given the armed-token verdict — the pure gate
/// tested without a runner (mirrors the retired `router_action::pre_apply_decision`
/// idiom, now token-driven).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloudDecision {
    /// A read verb — always served.
    Read,
    /// A mutation and a valid armed token — perform the real op.
    Apply,
    /// A mutation without a valid armed token — stage it (plan / `--check`).
    Staged,
}

/// The pure gate: reads serve; mutations apply iff `token_valid`, else stage. No
/// I/O — the gate is tested without a hypervisor. Replaces the pre-U2
/// `decide(verb, apply_armed)` env-wall signature.
#[must_use]
pub(crate) const fn decide(verb: CloudVerb, token_valid: bool) -> CloudDecision {
    if !verb.is_mutation() {
        CloudDecision::Read
    } else if token_valid {
        CloudDecision::Apply
    } else {
        CloudDecision::Staged
    }
}

// ─────────────────────────── the placement gate ───────────────────────────

/// Where a request is placed relative to this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placement {
    /// This node performs the mutation — it is the placement target (`body.node ==
    /// host`), or the request is node-agnostic (empty `body.node`, the legacy path).
    Local,
    /// The mutation is placed on another node — carries that node's id. This node
    /// does not perform it (the target does, or it is honestly gated when the
    /// target is unreachable).
    Remote(String),
}

/// The placement decision: a mutation is [`Placement::Local`] iff its `body.node`
/// is empty or equals this host, else [`Placement::Remote`]. Pure + testable — the
/// leader gate's replacement (routing is now by placement, not by a single elected
/// actor).
#[must_use]
pub(crate) fn placement_match(body_node: &str, host: &str) -> Placement {
    let node = body_node.trim();
    if node.is_empty() || node == host {
        Placement::Local
    } else {
        Placement::Remote(node.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> HmacTokenSigner {
        HmacTokenSigner::new(b"test-mesh-arming-key".to_vec())
    }

    #[test]
    fn a_freshly_minted_token_verifies_for_its_bound_verb_and_node() {
        let s = signer();
        let tok = ArmedToken::mint(&s, "nonce-abcdef", 10_000, "provision", "eagle");
        let verdict = verify_token(Some(&tok.encode()), "provision", "eagle", 5_000, &s);
        assert_eq!(verdict, TokenVerdict::Valid);
        assert!(verdict.is_valid());
    }

    #[test]
    fn a_token_round_trips_through_encode_parse() {
        let s = signer();
        let tok = ArmedToken::mint(&s, "nonce-abcdef", 10_000, "destroy", "db.mesh.internal");
        // A dotted FQDN node survives the pipe-delimited encoding.
        let back = ArmedToken::parse(&tok.encode()).expect("parse");
        assert_eq!(back, tok);
        assert_eq!(back.node, "db.mesh.internal");
    }

    #[test]
    fn every_failure_mode_is_a_distinct_honest_verdict_never_a_fabricated_pass() {
        let s = signer();
        // Missing.
        assert_eq!(
            verify_token(None, "provision", "eagle", 0, &s),
            TokenVerdict::Missing
        );
        assert_eq!(
            verify_token(Some("   "), "provision", "eagle", 0, &s),
            TokenVerdict::Missing
        );
        // Malformed (not a v1 token).
        assert_eq!(
            verify_token(Some("garbage"), "provision", "eagle", 0, &s),
            TokenVerdict::Malformed
        );
        // Malformed (stunted nonce).
        let short = ArmedToken::mint(&s, "abc", 10_000, "provision", "eagle");
        assert_eq!(
            verify_token(Some(&short.encode()), "provision", "eagle", 0, &s),
            TokenVerdict::Malformed
        );
        // Mismatch — right key, wrong verb.
        let tok = ArmedToken::mint(&s, "nonce-abcdef", 10_000, "provision", "eagle");
        assert_eq!(
            verify_token(Some(&tok.encode()), "destroy", "eagle", 0, &s),
            TokenVerdict::Mismatch
        );
        // Mismatch — right key, wrong node.
        assert_eq!(
            verify_token(Some(&tok.encode()), "provision", "otter", 0, &s),
            TokenVerdict::Mismatch
        );
        // Expired.
        assert_eq!(
            verify_token(Some(&tok.encode()), "provision", "eagle", 20_000, &s),
            TokenVerdict::Expired
        );
        // BadSignature — a token minted by a different key.
        let other = HmacTokenSigner::new(b"a-different-key".to_vec());
        let forged = ArmedToken::mint(&other, "nonce-abcdef", 10_000, "provision", "eagle");
        assert_eq!(
            verify_token(Some(&forged.encode()), "provision", "eagle", 5_000, &s),
            TokenVerdict::BadSignature
        );
    }

    #[test]
    fn the_null_signer_never_validates_any_token() {
        // A node with no arming key stages every mutation: even a well-formed token
        // minted by a real key fails the NullSigner's verification.
        let real = signer();
        let tok = ArmedToken::mint(&real, "nonce-abcdef", 10_000, "provision", "eagle");
        assert_eq!(
            verify_token(Some(&tok.encode()), "provision", "eagle", 0, &NullSigner),
            TokenVerdict::BadSignature
        );
    }

    #[test]
    fn decide_serves_reads_and_gates_mutations_on_the_token() {
        assert_eq!(decide(CloudVerb::List, false), CloudDecision::Read);
        assert_eq!(decide(CloudVerb::Status, true), CloudDecision::Read);
        assert_eq!(decide(CloudVerb::Provision, true), CloudDecision::Apply);
        assert_eq!(decide(CloudVerb::Provision, false), CloudDecision::Staged);
        assert_eq!(decide(CloudVerb::Destroy, false), CloudDecision::Staged);
    }

    #[test]
    fn placement_routes_local_versus_remote() {
        assert_eq!(placement_match("eagle", "eagle"), Placement::Local);
        // An empty (node-agnostic / legacy) request is local.
        assert_eq!(placement_match("", "eagle"), Placement::Local);
        assert_eq!(placement_match("   ", "eagle"), Placement::Local);
        assert_eq!(
            placement_match("otter", "eagle"),
            Placement::Remote("otter".to_string())
        );
    }
}

//! OW-4 — `mackesd onboard invite-issue`: mint a short-TTL, mesh-scoped join
//! invite.
//!
//! The shape mirrors [`crate::onboard::self_test`]: a small **pure core** (the
//! invite envelope, its two encodings, and the redemption checks) that the unit
//! tests pin, plus a thin **impure shell** ([`issue`] / [`resolve_mesh_id`]) that
//! owns the clock, the CSPRNG, and the bearer-ledger write.
//!
//! # No new crypto (§6) — the ledger record *is* the authentication
//! The invite is an authenticated **bearer token**, not a fresh signature scheme:
//! a 256-bit CSPRNG secret drawn exactly as [`crate::bearer_ledger::issue`] mints
//! its bearers, recorded in the same ledger so it can be verified as *pending* and
//! revoked *single-use*. There is no separate signing primitive to reuse here —
//! `nebula_admin` is the debug-SSH path probe (not a CA signer) and `enrollment`
//! builds node identities (not tokens) — so the bearer-ledger record carries the
//! authenticity, the same way every other join token in this daemon does.
//!
//! # What binds the invite to THIS mesh
//! The code is a self-describing envelope `{v, mesh_id, exp_ms, secret}`. The pure
//! [`verify`] rejects a **foreign-`mesh_id`** and an **expired `exp_ms`** offline
//! (no I/O). The ledger keys on the *whole* canonical payload (not the bare
//! `secret`), so tampering with the `mesh_id` or `exp_ms` changes the recorded
//! hash and the code no longer verifies as pending — the offline policy check and
//! the ledger capability check agree.
//!
//! # Follow-up (OW-5, deliberately NOT built here, §7)
//! The joiner/redemption half — present code -> CSR -> signed bundle -> overlay IP
//! — reuses `nebula_enroll_client`; it pairs the pure [`verify`] with the impure
//! [`is_recorded`] / [`revoke`] this module exposes. That is OW-4's completing
//! slice, tracked separately.

use std::fmt;
use std::io;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::nebula_enroll::JoinToken;

/// Envelope version. [`Invite::decode`] rejects any other value, so a future wire
/// change is a clean reject rather than a silent misparse.
pub const INVITE_V: u8 = 1;

/// Default invite TTL, in **minutes** — short by design (a join code is presented
/// promptly). The CLI `--ttl` overrides it.
pub const DEFAULT_TTL_MINUTES: u64 = 15;

/// Prefix on the typeable short code. Distinguishes the code form from the QR form
/// while both wrap the identical canonical payload.
const CODE_PREFIX: &str = "MDEINV1-";

/// URI scheme on the QR-encodable form (what a phone camera / joiner scans).
const QR_SCHEME: &str = "mde-invite:";

/// The self-describing invite envelope — the payload BOTH encodings carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    /// Envelope version ([`INVITE_V`]).
    pub v: u8,
    /// The mesh this invite is scoped to. [`verify`] rejects a redeeming mesh
    /// whose id differs (a foreign mesh's code can never join here).
    pub mesh_id: String,
    /// Absolute expiry, Unix-epoch milliseconds. [`verify`] rejects `now >= exp`.
    pub exp_ms: u64,
    /// The 256-bit CSPRNG bearer `secret` (URL-safe base64) — the unguessable
    /// capability, recorded (as part of the canonical payload) in the bearer
    /// ledger.
    pub secret: String,
}

impl Invite {
    /// The canonical, prefix-less payload both encodings wrap: URL-safe-no-pad
    /// base64 of the compact JSON. `serde_json` serialises the struct in field
    /// declaration order, so this is deterministic — the code form and the QR form
    /// embed byte-identical bytes, and re-encoding a decoded invite reproduces the
    /// same string (the property [`strip_wrapper`] relies on to key the ledger).
    #[must_use]
    fn canonical(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }

    /// The typeable short code (URL-safe alphabet, [`CODE_PREFIX`]-tagged).
    #[must_use]
    pub fn to_code(&self) -> String {
        format!("{CODE_PREFIX}{}", self.canonical())
    }

    /// The QR-encodable string — the same payload as [`Invite::to_code`], wrapped
    /// as a `mde-invite:` URI so a scanner / joiner recognises it.
    #[must_use]
    pub fn to_qr(&self) -> String {
        format!("{QR_SCHEME}{}", self.canonical())
    }

    /// Decode either encoding (code or QR, or a bare canonical payload) back into
    /// an [`Invite`]. Returns `None` on a bad base64 / JSON body or an unknown
    /// envelope version — every malformed input is a clean reject, never a panic.
    #[must_use]
    pub fn decode(s: &str) -> Option<Self> {
        let payload = strip_wrapper(s);
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?;
        let invite: Self = serde_json::from_slice(&bytes).ok()?;
        (invite.v == INVITE_V).then_some(invite)
    }
}

/// Strip a known wrapper (the [`CODE_PREFIX`] or the [`QR_SCHEME`]) to the shared
/// canonical payload. Both encodings normalise to the same bytes, so a code and
/// its QR twin resolve to one ledger key.
#[must_use]
fn strip_wrapper(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix(CODE_PREFIX)
        .or_else(|| s.strip_prefix(QR_SCHEME))
        .unwrap_or(s)
}

/// The outcome of a pure redemption check ([`verify`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Well-formed, right mesh, not yet expired — the code may be redeemed
    /// (subject to the impure ledger check, [`is_recorded`]).
    Valid,
    /// The code did not decode (bad encoding / unknown version).
    Malformed,
    /// Scoped to a different mesh than the redeeming one.
    ForeignMesh {
        /// The redeeming mesh's id.
        expected: String,
        /// The mesh id the code carries.
        found: String,
    },
    /// Past its expiry at the checking clock.
    Expired {
        /// The code's absolute expiry (epoch ms).
        exp_ms: u64,
        /// The checking clock (epoch ms).
        now_ms: u64,
    },
}

impl VerifyOutcome {
    /// `true` only for [`VerifyOutcome::Valid`].
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// A stable machine tag for logs / headless output.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Malformed => "malformed",
            Self::ForeignMesh { .. } => "foreign-mesh",
            Self::Expired { .. } => "expired",
        }
    }
}

/// Pure redemption check: decode `code` and reject a **foreign-mesh** or
/// **expired** invite against the injected `now_ms` + `mesh_id`.
///
/// No I/O, no clock — the redemption path (OW-5) pairs this with the impure
/// [`is_recorded`] ledger check. The expiry boundary is exclusive: a code is
/// [`VerifyOutcome::Valid`] strictly before `exp_ms`, and [`VerifyOutcome::Expired`]
/// at exactly `exp_ms`.
#[must_use]
pub fn verify(code: &str, now_ms: u64, mesh_id: &str) -> VerifyOutcome {
    let Some(invite) = Invite::decode(code) else {
        return VerifyOutcome::Malformed;
    };
    if invite.mesh_id != mesh_id {
        return VerifyOutcome::ForeignMesh {
            expected: mesh_id.to_string(),
            found: invite.mesh_id,
        };
    }
    if now_ms >= invite.exp_ms {
        return VerifyOutcome::Expired {
            exp_ms: invite.exp_ms,
            now_ms,
        };
    }
    VerifyOutcome::Valid
}

/// Absolute expiry for an invite minted at `now_ms` with lifetime `ttl`
/// (saturating — an absurd TTL clamps to [`u64::MAX`] rather than wrapping).
#[must_use]
pub fn expiry_ms(now_ms: u64, ttl: Duration) -> u64 {
    let ttl_ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
    now_ms.saturating_add(ttl_ms)
}

/// Pure ledger-recording decision: record an invite iff it is well-formed and
/// live at issue time.
///
/// We never persist a mesh-less or dead-on-arrival (already-expired) code, so the
/// ledger only ever holds redeemable invites.
#[must_use]
pub fn should_record(invite: &Invite, now_ms: u64) -> bool {
    invite.v == INVITE_V
        && !invite.mesh_id.is_empty()
        && !invite.secret.is_empty()
        && invite.exp_ms > now_ms
}

/// A freshly-minted invite plus both encodings the front-ends print / show.
#[derive(Debug, Clone)]
pub struct IssuedInvite {
    /// The envelope.
    pub invite: Invite,
    /// The typeable short code ([`Invite::to_code`]).
    pub code: String,
    /// The QR-encodable string ([`Invite::to_qr`]).
    pub qr: String,
    /// Whether [`issue`] recorded this invite in the bearer ledger (the
    /// [`should_record`] decision) — `false` only for a degenerate zero TTL.
    pub recorded: bool,
}

/// Draw a 256-bit CSPRNG secret, URL-safe-no-pad base64 — the SEC-3 strength
/// [`crate::bearer_ledger::issue`] uses for its bearers.
#[must_use]
fn mint_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Wall clock as Unix-epoch milliseconds (mirrors [`crate::bearer_ledger`]'s
/// timestamp). A clock before the epoch reads as `0`.
#[must_use]
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Impure shell — mint a short-TTL, mesh-scoped invite, record it in the bearer
/// ledger (so it can be verified as pending + revoked single-use), and return
/// both encodings.
///
/// Reuses the token infra end-to-end: the secret is a 256-bit CSPRNG bearer (as
/// [`crate::bearer_ledger::issue`] mints), and the canonical payload is recorded
/// via [`crate::bearer_ledger::record_issued`] — the (Syncthing-replicated)
/// ledger stores only its SHA-256, and [`crate::bearer_ledger::redeem`] (via
/// [`revoke`]) consumes it once.
///
/// # Errors
/// Propagates an IO failure writing the ledger entry.
pub fn issue(workgroup_root: &Path, mesh_id: &str, ttl: Duration) -> io::Result<IssuedInvite> {
    let now_ms = now_unix_ms();
    let invite = Invite {
        v: INVITE_V,
        mesh_id: mesh_id.to_string(),
        exp_ms: expiry_ms(now_ms, ttl),
        secret: mint_secret(),
    };
    let recorded = should_record(&invite, now_ms);
    if recorded {
        crate::bearer_ledger::record_issued(workgroup_root, &invite.canonical())?;
    }
    let code = invite.to_code();
    let qr = invite.to_qr();
    Ok(IssuedInvite {
        invite,
        code,
        qr,
        recorded,
    })
}

/// Resolve THIS node's mesh-id.
///
/// The authoritative source is the local founding bundle's `mesh_id` (the same
/// one `mackesd ca add-peer` reads); an un-founded box falls back to the
/// `mesh-<node>` default the `mackesd ca` verbs use.
#[must_use]
pub fn resolve_mesh_id(workgroup_root: &Path, node_id: &str) -> String {
    crate::ca::bundle::read_bundle(&crate::ca::bundle::bundle_path(workgroup_root, node_id))
        .map_or_else(|_| format!("mesh-{node_id}"), |bundle| bundle.mesh_id)
}

/// Impure counterpart to [`verify`]: is `presented` (a code or its QR twin) a
/// recorded, not-yet-revoked invite in `workgroup_root`'s bearer ledger?
///
/// Thin reuse of [`crate::bearer_ledger::is_pending`]; the redemption path (OW-5)
/// pairs it with the pure mesh + TTL check.
#[must_use]
pub fn is_recorded(workgroup_root: &Path, presented: &str) -> bool {
    crate::bearer_ledger::is_pending(workgroup_root, strip_wrapper(presented))
}

/// Revoke / consume `presented` single-use — the redemption sign or an operator
/// revoke. Returns `true` exactly once per issued invite. Thin reuse of
/// [`crate::bearer_ledger::redeem`].
#[must_use]
pub fn revoke(workgroup_root: &Path, presented: &str) -> bool {
    crate::bearer_ledger::redeem(workgroup_root, strip_wrapper(presented))
}

// ─────────────────────────────────────────────────────────────────
// OW-4 (completing slice) — redeem an MDEINV1 code on the JOIN side.
//
// The wizard mints an `MDEINV1-…` code here; the join/enroll path
// ([`crate::nebula_enroll`]) speaks the v3 `JoinToken`
// (`mesh:<id>@<ip>:<port>#<bearer>`). This bridge validates a presented
// code (offline mesh + TTL via [`verify`], then the ledger capability via
// [`is_recorded`]) and maps it onto that same [`JoinToken`], so the existing
// CSR→bundle machinery ([`crate::nebula_enroll::build_pending`] →
// `sign_csr_into_bundle`) consumes it UNCHANGED — no new enrollment surface.
// ─────────────────────────────────────────────────────────────────

/// Does `s` present as an MDEINV1 wizard invite (typeable code or QR twin)?
///
/// Lets the join entrypoint route an invite to [`redeem`] instead of the v3
/// [`crate::nebula_enroll::parse_join_token`] parser (which would reject it).
#[must_use]
pub fn looks_like_invite(s: &str) -> bool {
    let s = s.trim();
    s.starts_with(CODE_PREFIX) || s.starts_with(QR_SCHEME)
}

/// The lighthouse `/enroll` endpoint a redeemed invite must reach.
///
/// The MDEINV1
/// envelope binds only `{mesh, TTL, bearer}` — it deliberately carries **no**
/// network endpoint (a code is presented over many transports and stays short +
/// QR-friendly), so the redeemer supplies the endpoint out-of-band. This is the
/// one input the invite cannot provide; where it is genuinely unavailable the
/// live enroll leg is gated (a code alone cannot contact a lighthouse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollEndpoint {
    /// Lighthouse IPv4 (overlay or public) hosting `/enroll`.
    pub lighthouse: String,
    /// Lighthouse port.
    pub port: u16,
    /// Optional SHA-256 (lowercase hex) of the `/enroll` TLS cert — pins the
    /// network-enroll client (no trust-on-first-use). `None` → the co-located
    /// QNM-Shared flow, exactly as a v3 token with no `?fp=`.
    pub fp: Option<String>,
}

/// Why a presented MDEINV1 code was refused for redemption. Mirrors
/// [`VerifyOutcome`] (offline) plus the ledger-capability failure, as a typed
/// error the CLI/wizard surface — never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedeemError {
    /// The code did not decode (bad encoding / unknown envelope version).
    Malformed,
    /// Scoped to a different mesh than the redeeming one.
    ForeignMesh {
        /// The redeeming mesh's id.
        expected: String,
        /// The mesh id the code carries.
        found: String,
    },
    /// Past its expiry at the redeeming clock.
    Expired {
        /// The code's absolute expiry (epoch ms).
        exp_ms: u64,
        /// The redeeming clock (epoch ms).
        now_ms: u64,
    },
    /// Decoded + offline-valid, but the bearer ledger has no such *pending*
    /// invite: a **tampered** field (the ledger keys on the whole canonical
    /// payload, so any edit breaks the binding), a foreign/unsynced ledger, or
    /// an already-redeemed (single-use) code.
    NotIssued,
}

impl RedeemError {
    /// A stable machine tag for logs / headless output.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::ForeignMesh { .. } => "foreign-mesh",
            Self::Expired { .. } => "expired",
            Self::NotIssued => "not-issued",
        }
    }
}

impl fmt::Display for RedeemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => write!(
                f,
                "not a valid invite code — expected an `{CODE_PREFIX}…` code (or its \
                 `{QR_SCHEME}…` QR twin) from `mackesd onboard invite-issue`.",
            ),
            Self::ForeignMesh { expected, found } => write!(
                f,
                "this invite is scoped to mesh `{found}`, but this node is joining \
                 `{expected}` — an invite can only join the mesh it was minted for.",
            ),
            Self::Expired { exp_ms, now_ms } => write!(
                f,
                "invite expired ({now_ms} ≥ {exp_ms}, epoch ms) — join codes are \
                 short-lived by design; mint a fresh one with \
                 `mackesd onboard invite-issue` and present it promptly.",
            ),
            Self::NotIssued => write!(
                f,
                "invite is not an issued-and-unredeemed bearer in the ledger — it was \
                 tampered with, already redeemed (single-use), or minted on a mesh \
                 whose ledger this node has not synced. Mint a fresh invite and retry.",
            ),
        }
    }
}

impl std::error::Error for RedeemError {}

impl From<VerifyOutcome> for RedeemError {
    /// Map the offline [`verify`] outcome onto the redemption error. `Valid`
    /// has no error form, so it maps to [`RedeemError::NotIssued`] defensively
    /// (callers branch on `Valid` before converting).
    fn from(outcome: VerifyOutcome) -> Self {
        match outcome {
            VerifyOutcome::Malformed => Self::Malformed,
            VerifyOutcome::ForeignMesh { expected, found } => Self::ForeignMesh { expected, found },
            VerifyOutcome::Expired { exp_ms, now_ms } => Self::Expired { exp_ms, now_ms },
            VerifyOutcome::Valid => Self::NotIssued,
        }
    }
}

/// Pure mapping — a validated [`Invite`] + an out-of-band [`EnrollEndpoint`] →
/// the v3 [`JoinToken`] the existing enroll/CSR path consumes unchanged.
///
/// The `bearer` is the invite's **canonical payload** (what
/// [`crate::bearer_ledger::record_issued`] stored at mint time), NOT the bare
/// `secret` — so the lighthouse's `is_pending` check in `sign_csr_into_bundle`
/// matches the recorded key, and the redeemed CSR carries the identical
/// enrollment inputs a v3 join would. The `mesh_id` is the invite's own scope
/// (authoritative — the lighthouse signs under the mesh the token declares).
#[must_use]
pub fn to_join_token(invite: &Invite, endpoint: &EnrollEndpoint) -> JoinToken {
    JoinToken {
        mesh_id: invite.mesh_id.clone(),
        lighthouse: endpoint.lighthouse.clone(),
        port: endpoint.port,
        bearer: invite.canonical(),
        fp: endpoint.fp.clone(),
    }
}

/// Validate a presented invite `code` end-to-end for redemption.
///
/// Decode + the offline mesh/TTL check ([`verify`]) THEN the ledger capability
/// check ([`is_recorded`]). Returns the decoded [`Invite`], ready to map to
/// enroll inputs via [`to_join_token`].
///
/// This is the completing half of the OW-4 loop: it pairs the pure [`verify`]
/// with the impure [`is_recorded`] exactly as the module's design note
/// promised. Expired / foreign-mesh / tampered / already-redeemed codes are
/// refused with a typed [`RedeemError`], never a panic.
///
/// # Errors
/// Per [`RedeemError`].
pub fn validate_for_redeem(
    workgroup_root: &Path,
    code: &str,
    now_ms: u64,
    mesh_id: &str,
) -> Result<Invite, RedeemError> {
    // Offline gate first: mesh-scope + TTL, no I/O.
    match verify(code, now_ms, mesh_id) {
        VerifyOutcome::Valid => {}
        other => return Err(other.into()),
    }
    // Ledger gate: the presented code must be a recorded, not-yet-redeemed
    // invite. Tampering with any field changes the canonical key, so a forged
    // code is offline-Valid but never *pending* — this is what refuses it.
    if !is_recorded(workgroup_root, code) {
        return Err(RedeemError::NotIssued);
    }
    // Safe: `verify` already proved it decodes.
    Invite::decode(code).ok_or(RedeemError::Malformed)
}

/// Redeem an MDEINV1 invite `code` into the v3 [`JoinToken`] the enroll path
/// consumes: [`validate_for_redeem`] then [`to_join_token`]. The one-call
/// redemption entrypoint.
///
/// # Errors
/// Per [`RedeemError`] — refuses expired / foreign-mesh / tampered /
/// already-redeemed codes before any enrollment work.
pub fn redeem(
    workgroup_root: &Path,
    code: &str,
    now_ms: u64,
    mesh_id: &str,
    endpoint: &EnrollEndpoint,
) -> Result<JoinToken, RedeemError> {
    let invite = validate_for_redeem(workgroup_root, code, now_ms, mesh_id)?;
    Ok(to_join_token(&invite, endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic sample invite (fixed secret so encode/decode round-trips
    /// are pinnable). The impure tests below draw a real CSPRNG secret via [`issue`].
    fn sample(mesh: &str, exp_ms: u64) -> Invite {
        Invite {
            v: INVITE_V,
            mesh_id: mesh.to_string(),
            exp_ms,
            secret: "c2VjcmV0LWJlYXJlci0yNTYtYml0LXRlc3QtdmVjdG9yLXg".to_string(),
        }
    }

    #[test]
    fn code_and_qr_decode_back_to_the_same_invite() {
        let inv = sample("home-mesh", 1_700_000_000_000);
        let code = inv.to_code();
        let qr = inv.to_qr();
        assert!(code.starts_with(CODE_PREFIX));
        assert!(qr.starts_with(QR_SCHEME));
        // Same payload, two encodings: both wrap byte-identical canonical bytes.
        assert_eq!(code.strip_prefix(CODE_PREFIX), qr.strip_prefix(QR_SCHEME));
        assert_eq!(Invite::decode(&code), Some(inv.clone()));
        assert_eq!(Invite::decode(&qr), Some(inv.clone()));
        // A bare canonical payload (no wrapper) also decodes.
        assert_eq!(Invite::decode(inv.canonical().as_str()), Some(inv));
    }

    #[test]
    fn decode_rejects_garbage_and_foreign_versions() {
        assert_eq!(Invite::decode("not base64 !!"), None);
        assert_eq!(Invite::decode("MDEINV1-not base64 !!"), None);
        assert_eq!(Invite::decode(""), None);
        // A well-formed body carrying the wrong envelope version is rejected.
        let mut inv = sample("m", 10);
        inv.v = 9;
        let json = serde_json::to_vec(&inv).unwrap();
        let wrong = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        assert_eq!(Invite::decode(&format!("{CODE_PREFIX}{wrong}")), None);
    }

    #[test]
    fn verify_accepts_a_live_same_mesh_code() {
        let inv = sample("home-mesh", 2_000);
        assert_eq!(
            verify(&inv.to_code(), 1_999, "home-mesh"),
            VerifyOutcome::Valid
        );
        assert!(verify(&inv.to_qr(), 0, "home-mesh").is_valid());
    }

    #[test]
    fn verify_expiry_boundary_is_exclusive() {
        let inv = sample("home-mesh", 5_000);
        // Strictly before -> valid; at exactly exp -> expired; after -> expired.
        assert!(verify(&inv.to_code(), 4_999, "home-mesh").is_valid());
        assert_eq!(
            verify(&inv.to_code(), 5_000, "home-mesh"),
            VerifyOutcome::Expired {
                exp_ms: 5_000,
                now_ms: 5_000
            }
        );
        assert_eq!(
            verify(&inv.to_code(), 5_001, "home-mesh").reason(),
            "expired"
        );
    }

    #[test]
    fn verify_rejects_a_foreign_mesh_even_when_live() {
        let inv = sample("home-mesh", u64::MAX);
        let out = verify(&inv.to_code(), 0, "office-mesh");
        assert_eq!(
            out,
            VerifyOutcome::ForeignMesh {
                expected: "office-mesh".to_string(),
                found: "home-mesh".to_string(),
            }
        );
        assert!(!out.is_valid());
        assert_eq!(out.reason(), "foreign-mesh");
    }

    #[test]
    fn verify_reports_malformed_for_undecodable_codes() {
        assert_eq!(verify("garbage", 0, "home-mesh"), VerifyOutcome::Malformed);
        assert_eq!(verify("garbage", 0, "home-mesh").reason(), "malformed");
    }

    #[test]
    fn expiry_ms_is_now_plus_ttl_and_saturates() {
        assert_eq!(expiry_ms(1_000, Duration::from_secs(60)), 1_000 + 60_000);
        assert_eq!(expiry_ms(u64::MAX - 5, Duration::from_secs(60)), u64::MAX);
    }

    #[test]
    fn should_record_only_live_well_formed_invites() {
        let now = 1_000;
        assert!(should_record(&sample("m", now + 1), now), "live -> record");
        assert!(
            !should_record(&sample("m", now), now),
            "expiry == now -> dead on arrival"
        );
        assert!(
            !should_record(&sample("m", now - 1), now),
            "already expired -> skip"
        );
        assert!(
            !should_record(&sample("", now + 1), now),
            "mesh-less -> skip"
        );
        let mut no_secret = sample("m", now + 1);
        no_secret.secret = String::new();
        assert!(!should_record(&no_secret, now), "secret-less -> skip");
    }

    #[test]
    fn issue_records_the_canonical_payload_not_the_bare_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let issued = issue(tmp.path(), "home-mesh", Duration::from_secs(600)).unwrap();
        assert!(issued.recorded);
        assert_eq!(issued.invite.mesh_id, "home-mesh");
        // 32 bytes URL-safe-no-pad base64 = 43 chars (the SEC-3 256-bit strength).
        assert_eq!(issued.invite.secret.len(), 43);
        // Recorded under the canonical payload — presentable as EITHER encoding.
        assert!(is_recorded(tmp.path(), &issued.code));
        assert!(
            is_recorded(tmp.path(), &issued.qr),
            "the QR twin maps to the same ledger key"
        );
        // ...but NOT under the bare secret: the ledger binds mesh-id + expiry too.
        assert!(!crate::bearer_ledger::is_pending(
            tmp.path(),
            &issued.invite.secret
        ));
    }

    #[test]
    fn revoke_is_single_use_across_both_encodings() {
        let tmp = tempfile::tempdir().unwrap();
        let issued = issue(tmp.path(), "home-mesh", Duration::from_secs(600)).unwrap();
        assert!(is_recorded(tmp.path(), &issued.code));
        assert!(
            revoke(tmp.path(), &issued.qr),
            "first revoke consumes it (presented as the QR form)"
        );
        assert!(!is_recorded(tmp.path(), &issued.code), "spent");
        assert!(
            !revoke(tmp.path(), &issued.code),
            "replay refused (single-use)"
        );
    }

    #[test]
    fn tampering_with_the_expiry_breaks_the_ledger_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let issued = issue(tmp.path(), "home-mesh", Duration::from_secs(1)).unwrap();
        // Forge a far-future expiry: the *pure* verify is fooled by the plaintext
        // field, but the forged code was never recorded, so the ledger rejects it.
        let mut forged = issued.invite;
        forged.exp_ms = u64::MAX;
        let forged_code = forged.to_code();
        assert!(
            verify(&forged_code, u64::MAX - 1, "home-mesh").is_valid(),
            "the offline check trusts the plaintext expiry"
        );
        assert!(
            !is_recorded(tmp.path(), &forged_code),
            "but the ledger binds the whole payload — the forgery is not pending"
        );
    }

    #[test]
    fn resolve_mesh_id_falls_back_to_the_node_default_without_a_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_mesh_id(tmp.path(), "peer:anvil"),
            "mesh-peer:anvil",
            "no founding bundle -> the `mesh-<node>` default the CA verbs use"
        );
    }

    // ---- OW-4 redemption bridge (MDEINV1 -> v3 JoinToken) ----

    fn endpoint() -> EnrollEndpoint {
        EnrollEndpoint {
            lighthouse: "10.0.0.5".to_string(),
            port: 4242,
            fp: Some("a".repeat(64)),
        }
    }

    #[test]
    fn looks_like_invite_matches_both_encodings_only() {
        let inv = sample("home-mesh", 9_999);
        assert!(looks_like_invite(&inv.to_code()));
        assert!(looks_like_invite(&inv.to_qr()));
        assert!(
            looks_like_invite(&format!("  {}  ", inv.to_code())),
            "trims"
        );
        // A v3 join token is NOT an invite — the join path routes it elsewhere.
        assert!(!looks_like_invite("mesh:home-mesh@10.0.0.5:4242#bearer"));
        assert!(!looks_like_invite("garbage"));
    }

    #[test]
    fn redeem_accepts_a_valid_recorded_invite_into_enroll_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let issued = issue(tmp.path(), "home-mesh", Duration::from_secs(600)).unwrap();
        assert!(issued.recorded);
        let token = redeem(
            tmp.path(),
            &issued.code,
            issued.invite.exp_ms - 1,
            "home-mesh",
            &endpoint(),
        )
        .expect("a live, same-mesh, ledger-recorded invite redeems");
        assert_eq!(token.mesh_id, "home-mesh");
        assert_eq!(token.lighthouse, "10.0.0.5");
        assert_eq!(token.port, 4242);
        // The bearer is the CANONICAL payload the ledger recorded — so the
        // lighthouse's `is_pending` gate in `sign_csr_into_bundle` matches the
        // redeemed CSR exactly as it would a v3 join. This is the load-bearing
        // invariant that makes the CSR->bundle inputs identical to the v3 path.
        assert_eq!(token.bearer, issued.invite.canonical());
        assert!(
            crate::bearer_ledger::is_pending(tmp.path(), &token.bearer),
            "the mapped bearer is the pending ledger key the lighthouse signs against"
        );
        // The QR twin redeems to the identical token (one ledger key, two forms).
        let via_qr = redeem(
            tmp.path(),
            &issued.qr,
            issued.invite.exp_ms - 1,
            "home-mesh",
            &endpoint(),
        )
        .expect("QR twin redeems too");
        assert_eq!(via_qr, token);
    }

    #[test]
    fn redeem_refuses_an_expired_code() {
        let tmp = tempfile::tempdir().unwrap();
        let inv = sample("home-mesh", 5_000);
        // At exactly exp the offline gate rejects before any ledger I/O.
        let err = validate_for_redeem(tmp.path(), &inv.to_code(), 5_000, "home-mesh").unwrap_err();
        assert_eq!(
            err,
            RedeemError::Expired {
                exp_ms: 5_000,
                now_ms: 5_000
            }
        );
        assert_eq!(err.reason(), "expired");
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn redeem_refuses_a_foreign_mesh_code() {
        let tmp = tempfile::tempdir().unwrap();
        let inv = sample("home-mesh", u64::MAX);
        let err = validate_for_redeem(tmp.path(), &inv.to_code(), 0, "office-mesh").unwrap_err();
        assert_eq!(
            err,
            RedeemError::ForeignMesh {
                expected: "office-mesh".to_string(),
                found: "home-mesh".to_string(),
            }
        );
        assert_eq!(err.reason(), "foreign-mesh");
    }

    #[test]
    fn redeem_refuses_a_tampered_code_via_the_ledger_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let issued = issue(tmp.path(), "home-mesh", Duration::from_secs(1)).unwrap();
        // Forge a far-future expiry: the OFFLINE verify is fooled by the
        // plaintext field, but the forged canonical payload was never recorded,
        // so the ledger gate refuses it — a tampered code never mints a token.
        let mut forged = issued.invite;
        forged.exp_ms = u64::MAX;
        let forged_code = forged.to_code();
        assert!(
            verify(&forged_code, u64::MAX - 1, "home-mesh").is_valid(),
            "sanity: the offline check trusts the plaintext expiry"
        );
        let err =
            validate_for_redeem(tmp.path(), &forged_code, u64::MAX - 1, "home-mesh").unwrap_err();
        assert_eq!(err, RedeemError::NotIssued);
        assert_eq!(err.reason(), "not-issued");
        assert!(
            redeem(
                tmp.path(),
                &forged_code,
                u64::MAX - 1,
                "home-mesh",
                &endpoint()
            )
            .is_err(),
            "the one-call entrypoint refuses the forgery too — no JoinToken minted"
        );
    }

    #[test]
    fn to_join_token_maps_mdeinv1_onto_the_v3_enroll_inputs() {
        let inv = sample("home-mesh", 9_999);
        // fp=None models the co-located QNM-Shared flow (a v3 token with no ?fp=).
        let ep = EnrollEndpoint {
            lighthouse: "10.9.9.9".to_string(),
            port: 4242,
            fp: None,
        };
        let token = to_join_token(&inv, &ep);
        assert_eq!(token.mesh_id, "home-mesh");
        assert_eq!(token.port, 4242);
        assert_eq!(token.fp, None);
        // The bearer is the canonical payload, NEVER the bare secret — the
        // ledger keys on the whole envelope so mesh-id + expiry are bound in.
        assert_eq!(token.bearer, inv.canonical());
        assert_ne!(token.bearer, inv.secret);
        // It round-trips to the exact v3 wire shape the enroll path speaks.
        assert!(token.encode().starts_with("mesh:home-mesh@10.9.9.9:4242#"));
    }
}

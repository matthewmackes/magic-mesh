//! Workloads U2 — the two load-bearing gates of the `cloud` worker.
//!
//! This module owns the pure, I/O-free gate logic the drain + verb dispatch
//! consult:
//!
//! 1. **The armed-token gate** ([`verify_token`] / [`decide`]) — replaces the
//!    retired `MDE_CLOUD_APPLY=1` env wall. A live mutation is authorized by an
//!    **armed token** carrying a CSPRNG nonce + expiry, bound to the exact verb +
//!    placement node + mutation target + canonical request body it authorizes. The
//!    shipped root shell mints after typed confirmation; every provisioned daemon
//!    verifies with the same root-only, sealed HMAC credential. No Bus verb
//!    exposes mint authority.
//!
//! 2. **The placement gate** ([`placement_match`]) — replaces the leader gate. A
//!    mutation is performed by exactly the node it is placed on (`body.node ==
//!    self.host`); a mutation for another node is that node's to perform, and a
//!    mutation for an *unreachable* node is honestly gated (never a silent swallow).
//!    Reads are not placement-scoped — they stay local on every node.

pub use mackes_mesh_types::cloud::CloudArmedToken as ArmedToken;
use mackes_mesh_types::cloud::{
    cloud_request_digest, decode_cloud_arm_credential, CloudArmSigner, CloudTokenSigner,
    CLOUD_ARM_CREDENTIAL,
};

/// The minimum nonce length a well-formed armed token carries (a short/absent
/// nonce is a malformed capability, never accepted).
pub(crate) const TOKEN_NONCE_MIN_LEN: usize = 8;

/// Host-local replay root. It is deliberately outside the Syncthing-replicated
/// workgroup root; only the sealed credential is distributed mesh-wide.
pub(crate) const DEFAULT_AUTH_ROOT: &str = "/var/lib/mackesd/cloud-auth";

// ─────────────────────────── the signing seam ───────────────────────────

/// The token signing/verification seam.
pub trait TokenSigner: Send + Sync {
    /// The signature this signer produces over `payload` (and accepts on verify).
    fn sign_payload(&self, payload: &str) -> String;

    /// Verify a wire signature without an early-exit string comparison.
    fn verify_payload(&self, payload: &str, signature: &str) -> bool;
}

/// HMAC-SHA256 verifier loaded only from a systemd service credential.
pub(crate) struct HmacTokenSigner {
    signer: CloudArmSigner,
}

impl HmacTokenSigner {
    /// Construct from raw key bytes.
    #[must_use]
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self {
            signer: CloudArmSigner::new(key).expect("test cloud arming key is non-empty"),
        }
    }

    /// Load the root-only credential injected by systemd. There is deliberately
    /// no env-secret fallback and no key generation in the daemon.
    pub fn from_systemd_credential() -> Result<Self, String> {
        if !rustix::process::geteuid().is_root() {
            return Err("cloud authorization requires a root service process".to_string());
        }
        let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| "systemd cloud arming credential is unavailable".to_string())?;
        let path = directory.join(CLOUD_ARM_CREDENTIAL);
        let raw = std::fs::read(&path)
            .map_err(|e| format!("read systemd credential {}: {e}", path.display()))?;
        let key = decode_cloud_arm_credential(&raw).map_err(str::to_string)?;
        Ok(Self::new(key))
    }
}

impl TokenSigner for HmacTokenSigner {
    fn sign_payload(&self, payload: &str) -> String {
        self.signer.sign_payload(payload)
    }
    fn verify_payload(&self, payload: &str, signature: &str) -> bool {
        self.signer.verify_payload(payload, signature)
    }
}

impl CloudTokenSigner for HmacTokenSigner {
    fn sign_payload(&self, payload: &str) -> String {
        self.signer.sign_payload(payload)
    }
}

/// The no-key signer a node without a mesh arming key uses: it produces a
/// signature no client could reproduce, so every presented token fails the
/// signature check and every mutation fails closed (the "arming unavailable"
/// capability state). Never validates a token.
pub(crate) struct NullSigner;

impl TokenSigner for NullSigner {
    fn sign_payload(&self, _payload: &str) -> String {
        // A sentinel that no real token's `signature` field ever equals.
        "\u{0}arming-unavailable\u{0}".to_string()
    }
    fn verify_payload(&self, _payload: &str, _signature: &str) -> bool {
        false
    }
}

/// Stable, path-safe digest for a capability nonce. The durable replay ledger
/// stores only this digest, never an attacker-controlled filename or the nonce
/// itself.
#[must_use]
pub(crate) fn nonce_digest(nonce: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::digest(nonce.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// Atomically consume one capability nonce in the shared host-local replay
/// ledger. The row is a `0600` file named by [`nonce_digest`] whose contents are
/// the capability expiry. `create_new` is the cross-thread/process compare and
/// set; syncing both the file and containing directory makes a successful claim
/// survive a daemon restart or power loss.
///
/// Expired, well-formed rows are removed before the claim. Unknown files,
/// directories, symlinks, and malformed rows are never followed or deleted.
///
/// # Errors
///
/// Returns an error if the replay directory cannot be secured, an expired row
/// cannot be cleaned, or the new claim cannot be written and durably synced.
pub(crate) fn claim_nonce(
    root: &std::path::Path,
    nonce: &str,
    expires_at_ms: i64,
    now_ms: i64,
) -> Result<bool, String> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    fn sync_directory(dir: &std::path::Path) -> Result<(), String> {
        std::fs::File::open(dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync armed-token replay store {}: {error}", dir.display()))
    }

    fn is_nonce_row(name: &str) -> bool {
        name.len() == 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    let dir = root.join("spent-nonces");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("create armed-token replay store {}: {error}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("secure armed-token replay store {}: {error}", dir.display()))?;

    // Tokens are rejected as expired before reaching this seam, so deleting an
    // expired row cannot revive its capability. Validate both the filename and
    // file type first: cleanup must never follow or remove an unexpected entry.
    let entries = std::fs::read_dir(&dir)
        .map_err(|error| format!("read armed-token replay store {}: {error}", dir.display()))?;
    let mut removed_expired = false;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_nonce_row(&name) || !entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
            continue;
        }
        let path = entry.path();
        let expired = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .is_some_and(|expiry| expiry < now_ms);
        if !expired {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed_expired = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "remove expired armed-token nonce {}: {error}",
                    path.display()
                ));
            }
        }
    }
    if removed_expired {
        sync_directory(&dir)?;
    }

    let path = dir.join(nonce_digest(nonce));
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => {
            return Err(format!(
                "claim armed-token nonce {}: {error}",
                path.display()
            ));
        }
    };
    if let Err(error) = file
        .write_all(expires_at_ms.to_string().as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(&path);
        let _ = sync_directory(&dir);
        return Err(format!(
            "persist armed-token nonce {}: {error}",
            path.display()
        ));
    }
    sync_directory(&dir)?;
    Ok(true)
}

// ─────────────────────────── the armed-token gate ───────────────────────────

/// The verdict of verifying an armed token against a `(verb, node, now)` context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenVerdict {
    /// A well-formed, unexpired, correctly-bound, correctly-signed token.
    Valid,
    /// No token was presented — the mutation is refused (the default, safe path).
    Missing,
    /// A token was presented but is not a parseable `v2` armed token / has a stunted nonce.
    Malformed,
    /// The token's expiry is in the past.
    Expired,
    /// The token is validly signed but remains usable beyond the narrow
    /// privileged-mutation window accepted by the consumer.
    LifetimeTooLong,
    /// The mutation envelope does not use the one supported wire schema.
    UnsupportedSchema,
    /// The token authorizes a different verb or node than this request.
    Mismatch,
    /// The request body differs from the frozen body the shell authorized.
    RequestMismatch,
    /// The signature does not verify under the signer's key (forged / wrong key).
    BadSignature,
    /// A valid capability nonce was already consumed by an earlier request.
    Replayed,
    /// The durable nonce ledger could not record the capability, so apply fails closed.
    ReplayStoreUnavailable,
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
            Self::LifetimeTooLong => "armed token exceeds the 30-second lifetime",
            Self::UnsupportedSchema => "unsupported mutation schema version",
            Self::Mismatch => "armed token does not authorize this verb/node/target",
            Self::RequestMismatch => "armed token does not authorize this request body",
            Self::BadSignature => "armed token signature did not verify",
            Self::Replayed => "armed token was already used",
            Self::ReplayStoreUnavailable => "armed-token replay store is unavailable",
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
    target: &str,
    request_body: &str,
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
    if parsed.verb != verb || parsed.node != node || parsed.target != target {
        return TokenVerdict::Mismatch;
    }
    let Ok(request_sha256) = cloud_request_digest(request_body) else {
        return TokenVerdict::Malformed;
    };
    if parsed.request_sha256 != request_sha256 {
        return TokenVerdict::RequestMismatch;
    }
    if now_ms > parsed.expires_at_ms {
        return TokenVerdict::Expired;
    }
    if !signer.verify_payload(&parsed.signing_payload(), &parsed.signature) {
        return TokenVerdict::BadSignature;
    }
    TokenVerdict::Valid
}

// ─────────────────────────── the placement gate ───────────────────────────

/// Where a request is placed relative to this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placement {
    /// This node performs the mutation because it is the explicit placement
    /// target (`body.node == host`).
    Local,
    /// No placement was supplied. Mutations fail closed rather than being
    /// interpreted as local by every cloud worker on the mesh.
    Missing,
    /// The mutation is placed on another node — carries that node's id. This node
    /// does not perform it (the target does, or it is honestly gated when the
    /// target is unreachable).
    Remote(String),
}

/// The placement decision: an explicit matching node is [`Placement::Local`], a
/// blank node is [`Placement::Missing`], and another node is
/// [`Placement::Remote`]. Pure + testable — the leader gate's replacement.
#[must_use]
pub(crate) fn placement_match(body_node: &str, host: &str) -> Placement {
    let node = body_node.trim();
    if node.is_empty() {
        Placement::Missing
    } else if node == host {
        Placement::Local
    } else {
        Placement::Remote(node.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE;

    fn signer() -> HmacTokenSigner {
        HmacTokenSigner::new(b"test-mesh-arming-key".to_vec())
    }

    fn mint(
        signer: &HmacTokenSigner,
        nonce: &str,
        expires: i64,
        verb: &str,
        node: &str,
        target: &str,
        body: &str,
    ) -> ArmedToken {
        ArmedToken::mint(
            signer,
            nonce,
            expires,
            verb,
            node,
            target,
            &cloud_request_digest(body).unwrap(),
        )
    }

    #[test]
    fn a_freshly_minted_token_verifies_for_its_bound_verb_and_node() {
        let s = signer();
        let body = r#"{"node":"eagle"}"#;
        let tok = mint(
            &s,
            "nonce-abcdef",
            10_000,
            "provision",
            "eagle",
            CLOUD_ARM_NODE_SCOPE,
            body,
        );
        let verdict = verify_token(
            Some(&tok.encode()),
            "provision",
            "eagle",
            CLOUD_ARM_NODE_SCOPE,
            body,
            5_000,
            &s,
        );
        assert_eq!(verdict, TokenVerdict::Valid);
        assert!(verdict.is_valid());
    }

    #[test]
    fn a_token_round_trips_through_encode_parse() {
        let s = signer();
        let tok = mint(
            &s,
            "nonce-abcdef",
            10_000,
            "destroy",
            "db.mesh.internal",
            "database",
            r#"{"node":"db.mesh.internal","instance":"database"}"#,
        );
        // A dotted FQDN node survives the pipe-delimited encoding.
        let back = ArmedToken::parse(&tok.encode()).expect("parse");
        assert_eq!(back, tok);
        assert_eq!(back.node, "db.mesh.internal");
    }

    #[test]
    fn every_failure_mode_is_a_distinct_honest_verdict_never_a_fabricated_pass() {
        let s = signer();
        let body = r#"{"node":"eagle"}"#;
        // Missing.
        assert_eq!(
            verify_token(
                None,
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                0,
                &s
            ),
            TokenVerdict::Missing
        );
        assert_eq!(
            verify_token(
                Some("   "),
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                0,
                &s
            ),
            TokenVerdict::Missing
        );
        // Malformed (not a v1 token).
        assert_eq!(
            verify_token(
                Some("garbage"),
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                0,
                &s
            ),
            TokenVerdict::Malformed
        );
        // Malformed (stunted nonce).
        let short = mint(
            &s,
            "abc",
            10_000,
            "provision",
            "eagle",
            CLOUD_ARM_NODE_SCOPE,
            body,
        );
        assert_eq!(
            verify_token(
                Some(&short.encode()),
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                0,
                &s
            ),
            TokenVerdict::Malformed
        );
        // Mismatch — right key, wrong verb.
        let tok = mint(
            &s,
            "nonce-abcdef",
            10_000,
            "provision",
            "eagle",
            CLOUD_ARM_NODE_SCOPE,
            body,
        );
        assert_eq!(
            verify_token(
                Some(&tok.encode()),
                "destroy",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                0,
                &s
            ),
            TokenVerdict::Mismatch
        );
        // Mismatch — right key, wrong node.
        assert_eq!(
            verify_token(
                Some(&tok.encode()),
                "provision",
                "otter",
                CLOUD_ARM_NODE_SCOPE,
                body,
                0,
                &s
            ),
            TokenVerdict::Mismatch
        );
        // Mismatch — right verb/node, substituted target.
        assert_eq!(
            verify_token(
                Some(&tok.encode()),
                "provision",
                "eagle",
                "peer-workload",
                body,
                0,
                &s
            ),
            TokenVerdict::Mismatch
        );
        assert_eq!(
            verify_token(
                Some(&tok.encode()),
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                r#"{"node":"eagle","unexpected":true}"#,
                0,
                &s,
            ),
            TokenVerdict::RequestMismatch
        );
        // Expired.
        assert_eq!(
            verify_token(
                Some(&tok.encode()),
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                20_000,
                &s
            ),
            TokenVerdict::Expired
        );
        // BadSignature — a token minted by a different key.
        let other = HmacTokenSigner::new(b"a-different-key".to_vec());
        let forged = mint(
            &other,
            "nonce-abcdef",
            10_000,
            "provision",
            "eagle",
            CLOUD_ARM_NODE_SCOPE,
            body,
        );
        assert_eq!(
            verify_token(
                Some(&forged.encode()),
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                5_000,
                &s
            ),
            TokenVerdict::BadSignature
        );
    }

    #[test]
    fn the_null_signer_never_validates_any_token() {
        // A node with no arming key refuses every mutation: even a well-formed token
        // minted by a real key fails the NullSigner's verification.
        let real = signer();
        let body = r#"{"node":"eagle"}"#;
        let tok = mint(
            &real,
            "nonce-abcdef",
            10_000,
            "provision",
            "eagle",
            CLOUD_ARM_NODE_SCOPE,
            body,
        );
        assert_eq!(
            verify_token(
                Some(&tok.encode()),
                "provision",
                "eagle",
                CLOUD_ARM_NODE_SCOPE,
                body,
                0,
                &NullSigner,
            ),
            TokenVerdict::BadSignature
        );
    }

    #[test]
    fn placement_routes_local_versus_remote() {
        assert_eq!(placement_match("eagle", "eagle"), Placement::Local);
        assert_eq!(placement_match("", "eagle"), Placement::Missing);
        assert_eq!(placement_match("   ", "eagle"), Placement::Missing);
        assert_eq!(
            placement_match("otter", "eagle"),
            Placement::Remote("otter".to_string())
        );
    }

    #[test]
    fn packaging_delivers_the_credential_only_through_systemd_private_directories() {
        let dropin = include_str!("../../../../../../packaging/systemd/cloud-arm-credential.conf");
        let daemon = include_str!("../../../../../../packaging/systemd/mackesd.service");
        let shell = include_str!("../../../../../../packaging/bootc/units/mde-shell-egui.service");
        let helper =
            include_str!("../../../../../../install-helpers/provision-cloud-arm-credential.sh");
        assert!(dropin.contains(
            "LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key"
        ));
        assert!(!dropin.contains("Environment="));
        assert!(
            daemon
                .lines()
                .all(|line| !line.trim_start().starts_with("User=")),
            "mackesd remains an intentional root unit"
        );
        assert!(
            shell
                .lines()
                .all(|line| !line.trim_start().starts_with("User=")),
            "the shipped DRM shell remains root-owned"
        );
        assert!(helper.contains("mackesd.service mde-shell-egui.service"));
        assert!(helper.contains("/etc/systemd/system/$unit.d/50-cloud-arm-credential.conf"));
        assert!(!helper.contains("MDE_CLOUD_ARM_KEY"));
    }
}

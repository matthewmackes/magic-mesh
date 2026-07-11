//! NF-3.6.a (v2.5) — peer-enrollment helper.
//!
//! The Rust side of the v2.5 `mesh:<id>@<ip>:<port>#<bearer>`
//! join-token flow. Consumed by:
//!
//!   * the extended `mackesd enroll --token` CLI (bin/mackesd.rs)
//!   * the future `dev.mackes.MDE.Nebula.Enroll` D-Bus method
//!     (NF-3.6 — convenience shim over this module)
//!   * the wizard's Apply page (NF-14.4 / NF-14.5 — shells out to
//!     the CLI)
//!
//! Architecture: peer-side enrollment is QNM-Shared-mediated.
//! The peer writes a [`PendingEnrollment`] file at
//! `QNM-Shared/<self-id>/mackesd/pending-enroll.json`. The
//! lighthouse's `mackesd ca sign-csr <peer-id>` (NF-3.6.a CLI
//! helper) reads it, signs the cert, writes the
//! `nebula-bundle.json` back to QNM-Shared/<self-id>/mackesd/.
//! `nebula_supervisor` (NF-3.4) is already watching for that
//! bundle and materializes /etc/nebula/ when it appears.
//!
//! This module ships peer-side: publish the CSR + poll for the
//! signed bundle. Lighthouse-side signing is a separate concern
//! (see [`sign_pending_csr`]). The two flows share the [`JoinToken`]
//! parser + [`PendingEnrollment`] wire shape so they stay in lock-
//! step on every wire-format change.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::ca::bundle::{bundle_path, read_bundle};
use crate::enrollment::{build_identity, EnrolledIdentity};

/// QR-friendly upper bound on the wire-form join token.
///
/// Legacy v2.5 tokens carry a short bare bearer. OW-4 invite-issue can also
/// re-express a wizard invite as the same `mesh:` token by using the invite's
/// canonical payload as the bearer; that binds mesh-id + expiry to the ledger but
/// makes the token longer. Keep enough room for that form plus `?fp=<sha256>`.
/// The Python helper's `JOIN_TOKEN_MAX_LEN` lock at
/// `mackes/wizard/pages/mesh_passcode.py` tracks this value.
pub const JOIN_TOKEN_MAX_LEN: usize = 512;

/// Length of a SHA-256 fingerprint rendered as lowercase hex.
const SHA256_HEX_LEN: usize = 64;

/// Filename for the per-peer pending-enrollment CSR the
/// lighthouse looks for. Lives alongside `heartbeat.json` +
/// `nebula-bundle.json` in `QNM-Shared/<peer-id>/mackesd/`.
pub const PENDING_ENROLL_FILENAME: &str = "pending-enroll.json";

/// Default poll cadence — how often the peer-side waiter checks
/// for the signed bundle.
pub const ENROLL_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Default wait budget — peer-side waiter gives up after this
/// long and surfaces an informative timeout error pointing the
/// operator at the manual `mackesd ca sign-csr` recovery step.
pub const ENROLL_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Parsed wire-form of `mesh:<id>@<ip>:<port>#<bearer>`. Lock-step
/// with the Python `JoinToken` dataclass in
/// `mackes/wizard/pages/mesh_passcode.py` so the wizard + the
/// CLI consume the same shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinToken {
    /// URL-safe mesh identifier (no `@` / `:` / `#` / `/`).
    pub mesh_id: String,
    /// Lighthouse IPv4 address (overlay or public — both work).
    pub lighthouse: String,
    /// Lighthouse port (1..=65535).
    pub port: u16,
    /// Base32/URL-safe bearer the lighthouse validates against
    /// its pending-enroll allow-list.
    pub bearer: String,
    /// ONBOARD-1 — optional SHA-256 fingerprint (lowercase hex) of the
    /// lighthouse's `/enroll` HTTPS endpoint TLS cert. When present, the
    /// network-enroll client pins the lighthouse cert to this before sending
    /// the CSR (no trust-on-first-use). Absent → the legacy QNM-Shared flow
    /// (co-located nodes). Additive: v2.5 tokens parse with `fp: None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fp: Option<String>,
}

impl JoinToken {
    /// Round-trip back to the wire form. Symmetric with
    /// [`parse_join_token`]. Appends `?fp=<hex>` when a fingerprint is set.
    #[must_use]
    pub fn encode(&self) -> String {
        let base = format!(
            "mesh:{}@{}:{}#{}",
            self.mesh_id, self.lighthouse, self.port, self.bearer
        );
        match &self.fp {
            Some(fp) => format!("{base}?fp={fp}"),
            None => base,
        }
    }
}

/// Parse a wire-form join token. Returns `None` on any failure
/// (wrong shape, port out of range, non-IPv4 lighthouse, etc.).
/// Mirrors the Python `parse_join_token` rejection rules.
///
/// # Errors
///
/// Returns `None` rather than `Result` so the CLI / wizard /
/// D-Bus surface can render the same "invalid join token" copy
/// without branching on subtypes. Operators who need
/// fine-grained diagnostic messages should validate by
/// inspecting the wire shape directly.
#[must_use]
pub fn parse_join_token(raw: &str) -> Option<JoinToken> {
    if raw.is_empty() || raw.len() > JOIN_TOKEN_MAX_LEN {
        return None;
    }
    let stripped = raw.strip_prefix("mesh:")?;
    // mesh:<mesh_id>@<lighthouse>:<port>#<bearer>
    let (mesh_id, rest) = stripped.split_once('@')?;
    if mesh_id.is_empty() || !is_mesh_id_url_safe(mesh_id) {
        return None;
    }
    let (lighthouse_port, bearer_and_fp) = rest.split_once('#')?;
    // ONBOARD-1: split the additive `?fp=<hex>` suffix off the bearer.
    // v2.5 tokens have no `?fp=` and parse with `fp: None`.
    let (bearer, fp) = match bearer_and_fp.split_once("?fp=") {
        Some((bearer, fp_hex)) => {
            if fp_hex.len() != SHA256_HEX_LEN || !is_lower_hex(fp_hex) {
                return None;
            }
            (bearer, Some(fp_hex.to_string()))
        }
        None => (bearer_and_fp, None),
    };
    if bearer.is_empty() || !is_bearer_url_safe(bearer) {
        return None;
    }
    let (lighthouse, port_str) = lighthouse_port.rsplit_once(':')?;
    if lighthouse.is_empty() {
        return None;
    }
    let port: u16 = port_str.parse().ok()?;
    if port == 0 {
        return None;
    }
    if !is_ipv4(lighthouse) {
        return None;
    }
    Some(JoinToken {
        mesh_id: mesh_id.to_string(),
        lighthouse: lighthouse.to_string(),
        port,
        bearer: bearer.to_string(),
        fp,
    })
}

fn is_mesh_id_url_safe(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn is_bearer_url_safe(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '=' | '-'))
}

fn is_lower_hex(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
}

fn is_ipv4(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

/// Wire shape of the per-peer pending-enrollment CSR the peer
/// publishes to QNM-Shared. The lighthouse reads it, validates
/// the bearer, signs a cert, writes the signed bundle back. JSON
/// for self-contained sneakernet replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEnrollment {
    /// Token the peer presented. Carries the bearer the
    /// lighthouse cross-checks against its issued-but-unredeemed
    /// list.
    pub token: JoinToken,
    /// Peer's stable node id (e.g. `peer:anvil`). The lighthouse
    /// uses this as the row identifier in `nebula_peer_certs`.
    pub node_id: String,
    /// Hostname at enrollment time, for the display column on the
    /// roster.
    pub display_name: String,
    /// Hardware fingerprint — drives the idempotent re-enroll
    /// path. Lighthouse matches this against existing rows to
    /// refresh credentials in place when the same physical box
    /// re-enrolls.
    pub hw_fingerprint: String,
    /// Hex-encoded Ed25519 public key the peer just generated.
    /// The lighthouse signs a Nebula cert binding this key to the
    /// allocated overlay IP.
    pub public_key_hex: String,
    /// Unix-epoch seconds when the CSR was written. Used by the
    /// lighthouse to expire stale CSRs.
    pub created_at: i64,
}

/// Compute the per-peer pending-enrollment path under a
/// QNM-Shared root. Mirrors the `bundle_path` convention.
#[must_use]
pub fn pending_enroll_path(workgroup_root: &Path, peer_id: &str) -> PathBuf {
    workgroup_root
        .join(peer_id)
        .join("mackesd")
        .join(PENDING_ENROLL_FILENAME)
}

/// Outcome of a successful peer-side enrollment.
#[derive(Debug, Clone)]
pub struct EnrollOutcome {
    /// Overlay IP allocated by the lighthouse.
    pub overlay_ip: String,
    /// Lighthouse-side mesh-id confirmed by the bundle.
    pub mesh_id: String,
    /// Wall-clock time from CSR-publish to bundle-arrival.
    pub waited: Duration,
}

/// Errors a peer-side enrollment can hit. Each variant carries
/// the human-readable copy the CLI surfaces verbatim — keep them
/// actionable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollError {
    /// The wire-form token didn't parse. Includes the offending
    /// input length so operators can spot truncation.
    InvalidToken {
        /// Length of the rejected raw input.
        raw_len: usize,
    },
    /// Could not write the pending-enroll CSR (filesystem error).
    PublishFailed {
        /// Underlying error message.
        reason: String,
    },
    /// The lighthouse didn't sign within the wait budget. Carries
    /// the elapsed seconds so the CLI message can quote it.
    Timeout {
        /// Wall-clock seconds the waiter spent.
        elapsed_s: u64,
    },
    /// Bundle appeared but didn't parse — the lighthouse may have
    /// written a corrupt or version-mismatched file.
    BundleCorrupt {
        /// Underlying error message.
        reason: String,
    },
}

impl std::fmt::Display for EnrollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidToken { raw_len } => write!(
                f,
                "invalid join token (length {raw_len}). \
                 Expected mesh:<id>@<ipv4>:<port>#<bearer>, max {JOIN_TOKEN_MAX_LEN} chars.",
            ),
            Self::PublishFailed { reason } => write!(
                f,
                "could not publish pending-enroll CSR: {reason}. \
                 Check QNM-Shared is mounted + writable for the mackes user.",
            ),
            Self::Timeout { elapsed_s } => write!(
                f,
                "waited {elapsed_s} s for the lighthouse to sign — \
                 no bundle appeared. Run `mackesd ca sign-csr \
                 <your-node-id>` on the lighthouse and retry.",
            ),
            Self::BundleCorrupt { reason } => write!(
                f,
                "bundle arrived but didn't parse: {reason}. \
                 The lighthouse may have written an incompatible \
                 version — confirm both sides are on the same MDE \
                 release.",
            ),
        }
    }
}

impl std::error::Error for EnrollError {}

/// Publish the pending-enroll CSR to QNM-Shared. Writes the file
/// atomically (temp + rename). Idempotent — re-running overwrites
/// the previous CSR (lighthouse always reads the latest version).
///
/// # Errors
///
/// Surfaces filesystem errors as [`EnrollError::PublishFailed`].
pub fn publish_enrollment_request(
    workgroup_root: &Path,
    node_id: &str,
    pending: &PendingEnrollment,
) -> Result<PathBuf, EnrollError> {
    let path = pending_enroll_path(workgroup_root, node_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| EnrollError::PublishFailed {
            reason: e.to_string(),
        })?;
    }
    let body = serde_json::to_vec_pretty(pending).map_err(|e| EnrollError::PublishFailed {
        reason: e.to_string(),
    })?;
    // Atomic write: temp file + rename so a lighthouse polling
    // mid-write never reads a half-formed CSR.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &body).map_err(|e| EnrollError::PublishFailed {
        reason: e.to_string(),
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| EnrollError::PublishFailed {
        reason: e.to_string(),
    })?;
    Ok(path)
}

/// Wait for the lighthouse-signed bundle to appear in QNM-Shared.
/// Polls every [`ENROLL_POLL_INTERVAL`] until the bundle exists +
/// parses, or [`ENROLL_WAIT_TIMEOUT`] elapses. Returns the parsed
/// bundle on success.
///
/// # Errors
///
/// - [`EnrollError::Timeout`] when no bundle appears in the
///   budget.
/// - [`EnrollError::BundleCorrupt`] when a bundle appears but
///   doesn't parse.
pub fn wait_for_signed_bundle(
    workgroup_root: &Path,
    node_id: &str,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<(crate::ca::bundle::NebulaBundle, Duration), EnrollError> {
    let path = bundle_path(workgroup_root, node_id);
    let started = Instant::now();
    loop {
        if path.exists() {
            match read_bundle(&path) {
                Ok(bundle) => return Ok((bundle, started.elapsed())),
                Err(e) => {
                    return Err(EnrollError::BundleCorrupt {
                        reason: e.to_string(),
                    });
                }
            }
        }
        if started.elapsed() >= timeout {
            return Err(EnrollError::Timeout {
                elapsed_s: started.elapsed().as_secs(),
            });
        }
        std::thread::sleep(poll_interval);
    }
}

/// End-to-end peer-side enrollment from a raw join-token string.
/// Generates a fresh identity, writes the CSR, waits for the
/// lighthouse to sign. Returns [`EnrollOutcome`] on success.
///
/// On a peer that IS the lighthouse (the first peer in a new
/// mesh), the caller is expected to run `mackesd ca mint`
/// separately + skip this enroll flow entirely — the lighthouse
/// signs its own cert via the mint path.
///
/// # Errors
///
/// Per [`EnrollError`].
pub fn enroll_with_token(
    workgroup_root: &Path,
    node_id: &str,
    display_name: &str,
    raw_token: &str,
) -> Result<EnrollOutcome, EnrollError> {
    let token = parse_join_token(raw_token).ok_or(EnrollError::InvalidToken {
        raw_len: raw_token.len(),
    })?;
    let identity = build_identity();
    let pending = build_pending(&identity, node_id, display_name, token);
    publish_enrollment_request(workgroup_root, node_id, &pending)?;
    let (bundle, waited) = wait_for_signed_bundle(
        workgroup_root,
        node_id,
        ENROLL_POLL_INTERVAL,
        ENROLL_WAIT_TIMEOUT,
    )?;
    Ok(EnrollOutcome {
        overlay_ip: bundle.overlay_ip,
        mesh_id: bundle.mesh_id,
        waited,
    })
}

/// Pure helper — build the PendingEnrollment payload from a
/// freshly-minted identity + the parsed token. Split out so tests
/// can exercise the shape without spinning the filesystem.
#[must_use]
pub fn build_pending(
    identity: &EnrolledIdentity,
    node_id: &str,
    display_name: &str,
    token: JoinToken,
) -> PendingEnrollment {
    let public_key_hex = hex_bytes(identity.key.verifying_key().as_bytes());
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    PendingEnrollment {
        token,
        node_id: node_id.to_string(),
        display_name: display_name.to_string(),
        hw_fingerprint: identity.hw_fingerprint.clone(),
        public_key_hex,
        created_at,
    }
}

// ─────────────────────────────────────────────────────────────────
// NF-3.6.b — lighthouse-side signing flow.
// ─────────────────────────────────────────────────────────────────

/// Paths the lighthouse provides for the sign-csr flow. The
/// defaults mirror the v2.5 Nebula on-disk convention but every
/// field is overridable so tests + non-standard deployments can
/// redirect.
#[derive(Debug, Clone)]
pub struct SignCsrPaths {
    /// Public CA cert (PEM). Default `/etc/nebula/ca.crt`.
    pub ca_crt: PathBuf,
    /// Sealed CA private key. Default
    /// `/var/lib/mackesd/nebula-ca/ca.key`.
    pub ca_key: PathBuf,
    /// Scratch dir for the peer cert/key intermediate files.
    /// The unsealed PEM bytes get read back + embedded in the
    /// bundle; the on-disk files can be cleaned up by the caller
    /// after the bundle lands.
    pub scratch_dir: PathBuf,
}

impl SignCsrPaths {
    /// Production defaults — `/etc/nebula/ca.crt`,
    /// `/var/lib/mackesd/nebula-ca/ca.key`,
    /// `/var/lib/mackesd/nebula-ca/scratch/`.
    #[must_use]
    pub fn production_defaults() -> Self {
        Self {
            ca_crt: PathBuf::from("/etc/nebula/ca.crt"),
            ca_key: PathBuf::from("/var/lib/mackesd/nebula-ca/ca.key"),
            scratch_dir: PathBuf::from("/var/lib/mackesd/nebula-ca/scratch"),
        }
    }
}

/// Outcome of a successful lighthouse-side signing.
#[derive(Debug, Clone)]
pub struct SignOutcome {
    /// Peer that was signed.
    pub peer_id: String,
    /// Allocated overlay IP.
    pub overlay_ip: String,
    /// CA epoch the cert was signed under.
    pub epoch: i64,
    /// Path the bundle was written to under QNM-Shared.
    pub bundle_path: PathBuf,
}

/// Errors the lighthouse-side signing can hit. Distinct from
/// [`EnrollError`] (peer-side) so the CLI can render
/// lighthouse-specific copy without mixing in peer hints.
#[derive(Debug)]
pub enum SignCsrError {
    /// No pending-enroll CSR for the named peer.
    CsrMissing {
        /// Path the lighthouse looked at.
        path: PathBuf,
    },
    /// CSR file existed but didn't deserialize. Likely a
    /// version-mismatched peer.
    CsrCorrupt {
        /// Underlying parser error.
        reason: String,
    },
    /// ENT-1 (C1) — the presented bearer is not in the
    /// issued-but-unredeemed ledger: wrong, replayed, or never
    /// minted. Refused before any signing work.
    BearerNotIssued {
        /// The refusing node id (for the audit line).
        node_id: String,
    },
    /// Cert signing itself failed (nebula-cert missing, no
    /// active CA, permission denied on a path).
    SignFailed {
        /// Underlying CaError message.
        reason: String,
    },
    /// Bundle write to QNM-Shared failed.
    BundleWriteFailed {
        /// Underlying I/O error.
        reason: String,
    },
    /// Reading the peer key bytes back from the sealed file
    /// failed (uid mismatch, mode drift, etc.).
    KeyReadFailed {
        /// Underlying CaError message.
        reason: String,
    },
    /// TUNE-11 — the active-peer count already meets or exceeds
    /// the [`crate::ca::sign::MAX_PEER_CAP`] lock and the caller
    /// did not pass `allow_override = true`.
    PeerCapReached {
        /// Distinct active node_ids in `nebula_peer_certs` at the
        /// active epoch when the check fired.
        current: u32,
        /// The locked cap value (8 for 1.0).
        cap: u32,
    },
    /// EPIC-SEC-BANLIST (Q53) — the node-id appears in the union of
    /// every peer's ban list. A banned identity is refused
    /// enrollment even with a valid passcode + even across a CA
    /// rotation. Lift the ban by editing the ban list (no override
    /// flag — a ban is deliberate).
    NodeBanned {
        /// The banned node-id the CSR carried.
        node_id: String,
    },
}

impl std::fmt::Display for SignCsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CsrMissing { path } => write!(
                f,
                "no pending-enroll CSR at {}. Has the peer \
                 actually run `mackesd enroll --token`?",
                path.display(),
            ),
            Self::CsrCorrupt { reason } => write!(
                f,
                "pending-enroll CSR didn't parse: {reason}. \
                 Confirm both peers are on the same MDE release.",
            ),
            Self::BearerNotIssued { node_id } => write!(
                f,
                "enrollment of {node_id} refused: its bearer is not \
                 issued-and-unredeemed. Mint a fresh single-use token \
                 on this lighthouse with `mackesd enroll-token` and \
                 re-run the join with it (ENT-1).",
            ),
            Self::SignFailed { reason } => write!(
                f,
                "cert signing failed: {reason}. Likely no active \
                 CA (run `mackesd ca mint` first) or nebula-cert \
                 missing from PATH.",
            ),
            Self::BundleWriteFailed { reason } => write!(
                f,
                "bundle write to QNM-Shared failed: {reason}. \
                 Confirm the QNM-Shared mount is writable.",
            ),
            Self::KeyReadFailed { reason } => write!(
                f,
                "could not read signed peer key for bundle: \
                 {reason}. Check ownership on the scratch dir.",
            ),
            Self::PeerCapReached { current, cap } => write!(
                f,
                "MackesDE for Workgroups is sized for up to {cap} peers \
                 (Q3 lock). The mesh already has {current} active peer \
                 cert(s) at the current CA epoch. Run \
                 `mackesd ca sign-csr <node-id> --override-cap` to \
                 bypass; document the exception in \
                 docs/design/cap-overrides.md.",
            ),
            Self::NodeBanned { node_id } => write!(
                f,
                "node-id '{node_id}' is on the mesh ban list (Q53) and \
                 cannot re-join, even with a valid passcode. A ban is \
                 deliberate — there is no override flag. To lift it, run \
                 `mackesd ca unban {node_id}` on the peer that set it (or \
                 edit ban-list.json under that peer's QNM-Shared dir).",
            ),
        }
    }
}

impl std::error::Error for SignCsrError {}

/// Lighthouse-side helper that turns a pending-enroll CSR into
/// a signed bundle the peer can materialize. End-to-end:
///
///   1. Read PendingEnrollment from
///      `workgroup_root/<peer_id>/mackesd/pending-enroll.json`.
///   2. Call `ca::sign::sign_peer_cert` with `PeerRole::Peer`
///      under the active CA epoch (allocates overlay IP).
///   3. Read the unsealed peer key bytes back from the sealed
///      file (seal just enforces 0600 + uid match — the bytes
///      are the raw PEM).
///   4. Build a [`crate::ca::bundle::NebulaBundle`] containing
///      the signed cert + key + CA cert + lighthouse roster.
///   5. Write the bundle to
///      `workgroup_root/<peer_id>/mackesd/nebula-bundle.json` via
///      [`crate::ca::bundle::write_bundle`].
///   6. Return [`SignOutcome`] for the CLI to display.
///
/// The lighthouse roster passed via `lighthouses` is whatever
/// the caller decides — typically the single self-lighthouse +
/// any failover hosts. Empty rosters are accepted but produce a
/// bundle the peer's nebula_supervisor will warn about.
///
/// # Errors
///
/// Per [`SignCsrError`].
#[allow(clippy::too_many_arguments)]
pub fn sign_pending_csr<B: crate::ca::NebulaCertBackend + ?Sized>(
    backend: &B,
    conn: &rusqlite::Connection,
    workgroup_root: &Path,
    peer_id: &str,
    // The CALLER's advisory mesh-id. IGNORED: the authoritative mesh is
    // the one the peer's join token declares (`csr.token.mesh_id`) — the
    // peer is joining the mesh named in its token, and the lighthouse
    // signs under THAT mesh's active CA (erroring if it has none for it).
    // Both the CLI and the auto-signer used to pass a bogus
    // `mesh-<local-node-id>` fallback here, which is why manual sign-csr
    // AND the auto-signer both failed with "no active CA". Bed fix #5.
    _mesh_hint: &str,
    paths: &SignCsrPaths,
    lighthouses: Vec<crate::ca::bundle::LighthouseEntry>,
    allow_override: bool,
) -> Result<SignOutcome, SignCsrError> {
    let csr_path = pending_enroll_path(workgroup_root, peer_id);
    if !csr_path.exists() {
        return Err(SignCsrError::CsrMissing { path: csr_path });
    }
    let csr_bytes = std::fs::read(&csr_path).map_err(|e| SignCsrError::CsrCorrupt {
        reason: format!("read {}: {e}", csr_path.display()),
    })?;
    let csr: PendingEnrollment =
        serde_json::from_slice(&csr_bytes).map_err(|e| SignCsrError::CsrCorrupt {
            reason: e.to_string(),
        })?;
    // Delegate to the transport-independent signing core (the same
    // core the ONBOARD-2 network `/enroll` endpoint calls), then
    // ATOMICALLY consume the single-use bearer and write the bundle
    // only if we won that consume (security-5 — spend-then-deliver, so
    // two concurrent requests presenting the SAME bearer can never
    // both be honored).
    let signed_node_id = csr.node_id.clone();
    let bundle = sign_csr_into_bundle(
        backend,
        conn,
        workgroup_root,
        &csr,
        paths,
        lighthouses,
        allow_override,
    )?;
    // ENT-1 single-use (security-5): the earlier `is_pending` gate in
    // the core is only a pre-check; THIS atomic consume is the sole
    // race-decider. Of any racers that all passed the pre-check,
    // exactly one wins here (unlink is atomic) and delivers the
    // bundle; a loser is refused before writing, so two racers never
    // both clobber the shared bundle path. Closes the check-then-act
    // TOCTOU. (Narrow residual: a `write_bundle` I/O failure AFTER a
    // won consume burns the bearer without delivery — a legitimate
    // retry then needs a fresh token; acceptable vs. the double-honor
    // it replaces.)
    if !crate::bearer_ledger::consume(workgroup_root, &csr.token.bearer) {
        return Err(SignCsrError::BearerNotIssued {
            node_id: csr.node_id.clone(),
        });
    }
    let bundle_path = crate::ca::bundle::bundle_path(workgroup_root, peer_id);
    crate::ca::bundle::write_bundle(&bundle_path, &bundle).map_err(|e| {
        SignCsrError::BundleWriteFailed {
            reason: e.to_string(),
        }
    })?;
    Ok(SignOutcome {
        peer_id: signed_node_id,
        overlay_ip: bundle.overlay_ip,
        epoch: bundle.epoch,
        bundle_path,
    })
}

/// Transport-independent signing core (ONBOARD-2). Takes an
/// already-parsed [`PendingEnrollment`] and returns the signed
/// [`crate::ca::bundle::NebulaBundle`] **without** reading the CSR
/// from disk, consuming the bearer, or delivering the bundle — those
/// steps belong to the caller, because they differ between the
/// QNM-Shared file flow ([`sign_pending_csr`], which atomically
/// consumes the bearer then writes the bundle) and the network
/// `/enroll` endpoint (which atomically consumes the bearer then
/// returns the bundle in the HTTPS response). Both gate delivery on
/// winning [`crate::bearer_ledger::consume`] (security-5); the
/// `is_pending` gate below is only a fast pre-check, never the
/// single-use decision.
///
/// Runs the identical authorization gates as the file flow, in the
/// same order: ban-list → bearer ledger → 8-peer cap → sign. The
/// authoritative mesh is `csr.token.mesh_id` (bed fix #5), never a
/// caller-supplied hint.
///
/// # Errors
///
/// Per [`SignCsrError`] — except `CsrMissing` / `CsrCorrupt` (the
/// CSR is already parsed) and `BundleWriteFailed` (no write here).
#[allow(clippy::too_many_arguments)]
pub fn sign_csr_into_bundle<B: crate::ca::NebulaCertBackend + ?Sized>(
    backend: &B,
    conn: &rusqlite::Connection,
    workgroup_root: &Path,
    csr: &PendingEnrollment,
    paths: &SignCsrPaths,
    lighthouses: Vec<crate::ca::bundle::LighthouseEntry>,
    allow_override: bool,
) -> Result<crate::ca::bundle::NebulaBundle, SignCsrError> {
    // The mesh the peer is actually joining (authoritative — bed fix #5).
    let mesh_id: &str = &csr.token.mesh_id;
    // EPIC-SEC-BANLIST (Q53) — refuse a banned node-id BEFORE the
    // cap check + before any signing work. A ban is a deliberate,
    // permanent block that survives CA rotation; there is no
    // override (unlike the cap). The union spans every peer's ban
    // list under workgroup_root, so a ban set anywhere blocks everywhere.
    if crate::ca::ban_list::is_banned(workgroup_root, &csr.node_id) {
        tracing::warn!(
            target: "mackesd::ban_list",
            event = "enroll.banned.refused",
            peer_id = %csr.node_id,
            mesh_id = %mesh_id,
            "EPIC-SEC-BANLIST: refusing enrollment of a banned node-id",
        );
        return Err(SignCsrError::NodeBanned {
            node_id: csr.node_id.clone(),
        });
    }
    // ENT-1 (C1) — the enforcement the docs always claimed: the
    // bearer must be issued-and-unredeemed in the ledger. Wrong,
    // replayed, or absent → refused before the cap check + before
    // any signing work. `allow_override` deliberately does NOT
    // bypass this (the override is a capacity lever, never an
    // authentication one).
    if !crate::bearer_ledger::is_pending(workgroup_root, &csr.token.bearer) {
        tracing::warn!(
            target: "mackesd::bearer_ledger",
            event = "enroll.bearer.refused",
            peer_id = %csr.node_id,
            mesh_id = %mesh_id,
            "ENT-1: refusing enrollment — bearer not issued/already redeemed",
        );
        return Err(SignCsrError::BearerNotIssued {
            node_id: csr.node_id.clone(),
        });
    }
    // HA / turn-key (#12) — a join authorized by a role-scoped LIGHTHOUSE bearer is
    // signed as a Host (am_lighthouse) AND handed the CA private key, so the new
    // node becomes a full signing lighthouse with no manual scp. Gated on the
    // BEARER NOTE (operator intent via `add-peer --role lighthouse`), never a
    // self-asserted CSR field — an ordinary peer bearer can never pull the CA key
    // (ENT-12 containment).
    let lighthouse_authorized =
        crate::bearer_ledger::is_lighthouse_bearer(workgroup_root, &csr.token.bearer);
    // TUNE-11 — 8-peer cap (Q3 + Q22) enforcement. Counts
    // distinct active node_ids at the active epoch. The
    // UNIQUE(node_id, epoch) constraint on `nebula_peer_certs`
    // means same-epoch re-enrollment of an existing peer
    // already fails at the SQL layer with a clear constraint
    // error, so this gate is concerned only with NEW peers
    // pushing past the cap.
    let active_count = crate::ca::sign::count_active_peers(conn, mesh_id).map_err(|e| {
        SignCsrError::SignFailed {
            reason: e.to_string(),
        }
    })?;
    if active_count >= crate::ca::sign::MAX_PEER_CAP {
        if allow_override {
            tracing::warn!(
                target: "mackesd::cap_override",
                event = "cap.override.engaged",
                peer_id = %csr.node_id,
                mesh_id = %mesh_id,
                current = active_count,
                cap = crate::ca::sign::MAX_PEER_CAP,
                "TUNE-11: signing peer past the 8-peer cap by operator override",
            );
        } else {
            return Err(SignCsrError::PeerCapReached {
                current: active_count,
                cap: crate::ca::sign::MAX_PEER_CAP,
            });
        }
    }
    // Hand-off to the underlying ca::sign machinery. Output
    // paths go into the scratch dir keyed by node_id so multiple
    // concurrent signings don't trample each other.
    std::fs::create_dir_all(&paths.scratch_dir).map_err(|e| SignCsrError::SignFailed {
        reason: format!("mkdir {}: {e}", paths.scratch_dir.display()),
    })?;
    let crt_out = paths.scratch_dir.join(format!("{}.crt", csr.node_id));
    let key_out = paths.scratch_dir.join(format!("{}.key", csr.node_id));
    // Bed fix #7: a stale scratch cert from a PRIOR sign of this node would
    // make nebula-cert refuse to overwrite. That clearing now lives inside
    // sign_peer_cert (the single signer funnel — covers this path AND the
    // mesh-init self-sign), so no removal is needed here.
    //
    // MULTI-LH-IP-ALLOC — collect every overlay IP already assigned MESH-WIDE
    // from the shared peer directory (etcd, fs fallback) and hand it to the
    // signer. A JOINED lighthouse's local cert-store only knows the certs IT
    // signed, so without this it re-allocates from 10.42.0.1 and collides with
    // the founding lighthouse's assignments (caught live 2026-06-27: a node
    // enrolled via a new lighthouse was handed 10.42.0.1, lh1's own IP). The
    // directory is the SUBSTRATE-V2 source of truth for who holds which IP.
    let etcd_eps = crate::substrate::etcd::default_endpoints();
    let mut directory_taken: std::collections::HashSet<String> =
        crate::substrate::peers::read_directory(workgroup_root)
            .into_iter()
            .filter_map(|p| {
                p.overlay_ip
                    .as_deref()
                    .map(str::trim)
                    .filter(|ip| !ip.is_empty())
                    .map(String::from)
            })
            .collect();
    // MIG-2 — also union the sign-time reservations (`/mesh/ipalloc/`), which are
    // visible immediately, unlike the heartbeat-lagged peer directory. Without
    // this a concurrent sign on another lighthouse could pick an IP this one (or
    // a peer signed seconds ago) just assigned but hasn't heartbeated yet.
    if !etcd_eps.is_empty() {
        directory_taken.extend(crate::substrate::peers::reserved_overlay_ips_blocking(
            &etcd_eps,
        ));
    }
    let signed = crate::ca::sign::sign_peer_cert(
        backend,
        conn,
        mesh_id,
        &csr.node_id,
        if lighthouse_authorized {
            crate::ca::sign::PeerRole::Host
        } else {
            crate::ca::sign::PeerRole::Peer
        },
        &paths.ca_crt,
        &paths.ca_key,
        &crt_out,
        &key_out,
        &directory_taken,
    )
    .map_err(|e| SignCsrError::SignFailed {
        reason: e.to_string(),
    })?;
    // MIG-2 — record the just-assigned overlay IP in the shared reservation
    // keyspace IMMEDIATELY, so the next sign anywhere in the mesh sees it as
    // taken (best-effort; the directory read above is the fallback guard).
    if !etcd_eps.is_empty() {
        let _ = crate::substrate::peers::reserve_overlay_ip_blocking(
            &etcd_eps,
            &signed.overlay_ip,
            &csr.node_id,
        );
    }
    // Read the unsealed key bytes for the bundle. seal::read_sealed
    // only enforces 0600 + uid match; the bytes are the raw PEM.
    let peer_key_pem_bytes =
        crate::ca::seal::read_sealed(&key_out).map_err(|e| SignCsrError::KeyReadFailed {
            reason: e.to_string(),
        })?;
    let peer_key_pem =
        String::from_utf8(peer_key_pem_bytes).map_err(|e| SignCsrError::KeyReadFailed {
            reason: format!("peer key isn't UTF-8: {e}"),
        })?;
    // Read the CA cert PEM for the bundle.
    let ca_cert_pem =
        std::fs::read_to_string(&paths.ca_crt).map_err(|e| SignCsrError::SignFailed {
            reason: format!("read CA cert {}: {e}", paths.ca_crt.display()),
        })?;
    // #12 — a lighthouse-authorized join also receives the CA PRIVATE key (the
    // receiver seals it at rest) so the new node can itself sign/enroll. `None` for
    // ordinary peers — they never carry the CA key.
    let ca_key_pem = if lighthouse_authorized {
        Some(
            std::fs::read_to_string(&paths.ca_key).map_err(|e| SignCsrError::SignFailed {
                reason: format!("read CA key {}: {e}", paths.ca_key.display()),
            })?,
        )
    } else {
        None
    };
    // Look up the active epoch + assemble the bundle.
    let active_epoch = crate::ca::sign::active_epoch(conn, mesh_id)
        .map_err(|e| SignCsrError::SignFailed {
            reason: e.to_string(),
        })?
        .unwrap_or(0);
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(crate::ca::bundle::NebulaBundle {
        mesh_id: mesh_id.to_string(),
        epoch: active_epoch,
        ca_cert_pem,
        peer_cert_pem: signed.cert_pem,
        peer_key_pem,
        overlay_ip: signed.overlay_ip,
        mesh_cidr: format!("{}/16", crate::ca::sign::DEFAULT_MESH_CIDR_BASE),
        lighthouses,
        ca_key_pem,
        created_at,
    })
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ---- parse_join_token coverage --------------------------

    #[test]
    fn parse_round_trips_a_canonical_token() {
        let raw = "mesh:mesh-001@10.0.0.5:4242#dGVzdC1iZWFyZXItYWJjZGVm";
        let tok = parse_join_token(raw).expect("decoded");
        assert_eq!(tok.mesh_id, "mesh-001");
        assert_eq!(tok.lighthouse, "10.0.0.5");
        assert_eq!(tok.port, 4242);
        assert_eq!(tok.bearer, "dGVzdC1iZWFyZXItYWJjZGVm");
        assert_eq!(tok.fp, None);
        assert_eq!(tok.encode(), raw);
    }

    // ---- ONBOARD-1: cert-fingerprint suffix -----------------

    #[test]
    fn parse_round_trips_a_token_with_fp() {
        let fp = "a".repeat(SHA256_HEX_LEN);
        let raw = format!("mesh:mesh-001@10.0.0.5:4242#bearer?fp={fp}");
        let tok = parse_join_token(&raw).expect("decoded");
        assert_eq!(tok.bearer, "bearer");
        assert_eq!(tok.fp.as_deref(), Some(fp.as_str()));
        // encode() reproduces the `?fp=` suffix verbatim.
        assert_eq!(tok.encode(), raw);
    }

    #[test]
    fn parse_rejects_malformed_fp() {
        // Wrong length (63), uppercase hex, and non-hex all reject.
        let short = "a".repeat(SHA256_HEX_LEN - 1);
        assert!(parse_join_token(&format!("mesh:m@10.0.0.5:4242#b?fp={short}")).is_none());
        let upper = "A".repeat(SHA256_HEX_LEN);
        assert!(parse_join_token(&format!("mesh:m@10.0.0.5:4242#b?fp={upper}")).is_none());
        let nonhex = "g".repeat(SHA256_HEX_LEN);
        assert!(parse_join_token(&format!("mesh:m@10.0.0.5:4242#b?fp={nonhex}")).is_none());
    }

    #[test]
    fn parse_rejects_empty_and_oversized() {
        assert!(parse_join_token("").is_none());
        let long = format!(
            "mesh:m@10.0.0.5:4242#{}",
            "a".repeat(JOIN_TOKEN_MAX_LEN + 10)
        );
        assert!(parse_join_token(&long).is_none());
    }

    #[test]
    fn parse_rejects_wrong_scheme() {
        assert!(parse_join_token("https://example.com").is_none());
        assert!(parse_join_token("not-a-token").is_none());
        assert!(parse_join_token("MESH:m@10.0.0.5:4242#b").is_none());
    }

    #[test]
    fn parse_rejects_invalid_port() {
        // 0 and out-of-range both reject.
        assert!(parse_join_token("mesh:m@10.0.0.5:0#b").is_none());
        assert!(parse_join_token("mesh:m@10.0.0.5:99999#b").is_none());
        assert!(parse_join_token("mesh:m@10.0.0.5:abc#b").is_none());
    }

    #[test]
    fn parse_rejects_non_ipv4_lighthouse() {
        // IPv6 + hostname rejected per the v2.5 IPv4-only lock.
        assert!(parse_join_token("mesh:m@fe80::1:4242#b").is_none());
        assert!(parse_join_token("mesh:m@example.com:4242#b").is_none());
    }

    #[test]
    fn parse_rejects_empty_components() {
        assert!(parse_join_token("mesh:@10.0.0.5:4242#b").is_none());
        assert!(parse_join_token("mesh:m@10.0.0.5:4242#").is_none());
        assert!(parse_join_token("mesh:m@:4242#b").is_none());
    }

    #[test]
    fn parse_rejects_unsafe_mesh_id() {
        // @ / : / # / / are reserved separators — must reject.
        assert!(parse_join_token("mesh:bad@id@10.0.0.5:4242#b").is_none());
        // / not allowed in the URL-safe set per the Python lock.
        assert!(parse_join_token("mesh:bad/id@10.0.0.5:4242#b").is_none());
    }

    #[test]
    fn parse_accepts_url_safe_mesh_id() {
        for mesh_id in ["m", "mesh-001", "mesh_001", "mesh.001", "Mesh-A1.b_2"] {
            let raw = format!("mesh:{mesh_id}@10.0.0.5:4242#bearer");
            assert!(parse_join_token(&raw).is_some(), "{mesh_id}");
        }
    }

    // ---- error message ergonomics ---------------------------

    #[test]
    fn invalid_token_error_quotes_length() {
        let err = EnrollError::InvalidToken { raw_len: 99 };
        let s = err.to_string();
        assert!(s.contains("invalid join token"));
        assert!(s.contains("length 99"));
        assert!(s.contains("mesh:"));
    }

    #[test]
    fn timeout_error_quotes_elapsed_and_recovery_hint() {
        let err = EnrollError::Timeout { elapsed_s: 30 };
        let s = err.to_string();
        assert!(s.contains("waited 30 s"));
        assert!(s.contains("mackesd ca sign-csr"));
    }

    #[test]
    fn publish_failed_error_quotes_reason() {
        let err = EnrollError::PublishFailed {
            reason: "permission denied".into(),
        };
        let s = err.to_string();
        assert!(s.contains("permission denied"));
        assert!(s.contains("QNM-Shared"));
    }

    #[test]
    fn bundle_corrupt_error_quotes_reason() {
        let err = EnrollError::BundleCorrupt {
            reason: "missing field `mesh_id`".into(),
        };
        let s = err.to_string();
        assert!(s.contains("missing field"));
        assert!(s.contains("MDE release"));
    }

    // ---- publish + path conventions -------------------------

    #[test]
    fn pending_enroll_path_mirrors_bundle_path_convention() {
        let root = Path::new("/qnm");
        let p = pending_enroll_path(root, "peer:anvil");
        assert_eq!(
            p,
            PathBuf::from("/qnm/peer:anvil/mackesd/pending-enroll.json")
        );
    }

    #[test]
    fn publish_writes_atomically_and_creates_parent() {
        let tmp = tempdir().expect("tempdir");
        let identity = build_identity();
        let token = parse_join_token("mesh:m@10.0.0.5:4242#bearer").unwrap();
        let pending = build_pending(&identity, "peer:anvil", "anvil", token);
        let written =
            publish_enrollment_request(tmp.path(), "peer:anvil", &pending).expect("publish");
        assert!(written.exists());
        let on_disk: PendingEnrollment =
            serde_json::from_slice(&std::fs::read(&written).unwrap()).unwrap();
        assert_eq!(on_disk.node_id, "peer:anvil");
        assert_eq!(on_disk.display_name, "anvil");
        assert_eq!(on_disk.public_key_hex.len(), 64);
    }

    #[test]
    fn publish_is_idempotent() {
        let tmp = tempdir().expect("tempdir");
        let identity = build_identity();
        let token = parse_join_token("mesh:m@10.0.0.5:4242#bearer").unwrap();
        let pending = build_pending(&identity, "peer:anvil", "anvil", token);
        let p1 = publish_enrollment_request(tmp.path(), "peer:anvil", &pending).unwrap();
        let p2 = publish_enrollment_request(tmp.path(), "peer:anvil", &pending).unwrap();
        assert_eq!(p1, p2);
        // Temp file shouldn't survive the atomic rename.
        let tmp_file = p2.with_extension("json.tmp");
        assert!(!tmp_file.exists());
    }

    // ---- wait_for_signed_bundle -----------------------------

    #[test]
    fn wait_returns_timeout_when_no_bundle_appears() {
        let tmp = tempdir().expect("tempdir");
        let r = wait_for_signed_bundle(
            tmp.path(),
            "peer:anvil",
            Duration::from_millis(50),
            Duration::from_millis(200),
        );
        match r {
            Err(EnrollError::Timeout { elapsed_s: _ }) => {} // OK
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn wait_returns_bundle_when_one_arrives() {
        use crate::ca::bundle::{write_bundle, LighthouseEntry, NebulaBundle};
        let tmp = tempdir().expect("tempdir");
        // Pre-place a valid bundle.
        let bundle = NebulaBundle {
            mesh_id: "m".into(),
            epoch: 0,
            ca_cert_pem: "CA".into(),
            peer_cert_pem: "CERT".into(),
            peer_key_pem: "KEY".into(),
            overlay_ip: "10.42.0.5".into(),
            mesh_cidr: "10.42.0.0/16".into(),
            lighthouses: vec![LighthouseEntry {
                node_id: "peer:lh".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "203.0.113.5:4242".into(),
            }],
            ca_key_pem: None,
            created_at: 1716000000,
        };
        write_bundle(&bundle_path(tmp.path(), "peer:anvil"), &bundle).expect("write");
        let (got, _waited) = wait_for_signed_bundle(
            tmp.path(),
            "peer:anvil",
            Duration::from_millis(50),
            Duration::from_secs(2),
        )
        .expect("ok");
        assert_eq!(got.overlay_ip, "10.42.0.5");
        assert_eq!(got.mesh_id, "m");
    }

    #[test]
    fn wait_returns_bundle_corrupt_on_invalid_json() {
        let tmp = tempdir().expect("tempdir");
        let p = bundle_path(tmp.path(), "peer:anvil");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{not valid").unwrap();
        let r = wait_for_signed_bundle(
            tmp.path(),
            "peer:anvil",
            Duration::from_millis(50),
            Duration::from_secs(1),
        );
        match r {
            Err(EnrollError::BundleCorrupt { .. }) => {}
            other => panic!("expected BundleCorrupt, got {other:?}"),
        }
    }

    // ---- end-to-end enroll_with_token -----------------------

    #[test]
    fn enroll_with_token_returns_invalid_for_garbage() {
        let tmp = tempdir().expect("tempdir");
        let r = enroll_with_token(tmp.path(), "peer:anvil", "anvil", "not a token");
        assert!(matches!(r, Err(EnrollError::InvalidToken { .. })));
    }

    // ---- sign_pending_csr (lighthouse side) -----------------

    use crate::ca::{mint, MockBackend};

    /// Test backend that mimics the real `nebula-cert sign` refusal to
    /// overwrite an existing cert file — the behaviour MockBackend lacks
    /// (it `fs::write`s unconditionally). Used to reproduce bed bug #7:
    /// a leftover scratch cert from a prior sign must not wedge re-signs.
    struct RefuseOverwriteBackend;
    impl crate::ca::NebulaCertBackend for RefuseOverwriteBackend {
        fn mint_ca(
            &self,
            mesh_id: &str,
            crt_out: &Path,
            key_out: &Path,
        ) -> Result<(), crate::ca::CaError> {
            MockBackend.mint_ca(mesh_id, crt_out, key_out)
        }
        #[allow(clippy::too_many_arguments)]
        fn sign_peer(
            &self,
            ca_crt: &Path,
            ca_key: &Path,
            node_id: &str,
            overlay_ip: &str,
            cidr_prefix: u8,
            groups: &[&str],
            crt_out: &Path,
            key_out: &Path,
        ) -> Result<(), crate::ca::CaError> {
            if crt_out.exists() {
                return Err(crate::ca::CaError::Io(format!(
                    "refusing to overwrite existing cert: {}",
                    crt_out.display()
                )));
            }
            MockBackend.sign_peer(
                ca_crt,
                ca_key,
                node_id,
                overlay_ip,
                cidr_prefix,
                groups,
                crt_out,
                key_out,
            )
        }
    }

    #[test]
    fn sign_csr_clears_stale_scratch_cert_before_resigning() {
        // Bed fix #7 regression: a leftover scratch cert from a PRIOR
        // sign of this same node must be cleared first — nebula-cert
        // hard-refuses to overwrite it, which otherwise wedges every
        // re-enroll / re-issue. RefuseOverwriteBackend models that
        // refusal; seed a stale `<node>.crt` and assert the sign still
        // succeeds (the fix removes it before signing).
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let _ = place_csr(tmp.path(), "peer:anvil");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).expect("mkdir scratch");
        // Leftover from a prior sign — exactly what nebula-cert chokes on.
        std::fs::write(scratch.join("peer:anvil.crt"), b"STALE LEFTOVER")
            .expect("seed stale scratch cert");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: scratch,
        };
        let outcome = sign_pending_csr(
            &RefuseOverwriteBackend,
            &conn,
            tmp.path(),
            "peer:anvil",
            "test-mesh",
            &paths,
            Vec::new(),
            false,
        )
        .expect("re-sign must clear the stale scratch cert and succeed");
        assert!(outcome.bundle_path.exists());
    }

    fn fresh_store() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        conn
    }

    /// Mint a CA + return paths to the CA cert/key in the
    /// per-test scratch dir. The CA exists at epoch 0 for the
    /// mesh "test-mesh" after this returns.
    fn make_test_ca(tmp_dir: &Path, conn: &rusqlite::Connection) -> (PathBuf, PathBuf) {
        let ca_crt = tmp_dir.join("ca.crt");
        let ca_key = tmp_dir.join("ca.key");
        mint::mint_ca(
            &MockBackend,
            conn,
            "test-mesh",
            Some(&ca_crt),
            Some(&ca_key),
        )
        .expect("mint");
        (ca_crt, ca_key)
    }

    /// Write a pending-enroll CSR under workgroup_root/peer_id/mackesd/
    /// with its bearer seeded as issued (ENT-1 — the happy path).
    fn place_csr(workgroup_root: &Path, peer_id: &str) -> PendingEnrollment {
        let identity = build_identity();
        let token = parse_join_token("mesh:test-mesh@10.0.0.5:4242#bearer").unwrap();
        crate::bearer_ledger::record_issued(workgroup_root, &token.bearer).expect("seed bearer");
        let pending = build_pending(&identity, peer_id, "anvil", token);
        publish_enrollment_request(workgroup_root, peer_id, &pending).expect("publish");
        pending
    }

    /// Like [`place_csr`] but the bearer is deliberately NOT issued —
    /// the ENT-1 refusal path.
    fn place_csr_unissued(workgroup_root: &Path, peer_id: &str) -> PendingEnrollment {
        let identity = build_identity();
        let token = parse_join_token("mesh:test-mesh@10.0.0.5:4242#forged").unwrap();
        let pending = build_pending(&identity, peer_id, "anvil", token);
        publish_enrollment_request(workgroup_root, peer_id, &pending).expect("publish");
        pending
    }

    /// #12 — a CSR whose bearer was issued with the LIGHTHOUSE role note (the
    /// turn-key full-lighthouse path). The signer must hand back the CA key.
    fn place_csr_lighthouse(workgroup_root: &Path, peer_id: &str) -> PendingEnrollment {
        let identity = build_identity();
        let lh_bearer =
            crate::bearer_ledger::issue(workgroup_root, crate::bearer_ledger::LIGHTHOUSE_ROLE_NOTE)
                .expect("issue lighthouse-scoped bearer");
        let token = parse_join_token(&format!("mesh:test-mesh@10.0.0.5:4242#{lh_bearer}")).unwrap();
        let pending = build_pending(&identity, peer_id, "anvil", token);
        publish_enrollment_request(workgroup_root, peer_id, &pending).expect("publish");
        pending
    }

    #[test]
    fn lighthouse_scoped_bearer_delivers_the_ca_key() {
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let pending = place_csr_lighthouse(tmp.path(), "peer:newlh");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let bundle = sign_csr_into_bundle(
            &MockBackend,
            &conn,
            tmp.path(),
            &pending,
            &paths,
            Vec::new(),
            false,
        )
        .expect("sign");
        let key = bundle
            .ca_key_pem
            .as_deref()
            .expect("a lighthouse-scoped bearer must deliver the CA key");
        assert!(!key.is_empty(), "the delivered CA key must be non-empty");
    }

    #[test]
    fn ordinary_peer_bearer_never_carries_the_ca_key() {
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let pending = place_csr(tmp.path(), "peer:anvil"); // plain peer bearer (note "recorded")
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let bundle = sign_csr_into_bundle(
            &MockBackend,
            &conn,
            tmp.path(),
            &pending,
            &paths,
            Vec::new(),
            false,
        )
        .expect("sign");
        assert!(
            bundle.ca_key_pem.is_none(),
            "an ordinary peer must NEVER receive the CA key (ENT-12 containment)"
        );
    }

    #[test]
    fn sign_csr_errors_when_pending_missing() {
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let r = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:absent",
            "test-mesh",
            &paths,
            Vec::new(),
            false,
        );
        match r {
            Err(SignCsrError::CsrMissing { path }) => {
                assert!(path.ends_with("peer:absent/mackesd/pending-enroll.json"));
            }
            other => panic!("expected CsrMissing, got {other:?}"),
        }
    }

    #[test]
    fn sign_csr_happy_path_writes_bundle_with_signed_cert() {
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let pending = place_csr(tmp.path(), "peer:anvil");
        let paths = SignCsrPaths {
            ca_crt: ca_crt.clone(),
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let lighthouses = vec![crate::ca::bundle::LighthouseEntry {
            node_id: "peer:lh".into(),
            overlay_ip: "10.42.0.1".into(),
            external_addr: "lh.example:4242".into(),
        }];
        let outcome = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:anvil",
            "test-mesh",
            &paths,
            lighthouses.clone(),
            false,
        )
        .expect("sign");
        assert_eq!(outcome.peer_id, "peer:anvil");
        assert!(outcome.overlay_ip.starts_with("10.42."));
        assert_eq!(outcome.epoch, 0);
        assert!(outcome.bundle_path.exists());
        // Bundle parses + contains the signed cert + the CA PEM
        // we just minted + the lighthouse roster we passed.
        let bundle = crate::ca::bundle::read_bundle(&outcome.bundle_path).expect("read");
        assert_eq!(bundle.mesh_id, "test-mesh");
        assert_eq!(bundle.overlay_ip, outcome.overlay_ip);
        assert_eq!(bundle.lighthouses, lighthouses);
        assert!(bundle.peer_cert_pem.contains("NEBULA"));
        assert!(bundle.ca_cert_pem.contains("NEBULA CA"));
        assert!(!bundle.peer_key_pem.is_empty());
        // Confirms the dropped CSR data round-tripped to the
        // bundle: same node_id is in nebula_peer_certs after
        // sign_peer_cert ran.
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = ?1",
                [&pending.node_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1);
    }

    #[test]
    fn sign_csr_ignores_advisory_mesh_hint_and_uses_token_mesh() {
        // Bed fix #5 regression: the caller's `_mesh_hint` is advisory and
        // MUST be ignored — the authoritative mesh is the one the peer's
        // join token declares (`csr.token.mesh_id` == "test-mesh", and the
        // CA is minted for "test-mesh"). Pass a BOGUS hint that has no CA;
        // under the old code this errored "no active CA for mesh
        // mesh-peer:bogus" (and silently broke the auto-signer). The sign
        // must now succeed and the bundle must carry the token's mesh.
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let _ = place_csr(tmp.path(), "peer:anvil");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let outcome = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:anvil",
            "mesh-peer:bogus", // advisory hint with no matching CA — ignored
            &paths,
            Vec::new(),
            false,
        )
        .expect("sign must use the token's mesh, not the bogus hint");
        let bundle = crate::ca::bundle::read_bundle(&outcome.bundle_path).expect("read");
        assert_eq!(bundle.mesh_id, "test-mesh");
    }

    #[test]
    fn sign_csr_refuses_unissued_bearer_ent1() {
        // ENT-1 acceptance: wrong / forged / absent bearer → refused
        // before any signing work, even with allow_override.
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let _ = place_csr_unissued(tmp.path(), "peer:forger");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let r = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:forger",
            "test-mesh",
            &paths,
            vec![],
            true, // the capacity override must NOT bypass auth
        );
        assert!(
            matches!(r, Err(SignCsrError::BearerNotIssued { ref node_id }) if node_id == "peer:forger"),
            "got {r:?}"
        );
    }

    #[test]
    fn sign_csr_bearer_is_single_use_ent1() {
        // ENT-1 acceptance: a valid bearer signs once; the replay of
        // the same bearer (a second CSR re-using it) is refused.
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        let _ = place_csr(tmp.path(), "peer:anvil"); // seeds "bearer"
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:anvil",
            "test-mesh",
            &paths,
            vec![],
            false,
        )
        .expect("first sign with the issued bearer");
        // A second box presents the SAME bearer (replay) — place the
        // CSR without re-seeding the ledger.
        let identity = build_identity();
        let token = parse_join_token("mesh:test-mesh@10.0.0.5:4242#bearer").unwrap();
        let pending = build_pending(&identity, "peer:replayer", "replayer", token);
        publish_enrollment_request(tmp.path(), "peer:replayer", &pending).expect("publish");
        let r = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:replayer",
            "test-mesh",
            &paths,
            vec![],
            false,
        );
        assert!(
            matches!(r, Err(SignCsrError::BearerNotIssued { .. })),
            "replayed bearer must be refused, got {r:?}"
        );
    }

    #[test]
    fn sign_csr_refuses_banned_node() {
        // EPIC-SEC-BANLIST — a banned node-id is refused at the sign
        // gate even with a valid pending CSR + an active CA, and even
        // when the peer cap has room. No override path.
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        place_csr(tmp.path(), "peer:evil");
        // Ban the node-id from a (different) peer's ban list — the
        // union check must still see it.
        crate::ca::ban_list::add_banned(tmp.path(), "peer:lh", "peer:evil").expect("ban");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let r = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:evil",
            "test-mesh",
            &paths,
            Vec::new(),
            // Even with override_cap = true, a ban is absolute.
            true,
        );
        match r {
            Err(SignCsrError::NodeBanned { node_id }) => {
                assert_eq!(node_id, "peer:evil");
            }
            other => panic!("expected NodeBanned, got {other:?}"),
        }
        // The banned node must NOT have landed a cert row.
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = ?1",
                ["peer:evil"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 0, "banned node must not be signed");
    }

    #[test]
    fn sign_csr_allows_unbanned_node_when_others_banned() {
        // A ban list containing OTHER node-ids must not block an
        // innocent peer.
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        place_csr(tmp.path(), "peer:anvil");
        crate::ca::ban_list::add_banned(tmp.path(), "peer:lh", "peer:someone-else").expect("ban");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let outcome = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:anvil",
            "test-mesh",
            &paths,
            Vec::new(),
            false,
        )
        .expect("innocent peer should sign");
        assert_eq!(outcome.peer_id, "peer:anvil");
    }

    #[test]
    fn sign_csr_errors_when_csr_is_garbage() {
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        // Place a malformed CSR (raw bytes that don't parse).
        let csr_path = pending_enroll_path(tmp.path(), "peer:anvil");
        std::fs::create_dir_all(csr_path.parent().unwrap()).unwrap();
        std::fs::write(&csr_path, "{not valid json").unwrap();
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let r = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:anvil",
            "test-mesh",
            &paths,
            Vec::new(),
            false,
        );
        assert!(matches!(r, Err(SignCsrError::CsrCorrupt { .. })));
    }

    #[test]
    fn sign_csr_errors_when_no_active_ca() {
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store(); // no mint_ca
        place_csr(tmp.path(), "peer:anvil");
        // Need SOMETHING at ca_crt path for the seal::read path
        // not to crash, but no SQL row means sign_peer_cert
        // refuses with "no active CA".
        let ca_crt = tmp.path().join("ca.crt");
        let ca_key = tmp.path().join("ca.key");
        std::fs::write(&ca_crt, "FAKE CA").unwrap();
        std::fs::write(&ca_key, "FAKE KEY").unwrap();
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let r = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:anvil",
            "test-mesh",
            &paths,
            Vec::new(),
            false,
        );
        match r {
            Err(SignCsrError::SignFailed { reason }) => {
                assert!(reason.contains("no active CA"), "reason: {reason}");
            }
            other => panic!("expected SignFailed, got {other:?}"),
        }
    }

    #[test]
    fn sign_csr_rejects_over_cap_peer_without_override() {
        // Pre-populate MAX_PEER_CAP peer certs at the active epoch, then
        // attempt to sign one more from scratch. The cap check fires
        // before any sign machinery runs. (§8: cap is 12 — 3 LH + 9 peers.)
        let cap = crate::ca::sign::MAX_PEER_CAP;
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        for i in 1..=cap {
            conn.execute(
                "INSERT INTO nebula_peer_certs \
                 (node_id, epoch, cert_pem, overlay_ip, expires_at) \
                 VALUES (?1, 0, 'pem', ?2, 9999999)",
                rusqlite::params![
                    format!("peer:slot-{i}"),
                    format!("10.42.{}.{}", i / 256, i % 256)
                ],
            )
            .unwrap();
        }
        let _pending = place_csr(tmp.path(), "peer:over");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let r = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:over",
            "test-mesh",
            &paths,
            Vec::new(),
            false,
        );
        match r {
            Err(SignCsrError::PeerCapReached {
                current,
                cap: reported,
            }) => {
                assert_eq!(current, cap);
                assert_eq!(reported, cap);
            }
            other => panic!("expected PeerCapReached, got {other:?}"),
        }
    }

    #[test]
    fn sign_csr_accepts_over_cap_peer_with_override() {
        let cap = crate::ca::sign::MAX_PEER_CAP;
        let tmp = tempdir().expect("tempdir");
        let conn = fresh_store();
        let (ca_crt, ca_key) = make_test_ca(tmp.path(), &conn);
        for i in 1..=cap {
            conn.execute(
                "INSERT INTO nebula_peer_certs \
                 (node_id, epoch, cert_pem, overlay_ip, expires_at) \
                 VALUES (?1, 0, 'pem', ?2, 9999999)",
                rusqlite::params![
                    format!("peer:slot-{i}"),
                    format!("10.42.{}.{}", i / 256, i % 256)
                ],
            )
            .unwrap();
        }
        let _pending = place_csr(tmp.path(), "peer:over");
        let paths = SignCsrPaths {
            ca_crt,
            ca_key,
            scratch_dir: tmp.path().join("scratch"),
        };
        let outcome = sign_pending_csr(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:over",
            "test-mesh",
            &paths,
            Vec::new(),
            true,
        )
        .expect("override path succeeds");
        assert_eq!(outcome.peer_id, "peer:over");
        // Row landed past the cap.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs \
                 WHERE node_id = 'peer:over' AND revoked_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn peer_cap_error_message_names_override_flag_and_doc() {
        let m = SignCsrError::PeerCapReached { current: 8, cap: 8 }.to_string();
        assert!(m.contains("8 peers"));
        assert!(m.contains("--override-cap"));
        assert!(m.contains("docs/design/cap-overrides.md"));
    }

    #[test]
    fn sign_outcome_error_messages_are_actionable() {
        let m = SignCsrError::CsrMissing {
            path: PathBuf::from("/qnm/peer:x/mackesd/pending-enroll.json"),
        }
        .to_string();
        assert!(m.contains("no pending-enroll CSR"));
        assert!(m.contains("mackesd enroll --token"));
        let s = SignCsrError::SignFailed { reason: "x".into() }.to_string();
        assert!(s.contains("`mackesd ca mint`"));
        let bw = SignCsrError::BundleWriteFailed {
            reason: "permission denied".into(),
        }
        .to_string();
        assert!(bw.contains("QNM-Shared mount"));
    }

    #[test]
    fn production_defaults_are_locked_paths() {
        let p = SignCsrPaths::production_defaults();
        assert_eq!(p.ca_crt, PathBuf::from("/etc/nebula/ca.crt"));
        assert_eq!(p.ca_key, PathBuf::from("/var/lib/mackesd/nebula-ca/ca.key"));
        assert_eq!(
            p.scratch_dir,
            PathBuf::from("/var/lib/mackesd/nebula-ca/scratch")
        );
    }
}

//! OW-13 — recovery + passive revocation.
//!
//! The design property: a reinstalled/replaced box re-enrolls **fresh** (a brand
//! new identity), and its OLD identity is *not* explicitly revoked with a `CRL` —
//! it is left to **expire** on its own short `TTL`. Because every cert is
//! short-lived and auto-renews before its lead-time cliff, recovery + node removal
//! need no `CRL` distribution and no private-key backup: the old cert simply lapses,
//! and the reinstalled box mints a new one.
//!
//! This module is the pure policy + injectable-seam core of that property. It
//! *orchestrates* the enrollment/revocation primitives the mesh already ships — it
//! does not re-implement any of them (§6):
//!
//! * **Short-`TTL` renewal** — [`plan_renewal`] decides `Renew | Ok | Expired` from
//!   a persisted cert expiry (`expires_at`, the field [`crate::ca::sign`] writes and
//!   [`crate::nebula_roster`] projects) + a caller-supplied `now` + a [`TtlPolicy`].
//!   Pure; the clock is passed in (the crate forbids ambient `Date::now()` in policy
//!   paths).
//! * **Passive revocation** — [`passive_revocation_status`] reports whether an old
//!   identity's cert has already lapsed (`Expired`) or is still within its short-`TTL`
//!   window (`StillValid`). For *immediate* removal, [`RecoveryApply::blocklist_old_identity`]
//!   records the old cert into the ENT-3 replicated blocklist (reuse
//!   [`crate::ca::blocklist`], the same machinery [`crate::leave`] uses).
//! * **Recovery re-enroll** — [`plan_recovery`] plans a reinstalled box's fresh
//!   enroll (mint a new identity via [`crate::enrollment::build_identity`], enroll
//!   fresh via [`crate::nebula_enroll`] / the [`crate::onboard::invite`] flow) while
//!   the old cert passively expires. The live re-enroll sits behind an injectable
//!   [`RecoveryApply`] seam; production [`LiveRecovery::reenroll`] is
//!   integration-gated (it needs the live CA signer + a reachable enroll endpoint)
//!   and returns a typed error — never a fake success (§7).
//!
//! # The shape mirrors the sibling onboard verbs (OW-7 / MV-7)
//! A pure planning core the unit tests pin, plus a thin injectable apply seam so the
//! live side effects are faked in tests and honestly integration-gated in
//! production — exactly as [`crate::onboard::spawn_lighthouse`] and
//! [`crate::adopt_xcp`] do. The `mackesd recovery` CLI verb (`bin/mackesd.rs`) is the
//! runtime face: it prints the plan + the old identity's passive-revocation status +
//! the renewal decision, `--dry-run`s the ordered steps, and drives the gated seam.

use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Part 1 — short-`TTL` renewal policy.
// ─────────────────────────────────────────────────────────────────────────────

/// The default short cert `TTL` a renewal mints, in seconds (24 h).
///
/// "Short" is the load-bearing property: a removed/reinstalled node's old cert
/// self-expires within a day, so passive revocation needs no `CRL` and recovery
/// needs no key-backup.
pub const DEFAULT_CERT_TTL_SECS: i64 = 24 * 60 * 60;

/// The default renewal lead time, in seconds (8 h — one third of the short `TTL`).
/// A cert is auto-renewed once it comes within this window of its expiry, so a live
/// node never reaches the expiry cliff.
pub const DEFAULT_RENEW_LEAD_SECS: i64 = 8 * 60 * 60;

/// The short-`TTL` auto-renewal policy: the `TTL` a renewal mints and the lead time
/// at which it renews. `Copy` + tiny so it is cheap to pass around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtlPolicy {
    /// The short `TTL` (seconds) a renewal mints onto the fresh cert.
    pub ttl_secs: i64,
    /// Renew once the cert is within this many seconds of its expiry.
    pub renew_lead_secs: i64,
}

impl TtlPolicy {
    /// The default short-`TTL` policy ([`DEFAULT_CERT_TTL_SECS`] /
    /// [`DEFAULT_RENEW_LEAD_SECS`]).
    #[must_use]
    pub const fn short_ttl() -> Self {
        Self {
            ttl_secs: DEFAULT_CERT_TTL_SECS,
            renew_lead_secs: DEFAULT_RENEW_LEAD_SECS,
        }
    }
}

impl Default for TtlPolicy {
    fn default() -> Self {
        Self::short_ttl()
    }
}

/// The renewal decision for a cert with a given expiry, relative to `now`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewDecision {
    /// Within the renewal lead time — auto-renew now. Carries the seconds left.
    Renew {
        /// Seconds remaining before the cert expires.
        remaining_secs: i64,
    },
    /// Still comfortably valid — nothing to do yet. Carries the seconds left.
    Ok {
        /// Seconds remaining before the cert expires.
        remaining_secs: i64,
    },
    /// Already past expiry — the cert has lapsed; re-enroll rather than renew.
    Expired {
        /// Seconds since the cert expired.
        overdue_secs: i64,
    },
}

impl RenewDecision {
    /// Whether the cert needs action (renew or re-enroll) rather than being fine.
    #[must_use]
    pub const fn needs_action(&self) -> bool {
        matches!(self, Self::Renew { .. } | Self::Expired { .. })
    }
}

/// Decide whether a cert should auto-renew before it expires.
///
/// Pure: the clock is passed in as `now` (Unix seconds). The `expires_at == 0`
/// epoch-lifetime sentinel (EFF-11 — [`crate::ca::sign`] writes it when a cert
/// carries no per-cert expiry) has nothing for a short-`TTL` policy to act on, so it
/// resolves to [`RenewDecision::Ok`] with a saturating remaining time.
#[must_use]
pub const fn plan_renewal(cert_expiry: i64, now: i64, policy: &TtlPolicy) -> RenewDecision {
    // The epoch-lifetime sentinel carries no short `TTL` — never renew it here.
    if cert_expiry <= 0 {
        return RenewDecision::Ok {
            remaining_secs: i64::MAX,
        };
    }
    let remaining = cert_expiry - now;
    if remaining <= 0 {
        return RenewDecision::Expired {
            overdue_secs: -remaining,
        };
    }
    if remaining <= policy.renew_lead_secs {
        RenewDecision::Renew {
            remaining_secs: remaining,
        }
    } else {
        RenewDecision::Ok {
            remaining_secs: remaining,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Part 2 — passive revocation (expire, don't `CRL`).
// ─────────────────────────────────────────────────────────────────────────────

/// The passive-revocation status of an old identity's cert, relative to `now`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationStatus {
    /// The old cert has not lapsed yet — it self-expires in `expires_in` seconds
    /// (no `CRL` needed; use the blocklist for immediate removal).
    StillValid {
        /// Seconds until the old cert self-expires.
        expires_in: i64,
    },
    /// The old cert has already lapsed — passive revocation is complete.
    Expired,
}

impl RevocationStatus {
    /// Whether the old cert has already passively expired.
    #[must_use]
    pub const fn is_expired(&self) -> bool {
        matches!(self, Self::Expired)
    }
}

/// Report whether an old identity's cert has passively expired yet.
///
/// Pure: `now` is passed in (Unix seconds). The `expires_at == 0` epoch-lifetime
/// sentinel never self-expires — passive revocation cannot reap it, so it reports
/// [`RevocationStatus::StillValid`] with a saturating window; the operator must use
/// [`RecoveryApply::blocklist_old_identity`] to remove it immediately.
#[must_use]
pub const fn passive_revocation_status(old_expiry: i64, now: i64) -> RevocationStatus {
    // The epoch-lifetime sentinel never self-expires — it needs the blocklist path.
    if old_expiry <= 0 {
        return RevocationStatus::StillValid {
            expires_in: i64::MAX,
        };
    }
    let remaining = old_expiry - now;
    if remaining <= 0 {
        RevocationStatus::Expired
    } else {
        RevocationStatus::StillValid {
            expires_in: remaining,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Part 3 — recovery re-enroll (plan + injectable seam).
// ─────────────────────────────────────────────────────────────────────────────

/// One ordered step of recovering a reinstalled box.
///
/// The steps *describe* the flow the real [`RecoveryApply`] drives over the existing
/// enroll/revocation primitives; each is phrased so a re-run on an already-recovered
/// box is a no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RecoveryStep {
    /// 1. Mint a fresh identity — a new `Ed25519` keypair + bearer via
    ///    [`crate::enrollment::build_identity`]. The old private key is gone with the
    ///    reinstall and is deliberately *not* needed (no key-backup).
    MintFreshIdentity,
    /// 2. Enroll fresh — present the join token to re-enroll via
    ///    [`crate::nebula_enroll::enroll_with_token`] (the [`crate::onboard::invite`]
    ///    flow); the lighthouse signs a new short-`TTL` cert.
    EnrollFresh,
    /// 3. The old cert is NOT `CRL`'d — its short `TTL` lapses on its own (passive
    ///    revocation). Immediate eviction via the blocklist is available if needed.
    OldIdentityLapses,
    /// 4. The fresh short-`TTL` cert auto-renews before each lead-time boundary
    ///    ([`plan_renewal`]), so the box stays up with no operator action.
    AutoRenewOngoing,
}

impl RecoveryStep {
    /// The canonical, ordered recovery sequence.
    #[must_use]
    pub fn ordered() -> Vec<Self> {
        vec![
            Self::MintFreshIdentity,
            Self::EnrollFresh,
            Self::OldIdentityLapses,
            Self::AutoRenewOngoing,
        ]
    }

    /// A one-line human description of the step.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::MintFreshIdentity => {
                "mint a fresh identity (new keypair + bearer via enrollment::build_identity) — the \
                 old private key is gone and is not needed (no key-backup)"
            }
            Self::EnrollFresh => {
                "enroll fresh via nebula_enroll::enroll_with_token (the onboard invite flow) — the \
                 lighthouse signs a new short-TTL cert"
            }
            Self::OldIdentityLapses => {
                "the old cert is NOT CRL'd — its short TTL lapses on its own (passive revocation); \
                 use --evict for immediate blocklist removal"
            }
            Self::AutoRenewOngoing => {
                "the fresh short-TTL cert auto-renews before each lead-time boundary (plan_renewal), \
                 so the box stays up with no operator action"
            }
        }
    }
}

/// Why a recovery cannot proceed right now — a real, retryable outcome the plan
/// carries (the operator fixes the blocker and retries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RecoveryBlockedReason {
    /// No fresh join token/invite to re-enroll with — the old bearer was single-use
    /// and is gone with the reinstall.
    NoJoinToken,
}

impl RecoveryBlockedReason {
    /// What the operator must fix before a retry succeeds.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::NoJoinToken => {
                "mint a fresh single-use invite on a lighthouse (`mackesd onboard invite`), then retry"
            }
        }
    }
}

impl std::fmt::Display for RecoveryBlockedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NoJoinToken => "no fresh join token / invite to re-enroll with",
        };
        f.write_str(s)
    }
}

/// The live facts [`gather`] reads off this node — the seam between the impure
/// probes and the pure [`plan_recovery`] fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryFacts {
    /// This mesh's id (from the founding bundle) — the box re-enrolls into THIS mesh.
    pub mesh_id: String,
    /// The old cert's persisted expiry (`expires_at`, Unix seconds) from the roster,
    /// if the node still has an active row. Drives the passive-revocation window +
    /// the renewal decision. `None` when the old cert isn't in the roster (already
    /// reaped or never present).
    pub old_cert_expiry: Option<i64>,
    /// Whether a fresh join token/invite is available to re-enroll with. `false` ⇒
    /// the plan is [`RecoveryPlan::Blocked`] (mint one, retry).
    pub join_token_present: bool,
}

/// A resolved recovery plan — the headless body the CLI prints and [`execute`]
/// drives.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum RecoveryPlan {
    /// The box can re-enroll: the mesh, the node id, and the ordered steps.
    Reenroll {
        /// The mesh the box re-enrolls into.
        mesh_id: String,
        /// The node id being recovered.
        node_id: String,
        /// The ordered recovery steps.
        steps: Vec<RecoveryStep>,
    },
    /// The recovery cannot proceed right now → a retryable blocked outcome once the
    /// [`RecoveryBlockedReason`]'s blocker clears.
    Blocked {
        /// Why recovery is blocked (and, via [`RecoveryBlockedReason::hint`], the fix).
        reason: RecoveryBlockedReason,
    },
}

impl RecoveryPlan {
    /// Whether a retry is available (always true for the blocked outcome — fix the
    /// blocker and retry).
    #[must_use]
    pub const fn retry_available(&self) -> bool {
        matches!(self, Self::Blocked { .. })
    }

    /// The ordered recovery steps (empty for a blocked plan) — the dry-run print.
    #[must_use]
    pub fn steps(&self) -> &[RecoveryStep] {
        match self {
            Self::Reenroll { steps, .. } => steps,
            Self::Blocked { .. } => &[],
        }
    }

    /// A one-line human summary (no trailing newline — the CLI wraps it in
    /// `println!`, mirroring the sibling verbs).
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Self::Blocked { reason } => {
                format!(
                    "cannot recover ({reason}) — retry available once you {}",
                    reason.hint()
                )
            }
            Self::Reenroll {
                mesh_id,
                node_id,
                steps,
            } => format!(
                "recover {node_id} by re-enrolling fresh into mesh `{mesh_id}` in {} step(s); the \
                 old identity is left to lapse (short-TTL passive revocation — no CRL, no key-backup)",
                steps.len()
            ),
        }
    }
}

/// Pure fold: turn a `node_id` + gathered [`RecoveryFacts`] into a [`RecoveryPlan`].
/// No I/O — fully unit-testable.
///
/// A box with no fresh join token cannot re-enroll (its old single-use bearer is
/// gone) and resolves to the retryable [`RecoveryPlan::Blocked`] outcome. Otherwise
/// the plan carries the ordered recovery steps.
#[must_use]
pub fn plan_recovery(node_id: &str, facts: &RecoveryFacts) -> RecoveryPlan {
    if !facts.join_token_present {
        return RecoveryPlan::Blocked {
            reason: RecoveryBlockedReason::NoJoinToken,
        };
    }
    RecoveryPlan::Reenroll {
        mesh_id: facts.mesh_id.clone(),
        node_id: node_id.to_string(),
        steps: RecoveryStep::ordered(),
    }
}

/// A request to re-enroll a reinstalled box — the input to
/// [`RecoveryApply::reenroll`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReenrollRequest {
    /// The mesh to re-enroll into.
    pub mesh_id: String,
    /// The node id being recovered.
    pub node_id: String,
    /// The ordered recovery steps to drive.
    pub steps: Vec<RecoveryStep>,
}

/// What a successful re-enroll produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReenrollReceipt {
    /// The recovered node id.
    pub node_id: String,
    /// The mesh it re-enrolled into.
    pub mesh_id: String,
    /// The overlay IP the fresh cert took.
    pub overlay_ip: String,
}

/// A request to immediately evict an old identity into the ENT-3 blocklist — the
/// optional fast path when you can't wait for the short-`TTL` cert to lapse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictRequest {
    /// The replicated QNM-Shared root the blocklist lives under.
    pub workgroup_root: PathBuf,
    /// The node id being evicted.
    pub node_id: String,
    /// The old cert's Nebula fingerprints (compute with [`fingerprint_old_cert`]).
    pub fingerprints: Vec<String>,
    /// Path to this node's persisted signing key — SEC-6-signs the retract when it
    /// loads, else the tolerant unsigned record is used. The CLI passes
    /// [`crate::node_key::DEFAULT_KEY_PATH`]; tests pass a temp path.
    pub node_key_path: PathBuf,
}

/// What a successful immediate eviction produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictReceipt {
    /// The blocklist record written under the replicated volume.
    pub blocklist_path: PathBuf,
    /// Whether the retract was SEC-6-signed (`false` = tolerant unsigned fallback).
    pub signed: bool,
}

/// A typed failure from the injectable [`RecoveryApply`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    /// The live path is not runnable in this build/environment yet — it needs a real
    /// prerequisite (the CA signer + a reachable enroll endpoint). Names the step +
    /// what is missing. §7-legal: a real method returning a real typed error, exactly
    /// as OW-7's / MV-7's apply seams do.
    IntegrationGated {
        /// Which seam step (`reenroll` / `blocklist`).
        step: &'static str,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// A step failed for a concrete runtime reason.
    Failed {
        /// Which seam step failed.
        step: &'static str,
        /// The failure detail.
        reason: String,
    },
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { step, reason } => {
                write!(f, "{step}: integration-gated — {reason}")
            }
            Self::Failed { step, reason } => write!(f, "{step}: {reason}"),
        }
    }
}

impl std::error::Error for RecoveryError {}

/// The injectable side-effect seam. Production is [`LiveRecovery`]; tests use a
/// recording fake so the pure orchestration is exercised without a real enroll /
/// blocklist write.
pub trait RecoveryApply {
    /// Re-enroll the reinstalled box fresh: mint a new identity and enroll it into
    /// the mesh (reuse [`crate::enrollment`] + [`crate::nebula_enroll`] /
    /// [`crate::onboard::invite`]) while the old cert passively expires. Returns the
    /// [`ReenrollReceipt`].
    ///
    /// # Errors
    /// A [`RecoveryError`] — `IntegrationGated` when the live re-enroll can't run yet
    /// (needs the CA signer + a reachable enroll endpoint), else `Failed`.
    fn reenroll(&self, req: &ReenrollRequest) -> Result<ReenrollReceipt, RecoveryError>;

    /// Optional immediate (hostile) eviction: record the old identity's cert
    /// fingerprints into the replicated ENT-3 blocklist (reuse
    /// [`crate::ca::blocklist`], the same machinery [`crate::leave`] uses) so peers
    /// drop its tunnels within a supervisor tick, instead of waiting for the
    /// short-`TTL` cert to passively lapse.
    ///
    /// # Errors
    /// A [`RecoveryError::Failed`] when there are no fingerprints to record or the
    /// blocklist write fails.
    fn blocklist_old_identity(&self, req: &EvictRequest) -> Result<EvictReceipt, RecoveryError>;
}

/// Production [`RecoveryApply`].
///
/// * [`reenroll`](RecoveryApply::reenroll) is honestly **integration-gated** — the
///   live re-enroll needs the CA signer + a reachable enroll endpoint, so it returns
///   a typed [`RecoveryError::IntegrationGated`], never a fake success (§7).
/// * [`blocklist_old_identity`](RecoveryApply::blocklist_old_identity) is **live** —
///   it genuinely reuses the shipped ENT-3 [`crate::ca::blocklist`] machinery to
///   record the retract (the same path [`crate::leave`] takes on exit).
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveRecovery;

impl RecoveryApply for LiveRecovery {
    fn reenroll(&self, req: &ReenrollRequest) -> Result<ReenrollReceipt, RecoveryError> {
        Err(RecoveryError::IntegrationGated {
            step: "reenroll",
            reason: format!(
                "needs the live CA signer + a reachable enroll endpoint (`mackesd enroll --token` / \
                 the ONBOARD-2 `/enroll`) to re-enroll {} fresh into `{}`",
                req.node_id, req.mesh_id
            ),
        })
    }

    fn blocklist_old_identity(&self, req: &EvictRequest) -> Result<EvictReceipt, RecoveryError> {
        if req.fingerprints.is_empty() {
            return Err(RecoveryError::Failed {
                step: "blocklist",
                reason: "no cert fingerprints to evict — compute them with `fingerprint_old_cert` \
                         from the old cert PEM first"
                    .to_string(),
            });
        }
        // Reuse the ENT-3 blocklist machinery: SEC-6-sign the retract when this
        // node's signing key loads, else fall back to the tolerant unsigned record
        // (exactly the signed-then-unsigned fallback `leave` uses on exit).
        let written = crate::node_key::load_or_create(&req.node_key_path).map_or_else(
            |_| {
                crate::ca::blocklist::record_revoked(
                    &req.workgroup_root,
                    &req.node_id,
                    &req.fingerprints,
                )
                .map(|path| (path, false))
            },
            |key| {
                crate::ca::blocklist::record_revoked_signed(
                    &req.workgroup_root,
                    &req.node_id,
                    &req.fingerprints,
                    &req.node_id,
                    &key,
                )
                .map(|path| (path, true))
            },
        );
        let (blocklist_path, signed) = written.map_err(|e| RecoveryError::Failed {
            step: "blocklist",
            reason: e.to_string(),
        })?;
        Ok(EvictReceipt {
            blocklist_path,
            signed,
        })
    }
}

/// The result of an [`execute`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// The box was re-enrolled fresh.
    Reenrolled {
        /// The receipt the re-enroll returned.
        receipt: ReenrollReceipt,
    },
    /// The plan was blocked — nothing was touched; a retry is available.
    Blocked {
        /// Why recovery was blocked.
        reason: RecoveryBlockedReason,
    },
}

/// Pure orchestration over the [`RecoveryApply`] seam.
///
/// For a [`RecoveryPlan::Reenroll`] drive the re-enroll; for [`RecoveryPlan::Blocked`]
/// short-circuit to the retryable outcome (no seam call). The optional immediate
/// eviction ([`RecoveryApply::blocklist_old_identity`]) is a separate operator action
/// (the CLI `--evict` flag), not part of this recovery orchestration — the default
/// path lets the old cert lapse passively.
///
/// # Errors
/// Propagates the [`RecoveryError`] the re-enroll returns (integration-gated in
/// production).
pub fn execute(
    plan: &RecoveryPlan,
    apply: &dyn RecoveryApply,
) -> Result<RecoveryOutcome, RecoveryError> {
    match plan {
        RecoveryPlan::Blocked { reason } => Ok(RecoveryOutcome::Blocked { reason: *reason }),
        RecoveryPlan::Reenroll {
            mesh_id,
            node_id,
            steps,
        } => {
            let receipt = apply.reenroll(&ReenrollRequest {
                mesh_id: mesh_id.clone(),
                node_id: node_id.clone(),
                steps: steps.clone(),
            })?;
            Ok(RecoveryOutcome::Reenrolled { receipt })
        }
    }
}

/// Impure probe shell: gather the live recovery facts off this node.
///
/// Reuses [`crate::onboard::invite::resolve_mesh_id`] for the mesh id and reads the
/// old cert's persisted `expires_at` out of an exported [`crate::nebula_roster`]
/// (the field [`crate::ca::sign`] persists) via [`old_cert_expiry_of`]. Whether a
/// fresh join token is present is decided by the caller (the CLI keys it off the
/// `--token` flag) and passed through.
#[must_use]
pub fn gather(
    workgroup_root: &Path,
    node_id: &str,
    roster: &[crate::nebula_roster::RosterRow],
    join_token_present: bool,
) -> RecoveryFacts {
    RecoveryFacts {
        mesh_id: crate::onboard::invite::resolve_mesh_id(workgroup_root, node_id),
        old_cert_expiry: old_cert_expiry_of(roster, node_id),
        join_token_present,
    }
}

/// Read the persisted cert expiry (`expires_at`, Unix seconds) for `node_id`.
///
/// Reuses the field [`crate::ca::sign`] writes and [`crate::nebula_roster::export_roster`]
/// projects out of an exported roster. `None` when the node has no active roster row.
#[must_use]
pub fn old_cert_expiry_of(
    roster: &[crate::nebula_roster::RosterRow],
    node_id: &str,
) -> Option<i64> {
    roster
        .iter()
        .find(|r| r.node_id == node_id)
        .map(|r| r.expires_at)
}

/// Fingerprint an old cert PEM for the immediate-eviction blocklist path.
///
/// A thin reuse of [`crate::ca::blocklist::fingerprint_cert_pem`] (the authoritative
/// `nebula-cert` fingerprint format). `None` when `nebula-cert` is unavailable or the
/// PEM can't be parsed, so callers surface an honest failure rather than a fake
/// eviction.
#[must_use]
pub fn fingerprint_old_cert(cert_pem: &str) -> Option<Vec<String>> {
    crate::ca::blocklist::fingerprint_cert_pem(cert_pem).map(|fp| vec![fp])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // A representative "now" — well past the Unix epoch, like a real clock.
    const NOW: i64 = 1_800_000_000;

    // ---- Part 1: plan_renewal --------------------------------------------

    #[test]
    fn renewal_renews_at_and_within_the_lead_time_boundary() {
        let policy = TtlPolicy::short_ttl();
        let lead = policy.renew_lead_secs;
        // Exactly at the lead-time boundary → renew.
        assert_eq!(
            plan_renewal(NOW + lead, NOW, &policy),
            RenewDecision::Renew {
                remaining_secs: lead
            }
        );
        // One second inside the lead window → renew.
        assert_eq!(
            plan_renewal(NOW + lead - 1, NOW, &policy),
            RenewDecision::Renew {
                remaining_secs: lead - 1
            }
        );
    }

    #[test]
    fn renewal_is_ok_before_the_lead_time() {
        let policy = TtlPolicy::short_ttl();
        let lead = policy.renew_lead_secs;
        // One second before the lead window opens → still ok.
        assert_eq!(
            plan_renewal(NOW + lead + 1, NOW, &policy),
            RenewDecision::Ok {
                remaining_secs: lead + 1
            }
        );
        // Full TTL remaining → ok, and it does NOT need action.
        let d = plan_renewal(NOW + policy.ttl_secs, NOW, &policy);
        assert_eq!(
            d,
            RenewDecision::Ok {
                remaining_secs: policy.ttl_secs
            }
        );
        assert!(!d.needs_action());
    }

    #[test]
    fn renewal_reports_expired_at_and_past_the_boundary() {
        let policy = TtlPolicy::short_ttl();
        // Exactly at expiry (remaining == 0) is already expired.
        assert_eq!(
            plan_renewal(NOW, NOW, &policy),
            RenewDecision::Expired { overdue_secs: 0 }
        );
        // Five seconds past expiry.
        let d = plan_renewal(NOW - 5, NOW, &policy);
        assert_eq!(d, RenewDecision::Expired { overdue_secs: 5 });
        assert!(d.needs_action());
    }

    #[test]
    fn renewal_treats_the_epoch_lifetime_sentinel_as_ok() {
        // expires_at == 0 (and any non-positive) is the EFF-11 epoch-lifetime
        // sentinel — a short-TTL policy has nothing to renew.
        let policy = TtlPolicy::short_ttl();
        assert_eq!(
            plan_renewal(0, NOW, &policy),
            RenewDecision::Ok {
                remaining_secs: i64::MAX
            }
        );
        assert_eq!(
            plan_renewal(-1, NOW, &policy),
            RenewDecision::Ok {
                remaining_secs: i64::MAX
            }
        );
    }

    #[test]
    fn renewal_honors_a_custom_lead_time() {
        let policy = TtlPolicy {
            ttl_secs: 3600,
            renew_lead_secs: 600,
        };
        // 700s left > 600s lead → ok.
        assert!(matches!(
            plan_renewal(NOW + 700, NOW, &policy),
            RenewDecision::Ok { .. }
        ));
        // 600s left == lead → renew.
        assert!(matches!(
            plan_renewal(NOW + 600, NOW, &policy),
            RenewDecision::Renew { .. }
        ));
    }

    // ---- Part 2: passive_revocation_status -------------------------------

    #[test]
    fn passive_revocation_still_valid_before_expiry() {
        assert_eq!(
            passive_revocation_status(NOW + 3600, NOW),
            RevocationStatus::StillValid { expires_in: 3600 }
        );
        assert!(!passive_revocation_status(NOW + 3600, NOW).is_expired());
    }

    #[test]
    fn passive_revocation_expired_at_and_past_the_boundary() {
        // Exactly at expiry counts as expired (passive revocation complete).
        assert_eq!(
            passive_revocation_status(NOW, NOW),
            RevocationStatus::Expired
        );
        assert!(passive_revocation_status(NOW - 1, NOW).is_expired());
    }

    #[test]
    fn passive_revocation_sentinel_never_self_expires() {
        // The epoch-lifetime sentinel never lapses — it stays valid (the operator
        // must use the blocklist path to reap it immediately).
        assert_eq!(
            passive_revocation_status(0, NOW),
            RevocationStatus::StillValid {
                expires_in: i64::MAX
            }
        );
    }

    // ---- Part 3: plan_recovery -------------------------------------------

    fn facts(join_token_present: bool, old_cert_expiry: Option<i64>) -> RecoveryFacts {
        RecoveryFacts {
            mesh_id: "home-deadbeef".to_string(),
            old_cert_expiry,
            join_token_present,
        }
    }

    #[test]
    fn plan_recovery_with_a_token_plans_a_fresh_reenroll() {
        let plan = plan_recovery("peer:anvil", &facts(true, Some(NOW + 3600)));
        match &plan {
            RecoveryPlan::Reenroll {
                mesh_id,
                node_id,
                steps,
            } => {
                assert_eq!(mesh_id, "home-deadbeef");
                assert_eq!(node_id, "peer:anvil");
                assert_eq!(steps, &RecoveryStep::ordered());
            }
            RecoveryPlan::Blocked { .. } => panic!("expected a reenroll plan"),
        }
        assert!(!plan.retry_available());
        assert_eq!(plan.steps().len(), 4);
        assert!(plan.human().contains("no CRL, no key-backup"));
    }

    #[test]
    fn plan_recovery_without_a_token_is_blocked_with_retry() {
        let plan = plan_recovery("peer:anvil", &facts(false, None));
        assert_eq!(
            plan,
            RecoveryPlan::Blocked {
                reason: RecoveryBlockedReason::NoJoinToken
            }
        );
        assert!(plan.retry_available());
        assert!(plan.steps().is_empty());
        assert!(plan.human().contains("mackesd onboard invite"));
    }

    #[test]
    fn recovery_steps_are_ordered_and_stable() {
        let steps = RecoveryStep::ordered();
        assert_eq!(
            steps,
            vec![
                RecoveryStep::MintFreshIdentity,
                RecoveryStep::EnrollFresh,
                RecoveryStep::OldIdentityLapses,
                RecoveryStep::AutoRenewOngoing,
            ],
            "the recovery order is fixed"
        );
        // Mint the fresh identity before enrolling it.
        let mint = steps
            .iter()
            .position(|s| *s == RecoveryStep::MintFreshIdentity)
            .unwrap();
        let enroll = steps
            .iter()
            .position(|s| *s == RecoveryStep::EnrollFresh)
            .unwrap();
        assert!(mint < enroll, "mint the identity before enrolling");
        // Every step has a non-empty description.
        assert!(steps.iter().all(|s| !s.describe().is_empty()));
    }

    #[test]
    fn recovery_plan_serializes() {
        // The plan is Serialize (the CLI can render it); exercise the path.
        let plan = plan_recovery("peer:anvil", &facts(true, None));
        let json = serde_json::to_string(&plan).expect("serialize");
        assert!(json.contains("Reenroll"));
        assert!(json.contains("MintFreshIdentity"));
    }

    // ---- gather + roster helpers -----------------------------------------

    fn roster_row(node_id: &str, expires_at: i64) -> crate::nebula_roster::RosterRow {
        crate::nebula_roster::RosterRow {
            node_id: node_id.to_string(),
            name: node_id.to_string(),
            overlay_ip: "10.42.0.5".to_string(),
            epoch: 0,
            cert_pem: "-----BEGIN NEBULA CERTIFICATE-----\nX\n-----END NEBULA CERTIFICATE-----\n"
                .to_string(),
            created_at: NOW - 3600,
            expires_at,
            groups: "peer".to_string(),
        }
    }

    #[test]
    fn old_cert_expiry_reads_the_persisted_expires_at() {
        let roster = vec![
            roster_row("peer:anvil", NOW + 1234),
            roster_row("peer:birch", 999),
        ];
        assert_eq!(old_cert_expiry_of(&roster, "peer:anvil"), Some(NOW + 1234));
        assert_eq!(old_cert_expiry_of(&roster, "peer:birch"), Some(999));
        // A node with no active row → None.
        assert_eq!(old_cert_expiry_of(&roster, "peer:cedar"), None);
    }

    #[test]
    fn gather_resolves_mesh_id_and_old_cert_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let roster = vec![roster_row("peer:anvil", NOW + 42)];
        let f = gather(tmp.path(), "peer:anvil", &roster, true);
        // No founding bundle in the temp root → resolve_mesh_id falls back to the
        // node default (reused, not reinvented).
        assert_eq!(f.mesh_id, "mesh-peer:anvil");
        assert_eq!(f.old_cert_expiry, Some(NOW + 42));
        assert!(f.join_token_present);
    }

    #[test]
    fn fingerprint_old_cert_is_none_for_an_unparseable_pem() {
        // An empty / bogus PEM can't be fingerprinted (nebula-cert can't print it),
        // so the immediate-evict path fails honestly rather than faking a fingerprint.
        assert!(fingerprint_old_cert("").is_none());
    }

    // ---- execute + the RecoveryApply seam --------------------------------

    /// Recording [`RecoveryApply`] fake: records the ordered calls so the pure
    /// orchestration is asserted without a real enroll / blocklist write.
    struct RecordingApply {
        calls: RefCell<Vec<String>>,
        seen_req: RefCell<Option<ReenrollRequest>>,
    }

    impl RecordingApply {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                seen_req: RefCell::new(None),
            }
        }
    }

    impl RecoveryApply for RecordingApply {
        fn reenroll(&self, req: &ReenrollRequest) -> Result<ReenrollReceipt, RecoveryError> {
            self.calls.borrow_mut().push("reenroll".to_string());
            *self.seen_req.borrow_mut() = Some(req.clone());
            Ok(ReenrollReceipt {
                node_id: req.node_id.clone(),
                mesh_id: req.mesh_id.clone(),
                overlay_ip: "10.42.0.7".to_string(),
            })
        }
        fn blocklist_old_identity(
            &self,
            _req: &EvictRequest,
        ) -> Result<EvictReceipt, RecoveryError> {
            self.calls.borrow_mut().push("blocklist".to_string());
            Ok(EvictReceipt {
                blocklist_path: PathBuf::from("/dev/null"),
                signed: false,
            })
        }
    }

    #[test]
    fn execute_drives_the_reenroll_for_a_plan() {
        let plan = plan_recovery("peer:anvil", &facts(true, Some(NOW + 10)));
        let apply = RecordingApply::new();
        let outcome = execute(&plan, &apply).expect("execute");
        match outcome {
            RecoveryOutcome::Reenrolled { receipt } => {
                assert_eq!(receipt.node_id, "peer:anvil");
                assert_eq!(receipt.mesh_id, "home-deadbeef");
                assert_eq!(receipt.overlay_ip, "10.42.0.7");
            }
            RecoveryOutcome::Blocked { .. } => panic!("expected a re-enrolled outcome"),
        }
        // Only the re-enroll ran (the blocklist eviction is a separate action).
        assert_eq!(*apply.calls.borrow(), vec!["reenroll"]);
        assert_eq!(
            apply.seen_req.borrow().as_ref().map(|r| r.steps.clone()),
            Some(RecoveryStep::ordered())
        );
    }

    #[test]
    fn execute_short_circuits_a_blocked_plan() {
        let plan = plan_recovery("peer:anvil", &facts(false, None));
        let apply = RecordingApply::new();
        let outcome = execute(&plan, &apply).expect("execute");
        assert_eq!(
            outcome,
            RecoveryOutcome::Blocked {
                reason: RecoveryBlockedReason::NoJoinToken
            }
        );
        assert!(apply.calls.borrow().is_empty(), "no seam call when blocked");
    }

    #[test]
    fn live_reenroll_is_integration_gated_not_fake_success() {
        let apply = LiveRecovery;
        let err = apply
            .reenroll(&ReenrollRequest {
                mesh_id: "home-deadbeef".to_string(),
                node_id: "peer:anvil".to_string(),
                steps: RecoveryStep::ordered(),
            })
            .expect_err("live re-enroll must not fake success");
        match err {
            RecoveryError::IntegrationGated { step, reason } => {
                assert_eq!(step, "reenroll");
                assert!(reason.contains("CA signer"), "reason names the CA signer");
                assert!(
                    reason.contains("enroll endpoint"),
                    "reason names the endpoint"
                );
            }
            RecoveryError::Failed { .. } => panic!("expected an integration-gated error"),
        }
    }

    #[test]
    fn execute_propagates_the_integration_gated_error() {
        let plan = plan_recovery("peer:anvil", &facts(true, None));
        let err = execute(&plan, &LiveRecovery).expect_err("live path is gated");
        assert!(matches!(
            err,
            RecoveryError::IntegrationGated {
                step: "reenroll",
                ..
            }
        ));
    }

    // ---- immediate eviction (reuse ca::blocklist) ------------------------

    const FP: &str = "abababababababababababababababababababababababababababababababab";

    #[test]
    fn live_blocklist_old_identity_really_records_the_retract() {
        // The immediate-evict path genuinely reuses the shipped ENT-3 blocklist —
        // it writes a signed retract that unions into all_fingerprints.
        let tmp = tempfile::tempdir().unwrap();
        let req = EvictRequest {
            workgroup_root: tmp.path().to_path_buf(),
            node_id: "peer:anvil".to_string(),
            fingerprints: vec![FP.to_string()],
            node_key_path: tmp.path().join("node-signing.key"),
        };
        let receipt = LiveRecovery
            .blocklist_old_identity(&req)
            .expect("blocklist write");
        assert!(receipt.blocklist_path.exists());
        assert!(receipt.signed, "a loadable key SEC-6-signs the retract");
        // The retract is now part of the mesh-wide blocklist union.
        assert_eq!(crate::ca::blocklist::all_fingerprints(tmp.path()), vec![FP]);
    }

    #[test]
    fn live_blocklist_old_identity_rejects_an_empty_fingerprint_set() {
        let tmp = tempfile::tempdir().unwrap();
        let req = EvictRequest {
            workgroup_root: tmp.path().to_path_buf(),
            node_id: "peer:anvil".to_string(),
            fingerprints: Vec::new(),
            node_key_path: tmp.path().join("node-signing.key"),
        };
        match LiveRecovery.blocklist_old_identity(&req) {
            Err(RecoveryError::Failed { step, .. }) => assert_eq!(step, "blocklist"),
            other => panic!("expected a Failed error, got {other:?}"),
        }
    }
}

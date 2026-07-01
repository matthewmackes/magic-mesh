//! MV-7 — `mackesd adopt-xcp`: day-2 **adopt an existing XCP-ng host** into this
//! mesh, without rebuilding it as a Fedora+KVM node.
//!
//! Enroll its `dom0` as a **static Nebula member** (a plain overlay member —
//! *not* a deployment role) and drive its `XAPI` toolstack via `xe`/`tofu` "as the
//! live farm does" (the `infra/tofu/xen-xapi` path), so the box serves VMs to the
//! mesh.
//!
//! # Why this verb exists
//! The KVM catalog (MV-1) is the *green-field* virtualization story — a fresh
//! Fedora node. But the live farm already runs `XCP-ng` dom0s serving VMs. Rather
//! than reprovision them, MV-7 *adopts* one in place: it joins the overlay as a
//! pinned member so mesh peers can reach it, and its existing toolstack is driven
//! through the same `xe`/`tofu` provider the farm's `IaC` uses. It takes **no role**
//! — a role runs mackesd's worker set, which an adopted hypervisor does not; it is
//! a static Nebula member that *hosts* VMs.
//!
//! # The shape mirrors the sibling onboard verbs (OW-5 / OW-7)
//! A pure planning core the unit tests pin, plus a thin **injectable apply seam** so
//! the live side effects are faked in tests and honestly integration-gated in
//! production.
//! * [`gather`] — impure probe: reads the mesh-id + the CA-holder overlay IP (the
//!   founding bundle) and whether the target host credential resolves.
//! * [`plan_adopt`] — pure fold: `[AdoptTarget] + [AdoptFacts] → [AdoptPlan]`,
//!   yielding the ordered, idempotent [`AdoptStep`]s — or the retryable
//!   [`AdoptPlan::Blocked`] outcome when a prerequisite is absent.
//! * [`Adopter`] — the injectable side-effect seam ([`Adopter::enroll_member`] →
//!   [`Adopter::drive_toolstack`]). Production [`LiveAdopter`] returns a typed
//!   [`AdoptError::IntegrationGated`] naming exactly what the live call needs (the
//!   CA signer + live SSH, then live `xe`/`tofu` + host creds); tests drive a
//!   recording fake.
//! * [`execute`] — pure orchestration over the seam (enroll → drive-toolstack, in
//!   that order), fully unit-tested through the fake.
//!
//! # Reuse, not reimplementation (§6)
//! This verb is glue over mechanisms the mesh already has:
//! * The **identity/CA** is [`mackesd_core`](crate)'s existing signer — a
//!   member-scoped enroll ([`crate::nebula_enroll`]) signs the dom0's Host cert off
//!   this node's CA ([`crate::ca`]). We do not re-derive any crypto; the ordered
//!   [`AdoptStep`]s *describe* that flow so the real [`Adopter`] drives it.
//! * The **peer roster** row is [`mackes_mesh_types::peers::PeerRecord`] — the same
//!   replicated directory every other node publishes to; adoption records the dom0
//!   there so peers discover it.
//! * The live toolstack apply is the farm's existing `xen-xapi` `tofu` provider
//!   (driven over `xe`), which the real [`Adopter`] shells to.
//!
//! # This slice (MV-7): the pure core + the injectable seam — NOT the live infra
//! The live member enroll / SSH materialize / real `xe`/`tofu` apply land behind
//! [`Adopter`], exactly as OW-5's live `nmcli` apply sits behind
//! [`crate::onboard::network::KeyfileSink`] and OW-7's live provision sits behind
//! [`crate::onboard::spawn_lighthouse::Provisioner`]. [`LiveAdopter`] returning a
//! typed `IntegrationGated` error (never a fake success) is §7-legal.

use std::path::Path;

/// The existing XCP-ng host to adopt — its connection info plus a credential
/// **handle** (never the secret itself).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdoptTarget {
    /// The `XCP-ng` pool master / `dom0` address `xe` and peers reach it at — a
    /// hostname or IP (e.g. `172.20.0.9`).
    pub pool_address: String,
    /// The overlay IP the `dom0` takes as a **static** Nebula member once enrolled
    /// (a pinned member address, e.g. `10.42.0.9`).
    pub overlay_ip: String,
    /// A **handle** to the host root credential (e.g. `secret:xcp-host`), never the
    /// secret itself — resolved at apply time by the injected [`Adopter`]. The pure
    /// plan never carries a secret.
    pub credential_ref: String,
}

/// The live facts [`gather`] reads off this node — the seam between the impure
/// probes and the pure [`plan_adopt`] fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptFacts {
    /// This mesh's id (from the founding bundle) — the host joins THIS mesh.
    pub mesh_id: String,
    /// This node's overlay IP if it holds the CA (is founded) — the signer that
    /// mints the member's Host cert. `None` ⇒ nothing can sign the adoption.
    pub ca_holder_overlay_ip: Option<String>,
    /// Whether the target's [`AdoptTarget::credential_ref`] actually resolves.
    /// `false` ⇒ `xe`/`tofu` can't reach the host ⇒ the retryable blocked branch.
    pub credential_present: bool,
}

/// Why an adoption cannot proceed right now — a real, retryable outcome the plan
/// carries (the operator fixes the blocker and retries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum BlockedReason {
    /// This node has not founded a mesh, so it holds no CA to sign the member.
    NotFounded,
    /// The target's credential handle does not resolve — `xe`/`tofu` cannot reach
    /// the host.
    NoCredential,
}

impl BlockedReason {
    /// What the operator must fix before a retry succeeds.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::NotFounded => "found this mesh first (`mackesd onboard mesh-create`), then retry",
            Self::NoCredential => {
                "provide the host root credential the handle points at, then retry"
            }
        }
    }
}

impl std::fmt::Display for BlockedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NotFounded => "no founded mesh / CA on this node",
            Self::NoCredential => "the host credential handle does not resolve",
        };
        f.write_str(s)
    }
}

/// One ordered, idempotent step of adopting the host.
///
/// The steps *describe* the flow the real [`Adopter`] drives (enroll the `dom0` as
/// a static Nebula member, then drive its toolstack); each is phrased so a re-run on
/// an already-adopted host is a no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AdoptStep {
    /// 1. Mint a **static-member** join bearer on the CA holder — a plain overlay
    ///    member scope (NOT lighthouse, NOT a deployment role): it authorizes a
    ///    Host-cert enroll only, so the `dom0` joins the overlay without taking a
    ///    role or the CA key. Idempotent: reuse an outstanding member bearer.
    MintMemberToken,
    /// 2. Enroll the `dom0` as a static Nebula member: sign its Host cert off this
    ///    node's CA and materialize `/etc/nebula` pinned to this mesh's lighthouses
    ///    (`static_host_map`) at the target's fixed overlay IP. No-op if it is
    ///    already enrolled at that IP.
    EnrollStaticMember,
    /// 3. Record the `dom0` as an adopted-XCP [`mackes_mesh_types::peers::PeerRecord`]
    ///    roster row (its overlay IP + the `xcp` note) so peers discover it. No-op
    ///    if the row already matches.
    RegisterInRoster,
    /// 4. Attach to the `dom0`'s `XAPI` toolstack: reach `xe` against the pool
    ///    master with the resolved credential (the same handle the live farm's
    ///    `xen-xapi` `tofu` provider uses). No-op if already reachable.
    AttachToolstack,
    /// 5. Drive `xe`/`tofu` to converge the host's VM-serving baseline (the guest
    ///    network on the overlay bridge + the mesh SR) as the live farm does, so it
    ///    serves VMs to the mesh. Idempotent `tofu` apply.
    ConvergeToolstack,
}

impl AdoptStep {
    /// The canonical, ordered adopt sequence.
    #[must_use]
    pub fn ordered() -> Vec<Self> {
        vec![
            Self::MintMemberToken,
            Self::EnrollStaticMember,
            Self::RegisterInRoster,
            Self::AttachToolstack,
            Self::ConvergeToolstack,
        ]
    }

    /// A one-line human description of the step.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::MintMemberToken => {
                "mint a static-member join token (a plain overlay member scope — not lighthouse, \
                 not a role)"
            }
            Self::EnrollStaticMember => {
                "enroll the dom0 as a static Nebula member: sign its Host cert + materialize \
                 /etc/nebula pinned to the lighthouses"
            }
            Self::RegisterInRoster => {
                "record it as an adopted-XCP peer roster row so peers discover it"
            }
            Self::AttachToolstack => {
                "attach to the dom0's XAPI toolstack (reach xe against the pool master)"
            }
            Self::ConvergeToolstack => {
                "drive xe/tofu to converge the VM-serving baseline (overlay bridge + mesh SR), as \
                 the live farm does"
            }
        }
    }
}

/// A resolved adopt plan — the headless body the CLI prints and [`execute`] drives.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum AdoptPlan {
    /// The host can be adopted: the mesh it joins, the target, and the ordered,
    /// idempotent steps (enroll the static member → drive the toolstack).
    Adopt {
        /// The mesh the host joins.
        mesh_id: String,
        /// The host being adopted.
        target: AdoptTarget,
        /// The ordered, idempotent adopt steps.
        steps: Vec<AdoptStep>,
    },
    /// The adoption cannot proceed right now → a retryable blocked outcome once the
    /// [`BlockedReason`]'s blocker clears.
    Blocked {
        /// Why adoption is blocked (and, via [`BlockedReason::hint`], the fix).
        reason: BlockedReason,
    },
}

impl AdoptPlan {
    /// Whether a retry is available (always true for the blocked outcome — fix the
    /// blocker and retry).
    #[must_use]
    pub const fn retry_available(&self) -> bool {
        matches!(self, Self::Blocked { .. })
    }

    /// The ordered adopt steps (empty for a blocked plan) — the dry-run print.
    #[must_use]
    pub fn steps(&self) -> &[AdoptStep] {
        match self {
            Self::Adopt { steps, .. } => steps,
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
                    "cannot adopt ({reason}) — retry available once you {}",
                    reason.hint()
                )
            }
            Self::Adopt {
                mesh_id,
                target,
                steps,
            } => format!(
                "adopt XCP-ng host {} into mesh `{mesh_id}` as a static Nebula member at {}, then \
                 drive its toolstack in {} step(s)",
                target.pool_address,
                target.overlay_ip,
                steps.len()
            ),
        }
    }
}

/// Pure fold: turn an [`AdoptTarget`] + gathered [`AdoptFacts`] into an
/// [`AdoptPlan`]. No I/O — fully unit-testable.
///
/// An un-founded node (no CA to sign the member) or an unresolvable host credential
/// (no way to reach `xe`/`tofu`) resolve to the retryable [`AdoptPlan::Blocked`]
/// outcome. Otherwise the plan carries the ordered adopt steps.
#[must_use]
pub fn plan_adopt(target: &AdoptTarget, facts: &AdoptFacts) -> AdoptPlan {
    // Adoption signs the dom0's Host cert off THIS node's CA, so it must be founded.
    if facts.ca_holder_overlay_ip.is_none() {
        return AdoptPlan::Blocked {
            reason: BlockedReason::NotFounded,
        };
    }
    // No resolvable host credential ⇒ xe/tofu can't reach the host ⇒ blocked. A
    // real code path (retryable), not a comment.
    if !facts.credential_present {
        return AdoptPlan::Blocked {
            reason: BlockedReason::NoCredential,
        };
    }
    AdoptPlan::Adopt {
        mesh_id: facts.mesh_id.clone(),
        target: target.clone(),
        steps: AdoptStep::ordered(),
    }
}

/// The reachable host an [`Adopter`] enrolled — the endpoint the toolstack drive
/// runs against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedHost {
    /// The `dom0` / pool address `xe` and peers reach it at.
    pub pool_address: String,
    /// The overlay IP the `dom0` took as a static Nebula member once enrolled.
    pub overlay_ip: String,
}

/// A typed failure from the injectable [`Adopter`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptError {
    /// The live path is not runnable in this build/environment yet — it needs a
    /// real prerequisite (the CA signer + live SSH, or live `xe`/`tofu` + host
    /// creds). Names the step + what is missing. §7-legal: a real method returning
    /// a real typed error, exactly as OW-5's apply does when `NetworkManager` is
    /// unreachable.
    IntegrationGated {
        /// Which seam step (`enroll-member` / `drive-toolstack`).
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

impl std::fmt::Display for AdoptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { step, reason } => {
                write!(f, "{step}: integration-gated — {reason}")
            }
            Self::Failed { step, reason } => write!(f, "{step}: {reason}"),
        }
    }
}

impl std::error::Error for AdoptError {}

/// The injectable side-effect seam. Production is [`LiveAdopter`]; tests use a
/// recording fake so the pure orchestration is exercised without a real member
/// enroll / SSH / `xe`/`tofu` apply.
pub trait Adopter {
    /// Enroll `target`'s `dom0` into `mesh_id` as a static Nebula member: mint a
    /// member-scoped token, sign its Host cert off this node's CA, materialize
    /// `/etc/nebula` pinned to the lighthouses, and record its roster row. Returns
    /// the reachable [`AdoptedHost`].
    ///
    /// # Errors
    /// An [`AdoptError`] — `IntegrationGated` when the live enroll can't run yet
    /// (needs the CA signer + live SSH), else `Failed`.
    fn enroll_member(&self, target: &AdoptTarget, mesh_id: &str)
        -> Result<AdoptedHost, AdoptError>;

    /// Drive the `dom0`'s `XAPI` toolstack via `xe`/`tofu` (the live farm's
    /// `xen-xapi` path) through the ordered, idempotent `steps`, so it serves VMs
    /// to the mesh.
    ///
    /// # Errors
    /// An [`AdoptError`] — `IntegrationGated` without live `xe`/`tofu` + host creds,
    /// else `Failed`.
    fn drive_toolstack(&self, host: &AdoptedHost, steps: &[AdoptStep]) -> Result<(), AdoptError>;
}

/// Production [`Adopter`] — the live member enroll + `xe`/`tofu` toolstack drive.
///
/// This slice (MV-7) delivers the pure core + the seam; the live executors (the
/// member-scoped enroll + SSH materialize, and the real `xen-xapi` `tofu` apply
/// over `xe`) are wired by a later MV unit. Until then each method returns a typed
/// [`AdoptError::IntegrationGated`] naming exactly what the live call needs — never
/// a fake success (§7).
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveAdopter;

impl Adopter for LiveAdopter {
    fn enroll_member(
        &self,
        target: &AdoptTarget,
        _mesh_id: &str,
    ) -> Result<AdoptedHost, AdoptError> {
        Err(AdoptError::IntegrationGated {
            step: "enroll-member",
            reason: format!(
                "needs the live CA signer to mint a static-member token + sign the dom0's Host \
                 cert, and live SSH to {} to materialize /etc/nebula",
                target.pool_address
            ),
        })
    }

    fn drive_toolstack(&self, host: &AdoptedHost, _steps: &[AdoptStep]) -> Result<(), AdoptError> {
        Err(AdoptError::IntegrationGated {
            step: "drive-toolstack",
            reason: format!(
                "needs live xe/tofu (the xen-xapi provider) + the resolved host credential \
                 against {}",
                host.pool_address
            ),
        })
    }
}

/// The result of an [`execute`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptOutcome {
    /// The host was enrolled as a static member + its toolstack driven.
    Adopted {
        /// The reachable host the enroll returned.
        host: AdoptedHost,
    },
    /// The plan was blocked — nothing was touched; a retry is available.
    Blocked {
        /// Why adoption was blocked.
        reason: BlockedReason,
    },
}

/// Pure orchestration over the [`Adopter`] seam.
///
/// For an [`AdoptPlan::Adopt`] run enroll-member → drive-toolstack **in that
/// order**; for [`AdoptPlan::Blocked`] short-circuit to the retryable outcome (no
/// seam calls). This is the tested orchestration the fake pins; the real side
/// effects live entirely in the injected `adopter`.
///
/// # Errors
/// Propagates the first [`AdoptError`] any seam step returns.
pub fn execute(plan: &AdoptPlan, adopter: &dyn Adopter) -> Result<AdoptOutcome, AdoptError> {
    match plan {
        AdoptPlan::Blocked { reason } => Ok(AdoptOutcome::Blocked { reason: *reason }),
        AdoptPlan::Adopt {
            mesh_id,
            target,
            steps,
        } => {
            let host = adopter.enroll_member(target, mesh_id)?;
            adopter.drive_toolstack(&host, steps)?;
            Ok(AdoptOutcome::Adopted { host })
        }
    }
}

/// Impure probe shell: gather the live adopt facts off this node.
///
/// Best-effort — a missing bundle / unresolved credential degrades to `None`/`false`
/// fields rather than erroring, so the pure [`plan_adopt`] fold always runs and
/// produces the real verdict (blocked when a prerequisite is absent). The mesh-id +
/// CA holder come from the founding bundle (reuse, not reinvention).
#[must_use]
pub fn gather(workgroup_root: &Path, node_id: &str, target: &AdoptTarget) -> AdoptFacts {
    let mesh_id = crate::onboard::invite::resolve_mesh_id(workgroup_root, node_id);
    let ca_holder_overlay_ip =
        crate::ca::bundle::read_bundle(&crate::ca::bundle::bundle_path(workgroup_root, node_id))
            .ok()
            .map(|b| b.overlay_ip);
    AdoptFacts {
        mesh_id,
        ca_holder_overlay_ip,
        credential_present: credential_present(&target.credential_ref),
    }
}

/// Whether the credential the handle names resolves in this environment. Best-effort
/// env probe (mirrors OW-7's `cloud_token_present`): a `scheme:NAME` / bare `NAME`
/// handle is present iff the derived env var is set and non-empty. The pure
/// [`plan_adopt`] keys the retryable no-credential branch off this signal.
fn credential_present(credential_ref: &str) -> bool {
    let name = credential_ref
        .split_once(':')
        .map_or(credential_ref, |(_scheme, rest)| rest);
    if name.trim().is_empty() {
        return false;
    }
    let upper = name.to_ascii_uppercase().replace('-', "_");
    [name, upper.as_str()]
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn target() -> AdoptTarget {
        AdoptTarget {
            pool_address: "172.20.0.9".to_string(),
            overlay_ip: "10.42.0.9".to_string(),
            credential_ref: "secret:xcp-host".to_string(),
        }
    }

    fn facts(founded: bool, credential: bool) -> AdoptFacts {
        AdoptFacts {
            mesh_id: "home-deadbeef".to_string(),
            ca_holder_overlay_ip: founded.then(|| "10.42.0.1".to_string()),
            credential_present: credential,
        }
    }

    #[test]
    fn founded_with_credential_plans_an_adopt() {
        let plan = plan_adopt(&target(), &facts(true, true));
        match &plan {
            AdoptPlan::Adopt {
                mesh_id,
                target: t,
                steps,
            } => {
                assert_eq!(mesh_id, "home-deadbeef");
                assert_eq!(t.pool_address, "172.20.0.9");
                assert_eq!(t.overlay_ip, "10.42.0.9");
                assert_eq!(steps, &AdoptStep::ordered());
            }
            AdoptPlan::Blocked { .. } => panic!("expected an adopt plan"),
        }
        assert!(!plan.retry_available());
        assert_eq!(plan.steps().len(), 5);
    }

    #[test]
    fn unfounded_node_is_blocked_with_retry() {
        // No CA holder ⇒ nothing to sign the member's Host cert, even with creds.
        let plan = plan_adopt(&target(), &facts(false, true));
        assert_eq!(
            plan,
            AdoptPlan::Blocked {
                reason: BlockedReason::NotFounded
            }
        );
        assert!(
            plan.retry_available(),
            "the operator can retry once founded"
        );
        assert!(plan.steps().is_empty());
        assert!(plan.human().contains("found this mesh first"));
    }

    #[test]
    fn missing_credential_is_blocked_with_retry() {
        // The headline no-credential → blocked + retry branch (a real path).
        let plan = plan_adopt(&target(), &facts(true, false));
        assert_eq!(
            plan,
            AdoptPlan::Blocked {
                reason: BlockedReason::NoCredential
            }
        );
        assert!(plan.retry_available());
        assert!(plan.human().contains("retry available"));
    }

    #[test]
    fn adopt_steps_are_ordered_and_stable() {
        let steps = AdoptStep::ordered();
        assert_eq!(
            steps,
            vec![
                AdoptStep::MintMemberToken,
                AdoptStep::EnrollStaticMember,
                AdoptStep::RegisterInRoster,
                AdoptStep::AttachToolstack,
                AdoptStep::ConvergeToolstack,
            ],
            "the adopt order is fixed"
        );
        // The member token must precede the enroll that consumes it.
        let mint = steps
            .iter()
            .position(|s| *s == AdoptStep::MintMemberToken)
            .unwrap();
        let enroll = steps
            .iter()
            .position(|s| *s == AdoptStep::EnrollStaticMember)
            .unwrap();
        assert!(mint < enroll, "mint the member token before enrolling");
        // The static-member enroll must precede driving the toolstack.
        let converge = steps
            .iter()
            .position(|s| *s == AdoptStep::ConvergeToolstack)
            .unwrap();
        assert!(
            enroll < converge,
            "enroll the member before driving xe/tofu"
        );
        // Every step has a non-empty description.
        assert!(steps.iter().all(|s| !s.describe().is_empty()));
    }

    #[test]
    fn human_describes_the_adopt_as_a_static_member() {
        let plan = plan_adopt(&target(), &facts(true, true));
        let h = plan.human();
        assert!(h.contains("static Nebula member"));
        assert!(h.contains("172.20.0.9"), "names the pool address");
        assert!(h.contains("5 step(s)"), "names the step count");
    }

    #[test]
    fn adopt_target_round_trips_through_serde() {
        let t = target();
        let json = serde_json::to_string(&t).expect("serialize");
        let back: AdoptTarget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(t, back);
    }

    /// Recording [`Adopter`] fake: records the ordered calls so the pure
    /// orchestration is asserted without a real enroll / `xe`/`tofu` apply.
    struct RecordingAdopter {
        calls: RefCell<Vec<String>>,
        seen_mesh_id: RefCell<Option<String>>,
        seen_steps: RefCell<Vec<AdoptStep>>,
    }

    impl RecordingAdopter {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                seen_mesh_id: RefCell::new(None),
                seen_steps: RefCell::new(Vec::new()),
            }
        }
    }

    impl Adopter for RecordingAdopter {
        fn enroll_member(
            &self,
            target: &AdoptTarget,
            mesh_id: &str,
        ) -> Result<AdoptedHost, AdoptError> {
            self.calls.borrow_mut().push("enroll_member".to_string());
            *self.seen_mesh_id.borrow_mut() = Some(mesh_id.to_string());
            Ok(AdoptedHost {
                pool_address: target.pool_address.clone(),
                overlay_ip: target.overlay_ip.clone(),
            })
        }
        fn drive_toolstack(
            &self,
            host: &AdoptedHost,
            steps: &[AdoptStep],
        ) -> Result<(), AdoptError> {
            assert_eq!(
                host.pool_address, "172.20.0.9",
                "drive-toolstack sees enroll's host"
            );
            self.calls.borrow_mut().push("drive_toolstack".to_string());
            *self.seen_steps.borrow_mut() = steps.to_vec();
            Ok(())
        }
    }

    #[test]
    fn execute_drives_the_seam_in_order() {
        let plan = plan_adopt(&target(), &facts(true, true));
        let adopter = RecordingAdopter::new();
        let outcome = execute(&plan, &adopter).expect("execute");
        match outcome {
            AdoptOutcome::Adopted { host } => {
                assert_eq!(host.pool_address, "172.20.0.9");
                assert_eq!(host.overlay_ip, "10.42.0.9");
            }
            AdoptOutcome::Blocked { .. } => panic!("expected an adopted outcome"),
        }
        // The seam ran enroll_member → drive_toolstack, in that order.
        assert_eq!(
            *adopter.calls.borrow(),
            vec!["enroll_member", "drive_toolstack"]
        );
        // enroll_member received the mesh-id; drive_toolstack the ordered steps.
        assert_eq!(
            adopter.seen_mesh_id.borrow().as_deref(),
            Some("home-deadbeef")
        );
        assert_eq!(*adopter.seen_steps.borrow(), AdoptStep::ordered());
    }

    #[test]
    fn execute_short_circuits_a_blocked_plan() {
        // A blocked plan makes no seam calls — nothing to adopt.
        let plan = plan_adopt(&target(), &facts(false, true));
        let adopter = RecordingAdopter::new();
        let outcome = execute(&plan, &adopter).expect("execute");
        assert_eq!(
            outcome,
            AdoptOutcome::Blocked {
                reason: BlockedReason::NotFounded
            }
        );
        assert!(
            adopter.calls.borrow().is_empty(),
            "no seam calls when blocked"
        );
    }

    #[test]
    fn live_adopter_is_integration_gated_not_fake_success() {
        let adopter = LiveAdopter;
        let err = adopter
            .enroll_member(&target(), "home-deadbeef")
            .expect_err("live enroll must not fake success");
        match err {
            AdoptError::IntegrationGated { step, reason } => {
                assert_eq!(step, "enroll-member");
                assert!(reason.contains("CA signer"), "reason names the CA signer");
            }
            AdoptError::Failed { .. } => panic!("expected an integration-gated error"),
        }
        // drive-toolstack is likewise integration-gated (typed, honest) — names the
        // live xe/tofu + host creds it needs.
        let host = AdoptedHost {
            pool_address: "172.20.0.9".to_string(),
            overlay_ip: "10.42.0.9".to_string(),
        };
        match adopter.drive_toolstack(&host, &AdoptStep::ordered()) {
            Err(AdoptError::IntegrationGated { step, reason }) => {
                assert_eq!(step, "drive-toolstack");
                assert!(reason.contains("xe/tofu"), "reason names live xe/tofu");
                assert!(reason.contains("credential"), "reason names the host creds");
            }
            other => panic!("expected an integration-gated error, got {other:?}"),
        }
    }

    #[test]
    fn execute_propagates_the_integration_gated_error() {
        // Through the LIVE adopter, execute surfaces the first typed error.
        let plan = plan_adopt(&target(), &facts(true, true));
        let err = execute(&plan, &LiveAdopter).expect_err("live path is gated");
        assert!(matches!(
            err,
            AdoptError::IntegrationGated {
                step: "enroll-member",
                ..
            }
        ));
    }

    #[test]
    fn credential_present_probes_the_handle_env() {
        // Unique var per test run so parallel `cargo test` workers don't collide.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("MDE_ADOPT_CRED_TEST_{nonce}");
        // Absent ⇒ not present.
        assert!(!credential_present(&format!("secret:{name}")));
        // Set ⇒ present (the `secret:` scheme is stripped to the var name).
        std::env::set_var(&name, "root-pw");
        assert!(credential_present(&format!("secret:{name}")));
        std::env::remove_var(&name);
        // An empty handle is never present.
        assert!(!credential_present(""));
        assert!(!credential_present("secret:"));
    }
}

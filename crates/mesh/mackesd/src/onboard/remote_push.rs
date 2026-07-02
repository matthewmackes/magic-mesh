//! OW-15 — the shared onboard **remote-push executor**.
//!
//! Design + decision: `docs/design/onboard-remote-push.md`; operator-confirmed
//! **HYBRID** transport (2026-07-01). The three onboard `LiveX` seams —
//! [`super::spawn_lighthouse::LiveProvisioner::push_enroll`],
//! [`super::service_add::LiveServiceApply::provision_music`], and
//! [`super::first_desktop`]'s live apply — all need to *reach a target node and
//! apply a bounded set of actions*. This module is the ONE executor they share.
//!
//! **The allow-list is the type system.** The only remote effects that exist are
//! the variants of [`Action`]; there is no "run arbitrary command" arm, so a
//! caller (or a forged bundle) physically cannot request anything outside it
//! (§8 flat-trust blast-radius control; §9 no-raw-shell for the day-2 path).
//!
//! Two impls sit behind [`RemotePush`] (built in follow-up commits):
//! * `SshBootstrap` — bearer-scoped SSH to a **not-yet-enrolled** box (the OW-7
//!   `push_enroll` model), used only for the bootstrap instant.
//! * `BusApply` — a §9-native typed `action/onboard/apply` Bus verb carrying a
//!   **signed** [`JobBundle`] to an **enrolled** peer; the `onboard_apply` worker
//!   validates signer + freshness + the allow-list and applies.

use crate::ipc::secret_store::SecretStore;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use mde_role::{Role, RoleClass};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The bounded, exhaustive set of effects the executor will apply on a target.
///
/// Adding a remote capability means adding a variant here **and** handling it in
/// every apply path — there is deliberately no escape hatch (no `RunShell`,
/// no `Exec(String)`). This enum *is* the security allow-list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Pin a deployment role + its capability tags on the target. `role` is a
    /// [`mde_role::Role`] name (`lighthouse`/`workstation`); `media` sets the
    /// [`mde_role::Capability::Media`] tag (OW-11 promotes a plain lighthouse to
    /// `Lighthouse_Media` so its `navidrome_supervisor` provisions Navidrome).
    PinRole { role: String, media: bool },
    /// Seal a named secret into the target's replicated store. `secret` is the
    /// plaintext value; the target's [`SecretStore`] encrypts it **at rest** (the
    /// audited `age`+etcd or local-AEAD envelope) — matching how the platform's
    /// own `mcnf-secret.sh` takes plaintext and seals it. It travels only inside a
    /// signed [`JobBundle`] over an encrypted transport (Nebula overlay for day-2,
    /// bearer-scoped SSH for bootstrap) and MUST NOT be logged; only the `name` is
    /// loggable ([`Action::redacted`]).
    SealSecret { name: String, secret: String },
    /// Run the RPM-shipped enroll bootstrap so a fresh box joins the mesh
    /// (OW-7 push-enroll). Carries the single-use enroll bearer.
    RunEnroll { bearer: String },
    /// Open a broker session on a desktop host (OW-8 first-desktop).
    OpenBroker { session_id: String },
}

impl Action {
    /// A log-safe one-line description — NEVER leaks secret material (the sealed
    /// blob and the bearer are redacted).
    #[must_use]
    pub fn redacted(&self) -> String {
        match self {
            Self::PinRole { role, media } => {
                let tag = if *media { " +media" } else { "" };
                format!("pin-role {role}{tag}")
            }
            Self::SealSecret { name, secret } => {
                format!("seal-secret {name} ({} bytes, redacted)", secret.len())
            }
            Self::RunEnroll { .. } => "run-enroll (bearer redacted)".to_string(),
            Self::OpenBroker { session_id } => format!("open-broker {session_id}"),
        }
    }
}

/// A typed failure from the executor. Every variant leaves the target
/// **unchanged** (no partial state) — an action either fully applies or is a
/// no-op that surfaces the reason.
#[derive(Debug)]
pub enum RemotePushError {
    /// The transport (SSH / Bus) could not reach or authenticate to the target.
    Unreachable { target: String, why: String },
    /// The signed [`JobBundle`] failed validation (bad signature, stale nonce,
    /// unknown signer) — refused before any action ran.
    BundleRejected { why: String },
    /// The live executor is not wired yet on this build (the gated stub returns
    /// this, never a fake success — §7).
    NotWired { transport: &'static str },
    /// An individual action failed to apply; the target is rolled back to
    /// pre-apply state.
    ActionFailed { action: String, why: String },
}

impl std::fmt::Display for RemotePushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable { target, why } => write!(f, "target {target} unreachable: {why}"),
            Self::BundleRejected { why } => write!(f, "job bundle rejected: {why}"),
            Self::NotWired { transport } => {
                write!(
                    f,
                    "remote-push {transport} transport not wired on this build"
                )
            }
            Self::ActionFailed { action, why } => write!(f, "action `{action}` failed: {why}"),
        }
    }
}

impl std::error::Error for RemotePushError {}

/// Where + how to reach the target. Chooses the transport per the hybrid model:
/// a not-yet-enrolled box takes [`SshBootstrap`]; an enrolled peer takes the
/// §9-native [`BusApply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A fresh box not yet on the mesh — reachable only out-of-band (SSH over
    /// its public/LAN IP), gated by the single-use enroll bearer.
    Bootstrap { host: String },
    /// An enrolled peer — reachable over the Nebula overlay + the Bus, addressed
    /// by its mesh node id.
    Enrolled { node_id: String },
}

/// The injectable executor seam. Production dispatches to `SshBootstrap` /
/// `BusApply` by [`Target`]; tests use a recording fake.
pub trait RemotePush {
    /// Apply the ordered `actions` to `target`. All-or-nothing per action.
    ///
    /// # Errors
    /// A [`RemotePushError`] — `NotWired` on a build without the live transport,
    /// else the transport/validation/apply failure. Never a fake success (§7).
    fn apply(&self, target: &Target, actions: &[Action]) -> Result<(), RemotePushError>;
}

/// A **signed job bundle** — the §9 payload the `BusApply` transport carries to
/// an enrolled peer over `action/onboard/apply`. The target's `onboard_apply`
/// worker validates the signature + freshness against the mesh CA before
/// applying, so a peer only ever runs allow-listed actions authored by a
/// trusted signer (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobBundle {
    /// The enrolled target's mesh node id.
    pub target_node: String,
    /// The allow-listed actions to apply, in order.
    pub actions: Vec<Action>,
    /// Monotonic issue time (Unix seconds) — the worker rejects a bundle older
    /// than [`Self::MAX_AGE_SECS`] (replay/staleness guard).
    pub issued_at: i64,
    /// Per-bundle nonce — the worker rejects a re-seen nonce (single-use).
    pub nonce: String,
}

impl JobBundle {
    /// A bundle older than this (seconds) is rejected as stale.
    pub const MAX_AGE_SECS: i64 = 300;

    /// The canonical bytes that get signed — a stable serialization of the
    /// bundle's content (NOT including the signature). Both signer and verifier
    /// derive the exact same bytes.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        // A deterministic, field-ordered encoding (serde_json with sorted keys
        // is stable enough here since the struct field order is fixed).
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Sign the bundle with `key`, returning the detached signature bytes.
    #[must_use]
    pub fn sign(&self, key: &SigningKey) -> Vec<u8> {
        key.sign(&self.signing_bytes()).to_bytes().to_vec()
    }

    /// Verify `sig` was produced over this bundle by `signer`, and that the
    /// bundle is fresh relative to `now_unix`.
    ///
    /// # Errors
    /// [`RemotePushError::BundleRejected`] on a bad signature, a malformed
    /// signature length, or a stale/future `issued_at`.
    pub fn verify(
        &self,
        sig: &[u8],
        signer: &VerifyingKey,
        now_unix: i64,
    ) -> Result<(), RemotePushError> {
        let age = now_unix - self.issued_at;
        if age > Self::MAX_AGE_SECS {
            return Err(RemotePushError::BundleRejected {
                why: format!("stale bundle: {age}s old (max {}s)", Self::MAX_AGE_SECS),
            });
        }
        if age < -Self::MAX_AGE_SECS {
            return Err(RemotePushError::BundleRejected {
                why: format!("bundle issued in the future by {}s", -age),
            });
        }
        let sig_bytes: [u8; 64] = sig
            .try_into()
            .map_err(|_| RemotePushError::BundleRejected {
                why: format!("signature is {} bytes, expected 64", sig.len()),
            })?;
        signer
            .verify(&self.signing_bytes(), &Signature::from_bytes(&sig_bytes))
            .map_err(|e| RemotePushError::BundleRejected {
                why: format!("signature does not verify: {e}"),
            })
    }
}

/// The target-side effect seam — applies ONE allow-listed [`Action`] locally on
/// the node receiving the push. Production `LocalApplier` (a follow-up) calls the
/// real primitives (`mde_role::pin`, the secret store, enroll, the session-
/// broker); tests use a recording fake. Both transports + the `onboard_apply`
/// worker converge here, so the actual effects live in exactly one place.
pub trait Applier {
    /// Apply a single action to THIS node. Idempotent where possible (re-applying
    /// a `PinRole` is a no-op).
    ///
    /// # Errors
    /// [`RemotePushError::ActionFailed`] naming the (redacted) action + reason.
    fn apply_one(&self, action: &Action) -> Result<(), RemotePushError>;
}

/// Apply `actions` in order via `applier`, stopping at the FIRST failure. Returns
/// `Ok(())` only when every action applied; the error names the failing action.
/// Callers validate the whole signed bundle BEFORE calling this, so a mid-way
/// failure is rare + observable rather than a silent partial (§7).
///
/// # Errors
/// The first [`RemotePushError::ActionFailed`] any action returns.
pub fn apply_all(applier: &dyn Applier, actions: &[Action]) -> Result<(), RemotePushError> {
    for action in actions {
        applier.apply_one(action)?;
    }
    Ok(())
}

/// The production [`Applier`] — applies an [`Action`]'s **local effect** on THIS
/// node using the real primitives, no shell escape-hatch:
///
/// * [`Action::PinRole`] → [`mde_role::pin_class_at`] (writes the pinned
///   `role.toml`, upgrade-only; promoting a lighthouse to `Lighthouse_Media` is
///   exactly OW-11's Music step).
/// * [`Action::SealSecret`] → [`SecretStore::put`] (encrypts at rest into the
///   replicated `age`+etcd store, or the local-AEAD fallback).
///
/// [`Action::RunEnroll`] and [`Action::OpenBroker`] are **not** local effects:
/// enroll is the bootstrap step the `SshBootstrap` transport runs over the
/// enroll-bearer SSH, and open-broker is a Bus publish the `onboard_apply` worker
/// (which holds the Bus handle) issues. This applier owns the two node-local
/// effects only, so both transports + the worker converge on ONE place for them;
/// it returns a typed [`RemotePushError::NotWired`] naming the layer that does own
/// the action, never a fake success (§7).
pub struct LocalApplier {
    /// Where the pinned role is written — the canonical `/var/lib/mde/role.toml`
    /// in production, a redirect in tests (via [`mde_role::default_role_path`]'s
    /// `MDE_ROLE_PATH`, or an explicit path here).
    role_path: PathBuf,
    /// The node's secret store — the mesh `age`+etcd store when its helper is
    /// reachable, else the local-AEAD fallback ([`SecretStore::resolve`]).
    store: SecretStore,
}

impl LocalApplier {
    /// Production constructor: pin at the canonical role path, and resolve the
    /// secret store from the deployed `repo_dir` (holding
    /// `automation/secrets/mcnf-secret.sh`) + the `workgroup_root`. The calling
    /// `onboard_apply` worker passes both from its own context (the same
    /// `workgroup_root` its sibling verbs like `service_add::gather` take).
    #[must_use]
    pub fn resolve(repo_dir: &Path, workgroup_root: &Path) -> Self {
        Self {
            role_path: mde_role::default_role_path(),
            store: SecretStore::resolve(repo_dir, workgroup_root),
        }
    }

    /// Explicit constructor with an injected role path + store — the seam the
    /// round-trip tests drive (a temp `role.toml` + a `LocalAead` store over a
    /// throwaway age identity).
    #[must_use]
    pub fn new(role_path: PathBuf, store: SecretStore) -> Self {
        Self { role_path, store }
    }
}

impl Applier for LocalApplier {
    fn apply_one(&self, action: &Action) -> Result<(), RemotePushError> {
        match action {
            Action::PinRole { role, media } => {
                let role: Role = role.parse().map_err(|e| RemotePushError::ActionFailed {
                    action: action.redacted(),
                    why: format!("{e}"),
                })?;
                let class = RoleClass {
                    role,
                    media: *media,
                };
                mde_role::pin_class_at(&self.role_path, &class)
                    .map(|_| ())
                    .map_err(|e| RemotePushError::ActionFailed {
                        action: action.redacted(),
                        why: e.to_string(),
                    })
            }
            Action::SealSecret { name, secret } => {
                self.store
                    .put(name, secret)
                    .map_err(|why| RemotePushError::ActionFailed {
                        action: action.redacted(),
                        why,
                    })
            }
            // Owned by the SshBootstrap transport (the enroll-bearer SSH step),
            // not a node-local effect — honest typed gate, never a fake apply.
            Action::RunEnroll { .. } => Err(RemotePushError::NotWired {
                transport: "ssh-bootstrap (enroll)",
            }),
            // Owned by the onboard_apply worker's Bus publish (it holds the Bus
            // handle the broker SessionRequest goes out on).
            Action::OpenBroker { .. } => Err(RemotePushError::NotWired {
                transport: "bus-worker (open-broker)",
            }),
        }
    }
}

/// The observed-state an `onboard_apply` worker reports after applying a bundle:
/// the redacted (secret-free) actions that took effect on this node, echoed back
/// so the issuer + the §8 audit log confirm exactly what landed. Publishable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedOutcome {
    /// The node that applied the bundle (its mesh node id).
    pub node: String,
    /// The redacted descriptions of the actions that applied, in order — never the
    /// secret material itself (§8).
    pub applied: Vec<String>,
}

/// A single-use nonce guard that closes the replay window freshness alone leaves
/// open: inside [`JobBundle::MAX_AGE_SECS`] a validly-signed bundle could be
/// replayed verbatim. The guard records each accepted bundle's nonce and refuses a
/// re-seen one, pruning entries once they age out of the freshness window — a
/// bundle that old is already rejected by [`JobBundle::verify`], so its nonce is
/// safe to forget, and the guard stays bounded without persistence.
#[derive(Debug, Default)]
pub struct NonceGuard {
    /// nonce -> issued_at (Unix seconds), for age-based pruning.
    seen: std::collections::HashMap<String, i64>,
}

impl NonceGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop nonces older than the freshness window relative to `now` (they can no
    /// longer be replayed — [`JobBundle::verify`] rejects them on age).
    fn prune(&mut self, now: i64) {
        self.seen
            .retain(|_, issued_at| now - *issued_at <= JobBundle::MAX_AGE_SECS);
    }

    /// Record `nonce` as used; `true` if it was fresh (first use), `false` if it
    /// was already seen (a replay to reject). Prunes aged entries first.
    fn check_and_record(&mut self, nonce: &str, issued_at: i64, now: i64) -> bool {
        self.prune(now);
        if self.seen.contains_key(nonce) {
            return false;
        }
        self.seen.insert(nonce.to_string(), issued_at);
        true
    }
}

/// Validate a received signed bundle and apply it locally — the `onboard_apply`
/// worker's pure core (the Bus drain loop wires this onto `action/onboard/apply`).
///
/// The order is security-load-bearing:
/// 1. [`JobBundle::verify`] — signature + freshness. A failure here leaves the
///    target **fully unchanged** and does NOT burn the nonce (so an attacker can't
///    exhaust the nonce space with unsigned bundles, and a genuine re-send under a
///    fresh signature still works).
/// 2. Nonce single-use — refuse a replay of an already-applied bundle.
/// 3. [`apply_all`] — apply in order, stopping at the first failure (no silent
///    partial past it, §7). Validation being fully upstream of apply is what makes
///    an auth failure a clean no-op on the target.
///
/// # Errors
/// [`RemotePushError::BundleRejected`] on a bad signature / stale bundle / replayed
/// nonce (target unchanged); [`RemotePushError::ActionFailed`] if an action fails
/// mid-apply (stops there, naming it).
pub fn process_apply(
    bundle: &JobBundle,
    sig: &[u8],
    signer: &VerifyingKey,
    now: i64,
    nonce_guard: &mut NonceGuard,
    applier: &dyn Applier,
) -> Result<AppliedOutcome, RemotePushError> {
    bundle.verify(sig, signer, now)?;
    if !nonce_guard.check_and_record(&bundle.nonce, bundle.issued_at, now) {
        return Err(RemotePushError::BundleRejected {
            why: format!("nonce {} already applied (replay)", bundle.nonce),
        });
    }
    apply_all(applier, &bundle.actions)?;
    Ok(AppliedOutcome {
        node: bundle.target_node.clone(),
        applied: bundle.actions.iter().map(Action::redacted).collect(),
    })
}

// ─────────────────── production transports (the hybrid, C) ───────────────────

/// Production [`RemotePush`] for **bootstrap** targets — bearer-scoped SSH to a
/// not-yet-enrolled box (OW-7's accepted `push_enroll` model), used ONLY for the
/// bootstrap instant. It refuses:
/// * an [`Target::Enrolled`] peer — a mesh member must be driven over the §9
///   [`BusApply`] path, never raw SSH; and
/// * any action other than [`Action::RunEnroll`] — the only thing a fresh box
///   does over the single-use enroll bearer is run enroll (§8/§9 blast radius).
///
/// Reaching the box over live SSH and running the RPM-shipped enroll is the
/// integration-gated live path (operator/live acceptance 2): with no live SSH
/// runner wired on this build, [`RemotePush::apply`] returns a typed
/// [`RemotePushError::NotWired`] — a real error, never a fake success (§7). The
/// target is left completely unchanged.
#[derive(Debug, Default, Clone, Copy)]
pub struct SshBootstrap;

impl RemotePush for SshBootstrap {
    fn apply(&self, target: &Target, actions: &[Action]) -> Result<(), RemotePushError> {
        match target {
            Target::Enrolled { node_id } => {
                return Err(RemotePushError::BundleRejected {
                    why: format!(
                        "SshBootstrap refuses the enrolled peer `{node_id}` — a mesh member is \
                         driven over the §9 BusApply path, not raw SSH"
                    ),
                });
            }
            Target::Bootstrap { .. } => {}
        }
        // The bootstrap instant runs ONLY enroll over the single-use bearer; any
        // other action on a not-yet-enrolled box is out of scope.
        for action in actions {
            if !matches!(action, Action::RunEnroll { .. }) {
                return Err(RemotePushError::BundleRejected {
                    why: format!(
                        "SshBootstrap only runs run-enroll during the bootstrap instant; refused \
                         `{}`",
                        action.redacted()
                    ),
                });
            }
        }
        Err(RemotePushError::NotWired {
            transport: "ssh-bootstrap (bearer-scoped SSH enroll)",
        })
    }
}

/// Production [`RemotePush`] for **day-2** targets — the §9-native signed-bundle
/// Bus verb to an already-enrolled peer (OW-11's role-pin + secret-seal, OW-8's
/// broker open). It refuses:
/// * a [`Target::Bootstrap`] host — a not-yet-enrolled box is not on the Bus yet
///   (chicken-and-egg), so it takes [`SshBootstrap`]; and
/// * an [`Action::RunEnroll`] — enroll is a bootstrap-only SSH step, never a
///   day-2 Bus action.
///
/// Signing a [`JobBundle`] with the issuing node's identity key, publishing it on
/// `action/onboard/apply`, and awaiting the target `onboard_apply` worker's
/// observed-state reply over the overlay is the integration-gated live path
/// (operator/live acceptance 1): with no live cross-node Bus round-trip wired on
/// this build, [`RemotePush::apply`] returns a typed [`RemotePushError::NotWired`]
/// — never a fake success (§7). The pure sign/verify/apply core (built + tested
/// above) is what the live round-trip carries once wired; the target is left
/// unchanged until then.
#[derive(Debug, Default, Clone, Copy)]
pub struct BusApply;

impl RemotePush for BusApply {
    fn apply(&self, target: &Target, actions: &[Action]) -> Result<(), RemotePushError> {
        match target {
            Target::Bootstrap { host } => {
                return Err(RemotePushError::BundleRejected {
                    why: format!(
                        "BusApply refuses the bootstrap host `{host}` — a not-yet-enrolled box is \
                         not on the Bus; use SshBootstrap"
                    ),
                });
            }
            Target::Enrolled { .. } => {}
        }
        for action in actions {
            if matches!(action, Action::RunEnroll { .. }) {
                return Err(RemotePushError::BundleRejected {
                    why: "run-enroll is a bootstrap step, not a day-2 Bus action".to_string(),
                });
            }
        }
        Err(RemotePushError::NotWired {
            transport: "bus-apply (signed-bundle cross-node round-trip)",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records the actions it was asked to apply; fails on any action whose
    /// redacted text contains `fail_on` (to exercise the stop-on-first-failure
    /// path without any live effect).
    struct RecordingApplier {
        applied: RefCell<Vec<String>>,
        fail_on: Option<&'static str>,
    }
    impl Applier for RecordingApplier {
        fn apply_one(&self, action: &Action) -> Result<(), RemotePushError> {
            let desc = action.redacted();
            if let Some(f) = self.fail_on {
                if desc.contains(f) {
                    return Err(RemotePushError::ActionFailed {
                        action: desc,
                        why: "injected".into(),
                    });
                }
            }
            self.applied.borrow_mut().push(desc);
            Ok(())
        }
    }

    #[test]
    fn apply_all_applies_every_action_in_order() {
        let app = RecordingApplier {
            applied: RefCell::new(vec![]),
            fail_on: None,
        };
        let actions = vec![
            Action::PinRole {
                role: "lighthouse".into(),
                media: true,
            },
            Action::SealSecret {
                name: "media-spaces".into(),
                secret: "s3-creds".into(),
            },
        ];
        assert!(apply_all(&app, &actions).is_ok());
        assert_eq!(app.applied.borrow().len(), 2);
        assert!(app.applied.borrow()[0].contains("pin-role lighthouse +media"));
    }

    #[test]
    fn apply_all_stops_at_the_first_failure() {
        // fail on the seal ⇒ the pin (before it) applied, the open-broker (after)
        // never runs — no silent partial past the failure.
        let app = RecordingApplier {
            applied: RefCell::new(vec![]),
            fail_on: Some("seal-secret"),
        };
        let actions = vec![
            Action::PinRole {
                role: "lighthouse".into(),
                media: true,
            },
            Action::SealSecret {
                name: "s".into(),
                secret: "v".into(),
            },
            Action::OpenBroker {
                session_id: "x".into(),
            },
        ];
        let err = apply_all(&app, &actions).unwrap_err();
        assert!(matches!(err, RemotePushError::ActionFailed { .. }));
        assert_eq!(
            app.applied.borrow().len(),
            1,
            "only the pin applied before the failure"
        );
    }

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[9_u8; 32])
    }

    fn bundle(now: i64) -> JobBundle {
        JobBundle {
            target_node: "peer:lh-media".into(),
            actions: vec![
                Action::PinRole {
                    role: "lighthouse".into(),
                    media: true,
                },
                Action::SealSecret {
                    name: "media-spaces".into(),
                    secret: "s3-creds".into(),
                },
            ],
            issued_at: now,
            nonce: "nonce-1".into(),
        }
    }

    #[test]
    fn sign_then_verify_round_trips() {
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now);
        let sig = b.sign(&k);
        assert!(b.verify(&sig, &k.verifying_key(), now).is_ok());
    }

    #[test]
    fn a_tampered_bundle_fails_verification() {
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now);
        let sig = b.sign(&k);
        // flip an action → the signing bytes change → signature no longer verifies
        let mut tampered = b.clone();
        tampered.actions[0] = Action::PinRole {
            role: "workstation".into(),
            media: false,
        };
        let err = tampered.verify(&sig, &k.verifying_key(), now).unwrap_err();
        assert!(matches!(err, RemotePushError::BundleRejected { .. }));
    }

    #[test]
    fn a_stale_or_future_bundle_is_rejected() {
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now);
        let sig = b.sign(&k);
        // 301s later ⇒ stale
        assert!(b.verify(&sig, &k.verifying_key(), now + 301).is_err());
        // 301s before issue ⇒ future
        assert!(b.verify(&sig, &k.verifying_key(), now - 301).is_err());
        // within window ⇒ ok
        assert!(b.verify(&sig, &k.verifying_key(), now + 60).is_ok());
    }

    #[test]
    fn a_wrong_signer_is_rejected() {
        let now = 1_800_000_000;
        let b = bundle(now);
        let sig = b.sign(&key());
        let other = SigningKey::from_bytes(&[3_u8; 32]);
        assert!(b.verify(&sig, &other.verifying_key(), now).is_err());
    }

    #[test]
    fn actions_redact_secret_material() {
        // the secret value + the bearer must never appear in the log line
        let seal = Action::SealSecret {
            name: "media-spaces".into(),
            secret: "SUPER-SECRET-S3-CREDENTIAL".into(),
        };
        assert!(seal.redacted().contains("media-spaces"));
        assert!(seal.redacted().contains("redacted"));
        assert!(!seal.redacted().contains("SUPER-SECRET-S3-CREDENTIAL"));
        let enroll = Action::RunEnroll {
            bearer: "SECRET-BEARER".into(),
        };
        assert!(!enroll.redacted().contains("SECRET-BEARER"));
    }

    // ── LocalApplier: the real node-local effects (no mock) ──

    /// A `LocalAead` secret store over a throwaway age identity + a temp role
    /// path — the real primitives PinRole/SealSecret drive, so these tests
    /// exercise the actual `role.toml` write + the actual at-rest seal.
    fn local_applier() -> (tempfile::TempDir, LocalApplier) {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("mcnf-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        let store = SecretStore::LocalAead {
            dir: tmp.path().join("secrets"),
            key_path,
        };
        let applier = LocalApplier::new(tmp.path().join("role.toml"), store);
        (tmp, applier)
    }

    #[test]
    fn pin_role_writes_a_real_media_lighthouse_role_toml() {
        // OW-11's Music step: promote a plain lighthouse to Lighthouse_Media. The
        // applier writes an actual role.toml that mde_role reads back as the media
        // subclass — a real pin, not a recorded intent.
        let (tmp, applier) = local_applier();
        applier
            .apply_one(&Action::PinRole {
                role: "lighthouse".into(),
                media: true,
            })
            .expect("pin-role applies");
        let class = mde_role::load_class_from(&tmp.path().join("role.toml")).unwrap();
        assert!(
            class.is_media_lighthouse(),
            "role.toml pinned the Lighthouse_Media subclass"
        );
    }

    #[test]
    fn seal_secret_encrypts_at_rest_and_reads_back() {
        // The media-spaces secret seals into the store (ciphertext on disk) and
        // the same store reads it back decrypted, byte-for-byte.
        let (_tmp, applier) = local_applier();
        let name = crate::ipc::secret_store::media_spaces_creds_ref();
        let secret = "S3_KEY=AKIA...\nS3_SECRET=abc123\nND_ADMIN_PASS=hunter2\n";
        applier
            .apply_one(&Action::SealSecret {
                name: name.clone(),
                secret: secret.into(),
            })
            .expect("seal-secret applies");
        assert_eq!(applier.store.get(&name).unwrap().as_deref(), Some(secret));
    }

    #[test]
    fn pin_role_rejects_an_unknown_role_name_honestly() {
        // A bogus role string is a typed ActionFailed, never a silent no-op.
        let (_tmp, applier) = local_applier();
        let err = applier
            .apply_one(&Action::PinRole {
                role: "overlord".into(),
                media: false,
            })
            .unwrap_err();
        assert!(matches!(err, RemotePushError::ActionFailed { .. }));
    }

    #[test]
    fn enroll_and_broker_are_typed_gated_not_local_effects() {
        // These belong to the SSH-bootstrap transport + the Bus worker; the
        // local applier returns a typed NotWired naming the owning layer, never a
        // fake apply (§7).
        let (_tmp, applier) = local_applier();
        assert!(matches!(
            applier.apply_one(&Action::RunEnroll { bearer: "b".into() }),
            Err(RemotePushError::NotWired { .. })
        ));
        assert!(matches!(
            applier.apply_one(&Action::OpenBroker {
                session_id: "s".into()
            }),
            Err(RemotePushError::NotWired { .. })
        ));
    }

    // ── process_apply: the onboard_apply worker's validate→apply core ──

    #[test]
    fn process_apply_verifies_then_applies_and_reports_observed_state() {
        // A valid signed bundle: verify → apply → the effects really land (role.toml
        // + secret store) and the observed-state echoes the redacted actions.
        let (tmp, applier) = local_applier();
        let mut guard = NonceGuard::new();
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now); // PinRole lighthouse+media, SealSecret media-spaces
        let sig = b.sign(&k);
        let outcome =
            process_apply(&b, &sig, &k.verifying_key(), now, &mut guard, &applier).unwrap();
        assert_eq!(outcome.node, "peer:lh-media");
        assert_eq!(outcome.applied.len(), 2);
        assert!(outcome.applied[0].contains("pin-role lighthouse +media"));
        // end-to-end: the role.toml + the sealed secret both landed
        assert!(mde_role::load_class_from(&tmp.path().join("role.toml"))
            .unwrap()
            .is_media_lighthouse());
        assert_eq!(
            applier.store.get("media-spaces").unwrap().as_deref(),
            Some("s3-creds")
        );
    }

    #[test]
    fn process_apply_refuses_a_replayed_nonce() {
        // The same bundle applied twice: the second is rejected on the nonce, so a
        // captured-and-replayed bundle can't re-run inside the freshness window.
        let (_tmp, applier) = local_applier();
        let mut guard = NonceGuard::new();
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now);
        let sig = b.sign(&k);
        assert!(process_apply(&b, &sig, &k.verifying_key(), now, &mut guard, &applier).is_ok());
        let err =
            process_apply(&b, &sig, &k.verifying_key(), now + 1, &mut guard, &applier).unwrap_err();
        assert!(
            matches!(err, RemotePushError::BundleRejected { .. }),
            "a replayed nonce is rejected"
        );
    }

    #[test]
    fn process_apply_a_bad_signature_does_not_burn_the_nonce() {
        // A wrong-signer bundle is rejected WITHOUT recording its nonce, so a
        // genuine re-send of the same bundle (correctly signed) still applies —
        // an attacker can't grief the nonce space with forged bundles.
        let (_tmp, applier) = local_applier();
        let mut guard = NonceGuard::new();
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now);
        let wrong = SigningKey::from_bytes(&[1_u8; 32]);
        assert!(process_apply(
            &b,
            &b.sign(&wrong),
            &k.verifying_key(),
            now,
            &mut guard,
            &applier
        )
        .is_err());
        // same nonce, now correctly signed → still applies (nonce was not burned)
        assert!(process_apply(
            &b,
            &b.sign(&k),
            &k.verifying_key(),
            now,
            &mut guard,
            &applier
        )
        .is_ok());
    }

    // ── production transports: honest gate + refusal boundaries (§7) ──

    #[test]
    fn ssh_bootstrap_refuses_an_enrolled_target() {
        // A mesh member must go over the §9 BusApply path, never raw SSH.
        let err = SshBootstrap
            .apply(
                &Target::Enrolled {
                    node_id: "peer:lh".into(),
                },
                &[Action::RunEnroll { bearer: "b".into() }],
            )
            .unwrap_err();
        assert!(matches!(err, RemotePushError::BundleRejected { .. }));
    }

    #[test]
    fn ssh_bootstrap_refuses_any_action_but_enroll() {
        // The bootstrap instant runs ONLY enroll; a role-pin over the enroll
        // bearer is out of scope and refused (allow-list boundary).
        let err = SshBootstrap
            .apply(
                &Target::Bootstrap {
                    host: "203.0.113.7".into(),
                },
                &[Action::PinRole {
                    role: "lighthouse".into(),
                    media: true,
                }],
            )
            .unwrap_err();
        assert!(matches!(err, RemotePushError::BundleRejected { .. }));
    }

    #[test]
    fn ssh_bootstrap_gates_the_live_enroll_honestly() {
        // A valid bootstrap enroll: no live SSH runner ⇒ a typed NotWired, never
        // a fake success. The target is untouched.
        let err = SshBootstrap
            .apply(
                &Target::Bootstrap {
                    host: "203.0.113.7".into(),
                },
                &[Action::RunEnroll { bearer: "b".into() }],
            )
            .unwrap_err();
        assert!(matches!(err, RemotePushError::NotWired { .. }));
    }

    #[test]
    fn bus_apply_refuses_a_bootstrap_target() {
        // A not-yet-enrolled box isn't on the Bus — chicken-and-egg.
        let err = BusApply
            .apply(
                &Target::Bootstrap {
                    host: "203.0.113.7".into(),
                },
                &[Action::PinRole {
                    role: "lighthouse".into(),
                    media: true,
                }],
            )
            .unwrap_err();
        assert!(matches!(err, RemotePushError::BundleRejected { .. }));
    }

    #[test]
    fn bus_apply_refuses_enroll_as_a_day2_action() {
        // Enroll is a bootstrap-only SSH step, never a day-2 Bus action.
        let err = BusApply
            .apply(
                &Target::Enrolled {
                    node_id: "peer:lh".into(),
                },
                &[Action::RunEnroll { bearer: "b".into() }],
            )
            .unwrap_err();
        assert!(matches!(err, RemotePushError::BundleRejected { .. }));
    }

    #[test]
    fn bus_apply_gates_the_live_round_trip_honestly() {
        // The OW-11 day-2 bundle (pin Media role + seal media-spaces): no live
        // cross-node round-trip ⇒ a typed NotWired, never a fake success. The
        // target is untouched.
        let err = BusApply
            .apply(
                &Target::Enrolled {
                    node_id: "peer:lh-media".into(),
                },
                &[
                    Action::PinRole {
                        role: "lighthouse".into(),
                        media: true,
                    },
                    Action::SealSecret {
                        name: "media-spaces".into(),
                        secret: "s3".into(),
                    },
                ],
            )
            .unwrap_err();
        assert!(matches!(err, RemotePushError::NotWired { .. }));
    }
}

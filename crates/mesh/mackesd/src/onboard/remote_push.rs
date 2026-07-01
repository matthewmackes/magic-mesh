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

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The bounded, exhaustive set of effects the executor will apply on a target.
///
/// Adding a remote capability means adding a variant here **and** handling it in
/// every apply path — there is deliberately no escape hatch (no `RunShell`,
/// no `Exec(String)`). This enum *is* the security allow-list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Pin a deployment role/capability on the target (e.g. `Media` for OW-11's
    /// media-lighthouse, so its `navidrome_supervisor` provisions Navidrome).
    PinRole { role: String },
    /// Seal a named secret into the target's local store. The `sealed` value is
    /// the already-encrypted blob (LocalAead / age ciphertext) — it is opaque
    /// here and MUST NOT be logged; only the `name` is loggable.
    SealSecret { name: String, sealed: Vec<u8> },
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
            Self::PinRole { role } => format!("pin-role {role}"),
            Self::SealSecret { name, sealed } => {
                format!("seal-secret {name} ({} bytes, redacted)", sealed.len())
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
                role: "Media".into(),
            },
            Action::SealSecret {
                name: "media-spaces".into(),
                sealed: vec![1],
            },
        ];
        assert!(apply_all(&app, &actions).is_ok());
        assert_eq!(app.applied.borrow().len(), 2);
        assert!(app.applied.borrow()[0].contains("pin-role Media"));
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
                role: "Media".into(),
            },
            Action::SealSecret {
                name: "s".into(),
                sealed: vec![1],
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
                    role: "Media".into(),
                },
                Action::SealSecret {
                    name: "media-spaces".into(),
                    sealed: vec![1, 2, 3],
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
            role: "Server".into(),
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
        // the sealed blob + the bearer must never appear in the log line
        let seal = Action::SealSecret {
            name: "media-spaces".into(),
            sealed: vec![7; 32],
        };
        assert!(seal.redacted().contains("media-spaces"));
        assert!(seal.redacted().contains("redacted"));
        assert!(!seal.redacted().contains(&7u8.to_string().repeat(2)));
        let enroll = Action::RunEnroll {
            bearer: "SECRET-BEARER".into(),
        };
        assert!(!enroll.redacted().contains("SECRET-BEARER"));
    }
}

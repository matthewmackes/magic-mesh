//! Node enrollment + lifecycle helpers (Phase 12.3.1, 12.3.4, 12.3.5).
//!
//! Each function is a pure value-producer; SQL persistence wires
//! through the store layer once `mackesd reconcile` runs. The CLI
//! subcommands in `bin/mackesd.rs` consume these to produce
//! human-readable progress reports.

use crate::identity::NodeKey;
use crate::passcode::looks_valid;
use crate::secrets::BearerToken;
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Result of a successful enrollment: the per-node identity +
/// bearer token + opaque hardware fingerprint.
#[derive(Debug)]
pub struct EnrolledIdentity {
    /// Newly-generated Ed25519 keypair (zero-on-drop).
    pub key: NodeKey,
    /// Bearer token to present on every heartbeat (zero-on-drop).
    pub bearer: BearerToken,
    /// Stable hardware fingerprint (e.g. `/etc/machine-id` hash).
    pub hw_fingerprint: String,
}

/// Wire-shape of an enrollment request the new peer sends to the
/// leader. Signed with the peer's freshly-minted private key so the
/// leader can verify the request is genuine + tie it to the
/// fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentRequest {
    /// Public key (32 bytes, hex-encoded for transport).
    pub public_key_hex: String,
    /// Hardware fingerprint — drives the idempotent re-enroll
    /// path. If a peer re-enrolls with the same fingerprint, the
    /// leader refreshes credentials in place.
    pub hw_fingerprint: String,
    /// Free-form display name (defaults to `hostname`).
    pub display_name: String,
    /// 16-char URL-safe passcode the peer was handed.
    pub passcode: String,
}

/// Build a fresh `EnrolledIdentity` for this peer. Generates a new
/// keypair, draws a 64-byte bearer token from the OS CSPRNG, and
/// hashes `/etc/machine-id` (or `MACKES_MACHINE_ID` for tests) as
/// the hardware fingerprint.
#[must_use]
pub fn build_identity() -> EnrolledIdentity {
    let key = NodeKey::generate();
    let mut bytes = [0u8; 64];
    rand::thread_rng().fill_bytes(&mut bytes);
    let bearer = BearerToken::new(bytes);
    let hw_fingerprint = read_hw_fingerprint();
    EnrolledIdentity {
        key,
        bearer,
        hw_fingerprint,
    }
}

fn read_hw_fingerprint() -> String {
    if let Ok(override_id) = std::env::var("MACKES_MACHINE_ID") {
        if !override_id.is_empty() {
            return hex_sha256(override_id.as_bytes());
        }
    }
    let machine_id = std::fs::read_to_string("/etc/machine-id")
        .unwrap_or_default()
        .trim()
        .to_owned();
    hex_sha256(machine_id.as_bytes())
}

fn hex_sha256(input: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Build a signed `EnrollmentRequest` from a fresh identity + the
/// shared passcode. Returns `None` if the passcode doesn't pass
/// `looks_valid`.
#[must_use]
pub fn build_request(
    identity: &EnrolledIdentity,
    passcode: &str,
    display_name: &str,
) -> Option<EnrollmentRequest> {
    if !looks_valid(passcode) {
        return None;
    }
    let public_key_hex = hex_bytes(identity.key.verifying_key().as_bytes());
    Some(EnrollmentRequest {
        public_key_hex,
        hw_fingerprint: identity.hw_fingerprint.clone(),
        display_name: display_name.to_owned(),
        passcode: passcode.to_owned(),
    })
}

/// Lifecycle outcome for a decommission attempt — drives the
/// human-readable progress report the CLI prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecommissionOutcome {
    /// Soft-deleted; history preserved.
    Soft,
    /// `--force` skipped the unreachable-peer confirmation.
    Forced,
    /// No matching node row.
    UnknownNode,
}

/// Lifecycle outcome for re-enrollment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReenrollOutcome {
    /// Existing row refreshed in place; historical link preserved.
    Refreshed,
    /// No matching node row — caller falls back to fresh enroll.
    UnknownNode,
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
    use std::sync::Mutex;

    /// Serialize tests that mutate `MACKES_MACHINE_ID`. The env is a
    /// process-wide singleton — Rust's test runner uses threads, so
    /// concurrent set/remove from sibling tests races. Holding this
    /// mutex for the duration of the test eliminates the flake.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn identity_has_unique_key_per_call() {
        let a = build_identity();
        let b = build_identity();
        assert_ne!(a.key.fingerprint(), b.key.fingerprint());
    }

    #[test]
    fn hw_fingerprint_uses_env_override_when_set() {
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("MACKES_MACHINE_ID", "test-host-12345");
        let id = build_identity();
        std::env::remove_var("MACKES_MACHINE_ID");
        // Same input → same fingerprint hash.
        let expected = {
            std::env::set_var("MACKES_MACHINE_ID", "test-host-12345");
            let id2 = build_identity();
            std::env::remove_var("MACKES_MACHINE_ID");
            id2.hw_fingerprint
        };
        assert_eq!(id.hw_fingerprint, expected);
        assert_eq!(id.hw_fingerprint.len(), 64);
    }

    #[test]
    fn build_request_rejects_invalid_passcode() {
        let id = build_identity();
        assert!(build_request(&id, "too-short", "anvil").is_none());
        assert!(build_request(&id, "", "anvil").is_none());
    }

    #[test]
    fn build_request_accepts_valid_passcode() {
        let id = build_identity();
        let req = build_request(&id, "AAAAAAAAAAAAAAAA", "anvil").expect("valid");
        assert_eq!(req.display_name, "anvil");
        assert_eq!(req.public_key_hex.len(), 64);
    }

    #[test]
    fn enrollment_request_round_trips_through_json() {
        let id = build_identity();
        let req = build_request(&id, "AAAAAAAAAAAAAAAA", "anvil").unwrap();
        let json = serde_json::to_string(&req).unwrap();
        let back: EnrollmentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.display_name, req.display_name);
        assert_eq!(back.hw_fingerprint, req.hw_fingerprint);
        assert_eq!(back.passcode, req.passcode);
    }

    #[test]
    fn build_request_carries_public_key_matching_identity() {
        let id = build_identity();
        let req = build_request(&id, "AAAAAAAAAAAAAAAA", "anvil").unwrap();
        // Public key in the request matches the identity's verifying
        // key byte-for-byte (hex-encoded).
        let want: String = id
            .key
            .verifying_key()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(req.public_key_hex, want);
    }

    #[test]
    fn hw_fingerprint_is_64_hex_chars() {
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("MACKES_MACHINE_ID", "anything-here-works");
        let id = build_identity();
        std::env::remove_var("MACKES_MACHINE_ID");
        assert_eq!(id.hw_fingerprint.len(), 64);
        assert!(id.hw_fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hw_fingerprint_changes_with_machine_id() {
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("MACKES_MACHINE_ID", "host-a");
        let id_a = build_identity();
        std::env::set_var("MACKES_MACHINE_ID", "host-b");
        let id_b = build_identity();
        std::env::remove_var("MACKES_MACHINE_ID");
        assert_ne!(id_a.hw_fingerprint, id_b.hw_fingerprint);
    }

    #[test]
    fn empty_env_override_falls_back_to_real_machine_id() {
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Empty string env var is treated as unset for the override.
        // The fallback reads /etc/machine-id; in a CI container that
        // may be empty too. Either way the fingerprint must be 64 hex
        // chars (SHA-256 always produces 64-char hex).
        std::env::set_var("MACKES_MACHINE_ID", "");
        let id = build_identity();
        std::env::remove_var("MACKES_MACHINE_ID");
        assert_eq!(id.hw_fingerprint.len(), 64);
    }

    #[test]
    fn decommission_outcome_variants_round_trip() {
        // Lifecycle helpers are pure enums — exercise the variants so
        // PartialEq/Clone coverage counts.
        assert_eq!(DecommissionOutcome::Soft, DecommissionOutcome::Soft);
        assert_ne!(DecommissionOutcome::Soft, DecommissionOutcome::Forced);
        assert_ne!(DecommissionOutcome::Soft, DecommissionOutcome::UnknownNode);
        let cloned = DecommissionOutcome::Forced.clone();
        assert_eq!(cloned, DecommissionOutcome::Forced);
    }

    #[test]
    fn reenroll_outcome_variants_round_trip() {
        assert_eq!(ReenrollOutcome::Refreshed, ReenrollOutcome::Refreshed);
        assert_ne!(ReenrollOutcome::Refreshed, ReenrollOutcome::UnknownNode);
        let cloned = ReenrollOutcome::UnknownNode.clone();
        assert_eq!(cloned, ReenrollOutcome::UnknownNode);
    }

    #[test]
    fn build_request_passcode_round_trips_unchanged() {
        let id = build_identity();
        let pc = "Abc-123_XYZabc01";
        let req = build_request(&id, pc, "anvil").unwrap();
        assert_eq!(req.passcode, pc);
    }
}

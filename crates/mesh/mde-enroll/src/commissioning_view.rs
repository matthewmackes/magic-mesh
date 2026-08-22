//! WL-FUNC-023 S6 leftover — renderer-neutral token and capsule projection.
//!
//! GUI and TUI clients share this view so an enroll screen can show mesh,
//! target, pin, expiry, and digest identity without displaying a bearer or a
//! capsule signature. The command-template placeholder is not a minted token.

use mackes_mesh_types::lifecycle::CommissioningCapsuleV1;
use mackesd_core::nebula_enroll::parse_join_token;

/// Command-template placeholder refused by the lifecycle authority when no
/// minted bearer is present. A renderer must not present it as enrollment
/// material.
const JOIN_TOKEN_TEMPLATE: &str = "{{JOIN_TOKEN}}";

const DIGEST_PREFIX_CHARS: usize = 8;
const FINGERPRINT_PREFIX_CHARS: usize = 8;

/// Honest join-token projection: identity and pin only. The bearer never
/// enters this struct, so a renderer cannot leak it by printing the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinTokenView {
    pub mesh_id: String,
    pub lighthouse: String,
    pub port: u16,
    pub fingerprint_prefix: Option<String>,
    pub minted: bool,
}

impl JoinTokenView {
    /// Project a pasted token. Empty input and the authority template are
    /// refused as "not minted"; garbage is refused as invalid. A successful
    /// view is always `minted: true` and never carries the bearer.
    pub fn from_wire(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.contains(JOIN_TOKEN_TEMPLATE) {
            return Err("join token is a command template, not a minted bearer".to_owned());
        }
        let token = parse_join_token(trimmed).ok_or_else(|| "invalid join token".to_owned())?;
        if token.bearer == JOIN_TOKEN_TEMPLATE || token.bearer.contains(JOIN_TOKEN_TEMPLATE) {
            return Err("join token is a command template, not a minted bearer".to_owned());
        }
        Ok(Self {
            mesh_id: token.mesh_id,
            lighthouse: token.lighthouse,
            port: token.port,
            fingerprint_prefix: token
                .fp
                .as_deref()
                .map(|fp| fp.chars().take(FINGERPRINT_PREFIX_CHARS).collect()),
            minted: true,
        })
    }

    pub fn status_line(&self) -> String {
        let pin = self
            .fingerprint_prefix
            .as_deref()
            .map(|prefix| format!("fp {prefix}"))
            .unwrap_or_else(|| "unpinned".to_owned());
        format!(
            "token mesh:{} @ {}:{} ({pin}, bearer withheld)",
            self.mesh_id, self.lighthouse, self.port
        )
    }
}

/// Honest commissioning-capsule projection: target binding and digest prefix.
/// The signature never enters this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapsuleView {
    pub capsule_id: String,
    pub target_id: String,
    pub expires_at_ms: i64,
    pub digest_prefix: String,
    pub one_time: bool,
}

impl CapsuleView {
    /// Decode and bound-check a capsule at `now_ms`. Expired, replayable, or
    /// unsigned-looking envelopes refuse before any renderer line is built.
    pub fn from_wire(capsule_json: &str, now_ms: i64) -> Result<Self, String> {
        let capsule: CommissioningCapsuleV1 = serde_json::from_str(capsule_json)
            .map_err(|_| "invalid commissioning capsule".to_owned())?;
        capsule
            .validate_at(now_ms)
            .map_err(|error| format!("invalid commissioning capsule: {error:?}"))?;
        Ok(Self {
            capsule_id: capsule.capsule_id,
            target_id: capsule.target_id,
            expires_at_ms: capsule.expires_at_ms,
            digest_prefix: capsule
                .bootstrap_digest_hex
                .chars()
                .take(DIGEST_PREFIX_CHARS)
                .collect(),
            one_time: capsule.one_time,
        })
    }

    pub fn status_line(&self) -> String {
        format!(
            "capsule {} → {} digest {}… (one-time {}, expires {}, signature withheld)",
            self.capsule_id, self.target_id, self.digest_prefix, self.one_time, self.expires_at_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINTED_BEARER: &str = "single-use-bearer";
    const MINTED_TOKEN: &str = "mesh:home@10.0.0.5:4243#single-use-bearer?fp=\
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn capsule_json(expires_at_ms: i64, one_time: bool) -> String {
        serde_json::json!({
            "schema_version": 1,
            "capsule_id": "capsule-1",
            "target_id": "seat-15",
            "expires_at_ms": expires_at_ms,
            "bootstrap_digest_hex": "b".repeat(64),
            "one_time": one_time,
            "key_id": "commissioning-v1",
            "signature_hex": "c".repeat(128),
        })
        .to_string()
    }

    #[test]
    fn minted_token_projects_identity_and_withholds_the_bearer() {
        let view = JoinTokenView::from_wire(MINTED_TOKEN).unwrap();
        assert_eq!(view.mesh_id, "home");
        assert_eq!(view.lighthouse, "10.0.0.5");
        assert_eq!(view.port, 4243);
        assert_eq!(view.fingerprint_prefix.as_deref(), Some("aaaaaaaa"));
        assert!(view.minted);
        let line = view.status_line();
        assert!(line.contains("bearer withheld"));
        assert!(
            !line.contains(MINTED_BEARER),
            "status line leaked the bearer: {line}"
        );
        let debug = format!("{view:?}");
        assert!(
            !debug.contains(MINTED_BEARER),
            "view debug leaked the bearer: {debug}"
        );
    }

    #[test]
    fn template_and_empty_tokens_are_not_minted_bearers() {
        for raw in ["", "   ", JOIN_TOKEN_TEMPLATE, "{{JOIN_TOKEN}} extra"] {
            let error = JoinTokenView::from_wire(raw).expect_err(raw);
            assert_eq!(
                error, "join token is a command template, not a minted bearer",
                "raw={raw:?}"
            );
        }
        assert!(JoinTokenView::from_wire("garbage").is_err());
    }

    #[test]
    fn capsule_projects_binding_and_withholds_the_signature() {
        let signature = "c".repeat(128);
        let view = CapsuleView::from_wire(&capsule_json(2_000, true), 1_000).unwrap();
        assert_eq!(view.capsule_id, "capsule-1");
        assert_eq!(view.target_id, "seat-15");
        assert_eq!(view.digest_prefix, "bbbbbbbb");
        assert!(view.one_time);
        let line = view.status_line();
        assert!(line.contains("signature withheld"));
        assert!(
            !line.contains(&signature),
            "status line leaked the signature: {line}"
        );
        let debug = format!("{view:?}");
        assert!(
            !debug.contains(&signature),
            "view debug leaked the signature: {debug}"
        );
    }

    #[test]
    fn expired_or_replayable_capsules_do_not_project() {
        assert!(CapsuleView::from_wire(&capsule_json(1_000, true), 1_000).is_err());
        assert!(CapsuleView::from_wire(&capsule_json(2_000, false), 1_000).is_err());
    }
}

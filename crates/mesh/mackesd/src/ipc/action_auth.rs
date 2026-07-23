//! Shared authorization boundary for privileged `action/*` consumers.
//!
//! `/run/mde-bus` is intentionally writable across UIDs. Possession of an
//! action topic is therefore transport reachability, not administrative
//! authority. Privileged consumers pass the exact request body through
//! [`ActionAuthorizer::authorize`] before invoking a runner/backend.
//!
//! The verifier accepts only schema v1, an exact-body-bound HMAC capability
//! whose remaining lifetime is at most 30 seconds, and a nonce that can be
//! durably claimed in the shared host-local replay ledger. Missing credentials
//! install a verifier that rejects everything. The authorizer exposes no mint
//! API in production; mint authority remains in the root shell.

use std::path::PathBuf;
use std::sync::Arc;

use mackes_mesh_types::cloud::CloudArmedToken;

use crate::workers::cloud::{
    claim_nonce, verify_token, HmacTokenSigner, NullSigner, TokenSigner, DEFAULT_AUTH_ROOT,
};

/// Current privileged-action envelope schema.
pub const ACTION_SCHEMA_VERSION: u64 = 1;

/// Maximum capability lifetime accepted by a privileged consumer.
pub const MAX_AUTH_TTL_MS: i64 = 30_000;

/// The semantic context an HMAC capability must bind in addition to the exact
/// request body.
#[derive(Debug, Clone, Copy)]
pub struct MutationContext<'a> {
    /// Closed mutation verb, for example `pty-open` or `storage-apply`.
    pub verb: &'a str,
    /// Placement/consumer node scope.
    pub node: &'a str,
    /// Stable mutation target within that node scope.
    pub target: &'a str,
}

type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Verifier-only privileged-action gate shared by Bus consumers.
pub struct ActionAuthorizer {
    verifier: Arc<dyn TokenSigner>,
    auth_root: PathBuf,
    now: NowFn,
}

impl std::fmt::Debug for ActionAuthorizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionAuthorizer")
            .field("auth_root", &self.auth_root)
            .finish_non_exhaustive()
    }
}

impl ActionAuthorizer {
    /// Load the root-only systemd credential. A missing/unusable credential
    /// fails closed: the returned gate rejects every privileged mutation.
    #[must_use]
    pub fn production() -> Self {
        let verifier: Arc<dyn TokenSigner> = match HmacTokenSigner::from_systemd_credential() {
            Ok(verifier) => Arc::new(verifier),
            Err(error) => {
                tracing::error!(
                    target: "mackesd::action_auth",
                    %error,
                    "privileged Bus authorization unavailable; mutations are disabled"
                );
                Arc::new(NullSigner)
            }
        };
        Self {
            verifier,
            auth_root: PathBuf::from(DEFAULT_AUTH_ROOT),
            now: Arc::new(wall_now_ms),
        }
    }

    /// Verify schema, exact-body authority, bounded freshness, and durably
    /// consume the capability nonce. The nonce is claimed only after every
    /// other check succeeds.
    ///
    /// # Errors
    ///
    /// A log-safe refusal reason. The raw request body/token is never copied.
    pub fn authorize(&self, body: &str, context: MutationContext<'_>) -> Result<(), String> {
        if !super::body_within_cap(Some(body)) {
            return Err("request body exceeds the 64 KiB cap".to_string());
        }
        let envelope: serde_json::Value = serde_json::from_str(body)
            .map_err(|_| "request body is not a JSON object".to_string())?;
        let object = envelope
            .as_object()
            .ok_or_else(|| "request body is not a JSON object".to_string())?;
        if object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(ACTION_SCHEMA_VERSION)
        {
            return Err(format!(
                "privileged action requires schema_version {ACTION_SCHEMA_VERSION}"
            ));
        }
        let raw_token = object
            .get("armed_token")
            .and_then(serde_json::Value::as_str);
        let now = (self.now)();
        let verdict = verify_token(
            raw_token,
            context.verb,
            context.node,
            context.target,
            body,
            now,
            self.verifier.as_ref(),
        );
        if !verdict.is_valid() {
            return Err(verdict.reason().to_string());
        }
        let token = raw_token
            .and_then(CloudArmedToken::parse)
            .ok_or_else(|| "armed token is malformed".to_string())?;
        if token.expires_at_ms > now.saturating_add(MAX_AUTH_TTL_MS) {
            return Err("armed token exceeds the 30-second lifetime".to_string());
        }
        match claim_nonce(&self.auth_root, &token.nonce, token.expires_at_ms, now) {
            Ok(true) => Ok(()),
            Ok(false) => Err("armed token was already used".to_string()),
            Err(_) => Err("armed-token replay store is unavailable".to_string()),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn for_test(key: &[u8], auth_root: PathBuf, now_ms: i64) -> Self {
        Self {
            verifier: Arc::new(HmacTokenSigner::new(key.to_vec())),
            auth_root,
            now: Arc::new(move || now_ms),
        }
    }
}

fn wall_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) fn authorize_test_body(
    key: &[u8],
    unsigned_body: &str,
    context: MutationContext<'_>,
    nonce: &str,
    expires_at_ms: i64,
) -> String {
    use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmedToken};

    let signer = HmacTokenSigner::new(key.to_vec());
    let padded_nonce;
    let nonce = if nonce.len() >= 32 {
        nonce
    } else {
        padded_nonce = format!("{nonce}-0123456789abcdef0123456789abcdef");
        &padded_nonce
    };
    let token = CloudArmedToken::mint(
        &signer,
        nonce,
        expires_at_ms,
        context.verb,
        context.node,
        context.target,
        &cloud_request_digest(unsigned_body).expect("test request is valid JSON"),
    )
    .encode();
    let mut body: serde_json::Value =
        serde_json::from_str(unsigned_body).expect("test request is valid JSON");
    body.as_object_mut()
        .expect("test request is an object")
        .insert("armed_token".to_string(), serde_json::Value::String(token));
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"shared-action-auth-test-key";
    const NOW: i64 = 1_700_000_000_000;

    fn context() -> MutationContext<'static> {
        MutationContext {
            verb: "storage-apply",
            node: "eagle",
            target: "/dev/sdb",
        }
    }

    #[test]
    fn exact_body_capability_is_single_use_and_future_schema_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let gate = ActionAuthorizer::for_test(KEY, tmp.path().to_path_buf(), NOW);
        let unsigned = r#"{"armed_device":"/dev/sdb","schema_version":1,"verb":"apply"}"#;
        let armed = authorize_test_body(KEY, unsigned, context(), "nonce-once", NOW + 30_000);
        assert!(gate.authorize(&armed, context()).is_ok());
        assert!(gate
            .authorize(&armed, context())
            .unwrap_err()
            .contains("already used"));

        let future = armed.replace("\"schema_version\":1", "\"schema_version\":2");
        assert!(gate
            .authorize(&future, context())
            .unwrap_err()
            .contains("schema_version 1"));
    }

    #[test]
    fn tamper_and_overlong_lifetime_are_refused_without_claiming_nonce() {
        let tmp = tempfile::tempdir().unwrap();
        let gate = ActionAuthorizer::for_test(KEY, tmp.path().to_path_buf(), NOW);
        let unsigned = r#"{"armed_device":"/dev/sdb","schema_version":1,"verb":"apply"}"#;
        let armed = authorize_test_body(KEY, unsigned, context(), "nonce-tamper", NOW + 30_000);
        let tampered = armed.replace("/dev/sdb", "/dev/sdc");
        assert!(gate.authorize(&tampered, context()).is_err());
        assert!(gate.authorize(&armed, context()).is_ok());

        let long = authorize_test_body(KEY, unsigned, context(), "nonce-long", NOW + 30_001);
        assert!(gate
            .authorize(&long, context())
            .unwrap_err()
            .contains("30-second"));
    }
}

//! IAC-1 — Keystone v3 authentication: mint a token + read the service catalog.
//!
//! Design Q20/#2: the client authenticates against Keystone with the
//! [`CloudConfig`] loaded from `clouds.yaml`, and the **token response carries
//! the whole service catalog** — so one auth call yields both the bearer token
//! (for the resource/verb calls) and the authoritative service directory the IAC
//! surface renders.
//!
//! The seam is [`KeystoneAuth`]; the production impl [`KeystoneHttp`] issues the
//! standard `POST {auth_url}/auth/tokens` (the password rides the **JSON body**,
//! never argv, Q20). The pure pieces — [`build_password_auth_body`] +
//! [`token_url`] — are fixture-tested so the request shape can't silently drift;
//! [`mackes_mesh_types::openstack::ServiceCatalog::from_keystone_token_json`]
//! parses the catalog out of the response.

use std::time::Duration;

use serde::Deserialize;

use mackes_mesh_types::openstack::ServiceCatalog;

use super::config::CloudConfig;
use super::ClientError;

/// An authenticated Keystone session — the bearer token + the catalog it came
/// with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The Keystone `X-Subject-Token` — the bearer for every subsequent API
    /// call (`X-Auth-Token`).
    pub token: String,
    /// The service catalog the token response advertised.
    pub catalog: ServiceCatalog,
    /// The token's `expires_at`, when the response carried one (RFC3339).
    pub expires_at: Option<String>,
}

/// The injectable Keystone auth seam. [`KeystoneHttp`] is the production impl;
/// tests inject [`super::testkit::FakeKeystone`].
pub trait KeystoneAuth {
    /// Authenticate `cfg` and return the token + catalog.
    ///
    /// # Errors
    /// A typed [`ClientError`] on a transport failure or a non-2xx Keystone
    /// response (bad credentials, unreachable identity) — never a fabricated
    /// session.
    fn authenticate(&self, cfg: &CloudConfig) -> Result<Session, ClientError>;
}

// ─────────────────── pure: the v3 auth request builders ───────────────────

/// The Keystone token endpoint for an `auth_url`.
///
/// openstacksdk's `auth_url` is the identity root, usually already versioned
/// (`http://keystone.mesh:5000/v3`); the token endpoint is `<root>/auth/tokens`.
/// A trailing slash is normalized, and an un-versioned root gets `/v3` appended
/// so both `clouds.yaml` conventions work.
#[must_use]
pub fn token_url(auth_url: &str) -> String {
    let trimmed = auth_url.trim().trim_end_matches('/');
    // Append the version segment when the root doesn't already carry one.
    let versioned = if trimmed.ends_with("/v3") || trimmed.ends_with("/v2.0") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v3")
    };
    format!("{versioned}/auth/tokens")
}

/// Build the v3 **password** auth request body (scoped to the project when the
/// config names one).
///
/// The password rides this JSON body (never a command line, Q20). Domains
/// default to `Default` (set in [`CloudConfig`]). An unscoped config (no
/// `project_name`) produces an unscoped-token request — honest: the caller gets
/// whatever scope Keystone grants.
#[must_use]
pub fn build_password_auth_body(cfg: &CloudConfig) -> serde_json::Value {
    let identity = serde_json::json!({
        "methods": ["password"],
        "password": {
            "user": {
                "name": cfg.username,
                "domain": { "name": cfg.user_domain },
                "password": cfg.password,
            }
        }
    });
    // Scope to the project when one is named; an unscoped config omits `scope`.
    let mut auth = serde_json::json!({ "identity": identity });
    if let Some(project) = &cfg.project_name {
        auth["scope"] = serde_json::json!({
            "project": {
                "name": project,
                "domain": { "name": cfg.project_domain },
            }
        });
    }
    serde_json::json!({ "auth": auth })
}

/// Parse a Keystone token response body for its `expires_at`, when present.
fn parse_expires_at(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Root {
        token: Token,
    }
    #[derive(Deserialize)]
    struct Token {
        #[serde(default)]
        expires_at: Option<String>,
    }
    serde_json::from_str::<Root>(body.trim())
        .ok()
        .and_then(|r| r.token.expires_at)
        .filter(|s| !s.trim().is_empty())
}

/// Assemble a [`Session`] from a raw token header + response body (pure — shared
/// by the production impl and testable in isolation).
///
/// # Errors
/// [`ClientError::Auth`] on a missing token; [`ClientError::Catalog`] when the
/// body has no parseable catalog.
pub fn session_from_response(token: &str, body: &str) -> Result<Session, ClientError> {
    if token.trim().is_empty() {
        return Err(ClientError::Auth(
            "Keystone returned no X-Subject-Token".to_string(),
        ));
    }
    let catalog = ServiceCatalog::from_keystone_token_json(body)
        .map_err(|e| ClientError::Catalog(e.to_string()))?;
    Ok(Session {
        token: token.to_string(),
        catalog,
        expires_at: parse_expires_at(body),
    })
}

// ─────────────────────────── the HTTP production impl ───────────────────────────

/// The bound on one Keystone auth call.
pub const KEYSTONE_TIMEOUT: Duration = Duration::from_secs(30);

/// Production [`KeystoneAuth`]: the standard `POST {auth_url}/auth/tokens`.
///
/// Reaches Keystone over the mesh overlay (plaintext HTTP — the overlay is the
/// transport security, per the QUASAR-CLOUD design). Blocking, so a caller drives
/// it on a blocking thread (`reqwest::blocking` offloads to its own runtime, so
/// it is safe to call from within the async worker's `spawn_blocking` drain).
#[derive(Debug, Clone, Default)]
pub struct KeystoneHttp;

impl KeystoneHttp {
    /// Construct the production Keystone client.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl KeystoneAuth for KeystoneHttp {
    fn authenticate(&self, cfg: &CloudConfig) -> Result<Session, ClientError> {
        let url = token_url(&cfg.auth_url);
        let body = build_password_auth_body(cfg);
        let client = reqwest::blocking::Client::builder()
            .timeout(KEYSTONE_TIMEOUT)
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| ClientError::Transport(format!("POST {url}: {e}")))?;
        let status = resp.status();
        // The token is a response header; capture it before consuming the body.
        let token = resp
            .headers()
            .get("x-subject-token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .unwrap_or_default();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(ClientError::Auth(format!(
                "Keystone auth failed (HTTP {}): {}",
                status.as_u16(),
                text.trim()
            )));
        }
        session_from_response(&token, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::openstack::client::config::EndpointInterface;

    fn cfg() -> CloudConfig {
        CloudConfig {
            cloud: "mesh".into(),
            auth_url: "http://keystone.mesh:5000/v3".into(),
            username: "operator".into(),
            password: "s3cr3t".into(),
            project_name: Some("mesh".into()),
            project_domain: "Default".into(),
            user_domain: "Default".into(),
            region_name: Some("RegionOne".into()),
            interface: EndpointInterface::Public,
        }
    }

    #[test]
    fn token_url_versions_and_normalizes() {
        assert_eq!(
            token_url("http://keystone.mesh:5000/v3"),
            "http://keystone.mesh:5000/v3/auth/tokens"
        );
        assert_eq!(
            token_url("http://keystone.mesh:5000/v3/"),
            "http://keystone.mesh:5000/v3/auth/tokens"
        );
        // An un-versioned root gets /v3 appended.
        assert_eq!(
            token_url("http://keystone.mesh:5000"),
            "http://keystone.mesh:5000/v3/auth/tokens"
        );
    }

    #[test]
    fn password_body_scopes_to_the_project_and_carries_no_argv() {
        let body = build_password_auth_body(&cfg());
        // The scoped-password shape Keystone expects.
        assert_eq!(body["auth"]["identity"]["methods"][0], "password");
        assert_eq!(
            body["auth"]["identity"]["password"]["user"]["name"],
            "operator"
        );
        assert_eq!(
            body["auth"]["identity"]["password"]["user"]["password"],
            "s3cr3t"
        );
        assert_eq!(body["auth"]["scope"]["project"]["name"], "mesh");
        assert_eq!(
            body["auth"]["scope"]["project"]["domain"]["name"],
            "Default"
        );
    }

    #[test]
    fn an_unscoped_config_omits_the_scope() {
        let mut c = cfg();
        c.project_name = None;
        let body = build_password_auth_body(&c);
        assert!(body["auth"].get("scope").is_none());
    }

    #[test]
    fn session_from_a_real_token_response_parses_the_catalog() {
        let token = "gAAAAAtoken";
        let resp = r#"{
          "token": {
            "expires_at": "2026-07-04T12:00:00.000000Z",
            "catalog": [
              {"type":"compute","name":"nova","endpoints":[
                {"interface":"public","url":"http://nova.mesh:8774/v2.1","region":"RegionOne"}
              ]}
            ]
          }
        }"#;
        let session = session_from_response(token, resp).expect("session");
        assert_eq!(session.token, token);
        assert_eq!(
            session.expires_at.as_deref(),
            Some("2026-07-04T12:00:00.000000Z")
        );
        assert_eq!(session.catalog.services.len(), 1);
        assert_eq!(session.catalog.services[0].service_type, "compute");
    }

    #[test]
    fn a_missing_token_header_is_a_typed_auth_error() {
        // §7 — no token ⇒ honest failure, never a fabricated session.
        let err = session_from_response("", r#"{"token":{"catalog":[]}}"#)
            .expect_err("empty token must fail");
        assert!(matches!(err, ClientError::Auth(_)));
    }

    #[test]
    fn an_unparseable_catalog_is_a_typed_catalog_error() {
        let err =
            session_from_response("tok", r#"{"token":{}}"#).expect_err("no catalog must fail");
        assert!(matches!(err, ClientError::Catalog(_)));
    }
}

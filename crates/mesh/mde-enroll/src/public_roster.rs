//! Public lighthouse discovery for the enrollment UI.
//!
//! The public roster is a discovery convenience, not a trust authority.  The
//! join token remains the source of truth for mesh scope, bearer, port, and
//! the `/enroll` certificate fingerprint.  Selecting a roster name changes
//! only the TCP/TLS destination; the CSR still carries the unmodified token.

use std::fmt;
use std::net::Ipv4Addr;

use mackesd_core::nebula_enroll::JoinToken;

/// The operator-published public roster, in deterministic preference order.
///
/// Keep this list deliberately closed.  A DNS name outside this exact roster
/// is not a public-lighthouse discovery result and must not be accepted as a
/// hostname override by the installer.
pub const PUBLIC_LIGHTHOUSE_HOSTS: [&str; 3] = [
    "lighthouse1.ephemeral.team",
    "lighthouse2.ephemeral.team",
    "lighthouse3.ephemeral.team",
];

/// Help text shared by the enrollment UI and tests.
pub const PUBLIC_ROSTER_HELP: &str =
    "Public roster: Lighthouse1.ephemeral.team, Lighthouse2.ephemeral.team, Lighthouse3.ephemeral.team";

/// Where the enrollment worker will open its pinned TLS connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LighthouseEndpoint {
    /// A canonical IPv4 literal or one of [`PUBLIC_LIGHTHOUSE_HOSTS`].
    pub host: String,
    /// The port retained from the join token.
    pub port: u16,
    /// How this destination was selected, for diagnostics and tests.
    pub source: EndpointSource,
}

/// Selection provenance for a resolved endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointSource {
    /// The endpoint embedded in the join token was used unchanged.
    Token,
    /// An explicitly selected member of the closed public roster was used.
    PublicRoster,
    /// An explicitly entered IPv4 destination was used under the token's pin.
    PinnedOverride,
}

/// A fail-closed endpoint selection error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    /// The network path cannot authenticate without a certificate pin.
    MissingFingerprint,
    /// The token itself was manually constructed with a non-IPv4 endpoint.
    InvalidTokenEndpoint,
    /// The explicit field contained a hostname outside the public roster.
    UnlistedHostname(String),
    /// The explicit field was neither a listed hostname nor an IPv4 literal.
    InvalidOverride(String),
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFingerprint => write!(
                f,
                "a ?fp= certificate fingerprint is required for public or overridden endpoints"
            ),
            Self::InvalidTokenEndpoint => {
                write!(f, "join token carries an invalid lighthouse endpoint")
            }
            Self::UnlistedHostname(host) => write!(
                f,
                "unlisted lighthouse hostname {host:?} — use a public roster name or a pinned IPv4"
            ),
            Self::InvalidOverride(value) => write!(
                f,
                "invalid lighthouse override {value:?} — use a roster name or IPv4 address"
            ),
        }
    }
}

impl std::error::Error for EndpointError {}

/// Normalize a DNS name for closed-roster comparison.
///
/// DNS names are case-insensitive and a final root dot is equivalent, so
/// those two forms are normalized.  Empty labels, labels longer than 63
/// bytes, whitespace, controls, non-ASCII input, and other DNS punctuation
/// are rejected before roster lookup.
#[must_use]
pub fn normalize_hostname(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.is_ascii() || trimmed.len() > 253 {
        return None;
    }
    let without_root_dot = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if without_root_dot.is_empty() {
        return None;
    }
    if without_root_dot
        .chars()
        .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
    {
        return None;
    }

    let normalized = without_root_dot.to_ascii_lowercase();
    for label in normalized.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return None;
        }
    }
    Some(normalized)
}

/// Return the canonical public hostname for a normalized/case-insensitive
/// roster member, preserving the fixed roster order.
#[must_use]
pub fn public_hostname(input: &str) -> Option<&'static str> {
    let normalized = normalize_hostname(input)?;
    PUBLIC_LIGHTHOUSE_HOSTS
        .iter()
        .copied()
        .find(|host| *host == normalized)
}

/// Resolve the endpoint the worker may dial.
///
/// `override_input` is an explicit operator choice.  An empty value leaves
/// the token endpoint untouched.  A public roster name or IPv4 override is
/// allowed only when the token carries its certificate fingerprint; the
/// transport still performs the existing exact-fingerprint TLS verification.
/// The returned endpoint is intentionally separate from `JoinToken`, so an
/// override can never rewrite the token embedded in the CSR.
pub fn resolve_endpoint(
    token: &JoinToken,
    override_input: &str,
) -> Result<LighthouseEndpoint, EndpointError> {
    if token.fp.is_none() {
        return Err(EndpointError::MissingFingerprint);
    }
    if token.lighthouse.parse::<Ipv4Addr>().is_err() {
        return Err(EndpointError::InvalidTokenEndpoint);
    }

    let override_input = override_input.trim();
    if override_input.is_empty() {
        return Ok(LighthouseEndpoint {
            host: token.lighthouse.clone(),
            port: token.port,
            source: EndpointSource::Token,
        });
    }

    if let Some(host) = public_hostname(override_input) {
        return Ok(LighthouseEndpoint {
            host: host.to_owned(),
            port: token.port,
            source: EndpointSource::PublicRoster,
        });
    }

    // Preserve the existing explicit IPv4 override capability, but never
    // expand it to arbitrary DNS/TLS names.  The fingerprint check above is
    // mandatory, and the network client will pin the actual certificate.
    if override_input.parse::<Ipv4Addr>().is_ok() {
        return Ok(LighthouseEndpoint {
            host: override_input.to_owned(),
            port: token.port,
            source: EndpointSource::PinnedOverride,
        });
    }

    if normalize_hostname(override_input).is_some() {
        return Err(EndpointError::UnlistedHostname(override_input.to_owned()));
    }
    Err(EndpointError::InvalidOverride(override_input.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(fp: Option<&str>) -> JoinToken {
        JoinToken {
            mesh_id: "private-mesh".into(),
            lighthouse: "192.0.2.10".into(),
            port: 4243,
            bearer: "single-use-bearer".into(),
            fp: fp.map(str::to_owned),
        }
    }

    #[test]
    fn roster_order_is_stable_and_exact() {
        assert_eq!(
            PUBLIC_LIGHTHOUSE_HOSTS,
            [
                "lighthouse1.ephemeral.team",
                "lighthouse2.ephemeral.team",
                "lighthouse3.ephemeral.team",
            ]
        );
    }

    #[test]
    fn normalization_accepts_case_whitespace_and_root_dot() {
        assert_eq!(
            normalize_hostname("  LIGHTHOUSE2.EPHEMERAL.TEAM. "),
            Some("lighthouse2.ephemeral.team".into())
        );
        assert_eq!(
            public_hostname("LIGHTHOUSE1.EPHEMERAL.TEAM."),
            Some("lighthouse1.ephemeral.team")
        );
        assert!(normalize_hostname("lighthouse..ephemeral.team").is_none());
    }

    #[test]
    fn unlisted_hostname_fails_closed() {
        let err = resolve_endpoint(&token(Some("a".repeat(64).as_str())), "evil.example")
            .expect_err("unlisted host must not be a TLS destination");
        assert!(matches!(err, EndpointError::UnlistedHostname(_)));
    }

    #[test]
    fn missing_pin_rejects_roster_and_override_before_network() {
        let no_pin = token(None);
        assert_eq!(
            resolve_endpoint(&no_pin, "lighthouse1.ephemeral.team"),
            Err(EndpointError::MissingFingerprint)
        );
        assert_eq!(
            resolve_endpoint(&no_pin, "203.0.113.9"),
            Err(EndpointError::MissingFingerprint)
        );
    }

    #[test]
    fn blank_override_preserves_token_endpoint_and_csr_inputs() {
        let join = token(Some(&"a".repeat(64)));
        let endpoint = resolve_endpoint(&join, "").expect("token endpoint");
        assert_eq!(endpoint.host, join.lighthouse);
        assert_eq!(endpoint.port, join.port);
        assert_eq!(endpoint.source, EndpointSource::Token);
    }

    #[test]
    fn roster_override_changes_only_transport_destination() {
        let join = token(Some(&"a".repeat(64)));
        let endpoint =
            resolve_endpoint(&join, "Lighthouse3.ephemeral.team").expect("listed public host");
        assert_eq!(endpoint.host, "lighthouse3.ephemeral.team");
        assert_eq!(endpoint.port, join.port);
        assert_eq!(endpoint.source, EndpointSource::PublicRoster);
        assert_eq!(join.lighthouse, "192.0.2.10");
    }
}

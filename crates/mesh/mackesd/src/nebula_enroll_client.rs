//! ONBOARD-3 — the peer-side fingerprint-pinned network-enroll client.
//!
//! The peer half of the Magic onboarding wire path
//! (docs/design/magic-onboarding.md). Given a join token that carries
//! the lighthouse address, port, single-use bearer, and the endpoint
//! cert fingerprint (`?fp=` — ONBOARD-1), this:
//!
//!   1. generates a fresh Ed25519 identity + builds the CSR;
//!   2. POSTs the CSR to `https://<lighthouse>:<port>/enroll` over a
//!      TLS connection **pinned to the token's fingerprint** — the
//!      [`PinnedCertVerifier`] fails closed on any mismatch, so there
//!      is no trust-on-first-use MITM window and no CA needed yet;
//!   3. receives the signed [`NebulaBundle`] and materializes
//!      `/etc/nebula` from it via the existing
//!      [`crate::workers::nebula_supervisor::materialize_config`],
//!      whose `static_host_map` is rendered from
//!      `bundle.lighthouses[].external_addr` — the lighthouse's PUBLIC
//!      address, never a local interface (closes the MESH-2 class).
//!
//! After materialize, `serve`'s nebula process dials the lighthouse's
//! public IP outbound (NAT-friendly) and the overlay forms. §1: the
//! whole exchange is Nebula control-plane — no Tailscale/Headscale/DERP.

#![cfg(feature = "async-services")]

use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::ca::bundle::NebulaBundle;
use crate::nebula_enroll::JoinToken;
use crate::nebula_enroll_endpoint::fingerprint;

/// Whole-exchange budget — TCP connect + TLS handshake + POST + read.
pub const NETWORK_ENROLL_TIMEOUT: Duration = Duration::from_secs(20);

/// XPA-11 — the enroll transport occasionally fails on the first attempt (the
/// lighthouse busy, a cold relay path), so `network_enroll` retries **transient
/// transport errors** this many times. Deterministic failures (fp mismatch,
/// HTTP refusal, bad bundle) fail fast and are NOT retried.
pub const ENROLL_ATTEMPTS: u32 = 3;
/// Backoff between enroll transport retries.
pub const ENROLL_RETRY_BACKOFF: Duration = Duration::from_secs(3);

/// Errors the peer-side network enroll can hit.
#[derive(Debug)]
pub enum NetEnrollError {
    /// The token carried no `?fp=` — the network path requires a
    /// pinned fingerprint (absence falls back to the QNM-Shared flow).
    MissingFingerprint,
    /// TLS / TCP transport failure (connect, handshake, pin mismatch).
    Transport(String),
    /// The lighthouse refused the enroll — carries the HTTP status +
    /// the server's error message.
    Refused {
        /// HTTP status code (401 bearer, 403 banned, 409 cap, …).
        status: u16,
        /// Server-supplied error string.
        message: String,
    },
    /// The 200 response body didn't parse as a NebulaBundle.
    BadBundle(String),
    /// Materializing `/etc/nebula` from the bundle failed.
    Materialize(String),
}

impl std::fmt::Display for NetEnrollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFingerprint => write!(
                f,
                "join token has no `?fp=` fingerprint — the network enroll path \
                 requires it (a co-located node can fall back to QNM-Shared)",
            ),
            Self::Transport(e) => write!(f, "enroll transport failed: {e}"),
            Self::Refused { status, message } => {
                write!(
                    f,
                    "lighthouse refused enrollment (HTTP {status}): {message}"
                )
            }
            Self::BadBundle(e) => write!(f, "lighthouse returned an unparseable bundle: {e}"),
            Self::Materialize(e) => write!(f, "writing /etc/nebula failed: {e}"),
        }
    }
}

impl std::error::Error for NetEnrollError {}

/// A rustls [`ServerCertVerifier`] that accepts ONLY the cert whose
/// SHA-256 fingerprint matches the pinned value AND whose handshake
/// signature verifies under the crypto provider — so a MITM can
/// neither swap the cert nor replay the real (public) cert without
/// holding its private key. Fail-closed: any mismatch is a hard error.
#[derive(Debug)]
pub struct PinnedCertVerifier {
    pinned: String,
    provider: Arc<CryptoProvider>,
}

impl PinnedCertVerifier {
    /// Wrap a pinned lowercase-hex SHA-256 fingerprint + the provider
    /// whose algorithms verify the handshake signature.
    #[must_use]
    pub fn new(pinned: impl Into<String>, provider: Arc<CryptoProvider>) -> Self {
        Self {
            pinned: pinned.into(),
            provider,
        }
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let observed = fingerprint(end_entity.as_ref());
        if observed == self.pinned {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "enroll-fingerprint-mismatch: expected={} observed={}",
                self.pinned, observed,
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Build a rustls [`ClientConfig`] that trusts exactly the cert whose
/// fingerprint matches `pinned_fp`. Pinning replaces chain validation
/// (the endpoint is self-signed — there is no CA at enroll time).
fn pinned_client_config(pinned_fp: &str) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedCertVerifier::new(pinned_fp, provider.clone()));
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring provider supports the safe default protocol versions")
        .dangerous() // self-signed model — fingerprint pinning replaces chain validation
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(config)
}

/// POST a CSR body to `https://<lighthouse>:<port>/enroll` over a
/// fingerprint-pinned TLS connection and return the parsed bundle.
///
/// Pure transport — the caller builds `csr_json` and decides what to
/// do with the returned bundle. Separated so tests can drive it
/// against a loopback endpoint.
///
/// # Errors
/// Per [`NetEnrollError`] (transport, pin mismatch, HTTP refusal, or
/// bad bundle).
pub async fn enroll_over_network(
    lighthouse: &str,
    port: u16,
    pinned_fp: &str,
    csr_json: &[u8],
) -> Result<NebulaBundle, NetEnrollError> {
    let fut = enroll_over_network_inner(lighthouse, port, pinned_fp, csr_json);
    match tokio::time::timeout(NETWORK_ENROLL_TIMEOUT, fut).await {
        Ok(result) => result,
        Err(_) => Err(NetEnrollError::Transport(format!(
            "timed out after {}s",
            NETWORK_ENROLL_TIMEOUT.as_secs()
        ))),
    }
}

async fn enroll_over_network_inner(
    lighthouse: &str,
    port: u16,
    pinned_fp: &str,
    csr_json: &[u8],
) -> Result<NebulaBundle, NetEnrollError> {
    let config = pinned_client_config(pinned_fp);
    let connector = tokio_rustls::TlsConnector::from(config);
    // The verifier ignores the name (it pins the fp), but rustls
    // requires a ServerName. An IP literal parses to ServerName::IpAddress;
    // a hostname to DnsName — either is fine.
    let server_name = ServerName::try_from(lighthouse.to_string())
        .map_err(|e| NetEnrollError::Transport(format!("bad server name '{lighthouse}': {e}")))?;
    let tcp = tokio::net::TcpStream::connect((lighthouse, port))
        .await
        .map_err(|e| NetEnrollError::Transport(format!("connect {lighthouse}:{port}: {e}")))?;
    let mut tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| NetEnrollError::Transport(format!("tls handshake: {e}")))?;

    let request = format!(
        "POST /enroll HTTP/1.1\r\n\
         Host: {lighthouse}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        csr_json.len(),
    );
    tls.write_all(request.as_bytes())
        .await
        .map_err(|e| NetEnrollError::Transport(format!("write head: {e}")))?;
    tls.write_all(csr_json)
        .await
        .map_err(|e| NetEnrollError::Transport(format!("write body: {e}")))?;
    tls.flush()
        .await
        .map_err(|e| NetEnrollError::Transport(format!("flush: {e}")))?;

    let mut raw = Vec::new();
    tls.read_to_end(&mut raw)
        .await
        .map_err(|e| NetEnrollError::Transport(format!("read response: {e}")))?;

    let (status, body) = parse_http_response(&raw)
        .map_err(|e| NetEnrollError::Transport(format!("parse response: {e}")))?;
    if status != 200 {
        let message = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned());
        return Err(NetEnrollError::Refused { status, message });
    }
    serde_json::from_slice::<NebulaBundle>(&body)
        .map_err(|e| NetEnrollError::BadBundle(e.to_string()))
}

/// Parse a minimal HTTP/1.1 response into (status, body). Accepts the
/// `Connection: close` framing the endpoint always uses (read to EOF).
fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header/body separator")?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| "non-utf8 headers")?;
    let status_line = head.lines().next().ok_or("empty response")?;
    // "HTTP/1.1 200 OK"
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or("no status code")?
        .parse()
        .map_err(|_| "bad status code")?;
    Ok((status, raw[split + 4..].to_vec()))
}

/// End-to-end peer-side network enroll: build the identity + CSR from
/// `token`, POST it fingerprint-pinned, and materialize `/etc/nebula`
/// from the returned bundle. Also drops the bundle at the QNM-Shared
/// `bundle_path` so the supervisor's steady-state refresh stays
/// consistent. Returns the bundle (the caller logs overlay IP, etc.).
///
/// `config_dir` is `/etc/nebula` in production; tests redirect it.
///
/// # Errors
/// Per [`NetEnrollError`]. Requires `token.fp` to be set.
pub async fn network_enroll(
    workgroup_root: &std::path::Path,
    config_dir: &std::path::Path,
    node_id: &str,
    display_name: &str,
    token: JoinToken,
) -> Result<NebulaBundle, NetEnrollError> {
    let pinned_fp = token.fp.clone().ok_or(NetEnrollError::MissingFingerprint)?;
    let lighthouse = token.lighthouse.clone();
    let port = token.port;
    let identity = crate::enrollment::build_identity();
    let pending = crate::nebula_enroll::build_pending(&identity, node_id, display_name, token);
    let csr_json =
        serde_json::to_vec(&pending).map_err(|e| NetEnrollError::Transport(e.to_string()))?;

    // XPA-11 — retry transient transport errors (timeout / connection); fail
    // fast on deterministic ones (pin mismatch, HTTP refusal, bad bundle).
    let mut last_err: Option<NetEnrollError> = None;
    let mut bundle: Option<NebulaBundle> = None;
    for attempt in 1..=ENROLL_ATTEMPTS {
        match enroll_over_network(&lighthouse, port, &pinned_fp, &csr_json).await {
            Ok(b) => {
                bundle = Some(b);
                break;
            }
            Err(NetEnrollError::Transport(e)) if attempt < ENROLL_ATTEMPTS => {
                tracing::warn!(attempt, error = %e, "enroll transport failed; retrying");
                last_err = Some(NetEnrollError::Transport(e));
                tokio::time::sleep(ENROLL_RETRY_BACKOFF).await;
            }
            Err(other) => return Err(other),
        }
    }
    let bundle = bundle.ok_or_else(|| {
        last_err.unwrap_or_else(|| NetEnrollError::Transport("enroll failed".into()))
    })?;

    persist_bundle(workgroup_root, config_dir, node_id, &bundle)?;
    Ok(bundle)
}

/// Write the received bundle to `/etc/nebula` (via the supervisor's
/// materializer — `static_host_map` from `external_addr`, MESH-2 guard)
/// and to the QNM-Shared `bundle_path` for steady-state consistency.
///
/// Public so the enrollment TUI (`mde-enroll`, ONBOARD-5) can drive the
/// stages itself — `enroll_over_network` then `persist_bundle` — and
/// report granular progress between them, sharing this crate's building
/// blocks rather than reimplementing them.
///
/// # Errors
/// [`NetEnrollError::Materialize`] on a config/bundle write failure.
pub fn persist_bundle(
    workgroup_root: &std::path::Path,
    config_dir: &std::path::Path,
    node_id: &str,
    bundle: &NebulaBundle,
) -> Result<(), NetEnrollError> {
    // /etc/nebula — the live config nebula reads on serve.
    crate::workers::nebula_supervisor::materialize_config(
        config_dir,
        bundle,
        crate::workers::nebula_supervisor::ConfigRole::Peer,
        &[],
        workgroup_root,
    )
    .map_err(NetEnrollError::Materialize)?;
    // QNM-Shared bundle_path — where the supervisor re-reads on refresh.
    let bp = crate::ca::bundle::bundle_path(workgroup_root, node_id);
    crate::ca::bundle::write_bundle(&bp, bundle)
        .map_err(|e| NetEnrollError::Materialize(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nebula_enroll_endpoint::generate_endpoint_identity;
    use rustls::pki_types::pem::PemObject;

    // ---- the pinning verifier (security-critical) -----------

    #[test]
    fn verifier_accepts_matching_fingerprint() {
        let id = generate_endpoint_identity(&["127.0.0.1".into()]).unwrap();
        let der = CertificateDer::pem_slice_iter(id.cert_pem.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let v = PinnedCertVerifier::new(id.fingerprint.clone(), provider);
        let name = ServerName::try_from("127.0.0.1").unwrap();
        let now = UnixTime::since_unix_epoch(Duration::from_secs(1_700_000_000));
        assert!(v.verify_server_cert(&der, &[], &name, &[], now).is_ok());
    }

    #[test]
    fn verifier_rejects_wrong_fingerprint_fail_closed() {
        let id = generate_endpoint_identity(&["127.0.0.1".into()]).unwrap();
        let der = CertificateDer::pem_slice_iter(id.cert_pem.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        // Pin a DIFFERENT fp than the cert carries.
        let wrong = "0".repeat(64);
        let v = PinnedCertVerifier::new(wrong, provider);
        let name = ServerName::try_from("127.0.0.1").unwrap();
        let now = UnixTime::since_unix_epoch(Duration::from_secs(1_700_000_000));
        let err = v
            .verify_server_cert(&der, &[], &name, &[], now)
            .unwrap_err();
        assert!(format!("{err}").contains("fingerprint-mismatch"));
    }

    // ---- HTTP response parsing ------------------------------

    #[test]
    fn parse_response_extracts_status_and_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"{}");
    }

    #[test]
    fn parse_response_reads_error_status() {
        let raw = b"HTTP/1.1 401 Unauthorized\r\n\r\n{\"error\":\"bad bearer\"}";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 401);
        assert!(body.starts_with(b"{\"error\""));
    }
}

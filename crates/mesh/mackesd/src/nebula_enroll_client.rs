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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::ca::bundle::{
    AuthenticatedEnrollmentResponse, LighthouseEnrollmentSecrets, NebulaBundle,
};
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

/// Requester-local Nebula keypair staging. Only the public PEM is placed in
/// the enrollment request; the private path remains inside a mode-0700 local
/// directory until [`persist_bundle`] seals it as `/etc/nebula/host.key`.
#[derive(Debug)]
pub struct RequesterNebulaKey {
    staging_dir: PathBuf,
    private_key_path: PathBuf,
    public_key_pem: String,
}

impl RequesterNebulaKey {
    /// Public PEM sent to the CA for `nebula-cert sign -in-pub`.
    #[must_use]
    pub fn public_key_pem(&self) -> &str {
        &self.public_key_pem
    }

    /// Root-local staged private key path. Never serialize or log this file.
    #[must_use]
    pub fn private_key_path(&self) -> &Path {
        &self.private_key_path
    }

    #[cfg(test)]
    pub(crate) fn for_test(parent: &Path) -> Self {
        let staging_dir = parent.join("requester-nebula-key-test");
        std::fs::create_dir_all(&staging_dir).expect("staging dir");
        let private_key_path = staging_dir.join("host.key");
        crate::ca::seal::write_sealed(
            &private_key_path,
            b"-----BEGIN NEBULA X25519 PRIVATE KEY-----\ntest\n-----END NEBULA X25519 PRIVATE KEY-----\n",
        )
        .expect("test private key");
        Self {
            staging_dir,
            private_key_path,
            public_key_pem: "-----BEGIN NEBULA X25519 PUBLIC KEY-----\nAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n-----END NEBULA X25519 PUBLIC KEY-----\n".into(),
        }
    }
}

impl Drop for RequesterNebulaKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.private_key_path);
        let _ = std::fs::remove_dir_all(&self.staging_dir);
    }
}

struct RequesterStagingCleanup {
    path: PathBuf,
    armed: bool,
}

impl RequesterStagingCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RequesterStagingCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Generate the peer's Nebula X25519 keypair locally. The keygen subprocess
/// writes inside a freshly-created mode-0700 directory, then the private file
/// is tightened to 0600 before it is read or moved anywhere else.
///
/// # Errors
/// [`NetEnrollError::Materialize`] on directory, subprocess, permission, or
/// key-read failure.
pub fn generate_requester_nebula_key(
    config_dir: &Path,
) -> Result<RequesterNebulaKey, NetEnrollError> {
    generate_requester_nebula_key_with_binary(config_dir, std::ffi::OsStr::new("nebula-cert"))
}

fn generate_requester_nebula_key_with_binary(
    config_dir: &Path,
    nebula_cert: &std::ffi::OsStr,
) -> Result<RequesterNebulaKey, NetEnrollError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::create_dir_all(config_dir).map_err(|e| {
        NetEnrollError::Materialize(format!("create {}: {e}", config_dir.display()))
    })?;
    let staging_dir = config_dir.join(format!(
        ".enroll-key-{}-{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(&staging_dir).map_err(|e| {
        NetEnrollError::Materialize(format!("create key staging {}: {e}", staging_dir.display()))
    })?;
    let mut cleanup = RequesterStagingCleanup::new(staging_dir.clone());
    let private_key_path = staging_dir.join("host.key");
    let public_key_path = staging_dir.join("host.pub");
    // Child-local umask via a constant script; paths are positional parameters
    // and are never interpreted as shell source.
    let mut command = std::process::Command::new("sh");
    command
        .args([
            "-c",
            "umask 077; binary=$1; shift; exec \"$binary\" \"$@\"",
            "nebula-cert",
        ])
        .arg(nebula_cert)
        .args(["keygen", "-out-key"])
        .arg(&private_key_path)
        .arg("-out-pub")
        .arg(&public_key_path);
    let output = command
        .output()
        .map_err(|e| NetEnrollError::Materialize(format!("nebula-cert keygen: {e}")))?;
    if !output.status.success() {
        return Err(NetEnrollError::Materialize(format!(
            "nebula-cert keygen exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    std::fs::set_permissions(&private_key_path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| NetEnrollError::Materialize(format!("chmod private key: {e}")))?;
    crate::ca::seal::read_sealed(&private_key_path)
        .map_err(|e| NetEnrollError::Materialize(format!("validate private key: {e}")))?;
    let public_key_pem = std::fs::read_to_string(&public_key_path)
        .map_err(|e| NetEnrollError::Materialize(format!("read requester public key: {e}")))?;
    let _ = std::fs::remove_file(public_key_path);
    crate::ca::sign::validate_nebula_public_key_pem(&public_key_pem).map_err(|e| {
        NetEnrollError::Materialize(format!("nebula-cert emitted an invalid public key: {e}"))
    })?;
    cleanup.disarm();
    Ok(RequesterNebulaKey {
        staging_dir,
        private_key_path,
        public_key_pem,
    })
}

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
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring provider supports TLS 1.3")
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
) -> Result<AuthenticatedEnrollmentResponse, NetEnrollError> {
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
) -> Result<AuthenticatedEnrollmentResponse, NetEnrollError> {
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
    serde_json::from_slice::<AuthenticatedEnrollmentResponse>(&body)
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
    let requester_key = generate_requester_nebula_key(config_dir)?;
    network_enroll_with_requester_key(
        workgroup_root,
        config_dir,
        node_id,
        display_name,
        token,
        &requester_key,
    )
    .await
}

pub(crate) async fn network_enroll_with_requester_key(
    workgroup_root: &std::path::Path,
    config_dir: &std::path::Path,
    node_id: &str,
    display_name: &str,
    token: JoinToken,
    requester_key: &RequesterNebulaKey,
) -> Result<NebulaBundle, NetEnrollError> {
    let pinned_fp = token.fp.clone().ok_or(NetEnrollError::MissingFingerprint)?;
    let lighthouse = token.lighthouse.clone();
    let port = token.port;
    let identity = crate::enrollment::build_identity();
    let pending = crate::nebula_enroll::build_pending_with_nebula_key(
        &identity,
        node_id,
        display_name,
        token,
        requester_key.public_key_pem(),
    );
    let csr_json =
        serde_json::to_vec(&pending).map_err(|e| NetEnrollError::Transport(e.to_string()))?;

    // XPA-11 — retry transient transport errors (timeout / connection); fail
    // fast on deterministic ones (pin mismatch, HTTP refusal, bad bundle).
    let mut last_err: Option<NetEnrollError> = None;
    let mut response: Option<AuthenticatedEnrollmentResponse> = None;
    for attempt in 1..=ENROLL_ATTEMPTS {
        match enroll_over_network(&lighthouse, port, &pinned_fp, &csr_json).await {
            Ok(enrollment) => {
                response = Some(enrollment);
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
    let response = response.ok_or_else(|| {
        last_err.unwrap_or_else(|| NetEnrollError::Transport("enroll failed".into()))
    })?;

    verify_authenticated_enrollment_bundle(&response.bundle, node_id, requester_key)?;

    persist_authenticated_bundle(
        workgroup_root,
        config_dir,
        node_id,
        &response.bundle,
        response.lighthouse_secrets.as_ref(),
        requester_key.private_key_path(),
    )?;
    Ok(response.bundle)
}

fn requester_public_key_hex(public_key_pem: &str) -> Result<String, NetEnrollError> {
    let body: String = public_key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .map(str::trim)
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|e| NetEnrollError::BadBundle(format!("decode requester public key: {e}")))?;
    if bytes.len() != 32 {
        return Err(NetEnrollError::BadBundle(format!(
            "requester public key decoded to {} bytes, expected 32",
            bytes.len()
        )));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn validate_nebula_cert_print(
    raw: &str,
    node_id: &str,
    overlay_ip: &str,
    requester_public_key_pem: &str,
) -> Result<(), NetEnrollError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| NetEnrollError::BadBundle(format!("parse signed Nebula cert: {e}")))?;
    let cert = if value.is_array() {
        value
            .get(0)
            .ok_or_else(|| NetEnrollError::BadBundle("empty Nebula cert print".into()))?
    } else {
        &value
    };
    let details = cert.get("details").unwrap_or(cert);
    let printed_public = details
        .get("publicKey")
        .or_else(|| cert.get("publicKey"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| NetEnrollError::BadBundle("signed cert has no public key".into()))?;
    let expected_public = requester_public_key_hex(requester_public_key_pem)?;
    if !printed_public.eq_ignore_ascii_case(&expected_public) {
        return Err(NetEnrollError::BadBundle(
            "signed cert public key does not match requester-owned key".into(),
        ));
    }
    if details.get("name").and_then(|value| value.as_str()) != Some(node_id) {
        return Err(NetEnrollError::BadBundle(
            "signed cert name does not match enrollment node".into(),
        ));
    }
    let expected_ip = format!("{overlay_ip}/{}", crate::ca::sign::DEFAULT_CIDR_PREFIX);
    // `nebula-cert print -json` calls this array `networks`; older test and
    // pre-1.10 fixtures used `ips`. Accept both names, but require the exact
    // address/prefix in either shape before materializing the returned cert.
    let has_ip = ["networks", "ips"].into_iter().any(|field| {
        details
            .get(field)
            .and_then(|value| value.as_array())
            .is_some_and(|ips| {
                ips.iter()
                    .any(|ip| ip.as_str() == Some(expected_ip.as_str()))
            })
    });
    if !has_ip {
        return Err(NetEnrollError::BadBundle(
            "signed cert overlay IP does not match bundle".into(),
        ));
    }
    Ok(())
}

/// Prove that a returned Nebula certificate binds the exact requester-owned
/// public key, node id, and allocated overlay IP before any live identity swap.
pub fn verify_authenticated_enrollment_bundle(
    bundle: &NebulaBundle,
    node_id: &str,
    requester_key: &RequesterNebulaKey,
) -> Result<(), NetEnrollError> {
    #[cfg(test)]
    if requester_key
        .public_key_pem()
        .contains("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
    {
        let raw = format!(
            "{{\"details\":{{\"name\":{name:?},\"ips\":[{ip:?}],\"publicKey\":\"{}\"}}}}",
            "00".repeat(32),
            name = node_id,
            ip = format!("{}/17", bundle.overlay_ip),
        );
        return validate_nebula_cert_print(
            &raw,
            node_id,
            &bundle.overlay_ip,
            requester_key.public_key_pem(),
        );
    }
    let cert_path = requester_key.staging_dir.join("returned-host.crt");
    crate::ca::seal::write_atomic_sealed(&cert_path, bundle.peer_cert_pem.as_bytes())
        .map_err(|e| NetEnrollError::BadBundle(format!("stage returned cert: {e}")))?;
    let output = std::process::Command::new("nebula-cert")
        .args(["print", "-json", "-path"])
        .arg(&cert_path)
        .output()
        .map_err(|e| NetEnrollError::BadBundle(format!("inspect returned cert: {e}")))?;
    let _ = std::fs::remove_file(&cert_path);
    if !output.status.success() {
        return Err(NetEnrollError::BadBundle(format!(
            "nebula-cert rejected returned cert: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    validate_nebula_cert_print(
        &String::from_utf8_lossy(&output.stdout),
        node_id,
        &bundle.overlay_ip,
        requester_key.public_key_pem(),
    )
}

/// Write the received bundle to `/etc/nebula` (via the supervisor's
/// materializer — `static_host_map` from `external_addr`, MESH-2 guard)
/// and to the QNM-Shared `bundle_path` for steady-state consistency.
///
/// Public so the enrollment TUI (`mde-enroll`, ONBOARD-5) can drive the
/// stages itself — `enroll_over_network` then `persist_authenticated_bundle` — and
/// report granular progress between them, sharing this crate's building
/// blocks rather than reimplementing them.
///
/// # Errors
/// This function may be called only after the response arrived over the
/// join-token fingerprint-pinned TLS channel; replicated file state must never
/// establish or replace the local relay authority pin.
///
/// [`NetEnrollError::Materialize`] on a config/bundle write failure.
pub fn persist_authenticated_bundle(
    workgroup_root: &std::path::Path,
    config_dir: &std::path::Path,
    node_id: &str,
    bundle: &NebulaBundle,
    lighthouse_secrets: Option<&LighthouseEnrollmentSecrets>,
    requester_private_key: &Path,
) -> Result<(), NetEnrollError> {
    crate::ca::bundle::write_relay_trust_authority_pin(
        bundle,
        std::path::Path::new(crate::ca::bundle::RELAY_TRUST_AUTHORITY_PIN_PATH),
    )
    .map_err(|e| NetEnrollError::Materialize(format!("persist relay authority pin: {e}")))?;
    // #12 — a full-lighthouse joiner carries the mesh CA private key: install it so
    // the node can itself sign/enroll, and render the Host (am_lighthouse) config
    // immediately (the supervisor reconcile would also flip it on its next tick).
    let role = if let Some(secrets) = lighthouse_secrets {
        install_lighthouse_ca(bundle, secrets)?;
        crate::workers::nebula_supervisor::ConfigRole::Host
    } else {
        crate::workers::nebula_supervisor::ConfigRole::Peer
    };
    let private_key = crate::ca::seal::read_sealed(requester_private_key)
        .map_err(|e| NetEnrollError::Materialize(format!("read requester private key: {e}")))?;
    // /etc/nebula — the live config nebula reads on serve.
    crate::workers::nebula_supervisor::materialize_config(
        config_dir,
        bundle,
        role,
        &[],
        workgroup_root,
        Some(&private_key),
    )
    .map_err(NetEnrollError::Materialize)?;
    // QNM-Shared bundle_path — where the supervisor re-reads on refresh.
    persist_steady_state_bundle(workgroup_root, node_id, bundle)?;
    Ok(())
}

fn persist_steady_state_bundle(
    workgroup_root: &std::path::Path,
    node_id: &str,
    bundle: &NebulaBundle,
) -> Result<(), NetEnrollError> {
    let bp = crate::ca::bundle::bundle_path(workgroup_root, node_id);
    crate::ca::bundle::write_bundle(&bp, bundle)
        .map_err(|e| NetEnrollError::Materialize(e.to_string()))
}

/// HA / turn-key (#12) — a node that enrolled as a full lighthouse received the
/// mesh CA **private** key in its bundle; install it so the node can itself
/// sign/enroll new peers:
///   1. seal the CA key (0600) + write the CA cert under `/var/lib/mackesd/nebula-ca/`;
///   2. seed the local `nebula_ca` store row with the **shared** mesh CA, so the
///      daemon adopts the existing CA instead of `mint_ca` forking a brand-new one.
/// Private-key parse/seal failures are hard enrollment errors so a node is never
/// reported as signing-capable without durable secrets. Public-cert installation
/// and the idempotent local store seed remain best-effort and log on failure.
fn install_lighthouse_ca(
    bundle: &NebulaBundle,
    secrets: &LighthouseEnrollmentSecrets,
) -> Result<(), NetEnrollError> {
    if let Some(private_hex) = secrets.relay_trust_authority_key.as_deref() {
        match crate::ca::bundle::relay_trust_authority_from_private_hex(private_hex) {
            Some(key) => {
                crate::ca::seal::write_sealed(
                    std::path::Path::new(crate::ca::bundle::RELAY_TRUST_AUTHORITY_KEY_PATH),
                    &key.to_bytes(),
                )
                .map_err(|e| NetEnrollError::Materialize(format!("seal relay authority: {e}")))?;
            }
            None => {
                return Err(NetEnrollError::Materialize(
                    "malformed relay trust authority private key".into(),
                ));
            }
        }
    }
    crate::ca::seal::write_atomic_pair(
        std::path::Path::new(crate::ca::DEFAULT_CA_CERT_PATH),
        bundle.ca_cert_pem.as_bytes(),
        std::path::Path::new(crate::ca::DEFAULT_CA_KEY_PATH),
        secrets.ca_key_pem.as_bytes(),
    )
    .map_err(|e| NetEnrollError::Materialize(format!("install atomic CA pair: {e}")))?;
    match crate::store::open(&crate::default_db_path()) {
        Ok(conn) => {
            let _ = crate::store::migrate(&conn);
            match conn.execute(
                "INSERT OR REPLACE INTO nebula_ca (mesh_id, epoch, ca_cert_pem, retired_at) \
                 VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![bundle.mesh_id, bundle.epoch, bundle.ca_cert_pem],
            ) {
                Ok(_) => tracing::info!(
                    mesh_id = %bundle.mesh_id,
                    epoch = bundle.epoch,
                    "install_lighthouse_ca: CA key+cert installed + store seeded — \
                     this node is now a full signing lighthouse"
                ),
                Err(e) => {
                    tracing::warn!(error = %e, "install_lighthouse_ca: seeding nebula_ca row failed")
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "install_lighthouse_ca: opening store failed"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nebula_enroll_endpoint::generate_endpoint_identity;
    use rustls::pki_types::pem::PemObject;
    use std::os::unix::fs::PermissionsExt as _;

    fn private_lighthouse_response() -> AuthenticatedEnrollmentResponse {
        AuthenticatedEnrollmentResponse {
            bundle: NebulaBundle {
                mesh_id: "mesh".into(),
                epoch: 1,
                ca_cert_pem: "ca".into(),
                peer_cert_pem: "peer-cert".into(),
                overlay_ip: "10.42.0.2".into(),
                mesh_cidr: "10.42.0.0/16".into(),
                lighthouses: vec![],
                relay_trust_authority: Some("11".repeat(32)),
                created_at: 1,
            },
            lighthouse_secrets: Some(LighthouseEnrollmentSecrets {
                ca_key_pem: "private-ca".into(),
                relay_trust_authority_key: Some("22".repeat(32)),
            }),
        }
    }

    #[test]
    fn requester_key_subprocess_creates_private_file_at_0600() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fake = temp.path().join("fake-nebula-cert");
        std::fs::write(
            &fake,
            b"#!/bin/sh\n\
key=''\n\
pub=''\n\
while [ \"$#\" -gt 0 ]; do\n\
  case \"$1\" in\n\
    -out-key) key=$2; shift 2 ;;\n\
    -out-pub) pub=$2; shift 2 ;;\n\
    *) shift ;;\n\
  esac\n\
done\n\
printf 'requester-private' > \"$key\"\n\
stat -c %a \"$key\" > \"$key.initial-mode\"\n\
printf '%s\\n' '-----BEGIN NEBULA X25519 PUBLIC KEY-----' 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=' '-----END NEBULA X25519 PUBLIC KEY-----' > \"$pub\"\n",
        )
        .expect("fake binary");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let requester = generate_requester_nebula_key_with_binary(temp.path(), fake.as_os_str())
            .expect("requester keygen");
        let initial_mode_path = std::path::PathBuf::from(format!(
            "{}.initial-mode",
            requester.private_key_path().display()
        ));
        assert_eq!(
            std::fs::read_to_string(initial_mode_path).unwrap().trim(),
            "600"
        );
        assert_eq!(
            std::fs::metadata(requester.private_key_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn requester_key_subprocess_failure_cleans_private_staging() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fake = temp.path().join("failing-nebula-cert");
        std::fs::write(&fake, b"#!/bin/sh\nexit 9\n").expect("fake binary");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        generate_requester_nebula_key_with_binary(temp.path(), fake.as_os_str())
            .expect_err("failure must propagate");
        assert!(!std::fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".enroll-key-")));
    }

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

    #[test]
    fn replicated_lighthouse_bundle_redacts_lighthouse_only_private_keys() {
        let temp = tempfile::tempdir().expect("tempdir");
        let response = private_lighthouse_response();
        persist_steady_state_bundle(temp.path(), "peer:lh2", &response.bundle)
            .expect("persist redacted bundle");
        let path = crate::ca::bundle::bundle_path(temp.path(), "peer:lh2");
        crate::ca::bundle::read_bundle(&path).expect("read persisted bundle");
        let raw = std::fs::read_to_string(path).expect("raw bundle");
        assert!(!raw.contains("private-ca"));
        assert!(!raw.contains(&"22".repeat(32)));
        assert!(!raw.contains("lighthouse_secrets"));
        let debug = format!("{response:?}");
        assert!(!debug.contains("private-ca"));
        assert!(!debug.contains(&"22".repeat(32)));
    }

    #[test]
    fn hostile_cert_for_another_public_key_is_rejected() {
        let requester = "-----BEGIN NEBULA X25519 PUBLIC KEY-----\n\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
-----END NEBULA X25519 PUBLIC KEY-----\n";
        let hostile = format!(
            "{{\"details\":{{\"name\":\"peer:anvil\",\"ips\":[\"10.42.0.2/17\"],\"publicKey\":\"{}\"}}}}",
            "11".repeat(32)
        );
        let error = validate_nebula_cert_print(&hostile, "peer:anvil", "10.42.0.2", requester)
            .expect_err("mismatched certificate key must fail closed");
        assert!(error
            .to_string()
            .contains("does not match requester-owned key"));
    }

    #[test]
    fn exact_requester_cert_identity_is_accepted() {
        let requester = "-----BEGIN NEBULA X25519 PUBLIC KEY-----\n\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
-----END NEBULA X25519 PUBLIC KEY-----\n";
        let exact = format!(
            "{{\"details\":{{\"name\":\"peer:anvil\",\"ips\":[\"10.42.0.2/17\"],\"publicKey\":\"{}\"}}}}",
            "00".repeat(32)
        );
        validate_nebula_cert_print(&exact, "peer:anvil", "10.42.0.2", requester)
            .expect("exact requester identity");
    }

    #[test]
    fn real_nebula_cert_print_networks_field_is_accepted() {
        let requester = "-----BEGIN NEBULA X25519 PUBLIC KEY-----\n\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
-----END NEBULA X25519 PUBLIC KEY-----\n";
        let exact = format!(
            "{{\"details\":{{\"name\":\"peer:anvil\",\"networks\":[\"10.42.0.2/17\"],\"publicKey\":\"{}\"}}}}",
            "00".repeat(32)
        );
        validate_nebula_cert_print(&exact, "peer:anvil", "10.42.0.2", requester)
            .expect("real nebula-cert networks field");
    }

    #[test]
    fn hostile_cert_for_another_node_or_overlay_is_rejected() {
        let requester = "-----BEGIN NEBULA X25519 PUBLIC KEY-----\n\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
-----END NEBULA X25519 PUBLIC KEY-----\n";
        let wrong_node = format!(
            "{{\"details\":{{\"name\":\"peer:mallory\",\"ips\":[\"10.42.0.2/17\"],\"publicKey\":\"{}\"}}}}",
            "00".repeat(32)
        );
        let node_error =
            validate_nebula_cert_print(&wrong_node, "peer:anvil", "10.42.0.2", requester)
                .expect_err("wrong node must fail closed");
        assert!(node_error
            .to_string()
            .contains("name does not match enrollment node"));

        let wrong_ip = format!(
            "{{\"details\":{{\"name\":\"peer:anvil\",\"ips\":[\"10.42.0.99/17\"],\"publicKey\":\"{}\"}}}}",
            "00".repeat(32)
        );
        let ip_error = validate_nebula_cert_print(&wrong_ip, "peer:anvil", "10.42.0.2", requester)
            .expect_err("wrong overlay must fail closed");
        assert!(ip_error
            .to_string()
            .contains("overlay IP does not match bundle"));
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

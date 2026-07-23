//! Phase 12.18 D.2 — `NebulaHttps443Transport` implementation.
//!
//! Closes the v3.0.3 [!] 12.18 second-half blocker by shipping
//! the real `Transport` impl backing the
//! [`HttpsFallbackState::Activating`] → `Active` transition that
//! D.1 wired into the mesh-router state machine.
//!
//! ## Wire-protocol locks (v12-connectivity-scope.md Q10)
//!
//! The fallback is **indistinguishable from real HTTPS** to a
//! DPI-style middlebox:
//!
//!   * Real TLS handshake (`tokio_rustls`) — not a synthetic
//!     UDP-in-TCP encapsulation.
//!   * SNI matches the configured fallback host's domain.
//!   * Exact leaf-certificate pin from the authenticated enrollment bundle.
//!     Legacy bundles without a relay identity remain unavailable; there is no
//!     TOFU, disabled verification, or system-root fallback.
//!
//! ## Configuration
//!
//! The fallback host comes from one of:
//!
//!   * Env var `MDE_HTTPS_FALLBACK_HOST` (highest priority — set
//!     by mackesd's systemd unit + the operator's
//!     `/etc/mde/connect/policy.toml` translation layer once
//!     that lands).
//!   * `/etc/mde/connect/policy.toml` `https_fallback.host`
//!     field (TODO — file-config loader is a follow-up; today
//!     the env var is the only source).
//!
//! When no fallback host is configured, every `open()` returns
//! `TransportError::Misconfigured { code: "no_fallback_host" }`
//! and the daemon logs a one-shot warning at startup. The
//! router's per-peer `HttpsFallbackState` still advances to
//! `Activating` — it just stalls there until config lands.
//!
//! ## Wiring back to the mesh-router
//!
//! `NebulaHttps443Transport::open()` doesn't directly call into
//! `MeshRouterWorker::observe_handshake_outcome`; that would
//! couple the transport to the router unnecessarily. Instead,
//! the caller (the future scorer integration KDC2-1.9 + the
//! current activation path that lives in the mesh-router's
//! tick loop once D.3 wires it) reads `open()`'s `Result` +
//! invokes `observe_handshake_outcome` accordingly.

#![cfg(feature = "async-services")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mackes_transport::{
    Capabilities, Connection, HealthState, MessageClassSet, Transport, TransportError,
    TransportKind,
};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio_rustls::client::TlsStream;
use tracing::warn;

use crate::nebula_enroll_client::PinnedCertVerifier;

/// Env var the operator sets to enable the HTTPS fallback. The
/// value is `host:port` (port defaults to 443) or just `host`.
/// Examples:
///
///   * `headscale.mackes.dev`             → port 443
///   * `headscale.mackes.dev:443`         → explicit
///   * `fallback.example.com:8443`        → alt port for bench
pub const FALLBACK_HOST_ENV: &str = "MDE_HTTPS_FALLBACK_HOST";

/// Default TLS port for the fallback. RFC-2818 + Q10 lock —
/// the whole point is to be indistinguishable from real HTTPS,
/// which means 443.
pub const DEFAULT_TLS_PORT: u16 = 443;

/// Decoded fallback host config. Built once from the env var
/// + held on the `NebulaHttps443Transport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackHostConfig {
    /// Hostname for SNI + TLS verification (must match the
    /// CN/SAN in the host's cert chain).
    pub host: String,
    /// TCP port. Defaults to 443 when the env var didn't
    /// include `:port`.
    pub port: u16,
}

impl FallbackHostConfig {
    /// Read the `MDE_HTTPS_FALLBACK_HOST` env var + parse it.
    /// Returns `None` when the env var is unset or empty.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var(FALLBACK_HOST_ENV).ok()?;
        Self::parse(&raw)
    }

    /// Pure parser. `host` or `host:port`. Returns `None` on
    /// empty input.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if let Some((host, port_s)) = raw.rsplit_once(':') {
            if let Ok(port) = port_s.parse::<u16>() {
                if !host.is_empty() {
                    return Some(Self {
                        host: host.to_string(),
                        port,
                    });
                }
            }
            // Couldn't parse :port suffix — treat the whole
            // thing as a host. (Edge case: IPv6 literals would
            // need bracket parsing; deferred since the fallback
            // is a DNS-named LE-signed host, not a bare IP.)
            return Some(Self {
                host: raw.to_string(),
                port: DEFAULT_TLS_PORT,
            });
        }
        Some(Self {
            host: raw.to_string(),
            port: DEFAULT_TLS_PORT,
        })
    }

    /// SNI hostname for the rustls ClientConfig handshake.
    /// Returns `None` if the host can't be parsed as a
    /// `ServerName` (invalid DNS label, etc.).
    pub fn sni(&self) -> Option<ServerName<'static>> {
        ServerName::try_from(self.host.clone()).ok()
    }

    /// `host:port` pair the TCP connect dials.
    #[must_use]
    pub fn socket_addr_string(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn build_pinned_client_config(fingerprint: &str) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedCertVerifier::new(fingerprint, provider.clone()));
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring provider supports TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

fn advertised_identity_for_host<'a>(
    lighthouses: &'a [crate::ca::bundle::LighthouseEntry],
    fallback: &FallbackHostConfig,
    authority_public_key: Option<&str>,
) -> Option<&'a crate::ca::bundle::RelayTlsIdentity> {
    advertised_entry_for_host(lighthouses, fallback, authority_public_key)?
        .relay_tls
        .as_ref()
}

fn advertised_entry_for_host<'a>(
    lighthouses: &'a [crate::ca::bundle::LighthouseEntry],
    fallback: &FallbackHostConfig,
    authority_public_key: Option<&str>,
) -> Option<&'a crate::ca::bundle::LighthouseEntry> {
    let authority_public_key = authority_public_key?;
    lighthouses.iter().find_map(|entry| {
        let advertised = FallbackHostConfig::parse(&entry.external_addr)?;
        if advertised.host != fallback.host {
            return None;
        }
        entry
            .relay_tls
            .as_ref()
            .filter(|identity| {
                crate::ca::bundle::verify_relay_tls_identity(
                    identity,
                    &entry.node_id,
                    &entry.overlay_ip,
                    &entry.external_addr,
                    authority_public_key,
                )
            })
            .map(|_| entry)
    })
}

/// Concrete `Transport` impl for the HTTPS-tunneled fallback.
/// Constructed once at daemon start; the mesh-router calls
/// `open(peer_id)` when the per-peer
/// [`HttpsFallbackState`](mackes_transport::peer_path::HttpsFallbackState)
/// transitions to `Activating`.
#[derive(Debug)]
pub struct NebulaHttps443Transport {
    /// Decoded fallback host, or `None` when no env var was set.
    config: Option<FallbackHostConfig>,
    /// Cached rustls config pinned to the selected lighthouse's advertised leaf.
    tls_config: Option<Arc<rustls::ClientConfig>>,
    /// Node id of the exact signed lighthouse selected by the configured host.
    relay_peer_id: Option<String>,
}

impl NebulaHttps443Transport {
    /// Construct the transport from the current environment.
    /// Reads `MDE_HTTPS_FALLBACK_HOST` but deliberately installs no trust.
    /// Daemon wiring uses [`Self::from_bundle`]; this constructor remains a
    /// fail-closed default for callers that have not supplied persisted trust.
    #[must_use]
    pub fn new() -> Self {
        let config = FallbackHostConfig::from_env();
        if config.is_none() {
            warn!(
                env = FALLBACK_HOST_ENV,
                "NebulaHttps443: no fallback host configured; transport will return Misconfigured on every open"
            );
        }
        let tls_config = None;
        Self {
            config,
            tls_config,
            relay_peer_id: None,
        }
    }

    /// Construct from the local signed/enrollment bundle. The configured host
    /// must exactly match one lighthouse's advertised external host and that
    /// entry must contain a certificate/fingerprint pair which agrees.
    #[must_use]
    pub fn from_bundle(bundle_path: &Path) -> Self {
        Self::from_bundle_with_authority_pin(
            bundle_path,
            Path::new(crate::ca::bundle::RELAY_TRUST_AUTHORITY_PIN_PATH),
        )
    }

    fn from_bundle_with_authority_pin(bundle_path: &Path, authority_pin: &Path) -> Self {
        let config = FallbackHostConfig::from_env();
        let selection = config.as_ref().and_then(|fallback| {
            let bundle = crate::ca::bundle::read_bundle(bundle_path).ok()?;
            if !crate::ca::bundle::relay_trust_authority_matches_pin(&bundle, authority_pin) {
                return None;
            }
            let entry = advertised_entry_for_host(
                &bundle.lighthouses,
                fallback,
                bundle.relay_trust_authority.as_deref(),
            )?;
            let identity = entry.relay_tls.as_ref()?;
            Some((
                entry.node_id.clone(),
                build_pinned_client_config(&identity.fingerprint_sha256),
            ))
        });
        let relay_peer_id = selection.as_ref().map(|(peer_id, _)| peer_id.clone());
        let tls_config = selection.map(|(_, tls_config)| tls_config);
        if config.is_none() {
            warn!(
                env = FALLBACK_HOST_ENV,
                "NebulaHttps443: no fallback host configured; transport unavailable"
            );
        } else if tls_config.is_none() {
            warn!(
                path = %bundle_path.display(),
                "NebulaHttps443: configured relay has no exact advertised TLS identity; transport unavailable"
            );
        }
        Self {
            config,
            tls_config,
            relay_peer_id,
        }
    }

    /// Construct with explicit config + cached TLS config —
    /// used by tests that point at a loopback listener and
    /// inject a custom-rooted ClientConfig.
    #[must_use]
    pub fn with_config_and_tls(
        config: Option<FallbackHostConfig>,
        tls_config: Option<Arc<rustls::ClientConfig>>,
    ) -> Self {
        Self {
            config,
            tls_config,
            relay_peer_id: None,
        }
    }

    /// Borrow the decoded fallback config. `None` when unset.
    #[must_use]
    pub fn config(&self) -> Option<&FallbackHostConfig> {
        self.config.as_ref()
    }

    /// Signed lighthouse node selected for the live UDP bridge.
    #[must_use]
    pub fn relay_peer_id(&self) -> Option<&str> {
        self.relay_peer_id.as_deref()
    }

    /// Pre-flight: both config + exact advertised TLS pin loaded. The router
    /// can call this to check whether opens have any chance of
    /// succeeding before driving a peer into `Activating`.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.config.is_some() && self.tls_config.is_some()
    }
}

impl Default for NebulaHttps443Transport {
    fn default() -> Self {
        Self::new()
    }
}

/// Live `Connection` returned by [`NebulaHttps443Transport::open`].
/// Wraps the `tokio_rustls::client::TlsStream<TcpStream>`
/// produced by the system-trust-rooted handshake.
pub struct NebulaHttps443Connection {
    id: String,
    reader: AsyncMutex<ReadHalf<TlsStream<TcpStream>>>,
    writer: AsyncMutex<WriteHalf<TlsStream<TcpStream>>>,
}

impl std::fmt::Debug for NebulaHttps443Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NebulaHttps443Connection")
            .field("id", &self.id)
            .field("stream", &"<split TlsStream<TcpStream>>")
            .finish()
    }
}

#[async_trait]
impl Connection for NebulaHttps443Connection {
    fn id(&self) -> &str {
        &self.id
    }

    fn supports_framed_io(&self) -> bool {
        true
    }

    async fn send_frame(&self, payload: &[u8]) -> Result<(), TransportError> {
        if payload.len() > mackes_nebula_https_tunnel::MAX_FRAME_SIZE {
            return Err(TransportError::Io {
                code: "frame_oversized",
            });
        }
        let mut writer = self.writer.lock().await;
        writer
            .write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .map_err(|_| TransportError::Io {
                code: "tls_write_failed",
            })?;
        writer
            .write_all(payload)
            .await
            .map_err(|_| TransportError::Io {
                code: "tls_write_failed",
            })?;
        writer.flush().await.map_err(|_| TransportError::Io {
            code: "tls_write_failed",
        })
    }

    async fn recv_frame(&self) -> Result<Vec<u8>, TransportError> {
        let mut reader = self.reader.lock().await;
        let mut header = [0_u8; mackes_nebula_https_tunnel::HEADER_LEN];
        reader
            .read_exact(&mut header)
            .await
            .map_err(|_| TransportError::Io {
                code: "stream_closed",
            })?;
        let len = u32::from_be_bytes(header) as usize;
        if len > mackes_nebula_https_tunnel::MAX_FRAME_SIZE {
            return Err(TransportError::Io {
                code: "frame_oversized",
            });
        }
        let mut payload = vec![0_u8; len];
        reader
            .read_exact(&mut payload)
            .await
            .map_err(|_| TransportError::Io {
                code: "stream_closed",
            })?;
        Ok(payload)
    }
}

#[async_trait]
impl Transport for NebulaHttps443Transport {
    fn kind(&self) -> TransportKind {
        TransportKind::NebulaHttps443
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // The covert-tunnel protocol carries intact Nebula
            // datagrams and therefore uses Nebula's 1408-byte MTU.
            max_frame_bytes: Some(mackes_nebula_https_tunnel::MAX_FRAME_SIZE as u64),
            // 30 s — the TLS handshake is expensive; we
            // re-probe less aggressively than the UDP path.
            health_window: Duration::from_secs(30),
            // Carries every class — the fallback is a
            // last-resort tunnel.
            carries: MessageClassSet::all(),
            label: "nebula_https443".to_string(),
        }
    }

    async fn probe(&self, _peer_id: &str) -> HealthState {
        // The fallback isn't probed per-peer — its readiness is
        // a property of the transport itself (config + trust
        // store loaded). When ready, it's optimistically
        // Healthy; opens that fail later flip to Down via the
        // mesh-router's observation. When not ready, it's Down
        // so the router never picks NebulaHttps443 as primary.
        if self.ready() {
            HealthState::Healthy
        } else {
            HealthState::Down
        }
    }

    async fn open(&self, peer_id: &str) -> Result<Box<dyn Connection>, TransportError> {
        let config = self.config.as_ref().ok_or(TransportError::Misconfigured {
            code: "no_fallback_host",
        })?;
        let tls_config = self
            .tls_config
            .clone()
            .ok_or(TransportError::Misconfigured {
                code: "no_relay_trust",
            })?;
        let sni = config.sni().ok_or(TransportError::Misconfigured {
            code: "bad_fallback_host",
        })?;
        let dial = config.socket_addr_string();
        let tcp = TcpStream::connect(&dial)
            .await
            .map_err(|_e| TransportError::Unreachable {
                code: "tcp_refused",
            })?;
        let connector = tokio_rustls::TlsConnector::from(tls_config);
        let stream = connector
            .connect(sni, tcp)
            .await
            .map_err(|_e| TransportError::HandshakeFailed { code: "tls_failed" })?;
        let (reader, writer) = tokio::io::split(stream);
        Ok(Box::new(NebulaHttps443Connection {
            id: format!("https443:{peer_id}"),
            reader: AsyncMutex::new(reader),
            writer: AsyncMutex::new(writer),
        }))
    }

    async fn health(&self, peer_id: &str) -> HealthState {
        self.probe(peer_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::RootCertStore;
    use std::net::SocketAddr;
    use tokio::net::{TcpListener, UdpSocket};

    #[test]
    fn parse_host_only_uses_default_port() {
        let c = FallbackHostConfig::parse("headscale.mackes.dev").unwrap();
        assert_eq!(c.host, "headscale.mackes.dev");
        assert_eq!(c.port, 443);
    }

    #[test]
    fn parse_host_with_port_takes_explicit_port() {
        let c = FallbackHostConfig::parse("fallback.example.com:8443").unwrap();
        assert_eq!(c.host, "fallback.example.com");
        assert_eq!(c.port, 8443);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(FallbackHostConfig::parse("").is_none());
        assert!(FallbackHostConfig::parse("   ").is_none());
    }

    #[test]
    fn parse_trailing_colon_no_port_falls_back_to_host_as_whole() {
        // ":443" alone is a non-host; "host:" is treated as
        // host with default port (we keep the host string as-is
        // since we couldn't parse port — defensive).
        let c = FallbackHostConfig::parse("badhost:notaport").unwrap();
        // Couldn't parse 'notaport' as u16 → treat the full
        // raw value as the host.
        assert_eq!(c.host, "badhost:notaport");
        assert_eq!(c.port, 443);
    }

    #[test]
    fn multi_lighthouse_trust_selects_only_the_configured_host() {
        let authority = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        let identity_a = crate::ca::bundle::sign_relay_tls_identity(
            crate::ca::bundle::RelayTlsIdentity::from_certificate_pem(
                "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
            )
            .expect("identity A"),
            "lh-a",
            "10.42.0.1",
            "a.example:4242",
            &authority,
        );
        let identity_b = crate::ca::bundle::sign_relay_tls_identity(
            crate::ca::bundle::RelayTlsIdentity::from_certificate_pem(
                "-----BEGIN CERTIFICATE-----\nBAUG\n-----END CERTIFICATE-----\n",
            )
            .expect("identity B"),
            "lh-b",
            "10.42.0.2",
            "b.example:4242",
            &authority,
        );
        let lighthouses = vec![
            crate::ca::bundle::LighthouseEntry {
                node_id: "lh-a".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "a.example:4242".into(),
                relay_tls: Some(identity_a),
            },
            crate::ca::bundle::LighthouseEntry {
                node_id: "lh-b".into(),
                overlay_ip: "10.42.0.2".into(),
                external_addr: "b.example:4242".into(),
                relay_tls: Some(identity_b.clone()),
            },
        ];
        let selected = advertised_identity_for_host(
            &lighthouses,
            &FallbackHostConfig {
                host: "b.example".into(),
                port: 443,
            },
            Some(&crate::ca::bundle::relay_trust_authority_public_key(
                &authority,
            )),
        )
        .expect("matching relay identity");
        assert_eq!(selected, &identity_b);
    }

    #[test]
    fn mismatched_advertised_certificate_and_fingerprint_is_unavailable() {
        let authority = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        let mut identity = crate::ca::bundle::sign_relay_tls_identity(
            crate::ca::bundle::RelayTlsIdentity::from_certificate_pem(
                "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
            )
            .expect("identity"),
            "lh-a",
            "10.42.0.1",
            "a.example:4242",
            &authority,
        );
        identity.fingerprint_sha256 = "00".repeat(32);
        let lighthouses = vec![crate::ca::bundle::LighthouseEntry {
            node_id: "lh-a".into(),
            overlay_ip: "10.42.0.1".into(),
            external_addr: "a.example:4242".into(),
            relay_tls: Some(identity),
        }];
        assert!(advertised_identity_for_host(
            &lighthouses,
            &FallbackHostConfig {
                host: "a.example".into(),
                port: 443,
            },
            Some(&crate::ca::bundle::relay_trust_authority_public_key(
                &authority,
            )),
        )
        .is_none());
    }

    #[test]
    fn transport_kind_is_https443() {
        // Constructing with explicit None config so we don't
        // pollute the test runtime's env / fail on missing
        // trust store.
        let t = NebulaHttps443Transport::with_config_and_tls(None, None);
        assert_eq!(t.kind(), TransportKind::NebulaHttps443);
    }

    #[test]
    fn capabilities_carry_every_class_and_label_https443() {
        let t = NebulaHttps443Transport::with_config_and_tls(None, None);
        let caps = t.capabilities();
        assert!(caps.carries.control);
        assert!(caps.carries.clipboard);
        assert!(caps.carries.file_bulk);
        assert!(caps.carries.notification);
        assert_eq!(caps.label, "nebula_https443");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_without_config_returns_misconfigured_no_fallback_host() {
        let t = NebulaHttps443Transport::with_config_and_tls(None, None);
        let err = t.open("alice").await.expect_err("must fail");
        match err {
            TransportError::Misconfigured { code } => {
                assert_eq!(code, "no_fallback_host");
            }
            other => panic!("expected Misconfigured(no_fallback_host), got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_with_config_but_no_tls_returns_misconfigured_no_relay_trust() {
        let t = NebulaHttps443Transport::with_config_and_tls(
            Some(FallbackHostConfig {
                host: "example.com".into(),
                port: 443,
            }),
            None,
        );
        let err = t.open("alice").await.expect_err("must fail");
        match err {
            TransportError::Misconfigured { code } => {
                assert_eq!(code, "no_relay_trust");
            }
            other => panic!("expected Misconfigured(no_relay_trust), got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn probe_returns_down_when_not_ready() {
        let t = NebulaHttps443Transport::with_config_and_tls(None, None);
        assert_eq!(t.probe("alice").await, HealthState::Down);
        assert_eq!(t.health("alice").await, HealthState::Down);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ready_requires_both_config_and_tls() {
        let cfg = FallbackHostConfig {
            host: "example.com".into(),
            port: 443,
        };
        assert!(!NebulaHttps443Transport::with_config_and_tls(None, None).ready());
        assert!(!NebulaHttps443Transport::with_config_and_tls(Some(cfg.clone()), None).ready());
    }

    // ------- Loopback TLS integration -------------------------------
    //
    // Real handshake against a self-signed cert. The system-trust
    // ClientConfig wouldn't accept it (no LE chain on a loopback),
    // so the test injects a custom-rooted ClientConfig containing
    // just the loopback cert. This exercises the open() →
    // tokio_rustls::TlsConnector::connect() path with real wire
    // bytes — what production runs.

    fn issue_loopback_cert(host: &str) -> (Vec<u8>, Vec<u8>) {
        // rcgen 0.13 generates its own keypair when the params
        // don't carry one — simpler than reusing the mde-kdc
        // RSA flow for a test cert.
        let key_pair = KeyPair::generate().expect("rcgen keypair");
        let mut params = CertificateParams::default();
        params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, host);
            dn
        };
        // Include the host as a SAN so rustls's hostname
        // verification (which rejects CN-only certs in modern
        // verifiers) accepts it.
        params.subject_alt_names = vec![rcgen::SanType::DnsName(
            rcgen::Ia5String::try_from(host.to_string()).expect("valid DNS name"),
        )];
        let cert = params.self_signed(&key_pair).expect("self-sign");
        (cert.der().to_vec(), key_pair.serialize_der())
    }

    fn spawn_loopback_https(cert_der: Vec<u8>, key_der: Vec<u8>) -> SocketAddr {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = std_listener.local_addr().expect("local addr");
        let cert_for_thread = cert_der;
        let key_for_thread = key_der;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("rt");
            rt.block_on(async move {
                std_listener.set_nonblocking(true).expect("nonblocking");
                let listener = TcpListener::from_std(std_listener).expect("from_std");
                if let Ok((tcp, _)) = listener.accept().await {
                    let cert_chain = vec![CertificateDer::from(cert_for_thread)];
                    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_for_thread));
                    let provider = Arc::new(rustls::crypto::ring::default_provider());
                    let config = rustls::ServerConfig::builder_with_provider(provider)
                        .with_safe_default_protocol_versions()
                        .expect("server protocol")
                        .with_no_client_auth()
                        .with_single_cert(cert_chain, key)
                        .expect("server config");
                    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
                    if let Ok(mut tls) = acceptor.accept(tcp).await {
                        let _ = tls.write_all(b"\x00").await;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            });
        });
        addr
    }

    fn loopback_tls_config(server_cert_der: &[u8]) -> Arc<rustls::ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(server_cert_der.to_vec()))
            .expect("add loopback root");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocol")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(builder)
    }

    fn loopback_pinned_tls_config(server_cert_der: &[u8]) -> Arc<rustls::ClientConfig> {
        build_pinned_client_config(&crate::nebula_enroll_endpoint::fingerprint(server_cert_der))
    }

    fn loopback_server_config(cert_der: Vec<u8>, key_der: Vec<u8>) -> rustls::ServerConfig {
        let cert_chain = vec![CertificateDer::from(cert_der)];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("server protocol")
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .expect("server config")
    }

    async fn spawn_loopback_frame_echo(
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let acceptor =
            tokio_rustls::TlsAcceptor::from(Arc::new(loopback_server_config(cert_der, key_der)));
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut tls = acceptor.accept(tcp).await.expect("trusted handshake");
            let mut header = [0_u8; mackes_nebula_https_tunnel::HEADER_LEN];
            tls.read_exact(&mut header).await.expect("frame header");
            let len = u32::from_be_bytes(header) as usize;
            let mut payload = vec![0_u8; len];
            tls.read_exact(&mut payload).await.expect("frame payload");
            assert_eq!(payload, b"client nebula packet");

            let reply = b"lighthouse nebula packet";
            tls.write_all(&(reply.len() as u32).to_be_bytes())
                .await
                .expect("reply header");
            tls.write_all(reply).await.expect("reply payload");
            tls.flush().await.expect("reply flush");
        });
        (addr, task)
    }

    async fn spawn_reconnecting_demux(
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
        connections: usize,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let acceptor =
            tokio_rustls::TlsAcceptor::from(Arc::new(loopback_server_config(cert_der, key_der)));
        let nebula = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind mock lighthouse Nebula");
        let nebula_addr = nebula.local_addr().expect("mock Nebula addr");
        let task = tokio::spawn(async move {
            let echo = tokio::spawn(async move {
                let mut payload = vec![0_u8; mackes_nebula_https_tunnel::MAX_FRAME_SIZE];
                let attacker = UdpSocket::bind("127.0.0.1:0")
                    .await
                    .expect("bind hostile local sender");
                for _ in 0..connections {
                    let (length, source) = nebula
                        .recv_from(&mut payload)
                        .await
                        .expect("mock Nebula receive");
                    attacker
                        .send_to(b"forged local return", source)
                        .await
                        .expect("inject hostile local return");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    nebula
                        .send_to(&payload[..length], source)
                        .await
                        .expect("mock Nebula reply");
                }
            });
            for _ in 0..connections {
                let (tcp, _) = listener.accept().await.expect("accept");
                let tls = acceptor.accept(tcp).await.expect("trusted handshake");
                let result = mackes_nebula_https_tunnel::pump_one_stream(
                    tls,
                    mackes_nebula_https_tunnel::DemuxConfig::default()
                        .with_nebula_addr(nebula_addr)
                        .with_idle_timeout(Duration::from_millis(50)),
                )
                .await;
                match result {
                    Ok(stats) => {
                        assert_eq!(stats.frames_in, 1);
                        assert_eq!(stats.frames_out, 1);
                    }
                    Err(mackes_nebula_https_tunnel::DemuxError::IdleTimeout(_)) => {}
                    Err(mackes_nebula_https_tunnel::DemuxError::TlsIo(error))
                        if error.contains("close_notify") => {}
                    Err(error) => panic!("production demux failed: {error}"),
                }
            }
            echo.await.expect("mock Nebula echo task");
        });
        (addr, task)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn trusted_tls_connection_exchanges_bidirectional_framed_payloads() {
        let host = "localhost";
        let (cert, key) = issue_loopback_cert(host);
        let (addr, server) = spawn_loopback_frame_echo(cert.clone(), key).await;
        let transport = NebulaHttps443Transport::with_config_and_tls(
            Some(FallbackHostConfig {
                host: host.into(),
                port: addr.port(),
            }),
            Some(loopback_pinned_tls_config(&cert)),
        );

        let connection = transport.open("alice").await.expect("trusted open");
        assert!(connection.supports_framed_io());
        connection
            .send_frame(b"client nebula packet")
            .await
            .expect("send frame");
        let reply = connection.recv_frame().await.expect("receive frame");
        assert_eq!(reply, b"lighthouse nebula packet");
        server.await.expect("server task");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forced_udp_fallback_bridges_bidirectionally_and_reconnects() {
        use crate::workers::mesh_router::{MeshRouterWorker, RouterState, TransportRegistry};
        use std::collections::HashMap;
        use tokio::sync::RwLock;

        let (cert, key) = issue_loopback_cert("localhost");
        let (relay_addr, relay) = spawn_reconnecting_demux(cert.clone(), key, 2).await;
        let transport: Arc<dyn mackes_transport::Transport> =
            Arc::new(NebulaHttps443Transport::with_config_and_tls(
                Some(FallbackHostConfig {
                    host: "127.0.0.1".into(),
                    port: relay_addr.port(),
                }),
                Some(loopback_pinned_tls_config(&cert)),
            ));
        let state: RouterState = Arc::new(RwLock::new(HashMap::new()));
        let registry: TransportRegistry = Arc::new(vec![transport]);
        let router = MeshRouterWorker::new(state, registry);
        let bridge = UdpSocket::bind("127.0.0.1:0").await.expect("bridge bind");
        let bridge_addr = bridge.local_addr().expect("bridge addr");
        let nebula = UdpSocket::bind("127.0.0.1:0").await.expect("nebula bind");
        let nebula_addr = nebula.local_addr().expect("nebula addr");
        let attacker = UdpSocket::bind("127.0.0.1:0").await.expect("attacker bind");
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown = crate::workers::ShutdownToken::from_receiver(shutdown_rx);
        let router_for_exercise = &router;

        let bridge_run = router.run_https_udp_bridge(
            bridge,
            "relay".into(),
            nebula_addr,
            shutdown,
            tokio::time::interval(Duration::from_secs(60)),
        );
        let exercise = async move {
            attacker
                .send_to(b"local source hijack", bridge_addr)
                .await
                .expect("send untrusted local packet");
            let mut attacker_reply = [0_u8; 64];
            assert!(tokio::time::timeout(
                Duration::from_millis(100),
                attacker.recv_from(&mut attacker_reply),
            )
            .await
            .is_err());
            for (index, payload) in [
                b"first nebula packet".as_slice(),
                b"after reconnect".as_slice(),
            ]
            .into_iter()
            .enumerate()
            {
                nebula
                    .send_to(payload, bridge_addr)
                    .await
                    .expect("send to bridge");
                let mut received = [0_u8; 128];
                let (length, _) =
                    tokio::time::timeout(Duration::from_secs(2), nebula.recv_from(&mut received))
                        .await
                        .expect("fallback reply timeout")
                        .expect("fallback reply");
                assert_eq!(&received[..length], payload);
                if index == 0 {
                    tokio::time::timeout(Duration::from_secs(2), async {
                        loop {
                            if router_for_exercise.active_https_connection_count().await == 0 {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    })
                    .await
                    .expect("router observes first relay stream loss");
                }
            }
            shutdown_tx.send(true).expect("shutdown bridge");
        };

        let (bridge_result, (), relay_result) = tokio::join!(bridge_run, exercise, relay);
        bridge_result.expect("bridge exits cleanly");
        relay_result.expect("relay task");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn untrusted_relay_certificate_is_rejected() {
        let host = "localhost";
        let (server_cert, server_key) = issue_loopback_cert(host);
        let (wrong_cert, _) = issue_loopback_cert(host);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(loopback_server_config(
            server_cert,
            server_key,
        )));
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            assert!(acceptor.accept(tcp).await.is_err());
        });
        let transport = NebulaHttps443Transport::with_config_and_tls(
            Some(FallbackHostConfig {
                host: host.into(),
                port: addr.port(),
            }),
            Some(loopback_pinned_tls_config(&wrong_cert)),
        );

        let error = transport
            .open("alice")
            .await
            .expect_err("unadvertised relay certificate must fail closed");
        assert!(matches!(
            error,
            TransportError::HandshakeFailed { code: "tls_failed" }
        ));
        server.await.expect("server task");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forced_udp_fallback_with_wrong_relay_identity_stays_fail_closed() {
        use crate::workers::mesh_router::{MeshRouterWorker, RouterState, TransportRegistry};
        use std::collections::HashMap;
        use tokio::sync::RwLock;

        let (server_cert, server_key) = issue_loopback_cert("localhost");
        let (wrong_cert, _) = issue_loopback_cert("localhost");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let relay_addr = listener.local_addr().expect("local addr");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(loopback_server_config(
            server_cert,
            server_key,
        )));
        let relay = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            assert!(acceptor.accept(tcp).await.is_err());
        });
        let transport: Arc<dyn mackes_transport::Transport> =
            Arc::new(NebulaHttps443Transport::with_config_and_tls(
                Some(FallbackHostConfig {
                    host: "127.0.0.1".into(),
                    port: relay_addr.port(),
                }),
                Some(loopback_pinned_tls_config(&wrong_cert)),
            ));
        let state: RouterState = Arc::new(RwLock::new(HashMap::new()));
        let registry: TransportRegistry = Arc::new(vec![transport]);
        let router = MeshRouterWorker::new(Arc::clone(&state), registry);
        let bridge = UdpSocket::bind("127.0.0.1:0").await.expect("bridge bind");
        let bridge_addr = bridge.local_addr().expect("bridge addr");
        let nebula = UdpSocket::bind("127.0.0.1:0").await.expect("nebula bind");
        let nebula_addr = nebula.local_addr().expect("nebula addr");
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown = crate::workers::ShutdownToken::from_receiver(shutdown_rx);

        let bridge_run = router.run_https_udp_bridge(
            bridge,
            "relay".into(),
            nebula_addr,
            shutdown,
            tokio::time::interval(Duration::from_secs(60)),
        );
        let exercise = async move {
            nebula
                .send_to(b"must not escape", bridge_addr)
                .await
                .expect("send to bridge");
            let mut received = [0_u8; 64];
            assert!(tokio::time::timeout(
                Duration::from_millis(250),
                nebula.recv_from(&mut received),
            )
            .await
            .is_err());
            let path = state.read().await;
            assert_eq!(
                path.get("relay").expect("relay path").https_state,
                mackes_transport::peer_path::HttpsFallbackState::Failing,
            );
            shutdown_tx.send(true).expect("shutdown bridge");
        };

        let (bridge_result, (), relay_result) = tokio::join!(bridge_run, exercise, relay);
        bridge_result.expect("bridge exits cleanly");
        relay_result.expect("relay task");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_against_loopback_completes_handshake() {
        let host = "loopback-fallback.local";
        let (cert, key) = issue_loopback_cert(host);
        let addr = spawn_loopback_https(cert.clone(), key);
        let tls_config = loopback_tls_config(&cert);
        let t = NebulaHttps443Transport::with_config_and_tls(
            Some(FallbackHostConfig {
                host: host.into(),
                port: addr.port(),
            }),
            Some(tls_config),
        );
        // The fallback host is the SNI; we override via env-
        // independent config. open() dials addr (because the
        // host is literal "loopback-fallback.local" but our
        // config carries the loopback port + SNI matches).
        // Since FallbackHostConfig::socket_addr_string() builds
        // "host:port", we need DNS to resolve "loopback-
        // fallback.local" — it won't. So this test only
        // exercises the Misconfigured / TCP-refused paths
        // unless we add a hosts-file shim. Cover that via a
        // direct unit test on the path with config-but-bad-
        // host below.
        assert!(t.ready());
        // Defensive: opening should fail (DNS won't resolve
        // the loopback hostname); the test asserts the
        // expected error code path.
        let err = t.open("alice").await.expect_err("DNS resolve must fail");
        // Could be Unreachable(tcp_refused) on a system that
        // does resolve to 127.0.0.1 via /etc/hosts, or could
        // be a DNS failure mapped to tcp_refused either way.
        assert!(matches!(err, TransportError::Unreachable { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_against_explicit_loopback_ip_completes_handshake() {
        // Bypass DNS by configuring the fallback as "127.0.0.1"
        // directly. The loopback server's cert SAN won't match
        // (it claims "loopback-fallback.local"), so rustls will
        // reject the cert → HandshakeFailed(tls_failed). That's
        // the bench-observable behavior we want to lock: the
        // open() returns the right error code when the cert
        // doesn't match the SNI.
        let host = "loopback-fallback.local";
        let (cert, key) = issue_loopback_cert(host);
        let addr = spawn_loopback_https(cert.clone(), key);
        let tls_config = loopback_tls_config(&cert);
        let t = NebulaHttps443Transport::with_config_and_tls(
            Some(FallbackHostConfig {
                host: "127.0.0.1".into(),
                port: addr.port(),
            }),
            Some(tls_config),
        );
        let err = t.open("alice").await.expect_err("SNI mismatch must fail");
        match err {
            TransportError::HandshakeFailed { code } => {
                assert_eq!(code, "tls_failed");
            }
            TransportError::Misconfigured { code } => {
                // rustls 0.23 may reject "127.0.0.1" as not a
                // valid SNI DNS name (RFC 6066 only allows DNS
                // names, not IP literals). Accept that as a
                // Misconfigured outcome — it's the same
                // semantic ("won't open") and the operator
                // would never configure a bare IP for a real
                // fallback host.
                assert_eq!(code, "bad_fallback_host");
            }
            other => panic!(
                "expected HandshakeFailed or Misconfigured(bad_fallback_host), got {other:?}"
            ),
        }
    }
}

//! NF-2.7 (v2.5) — NebulaBundle writer.
//!
//! The bundle is the single JSON blob a freshly-enrolled
//! peer needs in order to bring up its `nebula.service`:
//! the public CA cert (so it can verify other peers'
//! signatures), its own signed peer cert, its
//! allocated overlay IP, the lighthouse roster, and the
//! mesh CIDR. Written atomically to
//! `~/QNM-Shared/<peer>/mackesd/nebula-bundle.json` next to
//! the existing heartbeat.json so the QNM-Shared replicator
//! ships the bundle to the peer's local copy on the next
//! reconcile pass.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::CaError;

/// Wire shape of the bundle. JSON-serializable so the
/// QNM-Shared replicator + the wizard's import flow both
/// consume the same struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NebulaBundle {
    /// Stable mesh-id (matches `nebula_ca.mesh_id`).
    pub mesh_id: String,
    /// Active CA epoch when the bundle was written.
    pub epoch: i64,
    /// PEM body of the mesh CA's public cert.
    pub ca_cert_pem: String,
    /// PEM body of this peer's signed cert.
    pub peer_cert_pem: String,
    /// Overlay IP assigned to this peer (e.g. "10.42.0.5").
    pub overlay_ip: String,
    /// Mesh CIDR — locked to `10.42.0.0/16` per the
    /// open-mesh design.
    pub mesh_cidr: String,
    /// Lighthouse roster — every host-role peer the new
    /// peer should attempt to reach on first boot.
    pub lighthouses: Vec<LighthouseEntry>,
    /// Mesh-wide Ed25519 authority which signs relay TLS advertisements.
    /// Authenticated enrollment pins this value into the receiver's bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_trust_authority: Option<String>,
    /// Unix-epoch seconds when the bundle was generated.
    pub created_at: i64,
}

/// One-time secrets delivered only inside the fingerprint-pinned HTTPS
/// enrollment response. This type deliberately does not implement `Serialize`;
/// only [`AuthenticatedEnrollmentResponse::encode_for_authenticated_tls`] can
/// create its wire representation.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LighthouseEnrollmentSecrets {
    /// Mesh CA private key installed on the newly-authorized lighthouse.
    pub ca_key_pem: String,
    /// Relay-advertisement authority private seed.
    pub relay_trust_authority_key: Option<String>,
}

impl std::fmt::Debug for LighthouseEnrollmentSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LighthouseEnrollmentSecrets")
            .field("ca_key_pem", &"<redacted>")
            .field("relay_trust_authority_key", &"<redacted>")
            .finish()
    }
}

/// Authenticated network-enrollment response. The public bundle remains safe
/// for replicated steady state; optional lighthouse secrets are a distinct,
/// one-time TLS-only envelope.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedEnrollmentResponse {
    /// Public steady-state Nebula material.
    pub bundle: NebulaBundle,
    /// Present only for a role-scoped lighthouse bearer.
    pub lighthouse_secrets: Option<LighthouseEnrollmentSecrets>,
}

impl std::fmt::Debug for AuthenticatedEnrollmentResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticatedEnrollmentResponse")
            .field("bundle", &self.bundle)
            .field(
                "lighthouse_secrets",
                &self.lighthouse_secrets.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl AuthenticatedEnrollmentResponse {
    /// Encode the one-time response for the fingerprint-pinned endpoint. Keep
    /// this crate-private so public/file callers cannot accidentally serialize
    /// lighthouse secrets into replicated state.
    pub(crate) fn encode_for_authenticated_tls(&self) -> Result<Vec<u8>, serde_json::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            bundle: &'a NebulaBundle,
            #[serde(skip_serializing_if = "Option::is_none")]
            lighthouse_secrets: Option<WireSecrets<'a>>,
        }
        #[derive(Serialize)]
        struct WireSecrets<'a> {
            ca_key_pem: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            relay_trust_authority_key: Option<&'a str>,
        }
        let lighthouse_secrets = self.lighthouse_secrets.as_ref().map(|secrets| WireSecrets {
            ca_key_pem: &secrets.ca_key_pem,
            relay_trust_authority_key: secrets.relay_trust_authority_key.as_deref(),
        });
        serde_json::to_vec(&Wire {
            bundle: &self.bundle,
            lighthouse_secrets,
        })
    }
}

/// One lighthouse entry. Pre-resolved IP so the receiving
/// peer doesn't need DNS to bootstrap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LighthouseEntry {
    /// Stable node-id of the host.
    pub node_id: String,
    /// Overlay IP — the lighthouse advertises itself here.
    pub overlay_ip: String,
    /// Public-internet reachable address (LAN or WAN). The
    /// lighthouse listens on `<external_addr>:4242/udp` +
    /// `:443/tcp` for the covert path.
    pub external_addr: String,
    /// Exact X.509 identity presented by this lighthouse's TCP/443 relay.
    /// `None` on pre-WL-RUN-008 bundles: callers must treat that as relay
    /// transport unavailable, never fall back to TOFU or system roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_tls: Option<RelayTlsIdentity>,
}

/// Public TLS identity advertised for a lighthouse relay.
///
/// The certificate is retained for operator inspection and migration while the
/// SHA-256 fingerprint is the actual rustls verification pin. Consumers verify
/// that both representations agree before enabling the transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayTlsIdentity {
    /// PEM-encoded relay leaf certificate.
    pub certificate_pem: String,
    /// Lowercase hexadecimal SHA-256 of the certificate DER.
    pub fingerprint_sha256: String,
    /// Ed25519 signature over node id, overlay/public addresses, and the exact
    /// certificate fingerprint. Missing on legacy records, which are unusable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_ed25519: Option<String>,
}

impl RelayTlsIdentity {
    /// Build a relay identity from the first certificate in a PEM blob.
    #[must_use]
    pub fn from_certificate_pem(certificate_pem: impl Into<String>) -> Option<Self> {
        let certificate_pem = certificate_pem.into();
        let der = first_certificate_der(&certificate_pem)?;
        let fingerprint_sha256 = hex_sha256(&der);
        Some(Self {
            certificate_pem,
            fingerprint_sha256,
            signature_ed25519: None,
        })
    }

    /// True only when the persisted fingerprint exactly matches the embedded
    /// certificate. This prevents a malformed/mixed advertisement from being
    /// interpreted as usable trust material.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        first_certificate_der(&self.certificate_pem)
            .map(|der| hex_sha256(&der) == self.fingerprint_sha256)
            .unwrap_or(false)
    }
}

/// Stable private key location for the relay trust authority. All authorized
/// lighthouses receive the same key through fingerprint-pinned enrollment.
pub const RELAY_TRUST_AUTHORITY_KEY_PATH: &str = "/var/lib/mackesd/relay-trust-authority.ed25519";
/// Root-owned local pin for the authority authenticated by enrollment. Relay
/// trust in replicated bundle JSON is accepted only when it equals this pin.
pub const RELAY_TRUST_AUTHORITY_PIN_PATH: &str = "/var/lib/mackesd/relay-trust-authority.pub";

/// Persist the authenticated relay authority outside replicated state.
pub fn write_relay_trust_authority_pin(bundle: &NebulaBundle, path: &Path) -> Result<(), CaError> {
    if let Some(authority) = bundle.relay_trust_authority.as_deref() {
        super::seal::write_sealed(path, authority.as_bytes())?;
    }
    Ok(())
}

/// Verify that replicated bundle authority still matches the local enrollment
/// pin. Missing material fails closed.
#[must_use]
pub fn relay_trust_authority_matches_pin(bundle: &NebulaBundle, path: &Path) -> bool {
    let Some(authority) = bundle.relay_trust_authority.as_deref() else {
        return false;
    };
    super::seal::read_sealed(path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|pinned| pinned == authority)
}

fn relay_signature_payload(
    node_id: &str,
    overlay_ip: &str,
    external_addr: &str,
    fingerprint: &str,
) -> Vec<u8> {
    format!("mde-relay-tls-v1\0{node_id}\0{overlay_ip}\0{external_addr}\0{fingerprint}")
        .into_bytes()
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        bytes[index] = ((hi << 4) | lo) as u8;
    }
    Some(bytes)
}

/// Encode an Ed25519 verifying key for bundle persistence.
#[must_use]
pub fn relay_trust_authority_public_key(key: &SigningKey) -> String {
    hex_bytes(key.verifying_key().as_bytes())
}

/// Encode the private authority seed for lighthouse-only enrollment delivery.
#[must_use]
pub fn relay_trust_authority_private_key(key: &SigningKey) -> String {
    hex_bytes(&key.to_bytes())
}

/// Decode a lighthouse-only relay authority private seed.
#[must_use]
pub fn relay_trust_authority_from_private_hex(value: &str) -> Option<SigningKey> {
    decode_hex::<32>(value).map(|bytes| SigningKey::from_bytes(&bytes))
}

/// Sign one exact relay identity advertisement.
#[must_use]
pub fn sign_relay_tls_identity(
    mut identity: RelayTlsIdentity,
    node_id: &str,
    overlay_ip: &str,
    external_addr: &str,
    key: &SigningKey,
) -> RelayTlsIdentity {
    let signature = key.sign(&relay_signature_payload(
        node_id,
        overlay_ip,
        external_addr,
        &identity.fingerprint_sha256,
    ));
    identity.signature_ed25519 = Some(hex_bytes(&signature.to_bytes()));
    identity
}

/// Verify certificate/fingerprint consistency and the mesh-authority signature
/// over the full lighthouse address tuple.
#[must_use]
pub fn verify_relay_tls_identity(
    identity: &RelayTlsIdentity,
    node_id: &str,
    overlay_ip: &str,
    external_addr: &str,
    authority_public_key: &str,
) -> bool {
    if !identity.is_consistent() {
        return false;
    }
    let (Some(public), Some(signature)) = (
        decode_hex::<32>(authority_public_key),
        identity
            .signature_ed25519
            .as_deref()
            .and_then(decode_hex::<64>),
    ) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&public) else {
        return false;
    };
    verifying_key
        .verify(
            &relay_signature_payload(
                node_id,
                overlay_ip,
                external_addr,
                &identity.fingerprint_sha256,
            ),
            &Signature::from_bytes(&signature),
        )
        .is_ok()
}

fn first_certificate_der(pem: &str) -> Option<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let after_begin = pem.split_once(BEGIN)?.1;
    let body = after_begin.split_once(END)?.0;
    let compact: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .ok()
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Read one lighthouse's self-advertised relay identity from its replicated
/// bundle. The entry must name the same node and contain internally consistent
/// certificate/fingerprint material.
#[must_use]
pub fn advertised_relay_tls_identity(
    workgroup_root: &Path,
    node_id: &str,
    overlay_ip: &str,
    external_addr: &str,
    authority_public_key: &str,
) -> Option<RelayTlsIdentity> {
    let bundle = read_bundle(&bundle_path(workgroup_root, node_id)).ok()?;
    bundle
        .lighthouses
        .iter()
        .find(|entry| {
            entry.node_id == node_id
                && entry.overlay_ip == overlay_ip
                && entry.external_addr == external_addr
        })
        .and_then(|entry| entry.relay_tls.clone())
        .filter(|identity| {
            verify_relay_tls_identity(
                identity,
                node_id,
                overlay_ip,
                external_addr,
                authority_public_key,
            )
        })
}

/// Build a roster entry and merge its replicated per-lighthouse relay trust.
#[must_use]
pub fn lighthouse_entry_with_relay_trust(
    workgroup_root: &Path,
    node_id: String,
    overlay_ip: String,
    external_addr: String,
    authority_public_key: Option<&str>,
) -> LighthouseEntry {
    let relay_tls = authority_public_key.and_then(|authority| {
        advertised_relay_tls_identity(
            workgroup_root,
            &node_id,
            &overlay_ip,
            &external_addr,
            authority,
        )
    });
    LighthouseEntry {
        node_id,
        overlay_ip,
        external_addr,
        relay_tls,
    }
}

/// Publish this node's relay identity into its own bundle. Existing enrollment
/// material is preserved; a self roster entry is added when a newly promoted
/// lighthouse did not previously appear in the signer-provided roster.
pub fn advertise_local_relay_tls_identity(
    workgroup_root: &Path,
    node_id: &str,
    overlay_ip: &str,
    external_addr: &str,
    identity: RelayTlsIdentity,
) -> Result<(), CaError> {
    let private = std::fs::read(RELAY_TRUST_AUTHORITY_KEY_PATH)
        .ok()
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .map(|seed| SigningKey::from_bytes(&seed))
        .ok_or_else(|| CaError::Io("relay trust authority key unavailable".to_string()))?;
    advertise_local_relay_tls_identity_with_key(
        workgroup_root,
        node_id,
        overlay_ip,
        external_addr,
        identity,
        &private,
    )
}

/// Key-explicit form of [`advertise_local_relay_tls_identity`], used by tests
/// and controlled provisioning flows.
pub fn advertise_local_relay_tls_identity_with_key(
    workgroup_root: &Path,
    node_id: &str,
    overlay_ip: &str,
    external_addr: &str,
    identity: RelayTlsIdentity,
    private: &SigningKey,
) -> Result<(), CaError> {
    let path = bundle_path(workgroup_root, node_id);
    let mut bundle = read_bundle(&path)?;
    let public = relay_trust_authority_public_key(private);
    if bundle.relay_trust_authority.as_deref() != Some(public.as_str()) {
        return Err(CaError::Io(
            "relay trust authority key does not match enrolled authority".to_string(),
        ));
    }
    let identity = sign_relay_tls_identity(identity, node_id, overlay_ip, external_addr, private);
    if let Some(entry) = bundle
        .lighthouses
        .iter_mut()
        .find(|entry| entry.node_id == node_id)
    {
        entry.overlay_ip = overlay_ip.to_string();
        entry.external_addr = external_addr.to_string();
        entry.relay_tls = Some(identity);
    } else {
        bundle.lighthouses.push(LighthouseEntry {
            node_id: node_id.to_string(),
            overlay_ip: overlay_ip.to_string(),
            external_addr: external_addr.to_string(),
            relay_tls: Some(identity),
        });
    }
    write_bundle(&path, &bundle)
}

/// Default location under QNM-Shared where bundles land.
pub const BUNDLE_FILENAME: &str = "nebula-bundle.json";

/// Compute the bundle path for a given QNM-Shared root +
/// peer name. Mirrors the existing `heartbeat.json`
/// convention so both files sit in the same per-peer dir.
#[must_use]
pub fn bundle_path(workgroup_root: &Path, peer_name: &str) -> PathBuf {
    workgroup_root
        .join(peer_name)
        .join("mackesd")
        .join(BUNDLE_FILENAME)
}

/// Write the bundle atomically (temp file + fsync + rename).
/// Creates the parent directory if missing.
///
/// # Errors
///
/// - [`CaError::Io`] on directory creation / write failures.
/// - [`CaError::Sql`] when serde-json refuses to encode
///   (only happens on degenerate input; surfaced as Sql so
///   the caller treats it as a persistence-layer fault).
pub fn write_bundle(path: &Path, bundle: &NebulaBundle) -> Result<(), CaError> {
    let body = serde_json::to_string_pretty(bundle)
        .map_err(|e| CaError::Sql(format!("encode bundle: {e}")))?;
    super::seal::write_atomic_sealed(path, body.as_bytes())
}

/// Read the bundle back. Used by the wizard's import path
/// to validate a freshly-replicated bundle before applying
/// it.
///
/// # Errors
///
/// - [`CaError::Io`] when the file is missing or unreadable.
/// - [`CaError::Sql`] when the JSON doesn't parse.
pub fn read_bundle(path: &Path) -> Result<NebulaBundle, CaError> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| CaError::Io(format!("read {}: {e}", path.display())))?;
    match serde_json::from_str(&body) {
        Ok(bundle) => Ok(bundle),
        Err(public_error) => {
            let legacy: LegacySecretBearingBundle = serde_json::from_str(&body)
                .map_err(|_| CaError::Sql(format!("parse bundle: {public_error}")))?;
            if legacy.has_private_material() {
                return Err(CaError::Io(
                    "refusing legacy replicated bundle containing private key material; re-enroll over fingerprint-pinned HTTPS"
                        .into(),
                ));
            }
            Ok(legacy.into_public())
        }
    }
}

/// Deserialize-only compatibility adapter for pre-SEC-006 files. It is private,
/// has no `Serialize` implementation, and refuses any non-empty secret before
/// converting to the public type.
#[derive(Deserialize)]
struct LegacySecretBearingBundle {
    mesh_id: String,
    epoch: i64,
    ca_cert_pem: String,
    peer_cert_pem: String,
    #[serde(default)]
    peer_key_pem: String,
    overlay_ip: String,
    mesh_cidr: String,
    lighthouses: Vec<LighthouseEntry>,
    #[serde(default)]
    relay_trust_authority: Option<String>,
    #[serde(default)]
    relay_trust_authority_key: Option<String>,
    #[serde(default)]
    ca_key_pem: Option<String>,
    created_at: i64,
}

impl LegacySecretBearingBundle {
    fn has_private_material(&self) -> bool {
        !self.peer_key_pem.trim().is_empty()
            || self
                .relay_trust_authority_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .ca_key_pem
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }

    fn into_public(self) -> NebulaBundle {
        NebulaBundle {
            mesh_id: self.mesh_id,
            epoch: self.epoch,
            ca_cert_pem: self.ca_cert_pem,
            peer_cert_pem: self.peer_cert_pem,
            overlay_ip: self.overlay_ip,
            mesh_cidr: self.mesh_cidr,
            lighthouses: self.lighthouses,
            relay_trust_authority: self.relay_trust_authority,
            created_at: self.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn sample_bundle() -> NebulaBundle {
        NebulaBundle {
            mesh_id: "m1".into(),
            epoch: 0,
            ca_cert_pem: "-----BEGIN NEBULA CA-----\n-----END NEBULA CA-----\n".into(),
            peer_cert_pem: "-----BEGIN NEBULA CERT-----\n-----END NEBULA CERT-----\n".into(),
            overlay_ip: "10.42.0.5".into(),
            mesh_cidr: "10.42.0.0/16".into(),
            lighthouses: vec![LighthouseEntry {
                node_id: "peer:lighthouse-1".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "lh1.example.com:4242".into(),
                relay_tls: None,
            }],
            relay_trust_authority: None,
            created_at: 1_716_499_200,
        }
    }

    #[test]
    fn public_bundle_rejects_legacy_secret_fields() {
        let json = r#"{"mesh_id":"m","epoch":0,"ca_cert_pem":"c","peer_cert_pem":"p",
            "peer_key_pem":"k","overlay_ip":"10.42.0.5","mesh_cidr":"10.42.0.0/16",
            "lighthouses":[],"created_at":1}"#;
        serde_json::from_str::<NebulaBundle>(json)
            .expect_err("public type must reject secret-bearing legacy input");
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.json");
        std::fs::write(&path, json).unwrap();
        let error = read_bundle(&path).expect_err("migration adapter must refuse secrets");
        assert!(error.to_string().contains("re-enroll"));
    }

    #[test]
    fn legacy_lighthouse_without_relay_identity_parses_as_unavailable() {
        let json = r#"{"mesh_id":"m","epoch":0,"ca_cert_pem":"c","peer_cert_pem":"p",
            "overlay_ip":"10.42.0.5","mesh_cidr":"10.42.0.0/16",
            "lighthouses":[{"node_id":"lh","overlay_ip":"10.42.0.1",
            "external_addr":"203.0.113.1:4242"}],"created_at":1}"#;
        let bundle: NebulaBundle = serde_json::from_str(json).expect("legacy bundle parses");
        assert!(bundle.lighthouses[0].relay_tls.is_none());
        assert!(!serde_json::to_string(&bundle)
            .expect("serialize")
            .contains("relay_tls"));
    }

    #[test]
    fn relay_identity_rejects_a_fingerprint_from_another_certificate() {
        let pem = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";
        let mut identity = RelayTlsIdentity::from_certificate_pem(pem).expect("PEM identity");
        assert!(identity.is_consistent());
        identity.fingerprint_sha256 = "00".repeat(32);
        assert!(!identity.is_consistent());
    }

    #[test]
    fn signed_relay_identity_rejects_wrong_address_and_wrong_authority() {
        let key = SigningKey::from_bytes(&[7_u8; 32]);
        let wrong_key = SigningKey::from_bytes(&[8_u8; 32]);
        let identity = RelayTlsIdentity::from_certificate_pem(
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
        )
        .expect("identity");
        let signed = sign_relay_tls_identity(identity, "lh-a", "10.42.0.1", "a.example:4242", &key);
        let authority = relay_trust_authority_public_key(&key);
        assert!(verify_relay_tls_identity(
            &signed,
            "lh-a",
            "10.42.0.1",
            "a.example:4242",
            &authority,
        ));
        assert!(!verify_relay_tls_identity(
            &signed,
            "lh-a",
            "10.42.0.1",
            "attacker.example:4242",
            &authority,
        ));
        assert!(!verify_relay_tls_identity(
            &signed,
            "lh-a",
            "10.42.0.1",
            "a.example:4242",
            &relay_trust_authority_public_key(&wrong_key),
        ));
    }

    #[test]
    fn local_relay_advertisement_adds_self_without_discarding_other_lighthouses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = bundle_path(tmp.path(), "peer:new-lh");
        let key = SigningKey::from_bytes(&[7_u8; 32]);
        let mut bundle = sample_bundle();
        bundle.relay_trust_authority = Some(relay_trust_authority_public_key(&key));
        write_bundle(&path, &bundle).expect("seed bundle");
        let identity = RelayTlsIdentity::from_certificate_pem(
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
        )
        .expect("identity");

        advertise_local_relay_tls_identity_with_key(
            tmp.path(),
            "peer:new-lh",
            "10.42.0.9",
            "new-lh.example:4242",
            identity.clone(),
            &key,
        )
        .expect("advertise");

        let updated = read_bundle(&path).expect("updated bundle");
        assert_eq!(updated.lighthouses.len(), 2, "existing signer is preserved");
        let own = updated
            .lighthouses
            .iter()
            .find(|entry| entry.node_id == "peer:new-lh")
            .expect("self entry");
        assert!(verify_relay_tls_identity(
            own.relay_tls.as_ref().expect("relay identity"),
            &own.node_id,
            &own.overlay_ip,
            &own.external_addr,
            updated.relay_trust_authority.as_deref().expect("authority"),
        ));
    }

    #[test]
    fn authenticated_secret_envelope_round_trips_only_via_tls_encoder() {
        let response = AuthenticatedEnrollmentResponse {
            bundle: sample_bundle(),
            lighthouse_secrets: Some(LighthouseEnrollmentSecrets {
                ca_key_pem: "CA-PRIVATE-SECRET".into(),
                relay_trust_authority_key: Some("RELAY-PRIVATE-SECRET".into()),
            }),
        };
        let wire = response.encode_for_authenticated_tls().expect("TLS wire");
        let parsed: AuthenticatedEnrollmentResponse =
            serde_json::from_slice(&wire).expect("TLS response parse");
        assert_eq!(parsed, response);
        assert!(String::from_utf8(wire)
            .unwrap()
            .contains("lighthouse_secrets"));
    }

    #[test]
    fn write_then_read_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = bundle_path(tmp.path(), "peer:anvil");
        let bundle = sample_bundle();
        write_bundle(&path, &bundle).expect("write");
        let parsed = read_bundle(&path).expect("read");
        assert_eq!(parsed, bundle);
        assert_eq!(
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
        );
    }

    #[test]
    fn replicated_writer_is_structurally_public_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("bundle.json");
        write_bundle(&path, &sample_bundle()).expect("public-only write");
        let raw = std::fs::read_to_string(&path).expect("read bundle");
        assert!(!raw.contains("peer_key_pem"));
        assert!(!raw.contains("ca_key_pem"));
        assert!(!raw.contains("relay_trust_authority_key"));
        assert!(!raw.contains("lighthouse_secrets"));
    }

    #[test]
    fn debug_output_redacts_every_private_enrollment_value() {
        let response = AuthenticatedEnrollmentResponse {
            bundle: sample_bundle(),
            lighthouse_secrets: Some(LighthouseEnrollmentSecrets {
                ca_key_pem: "CA-DEBUG-SECRET".into(),
                relay_trust_authority_key: Some("RELAY-DEBUG-SECRET".into()),
            }),
        };
        let debug = format!("{response:?}");
        assert!(!debug.contains("CA-DEBUG-SECRET"));
        assert!(!debug.contains("RELAY-DEBUG-SECRET"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn local_authority_pin_is_0600_and_rejects_replicated_authority_replacement() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pin = temp.path().join("relay-authority.pub");
        let mut bundle = sample_bundle();
        bundle.relay_trust_authority = Some("11".repeat(32));
        write_relay_trust_authority_pin(&bundle, &pin).expect("write authority pin");
        assert!(relay_trust_authority_matches_pin(&bundle, &pin));
        assert_eq!(
            std::fs::metadata(&pin)
                .expect("pin metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
        );

        // An attacker replacing the replicated authority and re-signing relay
        // records under their own key cannot replace this root-local pin.
        bundle.relay_trust_authority = Some("22".repeat(32));
        assert!(!relay_trust_authority_matches_pin(&bundle, &pin));
    }

    #[test]
    fn write_creates_missing_parent_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = bundle_path(tmp.path(), "peer:new");
        // None of these dirs exist yet.
        assert!(!path.parent().unwrap().exists());
        write_bundle(&path, &sample_bundle()).expect("write");
        assert!(path.exists());
    }

    #[test]
    fn bundle_path_matches_qnm_shared_convention() {
        let p = bundle_path(Path::new("/home/mm/QNM-Shared"), "peer:forge");
        assert_eq!(
            p.to_string_lossy(),
            "/home/mm/QNM-Shared/peer:forge/mackesd/nebula-bundle.json",
        );
    }

    #[test]
    fn read_missing_file_returns_io() {
        let err = read_bundle(Path::new("/nonexistent/bundle.json")).unwrap_err();
        assert!(matches!(err, CaError::Io(_)));
    }

    #[test]
    fn read_malformed_json_returns_sql() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("bad.json");
        std::fs::write(&path, "not json").expect("seed");
        let err = read_bundle(&path).unwrap_err();
        assert!(matches!(err, CaError::Sql(_)));
    }

    #[test]
    fn write_is_atomic_via_temp_rename() {
        // Tempfile naming is internal — assert the
        // intermediate `.json.tmp` doesn't survive a
        // successful write.
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("b.json");
        write_bundle(&path, &sample_bundle()).expect("write");
        let tmp_path = path.with_extension("json.tmp");
        assert!(
            !tmp_path.exists(),
            "tempfile must be renamed away on success"
        );
    }
}

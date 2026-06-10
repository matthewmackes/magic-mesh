//! SEC-4 (Q24/25) — the operator-initiated outbound first pair.
//!
//! Every piece existed; nothing orchestrated them. This module is
//! that flow: dial the device's TLS port unpinned (the deliberate
//! TOFU moment — Q21/22), **capture the fingerprint of the cert the
//! device actually presented**, derive + persist a session key
//! (SEC-8's hook, now called from production), and write the
//! [`crate::pairing::DeviceRecord`] with the captured pin — so every
//! later connection runs the strict [`PinnedFingerprintVerifier`]
//! path. RSA-4096 identity stays as locked (Q23): the TLS identity
//! cert is issued over the host's RSA-4096 key.

use std::net::SocketAddr;

use mde_kdc_proto::crypto::generate_session_key;

use crate::error::HostError;
use crate::pairing::{DeviceRecord, PairingStore};
use crate::tls;

/// What a completed first pair pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairOutcome {
    pub device_id: String,
    /// SHA-256 fingerprint of the cert the device presented —
    /// the pin all future connections verify against.
    pub fingerprint: String,
    pub addr: SocketAddr,
}

/// Run the outbound first pair against `addr`.
///
/// Refuses when the device is already paired with a pin (re-pairing
/// a pinned device must be an explicit unpair first — silently
/// re-TOFUing a known device is how MITM gets invited back).
///
/// # Errors
/// Dial/TLS failures, missing presented cert, store persistence.
pub async fn first_pair(
    store: &PairingStore,
    device_id: &str,
    device_name: &str,
    addr: SocketAddr,
) -> Result<PairOutcome, HostError> {
    if let Some(existing) = store.get(device_id) {
        if !existing.fingerprint.is_empty() {
            return Err(HostError::Pairing(format!(
                "{device_id} is already paired with a pinned fingerprint — \
                 unpair first (re-TOFU of a known device is refused, SEC-4)"
            )));
        }
    }
    // 1. The TOFU dial: no pin yet, accept the presented cert once.
    let stream = tls::connect_pinned_tls(addr, device_id, None)
        .await
        .map_err(|e| HostError::Pairing(format!("first-pair dial {addr}: {e}")))?;
    // 2. Capture what the device actually presented.
    let (_, conn) = stream.get_ref();
    let presented = conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or_else(|| {
            HostError::Pairing("device presented no certificate during first pair".into())
        })?;
    let fingerprint = tls::compute_fingerprint(presented.as_ref());
    // 3. Session key — generated now, sealed at rest (SEC-8).
    let session =
        generate_session_key().map_err(|e| HostError::Pairing(format!("session keygen: {e}")))?;
    store.install_and_persist_session(device_id, &session)?;
    // 4. The pin write — the moment trust-on-first-use becomes trust.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    store.pair(DeviceRecord {
        device_id: device_id.to_string(),
        device_name: device_name.to_string(),
        paired_at_ms: now_ms,
        fingerprint: fingerprint.clone(),
    })?;
    tracing::info!(
        device = %device_id, %fingerprint, %addr,
        "SEC-4: outbound first pair complete — fingerprint pinned"
    );
    Ok(PairOutcome {
        device_id: device_id.to_string(),
        fingerprint,
        addr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio_rustls::TlsAcceptor;

    /// Loopback TLS "device": a one-connection acceptor presenting a
    /// fresh identity cert; returns (addr, expected_fingerprint).
    async fn spawn_device() -> (SocketAddr, String) {
        let pkcs8 = crate::keygen::generate_pkcs8().expect("keygen");
        let cert = crate::keygen::issue_identity_cert(&pkcs8, "device-T").expect("cert");
        let expected = tls::compute_fingerprint(&cert);
        let config = tls::build_server_config(&cert, &pkcs8).expect("server config");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                let acceptor = TlsAcceptor::from(Arc::new(config));
                if let Ok(stream) = acceptor.accept(tcp).await {
                    // Hold the link briefly so the client can read certs.
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    drop(stream);
                }
            }
        });
        (addr, expected)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_pair_captures_the_presented_fingerprint_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PairingStore::open(tmp.path()).expect("store");
        let (addr, expected_fp) = spawn_device().await;

        let outcome = first_pair(&store, "device-T", "Test Phone", addr)
            .await
            .expect("first pair");
        // The pin is what the device PRESENTED, not operator input.
        assert_eq!(outcome.fingerprint, expected_fp);
        let rec = store.get("device-T").expect("paired");
        assert_eq!(rec.fingerprint, expected_fp);
        assert_eq!(rec.device_name, "Test Phone");
        // SEC-8 — the session survived to disk (sealed).
        assert!(tmp.path().join("sessions.enc").exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn repairing_a_pinned_device_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PairingStore::open(tmp.path()).expect("store");
        store
            .pair(DeviceRecord {
                device_id: "device-T".into(),
                device_name: "Phone".into(),
                paired_at_ms: 1,
                fingerprint: "aa".repeat(32),
            })
            .unwrap();
        let r = first_pair(&store, "device-T", "Phone", "127.0.0.1:1".parse().unwrap()).await;
        assert!(
            matches!(&r, Err(HostError::Pairing(m)) if m.contains("unpair first")),
            "re-TOFU of a pinned device must be refused: {r:?}"
        );
    }
}

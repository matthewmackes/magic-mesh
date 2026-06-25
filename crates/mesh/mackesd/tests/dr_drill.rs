//! EFF-28 — the disaster-recovery drill, as an integration test.
//!
//! Walks the lighthouse-loss runbook (`docs/help/mesh-recovery.md`)
//! end-to-end against the real library surfaces, so the DR path can
//! never silently rot:
//!
//!   1. **Backup exists** — the daily worker's artifact: a sealed +
//!      armored `state-backup.enc` carrying the CA + peer certs
//!      (what survives on the replicated volume when the lighthouse
//!      box dies).
//!   2. **Lighthouse loss** — the rebuilt box starts from a FRESH,
//!      empty store.
//!   3. **Restore** — dearmor → unseal → `restore_to_store`; the
//!      wrong passphrase must fail (tamper/typo detection), the
//!      right one must land every row.
//!   4. **Re-enroll capability** — post-restore, the box can mint a
//!      new single-use enroll bearer (the `enroll-token` path) and
//!      re-export a fresh backup (the chain is re-established).

use mackesd_core::bearer_ledger;
use mackesd_core::ca::backup::{
    armor, dearmor, restore_to_store, seal, unseal, BundlePlaintext, CaCertRow, PeerCertRow,
};

const PASSPHRASE: &str = "drill-passphrase-0123";

/// The artifact a healthy lighthouse leaves behind on the replicated
/// volume — same shape the `nebula_ca_backup` worker writes.
fn surviving_bundle() -> BundlePlaintext {
    BundlePlaintext {
        schema_version: 1,
        exported_at: 1_750_000_000,
        mesh_id: "mesh-drill".into(),
        ca_certs: vec![CaCertRow {
            epoch: 3,
            ca_cert_pem: "-----BEGIN NEBULA CERTIFICATE-----\nDRILL-CA\n-----END NEBULA CERTIFICATE-----\n".into(),
            ca_key_pem: "-----BEGIN NEBULA ED25519 PRIVATE KEY-----\nDRILL-KEY\n-----END NEBULA ED25519 PRIVATE KEY-----\n".into(),
            created_at: 1_749_000_000,
            retired_at: None,
        }],
        peer_certs: vec![
            PeerCertRow {
                node_id: "peer:oak".into(),
                epoch: 3,
                cert_pem: "-----BEGIN CERT-----\nOAK\n-----END CERT-----\n".into(),
                overlay_ip: "10.42.0.5".into(),
                created_at: 1_749_100_000,
                expires_at: 0, // epoch-lifetime sentinel (SEC-1/Q19)
            },
            PeerCertRow {
                node_id: "peer:pine".into(),
                epoch: 3,
                cert_pem: "-----BEGIN CERT-----\nPINE\n-----END CERT-----\n".into(),
                overlay_ip: "10.42.0.6".into(),
                created_at: 1_749_200_000,
                expires_at: 0,
            },
        ],
    }
}

#[test]
fn dr_drill_lighthouse_loss_restore_reenroll() {
    // ── 1. The surviving artifact ────────────────────────────────
    let bundle = surviving_bundle();
    let sealed = seal(PASSPHRASE, &bundle).expect("seal");
    let armored = armor(&sealed, bundle.exported_at);
    let drill_dir = tempfile::tempdir().expect("tempdir");
    let bundle_path = drill_dir.path().join("state-backup.enc");
    std::fs::write(&bundle_path, &armored).expect("write bundle");

    // ── 2. Lighthouse loss: the rebuilt box's store is EMPTY ────
    let conn = mackesd_core::store::open_in_memory().expect("fresh store");
    let pre: i64 = conn
        .query_row("SELECT COUNT(*) FROM nebula_peer_certs", [], |r| r.get(0))
        .expect("count");
    assert_eq!(pre, 0, "the rebuilt box starts with no peer certs");

    // ── 3a. Restore with the WRONG passphrase must fail ─────────
    let read_back = std::fs::read_to_string(&bundle_path).expect("read bundle");
    let sealed_back = dearmor(&read_back).expect("dearmor");
    assert!(
        unseal("not-the-passphrase", &sealed_back).is_err(),
        "wrong passphrase must be rejected (AEAD)"
    );

    // ── 3b. Restore with the RIGHT passphrase lands every row ───
    let plaintext = unseal(PASSPHRASE, &sealed_back).expect("unseal");
    assert_eq!(plaintext.mesh_id, "mesh-drill");
    restore_to_store(&conn, &plaintext).expect("restore_to_store");

    let ca_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM nebula_ca", [], |r| r.get(0))
        .expect("ca count");
    let peer_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM nebula_peer_certs", [], |r| r.get(0))
        .expect("peer count");
    assert_eq!(ca_rows, 1, "CA epoch restored");
    assert_eq!(peer_rows, 2, "both peer certs restored");
    let (epoch, ip): (i64, String) = conn
        .query_row(
            "SELECT epoch, overlay_ip FROM nebula_peer_certs WHERE node_id = 'peer:oak'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("oak row");
    assert_eq!((epoch, ip.as_str()), (3, "10.42.0.5"));

    // ── 4a. Re-enroll capability: a fresh single-use bearer mints ─
    let qnm = drill_dir.path().join("qnm");
    let bearer = bearer_ledger::issue(&qnm, "dr-drill rejoin").expect("bearer mints post-restore");
    // 32 random bytes, URL-safe base64 unpadded → 43 chars.
    assert_eq!(bearer.len(), 43, "256-bit base64url bearer");
    assert!(bearer
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));

    // ── 4b. The backup chain re-establishes: export-after-restore ─
    let re_exported = mackesd_core::ca::backup::assemble_from_store(
        &conn,
        "mesh-drill",
        &plaintext.ca_certs[0].ca_key_pem,
    )
    .expect("assemble from the restored store");
    assert_eq!(re_exported.ca_certs.len(), 1);
    assert_eq!(re_exported.peer_certs.len(), 2);
    let resealed = seal(PASSPHRASE, &re_exported).expect("re-seal");
    let round = unseal(PASSPHRASE, &resealed).expect("re-unseal");
    assert_eq!(round.peer_certs.len(), 2, "the new backup round-trips");
}

#[test]
fn dr_drill_tampered_bundle_is_rejected() {
    // A flipped ciphertext byte must fail the AEAD — the drill's
    // integrity leg (and what `state-restore --verify` exercises).
    let bundle = surviving_bundle();
    let mut sealed = seal(PASSPHRASE, &bundle).expect("seal");
    let mid = sealed.len() / 2;
    sealed[mid] ^= 0x01;
    assert!(
        unseal(PASSPHRASE, &sealed).is_err(),
        "tampered ciphertext must be rejected"
    );
}

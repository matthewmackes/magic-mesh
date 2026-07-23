//! NF-18.1 (v2.5) — passphrase-encrypted CA backup format.
//!
//! Operator-driven CA disaster-recovery backup. Bundle layout:
//!
//! ```text
//! -----BEGIN MACKES NEBULA CA EXPORT-----
//! Version: 1
//! Exported-At: 2026-05-24T10:30:00Z
//!
//! <base64 of binary bundle>
//! -----END MACKES NEBULA CA EXPORT-----
//! ```
//!
//! The binary bundle is:
//!
//!   [0..4]   Magic   `MNCA` ("Mackes Nebula CA Archive")
//!   [4]      Version `0x01`
//!   [5..21]  Salt    16 random bytes — Argon2id input
//!   [21..45] Nonce   24 random bytes — XChaCha20-Poly1305
//!   [45..]   Ciphertext (XChaCha20-Poly1305 over the JSON plaintext)
//!
//! Plaintext JSON shape: [`BundlePlaintext`]. Carries the CA cert
//! + key PEMs + every signed peer cert under the active epoch.
//!
//! The binary envelope itself (magic + version + salt + nonce +
//! ciphertext), the Argon2id + XChaCha20-Poly1305 primitives, and the
//! [`BackupError`] type now live in the shared [`mde_seal`] leaf crate
//! (arch-7) so `browser_passkeys`, the VPN secret store, and this CA
//! disaster-recovery path all share ONE audited seal implementation.
//! They are re-exported below VERBATIM, so this module's callers +
//! wrappers are byte-identical to the pre-extraction code.
//!
//! Crypto choices (best-choice per iteration skill standing
//! authorizations, locked 2026-05-24):
//!
//!   * **KDF:** Argon2id, default params (t=2, m=19456 KiB, p=1).
//!     Picks the OWASP 2023 baseline; trades off ~1 s of
//!     derivation time on a desktop for memory-hard resistance.
//!   * **AEAD:** XChaCha20-Poly1305 (24-byte nonce). The
//!     wider nonce eliminates birthday-bound concerns even
//!     under random-nonce-per-message policy.
//!   * **Versioned envelope:** future swaps (AES-GCM, libsodium,
//!     etc.) ship as a new version byte without breaking old
//!     backups. Today only `0x01` exists.
//!
//! Threat model: an adversary with stolen .enc bundle bytes,
//! offline-attacker compute, no online oracle. They need to
//! brute-force the passphrase to recover the CA key. Argon2id's
//! memory hardness raises the per-guess cost well past
//! commodity-GPU brute-force feasibility for any operator-typed
//! passphrase ≥ 8 random chars.

use serde::{Deserialize, Serialize};

use super::CaError;

// arch-7 — the passphrase-sealed byte envelope, the `BackupError` type, the raw
// `seal_bytes`/`unseal_bytes` primitives, and the framing constants now live in
// the shared `mde-seal` leaf crate. Re-exported here so every existing
// `ca::backup::{seal_bytes, unseal_bytes, BackupError, BUNDLE_MAGIC, …}` caller
// (and this module's `seal`/`unseal`/`armor` wrappers below) compiles unchanged.
pub use mde_seal::{
    seal_bytes, unseal_bytes, BackupError, BUNDLE_MAGIC, BUNDLE_VERSION, HEADER_LEN, NONCE_LEN,
    SALT_LEN,
};

/// Plaintext JSON the [`seal`] caller hands in. The CA mint
/// path writes its own files separately; this format is the
/// off-cluster shareable copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundlePlaintext {
    /// Bundle plaintext schema version. `1` = CA-only. Readers
    /// tolerate unknown fields via serde defaults (so bundles written
    /// by older daemons that carried a `meshfs_snapshot` still decode —
    /// that LizardFS field is now dropped on read).
    pub schema_version: u32,
    /// Unix-epoch seconds when the export was generated.
    pub exported_at: i64,
    /// The mesh-id this CA serves.
    pub mesh_id: String,
    /// PEM body of the public CA cert (one per active epoch —
    /// rare to have more than one, but the field is a Vec for
    /// the multi-epoch case where an operator wants both the
    /// current and one-back CA in the backup).
    pub ca_certs: Vec<CaCertRow>,
    /// One row per signed peer cert under the active epoch.
    pub peer_certs: Vec<PeerCertRow>,
}

/// One CA cert + matching private key entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaCertRow {
    /// CA epoch.
    pub epoch: i64,
    /// PEM body of the CA cert.
    pub ca_cert_pem: String,
    /// PEM body of the CA private key. Sensitive — never
    /// emitted outside the encrypted envelope.
    pub ca_key_pem: String,
    /// Unix-epoch seconds when the CA was minted.
    pub created_at: i64,
    /// Unix-epoch seconds when retired; `None` for the active CA.
    pub retired_at: Option<i64>,
}

/// One peer cert entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCertRow {
    /// Peer's node-id.
    pub node_id: String,
    /// CA epoch under which the cert was signed.
    pub epoch: i64,
    /// PEM body of the peer cert.
    pub cert_pem: String,
    /// Overlay IP allocated to the peer.
    pub overlay_ip: String,
    /// Requester-owned Nebula public key used for public-key-only rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key_pem: Option<String>,
    /// Unix-epoch seconds when the cert was signed.
    pub created_at: i64,
    /// Unix-epoch seconds when the cert expires.
    pub expires_at: i64,
}

impl From<BackupError> for CaError {
    fn from(e: BackupError) -> Self {
        CaError::Io(format!("backup: {e}"))
    }
}

/// Encrypt + serialize. Returns the binary bundle (NOT the
/// ASCII-armored envelope — use [`armor`] for that).
///
/// # Errors
///
/// Per [`BackupError`].
pub fn seal(passphrase: &str, plaintext: &BundlePlaintext) -> Result<Vec<u8>, BackupError> {
    let json = serde_json::to_vec(plaintext).map_err(|e| BackupError::Json(e.to_string()))?;
    seal_bytes(passphrase, &json)
}

/// Decrypt + deserialize. Inverse of [`seal`]. Accepts the
/// binary bundle (post-armor-strip) OR the raw bytes returned
/// by [`seal`].
///
/// # Errors
///
/// Per [`BackupError`]. Wrong passphrase + tampered ciphertext
/// both surface as `Aead` (intentional — the AEAD-tag-mismatch
/// error is indistinguishable, and exposing the distinction
/// would help an attacker confirm a tamper attempt).
pub fn unseal(passphrase: &str, sealed: &[u8]) -> Result<BundlePlaintext, BackupError> {
    let plain_bytes = unseal_bytes(passphrase, sealed)?;
    serde_json::from_slice(&plain_bytes).map_err(|e| BackupError::Json(e.to_string()))
}

/// ASCII-armor a binary bundle for sneakernet-friendly transport
/// (paste into a chat, attach to email, etc.).
#[must_use]
pub fn armor(binary: &[u8], exported_at: i64) -> String {
    use base64::Engine;
    let body = base64::engine::general_purpose::STANDARD.encode(binary);
    // 64-char-wide body wrap matches the PEM convention.
    let wrapped: String = body
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let ts = format_iso8601(exported_at);
    format!(
        "-----BEGIN MACKES NEBULA CA EXPORT-----\nVersion: {BUNDLE_VERSION}\nExported-At: {ts}\n\n{wrapped}\n-----END MACKES NEBULA CA EXPORT-----\n",
    )
}

/// Strip the ASCII armor + base64-decode back to the binary
/// bundle. Symmetric with [`armor`]. Tolerant of CRLF line
/// endings + extra whitespace.
///
/// # Errors
///
/// Returns [`BackupError::Armor`] on missing delimiters or
/// base64-decode failure.
pub fn dearmor(armored: &str) -> Result<Vec<u8>, BackupError> {
    use base64::Engine;
    if !armored.contains("BEGIN MACKES NEBULA CA EXPORT") {
        return Err(BackupError::Armor(
            "missing BEGIN delimiter — input is not a Mackes Nebula CA bundle".into(),
        ));
    }
    let body: String = armored
        .lines()
        .skip_while(|l| !l.trim().is_empty())
        .skip(1) // the blank line itself
        .take_while(|l| !l.trim_start().starts_with("-----END"))
        // base64 STANDARD doesn't tolerate inline whitespace —
        // filter out spaces / tabs / CR / LF in case the bundle
        // got mangled by a chat client. Newlines between
        // 64-char chunks are already stripped via .lines().
        .map(|l| l.chars().filter(|c| !c.is_whitespace()).collect::<String>())
        .collect();
    if body.is_empty() {
        return Err(BackupError::Armor(
            "no body found between BEGIN/END delimiters".into(),
        ));
    }
    base64::engine::general_purpose::STANDARD
        .decode(&body)
        .map_err(|e| BackupError::Armor(e.to_string()))
}

// ----- internals --------------------------------------------

fn format_iso8601(epoch_secs: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_opt(epoch_secs, 0)
        .single()
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("epoch:{epoch_secs}"))
}

/// Pure helper — read every active CA + peer cert row from the
/// store and assemble a [`BundlePlaintext`] ready for [`seal`].
/// The caller passes `mesh_id` so the export is mesh-scoped (a
/// future multi-mesh deployment can have separate bundles per
/// mesh).
///
/// # Errors
///
/// Returns [`CaError::Sql`] on any SQLite read failure.
pub fn assemble_from_store(
    conn: &rusqlite::Connection,
    mesh_id: &str,
    ca_key_pem: &str,
) -> Result<BundlePlaintext, CaError> {
    let mut ca_certs = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT epoch, ca_cert_pem, created_at, retired_at \
                 FROM nebula_ca WHERE mesh_id = ?1 \
                 ORDER BY epoch DESC LIMIT 1",
            )
            .map_err(|e| CaError::Sql(e.to_string()))?;
        let rows = stmt
            .query_map([mesh_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(|e| CaError::Sql(e.to_string()))?;
        for row in rows {
            let (epoch, cert_pem, created_at, retired_at) =
                row.map_err(|e| CaError::Sql(e.to_string()))?;
            ca_certs.push(CaCertRow {
                epoch,
                ca_cert_pem: cert_pem,
                ca_key_pem: ca_key_pem.to_string(),
                created_at,
                retired_at,
            });
        }
    }
    let mut peer_certs = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT node_id, epoch, cert_pem, overlay_ip, public_key_pem, created_at, expires_at \
                 FROM nebula_peer_certs \
                 WHERE revoked_at IS NULL \
                 ORDER BY node_id ASC, epoch DESC",
            )
            .map_err(|e| CaError::Sql(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PeerCertRow {
                    node_id: r.get(0)?,
                    epoch: r.get(1)?,
                    cert_pem: r.get(2)?,
                    overlay_ip: r.get(3)?,
                    public_key_pem: r.get(4)?,
                    created_at: r.get(5)?,
                    expires_at: r.get(6)?,
                })
            })
            .map_err(|e| CaError::Sql(e.to_string()))?;
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let r = row.map_err(|e| CaError::Sql(e.to_string()))?;
            if seen.insert(r.node_id.clone()) {
                peer_certs.push(r);
            }
        }
    }
    let exported_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(BundlePlaintext {
        schema_version: 1,
        exported_at,
        mesh_id: mesh_id.to_string(),
        ca_certs,
        peer_certs,
    })
}

/// Pure helper — apply a [`BundlePlaintext`] to a target store.
/// INSERT-OR-REPLACE semantics: rows already present at the same
/// (mesh_id, epoch) get overwritten. Used by the
/// `mackesd ca import` CLI's restore path.
///
/// # Errors
///
/// Returns [`CaError::Sql`] on any SQLite write failure.
pub fn restore_to_store(
    conn: &rusqlite::Connection,
    bundle: &BundlePlaintext,
) -> Result<(), CaError> {
    for ca in &bundle.ca_certs {
        conn.execute(
            "INSERT OR REPLACE INTO nebula_ca \
             (mesh_id, epoch, ca_cert_pem, created_at, retired_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                bundle.mesh_id,
                ca.epoch,
                ca.ca_cert_pem,
                ca.created_at,
                ca.retired_at
            ],
        )
        .map_err(|e| CaError::Sql(e.to_string()))?;
    }
    for p in &bundle.peer_certs {
        conn.execute(
            "INSERT OR REPLACE INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, public_key_pem, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                p.node_id,
                p.epoch,
                p.cert_pem,
                p.overlay_ip,
                p.public_key_pem,
                p.created_at,
                p.expires_at,
            ],
        )
        .map_err(|e| CaError::Sql(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plaintext() -> BundlePlaintext {
        BundlePlaintext {
            schema_version: 1,
            exported_at: 1716000000,
            mesh_id: "mesh-test".into(),
            ca_certs: vec![CaCertRow {
                epoch: 0,
                ca_cert_pem: "-----BEGIN CERT-----\nCAFAKE\n-----END CERT-----\n".into(),
                ca_key_pem: "-----BEGIN KEY-----\nKEYFAKE\n-----END KEY-----\n".into(),
                created_at: 1716000000,
                retired_at: None,
            }],
            peer_certs: vec![PeerCertRow {
                node_id: "peer:anvil".into(),
                epoch: 0,
                cert_pem: "-----BEGIN CERT-----\nPEER\n-----END CERT-----\n".into(),
                overlay_ip: "10.42.0.5".into(),
                public_key_pem: Some("-----BEGIN NEBULA X25519 PUBLIC KEY-----\nPUB\n-----END NEBULA X25519 PUBLIC KEY-----\n".into()),
                created_at: 1716000000,
                expires_at: 1747536000,
            }],
        }
    }

    #[test]
    fn seal_then_unseal_round_trips() {
        let pt = sample_plaintext();
        let sealed = seal("correct horse battery staple", &pt).expect("seal");
        let back = unseal("correct horse battery staple", &sealed).expect("unseal");
        assert_eq!(back, pt);
    }

    #[test]
    fn seal_rejects_empty_passphrase() {
        let pt = sample_plaintext();
        assert!(matches!(seal("", &pt), Err(BackupError::EmptyPassphrase)));
    }

    #[test]
    fn unseal_rejects_empty_passphrase() {
        assert!(matches!(
            unseal("", &[0u8; 200]),
            Err(BackupError::EmptyPassphrase)
        ));
    }

    #[test]
    fn unseal_rejects_wrong_passphrase() {
        let pt = sample_plaintext();
        let sealed = seal("right", &pt).expect("seal");
        let r = unseal("wrong", &sealed);
        assert!(matches!(r, Err(BackupError::Aead(_))));
    }

    #[test]
    fn unseal_rejects_truncated_bundle() {
        let r = unseal("any", &[0u8; 10]);
        match r {
            Err(BackupError::Format(msg)) => assert!(msg.contains("too short")),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    #[test]
    fn unseal_rejects_bad_magic() {
        let mut bad = vec![0u8; HEADER_LEN + 10];
        bad[..4].copy_from_slice(b"NOPE");
        let r = unseal("any", &bad);
        match r {
            Err(BackupError::Format(msg)) => assert!(msg.contains("magic mismatch")),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    #[test]
    fn unseal_rejects_unknown_version() {
        let mut bad = vec![0u8; HEADER_LEN + 10];
        bad[..4].copy_from_slice(BUNDLE_MAGIC);
        bad[4] = 0xFF;
        let r = unseal("any", &bad);
        match r {
            Err(BackupError::Format(msg)) => assert!(msg.contains("unknown version")),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    #[test]
    fn unseal_rejects_tampered_ciphertext() {
        let pt = sample_plaintext();
        let mut sealed = seal("right", &pt).expect("seal");
        // Flip one bit of the ciphertext.
        sealed[HEADER_LEN + 5] ^= 0x01;
        let r = unseal("right", &sealed);
        assert!(matches!(r, Err(BackupError::Aead(_))));
    }

    #[test]
    fn armor_then_dearmor_round_trips() {
        let pt = sample_plaintext();
        let sealed = seal("pp", &pt).expect("seal");
        // Pick a timestamp + assert on the ISO-8601 formatting
        // rather than a specific date string — the project's
        // narrative dates are unrelated to actual unix epoch.
        let envelope = armor(&sealed, 1716000000);
        assert!(envelope.contains("-----BEGIN MACKES NEBULA CA EXPORT-----"));
        assert!(envelope.contains("-----END MACKES NEBULA CA EXPORT-----"));
        assert!(envelope.contains("Version: 1"));
        // 1716000000 = 2024-05-18T04:00:00Z
        assert!(envelope.contains("Exported-At: 2024-05-18T"));
        let back = dearmor(&envelope).expect("dearmor");
        assert_eq!(back, sealed);
    }

    #[test]
    fn dearmor_rejects_missing_delimiters() {
        let r = dearmor("not an armored bundle");
        assert!(matches!(r, Err(BackupError::Armor(_))));
    }

    #[test]
    fn dearmor_tolerates_extra_whitespace() {
        let pt = sample_plaintext();
        let sealed = seal("pp", &pt).expect("seal");
        let envelope = armor(&sealed, 1716000000);
        // Add some trailing spaces + CRLF.
        let messy = envelope.replace('\n', " \n");
        let back = dearmor(&messy).expect("tolerant dearmor");
        assert_eq!(back, sealed);
    }

    #[test]
    fn end_to_end_armored_round_trip() {
        let pt = sample_plaintext();
        let sealed = seal("correct horse battery staple", &pt).expect("seal");
        let envelope = armor(&sealed, 1716000000);
        let decoded = dearmor(&envelope).expect("dearmor");
        let back = unseal("correct horse battery staple", &decoded).expect("unseal");
        assert_eq!(back, pt);
    }

    // ---- DAR-2: the `secret-seal`/`secret-unseal` thin-CLI path ----
    // These exercise the EXACT call sequence the bin's cmd_secret_seal /
    // cmd_secret_unseal use (seal_bytes → armor → dearmor → unseal_bytes) over
    // an arbitrary-bytes payload, including an identity-sized one — so the DR
    // CA/identity bundle (DAR-42) is proven to round-trip without re-rolling
    // crypto, and a wrong passphrase is rejected with the existing AEAD error.

    /// An age X25519 identity is a single ~74-char `AGE-SECRET-KEY-1…` line; the
    /// DR bundle this CLI seals is a few such keys + a CA PEM. We seal a
    /// realistic identity-sized blob (NUL-free here, but the bytes path is
    /// binary-safe — see `secret_seal_path_is_binary_safe`).
    const IDENTITY_BLOB: &[u8] =
        b"AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n\
          -----BEGIN MACKES MESH AGE RECIPIENT-----\nage1exampleexampleexample\n";

    #[test]
    fn secret_seal_identity_round_trips_through_armor() {
        let pp = "operator-DR-bundle-passphrase";
        let sealed = seal_bytes(pp, IDENTITY_BLOB).expect("seal_bytes");
        let armored = armor(&sealed, 1716000000);
        let decoded = dearmor(&armored).expect("dearmor");
        let back = unseal_bytes(pp, &decoded).expect("unseal_bytes");
        assert_eq!(back, IDENTITY_BLOB, "exact identity bytes must round-trip");
    }

    #[test]
    fn secret_seal_path_is_binary_safe() {
        // The CLI reads stdin with read_to_end → arbitrary bytes incl. NUL.
        let blob: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let pp = "pp-binary";
        let sealed = seal_bytes(pp, &blob).expect("seal_bytes");
        let back = unseal_bytes(pp, &dearmor(&armor(&sealed, 0)).unwrap()).expect("unseal_bytes");
        assert_eq!(back, blob);
    }

    #[test]
    fn secret_unseal_rejects_wrong_passphrase_no_plaintext() {
        let sealed = seal_bytes("right-phrase", IDENTITY_BLOB).expect("seal_bytes");
        let armored = armor(&sealed, 0);
        let decoded = dearmor(&armored).expect("dearmor");
        let r = unseal_bytes("wrong-phrase", &decoded);
        assert!(
            matches!(r, Err(BackupError::Aead(_))),
            "wrong passphrase must fail AEAD, never return plaintext"
        );
    }

    #[test]
    fn secret_seal_rejects_empty_passphrase() {
        assert!(matches!(
            seal_bytes("", IDENTITY_BLOB),
            Err(BackupError::EmptyPassphrase)
        ));
    }

    // ---- store integration (assemble + restore) -------------

    fn fresh_store() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        conn
    }

    #[test]
    fn assemble_from_empty_store_returns_empty_lists() {
        let conn = fresh_store();
        let pt = assemble_from_store(&conn, "test-mesh", "FAKE-KEY").expect("assemble");
        assert!(pt.ca_certs.is_empty());
        assert!(pt.peer_certs.is_empty());
        assert_eq!(pt.mesh_id, "test-mesh");
    }

    #[test]
    fn assemble_pulls_ca_and_peer_rows() {
        let conn = fresh_store();
        conn.execute(
            "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, created_at, retired_at) \
             VALUES ('m1', 0, 'CA-PEM', 100, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, created_at, expires_at) \
             VALUES ('peer:a', 0, 'P1', '10.42.0.5', 100, 200)",
            [],
        )
        .unwrap();
        let pt = assemble_from_store(&conn, "m1", "CA-KEY").expect("assemble");
        assert_eq!(pt.ca_certs.len(), 1);
        assert_eq!(pt.ca_certs[0].ca_cert_pem, "CA-PEM");
        assert_eq!(pt.ca_certs[0].ca_key_pem, "CA-KEY");
        assert_eq!(pt.peer_certs.len(), 1);
        assert_eq!(pt.peer_certs[0].node_id, "peer:a");
    }

    #[test]
    fn restore_round_trips_through_assemble() {
        let src = fresh_store();
        src.execute(
            "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, created_at, retired_at) \
             VALUES ('m1', 0, 'CA-PEM', 100, NULL)",
            [],
        )
        .unwrap();
        src.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, created_at, expires_at) \
             VALUES ('peer:a', 0, 'P1', '10.42.0.5', 100, 200)",
            [],
        )
        .unwrap();
        let pt = assemble_from_store(&src, "m1", "CA-KEY").expect("assemble");

        // Fresh dest store + restore.
        let dest = fresh_store();
        restore_to_store(&dest, &pt).expect("restore");

        // Re-assemble from dest + compare.
        let pt2 = assemble_from_store(&dest, "m1", "CA-KEY").expect("re-assemble");
        assert_eq!(pt2.ca_certs.len(), 1);
        assert_eq!(pt2.peer_certs.len(), 1);
        assert_eq!(pt2.ca_certs[0].ca_cert_pem, "CA-PEM");
        assert_eq!(pt2.peer_certs[0].node_id, "peer:a");
        assert_eq!(pt2.peer_certs[0].overlay_ip, "10.42.0.5");
    }
}

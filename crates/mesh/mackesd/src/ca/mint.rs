//! NF-2.2 (v2.5) — mint the mesh CA.
//!
//! Idempotent: re-calling `mint_ca` on a mesh that already
//! has an active (non-retired) CA returns the existing
//! row's PEM rather than minting a duplicate. Re-mint is only
//! triggered by NF-2.5 epoch rotation (separate entry point).

use std::path::Path;

use rusqlite::Connection;

use super::{seal, CaError, NebulaCertBackend, DEFAULT_CA_CERT_PATH, DEFAULT_CA_KEY_PATH};

/// Outcome of [`mint_ca`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintOutcome {
    /// The CA was freshly minted at the supplied paths +
    /// inserted into the `nebula_ca` table at epoch 0.
    Created {
        /// PEM body that landed in the `ca_cert_pem` column.
        cert_pem: String,
    },
    /// A non-retired CA already existed at epoch >= 0 — its
    /// existing PEM is returned unchanged.
    AlreadyMinted {
        /// Active epoch (>= 0).
        epoch: i64,
        /// Existing cert_pem from the database.
        cert_pem: String,
    },
}

/// Mint the mesh CA. Writes the public cert to `crt_path`
/// (mode 0644) + sealed private key to `key_path`
/// (mode 0600 via [`seal::write_sealed`]). Inserts a row
/// into `nebula_ca` with `epoch = 0` + `retired_at = NULL`.
/// Idempotent — re-calling against a mesh with an active
/// CA returns the existing row.
///
/// # Errors
/// - [`CaError::BinaryMissing`] when `nebula-cert` isn't on
///   PATH (Fedora `nebula` package not installed).
/// - [`CaError::Subprocess`] on a non-zero subprocess exit.
/// - [`CaError::Io`] on cert / key write failures.
/// - [`CaError::Sql`] on database errors.
pub fn mint_ca<B: NebulaCertBackend>(
    backend: &B,
    conn: &Connection,
    mesh_id: &str,
    crt_path: Option<&Path>,
    key_path: Option<&Path>,
) -> Result<MintOutcome, CaError> {
    // Idempotency: short-circuit if an active CA already
    // exists for this mesh.
    if let Some((epoch, pem)) = current_ca(conn, mesh_id)? {
        return Ok(MintOutcome::AlreadyMinted {
            epoch,
            cert_pem: pem,
        });
    }

    let crt = crt_path.unwrap_or_else(|| Path::new(DEFAULT_CA_CERT_PATH));
    let key = key_path.unwrap_or_else(|| Path::new(DEFAULT_CA_KEY_PATH));

    backend.mint_ca(mesh_id, crt, key)?;

    // Backends must install the generated pair through the atomic-pair helper.
    // Validate the private half before publishing the database row.
    seal::read_sealed(key)?;

    // The canonical CA-pair view is a project-owned symlink, so use the seal
    // reader that validates that shape before opening the final descriptor.
    // It then verifies the opened inode is regular and consumes at most the
    // shared CA-file ceiling before we materialize UTF-8 or SQLite text.
    let cert_bytes = seal::read_no_follow(crt)
        .map_err(|e| CaError::Io(format!("read CA cert {}: {e}", crt.display())))?;
    let cert_pem = String::from_utf8(cert_bytes).map_err(|e| {
        CaError::Io(format!(
            "read CA cert {}: invalid UTF-8: {e}",
            crt.display()
        ))
    })?;

    crate::store::writer::request_or_execute(
        conn,
        crate::store::writer::WriteOp::MintCa {
            mesh_id: mesh_id.to_owned(),
            ca_cert_pem: cert_pem.clone(),
        },
    )
    .and_then(crate::store::writer::WriteResponse::into_count)
    .map_err(|e| CaError::Sql(e.to_string()))?;

    tracing::info!(mesh_id, "nebula CA minted at epoch 0");
    Ok(MintOutcome::Created { cert_pem })
}

/// Return the active (non-retired) CA row for `mesh_id` if
/// any. Pure read — no IO outside SQLite.
pub fn current_ca(conn: &Connection, mesh_id: &str) -> Result<Option<(i64, String)>, CaError> {
    let mut stmt = conn
        .prepare(
            "SELECT epoch, ca_cert_pem FROM nebula_ca \
             WHERE mesh_id = ?1 AND retired_at IS NULL \
             ORDER BY epoch DESC LIMIT 1",
        )
        .map_err(|e| CaError::Sql(e.to_string()))?;
    let row: Option<(i64, String)> = stmt
        .query_row([mesh_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok();
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::MockBackend;

    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum CertificateFixture {
        Oversized,
        FinalSymlink,
        Directory,
    }

    #[cfg(unix)]
    struct HandoffBackend {
        fixture: CertificateFixture,
    }

    #[cfg(unix)]
    impl NebulaCertBackend for HandoffBackend {
        fn mint_ca(&self, _mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError> {
            seal::write_sealed(
                key_out,
                b"-----BEGIN NEBULA CA KEY-----\nmock\n-----END NEBULA CA KEY-----\n",
            )?;

            match self.fixture {
                CertificateFixture::Oversized => std::fs::write(
                    crt_out,
                    vec![b'x'; (seal::MAX_SEALED_FILE_BYTES + 1) as usize],
                )
                .map_err(|e| CaError::Io(e.to_string())),
                CertificateFixture::FinalSymlink => {
                    let target = crt_out.with_file_name("certificate-target.crt");
                    std::fs::write(&target, b"-----BEGIN NEBULA CA-----\nforbidden\n")
                        .map_err(|e| CaError::Io(e.to_string()))?;
                    std::os::unix::fs::symlink(&target, crt_out)
                        .map_err(|e| CaError::Io(e.to_string()))
                }
                CertificateFixture::Directory => {
                    std::fs::create_dir(crt_out).map_err(|e| CaError::Io(e.to_string()))
                }
            }
        }
    }

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        conn
    }

    #[test]
    fn mint_writes_pem_and_inserts_row() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let conn = fresh_conn();
        let backend = MockBackend;
        let outcome = mint_ca(&backend, &conn, "test-mesh", Some(&crt), Some(&key)).expect("mint");
        match outcome {
            MintOutcome::Created { cert_pem } => {
                assert!(cert_pem.contains("BEGIN NEBULA CA"));
            }
            other => panic!("expected Created, got {other:?}"),
        }
        // Row landed.
        let active = current_ca(&conn, "test-mesh").unwrap();
        assert!(active.is_some());
        let (epoch, _) = active.unwrap();
        assert_eq!(epoch, 0);
    }

    #[test]
    fn mint_is_idempotent_when_active_ca_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let conn = fresh_conn();
        let backend = MockBackend;
        let _ = mint_ca(&backend, &conn, "m1", Some(&crt), Some(&key)).expect("mint 1");
        // Mutate the on-disk crt to verify the second call
        // does NOT overwrite it.
        std::fs::write(&crt, "tampered").expect("tamper");
        let outcome = mint_ca(&backend, &conn, "m1", Some(&crt), Some(&key)).expect("mint 2");
        assert!(matches!(
            outcome,
            MintOutcome::AlreadyMinted { epoch: 0, .. }
        ));
        assert_eq!(std::fs::read_to_string(&crt).unwrap(), "tampered");
    }

    #[test]
    fn current_ca_returns_none_on_empty_mesh() {
        let conn = fresh_conn();
        assert!(current_ca(&conn, "never-minted").unwrap().is_none());
    }

    #[test]
    fn key_is_sealed_at_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let conn = fresh_conn();
        let backend = MockBackend;
        mint_ca(&backend, &conn, "m1", Some(&crt), Some(&key)).expect("mint");
        let mode = std::fs::metadata(&key).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn mint_rejects_an_oversized_public_certificate_before_materialization() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let conn = fresh_conn();
        let backend = HandoffBackend {
            fixture: CertificateFixture::Oversized,
        };

        let error = mint_ca(&backend, &conn, "oversized", Some(&crt), Some(&key))
            .expect_err("oversized certificate must fail closed");
        assert!(
            error.to_string().contains("exceeds"),
            "unexpected error: {error}"
        );
        assert!(current_ca(&conn, "oversized").unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn mint_rejects_an_untrusted_final_certificate_symlink() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let target = tmp.path().join("certificate-target.crt");
        let conn = fresh_conn();
        let backend = HandoffBackend {
            fixture: CertificateFixture::FinalSymlink,
        };

        let error = mint_ca(&backend, &conn, "symlinked", Some(&crt), Some(&key))
            .expect_err("untrusted final symlink must fail closed");
        assert!(
            error.to_string().contains("untrusted sealed link"),
            "unexpected error: {error}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"-----BEGIN NEBULA CA-----\nforbidden\n"
        );
        assert!(current_ca(&conn, "symlinked").unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn mint_rejects_a_non_regular_public_certificate() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let conn = fresh_conn();
        let backend = HandoffBackend {
            fixture: CertificateFixture::Directory,
        };

        let error = mint_ca(&backend, &conn, "directory", Some(&crt), Some(&key))
            .expect_err("directory certificate must fail closed");
        assert!(
            error.to_string().contains("regular file"),
            "unexpected error: {error}"
        );
        assert!(current_ca(&conn, "directory").unwrap().is_none());
    }
}

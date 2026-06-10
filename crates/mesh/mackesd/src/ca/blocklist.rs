//! ENT-3 (C2) — the data-plane revocation blocklist.
//!
//! `ca revoke` used to stop at the DB mark + ban list: the Nebula
//! data plane kept trusting the revoked cert until natural expiry.
//! This module closes that gap fleet-wide: revocation records the
//! revoked certs' **Nebula fingerprints** under
//! `<root>/ca/blocklist/<node_id>.json` on the replicated volume;
//! every peer's `nebula_supervisor` unions the entries into its
//! rendered `pki.blocklist:` and reloads nebula — so a revoked node
//! loses every tunnel, everywhere, within one supervisor tick of
//! replication, not at cert expiry.

use std::io;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// The replicated blocklist directory.
#[must_use]
pub fn blocklist_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("ca").join("blocklist")
}

/// Canonical signing payload: tamper over node_id OR the
/// fingerprint set invalidates the signature.
fn canonical_payload(node_id: &str, fingerprints: &[String]) -> Vec<u8> {
    let mut fps: Vec<&str> = fingerprints.iter().map(String::as_str).collect();
    fps.sort_unstable();
    format!("mde-retract-v1:{node_id}:{}", fps.join(",")).into_bytes()
}

/// Record `node_id`'s revoked-cert fingerprints (atomic write).
/// Unsigned legacy form — production callers use
/// [`record_revoked_signed`] (SEC-6); this stays for migration +
/// tests of the tolerant reader.
///
/// # Errors
/// IO/serialization failures.
pub fn record_revoked(
    workgroup_root: &Path,
    node_id: &str,
    fingerprints: &[String],
) -> io::Result<PathBuf> {
    write_record(workgroup_root, node_id, fingerprints, None)
}

/// SEC-6 (Q28/29) — the signed retract record: gossiped peer-to-peer
/// like fleet revisions, attributable to the revoking node's
/// persisted signing key, tamper-evident over the canonical payload.
///
/// # Errors
/// IO/serialization failures.
pub fn record_revoked_signed(
    workgroup_root: &Path,
    node_id: &str,
    fingerprints: &[String],
    signer_node: &str,
    key: &SigningKey,
) -> io::Result<PathBuf> {
    let sig = key.sign(&canonical_payload(node_id, fingerprints));
    let meta = serde_json::json!({
        "signed_by": signer_node,
        "pubkey": key
            .verifying_key()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "sig": sig
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    });
    write_record(workgroup_root, node_id, fingerprints, Some(meta))
}

fn write_record(
    workgroup_root: &Path,
    node_id: &str,
    fingerprints: &[String],
    signature: Option<serde_json::Value>,
) -> io::Result<PathBuf> {
    let dir = blocklist_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    // Node ids carry a `peer:` prefix — keep filenames flat.
    let stem = node_id.replace(':', "_");
    let path = dir.join(format!("{stem}.json"));
    let mut body = serde_json::json!({ "node_id": node_id, "fingerprints": fingerprints });
    if let Some(sig) = signature {
        body["signature"] = sig;
    }
    let tmp = dir.join(format!(".{stem}.tmp"));
    std::fs::write(&tmp, body.to_string())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn hex_to_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0_u8; N];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

/// SEC-6 — validate a record's signature when present. `true` for
/// unsigned legacy records (verify-if-present migration stance — the
/// reader warns; enforcement tightens once every writer signs) and
/// for valid signatures; `false` for PRESENT-but-invalid signatures
/// (tampered records are dropped from the union).
fn signature_acceptable(v: &serde_json::Value) -> bool {
    let Some(sig_block) = v.get("signature") else {
        return true; // legacy unsigned — accepted with a warn
    };
    let (Some(node_id), Some(fps)) = (
        v.get("node_id").and_then(|n| n.as_str()),
        v.get("fingerprints").and_then(|f| f.as_array()),
    ) else {
        return false;
    };
    let fps: Vec<String> = fps
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();
    let (Some(pub_hex), Some(sig_hex)) = (
        sig_block.get("pubkey").and_then(|p| p.as_str()),
        sig_block.get("sig").and_then(|s| s.as_str()),
    ) else {
        return false;
    };
    let (Some(pub_bytes), Some(sig_bytes)) =
        (hex_to_bytes::<32>(pub_hex), hex_to_bytes::<64>(sig_hex))
    else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pub_bytes) else {
        return false;
    };
    vk.verify(
        &canonical_payload(node_id, &fps),
        &Signature::from_bytes(&sig_bytes),
    )
    .is_ok()
}

/// Union every entry's fingerprints — sorted + deduped, tolerant of
/// junk/half-replicated files. What the config renderer emits.
#[must_use]
pub fn all_fingerprints(workgroup_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(blocklist_dir(workgroup_root)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter(|v| {
            let ok = signature_acceptable(v);
            if !ok {
                tracing::warn!(
                    node = %v.get("node_id").and_then(|n| n.as_str()).unwrap_or("?"),
                    "SEC-6: blocklist record has an INVALID signature — dropped (tamper?)",
                );
            } else if v.get("signature").is_none() {
                tracing::warn!(
                    node = %v.get("node_id").and_then(|n| n.as_str()).unwrap_or("?"),
                    "SEC-6: unsigned legacy blocklist record accepted (re-revoke to sign)",
                );
            }
            ok
        })
        .flat_map(|v| {
            v.get("fingerprints")
                .and_then(|f| f.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .filter(|fp| fp.len() == 64 && fp.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Parse `nebula-cert print -json` output for the fingerprint (pure).
#[must_use]
pub fn parse_fingerprint_json(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    v.get("fingerprint")
        .and_then(|f| f.as_str())
        .map(str::to_string)
}

/// Fingerprint a cert PEM via `nebula-cert print -json` (the only
/// authoritative source of Nebula's own fingerprint format). `None`
/// when nebula-cert is unavailable — callers warn loudly (ENT-3 is
/// security-relevant; silent failure is not acceptable there).
#[must_use]
pub fn fingerprint_cert_pem(pem: &str) -> Option<String> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("mde-fp-{}.crt", std::process::id()));
    std::fs::write(&path, pem).ok()?;
    let out = std::process::Command::new("nebula-cert")
        .args(["print", "-json", "-path"])
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);
    let out = out.ok()?;
    if !out.status.success() {
        return None;
    }
    parse_fingerprint_json(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const FP_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn entries_union_sorted_deduped_and_junk_tolerant() {
        let tmp = tempfile::tempdir().unwrap();
        record_revoked(tmp.path(), "peer:oak", &[FP_B.into(), FP_A.into()]).unwrap();
        record_revoked(tmp.path(), "peer:elm", &[FP_A.into()]).unwrap();
        std::fs::write(blocklist_dir(tmp.path()).join("junk.json"), "{{").unwrap();
        std::fs::write(
            blocklist_dir(tmp.path()).join("short.json"),
            r#"{"fingerprints":["nothex"]}"#,
        )
        .unwrap();
        assert_eq!(all_fingerprints(tmp.path()), vec![FP_A, FP_B]);
    }

    #[test]
    fn signed_records_verify_and_tampered_ones_drop_sec6() {
        let tmp = tempfile::tempdir().unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        record_revoked_signed(tmp.path(), "peer:oak", &[FP_A.into()], "peer:lh", &key).unwrap();
        assert_eq!(
            all_fingerprints(tmp.path()),
            vec![FP_A],
            "valid sig accepted"
        );
        // Tamper: swap the fingerprint set under the same signature.
        let path = blocklist_dir(tmp.path()).join("peer_oak.json");
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v["fingerprints"] = serde_json::json!([FP_B]);
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(
            all_fingerprints(tmp.path()).is_empty(),
            "a tampered signed record must be dropped"
        );
    }

    #[test]
    fn unsigned_legacy_records_stay_accepted_during_migration() {
        let tmp = tempfile::tempdir().unwrap();
        record_revoked(tmp.path(), "peer:elm", &[FP_A.into()]).unwrap();
        assert_eq!(all_fingerprints(tmp.path()), vec![FP_A]);
    }

    #[test]
    fn empty_mesh_has_an_empty_blocklist() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(all_fingerprints(tmp.path()).is_empty());
    }

    #[test]
    fn fingerprint_json_parses_the_nebula_cert_shape() {
        let raw = format!(r#"{{"details":{{"name":"oak"}},"fingerprint":"{FP_A}"}}"#);
        assert_eq!(parse_fingerprint_json(&raw).as_deref(), Some(FP_A));
        assert!(parse_fingerprint_json("junk").is_none());
    }
}

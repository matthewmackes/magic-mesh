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

/// Replicated records are small JSON envelopes. Keep hostile/corrupt entries
/// bounded before serde materializes them, while leaving room for a large
/// signed revocation set.
const MAX_BLOCKLIST_RECORD_BYTES: usize = 64 * 1024;

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
    let fingerprints = merged_fingerprints(&record_path(workgroup_root, node_id), fingerprints);
    write_record(workgroup_root, node_id, &fingerprints, None)
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
    let fingerprints = merged_fingerprints(&record_path(workgroup_root, node_id), fingerprints);
    let sig = key.sign(&canonical_payload(node_id, &fingerprints));
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
    write_record(workgroup_root, node_id, &fingerprints, Some(meta))
}

fn record_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    blocklist_dir(workgroup_root).join(format!("{}.json", node_id.replace(':', "_")))
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
    let path = record_path(workgroup_root, node_id);
    let stem = node_id.replace(':', "_");
    let mut body = serde_json::json!({ "node_id": node_id, "fingerprints": fingerprints });
    if let Some(sig) = signature {
        body["signature"] = sig;
    }
    let tmp = dir.join(format!(".{stem}.tmp"));
    std::fs::write(&tmp, body.to_string())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn merged_fingerprints(path: &Path, fingerprints: &[String]) -> Vec<String> {
    let mut merged = fingerprints.to_vec();
    if let Some(raw) = read_blocklist_record(path) {
        if let Ok(existing) = serde_json::from_str::<serde_json::Value>(&raw) {
            if signature_acceptable(&existing) {
                merged.extend(
                    existing
                        .get("fingerprints")
                        .and_then(|value| value.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|value| value.as_str().map(str::to_string)),
                );
            }
        }
    }
    merged.retain(|fp| fp.len() == 64 && fp.bytes().all(|b| b.is_ascii_hexdigit()));
    merged.sort();
    merged.dedup();
    merged
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

/// Read one replicated record through a descriptor whose final path component
/// cannot be a symlink. The caller intentionally treats every error as a
/// dropped entry: blocklist replication is fail-soft for malformed or
/// half-written records, but must not turn an attacker-controlled file into an
/// unbounded allocation or a blocking read.
fn read_blocklist_record(path: &Path) -> Option<String> {
    use std::io::Read as _;

    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags};

        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?;
        std::fs::File::from(fd)
    };

    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if metadata.file_type().is_symlink() {
            return None;
        }
        std::fs::File::open(path).ok()?
    };

    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_BLOCKLIST_RECORD_BYTES as u64 {
        return None;
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_BLOCKLIST_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_BLOCKLIST_RECORD_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
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
        .filter_map(|e| read_blocklist_record(&e.path()))
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
///
/// Handles both cert formats: a **Nebula Certificate V1** prints a single JSON
/// object `{"fingerprint": …}`, while a **V2** cert (`-----BEGIN NEBULA
/// CERTIFICATE V2-----`) prints a JSON *array* of certs
/// `[{"fingerprint": …, "details": …}]`. Accept the object directly, or the
/// first element of the array — otherwise a V2 mesh reads every host cert as
/// absent (found live 2026-07-01: the self-test cert probe + the `leave`
/// revocation-eviction both silently no-op'd on the V2 fleet).
#[must_use]
pub fn parse_fingerprint_json(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let cert = if v.is_array() { v.get(0)? } else { &v };
    cert.get("fingerprint")
        .and_then(|f| f.as_str())
        .map(str::to_string)
}

/// Public identity facts printed by the authoritative `nebula-cert` parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NebulaPublicIdentity {
    /// Exact certificate name.
    pub(crate) name: String,
    /// Canonical address from the certificate's sole `/17` overlay network.
    pub(crate) address: String,
    /// Nebula's lowercase, bare SHA-256 certificate fingerprint.
    pub(crate) fingerprint: String,
}

/// Parse one V1 or V2 `nebula-cert print -json` document into the public facts
/// needed by the live overlay claimant. Multiple certificates, multiple
/// networks, non-canonical IPv4, a non-`/17` network, and malformed or
/// placeholder fingerprints fail closed.
#[must_use]
pub(crate) fn parse_public_identity_json(raw: &str) -> Option<NebulaPublicIdentity> {
    let value: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let certificate = match value {
        serde_json::Value::Array(ref certificates) if certificates.len() == 1 => {
            certificates.first()?
        }
        serde_json::Value::Object(_) => &value,
        _ => return None,
    };
    let details = certificate.get("details")?.as_object()?;
    let name = details.get("name")?.as_str()?;
    let networks = details
        .get("ips")
        .or_else(|| details.get("networks"))?
        .as_array()?;
    let [network] = networks.as_slice() else {
        return None;
    };
    let network = network.as_str()?;
    let (address_text, prefix) = network.split_once('/')?;
    if prefix != "17" {
        return None;
    }
    let address = address_text.parse::<std::net::Ipv4Addr>().ok()?;
    if address.to_string() != address_text {
        return None;
    }
    let fingerprint = certificate.get("fingerprint")?.as_str()?;
    if fingerprint.len() != 64
        || fingerprint.bytes().all(|byte| byte == b'0')
        || !fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return None;
    }
    Some(NebulaPublicIdentity {
        name: name.to_string(),
        address: address.to_string(),
        fingerprint: fingerprint.to_string(),
    })
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
    let mut command = std::process::Command::new("nebula-cert");
    command.args(["print", "-json", "-path"]).arg(&path);
    crate::lifecycle_child_env::strip_lifecycle_child_env(&mut command);
    let out = command.output();
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
    fn repeated_records_for_one_node_append_instead_of_unblocking_old_fingerprints() {
        let tmp = tempfile::tempdir().unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&[9_u8; 32]);
        record_revoked_signed(tmp.path(), "peer:oak", &[FP_A.into()], "peer:lh", &key).unwrap();
        record_revoked_signed(tmp.path(), "peer:oak", &[FP_B.into()], "peer:lh", &key).unwrap();

        assert_eq!(all_fingerprints(tmp.path()), vec![FP_A, FP_B]);
    }

    #[test]
    fn oversized_records_are_dropped_before_json_parsing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = blocklist_dir(tmp.path()).join("oversized.json");
        std::fs::create_dir_all(blocklist_dir(tmp.path())).unwrap();
        std::fs::write(&path, vec![b'x'; MAX_BLOCKLIST_RECORD_BYTES + 1]).unwrap();

        assert!(all_fingerprints(tmp.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_records_are_dropped_without_following_them() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.json");
        record_revoked(tmp.path(), "peer:target", &[FP_A.into()]).unwrap();
        std::fs::rename(blocklist_dir(tmp.path()).join("peer_target.json"), &target).unwrap();
        symlink(&target, blocklist_dir(tmp.path()).join("linked.json")).unwrap();

        assert!(all_fingerprints(tmp.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_records_are_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(blocklist_dir(tmp.path()).join("directory.json")).unwrap();

        assert!(all_fingerprints(tmp.path()).is_empty());
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

    #[test]
    fn fingerprint_json_parses_real_nebula_cert_1_9_output() {
        // ENT-3 verified against the real binary: this is verbatim
        // `nebula-cert print -json` from nebula 1.9.7 on a Fedora 42
        // VM (the mesh test bed). Locks the wire shape so a nebula
        // upgrade that moves the field is caught here, not in prod.
        let real = r#"{"details":{"curve":"CURVE25519","groups":[],"ips":["10.42.0.2/16"],"isCa":false,"issuer":"74a7736f35a00f55600a3c35f5974d7677f29216d639216693d9d48be79eec98","name":"pine","notAfter":"2027-06-10T13:18:18Z","notBefore":"2026-06-10T13:18:19Z","publicKey":"ba3416abbc426713473fcd0e712a2b77fa1f88e060e5a9a4b03056ab792f7273","subnets":[]},"fingerprint":"ac130a6002d11b21a83e12408615f81d06643f911ada972c24f3aaaaaaaaaaaa"}"#;
        let fp = parse_fingerprint_json(real).expect("real nebula-cert json parses");
        assert_eq!(
            fp.len(),
            64,
            "the real fingerprint is 64-hex (blocklist-valid)"
        );
        assert!(fp.starts_with("ac130a60"));
    }

    #[test]
    fn fingerprint_json_parses_v2_array_output() {
        // Nebula Certificate V2 (`-----BEGIN NEBULA CERTIFICATE V2-----`):
        // `nebula-cert print -json` emits a JSON ARRAY of certs, not a single
        // object. Verbatim-shaped from the live magic-mesh fleet (2026-07-01) —
        // before the array-form fix every V2 host cert read as absent, so the
        // OW-10 self-test cert probe false-FAILED and `leave` eviction silently
        // no-op'd in prod. This test locks the V2 wire shape.
        let v2 = format!(
            r#"[{{"curve":"CURVE25519","details":{{"groups":["role:host"],"isCa":false,"name":"peer:lh-magic-mesh"}},"fingerprint":"{FP_A}","publicKey":"abc"}}]"#
        );
        assert_eq!(parse_fingerprint_json(&v2).as_deref(), Some(FP_A));
        assert!(parse_fingerprint_json("[]").is_none()); // empty array ⇒ no cert
    }
}

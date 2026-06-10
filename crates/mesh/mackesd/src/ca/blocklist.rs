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

/// The replicated blocklist directory.
#[must_use]
pub fn blocklist_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("ca").join("blocklist")
}

/// Record `node_id`'s revoked-cert fingerprints (atomic write).
///
/// # Errors
/// IO/serialization failures.
pub fn record_revoked(
    workgroup_root: &Path,
    node_id: &str,
    fingerprints: &[String],
) -> io::Result<PathBuf> {
    let dir = blocklist_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    // Node ids carry a `peer:` prefix — keep filenames flat.
    let stem = node_id.replace(':', "_");
    let path = dir.join(format!("{stem}.json"));
    let body = serde_json::json!({ "node_id": node_id, "fingerprints": fingerprints });
    let tmp = dir.join(format!(".{stem}.tmp"));
    std::fs::write(&tmp, body.to_string())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
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

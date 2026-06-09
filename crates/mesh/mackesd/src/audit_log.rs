//! MESH-A-9 (v5.0.0) — audit log of network-state changes.
//!
//! Streams network-state changes (a host blocked/trusted, an ARP-spoof
//! or rogue-DHCP detection, a firewall DROP applied, …) into the
//! activity-as-files store as `kind="audit"` entries (R8-Q80), at the
//! Portal-33 path convention
//! `<XDG_DATA_HOME>/mde/activity/audit/<iso>-<hash>.json`. The full
//! Portal-33 activity subsystem (per-type retention, read/star/pin
//! state, 5-action menu) surfaces these later; [`AuditEntry`] is the
//! forward-compatible minimal record (extra fields a future schema
//! adds are tolerated on read).
//!
//! The defense detectors (MESH-A-6.1..6.5) + the trust/firewall actions
//! (A-4.d / A-5) feed [`write_audit_event`] as a follow-on; this ships
//! the writer + the `mackesd audit-log` CLI that exercises it.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// One audit-log entry — a network-state change, written as a
/// `kind="audit"` activity record (R8-Q80).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditEntry {
    /// Unix-epoch ms the event occurred.
    pub ts_ms: i64,
    /// Activity kind tag — always `"audit"` (distinguishes these from
    /// regular alerts/logins in the same activity surface).
    pub kind: String,
    /// Short event identifier (e.g. `host-blocked`, `arp-spoof-detected`).
    pub event: String,
    /// Free-form context (the IP/MAC involved, the operator, etc.).
    #[serde(default)]
    pub detail: String,
}

/// Content-addressed activity filename: `<iso>-<sha256[..16]>.json`,
/// colon-free (mirrors the netassess snapshot convention). Pure.
#[must_use]
pub fn activity_filename(iso8601: &str, json_body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(json_body.as_bytes());
    let hash = hasher.finalize();
    let short: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{}-{}.json", iso8601.replace(':', ""), short)
}

/// Write a `kind="audit"` network-state-change entry to
/// `<activity_root>/audit/<iso>-<hash>.json` (R8-Q80). Returns the
/// written path.
///
/// # Errors
///
/// I/O errors creating the `audit/` dir or writing the file.
pub fn write_audit_event(
    activity_root: &Path,
    event: &str,
    detail: &str,
) -> std::io::Result<PathBuf> {
    let now = chrono::Local::now();
    let entry = AuditEntry {
        ts_ms: now.timestamp_millis(),
        kind: "audit".to_string(),
        event: event.to_string(),
        detail: detail.to_string(),
    };
    let body = serde_json::to_string_pretty(&entry).unwrap_or_default();
    let dir = activity_root.join("audit");
    std::fs::create_dir_all(&dir)?;
    let iso = now.format("%Y%m%dT%H%M%S").to_string();
    let path = dir.join(activity_filename(&iso, &body));
    std::fs::write(&path, &body)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_filename_is_colon_free_with_hash() {
        let name = activity_filename("20260531T093000", "{\"x\":1}");
        assert!(name.ends_with(".json"));
        assert!(!name.contains(':'));
        assert!(name.starts_with("20260531T093000-"));
    }

    #[test]
    fn write_audit_event_writes_kind_audit_record() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let path = write_audit_event(root, "host-blocked", "10.0.0.66 by operator").unwrap();
        assert!(
            path.starts_with(root.join("audit")),
            "under the audit/ type dir"
        );
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.ends_with(".json") && !name.contains(':'));
        let entry: AuditEntry =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(entry.kind, "audit");
        assert_eq!(entry.event, "host-blocked");
        assert_eq!(entry.detail, "10.0.0.66 by operator");
        assert!(entry.ts_ms > 0);
    }
}

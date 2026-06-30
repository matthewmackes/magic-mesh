//! MESHFS-3 — minimal read-only Syncthing REST client for Mesh-Sync health.
//!
//! No new dependency: the API key is string-scanned out of the provisioned
//! `config.xml` (`/var/lib/mcnf-syncthing/config.xml`, folder id `mcnf-mesh`,
//! GUI on `:8384` — see `install-helpers/setup-syncthing.sh`) and the completion
//! query shells `curl`. Everything is **best-effort**: an absent config / key /
//! daemon degrades to `reachable: false` — never a panic, never a faked 100 %.
//!
//! Live verification is gated on a real Syncthing node; the pure parsers
//! (`parse_apikey` / `parse_gui_address` / `parse_completion`) are unit-tested
//! here so the wiring is provable without one.

use std::path::Path;

/// Canonical Syncthing config + folder, matching `setup-syncthing.sh`.
pub const CONFIG_PATH: &str = "/var/lib/mcnf-syncthing/config.xml";
/// The Mesh-Sync shared folder id (`--folder-id` default).
pub const FOLDER_ID: &str = "mcnf-mesh";

/// Scan the `<apikey>…</apikey>` out of a Syncthing `config.xml` body. Syncthing
/// writes exactly one apikey under `<gui>`. Pure + testable.
#[must_use]
pub fn parse_apikey(config_xml: &str) -> Option<String> {
    let start = config_xml.find("<apikey>")? + "<apikey>".len();
    let end = config_xml[start..].find("</apikey>")? + start;
    let key = config_xml[start..end].trim();
    (!key.is_empty()).then(|| key.to_string())
}

/// Scan the GUI `<address>…</address>` (the REST endpoint). Falls back to the
/// `setup-syncthing.sh` default `127.0.0.1:8384` when absent.
#[must_use]
pub fn parse_gui_address(config_xml: &str) -> String {
    config_xml
        .find("<address>")
        .and_then(|i| {
            let s = i + "<address>".len();
            config_xml[s..]
                .find("</address>")
                .map(|e| config_xml[s..s + e].trim().to_string())
        })
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| "127.0.0.1:8384".to_string())
}

/// Parse the `completion` percent out of `/rest/db/completion`'s JSON body.
#[must_use]
pub fn parse_completion(body: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("completion").and_then(serde_json::Value::as_f64)
}

/// Mesh-Sync replication health derived from Syncthing's REST API.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SyncHealth {
    /// Syncthing's REST API responded (daemon up + key valid).
    pub reachable: bool,
    /// Folder completion percent across the mesh (100 = fully replicated).
    pub completion_pct: f64,
}

/// Query the Mesh-Sync folder's completion via the local Syncthing REST API.
/// Best-effort — returns `SyncHealth::default()` (unreachable) when the config,
/// key, or daemon is absent.
#[must_use]
pub fn folder_health() -> SyncHealth {
    folder_health_at(Path::new(CONFIG_PATH))
}

/// [`folder_health`] against an explicit config path (test seam).
#[must_use]
pub fn folder_health_at(config_path: &Path) -> SyncHealth {
    let Ok(config_xml) = std::fs::read_to_string(config_path) else {
        return SyncHealth::default();
    };
    let Some(apikey) = parse_apikey(&config_xml) else {
        return SyncHealth::default();
    };
    let addr = parse_gui_address(&config_xml);
    let url = format!("http://{addr}/rest/db/completion?folder={FOLDER_ID}");
    let out = std::process::Command::new("curl")
        .args(["-s", "--max-time", "3", "-H"])
        .arg(format!("X-API-Key: {apikey}"))
        .arg(&url)
        .output();
    let Ok(out) = out else {
        return SyncHealth::default();
    };
    if !out.status.success() {
        return SyncHealth::default();
    }
    match parse_completion(&String::from_utf8_lossy(&out.stdout)) {
        Some(completion_pct) => SyncHealth {
            reachable: true,
            completion_pct,
        },
        None => SyncHealth::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_apikey_extracts_the_key() {
        let xml = r#"<configuration><gui><apikey>abc123XYZ</apikey><address>0.0.0.0:8384</address></gui></configuration>"#;
        assert_eq!(parse_apikey(xml).as_deref(), Some("abc123XYZ"));
    }

    #[test]
    fn parse_apikey_none_when_absent_or_empty() {
        assert!(parse_apikey("<configuration></configuration>").is_none());
        assert!(parse_apikey("<gui><apikey></apikey></gui>").is_none());
    }

    #[test]
    fn parse_gui_address_reads_it_or_defaults() {
        assert_eq!(
            parse_gui_address(r#"<gui><address>10.0.0.1:9999</address></gui>"#),
            "10.0.0.1:9999"
        );
        assert_eq!(parse_gui_address("<gui></gui>"), "127.0.0.1:8384");
    }

    #[test]
    fn parse_completion_reads_the_field() {
        assert_eq!(
            parse_completion(r#"{"completion": 87.5, "needBytes": 100}"#),
            Some(87.5)
        );
        assert_eq!(parse_completion(r#"{"completion": 100}"#), Some(100.0));
        assert!(parse_completion("not json").is_none());
    }

    #[test]
    fn folder_health_unreachable_when_config_absent() {
        let h = folder_health_at(Path::new("/no-such-syncthing-config-zzz.xml"));
        assert!(!h.reachable);
        assert_eq!(h.completion_pct, 0.0);
    }
}

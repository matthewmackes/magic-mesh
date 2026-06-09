//! Recents tab — reads call activity from the v6.0 activity-as-files
//! directory.
//!
//! Per [[project_v6_0_mde_portal]] R10-Q10 and the platform's
//! activity-as-files pattern, every call lands as a JSON file at
//! `~/.local/share/mde/activity/calls/<iso8601>-<hash>.json`. The
//! HUD's Recents tab reads the most-recent N entries at view time;
//! it does NOT watch the directory (the file count is small enough
//! that a per-render scan is cheap, and the operator only sees the
//! tab when explicitly switching to it).
//!
//! Empty-state placeholder renders when the directory is missing
//! or contains zero entries — required by VOIP-27 acceptance.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Maximum number of recents to surface. Matches the design
/// bundle's `RECENTS` array length (6 rows) so the visual budget
/// of the Recents tab stays consistent.
pub const RECENTS_LIMIT: usize = 12;

/// One call activity entry — JSON shape mded writes when a call
/// concludes (success or failure).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecentCall {
    /// `"in"` (incoming) or `"out"` (outgoing).
    pub dir: String,
    /// Display name (peer hostname for mesh, E.164 for PSTN).
    pub name: String,
    /// Dialed target (e.g., `"1003"` or `"915558675309"`).
    pub target: String,
    /// Human-friendly "when" string already formatted by mded
    /// ("12 min ago", "Mon 14:02"). Avoids re-parsing iso8601 here.
    pub when: String,
    /// Call duration as `"M:SS"` (or `"—"` for failures).
    #[serde(default = "default_duration")]
    pub dur: String,
    /// Did the call succeed?
    pub ok: bool,
    /// `true` for PSTN-via-Vitelity calls.
    #[serde(default)]
    pub pstn: bool,
    /// SIP failure reason when `ok=false` (e.g., `"503 Vitelity unreachable"`).
    #[serde(default)]
    pub fail: String,
    /// Transit-peer extension if the call went via VOIP-4 operator
    /// override (e.g., `"1004"`). Empty when direct.
    #[serde(default)]
    pub transit: String,
}

fn default_duration() -> String {
    "—".to_string()
}

/// Try to load up to `RECENTS_LIMIT` most-recent activity entries.
///
/// Sort key is filename — the activity-as-files pattern uses
/// `<iso8601>-<hash>.json` so lexicographic sort = chronological.
/// Returned vec is newest-first.
#[must_use]
pub fn load() -> Vec<RecentCall> {
    let Some(dir) = activity_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    paths.truncate(RECENTS_LIMIT);

    paths
        .into_iter()
        .filter_map(|p| {
            let body = fs::read_to_string(&p).ok()?;
            match serde_json::from_str::<RecentCall>(&body) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "skipping unparseable recent");
                    None
                }
            }
        })
        .collect()
}

/// Record an inbound call to the activity-as-files log (VOIP-28 slice 4).
/// Best-effort — a write failure is logged, never fatal. The filename is a
/// zero-padded epoch-nanos prefix so the lexicographic `load()` sort stays
/// chronological.
pub fn record_incoming(from: &str) {
    write_entry(&RecentCall {
        dir: "in".to_string(),
        name: from.to_string(),
        target: from.to_string(),
        when: "just now".to_string(),
        dur: default_duration(),
        ok: true,
        pstn: false,
        fail: String::new(),
        transit: String::new(),
    });
}

fn write_entry(call: &RecentCall) {
    let Some(dir) = activity_dir() else {
        return;
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, "voice-hud: cannot create activity dir");
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("{nanos:030}-call.json"));
    match serde_json::to_string(call) {
        Ok(json) => {
            if let Err(e) = fs::write(&path, json) {
                tracing::warn!(error = %e, "voice-hud: cannot write recent");
            }
        }
        Err(e) => tracing::warn!(error = %e, "voice-hud: cannot serialize recent"),
    }
}

/// `~/.local/share/mde/activity/calls/`.
fn activity_dir() -> Option<PathBuf> {
    let data = dirs::data_local_dir()?;
    Some(data.join("mde/activity/calls"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_returns_empty() {
        std::env::set_var(
            "XDG_DATA_HOME",
            "/tmp/mde-voice-hud-test-no-activity-dir-xyz123",
        );
        let recents = load();
        assert!(recents.is_empty());
    }

    #[test]
    fn json_shape_round_trips() {
        let sample = r#"{
            "dir": "out",
            "name": "bob-tower",
            "target": "1002",
            "when": "12 min ago",
            "dur": "8:42",
            "ok": true
        }"#;
        let r: RecentCall = serde_json::from_str(sample).expect("parses");
        assert_eq!(r.dir, "out");
        assert_eq!(r.target, "1002");
        assert!(r.ok);
        assert!(!r.pstn);
        assert!(r.fail.is_empty());
        assert!(r.transit.is_empty());
    }

    #[test]
    fn pstn_failure_shape_parses() {
        let sample = r#"{
            "dir": "out",
            "name": "+1 555 867 5309",
            "target": "915558675309",
            "when": "Yesterday",
            "dur": "—",
            "ok": false,
            "pstn": true,
            "fail": "503 Vitelity unreachable"
        }"#;
        let r: RecentCall = serde_json::from_str(sample).expect("parses");
        assert!(!r.ok);
        assert!(r.pstn);
        assert_eq!(r.fail, "503 Vitelity unreachable");
    }
}

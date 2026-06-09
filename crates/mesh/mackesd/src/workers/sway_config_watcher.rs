//! SWAY-8 (Q52–Q54) — sway config watcher + EDID hardware overlay.
//!
//! **Q53 — hardware overlay:** at startup and after every detected
//! config change the worker calls `swaymsg -t get_outputs` and writes
//! `~/.config/sway/config.d/00-hardware.conf` with per-monitor `output`
//! directives keyed by identity (`"<make> <model> <serial>"` when EDID
//! fields are present, port name as fallback). This file is included
//! automatically via the `include ~/.config/sway/config.d/*.conf`
//! directive already present in the platform sway template.
//!
//! **Q54 — live reload:** polls three paths every POLL_INTERVAL:
//!
//! - `~/.config/sway/config` — main user config.
//! - `~/.config/sway/config.d/` — operator overrides + MDE-generated
//!   fragments (including the 00-hardware overlay written by this worker).
//! - `~/.local/share/mde/mesh-storage/sway/` — GFS-replicated shared
//!   config (Q52, set up by `apply_sway_mesh_config_link` in birthright).
//!
//! When any watched mtime changes `swaymsg reload` is fired so the
//! updated config takes effect without a compositor restart.
//!
//! Degrades gracefully: when sway is not running (no `$SWAYSOCK`, or
//! `swaymsg` exits non-zero) the worker logs a warning and continues
//! polling. When the config directories do not exist (not yet seeded by
//! birthright) the worker also waits quietly.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{ShutdownToken, Worker};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const HARDWARE_CONF_NAME: &str = "00-hardware.conf";

// ── Worker struct ────────────────────────────────────────────────────────

/// Long-running worker: writes the EDID hardware overlay at startup then
/// polls for config-file changes and fires `swaymsg reload` on each.
pub struct SwayConfigWatcherWorker {
    /// Last observed mtime per absolute file path.
    last_mtimes: HashMap<PathBuf, SystemTime>,
}

impl SwayConfigWatcherWorker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_mtimes: HashMap::new(),
        }
    }
}

impl Default for SwayConfigWatcherWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for SwayConfigWatcherWorker {
    fn name(&self) -> &'static str {
        "sway_config_watcher"
    }

    async fn run(&mut self, shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Write the initial hardware overlay.  Silently skips when sway
        // is not yet running (mded can start before sway is fully up).
        write_hardware_overlay_async().await;

        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            if shutdown.is_shutdown() {
                return Ok(());
            }

            let changed = self.scan_for_changes();
            if changed {
                tracing::info!("sway_config_watcher: config changed; reloading sway");
                // Refresh the hardware overlay so a newly-connected
                // display is picked up without requiring re-login.
                write_hardware_overlay_async().await;
                swaymsg_reload().await;
            }
        }
    }
}

// ── Mtime scanning ───────────────────────────────────────────────────────

impl SwayConfigWatcherWorker {
    /// Walk the three watched locations, compare mtimes against
    /// `last_mtimes`, update the map, and return `true` if anything
    /// changed.
    fn scan_for_changes(&mut self) -> bool {
        let mut any_changed = false;

        let watch_paths = watched_paths();
        let mut current_mtimes: HashMap<PathBuf, SystemTime> = HashMap::new();

        for path in &watch_paths {
            collect_mtimes(path, &mut current_mtimes);
        }

        // Detect changes: new files, modified files.
        for (path, mtime) in &current_mtimes {
            match self.last_mtimes.get(path) {
                None => {
                    tracing::debug!(path = %path.display(), "sway_config_watcher: new file detected");
                    any_changed = true;
                }
                Some(&prev) if *mtime != prev => {
                    tracing::debug!(path = %path.display(), "sway_config_watcher: file modified");
                    any_changed = true;
                }
                _ => {}
            }
        }

        // Detect removals.
        for path in self.last_mtimes.keys() {
            if !current_mtimes.contains_key(path) {
                tracing::debug!(path = %path.display(), "sway_config_watcher: file removed");
                any_changed = true;
            }
        }

        self.last_mtimes = current_mtimes;
        any_changed
    }
}

// ── Pure helpers (testable) ──────────────────────────────────────────────

/// Paths to watch. Returns the canonical XDG locations; tests can
/// substitute their own via the lower-level helpers.
#[must_use]
pub fn watched_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(cfg) = dirs::config_dir() {
        let sway = cfg.join("sway");
        paths.push(sway.join("config"));
        paths.push(sway.join("config.d"));
    }
    if let Some(data) = dirs::data_local_dir() {
        paths.push(data.join("mde").join("mesh-storage").join("sway"));
    }
    paths
}

/// Collect mtimes from `path`. If it's a regular file, record its mtime.
/// If it's a directory, record the mtime of every `.conf` file within
/// (non-recursive; the config.d layout is flat).
pub fn collect_mtimes(path: &Path, map: &mut HashMap<PathBuf, SystemTime>) {
    match path.metadata() {
        Ok(m) if m.is_file() => {
            if let Ok(mtime) = m.modified() {
                map.insert(path.to_owned(), mtime);
            }
        }
        Ok(m) if m.is_dir() => {
            let Ok(dir) = std::fs::read_dir(path) else {
                return;
            };
            for entry in dir.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("conf") {
                    if let Ok(fm) = p.metadata() {
                        if let Ok(mtime) = fm.modified() {
                            map.insert(p, mtime);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

// ── Hardware overlay generation (Q53) ────────────────────────────────────

/// Generate `output` directives from `swaymsg -t get_outputs` JSON.
///
/// Each active output produces one `output <id> scale <scale>` line
/// where `<id>` is the EDID identity (`"<make> <model> <serial>"`) when
/// all three fields are non-empty, falling back to the port name.
///
/// Inactive outputs (lid-closed, DPMS-off) are included with their last
/// known scale so sway retains the setting across reconnects.
#[must_use]
pub fn generate_hardware_overlay(outputs_json: &str) -> String {
    let mut lines = vec![
        "# MDE per-peer hardware overlay — generated by mded (SWAY-8/Q53).".to_owned(),
        "# DO NOT EDIT — regenerated automatically on config change.".to_owned(),
        String::new(),
    ];

    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(outputs_json) else {
        return lines.join("\n") + "\n";
    };
    let Some(arr) = parsed.as_array() else {
        return lines.join("\n") + "\n";
    };

    for output in arr {
        let Some(name) = output
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };

        // Build EDID-based identifier when all three fields are present.
        let make = output
            .get("make")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let model = output
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let serial = output
            .get("serial")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let identifier = if !make.is_empty() && !model.is_empty() && !serial.is_empty() {
            format!("\"{make} {model} {serial}\"")
        } else {
            name.to_owned()
        };

        let scale = output.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);

        // Round scale to 2 decimal places so tiny floats (1.000001)
        // don't produce spurious reloads on the next poll.
        let scale_str = if (scale - scale.round()).abs() < 0.01 {
            format!("{}", scale.round() as i64)
        } else {
            format!("{scale:.2}")
        };

        lines.push(format!("output {identifier} scale {scale_str}"));
    }

    if lines.len() == 3 {
        // No outputs parsed — return header only.
        return lines.join("\n") + "\n";
    }

    lines.join("\n") + "\n"
}

/// Path for the hardware overlay config fragment.
#[must_use]
pub fn hardware_overlay_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("sway")
            .join("config.d")
            .join(HARDWARE_CONF_NAME),
    )
}

/// Call `swaymsg -t get_outputs`, parse, generate, and atomically write
/// the hardware overlay. Logs warnings on failure; never propagates
/// errors to the caller (the overlay is best-effort).
pub async fn write_hardware_overlay_async() {
    let json = match tokio::process::Command::new("swaymsg")
        .args(["-t", "get_outputs"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => match String::from_utf8(out.stdout) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(error = %e, "sway_config_watcher: get_outputs output not UTF-8");
                return;
            }
        },
        Ok(out) => {
            tracing::debug!(
                code = ?out.status.code(),
                "sway_config_watcher: swaymsg get_outputs failed (sway not running?)"
            );
            return;
        }
        Err(e) => {
            tracing::debug!(error = %e, "sway_config_watcher: swaymsg not available");
            return;
        }
    };

    let content = generate_hardware_overlay(&json);
    let Some(path) = hardware_overlay_path() else {
        return;
    };

    // Skip write if content is unchanged (avoids a spurious reload cycle).
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing == content {
        return;
    }

    if let Err(e) = write_conf_atomic(&path, &content) {
        tracing::warn!(error = %e, "sway_config_watcher: failed to write hardware overlay");
    } else {
        tracing::info!(path = %path.display(), "sway_config_watcher: hardware overlay updated");
    }
}

/// Call `swaymsg reload`. Logs on failure; does not propagate errors.
pub async fn swaymsg_reload() {
    match tokio::process::Command::new("swaymsg")
        .arg("reload")
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            tracing::info!("sway_config_watcher: swaymsg reload succeeded");
        }
        Ok(out) => {
            tracing::warn!(
                code = ?out.status.code(),
                stderr = %String::from_utf8_lossy(&out.stderr),
                "sway_config_watcher: swaymsg reload non-zero exit"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "sway_config_watcher: swaymsg reload spawn failed");
        }
    }
}

/// Write `content` to `path` atomically (via a sibling `.tmp` rename).
/// Creates parent directories as needed.
pub fn write_conf_atomic(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("conf.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── generate_hardware_overlay ────────────────────────────────────────

    #[test]
    fn overlay_header_on_empty_json() {
        let out = generate_hardware_overlay("[]");
        assert!(out.starts_with("# MDE per-peer hardware overlay"));
        assert!(!out.contains("output "));
    }

    #[test]
    fn overlay_header_on_malformed_json() {
        let out = generate_hardware_overlay("not json");
        assert!(out.starts_with("# MDE per-peer hardware overlay"));
    }

    #[test]
    fn overlay_uses_edid_identifier_when_all_fields_present() {
        let json = r#"[{
            "name": "DP-3",
            "make": "Dell Inc.",
            "model": "U2720Q",
            "serial": "ABC123",
            "scale": 2.0,
            "active": true
        }]"#;
        let out = generate_hardware_overlay(json);
        assert!(out.contains(r#"output "Dell Inc. U2720Q ABC123" scale 2"#));
        assert!(!out.contains("output DP-3"));
    }

    #[test]
    fn overlay_falls_back_to_port_name_when_edid_incomplete() {
        let json = r#"[{
            "name": "DP-3",
            "make": "Dell Inc.",
            "model": "",
            "serial": "ABC123",
            "scale": 1.0,
            "active": true
        }]"#;
        let out = generate_hardware_overlay(json);
        assert!(out.contains("output DP-3 scale 1"));
    }

    #[test]
    fn overlay_skips_output_with_no_name() {
        let json = r#"[{"name": "", "scale": 1.0}]"#;
        let out = generate_hardware_overlay(json);
        assert!(!out.contains("output  "));
    }

    #[test]
    fn overlay_rounds_integer_scale() {
        let json = r#"[{"name":"eDP-1","make":"","model":"","serial":"","scale":2.0}]"#;
        let out = generate_hardware_overlay(json);
        assert!(out.contains("scale 2"), "Expected integer, got: {out}");
        assert!(!out.contains("scale 2.00"));
    }

    #[test]
    fn overlay_keeps_fractional_scale() {
        let json = r#"[{"name":"eDP-1","make":"","model":"","serial":"","scale":1.25}]"#;
        let out = generate_hardware_overlay(json);
        assert!(out.contains("scale 1.25"), "got: {out}");
    }

    #[test]
    fn overlay_multiple_outputs() {
        let json = r#"[
            {"name":"DP-1","make":"A","model":"B","serial":"C","scale":2.0},
            {"name":"eDP-1","make":"","model":"","serial":"","scale":1.0}
        ]"#;
        let out = generate_hardware_overlay(json);
        assert!(out.contains(r#"output "A B C" scale 2"#));
        assert!(out.contains("output eDP-1 scale 1"));
    }

    #[test]
    fn overlay_missing_scale_defaults_to_one() {
        let json = r#"[{"name":"eDP-1","make":"","model":"","serial":""}]"#;
        let out = generate_hardware_overlay(json);
        assert!(out.contains("scale 1"));
    }

    // ── collect_mtimes ───────────────────────────────────────────────────

    #[test]
    fn collect_mtimes_empty_dir_produces_no_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HashMap::new();
        collect_mtimes(dir.path(), &mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn collect_mtimes_only_picks_up_conf_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foo.conf"), "# conf").unwrap();
        std::fs::write(dir.path().join("bar.toml"), "# toml").unwrap();
        std::fs::write(dir.path().join("baz"), "# no ext").unwrap();
        let mut map = HashMap::new();
        collect_mtimes(dir.path(), &mut map);
        assert_eq!(map.len(), 1);
        assert!(map.keys().any(|p| p.file_name().unwrap() == "foo.conf"));
    }

    #[test]
    fn collect_mtimes_missing_path_produces_no_entries() {
        let mut map = HashMap::new();
        collect_mtimes(Path::new("/nonexistent/totally/missing"), &mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn collect_mtimes_regular_file_records_its_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sway.conf");
        std::fs::write(&f, "# main config").unwrap();
        let mut map = HashMap::new();
        collect_mtimes(&f, &mut map);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&f));
    }

    // ── write_conf_atomic ────────────────────────────────────────────────

    #[test]
    fn write_conf_atomic_round_trips_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("00-hardware.conf");
        write_conf_atomic(&path, "# test\noutput eDP-1 scale 1\n").unwrap();
        let read = std::fs::read_to_string(&path).unwrap();
        assert_eq!(read, "# test\noutput eDP-1 scale 1\n");
        assert!(!path.with_extension("conf.tmp").exists());
    }

    #[test]
    fn write_conf_atomic_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("sway")
            .join("config.d")
            .join("00-hardware.conf");
        write_conf_atomic(&path, "# test\n").unwrap();
        assert!(path.exists());
    }

    // ── scan_for_changes (integration) ──────────────────────────────────

    #[test]
    fn scan_detects_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("50-test.conf");

        let mut worker = SwayConfigWatcherWorker::new();
        // First scan — empty dir — no change.
        let mut map1 = HashMap::new();
        collect_mtimes(dir.path(), &mut map1);
        worker.last_mtimes = map1;

        // Write a new file.
        std::fs::write(&conf, "# added").unwrap();

        let changed = worker.scan_for_changes_in(dir.path());
        assert!(changed, "new file should be detected");
    }

    #[test]
    fn scan_detects_removed_file() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("10-test.conf");
        std::fs::write(&conf, "# present").unwrap();

        let mut worker = SwayConfigWatcherWorker::new();
        let mut map1 = HashMap::new();
        collect_mtimes(dir.path(), &mut map1);
        worker.last_mtimes = map1;

        // Remove the file.
        std::fs::remove_file(&conf).unwrap();

        let changed = worker.scan_for_changes_in(dir.path());
        assert!(changed, "removed file should be detected");
    }

    #[test]
    fn scan_no_change_when_file_stable() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("20-test.conf");
        std::fs::write(&conf, "# stable").unwrap();

        let mut worker = SwayConfigWatcherWorker::new();
        // Prime the worker with the current state.
        let changed_first = worker.scan_for_changes_in(dir.path());
        assert!(
            changed_first,
            "first scan with no prior state looks like new file"
        );

        // Second scan with same file — no change.
        let changed_second = worker.scan_for_changes_in(dir.path());
        assert!(!changed_second, "stable file should not trigger reload");
    }
}

// ── Test helper shim ─────────────────────────────────────────────────────

#[cfg(test)]
impl SwayConfigWatcherWorker {
    /// Scan a single directory for tests (avoids XDG path resolution).
    fn scan_for_changes_in(&mut self, dir: &Path) -> bool {
        let mut current: HashMap<PathBuf, SystemTime> = HashMap::new();
        collect_mtimes(dir, &mut current);

        let mut any = false;
        for (p, m) in &current {
            if self.last_mtimes.get(p) != Some(m) {
                any = true;
            }
        }
        for p in self.last_mtimes.keys() {
            if !current.contains_key(p) {
                any = true;
            }
        }
        self.last_mtimes = current;
        any
    }
}

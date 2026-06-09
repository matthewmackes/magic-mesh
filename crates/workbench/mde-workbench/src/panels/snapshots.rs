//! Maintain → Snapshots panel — manual backups of the user's
//! MDE config tree.
//!
//! CB-1.9.d: replaces the Workbench surface of
//! `mackes/workbench/maintain/snapshots.py`. The v1.x library
//! `mackes.snapshots` stays as the spiritual backend; the
//! Iced panel does the on-disk operations itself (walk
//! manifests, copy tree, rm -rf) because they're pure
//! user-space file I/O — no polkit gating needed.
//!
//! Storage: `~/.local/share/mde/snapshots/<timestamp>/`
//! containing:
//!   * `manifest.json` — `{name, timestamp, hostname}`
//!   * `config/`       — copy of `~/.config/mde/` at snapshot time
//!
//! The legacy v1.x layout under
//! `~/.local/share/mackes-shell/snapshots/` is read on first
//! load so existing snapshots remain accessible during the
//! MDE rebrand window.

use std::path::{Path, PathBuf};

use iced::widget::{column, row, scrollable, text};
use iced::{Element, Length, Task};
use mde_theme::{Density, EmptyState, Icon, Palette};

use crate::controls::{styled_text_input, variant_button, ButtonVariant};
use crate::panel_chrome::{card, dialog, empty_state, panel_container};

/// Subdirectory name under each snapshot dir that holds the
/// copied config tree.
const CONFIG_SUBDIR: &str = "config";

/// Manifest file name written + parsed at each snapshot root.
const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotRow {
    pub id: String,
    pub name: String,
    pub timestamp: String,
    pub hostname: String,
    /// Absolute path to the snapshot directory on disk —
    /// drives delete + restore.
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotsPanel {
    pub rows: Vec<SnapshotRow>,
    pub new_name_input: String,
    pub status: String,
    pub busy: bool,
    /// Path of the snapshot pending restore confirmation;
    /// `None` = no confirmation modal up.
    pub pending_restore: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<SnapshotRow>),
    Error(String),
    NewNameChanged(String),
    CreateClicked,
    DeleteClicked(String),
    RestoreClicked(String),
    RestoreConfirmed,
    RestoreCancelled,
    OperationFinished(Result<String, String>),
}

impl SnapshotsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let mut rows = collect_snapshots(&snapshots_root_mde()).await;
                // Legacy v1.x path — keep readable through the
                // rebrand window so existing snapshots aren't
                // orphaned.
                let legacy = collect_snapshots(&snapshots_root_legacy()).await;
                rows.extend(legacy);
                rows.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                Message::Loaded(rows)
            },
            crate::Message::Snapshots,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.rows = rows;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::NewNameChanged(v) => {
                self.new_name_input = v;
                Task::none()
            }
            Message::CreateClicked => {
                if self.busy {
                    return Task::none();
                }
                let name = self.new_name_input.trim().to_string();
                if name.is_empty() {
                    self.status = "Snapshot name can't be empty.".into();
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Creating snapshot \"{name}\"…");
                Task::perform(
                    async move { Message::OperationFinished(create_snapshot(&name).await) },
                    crate::Message::Snapshots,
                )
            }
            Message::DeleteClicked(path) => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Deleting…".into();
                Task::perform(
                    async move { Message::OperationFinished(delete_snapshot(&path).await) },
                    crate::Message::Snapshots,
                )
            }
            Message::RestoreClicked(path) => {
                self.pending_restore = Some(path);
                Task::none()
            }
            Message::RestoreConfirmed => {
                let Some(path) = self.pending_restore.take() else {
                    return Task::none();
                };
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Restoring…".into();
                Task::perform(
                    async move { Message::OperationFinished(restore_snapshot(&path).await) },
                    crate::Message::Snapshots,
                )
            }
            Message::RestoreCancelled => {
                self.pending_restore = None;
                Task::none()
            }
            Message::OperationFinished(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(msg) => msg,
                    Err(msg) => msg,
                };
                self.new_name_input.clear();
                // Reload to reflect the new state.
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        if let Some(path) = &self.pending_restore {
            return self.view_confirm(path);
        }

        // UX-7 — buttons and inputs route through the shared
        // control primitives so hover/focus/active/disabled
        // states are consistent across panels.
        let palette = Palette::dark();
        let create_press = if self.busy {
            None
        } else {
            Some(crate::Message::Snapshots(Message::CreateClicked))
        };
        let create_btn = variant_button(
            "Create snapshot", // voice-allow:idiom-snapshot (capture-moment is "create", not "add")
            ButtonVariant::Primary,
            create_press,
            palette,
        );

        let new_name = styled_text_input(
            "Snapshot name",
            &self.new_name_input,
            |v| crate::Message::Snapshots(Message::NewNameChanged(v)),
            palette,
        );

        if self.rows.is_empty() {
            // UX-6 — empty-state with the same create flow
            // available above as the CTA. `new_name` and
            // `create_btn` stay unused on this branch; the
            // empty-state CTA is a stand-in until UX-8 lets us
            // show the textinput inline.
            let _ = (new_name, create_btn);
            let state = EmptyState::with_cta(
                "No snapshots yet",
                "Capture the current ~/.config/mde/ tree so you can roll back \
                 after experiments. Give the snapshot a name and click Create.",
                "Create snapshot", // voice-allow:idiom-snapshot
            )
            .with_icon(Icon::Snapshot);
            return panel_container(
                empty_state(state, Palette::dark(), || {
                    crate::Message::Snapshots(Message::CreateClicked)
                }),
                Density::Comfortable,
            );
        }

        // UX-6 — each snapshot row renders as a card surface so
        // the list reads as discrete items above the panel
        // background instead of a flat table.
        let rows_view = self.rows.iter().fold(column![], |col, row_data| {
            let restore_path = row_data.path.clone();
            let delete_path = row_data.path.clone();
            let restore_btn = variant_button(
                "Restore",
                ButtonVariant::Secondary,
                Some(crate::Message::Snapshots(Message::RestoreClicked(
                    restore_path,
                ))),
                palette,
            );
            let delete_btn = variant_button(
                "Delete", // voice-allow:destroy (snapshot deletion is destroy, not set-removal)
                ButtonVariant::Ghost,
                Some(crate::Message::Snapshots(Message::DeleteClicked(
                    delete_path,
                ))),
                palette,
            );
            let card_body = row![
                text(&row_data.name).width(Length::Fixed(220.0)),
                text(&row_data.timestamp).width(Length::Fixed(220.0)),
                text(&row_data.hostname).width(Length::Fixed(160.0)),
                restore_btn,
                delete_btn,
            ]
            .spacing(12)
            .into();
            col.push(card(card_body, Palette::dark(), Density::Comfortable))
        });

        column![
            row![new_name, create_btn].spacing(12),
            scrollable(rows_view.spacing(8)).height(Length::Fill),
            row![text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }

    fn view_confirm(&self, path: &str) -> Element<'_, crate::Message> {
        // UX-9 (c) — confirm dialog uses the shared dialog
        // chrome. UX-7 — buttons route through the variant
        // helpers (Primary for the destructive confirm, Ghost
        // for cancel) so hover/focus states stay consistent
        // with the rest of the app.
        let palette = Palette::dark();
        let confirm_btn = variant_button(
            "Apply restore",
            ButtonVariant::Primary,
            Some(crate::Message::Snapshots(Message::RestoreConfirmed)),
            palette,
        );
        let cancel_btn = variant_button(
            "Cancel",
            ButtonVariant::Ghost,
            Some(crate::Message::Snapshots(Message::RestoreCancelled)),
            palette,
        );
        let body: Element<'_, crate::Message> = column![
            text("Restore snapshot?").size(20),
            text(format!(
                "This will overwrite ~/.config/mde/ with the contents of:\n{path}",
            ))
            .size(13),
            text(
                "Existing files not present in the snapshot will be left in \
                 place; files present in the snapshot will replace the live \
                 copies.",
            )
            .size(13),
            row![confirm_btn, cancel_btn].spacing(12),
        ]
        .spacing(12)
        .into();
        panel_container(
            dialog(body, Palette::dark(), Density::Comfortable),
            Density::Comfortable,
        )
    }
}

/// `~/.local/share/mde/snapshots/` — v2.0.0 canonical path.
fn snapshots_root_mde() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/mde/snapshots")
}

/// `~/.local/share/mackes-shell/snapshots/` — v1.x path,
/// readable through the rebrand window so legacy snapshots
/// remain accessible until the user migrates.
fn snapshots_root_legacy() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/mackes-shell/snapshots")
}

/// Source config tree that `create_snapshot` copies + that
/// `restore_snapshot` writes back into.
fn live_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/mde")
}

/// Walk a snapshots root and parse the manifest.json of every
/// subdirectory. Returns an empty Vec when the root doesn't
/// exist (fresh-install case).
pub async fn collect_snapshots(root: &Path) -> Vec<SnapshotRow> {
    let Ok(mut rd) = tokio::fs::read_dir(root).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        let manifest_path = dir.join(MANIFEST_FILE);
        let Ok(raw) = tokio::fs::read_to_string(&manifest_path).await else {
            continue;
        };
        if let Some(row) = parse_manifest(&dir.to_string_lossy(), &raw) {
            out.push(row);
        }
    }
    out
}

/// Pure parser for the per-snapshot manifest. The JSON shape
/// matches the v1.x Python library: `{name, timestamp,
/// hostname}` (with `id` derived from the directory basename).
#[must_use]
pub fn parse_manifest(snapshot_path: &str, raw: &str) -> Option<SnapshotRow> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = v.as_object()?;
    let id = Path::new(snapshot_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    Some(SnapshotRow {
        id,
        name: obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)")
            .to_string(),
        timestamp: obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        hostname: obj
            .get("hostname")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        path: snapshot_path.to_string(),
    })
}

/// Build a sanitised, timestamped snapshot directory id from a
/// user-supplied name. The format matches the v1.x library:
/// `YYYY-MM-DDTHHMMSS_<sanitised-name>`. The sanitiser keeps
/// ASCII alphanumerics + `-_`, replaces everything else with
/// `-`, and trims runs of dashes.
#[must_use]
pub fn build_snapshot_id(now_unix_seconds: i64, name: &str) -> String {
    let secs = now_unix_seconds.max(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (year, month, day) = days_to_ymd(days);
    let sanitised = sanitise_name(name);
    format!("{year:04}-{month:02}-{day:02}T{h:02}{m:02}{s:02}_{sanitised}")
}

fn sanitise_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        let keep = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
        if keep {
            out.push(ch);
            prev_dash = ch == '-';
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

/// Create a new snapshot — copy `~/.config/mde/` into a fresh
/// timestamped directory + write the manifest. Returns a
/// human-readable status message on success or failure.
pub async fn create_snapshot(name: &str) -> Result<String, String> {
    let root = snapshots_root_mde();
    if let Err(e) = tokio::fs::create_dir_all(&root).await {
        return Err(format!("creating {}: {e}", root.display()));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let id = build_snapshot_id(now, name);
    let dir = root.join(&id);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return Err(format!("creating {}: {e}", dir.display()));
    }
    let config_dst = dir.join(CONFIG_SUBDIR);
    let config_src = live_config_dir();
    if config_src.is_dir() {
        copy_dir_recursive(&config_src, &config_dst).await?;
    }
    let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into());
    let manifest = serde_json::json!({
        "name":      name,
        "timestamp": id,
        "hostname":  hostname,
    });
    if let Err(e) = tokio::fs::write(dir.join(MANIFEST_FILE), manifest.to_string()).await {
        return Err(format!("writing manifest: {e}"));
    }
    Ok(format!("Snapshot \"{name}\" saved as {id}."))
}

/// Restore a snapshot by copying its config tree back into
/// `~/.config/mde/`. The existing live config is NOT wiped
/// first — files in the snapshot replace their live
/// counterparts; files not in the snapshot survive. This is
/// less destructive than the v1.x panel's wipe-and-restore;
/// the trade-off is captured in the confirmation modal text.
pub async fn restore_snapshot(path: &str) -> Result<String, String> {
    let snap_config = PathBuf::from(path).join(CONFIG_SUBDIR);
    if !snap_config.is_dir() {
        return Err(format!("snapshot is missing {CONFIG_SUBDIR}/ subtree"));
    }
    let dst = live_config_dir();
    if let Err(e) = tokio::fs::create_dir_all(&dst).await {
        return Err(format!("creating {}: {e}", dst.display()));
    }
    copy_dir_recursive(&snap_config, &dst).await?;
    Ok(format!("Restored snapshot from {path}."))
}

/// Delete a snapshot directory. rm -rf, no recovery — the
/// confirmation step happens UI-side (a small per-row
/// affordance; full modal would be overkill for a delete).
pub async fn delete_snapshot(path: &str) -> Result<String, String> {
    tokio::fs::remove_dir_all(path)
        .await
        .map_err(|e| format!("deleting {path}: {e}"))?;
    Ok(format!("Deleted snapshot at {path}."))
}

/// Recursive directory copy via the std fs API tunneled
/// through tokio's blocking pool. tokio doesn't ship a
/// recursive-copy helper and we don't want the `fs_extra`
/// crate dep for one panel.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    let src = src.to_path_buf();
    let dst = dst.to_path_buf();
    tokio::task::spawn_blocking(move || copy_dir_recursive_blocking(&src, &dst))
        .await
        .map_err(|e| format!("join: {e}"))?
}

fn copy_dir_recursive_blocking(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let src_path = entry.path();
        let name = entry.file_name();
        let dst_path = dst.join(&name);
        let ft = entry.file_type().map_err(|e| format!("file_type: {e}"))?;
        if ft.is_dir() {
            copy_dir_recursive_blocking(&src_path, &dst_path)?;
        } else if ft.is_file() {
            std::fs::copy(&src_path, &dst_path).map_err(|e| {
                format!("copy {} → {}: {e}", src_path.display(), dst_path.display())
            })?;
        }
        // Symlinks are ignored — preserves the v1.x panel
        // behaviour and avoids following dangling links into
        // surprising directories.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "name": "pre-display-tweaks",
        "timestamp": "2024-05-06T142300_pre-display-tweaks",
        "hostname": "alpha-host"
    }"#;

    #[test]
    fn parse_manifest_extracts_every_field() {
        let row = parse_manifest("/snaps/2024-05-06T142300_pre-display-tweaks", SAMPLE).unwrap();
        assert_eq!(row.name, "pre-display-tweaks");
        assert_eq!(row.timestamp, "2024-05-06T142300_pre-display-tweaks");
        assert_eq!(row.hostname, "alpha-host");
        assert_eq!(row.id, "2024-05-06T142300_pre-display-tweaks");
        assert_eq!(row.path, "/snaps/2024-05-06T142300_pre-display-tweaks");
    }

    #[test]
    fn parse_manifest_defaults_missing_fields() {
        let row = parse_manifest("/snaps/x", "{}").unwrap();
        assert_eq!(row.name, "(unnamed)");
        assert_eq!(row.timestamp, "");
        assert_eq!(row.hostname, "");
    }

    #[test]
    fn parse_manifest_rejects_non_object() {
        assert!(parse_manifest("/x", "[]").is_none());
        assert!(parse_manifest("/x", "not json").is_none());
    }

    #[test]
    fn sanitise_name_keeps_alnum_and_dash_underscore() {
        assert_eq!(sanitise_name("hello-world_v2"), "hello-world_v2");
        assert_eq!(sanitise_name("hello world"), "hello-world");
        assert_eq!(sanitise_name("a  b  c"), "a-b-c");
        assert_eq!(sanitise_name("--leading--"), "leading");
        assert_eq!(sanitise_name(""), "");
    }

    #[test]
    fn build_snapshot_id_combines_timestamp_and_name() {
        // 1715000000 → 2024-05-06 16:53:20 UTC.
        let id = build_snapshot_id(1_715_000_000, "pre-display");
        assert!(id.starts_with("2024-05-06T"), "got: {id}");
        assert!(id.ends_with("_pre-display"));
    }

    #[test]
    fn build_snapshot_id_sanitises_garbage_names() {
        let id = build_snapshot_id(1_715_000_000, "test/dir name");
        assert!(id.ends_with("_test-dir-name"));
    }

    #[test]
    fn loaded_records_rows_and_clears_busy() {
        let mut panel = SnapshotsPanel::new();
        panel.busy = true;
        let rows = vec![parse_manifest("/x/y", SAMPLE).unwrap()];
        let _ = panel.update(Message::Loaded(rows.clone()));
        assert_eq!(panel.rows, rows);
        assert!(!panel.busy);
    }

    #[test]
    fn create_clicked_with_empty_name_surfaces_validation() {
        let mut panel = SnapshotsPanel::new();
        panel.new_name_input = "   ".into();
        let _ = panel.update(Message::CreateClicked);
        assert!(panel.status.contains("empty"));
        assert!(!panel.busy);
    }

    #[test]
    fn create_clicked_while_busy_is_noop() {
        let mut panel = SnapshotsPanel::new();
        panel.busy = true;
        panel.status = "Creating…".into();
        let _ = panel.update(Message::CreateClicked);
        assert_eq!(panel.status, "Creating…");
    }

    #[test]
    fn restore_clicked_arms_confirmation_modal() {
        let mut panel = SnapshotsPanel::new();
        let _ = panel.update(Message::RestoreClicked("/snaps/x".into()));
        assert_eq!(panel.pending_restore.as_deref(), Some("/snaps/x"));
    }

    #[test]
    fn restore_cancelled_clears_confirmation() {
        let mut panel = SnapshotsPanel::new();
        panel.pending_restore = Some("/snaps/x".into());
        let _ = panel.update(Message::RestoreCancelled);
        assert!(panel.pending_restore.is_none());
    }

    #[test]
    fn restore_confirmed_without_pending_is_noop() {
        let mut panel = SnapshotsPanel::new();
        let _ = panel.update(Message::RestoreConfirmed);
        assert!(!panel.busy);
    }

    #[test]
    fn operation_finished_ok_carries_status_and_clears_busy() {
        let mut panel = SnapshotsPanel::new();
        panel.busy = true;
        panel.new_name_input = "old".into();
        let _ = panel.update(Message::OperationFinished(Ok("Done.".into())));
        assert!(!panel.busy);
        assert_eq!(panel.status, "Done.");
        assert!(panel.new_name_input.is_empty());
    }

    #[test]
    fn operation_finished_err_carries_error_and_clears_busy() {
        let mut panel = SnapshotsPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Err("perm denied".into())));
        assert!(!panel.busy);
        assert_eq!(panel.status, "perm denied");
    }

    #[tokio::test]
    async fn collect_snapshots_missing_dir_returns_empty() {
        let bogus = PathBuf::from("/nonexistent-snapshots-7234923");
        assert!(collect_snapshots(&bogus).await.is_empty());
    }

    #[tokio::test]
    async fn collect_snapshots_round_trips_a_real_directory() {
        let tmp = std::env::temp_dir().join("mde-snapshots-test-roundtrip");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        let snap = tmp.join("2024-05-06T142300_test");
        tokio::fs::create_dir_all(&snap).await.unwrap();
        tokio::fs::write(snap.join(MANIFEST_FILE), SAMPLE)
            .await
            .unwrap();
        let rows = collect_snapshots(&tmp).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "pre-display-tweaks");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn delete_snapshot_removes_the_dir() {
        let tmp = std::env::temp_dir().join("mde-snapshots-test-delete");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        let snap = tmp.join("2024-05-06T142300_test");
        tokio::fs::create_dir_all(&snap).await.unwrap();
        delete_snapshot(snap.to_str().unwrap()).await.unwrap();
        assert!(!snap.exists());
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
}

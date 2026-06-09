//! System → Default Apps panel — edits
//! `~/.config/mimeapps.list` (the XDG-standard user MIME-to-
//! application mapping).
//!
//! CB-1.9.b: replaces the v1.x
//! `mackes/workbench/system/default_apps.py`. The Python panel
//! walked every .desktop file under XDG application dirs to
//! discover MIME-type handlers, then read/wrote mimeapps.list
//! via configparser. The Rust port keeps the same shape but
//! does the .desktop walking + mimeapps.list editing inline
//! (no new mded subcommand) — the operations are pure file I/O
//! against the user's `~/.config` and `~/.local/share`, no
//! polkit gating needed.
//!
//! Eight curated categories per the v1.x lock — each maps to
//! one or more canonical MIME types, and changing the dropdown
//! writes the same desktop-id into the
//! `[Default Applications]` section for every MIME in the
//! group.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use iced::widget::{column, pick_list, row, text};
use iced::{Element, Length, Task};

/// Curated MIME categories. Order is the panel render order.
pub const CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Web browser",
        &[
            "x-scheme-handler/http",
            "x-scheme-handler/https",
            "text/html",
        ],
    ),
    ("Email", &["x-scheme-handler/mailto", "message/rfc822"]),
    ("File manager", &["inode/directory"]),
    ("Terminal", &["x-scheme-handler/terminal"]),
    ("Text editor", &["text/plain"]),
    ("Image viewer", &["image/png", "image/jpeg", "image/webp"]),
    ("Video player", &["video/mp4", "video/x-matroska"]),
    ("Audio player", &["audio/mpeg", "audio/flac", "audio/ogg"]),
    ("PDF viewer", &["application/pdf"]),
];

/// A discovered .desktop file — `id` is the bare filename
/// (e.g. `firefox.desktop`), `name` is the localized
/// human-readable label from the `Name=` line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopHandler {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct DefaultAppsPanel {
    /// Per-category list of handlers found on disk that declared
    /// at least one of the category's MIME types.
    pub handlers_per_category: Vec<Vec<DesktopHandler>>,
    /// Current default per category — empty when no row exists
    /// in `mimeapps.list`'s `[Default Applications]` block.
    pub current_per_category: Vec<String>,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        handlers_per_category: Vec<Vec<DesktopHandler>>,
        current_per_category: Vec<String>,
    },
    Error(String),
    CategorySelected {
        category_idx: usize,
        desktop_id: String,
    },
    Applied,
}

impl DefaultAppsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let app_dirs = xdg_application_dirs();
                let all_handlers = discover_handlers(&app_dirs).await;
                let mimeapps = read_mimeapps_list().await;
                let current = current_defaults_for_categories(&mimeapps);
                let handlers_per_category: Vec<Vec<DesktopHandler>> = CATEGORIES
                    .iter()
                    .map(|(_, mimes)| {
                        let mut acc: HashMap<String, DesktopHandler> = HashMap::new();
                        for m in mimes.iter() {
                            if let Some(hs) = all_handlers.get(*m) {
                                for h in hs {
                                    acc.entry(h.id.clone()).or_insert_with(|| h.clone());
                                }
                            }
                        }
                        let mut v: Vec<DesktopHandler> = acc.into_values().collect();
                        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                        v
                    })
                    .collect();
                Message::Loaded {
                    handlers_per_category,
                    current_per_category: current,
                }
            },
            crate::Message::DefaultApps,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                handlers_per_category,
                current_per_category,
            } => {
                self.handlers_per_category = handlers_per_category;
                self.current_per_category = current_per_category;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::CategorySelected {
                category_idx,
                desktop_id,
            } => {
                if self.busy {
                    return Task::none();
                }
                if category_idx >= self.current_per_category.len() {
                    return Task::none();
                }
                self.current_per_category[category_idx] = desktop_id.clone();
                self.busy = true;
                self.status = format!("Setting default to {desktop_id}…");
                let mimes: Vec<String> = CATEGORIES[category_idx]
                    .1
                    .iter()
                    .map(|m| (*m).to_string())
                    .collect();
                Task::perform(
                    async move {
                        let path = mimeapps_path();
                        let _ = write_mimeapps_defaults(&path, &mimes, &desktop_id).await;
                        Message::Applied
                    },
                    crate::Message::DefaultApps,
                )
            }
            Message::Applied => {
                self.status = "Applied.".into();
                self.busy = false;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let mut col = column![].spacing(12);
        for (idx, (label, _)) in CATEGORIES.iter().enumerate() {
            let handlers = self
                .handlers_per_category
                .get(idx)
                .cloned()
                .unwrap_or_default();
            let current = self
                .current_per_category
                .get(idx)
                .cloned()
                .unwrap_or_default();
            let id_list: Vec<String> = handlers.iter().map(|h| h.id.clone()).collect();
            let pick: pick_list::PickList<'_, String, _, _, crate::Message> = pick_list(
                id_list,
                if current.is_empty() {
                    None
                } else {
                    Some(current)
                },
                move |v| {
                    crate::Message::DefaultApps(Message::CategorySelected {
                        category_idx: idx,
                        desktop_id: v,
                    })
                },
            );
            col = col.push(row![text(*label).width(Length::Fixed(180.0)), pick,].spacing(12));
        }
        col.push(row![text(&self.status).size(13)].spacing(12))
            .width(Length::Fill)
            .into()
    }
}

/// Resolve the standard XDG application directories — same set
/// the Python panel walked: per-user, system-local, system.
#[must_use]
pub fn xdg_application_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = Vec::new();
    if !home.is_empty() {
        out.push(PathBuf::from(&home).join(".local/share/applications"));
    }
    out.push(PathBuf::from("/usr/local/share/applications"));
    out.push(PathBuf::from("/usr/share/applications"));
    out
}

#[must_use]
fn mimeapps_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/mimeapps.list")
}

/// Walk every `.desktop` file under the given roots and build a
/// `MIME-type -> [DesktopHandler]` map. The Python panel
/// shadowed earlier roots with later ones; we do the same by
/// keeping the first definition for each (id, mime) pair —
/// the per-user dir lands first, so it shadows system entries.
pub async fn discover_handlers(roots: &[PathBuf]) -> HashMap<String, Vec<DesktopHandler>> {
    let mut out: HashMap<String, Vec<DesktopHandler>> = HashMap::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();
    for root in roots {
        let Ok(mut rd) = tokio::fs::read_dir(root).await else {
            continue;
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if seen_ids.contains_key(id) {
                continue;
            }
            let Ok(raw) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let Some(handler) = parse_desktop_entry(id, &raw) else {
                continue;
            };
            seen_ids.insert(id.to_string(), ());
            // Walk the MimeType= line to bucket per MIME.
            for m in handler_mime_types(&raw) {
                out.entry(m).or_default().push(handler.clone());
            }
        }
    }
    out
}

/// Parse a `.desktop` file into a `DesktopHandler` if it
/// declares an applications-style `Desktop Entry` group + isn't
/// `NoDisplay=true` / `Hidden=true`. Returns `None` for invalid
/// files so the walker can skip them silently.
#[must_use]
pub fn parse_desktop_entry(id: &str, raw: &str) -> Option<DesktopHandler> {
    let mut in_section = false;
    let mut name: Option<String> = None;
    let mut nodisplay = false;
    let mut hidden = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == "[Desktop Entry]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Name=") {
            if name.is_none() {
                name = Some(rest.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("NoDisplay=") {
            nodisplay = rest.eq_ignore_ascii_case("true");
        } else if let Some(rest) = trimmed.strip_prefix("Hidden=") {
            hidden = rest.eq_ignore_ascii_case("true");
        }
    }
    if nodisplay || hidden {
        return None;
    }
    Some(DesktopHandler {
        id: id.to_string(),
        name: name.unwrap_or_else(|| id.trim_end_matches(".desktop").to_string()),
    })
}

/// Pull the list of MIME types a `.desktop` entry declares.
/// Returns an empty Vec when the file has no `MimeType=` line —
/// the walker skips those entirely.
#[must_use]
pub fn handler_mime_types(raw: &str) -> Vec<String> {
    let mut in_section = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == "[Desktop Entry]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("MimeType=") {
            return rest
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
        }
    }
    Vec::new()
}

/// Read the user's `~/.config/mimeapps.list` into a single
/// flat `mime -> desktop_id` map (the
/// `[Default Applications]` section only).
pub async fn read_mimeapps_list() -> HashMap<String, String> {
    let path = mimeapps_path();
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return HashMap::new();
    };
    parse_mimeapps_defaults(&raw)
}

/// Pure parser for the `[Default Applications]` section of a
/// `mimeapps.list` payload. Other sections (`Added`,
/// `Removed Associations`) are intentionally ignored — the
/// panel only edits the `Default` block.
#[must_use]
pub fn parse_mimeapps_defaults(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut in_default = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_default = trimmed == "[Default Applications]";
            continue;
        }
        if !in_default {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            let (k, v) = trimmed.split_at(eq);
            out.insert(k.trim().to_string(), v[1..].trim().to_string());
        }
    }
    out
}

/// Resolve the current default per category, given a flat
/// `mime -> desktop_id` map. Returns the desktop_id of the
/// first MIME in the category that has a default — matches the
/// v1.x panel's "first MIME's default wins" UI semantic.
#[must_use]
pub fn current_defaults_for_categories(mimeapps: &HashMap<String, String>) -> Vec<String> {
    CATEGORIES
        .iter()
        .map(|(_, mimes)| {
            mimes
                .iter()
                .find_map(|m| mimeapps.get(*m).cloned())
                .unwrap_or_default()
        })
        .collect()
}

/// Write the given `desktop_id` as the default for every MIME
/// in `mimes` within the `[Default Applications]` section of
/// `mimeapps.list`. Preserves every other line. Creates the
/// file (and section) if absent.
///
/// # Errors
///
/// Returns `Err(String)` when the parent directory is unwritable
/// or the file can't be opened for writing.
pub async fn write_mimeapps_defaults(
    path: &Path,
    mimes: &[String],
    desktop_id: &str,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let existing = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let rewritten = rewrite_mimeapps(&existing, mimes, desktop_id);
    tokio::fs::write(path, rewritten)
        .await
        .map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Pure helper for `write_mimeapps_defaults`. Rewrites the
/// `[Default Applications]` section to reflect the new
/// `mime -> desktop_id` pairs while preserving every other
/// section verbatim. Creates the section if missing.
#[must_use]
pub fn rewrite_mimeapps(existing: &str, mimes: &[String], desktop_id: &str) -> String {
    let mut out = String::new();
    let mut in_default = false;
    let mut seen_default = false;
    let mut written: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let to_write: Vec<&str> = mimes.iter().map(String::as_str).collect();
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_default {
                // We're exiting the Default Applications block;
                // flush any unwritten mime= lines now.
                for m in &to_write {
                    if !written.contains(m) {
                        out.push_str(&format!("{m}={desktop_id}\n"));
                        written.insert(m);
                    }
                }
            }
            in_default = trimmed == "[Default Applications]";
            if in_default {
                seen_default = true;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_default {
            if let Some(eq) = trimmed.find('=') {
                let key = trimmed[..eq].trim();
                if to_write.contains(&key) {
                    out.push_str(&format!("{key}={desktop_id}\n"));
                    written.insert(to_write.iter().find(|k| **k == key).copied().unwrap_or(""));
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    // EOF inside the Default block — flush remaining.
    if in_default {
        for m in &to_write {
            if !written.contains(m) {
                out.push_str(&format!("{m}={desktop_id}\n"));
                written.insert(m);
            }
        }
    }
    // No Default block existed in the file — append one.
    if !seen_default {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("[Default Applications]\n");
        for m in &to_write {
            out.push_str(&format!("{m}={desktop_id}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_lock_matches_v1_python_panel() {
        let labels: Vec<&str> = CATEGORIES.iter().map(|(l, _)| *l).collect();
        assert!(labels.contains(&"Web browser"));
        assert!(labels.contains(&"Email"));
        assert!(labels.contains(&"Terminal"));
        assert!(labels.contains(&"PDF viewer"));
        assert_eq!(labels.len(), 9);
    }

    #[test]
    fn parse_desktop_entry_extracts_name_when_present() {
        let raw = "\
[Desktop Entry]
Type=Application
Name=Firefox
Exec=firefox %U
";
        let h = parse_desktop_entry("firefox.desktop", raw).unwrap();
        assert_eq!(h.id, "firefox.desktop");
        assert_eq!(h.name, "Firefox");
    }

    #[test]
    fn parse_desktop_entry_falls_back_to_id_stem_when_name_missing() {
        let raw = "[Desktop Entry]\nType=Application\n";
        let h = parse_desktop_entry("foo.desktop", raw).unwrap();
        assert_eq!(h.name, "foo"); // voice-allow:test-data
    }

    #[test]
    fn parse_desktop_entry_skips_nodisplay_and_hidden() {
        assert!(parse_desktop_entry(
            "hidden.desktop",
            "[Desktop Entry]\nName=X\nNoDisplay=true\n"
        )
        .is_none());
        assert!(
            parse_desktop_entry("hidden.desktop", "[Desktop Entry]\nName=X\nHidden=TRUE\n")
                .is_none()
        );
    }

    #[test]
    fn parse_desktop_entry_ignores_non_entry_sections() {
        let raw = "\
[Desktop Action New]
Name=New Window
Exec=foo --new
[Desktop Entry]
Name=Real
";
        let h = parse_desktop_entry("x.desktop", raw).unwrap();
        assert_eq!(h.name, "Real");
    }

    #[test]
    fn handler_mime_types_extracts_semicolon_list() {
        let raw = "\
[Desktop Entry]
Name=Firefox
MimeType=text/html;application/xhtml+xml;x-scheme-handler/http;
";
        let mimes = handler_mime_types(raw);
        assert!(mimes.contains(&"text/html".to_string()));
        assert!(mimes.contains(&"x-scheme-handler/http".to_string()));
        assert_eq!(mimes.len(), 3);
    }

    #[test]
    fn handler_mime_types_empty_when_no_mimetype_line() {
        assert!(handler_mime_types("[Desktop Entry]\nName=X\n").is_empty());
    }

    #[test]
    fn parse_mimeapps_defaults_only_default_section() {
        let raw = "\
[Default Applications]
text/html=firefox.desktop
inode/directory=nautilus.desktop

[Added Associations]
text/html=chromium.desktop;
";
        let parsed = parse_mimeapps_defaults(raw);
        assert_eq!(
            parsed.get("text/html"),
            Some(&"firefox.desktop".to_string())
        );
        assert_eq!(
            parsed.get("inode/directory"),
            Some(&"nautilus.desktop".to_string())
        );
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn current_defaults_for_categories_uses_first_mime_in_group() {
        let mut mimeapps = HashMap::new();
        mimeapps.insert(
            "x-scheme-handler/http".into(),
            "firefox.desktop".to_string(),
        );
        mimeapps.insert("inode/directory".into(), "nautilus.desktop".to_string());
        let defaults = current_defaults_for_categories(&mimeapps);
        // CATEGORIES[0] is "Web browser" with http first.
        assert_eq!(defaults[0], "firefox.desktop");
        // "File manager" with inode/directory.
        let file_mgr_idx = CATEGORIES
            .iter()
            .position(|(label, _)| *label == "File manager")
            .unwrap();
        assert_eq!(defaults[file_mgr_idx], "nautilus.desktop");
    }

    #[test]
    fn rewrite_mimeapps_replaces_existing_default_lines() {
        let existing = "\
[Default Applications]
text/html=old-browser.desktop
inode/directory=keep-fm.desktop
";
        let out = rewrite_mimeapps(existing, &["text/html".to_string()], "new-browser.desktop");
        assert!(out.contains("text/html=new-browser.desktop"));
        assert!(out.contains("inode/directory=keep-fm.desktop"));
        assert!(!out.contains("old-browser.desktop"));
    }

    #[test]
    fn rewrite_mimeapps_appends_default_section_when_absent() {
        let existing = "[Added Associations]\nfoo=bar.desktop\n";
        let out = rewrite_mimeapps(existing, &["text/html".to_string()], "firefox.desktop");
        assert!(out.contains("[Default Applications]"));
        assert!(out.contains("text/html=firefox.desktop"));
        // Verify the existing block was preserved.
        assert!(out.contains("[Added Associations]"));
        assert!(out.contains("foo=bar.desktop"));
    }

    #[test]
    fn rewrite_mimeapps_appends_missing_mime_to_existing_default() {
        let existing = "\
[Default Applications]
inode/directory=fm.desktop
";
        let out = rewrite_mimeapps(existing, &["text/html".to_string()], "firefox.desktop");
        assert!(out.contains("text/html=firefox.desktop"));
        assert!(out.contains("inode/directory=fm.desktop"));
    }

    #[test]
    fn rewrite_mimeapps_writes_each_mime_in_group() {
        let existing = "[Default Applications]\n";
        let out = rewrite_mimeapps(
            existing,
            &[
                "x-scheme-handler/http".to_string(),
                "x-scheme-handler/https".to_string(),
            ],
            "firefox.desktop",
        );
        assert!(out.contains("x-scheme-handler/http=firefox.desktop"));
        assert!(out.contains("x-scheme-handler/https=firefox.desktop"));
    }

    #[test]
    fn loaded_records_state_and_clears_status() {
        let mut panel = DefaultAppsPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Loaded {
            handlers_per_category: vec![Vec::new(); CATEGORIES.len()],
            current_per_category: vec![String::new(); CATEGORIES.len()],
        });
        assert!(!panel.busy);
        assert!(panel.status.is_empty());
    }

    #[test]
    fn category_selected_out_of_bounds_is_noop() {
        let mut panel = DefaultAppsPanel::new();
        panel.current_per_category = vec!["a.desktop".into()];
        let _ = panel.update(Message::CategorySelected {
            category_idx: 99,
            desktop_id: "b.desktop".into(),
        });
        assert_eq!(panel.current_per_category[0], "a.desktop");
    }

    #[test]
    fn applied_clears_busy_and_records_status() {
        let mut panel = DefaultAppsPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Applied);
        assert!(!panel.busy);
        assert_eq!(panel.status, "Applied.");
    }
}

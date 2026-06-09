//! AIR-11.b (v6.1) — persisted library-view preferences.
//!
//! The grid's sort selection persists across launches (and, on mesh-home,
//! across peers) via `~/.local/share/mde/music-prefs.json` (Q13). The
//! read/write + [`apply_sort`] helpers are pure + unit-tested; the Iced
//! view loads the prefs at startup, toggles + saves on the sort control,
//! and applies the order before laying the grid out. (Per-page scroll-
//! position persistence is the remaining AIR-11.b slice.)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::library::LibraryItem;

/// How the library grid is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    /// A→Z by name (default).
    #[default]
    NameAsc,
    /// Z→A by name.
    NameDesc,
}

impl SortKey {
    /// The other order (what the sort control switches to).
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::NameAsc => Self::NameDesc,
            Self::NameDesc => Self::NameAsc,
        }
    }

    /// A short label for the sort control.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NameAsc => "Name A–Z",
            Self::NameDesc => "Name Z–A",
        }
    }
}

/// Persisted library-view preferences.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MusicPrefs {
    #[serde(default)]
    pub sort: SortKey,
    /// AIR-11.c.4 — per-route library-grid scroll offset (y), persisted so
    /// the scroll position survives a relaunch. Keyed by breadcrumb segment.
    #[serde(default)]
    pub scroll: std::collections::BTreeMap<String, f32>,
}

/// `$HOME/.local/share/mde/music-prefs.json`.
#[must_use]
pub fn prefs_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    Path::new(&home).join(".local/share/mde/music-prefs.json")
}

/// Read prefs from `path` (defaults when absent/malformed — prefs are a
/// rebuildable convenience, never a hard error).
#[must_use]
pub fn read_from(path: &Path) -> MusicPrefs {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write prefs to `path` (best-effort; creates the parent dir).
pub fn write_to(path: &Path, prefs: &MusicPrefs) {
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }
}

/// Load prefs from the default path.
#[must_use]
pub fn load() -> MusicPrefs {
    read_from(&prefs_path())
}

/// Save prefs to the default path.
pub fn save(prefs: &MusicPrefs) {
    write_to(&prefs_path(), prefs);
}

/// Order `items` by label per `key` (case-insensitive).
pub fn apply_sort(items: &mut [LibraryItem], key: SortKey) {
    items.sort_by(|a, b| {
        let ord = a.label.to_lowercase().cmp(&b.label.to_lowercase());
        match key {
            SortKey::NameAsc => ord,
            SortKey::NameDesc => ord.reverse(),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_toggles_and_labels() {
        assert_eq!(SortKey::default(), SortKey::NameAsc);
        assert_eq!(SortKey::NameAsc.toggled(), SortKey::NameDesc);
        assert_eq!(SortKey::NameDesc.toggled(), SortKey::NameAsc);
        assert!(!SortKey::NameAsc.label().is_empty());
    }

    #[test]
    fn prefs_round_trip_and_default() {
        let p = std::env::temp_dir().join(format!("mde-music-prefs-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert_eq!(read_from(&p), MusicPrefs::default()); // absent → default
        write_to(
            &p,
            &MusicPrefs {
                sort: SortKey::NameDesc,
                ..Default::default()
            },
        );
        assert_eq!(read_from(&p).sort, SortKey::NameDesc);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn apply_sort_orders_by_label_case_insensitive() {
        let mut items = vec![
            LibraryItem {
                id: "1".into(),
                label: "Zoo".into(),
                art_id: None,
            },
            LibraryItem {
                id: "2".into(),
                label: "apple".into(),
                art_id: None,
            },
            LibraryItem {
                id: "3".into(),
                label: "Mango".into(),
                art_id: None,
            },
        ];
        apply_sort(&mut items, SortKey::NameAsc);
        assert_eq!(
            items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["apple", "Mango", "Zoo"]
        );
        apply_sort(&mut items, SortKey::NameDesc);
        assert_eq!(
            items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["Zoo", "Mango", "apple"]
        );
    }
}

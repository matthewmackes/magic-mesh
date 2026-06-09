//! Legacy state inventory (Phase 12.13.1).
//!
//! Produces a structured catalog of every JSON / TOML / cache file
//! that today lives under the three legacy roots called out by the
//! 12.13 lock:
//!
//!   * `~/.config/mackes-shell/`
//!   * `~/.qnm-sync/`
//!   * `~/.cache/mackes/`
//!
//! The inventory is the *first* step of the migration path — it does
//! NOT mutate anything, and it does NOT decide what gets imported.
//! It just tells the operator (and 12.13.2's importer) what's on
//! disk so they can reason about it before running
//! `mackesd import-legacy`.
//!
//! Heuristics:
//!
//!   * Classification by file extension (`.json` → `JsonConfig` or
//!     `JsonCache` depending on whether the path contains a cache
//!     segment; `.toml` → `TomlConfig`; other extensions in cache
//!     dirs → `BinaryCache`; everything else → `Unknown`).
//!   * Mesh-relatedness by case-insensitive filename substring match
//!     against a small allow-list (`mesh`, `peer`, `tailscale`,
//!     `headscale`, `qnm`). The operator can use `--mesh-only` to
//!     trim the output to just the artifacts the importer will care
//!     about.
//!
//! Bounded recursion: the walker descends at most `MAX_DEPTH` levels
//! below each root so a stray symlink loop or a deeply-nested
//! `node_modules`-style tree (unlikely under these roots, but cheap
//! to defend against) can't run away. Symlinks are not followed.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Maximum directory depth the walker will descend under any single
/// root. 4 levels is enough to cover today's layouts (e.g.
/// `~/.config/mackes-shell/<preset>/<panel>/state.json`) with margin.
pub const MAX_DEPTH: usize = 4;

/// Classification of a single on-disk artifact.
///
/// The variants are coarse on purpose — the importer (12.13.2) is
/// what knows how to parse each one; the inventory just needs to
/// tell the operator what shape it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// JSON file under a `config` root (e.g. `~/.config/mackes-shell/`).
    JsonConfig,
    /// TOML file under a `config` root.
    TomlConfig,
    /// JSON file under a `cache` root (e.g. `~/.cache/mackes/`) or
    /// a path segment that contains `cache`.
    JsonCache,
    /// Non-JSON/TOML file under a cache root. Usually a binary blob
    /// (sqlite db, pickle, image, etc.).
    BinaryCache,
    /// Anything else: doesn't match a known extension and isn't in a
    /// cache-ish path. The operator can inspect manually.
    Unknown,
}

/// A single inventoried artifact: where it lives, how big it is,
/// when it last changed, what shape it is, and whether the filename
/// hints at mesh state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyArtifact {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// File size in bytes (from `std::fs::Metadata::len`).
    pub size_bytes: u64,
    /// Modification time in milliseconds since the Unix epoch. `0`
    /// when the underlying filesystem reports a pre-epoch mtime or
    /// when the mtime is unavailable.
    pub mtime_ms: i64,
    /// Coarse classification — see [`ArtifactKind`].
    pub artifact_kind: ArtifactKind,
    /// `true` when the filename matches the mesh-related heuristic.
    pub mesh_data: bool,
}

/// Return the three legacy roots called out by the 12.13 lock with
/// `$HOME` resolved. Falls back to `.` when `$HOME` is unset (CI /
/// sandboxed environments) so the function is always callable.
#[must_use]
pub fn default_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    vec![
        home.join(".config/mackes-shell"),
        home.join(".qnm-sync"),
        home.join(".cache/mackes"),
    ]
}

/// Filename substrings that count as "mesh data" for the
/// 12.13.1 inventory heuristic. Lowercased for case-insensitive
/// match.
const MESH_NEEDLES: &[&str] = &["mesh", "peer", "tailscale", "headscale", "qnm"];

/// Mesh-related filename heuristic. Case-insensitive substring match
/// against `mesh`, `peer`, `tailscale`, `headscale`, and `qnm` — the
/// terms that show up in every legacy module name today.
#[must_use]
pub fn is_mesh_related(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    MESH_NEEDLES.iter().any(|n| lower.contains(n))
}

/// Walk every root recursively (bounded to [`MAX_DEPTH`]) and return
/// a flat catalog of every regular file found. Missing roots are
/// silently skipped — a clean install simply produces an empty
/// inventory.
///
/// Symlinks are NOT followed. The walker is intentionally
/// best-effort: an unreadable directory or a transient I/O error on
/// `metadata()` skips that entry rather than aborting the whole scan.
#[must_use]
pub fn inventory(roots: &[PathBuf]) -> Vec<LegacyArtifact> {
    let mut out = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        walk(root, 0, &mut out);
    }
    out
}

/// Recursive helper. Pushes regular files into `out`; descends into
/// directories until `depth == MAX_DEPTH`.
fn walk(dir: &Path, depth: usize, out: &mut Vec<LegacyArtifact>) {
    // The root file itself is a legitimate (if unusual) inventory
    // target — if someone passes a single-file root, classify it.
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return;
    };
    if meta.file_type().is_symlink() {
        // Never follow symlinks. They aren't classified either —
        // following them risks loops + double-counting.
        return;
    }
    if meta.is_file() {
        out.push(classify(dir, &meta));
        return;
    }
    if !meta.is_dir() {
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(child_meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if child_meta.file_type().is_symlink() {
            continue;
        }
        if child_meta.is_dir() {
            walk(&path, depth + 1, out);
        } else if child_meta.is_file() {
            out.push(classify(&path, &child_meta));
        }
    }
}

/// Build a [`LegacyArtifact`] for a single regular file. Caller
/// has already confirmed the metadata corresponds to a file.
fn classify(path: &Path, meta: &fs::Metadata) -> LegacyArtifact {
    let size_bytes = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0);

    LegacyArtifact {
        path: path.to_path_buf(),
        size_bytes,
        mtime_ms,
        artifact_kind: kind_for(path),
        mesh_data: is_mesh_related(path),
    }
}

/// Classify by extension + path segment. The order of the matches
/// matters: a `.json` in a `cache` segment is a `JsonCache`, not a
/// `JsonConfig`, even if the file lives under `.config/`. That
/// matches today's layout (e.g.
/// `~/.config/mackes-shell/cache/peer-graph.json`).
fn kind_for(path: &Path) -> ArtifactKind {
    let in_cache_segment = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .any(|s| s.eq_ignore_ascii_case("cache") || s == ".cache");

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);

    match ext.as_deref() {
        Some("json") => {
            if in_cache_segment {
                ArtifactKind::JsonCache
            } else {
                ArtifactKind::JsonConfig
            }
        }
        Some("toml") => ArtifactKind::TomlConfig,
        _ if in_cache_segment => ArtifactKind::BinaryCache,
        _ => ArtifactKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    /// Empty roots → empty inventory. No I/O, no panics.
    #[test]
    fn empty_roots_yield_empty_inventory() {
        let inv = inventory(&[]);
        assert!(inv.is_empty());
    }

    /// Missing root paths are skipped silently — a clean install has
    /// no legacy files and should produce an empty inventory, not an
    /// error.
    #[test]
    fn missing_roots_are_skipped() {
        let inv = inventory(&[PathBuf::from("/nonexistent/path/that/will/not/be")]);
        assert!(inv.is_empty());
    }

    /// A single JSON config file is detected with the right kind +
    /// size + non-zero mtime, and is NOT flagged as mesh data when
    /// the filename has no mesh-y substring.
    #[test]
    fn detects_json_config_file() {
        let tmp = tempdir().expect("tempdir");
        let cfg_dir = tmp.path().join("mackes-shell");
        fs::create_dir(&cfg_dir).expect("mkdir");
        let f = cfg_dir.join("settings.json");
        let mut h = File::create(&f).expect("create");
        h.write_all(br#"{"theme":"hashbang"}"#).expect("write");
        drop(h);

        let inv = inventory(&[cfg_dir]);
        assert_eq!(inv.len(), 1);
        let a = &inv[0];
        assert_eq!(a.artifact_kind, ArtifactKind::JsonConfig);
        assert!(a.size_bytes > 0);
        assert!(a.mtime_ms > 0);
        assert!(!a.mesh_data);
    }

    /// A single TOML config file is detected as `TomlConfig`.
    #[test]
    fn detects_toml_config_file() {
        let tmp = tempdir().expect("tempdir");
        let f = tmp.path().join("pyproject.toml");
        File::create(&f)
            .expect("create")
            .write_all(b"[tool.mackes]\n")
            .expect("write");

        let inv = inventory(&[tmp.path().to_path_buf()]);
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].artifact_kind, ArtifactKind::TomlConfig);
    }

    /// Mesh-related filenames flip the `mesh_data` flag.
    #[test]
    fn flags_mesh_related_filenames() {
        let tmp = tempdir().expect("tempdir");
        for name in [
            "tailscale.state",
            "headscale-peers.json",
            "mesh.toml",
            "peer-graph.json",
        ] {
            File::create(tmp.path().join(name))
                .expect("create")
                .write_all(b"x")
                .expect("write");
        }

        let inv = inventory(&[tmp.path().to_path_buf()]);
        assert_eq!(inv.len(), 4);
        for a in &inv {
            assert!(
                a.mesh_data,
                "expected mesh_data=true for {}",
                a.path.display()
            );
        }
    }

    /// Non-mesh filenames stay `mesh_data = false`.
    #[test]
    fn non_mesh_filenames_are_not_flagged() {
        let tmp = tempdir().expect("tempdir");
        for name in ["pinned-apps.json", "preset.toml", "theme.json"] {
            File::create(tmp.path().join(name))
                .expect("create")
                .write_all(b"x")
                .expect("write");
        }

        let inv = inventory(&[tmp.path().to_path_buf()]);
        assert_eq!(inv.len(), 3);
        for a in &inv {
            assert!(
                !a.mesh_data,
                "expected mesh_data=false for {}",
                a.path.display()
            );
        }
    }

    /// `is_mesh_related` matches case-insensitively.
    #[test]
    fn mesh_match_is_case_insensitive() {
        assert!(is_mesh_related(Path::new("/x/Mesh.json")));
        assert!(is_mesh_related(Path::new("/x/HEADSCALE-peers.json")));
        assert!(is_mesh_related(Path::new("/x/TailScale.state")));
        assert!(!is_mesh_related(Path::new("/x/settings.json")));
    }

    /// Bounded recursion: a tree deeper than `MAX_DEPTH` only yields
    /// files at depths `<= MAX_DEPTH`. We build `MAX_DEPTH + 3`
    /// nested directories with a single file at each level, then
    /// verify the walker stops descending past `MAX_DEPTH`.
    #[test]
    fn recursion_is_bounded() {
        let tmp = tempdir().expect("tempdir");
        let mut cur = tmp.path().to_path_buf();
        let total = MAX_DEPTH + 3;
        for i in 0..total {
            cur = cur.join(format!("d{i}"));
            fs::create_dir(&cur).expect("mkdir");
            File::create(cur.join(format!("f{i}.json")))
                .expect("create")
                .write_all(b"{}")
                .expect("write");
        }

        let inv = inventory(&[tmp.path().to_path_buf()]);
        // The walker enters the root (depth 0), so it can list and
        // classify files at depths 1..=MAX_DEPTH. Anything deeper is
        // dropped.
        assert!(
            inv.len() <= MAX_DEPTH,
            "walker descended past MAX_DEPTH: found {} files (MAX_DEPTH={})",
            inv.len(),
            MAX_DEPTH,
        );
        assert!(
            !inv.is_empty(),
            "walker bailed too early: found 0 files in a {total}-level tree",
        );
    }

    /// JSON files under a `cache/` segment are classified
    /// `JsonCache`, not `JsonConfig`. Mirrors today's layout where
    /// `~/.config/mackes-shell/cache/peer-graph.json` is a cache.
    #[test]
    fn json_inside_cache_segment_is_cache() {
        let tmp = tempdir().expect("tempdir");
        let cache = tmp.path().join("mackes-shell").join("cache");
        fs::create_dir_all(&cache).expect("mkdir");
        File::create(cache.join("peer-graph.json"))
            .expect("create")
            .write_all(b"{}")
            .expect("write");

        let inv = inventory(&[tmp.path().to_path_buf()]);
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].artifact_kind, ArtifactKind::JsonCache);
        assert!(inv[0].mesh_data, "peer-graph.json should be mesh-related");
    }

    /// Binary blobs under a cache segment fall to `BinaryCache`;
    /// extensionless or unknown files outside cache fall to
    /// `Unknown`.
    #[test]
    fn classifies_binary_and_unknown() {
        let tmp = tempdir().expect("tempdir");
        let cache = tmp.path().join("cache");
        fs::create_dir(&cache).expect("mkdir");
        File::create(cache.join("blob.bin"))
            .expect("create")
            .write_all(b"\x00\x01")
            .expect("write");
        File::create(tmp.path().join("README"))
            .expect("create")
            .write_all(b"hi")
            .expect("write");

        let inv = inventory(&[tmp.path().to_path_buf()]);
        let kinds: Vec<_> = inv.iter().map(|a| a.artifact_kind).collect();
        assert!(kinds.contains(&ArtifactKind::BinaryCache));
        assert!(kinds.contains(&ArtifactKind::Unknown));
    }

    /// `default_roots` always returns three entries, even when
    /// `$HOME` is unset — important so the CLI can render a stable
    /// summary in CI environments.
    #[test]
    fn default_roots_has_three_entries() {
        assert_eq!(default_roots().len(), 3);
    }
}

//! MEDIA-7: the local media library — a browsable index of chosen folders.
//!
//! Where the [`Player`](crate::Player) folds config to mpv, the library is a
//! **pure model** (like the MEDIA-6 [`Playlist`](crate::Playlist)): a set of chosen
//! root folders walked for media files, each recorded as a [`LibraryItem`] with a
//! typed [`MediaMetadata`] record, kept in a browsable / searchable / sortable
//! in-memory index and persisted with [`serde`] (JSON). It owns *what media exists
//! locally* — none of it needs an engine, so all of it is unit-tested with no mpv.
//!
//! # What is indexed
//!
//! [`Library::index_folder`] records a chosen root and walks it (a dependency-free
//! `std::fs` recursive walk — symlinks are not followed, so a self-referential link
//! cannot loop the scan). Every file whose extension is a known media type
//! ([`MediaKind::from_extension`]) becomes a [`LibraryItem`]; re-indexing the same
//! root **merges** (existing items keep their original date-added order and are
//! metadata-refreshed, new files are appended), so a rescan never loses history.
//!
//! # Metadata (§6 honest scope)
//!
//! [`MediaMetadata::from_path`] derives the two facts the filesystem itself carries
//! — a display `title` (the cleaned file stem) and the [`MediaKind`] (from the
//! extension) — with **no dependency**. The richer `artist` / `album` /
//! `duration_secs` fields are typed on the record and the browse folds sort + search
//! by them, but populating them from *embedded tags* (or an online `TMDB`/`TVDB`
//! scrape) needs a tag/probe dependency or network egress; that enrichment is
//! honest-gated exactly like the crate's `opensubtitles` egress (see the crate
//! `Cargo.toml`) and is applied through [`MediaMetadata::with_artist`] /
//! [`with_album`](MediaMetadata::with_album) /
//! [`with_duration`](MediaMetadata::with_duration) — e.g. the surface backfills a
//! real `duration_secs` from the [`Player`](crate::Player)'s learned duration.
//!
//! # Browsing
//!
//! [`Library::browse`] applies a [`BrowseQuery`] — a case-insensitive `search`
//! substring, an optional [`MediaKind`] filter, and a [`SortKey`] (ascending or
//! descending) — returning the matching items in order. It is a pure fold over the
//! index, so it is fully unit-tested.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Whether an indexed item is audio or video, from its file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// An audio file (`.mp3`, `.flac`, `.opus`, …).
    Audio,
    /// A video file (`.mkv`, `.mp4`, `.webm`, …).
    Video,
}

impl MediaKind {
    /// Classify a file extension (case-insensitive, no leading dot) as a known
    /// media type, or [`None`] when it is not one this player indexes.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        const AUDIO: &[&str] = &[
            "mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "aac", "wma", "alac", "aiff", "aif",
            "ape", "mka", "wv", "mpc",
        ];
        const VIDEO: &[&str] = &[
            "mkv", "mp4", "m4v", "avi", "mov", "wmv", "flv", "webm", "mpg", "mpeg", "ts", "m2ts",
            "mts", "vob", "ogv", "3gp", "divx",
        ];
        let ext = ext.to_ascii_lowercase();
        if AUDIO.contains(&ext.as_str()) {
            Some(Self::Audio)
        } else if VIDEO.contains(&ext.as_str()) {
            Some(Self::Video)
        } else {
            None
        }
    }
}

/// The typed metadata of one indexed media file.
///
/// `title` + `kind` are always derived from the path ([`MediaMetadata::from_path`]);
/// `artist` / `album` / `duration_secs` are typed fields the browse folds honour but
/// which are populated by enrichment (embedded tags / a probe / the player's learned
/// duration) — see the [module docs](self).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// A human display title — the cleaned file stem, unless enriched.
    pub title: String,
    /// The performing artist, if known (embedded-tag / probe enrichment).
    pub artist: Option<String>,
    /// The album / collection, if known (embedded-tag / probe enrichment).
    pub album: Option<String>,
    /// The runtime in seconds, if known (probe / the player's learned duration).
    pub duration_secs: Option<f64>,
    /// Whether this is an audio or a video item.
    pub kind: MediaKind,
}

impl MediaMetadata {
    /// Derive the metadata carried by the `path` itself — a cleaned-stem `title` and
    /// the [`MediaKind`] from the extension — or [`None`] when the extension is not a
    /// media type this player indexes.
    ///
    /// This is dependency-free: it reads nothing but the path. `artist` / `album` /
    /// `duration_secs` are left [`None`] for later enrichment.
    #[must_use]
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        let path = path.as_ref();
        let ext = path.extension().and_then(|e| e.to_str())?;
        let kind = MediaKind::from_extension(ext)?;
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map_or_else(|| "untitled".to_owned(), clean_title);
        Some(Self {
            title,
            artist: None,
            album: None,
            duration_secs: None,
            kind,
        })
    }

    /// Replace the display `title` (enrichment override).
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the `artist` (embedded-tag / probe enrichment).
    #[must_use]
    pub fn with_artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    /// Set the `album` (embedded-tag / probe enrichment).
    #[must_use]
    pub fn with_album(mut self, album: impl Into<String>) -> Self {
        self.album = Some(album.into());
        self
    }

    /// Set the `duration_secs` (probe / the player's learned duration).
    #[must_use]
    pub const fn with_duration(mut self, secs: f64) -> Self {
        self.duration_secs = Some(secs);
        self
    }
}

/// Clean a raw file stem into a display title: underscores → spaces, runs of
/// whitespace collapsed, trimmed. Deterministic, so it is unit-testable.
fn clean_title(stem: &str) -> String {
    let spaced = stem.replace(['_'], " ");
    let cleaned = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        stem.to_owned()
    } else {
        cleaned
    }
}

/// One entry in the [`Library`] — a media path plus its typed [`MediaMetadata`] and
/// the order in which it entered the index (its "date added" proxy).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryItem {
    /// The media path (the index key + what [`Player::load`](crate::Player::load)
    /// opens).
    pub path: String,
    /// The item's typed metadata.
    pub metadata: MediaMetadata,
    /// A monotonically increasing sequence assigned when the item was first indexed
    /// — the deterministic "date added" order (survives a re-index / save-load).
    pub added_seq: u64,
}

/// Which field [`Library::browse`] sorts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    /// By display title (case-insensitive).
    #[default]
    Title,
    /// By artist (case-insensitive; unknown artists sort last ascending).
    Artist,
    /// By album (case-insensitive; unknown albums sort last ascending).
    Album,
    /// By runtime (unknown durations sort last ascending).
    Duration,
    /// By the order the item entered the library (date added).
    DateAdded,
}

/// A browse request over the [`Library`] — a case-insensitive `search`, an optional
/// [`MediaKind`] filter, and a [`SortKey`] with a direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BrowseQuery {
    /// A case-insensitive substring matched against title / artist / album / path;
    /// [`None`] (or empty) matches everything.
    pub search: Option<String>,
    /// Restrict to one [`MediaKind`]; [`None`] returns both.
    pub kind: Option<MediaKind>,
    /// The field to sort on.
    pub sort: SortKey,
    /// Sort descending instead of ascending.
    pub descending: bool,
}

impl BrowseQuery {
    /// An everything-query in the default sort (title ascending).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// This query with its `search` needle set.
    #[must_use]
    pub fn with_search(mut self, needle: impl Into<String>) -> Self {
        self.search = Some(needle.into());
        self
    }

    /// This query restricted to one [`MediaKind`].
    #[must_use]
    pub const fn with_kind(mut self, kind: MediaKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// This query sorted on `sort`, ascending.
    #[must_use]
    pub const fn sorted_by(mut self, sort: SortKey) -> Self {
        self.sort = sort;
        self
    }

    /// This query in descending order.
    #[must_use]
    pub const fn descending(mut self) -> Self {
        self.descending = true;
        self
    }
}

/// The local media library — the browsable, serde-persisted index (MEDIA-7).
///
/// Chosen root folders ([`roots`](Self::roots)) are walked into an index of
/// [`LibraryItem`]s keyed by path. All queries are pure; the surface (MEDIA-8) reads
/// the browse folds. Save/load is [`serde`] JSON, mirroring
/// [`Playlist`](crate::Playlist).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Library {
    /// The indexed items, keyed by path (a [`BTreeMap`] so iteration + serialization
    /// are deterministic).
    items: BTreeMap<String, LibraryItem>,
    /// The chosen root folders that have been indexed, in the order first added.
    roots: Vec<String>,
    /// The next `added_seq` to hand out (monotonic across the library's lifetime).
    next_seq: u64,
}

impl Library {
    /// An empty library: no roots, no items.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ── accessors ────────────────────────────────────────────────────────────

    /// The chosen root folders that have been indexed.
    #[must_use]
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// The number of indexed items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the library has no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The item at `path`, if indexed.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&LibraryItem> {
        self.items.get(path)
    }

    /// Whether `path` is indexed.
    #[must_use]
    pub fn contains(&self, path: &str) -> bool {
        self.items.contains_key(path)
    }

    /// All indexed items, in path order.
    pub fn items(&self) -> impl Iterator<Item = &LibraryItem> {
        self.items.values()
    }

    // ── mutation ─────────────────────────────────────────────────────────────

    /// Record a chosen root folder (deduplicated), without walking it.
    pub fn add_root(&mut self, root: impl Into<String>) {
        let root = root.into();
        if !self.roots.contains(&root) {
            self.roots.push(root);
        }
    }

    /// Insert or metadata-refresh the item at `path`. A brand-new path is appended
    /// with the next date-added sequence; an existing path keeps its original
    /// sequence and takes the new `metadata` (a merge). Returns `true` when the item
    /// was newly added.
    pub fn upsert(&mut self, path: impl Into<String>, metadata: MediaMetadata) -> bool {
        let path = path.into();
        if let Some(existing) = self.items.get_mut(&path) {
            existing.metadata = metadata;
            false
        } else {
            let added_seq = self.next_seq;
            self.next_seq += 1;
            self.items.insert(
                path.clone(),
                LibraryItem {
                    path,
                    metadata,
                    added_seq,
                },
            );
            true
        }
    }

    /// Remove the item at `path`, returning it if it was indexed.
    pub fn remove(&mut self, path: &str) -> Option<LibraryItem> {
        self.items.remove(path)
    }

    /// Index the chosen `root` folder: record it as a root, walk it for media files
    /// (recursively, not following symlinks), and [`upsert`](Self::upsert) each.
    /// Returns the number of **new** items added (a rescan that finds nothing new
    /// returns `0`). Re-indexing merges — existing items keep their date-added order.
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] when the chosen `root` itself cannot be read
    /// (missing / not a directory / permission). Unreadable *sub*directories under a
    /// readable root are skipped (fail-soft), so one bad child never aborts the scan.
    pub fn index_folder(&mut self, root: impl AsRef<Path>) -> std::io::Result<usize> {
        let root = root.as_ref();
        // Surface a hard error only when the chosen root itself is unreadable.
        std::fs::read_dir(root)?;
        self.add_root(root.to_string_lossy().into_owned());
        let mut files = Vec::new();
        walk(root, &mut files);
        // Deterministic add order (so `added_seq` / DateAdded is reproducible).
        files.sort();
        let mut added = 0;
        for file in files {
            if let Some(metadata) = MediaMetadata::from_path(&file) {
                let key = file.to_string_lossy().into_owned();
                if self.upsert(key, metadata) {
                    added += 1;
                }
            }
        }
        Ok(added)
    }

    // ── browsing ─────────────────────────────────────────────────────────────

    /// Browse the index through a [`BrowseQuery`] — filter by `search` + `kind`, then
    /// sort by the [`SortKey`] in the chosen direction. A pure fold; the returned
    /// items borrow from the library.
    #[must_use]
    pub fn browse(&self, query: &BrowseQuery) -> Vec<&LibraryItem> {
        let needle = query
            .search
            .as_deref()
            .map(str::to_lowercase)
            .filter(|s| !s.is_empty());
        let mut out: Vec<&LibraryItem> = self
            .items
            .values()
            .filter(|item| query.kind.is_none_or(|k| item.metadata.kind == k))
            .filter(|item| needle.as_deref().is_none_or(|n| item_matches(item, n)))
            .collect();
        out.sort_by(|a, b| {
            let ord = cmp_by(a, b, query.sort).then_with(|| a.path.cmp(&b.path));
            if query.descending {
                ord.reverse()
            } else {
                ord
            }
        });
        out
    }

    /// Convenience: the [`browse`](Self::browse) results for a case-insensitive
    /// `needle` search in the default sort (title ascending).
    #[must_use]
    pub fn search(&self, needle: &str) -> Vec<&LibraryItem> {
        self.browse(&BrowseQuery::new().with_search(needle))
    }

    // ── persistence ──────────────────────────────────────────────────────────

    /// Save the library to `path` as pretty JSON (persist the index).
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] if the file cannot be written.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Load a library from a JSON `path` written by [`save`](Self::save).
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] if the file cannot be read, or a mapped error
    /// if its contents are not a valid serialized [`Library`].
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json).map_err(std::io::Error::other)
    }
}

/// Whether `item` matches a lowercase search `needle` (title / artist / album / path).
fn item_matches(item: &LibraryItem, needle: &str) -> bool {
    let m = &item.metadata;
    m.title.to_lowercase().contains(needle)
        || m.artist
            .as_deref()
            .is_some_and(|a| a.to_lowercase().contains(needle))
        || m.album
            .as_deref()
            .is_some_and(|a| a.to_lowercase().contains(needle))
        || item.path.to_lowercase().contains(needle)
}

/// Compare two items by a [`SortKey`] (ascending). Unknown text/duration sorts last;
/// the caller applies a stable path tiebreak + the descending flip.
fn cmp_by(a: &LibraryItem, b: &LibraryItem, key: SortKey) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match key {
        SortKey::Title => a
            .metadata
            .title
            .to_lowercase()
            .cmp(&b.metadata.title.to_lowercase()),
        SortKey::Artist => cmp_opt_text(a.metadata.artist.as_deref(), b.metadata.artist.as_deref()),
        SortKey::Album => cmp_opt_text(a.metadata.album.as_deref(), b.metadata.album.as_deref()),
        SortKey::Duration => match (a.metadata.duration_secs, b.metadata.duration_secs) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
        SortKey::DateAdded => a.added_seq.cmp(&b.added_seq),
    }
}

/// Case-insensitive compare of two optional text fields, unknown (`None`) last.
fn cmp_opt_text(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Recursively collect the files under `dir` into `out` (a dependency-free walk).
/// Symlinks are not followed (so a self-referential link cannot loop the scan), and
/// an unreadable subdirectory is skipped rather than aborting the walk (fail-soft).
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(&entry.path(), out);
        } else if file_type.is_file() {
            out.push(entry.path());
        }
        // A symlink is neither `is_dir()` nor `is_file()` here (file_type does not
        // follow it), so it is intentionally skipped.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(title: &str) -> MediaMetadata {
        MediaMetadata {
            title: title.to_owned(),
            artist: None,
            album: None,
            duration_secs: None,
            kind: MediaKind::Audio,
        }
    }

    // ── metadata derivation ──────────────────────────────────────────────────

    #[test]
    fn from_path_derives_title_and_kind() {
        let m = MediaMetadata::from_path("/music/The_Best_Song.flac").expect("known ext");
        assert_eq!(m.title, "The Best Song");
        assert_eq!(m.kind, MediaKind::Audio);
        assert_eq!(m.artist, None);
        assert_eq!(m.duration_secs, None);

        let v = MediaMetadata::from_path("/films/Movie.Title.mkv").expect("known ext");
        assert_eq!(v.kind, MediaKind::Video);
    }

    #[test]
    fn from_path_rejects_non_media_extensions() {
        assert_eq!(MediaMetadata::from_path("/notes/readme.txt"), None);
        assert_eq!(MediaMetadata::from_path("/x/no_extension"), None);
    }

    #[test]
    fn extension_classification_is_case_insensitive() {
        assert_eq!(MediaKind::from_extension("MP3"), Some(MediaKind::Audio));
        assert_eq!(MediaKind::from_extension("MkV"), Some(MediaKind::Video));
        assert_eq!(MediaKind::from_extension("bin"), None);
    }

    #[test]
    fn enrichment_builders_populate_the_optional_fields() {
        let m = MediaMetadata::from_path("/m/track.mp3")
            .expect("audio")
            .with_title("Real Title")
            .with_artist("Artist")
            .with_album("Album")
            .with_duration(210.5);
        assert_eq!(m.title, "Real Title");
        assert_eq!(m.artist.as_deref(), Some("Artist"));
        assert_eq!(m.album.as_deref(), Some("Album"));
        assert_eq!(m.duration_secs, Some(210.5));
    }

    // ── upsert / merge ───────────────────────────────────────────────────────

    #[test]
    fn upsert_adds_then_merges_preserving_date_added() {
        let mut lib = Library::new();
        assert!(lib.upsert("/a.mp3", audio("A")));
        assert!(lib.upsert("/b.mp3", audio("B")));
        let a_seq = lib.get("/a.mp3").expect("a").added_seq;

        // Re-upsert "/a.mp3" with new metadata → merge (not a new add), same seq.
        assert!(!lib.upsert("/a.mp3", audio("A (remaster)")));
        assert_eq!(lib.len(), 2);
        assert_eq!(lib.get("/a.mp3").expect("a").metadata.title, "A (remaster)");
        assert_eq!(lib.get("/a.mp3").expect("a").added_seq, a_seq);
        // "/b.mp3" was added after "/a.mp3".
        assert!(lib.get("/b.mp3").expect("b").added_seq > a_seq);
    }

    #[test]
    fn remove_drops_the_item() {
        let mut lib = Library::new();
        lib.upsert("/a.mp3", audio("A"));
        assert!(lib.contains("/a.mp3"));
        assert_eq!(lib.remove("/a.mp3").expect("removed").path, "/a.mp3");
        assert!(!lib.contains("/a.mp3"));
        assert_eq!(lib.remove("/a.mp3"), None);
    }

    // ── browse: filter / search / sort ───────────────────────────────────────

    fn sample() -> Library {
        let mut lib = Library::new();
        lib.upsert(
            "/m/beta.mp3",
            audio("Beta")
                .with_artist("Zephyr")
                .with_album("Nights")
                .with_duration(180.0),
        );
        lib.upsert(
            "/m/alpha.mp3",
            audio("Alpha")
                .with_artist("Aurora")
                .with_album("Dawn")
                .with_duration(240.0),
        );
        lib.upsert(
            "/v/clip.mkv",
            MediaMetadata {
                title: "Gamma".to_owned(),
                artist: None,
                album: None,
                duration_secs: Some(60.0),
                kind: MediaKind::Video,
            },
        );
        lib
    }

    fn titles(items: &[&LibraryItem]) -> Vec<String> {
        items.iter().map(|i| i.metadata.title.clone()).collect()
    }

    #[test]
    fn browse_sorts_by_title_ascending_by_default() {
        let lib = sample();
        let out = lib.browse(&BrowseQuery::new());
        assert_eq!(titles(&out), vec!["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn browse_sorts_descending_when_asked() {
        let lib = sample();
        let out = lib.browse(&BrowseQuery::new().sorted_by(SortKey::Title).descending());
        assert_eq!(titles(&out), vec!["Gamma", "Beta", "Alpha"]);
    }

    #[test]
    fn browse_filters_by_kind() {
        let lib = sample();
        let audio_only = lib.browse(&BrowseQuery::new().with_kind(MediaKind::Audio));
        assert_eq!(titles(&audio_only), vec!["Alpha", "Beta"]);
        let video_only = lib.browse(&BrowseQuery::new().with_kind(MediaKind::Video));
        assert_eq!(titles(&video_only), vec!["Gamma"]);
    }

    #[test]
    fn search_matches_title_artist_album_and_path() {
        let lib = sample();
        // Artist substring, case-insensitive.
        assert_eq!(titles(&lib.search("aurora")), vec!["Alpha"]);
        // Album substring.
        assert_eq!(titles(&lib.search("nights")), vec!["Beta"]);
        // Path substring ("/v/clip.mkv").
        assert_eq!(titles(&lib.search("clip")), vec!["Gamma"]);
        // No match → empty.
        assert!(lib.search("zzz").is_empty());
        // Empty needle matches everything.
        assert_eq!(lib.search("").len(), 3);
    }

    #[test]
    fn browse_sorts_by_artist_with_unknown_last() {
        let lib = sample();
        let out = lib.browse(&BrowseQuery::new().sorted_by(SortKey::Artist));
        // Aurora, Zephyr, then the video (no artist) last.
        assert_eq!(titles(&out), vec!["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn browse_sorts_by_duration() {
        let lib = sample();
        let out = lib.browse(&BrowseQuery::new().sorted_by(SortKey::Duration));
        // 60 (Gamma) < 180 (Beta) < 240 (Alpha).
        assert_eq!(titles(&out), vec!["Gamma", "Beta", "Alpha"]);
    }

    #[test]
    fn browse_sorts_by_date_added() {
        let lib = sample();
        let out = lib.browse(&BrowseQuery::new().sorted_by(SortKey::DateAdded));
        // Insertion order: beta, alpha, clip.
        assert_eq!(titles(&out), vec!["Beta", "Alpha", "Gamma"]);
    }

    // ── filesystem index / scan ──────────────────────────────────────────────

    fn temp_dir(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mde-media-lib-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        path
    }

    #[test]
    fn index_folder_walks_recursively_and_skips_non_media() {
        let root = temp_dir("scan");
        std::fs::create_dir_all(root.join("sub")).expect("mkdir");
        std::fs::write(root.join("song.mp3"), b"x").expect("write");
        std::fs::write(root.join("readme.txt"), b"x").expect("write");
        std::fs::write(root.join("sub").join("movie.mkv"), b"x").expect("write");

        let mut lib = Library::new();
        let added = lib.index_folder(&root).expect("index");
        assert_eq!(added, 2, "the two media files, not the .txt");
        assert_eq!(lib.len(), 2);
        assert!(lib.roots().iter().any(|r| r == &root.to_string_lossy()));
        // The nested media file is indexed as video.
        let video = lib.browse(&BrowseQuery::new().with_kind(MediaKind::Video));
        assert_eq!(video.len(), 1);
        assert_eq!(video[0].metadata.title, "movie");

        // Rescan finds nothing new (merge, not duplicate).
        let again = lib.index_folder(&root).expect("reindex");
        assert_eq!(again, 0);
        assert_eq!(lib.len(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn index_missing_root_is_an_error() {
        let mut lib = Library::new();
        let missing = temp_dir("nope");
        assert!(lib.index_folder(&missing).is_err());
    }

    // ── persistence ──────────────────────────────────────────────────────────

    #[test]
    fn save_and_load_round_trip_through_a_file() {
        let lib = sample();
        let path = temp_dir("save").with_extension("json");
        lib.save(&path).expect("save");
        let back = Library::load(&path).expect("load");
        let _ = std::fs::remove_file(&path);
        assert_eq!(lib, back);
        // The browse fold is identical after a round-trip.
        assert_eq!(
            titles(&lib.browse(&BrowseQuery::new())),
            titles(&back.browse(&BrowseQuery::new()))
        );
    }
}

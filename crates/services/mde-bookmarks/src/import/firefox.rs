//! Firefox `places.sqlite` importer — bookmarks ONLY (operator security lock).
//!
//! ## Security posture (never read logins/cookies/history)
//!
//! * The database is opened **read-only + immutable**:
//!   `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI` with the `immutable=1` URI param.
//!   `SQLite` then never writes, never takes a lock, and never touches the
//!   `-wal`/`-shm` sidecars — the user's live profile is untouched.
//! * If the immutable open fails (a corrupt/locked file), we copy `places.sqlite`
//!   (and only its own sidecars) into a private tempdir and open the COPY
//!   read-only. The original is never mutated.
//! * We query ONLY `moz_bookmarks` (the tree) and `moz_places` (the URL of a
//!   bookmarked row, reached by `moz_bookmarks.fk`). We JOIN so only URLs that
//!   are *bookmarked* are read — never a history-only `moz_places` row, never
//!   `moz_historyvisits`, and nothing from the separate
//!   `cookies.sqlite` / `logins.json` / `key4.db` files.
//! * [`ensure_schema`] is the backstop: if a wrongly-detected `SQLite` file (e.g.
//!   `cookies.sqlite`) is passed in, the missing `moz_bookmarks` table makes the
//!   import error out BEFORE any content query runs — so cookie/login tables are
//!   never selected from even on a misdetect.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use url::Url;

use super::parsed::{ParsedBookmark, ParsedNode, ParsedTree};
use super::ImportError;
use crate::Source;

/// The Firefox places-root container guid (its children are the named roots).
const GUID_ROOT: &str = "root________";
/// The Firefox tags-root guid (its subtree encodes tags, mapped to `tags[]`).
const GUID_TAGS: &str = "tags________";

/// A friendly label for a Firefox root (whose stored title is empty/localized).
fn root_label(guid: &str, title: &str) -> String {
    match guid {
        "menu________" => "Bookmarks Menu".to_string(),
        "toolbar_____" => "Bookmarks Toolbar".to_string(),
        "unfiled_____" => "Other Bookmarks".to_string(),
        "mobile______" => "Mobile Bookmarks".to_string(),
        _ => title.to_string(),
    }
}

/// Parse a `places.sqlite` file into a [`ParsedTree`] (bookmarks only).
pub fn parse_places(path: &Path) -> Result<ParsedTree, ImportError> {
    // Prefer the immutable read-only open; fall back to a private copy only if
    // that open itself fails (locked/WAL). A *schema* error must NOT fall back —
    // a copy would not add the missing tables.
    if let Ok(conn) = open_ro_immutable(path) {
        return read_tree(&conn);
    }
    // `_dir` (the tempdir) must outlive `read_tree`, so keep it bound.
    let (conn, _dir) = open_via_copy(path)?;
    read_tree(&conn)
}

/// Open `path` read-only + immutable via a `file:…?immutable=1` URI.
fn open_ro_immutable(path: &Path) -> Result<Connection, ImportError> {
    let mut uri = Url::from_file_path(path).map_err(|()| ImportError::UnknownFormat)?;
    uri.set_query(Some("immutable=1"));
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    Connection::open_with_flags(uri.as_str(), flags).map_err(ImportError::from)
}

/// Copy `places.sqlite` (and only its own `-wal`/`-shm` sidecars) into a private
/// tempdir and open the copy read-only. The original is never touched.
fn open_via_copy(path: &Path) -> Result<(Connection, tempfile::TempDir), ImportError> {
    let dir = tempfile::tempdir()?;
    let name = path
        .file_name()
        .map_or_else(|| PathBuf::from("places.sqlite"), PathBuf::from);
    let dst = dir.path().join(&name);
    fs::copy(path, &dst)?;
    for suffix in ["-wal", "-shm"] {
        let mut src = path.as_os_str().to_os_string();
        src.push(suffix);
        let src = PathBuf::from(src);
        if src.exists() {
            let mut d = dst.as_os_str().to_os_string();
            d.push(suffix);
            // Best-effort: a missing sidecar just means no WAL to apply.
            let _ = fs::copy(&src, PathBuf::from(d));
        }
    }
    let conn = Connection::open_with_flags(&dst, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(ImportError::from)?;
    Ok((conn, dir))
}

/// Read the bookmark tree from an open connection.
fn read_tree(conn: &Connection) -> Result<ParsedTree, ImportError> {
    ensure_schema(conn)?;
    let tags = read_tags(conn)?;
    let folders = read_folders(conn)?;
    let bookmarks = read_bookmarks(conn, &tags)?;
    let roots = assemble(&folders, &bookmarks);
    Ok(ParsedTree {
        source: Source::Firefox,
        roots,
    })
}

/// Backstop: require the bookmark tables before any content query runs, so a
/// misdetected `cookies.sqlite`/`logins`-style db errors out untouched.
fn ensure_schema(conn: &Connection) -> Result<(), ImportError> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    let has_bookmarks = stmt.exists(["moz_bookmarks"])?;
    let has_places = stmt.exists(["moz_places"])?;
    if has_bookmarks && has_places {
        Ok(())
    } else {
        Err(ImportError::MissingBookmarkTables)
    }
}

/// `place id -> sorted tag names`, read from the tags subtree (lock Q11). A
/// tagged entry is a `type=1` row whose parent is a tag folder (a `type=2` child
/// of the tags root); the tag name is that folder's title.
fn read_tags(conn: &Connection) -> Result<BTreeMap<i64, Vec<String>>, ImportError> {
    let sql = "SELECT tagged.fk AS place_id, tagfolder.title AS tag \
               FROM moz_bookmarks AS tagged \
               JOIN moz_bookmarks AS tagfolder ON tagged.parent = tagfolder.id \
               JOIN moz_bookmarks AS tagroot ON tagfolder.parent = tagroot.id \
               WHERE tagroot.guid = ?1 AND tagged.type = 1 AND tagfolder.type = 2";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([GUID_TAGS], |r| {
        Ok((r.get::<_, i64>("place_id")?, r.get::<_, String>("tag")?))
    })?;
    let mut map: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for row in rows {
        let (place_id, tag) = row?;
        let tag = tag.trim().to_string();
        if tag.is_empty() {
            continue;
        }
        let entry = map.entry(place_id).or_default();
        if !entry.contains(&tag) {
            entry.push(tag);
        }
    }
    for tags in map.values_mut() {
        tags.sort();
    }
    Ok(map)
}

/// A raw `moz_bookmarks` folder row.
struct RawFolder {
    id: i64,
    parent: i64,
    position: i64,
    title: String,
    guid: String,
}

/// Read all folder rows (`type = 2`).
fn read_folders(conn: &Connection) -> Result<Vec<RawFolder>, ImportError> {
    let sql = "SELECT id, IFNULL(parent, 0) AS parent, IFNULL(position, 0) AS position, \
               IFNULL(title, '') AS title, IFNULL(guid, '') AS guid \
               FROM moz_bookmarks WHERE type = 2";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(RawFolder {
            id: r.get("id")?,
            parent: r.get("parent")?,
            position: r.get("position")?,
            title: r.get("title")?,
            guid: r.get("guid")?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// A raw bookmark row joined to its `moz_places` URL.
struct RawBookmark {
    parent: i64,
    position: i64,
    title: String,
    url: String,
    tags: Vec<String>,
    added_ms: u64,
}

/// Read all bookmark rows (`type = 1`) joined to `moz_places` by `fk` — so only
/// bookmarked URLs are read, never history-only rows.
fn read_bookmarks(
    conn: &Connection,
    tags: &BTreeMap<i64, Vec<String>>,
) -> Result<Vec<RawBookmark>, ImportError> {
    let sql = "SELECT b.parent AS parent, IFNULL(b.position, 0) AS position, \
               IFNULL(b.title, '') AS title, IFNULL(b.dateAdded, 0) AS date_added, \
               b.fk AS place_id, p.url AS url \
               FROM moz_bookmarks AS b \
               JOIN moz_places AS p ON b.fk = p.id \
               WHERE b.type = 1";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>("parent")?,
            r.get::<_, i64>("position")?,
            r.get::<_, String>("title")?,
            r.get::<_, i64>("date_added")?,
            r.get::<_, i64>("place_id")?,
            r.get::<_, String>("url")?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (parent, position, title, date_added, place_id, url) = row?;
        // Firefox internal smart-folder queries are not real links.
        if url.starts_with("place:") {
            continue;
        }
        // dateAdded is microseconds since the epoch → milliseconds.
        let added_ms = u64::try_from(date_added).unwrap_or(0) / 1000;
        out.push(RawBookmark {
            parent,
            position,
            title,
            url,
            tags: tags.get(&place_id).cloned().unwrap_or_default(),
            added_ms,
        });
    }
    Ok(out)
}

/// One child of a parent, ordered by Firefox `position` (folders + bookmarks
/// share a single position sequence within a parent).
enum Child<'a> {
    Folder(&'a RawFolder),
    Bookmark(&'a RawBookmark),
}

/// Build the visible parsed tree, excluding the root container and the tags
/// subtree (tags became `tags[]` already).
fn assemble(folders: &[RawFolder], bookmarks: &[RawBookmark]) -> Vec<ParsedNode> {
    let root_id = folders.iter().find(|f| f.guid == GUID_ROOT).map(|f| f.id);
    let tags_root_id = folders.iter().find(|f| f.guid == GUID_TAGS).map(|f| f.id);

    // parent id -> its children (folders + bookmarks), sorted by position.
    let mut index: BTreeMap<i64, Vec<(i64, Child)>> = BTreeMap::new();
    for f in folders {
        // Hide the root container itself and the whole tags subtree.
        if Some(f.id) == root_id || Some(f.id) == tags_root_id {
            continue;
        }
        if Some(f.parent) == tags_root_id {
            continue;
        }
        index
            .entry(f.parent)
            .or_default()
            .push((f.position, Child::Folder(f)));
    }
    for b in bookmarks {
        // Bookmarks directly under a tag folder are tag assignments, not links —
        // their fk was already turned into `tags[]`. They live under folders
        // whose parent is the tags root, so exclude those parents.
        index
            .entry(b.parent)
            .or_default()
            .push((b.position, Child::Bookmark(b)));
    }
    for kids in index.values_mut() {
        kids.sort_by(|a, b| a.0.cmp(&b.0));
    }
    // Drop tag-folder buckets so their child bookmarks never surface.
    if let Some(troot) = tags_root_id {
        let tag_folder_ids: Vec<i64> = folders
            .iter()
            .filter(|f| f.parent == troot)
            .map(|f| f.id)
            .collect();
        for id in tag_folder_ids {
            index.remove(&id);
        }
    }

    let top = root_id.unwrap_or(0);
    build(top, &index)
}

/// Recursively materialize the children of parent id `pid`.
fn build(pid: i64, index: &BTreeMap<i64, Vec<(i64, Child)>>) -> Vec<ParsedNode> {
    let Some(kids) = index.get(&pid) else {
        return Vec::new();
    };
    kids.iter()
        .map(|(_, child)| match child {
            Child::Folder(f) => ParsedNode::Folder {
                name: root_label(&f.guid, &f.title),
                children: build(f.id, index),
            },
            Child::Bookmark(b) => ParsedNode::Bookmark(ParsedBookmark {
                url: b.url.clone(),
                title: b.title.clone(),
                tags: b.tags.clone(),
                added_ms: b.added_ms,
            }),
        })
        .collect()
}

//! BOOKMARKS-3 importer acceptance (fixture-tested per format).
//!
//! Each format's fixture is imported into a [`Collection`], the planned ops are
//! applied, and the converged tree is asserted — proving the importers land
//! bookmarks under `Imported/<Browser>`, map Firefox tags → `tags[]` and
//! Chromium roots → named subfolders, dedup by normalized URL, re-import
//! idempotently, and (the operator lock) NEVER surface logins/cookies/history.

use std::path::{Path, PathBuf};

use mde_bookmarks::{
    import_file, import_file_as, Author, Bookmark, Collection, HlcClock, Item, Source,
};
use uuid::Uuid;

const NOW_MS: u64 = 1_700_000_000_000;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn clock() -> HlcClock {
    HlcClock::new("test-node".to_string())
}

fn author() -> Author {
    Author::new("alice".to_string(), "test-node".to_string())
}

/// Build a throwaway `places.sqlite` from the SQL fixture and return its path
/// (kept alive by the returned `TempDir`).
fn firefox_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("places.sqlite");
    let sql = std::fs::read_to_string(fixtures().join("firefox_places.sql")).expect("read sql");
    let conn = rusqlite::Connection::open(&db_path).expect("create db");
    conn.execute_batch(&sql).expect("build fixture db");
    conn.close().expect("close db");
    (dir, db_path)
}

/// Apply a fresh import of `path` into an empty collection and return both.
fn imported(collection: &mut Collection, ops: &[mde_bookmarks::Op]) {
    collection.apply_all(ops.iter());
}

/// The id of the folder reached by walking `path` from the root.
fn folder_id(c: &Collection, path: &[&str]) -> Option<Uuid> {
    let mut parent = None;
    'seg: for seg in path {
        for item in c.children(parent) {
            if let Item::Folder(f) = &item {
                if f.name == *seg {
                    parent = Some(f.id);
                    continue 'seg;
                }
            }
        }
        return None;
    }
    parent
}

/// The bookmarks directly under a folder id.
fn bookmarks_under(c: &Collection, parent: Option<Uuid>) -> Vec<Bookmark> {
    c.children(parent)
        .into_iter()
        .filter_map(|it| match it {
            Item::Bookmark(b) => Some(b),
            Item::Folder(_) => None,
        })
        .collect()
}

fn titles(bms: &[Bookmark]) -> Vec<String> {
    bms.iter().map(|b| b.title.clone()).collect()
}

#[test]
fn firefox_imports_bookmarks_only_with_tags_and_named_roots() {
    let (_dir, db) = firefox_db();
    let mut c = Collection::new();
    let mut clk = clock();
    let out = import_file(&c, &db, &mut clk, &author(), NOW_MS).expect("import");
    imported(&mut c, &out.ops);

    // Named roots under Imported/Firefox (guids → friendly labels).
    let dev = folder_id(&c, &["Imported", "Firefox", "Bookmarks Menu", "Dev"]).expect("menu/Dev");
    let dev_bms = bookmarks_under(&c, Some(dev));
    assert_eq!(titles(&dev_bms), vec!["Rust"]);
    assert_eq!(dev_bms[0].url, "https://rust-lang.org/");
    assert_eq!(dev_bms[0].source, Source::Firefox);

    let toolbar = folder_id(&c, &["Imported", "Firefox", "Bookmarks Toolbar"]).expect("toolbar");
    assert_eq!(titles(&bookmarks_under(&c, Some(toolbar))), vec!["News"]);

    // Firefox tag folder → tags[] on the bookmark (never a folder).
    let other = folder_id(&c, &["Imported", "Firefox", "Other Bookmarks"]).expect("unfiled");
    let tagged = bookmarks_under(&c, Some(other));
    assert_eq!(titles(&tagged), vec!["Tagged"]);
    assert_eq!(tagged[0].tags, vec!["reading".to_string()]);

    // The tags root / tag folder must NOT appear as folders anywhere.
    assert!(folder_id(&c, &["Imported", "Firefox", "tags"]).is_none());
    assert!(folder_id(&c, &["Imported", "Firefox", "Other Bookmarks", "reading"]).is_none());
    // The root container is not surfaced either.
    assert!(folder_id(&c, &["Imported", "Firefox", "root"]).is_none());
}

#[test]
fn firefox_never_reads_logins_cookies_or_history() {
    let (_dir, db) = firefox_db();
    let mut c = Collection::new();
    let mut clk = clock();
    let out = import_file(&c, &db, &mut clk, &author(), NOW_MS).expect("import");
    imported(&mut c, &out.ops);

    // The entire converged state must not contain any secret nor the
    // history-only URL that was never bookmarked.
    let json = serde_json::to_string(&c).expect("serialize");
    for forbidden in [
        "SECRET-COOKIE-VALUE",
        "SECRET-PASSWORD-VALUE",
        "secret-history",
        "victim",
    ] {
        assert!(
            !json.contains(forbidden),
            "imported state leaked forbidden data: {forbidden}"
        );
    }
}

#[test]
fn firefox_reimport_is_idempotent() {
    let (_dir, db) = firefox_db();
    let mut c = Collection::new();
    let mut clk = clock();
    let first = import_file(&c, &db, &mut clk, &author(), NOW_MS).expect("first import");
    imported(&mut c, &first.ops);
    assert!(first.bookmarks_added >= 3 && first.folders_created >= 4);

    // Re-importing the same file into the populated collection must add nothing.
    let second = import_file(&c, &db, &mut clk, &author(), NOW_MS).expect("second import");
    assert!(second.ops.is_empty(), "re-import produced ops: {second:?}");
    assert_eq!(second.bookmarks_added, 0);
    assert_eq!(second.folders_created, 0);
}

#[test]
fn chromium_roots_become_named_subfolders() {
    let path = fixtures().join("chromium_bookmarks.json");
    let mut c = Collection::new();
    let mut clk = clock();
    let out = import_file(&c, &path, &mut clk, &author(), NOW_MS).expect("import");
    imported(&mut c, &out.ops);

    let bar = folder_id(&c, &["Imported", "Chromium", "Bookmarks Bar"]).expect("bar");
    let bar_bms = bookmarks_under(&c, Some(bar));
    assert_eq!(titles(&bar_bms), vec!["Example"]);
    assert_eq!(bar_bms[0].source, Source::Chromium);

    let work = folder_id(&c, &["Imported", "Chromium", "Bookmarks Bar", "Work"]).expect("Work");
    assert_eq!(titles(&bookmarks_under(&c, Some(work))), vec!["Docs"]);

    let other = folder_id(&c, &["Imported", "Chromium", "Other"]).expect("Other");
    assert_eq!(
        titles(&bookmarks_under(&c, Some(other))),
        vec!["Other Link"]
    );

    // The empty `synced` root still becomes a named "Mobile" folder.
    assert!(folder_id(&c, &["Imported", "Chromium", "Mobile"]).is_some());
}

#[test]
fn safari_html_export_imports_nested_folders_and_tags() {
    let path = fixtures().join("safari_export.html");
    let mut c = Collection::new();
    let mut clk = clock();
    // A Safari export is Netscape HTML; the caller labels it Safari.
    let out =
        import_file_as(&c, &path, Source::Safari, &mut clk, &author(), NOW_MS).expect("import");
    imported(&mut c, &out.ops);

    let safari = folder_id(&c, &["Imported", "Safari"]).expect("Safari");
    let top = bookmarks_under(&c, Some(safari));
    assert_eq!(
        titles(&top),
        vec!["Apple & Co"],
        "HTML entity was unescaped"
    );
    assert_eq!(top[0].source, Source::Safari);

    let reading = folder_id(&c, &["Imported", "Safari", "Reading"]).expect("Reading");
    let reading_bms = bookmarks_under(&c, Some(reading));
    assert_eq!(titles(&reading_bms), vec!["Blog Post"]);
    assert_eq!(
        reading_bms[0].tags,
        vec!["tech".to_string(), "news".to_string()]
    );

    let nested = folder_id(&c, &["Imported", "Safari", "Reading", "Nested"]).expect("Nested");
    assert_eq!(
        titles(&bookmarks_under(&c, Some(nested))),
        vec!["Deep Link"]
    );
}

#[test]
fn netscape_dedup_normalizes_urls() {
    let path = fixtures().join("netscape_generic.html");
    let mut c = Collection::new();
    let mut clk = clock();
    let out = import_file(&c, &path, &mut clk, &author(), NOW_MS).expect("import");
    imported(&mut c, &out.ops);

    // Two entries differ only by fragment → one bookmark, deduped + refreshed.
    assert_eq!(out.bookmarks_added, 1);
    assert_eq!(out.bookmarks_deduped, 1);
    assert_eq!(out.bookmarks_refreshed, 1);

    let netscape = folder_id(&c, &["Imported", "Netscape"]).expect("Netscape");
    let bms = bookmarks_under(&c, Some(netscape));
    assert_eq!(bms.len(), 1, "normalized dedup collapsed the pair");
    // The stored URL is the first (original) one; the title was refreshed.
    assert_eq!(bms[0].url, "https://example.org/");
    assert_eq!(bms[0].title, "Example Org (again)");
}

#[test]
fn scan_profiles_lists_bookmark_files_and_skips_credential_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        fixtures().join("chromium_bookmarks.json"),
        dir.path().join("Bookmarks"),
    )
    .expect("copy json");
    std::fs::copy(
        fixtures().join("netscape_generic.html"),
        dir.path().join("export.html"),
    )
    .expect("copy html");
    // Credential files that must never be listed (nor even sniffed).
    std::fs::write(
        dir.path().join("cookies.sqlite"),
        b"SQLite format 3\0secret",
    )
    .expect("cookies");
    std::fs::write(dir.path().join("key4.db"), b"key material").expect("key4");
    std::fs::write(dir.path().join("logins.json"), br#"{"logins":[]}"#).expect("logins");

    let found = mde_bookmarks::scan_profiles(dir.path()).expect("scan");
    let names: Vec<String> = found
        .iter()
        .filter_map(|c| c.path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    assert!(names.contains(&"Bookmarks".to_string()));
    assert!(names.contains(&"export.html".to_string()));
    assert!(
        !names
            .iter()
            .any(|n| n == "cookies.sqlite" || n == "key4.db" || n == "logins.json"),
        "credential files must never be listed: {names:?}"
    );
}

#[test]
fn format_default_source_maps_each() {
    use mde_bookmarks::ImportFormat;
    assert_eq!(
        ImportFormat::FirefoxSqlite.default_source(),
        Source::Firefox
    );
    assert_eq!(
        ImportFormat::ChromiumJson.default_source(),
        Source::Chromium
    );
    assert_eq!(
        ImportFormat::NetscapeHtml.default_source(),
        Source::NetscapeHtml
    );
}

-- A minimal but authentic Firefox `places.sqlite` fixture for the BOOKMARKS-3
-- importer test. The test builds a throwaway SQLite file from this SQL, then the
-- importer opens THAT file read-only + immutable and must read bookmarks ONLY.
--
-- It deliberately includes:
--   * a history-only moz_places row (id 900) that is NOT referenced by any
--     bookmark — the importer must never surface it (no history);
--   * moz_cookies + moz_logins tables holding SECRET values — the importer must
--     never read them (the test greps the imported state for the secrets).

CREATE TABLE moz_places (
    id    INTEGER PRIMARY KEY,
    url   TEXT,
    title TEXT
);

CREATE TABLE moz_bookmarks (
    id        INTEGER PRIMARY KEY,
    type      INTEGER,          -- 1 = bookmark, 2 = folder
    fk        INTEGER,          -- -> moz_places.id for a bookmark
    parent    INTEGER,          -- -> moz_bookmarks.id
    position  INTEGER,
    title     TEXT,
    dateAdded INTEGER,          -- microseconds since the Unix epoch
    guid      TEXT
);

-- History-only URL (never bookmarked) — must NOT be imported.
INSERT INTO moz_places (id, url, title) VALUES
    (900, 'https://secret-history.example/visited-page', 'SECRET HISTORY ENTRY');

-- Bookmarked URLs.
INSERT INTO moz_places (id, url, title) VALUES
    (10, 'https://rust-lang.org/', 'Rust'),
    (11, 'https://news.example/article?utm_source=news#frag', 'News'),
    (12, 'https://tagged.example/', 'Tagged');

-- The root container and the named roots (guids are what Firefox actually uses).
INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid) VALUES
    (1, 2, NULL, 0, 0, '',        0,                'root________'),
    (2, 2, NULL, 1, 0, 'menu',    1600000000000000, 'menu________'),
    (3, 2, NULL, 1, 1, 'toolbar', 1600000000000000, 'toolbar_____'),
    (4, 2, NULL, 1, 2, 'tags',    1600000000000000, 'tags________'),
    (5, 2, NULL, 1, 3, 'unfiled', 1600000000000000, 'unfiled_____');

-- A nested folder under the menu.
INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid) VALUES
    (20, 2, NULL, 2, 0, 'Dev', 1600000000000000, 'folderDev___');

-- Bookmarks: Rust under menu/Dev, News under toolbar, Tagged under unfiled.
INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid) VALUES
    (30, 1, 10, 20, 0, 'Rust',   1600000000000000, 'bmRust______'),
    (31, 1, 11, 3,  0, 'News',   1600000000000000, 'bmNews______'),
    (32, 1, 12, 5,  0, 'Tagged', 1600000000000000, 'bmTagged____');

-- A tag folder under the tags root + the tagged entry (fk 12) -> tags[] on the
-- Tagged bookmark. Neither row may appear as a folder/bookmark in the import.
INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid) VALUES
    (40, 2, NULL, 4,  0, 'reading', 1600000000000000, 'tagReading__'),
    (41, 1, 12,   40, 0, 'Tagged',  1600000000000000, 'tagEntry____');

-- SECRET tables the importer must NEVER read.
CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, host TEXT, name TEXT, value TEXT);
INSERT INTO moz_cookies VALUES (1, 'evil.example', 'session', 'SECRET-COOKIE-VALUE');

CREATE TABLE moz_logins (id INTEGER PRIMARY KEY, hostname TEXT, username TEXT, password TEXT);
INSERT INTO moz_logins VALUES (1, 'https://bank.example', 'victim', 'SECRET-PASSWORD-VALUE');

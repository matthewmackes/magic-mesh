-- BUS-1.4 — initial schema for the per-peer message index.
--
-- Per `docs/design/v6.x-mackes-bus.md` §8:
--
--   * The authoritative store is the per-topic file tree at
--     `~/.local/share/mde/bus/<topic-path>/<ulid>.json`. That tree
--     lives on GFS (mesh-home replicated) and is the durable
--     record for cross-peer replay + audit.
--   * This SQLite database is the *local-peer queryable index* —
--     it answers "what's new since ULID X on topic Y?" queries
--     without walking the on-disk tree. Each peer maintains its
--     own `index.sqlite` (NOT on GFS — SQLite + networked FS is
--     a known footgun). Cross-peer aggregation is BUS-7
--     federation territory.
--
-- The schema is intentionally narrow — the index stores enough
-- to drive tail / history / retention queries without touching
-- the file tree, but does NOT replace it. `detect_divergence()`
-- is the safety net that catches index/file drift.

PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
-- Keep a checkpointable WAL from retaining hundreds of megabytes after a
-- burst. A pinned reader may temporarily delay truncation, but the next
-- successful checkpoint returns the file to this bounded target.
PRAGMA wal_autocheckpoint = 256;
PRAGMA journal_size_limit = 8388608;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS messages (
    -- ULID (Crockford base32 26 chars) — sortable + globally
    -- unique. Used as the cursor for `list_since` queries.
    ulid       TEXT NOT NULL PRIMARY KEY,
    -- Slash-hierarchy topic path (matches the on-disk dir name).
    topic      TEXT NOT NULL,
    -- Priority enum: 'min' | 'default' | 'high' | 'urgent'.
    priority   TEXT NOT NULL DEFAULT 'default',
    -- Title + body are duplicated from the JSON file so the
    -- index can serve tail / history without disk I/O. Both
    -- nullable because pure-payload publishes carry only a body.
    title      TEXT,
    body       TEXT,
    -- Large bodies stay in the authoritative JSON envelope instead of being
    -- duplicated into SQLite/WAL. 1 means `body` must be hydrated from
    -- `file_path` by the persistence API.
    body_external INTEGER NOT NULL DEFAULT 0,
    -- Unix ms — used by retention (BUS-1.9) for TTL queries.
    ts_unix_ms INTEGER NOT NULL,
    -- Relative path under `bus_root`. UNIQUE so we can detect
    -- accidental re-writes + serves as the file-tree pointer
    -- for divergence detection.
    file_path  TEXT NOT NULL UNIQUE
);

-- Tail queries scan by (topic, ulid) — the index supports the
-- `list_since(topic, since_ulid)` query plan directly.
CREATE INDEX IF NOT EXISTS idx_messages_topic_ulid
    ON messages (topic, ulid);

-- Retention scans by ts_unix_ms — BUS-1.9 will walk this index
-- to find messages past their priority-class TTL.
CREATE INDEX IF NOT EXISTS idx_messages_ts
    ON messages (ts_unix_ms);

-- Priority TTL passes probe this composite key directly instead of filtering a
-- timestamp scan across every retained priority.
CREATE INDEX IF NOT EXISTS idx_messages_priority_ts
    ON messages (priority, ts_unix_ms);

-- BUS-TOPIC-CATALOG-1 — a compact, trigger-maintained set of topics with at
-- least one live message. `SELECT DISTINCT topic FROM messages` scanned every
-- retained row each time one of the many polling workers discovered topics.
-- This table makes discovery proportional to the number of topics and gives
-- prefix readers a primary-key range scan without touching message bodies.
CREATE TABLE IF NOT EXISTS active_topics (
    topic TEXT NOT NULL PRIMARY KEY
) WITHOUT ROWID;

-- Backfill an upgraded index. INSERT OR IGNORE is idempotent; subsequent
-- writes/deletes are maintained by the triggers below.
INSERT OR IGNORE INTO active_topics (topic)
    SELECT DISTINCT topic FROM messages;

CREATE TRIGGER IF NOT EXISTS trg_messages_active_topic_insert
AFTER INSERT ON messages
BEGIN
    INSERT OR IGNORE INTO active_topics (topic) VALUES (NEW.topic);
END;

CREATE TRIGGER IF NOT EXISTS trg_messages_active_topic_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM active_topics
     WHERE topic = OLD.topic
       AND NOT EXISTS (
           SELECT 1 FROM messages
            WHERE topic = OLD.topic
            LIMIT 1
       );
END;

CREATE TRIGGER IF NOT EXISTS trg_messages_active_topic_update
AFTER UPDATE OF topic ON messages
WHEN OLD.topic <> NEW.topic
BEGIN
    INSERT OR IGNORE INTO active_topics (topic) VALUES (NEW.topic);
    DELETE FROM active_topics
     WHERE topic = OLD.topic
       AND NOT EXISTS (
           SELECT 1 FROM messages
            WHERE topic = OLD.topic
            LIMIT 1
       );
END;

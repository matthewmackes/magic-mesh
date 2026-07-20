-- WL-FUNC-011 Phase 1 — the collaboration read-side projection schema.
--
-- Two layers:
--
--   1. `collab_event` — the durable, idempotent event sink. Every signed
--      CollabEventEnvelope this node has accepted is stored here verbatim
--      (envelope_json), keyed by its EventId. `INSERT OR IGNORE` makes ingest
--      idempotent; the (space_id, clock_wall, clock_counter, event_id) columns
--      let the projector read a space's log back in the canonical convergent
--      order (clock, then EventId tiebreak) without parsing every blob.
--
--   2. The materialized read tables (spaces, members, messages, ...) — the
--      resolved rows that back the CollabReadModel shapes the surface renders.
--      They are NOT accumulated incrementally; on every `project(batch)` the
--      projector rebuilds each *touched* space's rows by folding that space's
--      full event set from `collab_event` in the canonical order. Rebuild is a
--      pure function of the event SET, so it is idempotent (re-applying an event
--      is a no-op) and order-independent (any two nodes with the same events
--      fold to byte-identical rows) — the convergence guarantee.
--
-- Every clock is stored as its two integer components so ORDER BY is a total,
-- deterministic order. All time is caller-injected (epoch ms); nothing here
-- reads a wall clock.

PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = OFF;

-- ---- Layer 1: the durable idempotent event sink -------------------------
CREATE TABLE IF NOT EXISTS collab_event (
    event_id      TEXT NOT NULL PRIMARY KEY,
    space_id      TEXT NOT NULL,
    actor         TEXT NOT NULL,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL,
    created_ms    INTEGER NOT NULL,
    kind_tag      TEXT NOT NULL,
    envelope_json TEXT NOT NULL
);

-- The canonical convergent read order for a space's log.
CREATE INDEX IF NOT EXISTS idx_event_space_order
    ON collab_event (space_id, clock_wall, clock_counter, event_id);

-- Presence is global (folded across every space, including the nil "global"
-- space) — this index serves the presence refold.
CREATE INDEX IF NOT EXISTS idx_event_kind
    ON collab_event (kind_tag, clock_wall, clock_counter, event_id);

-- ---- Layer 2: the materialized read tables ------------------------------

-- The space directory rail.
CREATE TABLE IF NOT EXISTS spaces (
    space_id          TEXT NOT NULL PRIMARY KEY,
    kind              TEXT NOT NULL,
    name              TEXT NOT NULL,
    created_ms        INTEGER NOT NULL,
    deleted           INTEGER NOT NULL DEFAULT 0,
    last_clock_wall   INTEGER NOT NULL DEFAULT 0,
    last_clock_counter INTEGER NOT NULL DEFAULT 0
);

-- Currently-present members of a space (a left member has no row).
CREATE TABLE IF NOT EXISTS members (
    space_id TEXT NOT NULL,
    actor    TEXT NOT NULL,
    role     TEXT NOT NULL,
    PRIMARY KEY (space_id, actor)
);

-- Messages (main timeline + thread replies). `body`/`edited`/`deleted` are the
-- resolved (LWW-by-clock body, monotonic tombstone) values from the fold.
CREATE TABLE IF NOT EXISTS messages (
    space_id     TEXT NOT NULL,
    event_id     TEXT NOT NULL PRIMARY KEY,
    author       TEXT NOT NULL,
    created_ms   INTEGER NOT NULL,
    thread_id    TEXT,
    body         TEXT NOT NULL,
    edited       INTEGER NOT NULL DEFAULT 0,
    deleted      INTEGER NOT NULL DEFAULT 0,
    delivery     TEXT NOT NULL DEFAULT 'sent',
    clock_wall   INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_space_order
    ON messages (space_id, clock_wall, clock_counter, event_id);

-- Reply threads.
CREATE TABLE IF NOT EXISTS threads (
    space_id      TEXT NOT NULL,
    thread_id     TEXT NOT NULL PRIMARY KEY,
    root_event_id TEXT NOT NULL,
    title         TEXT,
    resolved      INTEGER NOT NULL DEFAULT 0,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);

-- The alert inbox (folded from AlertRaised, resolved ack/snooze).
CREATE TABLE IF NOT EXISTS alerts (
    space_id      TEXT NOT NULL,
    event_id      TEXT NOT NULL PRIMARY KEY,
    severity      TEXT NOT NULL,
    source        TEXT NOT NULL,
    headline      TEXT NOT NULL,
    fields_json   TEXT NOT NULL,
    actions_json  TEXT NOT NULL,
    goto          TEXT,
    acknowledged  INTEGER NOT NULL DEFAULT 0,
    snoozed_until INTEGER,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);

-- The clipboard lane (tombstoned clips are not materialized).
CREATE TABLE IF NOT EXISTS clipboard (
    space_id      TEXT NOT NULL,
    event_id      TEXT NOT NULL PRIMARY KEY,
    kind          TEXT NOT NULL,
    preview       TEXT NOT NULL,
    sha256_hex    TEXT NOT NULL,
    source        TEXT NOT NULL,
    at_ms         INTEGER NOT NULL,
    pinned        INTEGER NOT NULL DEFAULT 0,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);

-- Linked file references (unlinked refs are not materialized). The canonical
-- file's bytes are content-addressed by sha256_hex and are NEVER purged by a
-- space or link tombstone.
CREATE TABLE IF NOT EXISTS file_refs (
    space_id      TEXT NOT NULL,
    file_ref_id   TEXT NOT NULL PRIMARY KEY,
    name          TEXT NOT NULL,
    size          INTEGER NOT NULL,
    sha256_hex    TEXT NOT NULL,
    mime          TEXT,
    linked_by     TEXT NOT NULL,
    linked_ms     INTEGER NOT NULL,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);

-- Transfer jobs (a read-side mirror; byte progress is WL-FUNC-006's ledger).
CREATE TABLE IF NOT EXISTS transfers (
    transfer_id   TEXT NOT NULL PRIMARY KEY,
    space_id      TEXT NOT NULL,
    file_ref_id   TEXT NOT NULL,
    method        TEXT NOT NULL,
    direction     TEXT NOT NULL,
    state         TEXT NOT NULL,
    moved         INTEGER NOT NULL DEFAULT 0,
    total         INTEGER NOT NULL DEFAULT 0,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);

-- Collaborative documents.
CREATE TABLE IF NOT EXISTS documents (
    space_id       TEXT NOT NULL,
    document_id    TEXT NOT NULL PRIMARY KEY,
    title          TEXT NOT NULL,
    latest_summary TEXT,
    participants_json TEXT NOT NULL DEFAULT '[]',
    clock_wall     INTEGER NOT NULL,
    clock_counter  INTEGER NOT NULL
);

-- Global presence board (one row per known member, LWW by clock).
CREATE TABLE IF NOT EXISTS presence (
    actor         TEXT NOT NULL PRIMARY KEY,
    presence      TEXT NOT NULL,
    status        TEXT,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);

-- Active/ended calls.
CREATE TABLE IF NOT EXISTS calls (
    call_id       TEXT NOT NULL PRIMARY KEY,
    space_id      TEXT NOT NULL,
    kind          TEXT NOT NULL,
    initiator     TEXT NOT NULL,
    started_ms    INTEGER NOT NULL,
    ended         INTEGER NOT NULL DEFAULT 0,
    clock_wall    INTEGER NOT NULL,
    clock_counter INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS call_participants (
    call_id TEXT NOT NULL,
    actor   TEXT NOT NULL,
    state   TEXT NOT NULL,
    muted   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (call_id, actor)
);

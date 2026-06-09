-- mackesd 0002_settings_session — v2.0.0 Phase A.4 (locked 2026-05-19)
--
-- Adds the schema surface the v2.0.0 unified backend needs on top of
-- the Phase 12 control-plane tables in 0001_init.sql:
--
--   settings                 every locally-applied settings knob
--                            (theme, font, display, power, etc.)
--   fleet_settings_apply_log per-peer settings-revision apply audit
--   session_state            mackes-session lifecycle bookkeeping
--   notifications            persisted org.freedesktop.Notifications
--                            queue (mackesd is now the daemon)
--
-- The Phase C `mackes-settingsd` worker module reads/writes `settings`
-- directly via `store::with_transaction`; every revision-driven push
-- from the control plane logs a row in `fleet_settings_apply_log` so
-- the fleet panel can show per-peer rollout status.

-- ---- settings ---------------------------------------------------
-- One row per (key, scope). `scope` is 'user' for the current session
-- and 'system' for machine-wide settings (a future split — today
-- everything is 'user' since the daemon runs in the user systemd
-- session). `source_revision_id` links to revisions.revision_id when
-- the value came from a fleet push; NULL when set locally.
CREATE TABLE settings (
    key                TEXT NOT NULL,
    scope              TEXT NOT NULL DEFAULT 'user'
                       CHECK (scope IN ('user','system')),
    value_json         TEXT NOT NULL,
    last_applied_at    TEXT NOT NULL,
    source_revision_id TEXT,
    PRIMARY KEY (key, scope)
);
CREATE INDEX idx_settings_revision     ON settings(source_revision_id);
CREATE INDEX idx_settings_last_applied ON settings(last_applied_at);

-- ---- fleet_settings_apply_log -----------------------------------
-- Append-only audit of every settings revision applied on this peer.
-- Mirrors the audit table's append-only discipline but is scoped to
-- settings (audit.rs covers the broader event log). One row per
-- (peer_id, revision_id, key) triple — a single revision can touch
-- many keys.
CREATE TABLE fleet_settings_apply_log (
    log_id        INTEGER PRIMARY KEY,
    peer_id       TEXT NOT NULL,
    revision_id   TEXT NOT NULL,
    key           TEXT NOT NULL,
    applied_at    TEXT NOT NULL,
    ok            INTEGER NOT NULL CHECK (ok IN (0,1)),
    error_text    TEXT
);
CREATE INDEX idx_fleet_apply_revision ON fleet_settings_apply_log(revision_id);
CREATE INDEX idx_fleet_apply_peer     ON fleet_settings_apply_log(peer_id, applied_at);

-- ---- session_state ----------------------------------------------
-- One row per logical session. Most of mackes-session's state lives
-- in memory; this table is the durable handoff (e.g. "what compositor
-- did we last successfully start, what time did we lock the screen").
-- Phase D wires it up.
CREATE TABLE session_state (
    session_id     TEXT PRIMARY KEY,
    compositor     TEXT NOT NULL CHECK (compositor IN ('sway','i3','unknown')),
    started_at     TEXT NOT NULL,
    last_login_at  TEXT,
    locked_at      TEXT,
    sealed_at      TEXT,
    metadata_json  TEXT NOT NULL DEFAULT '{}'
);

-- ---- notifications ----------------------------------------------
-- The persisted notification queue. mackesd is now the
-- `org.freedesktop.Notifications` server (Phase B.10), so we own this
-- table. Read/written by `workers/notifications_server.rs` (server),
-- `workers/notification_relay.rs` (peer-bridge), and the Iced applet
-- at `crates/mackes-applets/notifications/`. The `notification_id`
-- matches the integer ID the spec hands back to the calling app.
CREATE TABLE notifications (
    notification_id INTEGER PRIMARY KEY,
    sender          TEXT NOT NULL,
    summary         TEXT NOT NULL,
    body            TEXT NOT NULL DEFAULT '',
    app_icon        TEXT,
    hints_json      TEXT NOT NULL DEFAULT '{}',
    urgency         INTEGER NOT NULL DEFAULT 1
                    CHECK (urgency IN (0,1,2)),
    expire_after_ms INTEGER NOT NULL DEFAULT -1,
    created_at      TEXT NOT NULL,
    read_at         TEXT,
    dismissed_at    TEXT,
    -- Foreign peer that originated the notification (set by
    -- notification_relay.rs when forwarding from a mesh peer). NULL
    -- for locally-emitted notifications.
    origin_peer_id  TEXT
);
CREATE INDEX idx_notifications_unread   ON notifications(read_at) WHERE read_at IS NULL;
CREATE INDEX idx_notifications_undisposed ON notifications(dismissed_at) WHERE dismissed_at IS NULL;
CREATE INDEX idx_notifications_created  ON notifications(created_at);

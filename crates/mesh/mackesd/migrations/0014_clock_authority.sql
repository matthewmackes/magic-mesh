CREATE TABLE IF NOT EXISTS clock_authority (
    node_id         TEXT PRIMARY KEY NOT NULL,
    revision        INTEGER NOT NULL CHECK (revision > 0),
    snapshot_json   TEXT NOT NULL,
    action_cursor   TEXT,
    updated_at_ms   INTEGER NOT NULL CHECK (updated_at_ms > 0)
);

CREATE TABLE IF NOT EXISTS clock_request_ledger (
    node_id         TEXT NOT NULL,
    request_id      TEXT NOT NULL,
    revision        INTEGER NOT NULL CHECK (revision > 0),
    applied_at_ms   INTEGER NOT NULL CHECK (applied_at_ms > 0),
    PRIMARY KEY (node_id, request_id),
    FOREIGN KEY (node_id) REFERENCES clock_authority(node_id) ON DELETE CASCADE
);

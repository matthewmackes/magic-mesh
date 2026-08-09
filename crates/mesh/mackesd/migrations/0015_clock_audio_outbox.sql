CREATE TABLE IF NOT EXISTS clock_audio_outbox (
    node_id                TEXT NOT NULL,
    request_id             TEXT NOT NULL,
    occurrence_id          TEXT NOT NULL,
    global_event_id        TEXT NOT NULL,
    occurrence_generation INTEGER NOT NULL CHECK (occurrence_generation > 0),
    request_json           TEXT NOT NULL,
    created_at_ms          INTEGER NOT NULL CHECK (created_at_ms > 0),
    acknowledged_at_ms     INTEGER CHECK (acknowledged_at_ms > 0),
    PRIMARY KEY (node_id, request_id),
    FOREIGN KEY (node_id) REFERENCES clock_authority(node_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS clock_audio_outbox_pending
    ON clock_audio_outbox (node_id, acknowledged_at_ms, created_at_ms, request_id);

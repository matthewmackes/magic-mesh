-- mackesd 0001_init — initial schema (Phase 12.2.1)
--
-- Eight tables that together hold every authoritative Mesh fact. The
-- 7 state buckets from the /goal spec map onto these tables:
--
--   desired_config       desired state (versioned)
--   runtime_state        actual state per node (current revision applied)
--   observed_telemetry   raw heartbeats + link probes
--   topology_snapshot    calculated topology derived from desired + observed
--   pending_changes      revisions waiting on approval
--   applied_changes      revisions that landed cleanly (via deployments)
--   failed_changes       revisions that failed validation OR deployment
--
-- Plus three operational tables:
--
--   nodes                identity + heartbeat + lifecycle state
--   events               append-only audit log with hash chain
--   policies             named rule sets referenced by desired_config

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA synchronous  = NORMAL;

CREATE TABLE nodes (
    node_id            TEXT PRIMARY KEY,
    name               TEXT NOT NULL UNIQUE,
    public_key         TEXT NOT NULL,
    enrolled_at        TEXT NOT NULL,
    last_heartbeat_at  TEXT,
    health             TEXT NOT NULL DEFAULT 'unknown'
                       CHECK (health IN ('healthy','degraded','unreachable','unknown')),
    agent_version      TEXT,
    role               TEXT NOT NULL DEFAULT 'peer'
                       CHECK (role IN ('host','peer','observer','decommissioned')),
    region             TEXT,
    metadata_json      TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_nodes_health ON nodes(health);
CREATE INDEX idx_nodes_role   ON nodes(role);

CREATE TABLE policies (
    policy_id          TEXT PRIMARY KEY,
    name               TEXT NOT NULL UNIQUE,
    spec_json          TEXT NOT NULL,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

CREATE TABLE desired_config (
    revision_id        INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_revision_id INTEGER REFERENCES desired_config(revision_id),
    author             TEXT NOT NULL,
    message            TEXT NOT NULL,
    spec_json          TEXT NOT NULL,
    state              TEXT NOT NULL DEFAULT 'draft'
                       CHECK (state IN ('draft','validated','approved',
                                        'deploying','applied','verified',
                                        'failed_validation','failed_deployment',
                                        'rolled_back')),
    created_at         TEXT NOT NULL,
    applied_at         TEXT,
    verified_at        TEXT
);
CREATE INDEX idx_desired_state ON desired_config(state);

CREATE TABLE runtime_state (
    node_id            TEXT NOT NULL REFERENCES nodes(node_id),
    applied_revision   INTEGER REFERENCES desired_config(revision_id),
    last_apply_at      TEXT NOT NULL,
    drift_severity     TEXT NOT NULL DEFAULT 'none'
                       CHECK (drift_severity IN ('none','auto-repairable','manual-review')),
    drift_notes        TEXT,
    PRIMARY KEY (node_id)
);

CREATE TABLE observed_telemetry (
    seq                INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id            TEXT NOT NULL REFERENCES nodes(node_id),
    observed_at        TEXT NOT NULL,
    kind               TEXT NOT NULL,
    payload_json       TEXT NOT NULL
);
CREATE INDEX idx_telemetry_node_time ON observed_telemetry(node_id, observed_at);

CREATE TABLE topology_link_health (
    src_node_id        TEXT NOT NULL REFERENCES nodes(node_id),
    dst_node_id        TEXT NOT NULL REFERENCES nodes(node_id),
    latency_ms         REAL,
    loss_pct           REAL,
    throughput_mbps    REAL,
    measured_at        TEXT NOT NULL,
    PRIMARY KEY (src_node_id, dst_node_id)
);

CREATE TABLE events (
    seq                INTEGER PRIMARY KEY AUTOINCREMENT,
    prev_hash          TEXT NOT NULL DEFAULT '',
    hash               TEXT NOT NULL,
    kind               TEXT NOT NULL,
    actor              TEXT NOT NULL,
    payload_json       TEXT NOT NULL,
    created_at         TEXT NOT NULL
);
CREATE INDEX idx_events_kind ON events(kind);

CREATE TABLE leader_lease (
    -- singleton row (rowid = 1) carrying current leader info. Updated
    -- via UPDATE ... WHERE rowid = 1; the leader-election logic in
    -- mackesd consults this in addition to the QNM-Shared lockfile.
    rowid              INTEGER PRIMARY KEY CHECK (rowid = 1),
    leader_node_id     TEXT REFERENCES nodes(node_id),
    leased_at          TEXT,
    expires_at         TEXT
);

-- schema_migrations is created by mackesd_core::store::migrate() at
-- bootstrap time so it exists before this migration runs (we need to
-- query it to know whether to run this migration). Intentionally
-- omitted from the migration body to avoid the "already exists"
-- collision.

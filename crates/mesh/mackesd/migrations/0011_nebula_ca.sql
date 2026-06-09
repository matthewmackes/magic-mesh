-- NF-2.1 (v2.5) — Nebula CA + per-peer cert tables.
--
-- Schema additions for the v2.5 Nebula Fabric PKI. Strictly
-- additive per the 12.2.4-locked migration policy: zero existing
-- table changed, zero data movement.
--
-- Tables:
--
--   nebula_ca           one row per mesh, per epoch. mesh_id +
--                       epoch are the PK; current CA = the row
--                       whose retired_at IS NULL.
--   nebula_peer_certs   one row per node, per epoch. Issued by
--                       the active CA at sign-time + re-issued on
--                       epoch rotation (NF-2.5).
--
-- Index strategy: every read path queries by either
-- (mesh_id, epoch) or (node_id, epoch); both are PK so SQLite's
-- default rowid lookup wins. An `overlay_ip` lookup index is
-- added because the sign path needs to verify the proposed IP
-- hasn't been allocated to another peer.

CREATE TABLE nebula_ca (
    mesh_id      TEXT    NOT NULL,
    epoch        INTEGER NOT NULL,
    ca_cert_pem  TEXT    NOT NULL,
    -- Public-key-only field; the matching private key lives at
    -- /var/lib/mackesd/nebula-ca/ca.key (mode 0600, see
    -- NF-2.4 seal helper).
    created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
    retired_at   INTEGER,  -- NULL = current; non-NULL on rotation
    PRIMARY KEY (mesh_id, epoch)
);

CREATE INDEX nebula_ca_active ON nebula_ca (mesh_id) WHERE retired_at IS NULL;

CREATE TABLE nebula_peer_certs (
    node_id      TEXT    NOT NULL,
    epoch        INTEGER NOT NULL,
    cert_pem     TEXT    NOT NULL,
    overlay_ip   TEXT    NOT NULL,  -- 10.42.x.y form
    created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at   INTEGER NOT NULL,
    revoked_at   INTEGER,  -- NULL = active
    PRIMARY KEY (node_id, epoch)
);

CREATE UNIQUE INDEX nebula_peer_certs_overlay_ip
    ON nebula_peer_certs (overlay_ip, epoch)
    WHERE revoked_at IS NULL;

CREATE INDEX nebula_peer_certs_by_epoch ON nebula_peer_certs (epoch);

//! `SQLite` persistence (Phase 12.2 — locked 2026-05-19 in 12.A.2).
//!
//! Owns connection lifecycle, migration application, and the helpers
//! every other module uses to read or write the store. WAL mode is
//! enabled in `0001_init.sql` so readers (the panel's in-process
//! library link) never block writers (the daemon's reconcile loop).

use std::path::Path;

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension};

use crate::Result;

/// Numbered migration. Run in order; once applied, the version is
/// recorded in `schema_migrations`.
struct Migration {
    version: i64,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../../migrations/0001_init.sql"),
    },
    // v2.0.0 Phase A.4 (locked 2026-05-19) — adds the settings,
    // fleet_settings_apply_log, session_state, and notifications
    // tables the unified backend needs. Strictly additive: existing
    // Phase 12 tables are unchanged.
    Migration {
        version: 2,
        sql: include_str!("../../migrations/0002_settings_session.sql"),
    },
    // NF-2.1 (v2.5) — Nebula CA + per-peer cert tables.
    // Versioned 11 per the design doc's "m0011" naming.
    // Strictly additive — no existing table changes.
    Migration {
        version: 11,
        sql: include_str!("../../migrations/0011_nebula_ca.sql"),
    },
    // PEERVER-4 (v2.7) — nodes.mde_version column. Strictly additive;
    // populated by the health-reconciler peer-file mirror.
    Migration {
        version: 12,
        sql: include_str!("../../migrations/0012_peer_version.sql"),
    },
];

/// Open the store at `path`, creating its parent directory if needed
/// and applying every pending migration before returning.
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created, the
/// database cannot be opened (e.g. permission denied), or any
/// migration fails to apply.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating db parent dir {}", parent.display()))?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("opening sqlite db {}", path.display()))?;
    migrate(&conn)?;
    Ok(conn)
}

/// Open an in-memory store. Used by tests + dry-run paths so the real
/// `/var/lib/mackesd/mackesd.db` never gets clobbered.
///
/// # Errors
///
/// Returns an error if the in-memory connection can't open or migrate.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory().context("opening in-memory sqlite")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Apply every pending migration. Idempotent — already-applied
/// versions are skipped.
///
/// # Errors
///
/// Returns an error if a migration's SQL fails to execute or if the
/// `schema_migrations` table can't be created.
pub fn migrate(conn: &Connection) -> Result<()> {
    // Bootstrap the tracking table so we can read its current state
    // even on a fresh database.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
             version    INTEGER PRIMARY KEY,\
             applied_at TEXT NOT NULL\
         );",
    )
    .context("creating schema_migrations table")?;

    let applied: std::collections::HashSet<i64> = {
        let mut stmt = conn
            .prepare("SELECT version FROM schema_migrations")
            .context("listing applied migrations")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .context("iterating applied migrations")?;
        rows.collect::<rusqlite::Result<_>>()
            .context("reading applied migration row")?
    };

    for m in MIGRATIONS {
        if applied.contains(&m.version) {
            continue;
        }
        conn.execute_batch(m.sql)
            .with_context(|| format!("applying migration {}", m.version))?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?)",
            (m.version, chrono::Utc::now().to_rfc3339()),
        )
        .with_context(|| format!("recording migration {}", m.version))?;
    }
    Ok(())
}

/// Run `f` inside a SQLite transaction. Commits on Ok return;
/// rolls back on `Err`. The wrapper enforces Phase 12.2.3's atomic-
/// updates lock: every multi-row write is one transaction; failure
/// on any row rolls back the whole change.
///
/// # Errors
///
/// Returns whatever error `f` returns, OR any SQLite error from the
/// transaction begin/commit machinery.
pub fn with_transaction<F, T>(conn: &mut Connection, f: F) -> Result<T>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
{
    let tx = conn.transaction().context("starting transaction")?;
    let value = f(&tx)?;
    tx.commit().context("committing transaction")?;
    Ok(value)
}

/// Roll back a desired-config revision by writing the prior
/// revision's payload as a new revision (Phase 12.5.5). Pure
/// helper — the caller decides which prior revision to restore.
///
/// # Errors
///
/// Returns an error when the revisions table can't be written.
pub fn rollback_to_revision(
    conn: &mut Connection,
    target_id: &str,
    new_id: &str,
    author: &str,
) -> Result<usize> {
    with_transaction(conn, |tx| {
        let payload: String = tx
            .query_row(
                "SELECT payload_json FROM applied_changes WHERE revision_id = ? LIMIT 1",
                [target_id],
                |r| r.get::<_, String>(0),
            )
            .with_context(|| format!("loading payload for revision {target_id}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        let summary = format!("Rollback to {target_id}");
        let n = tx
            .execute(
                "INSERT INTO applied_changes \
                 (revision_id, author, summary, created_at, applied_at, payload_json) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                (new_id, author, &summary, &now, &now, &payload),
            )
            .with_context(|| format!("inserting rollback revision {new_id}"))?;
        Ok(n)
    })
}

/// Number of migrations that have run against this connection.
///
/// # Errors
///
/// Returns an error if the `schema_migrations` table can't be queried.
pub fn applied_migration_count(conn: &Connection) -> Result<i64> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .context("counting applied migrations")?;
    Ok(n)
}

/// One row from the `nodes` table — projection of the columns the
/// topology engine + lifecycle CLI consume.
#[derive(Debug, Clone)]
pub struct NodeRow {
    /// Stable node id (e.g. `peer:anvil`).
    pub node_id: String,
    /// Display name (typically the system hostname at enrollment).
    pub name: String,
    /// Hex/base64 public key string (Ed25519, per 12.3.2).
    pub public_key: String,
    /// `host` | `peer` | `observer` | `decommissioned`.
    pub role: String,
    /// `healthy` | `degraded` | `unreachable` | `unknown`.
    pub health: String,
    /// Optional region tag (used by `topology::calculate` for
    /// east-west allow-listing).
    pub region: Option<String>,
}

/// Load every row from the `nodes` table, ordered by `node_id` so
/// downstream code reads deterministically.
///
/// # Errors
///
/// Returns an error when the `nodes` table can't be queried.
pub fn list_nodes(conn: &Connection) -> Result<Vec<NodeRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT node_id, name, public_key, role, health, region \
             FROM nodes ORDER BY node_id ASC",
        )
        .context("preparing nodes query")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(NodeRow {
                node_id: r.get(0)?,
                name: r.get(1)?,
                public_key: r.get(2)?,
                role: r.get(3)?,
                health: r.get(4)?,
                region: r.get(5)?,
            })
        })
        .context("executing nodes query")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("materializing nodes rows")?;
    Ok(rows)
}

/// Insert a hash-chained event into the `events` table (Phase 12.6.3
/// + 12.10.3). Returns the inserted row's `seq`. Atomic — wraps the
/// read-prev-hash + insert in one transaction so two concurrent
/// writers can't both compute against the same `prev_hash`.
///
/// # Errors
///
/// Returns an error when the prior-row lookup or insert fails.
pub fn insert_event(
    conn: &mut Connection,
    kind: &str,
    actor: &str,
    payload_json: &str,
) -> Result<i64> {
    with_transaction(conn, |tx| {
        let prev_hash_hex: String = tx
            .query_row(
                "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default();
        let prev_bytes: [u8; 32] = if prev_hash_hex.is_empty() {
            [0u8; 32]
        } else {
            decode_sha256_hex(&prev_hash_hex).unwrap_or([0u8; 32])
        };
        let now = chrono::Utc::now();
        let ts_ms = now.timestamp_millis();
        let new_hash = crate::audit::next_hash(&prev_bytes, payload_json.as_bytes(), ts_ms);
        let new_hash_hex = encode_sha256_hex(&new_hash);
        tx.execute(
            "INSERT INTO events (prev_hash, hash, kind, actor, payload_json, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            (
                &prev_hash_hex,
                &new_hash_hex,
                kind,
                actor,
                payload_json,
                &now.to_rfc3339(),
            ),
        )
        .context("inserting event row")?;
        Ok(tx.last_insert_rowid())
    })
}

/// Read every audit row, ordered by `seq` ascending — exactly the
/// shape `audit::verify` expects.
///
/// # Errors
///
/// Returns an error when the `events` table can't be queried.
pub fn load_audit_rows(conn: &Connection) -> Result<Vec<crate::audit::AuditRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT seq, hash, payload_json, created_at \
             FROM events ORDER BY seq ASC",
        )
        .context("preparing events query")?;
    let rows = stmt
        .query_map([], |r| {
            let seq: i64 = r.get(0)?;
            let hash_hex: String = r.get(1)?;
            let payload: String = r.get(2)?;
            let ts: String = r.get(3)?;
            Ok((seq, hash_hex, payload, ts))
        })
        .context("executing events query")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("materializing events rows")?;
    let mut out = Vec::with_capacity(rows.len());
    for (seq, hash_hex, payload, ts) in rows {
        let hash = decode_sha256_hex(&hash_hex).unwrap_or([0u8; 32]);
        // SQLite TEXT timestamps are RFC3339; convert to epoch ms for
        // audit::verify's hash recomputation.
        let parsed = chrono::DateTime::parse_from_rfc3339(&ts)
            .map(|d| d.timestamp_millis())
            .unwrap_or(0);
        out.push(crate::audit::AuditRow {
            event_id: u64::try_from(seq).unwrap_or(0),
            payload: payload.into_bytes(),
            timestamp_ms: parsed,
            hash,
        });
    }
    Ok(out)
}

/// Set the `role` column for an existing node. Idempotent — running
/// twice with the same role is a no-op.
///
/// # Errors
///
/// Returns an error when the update fails (node missing, role
/// rejected by CHECK constraint, etc).
pub fn set_node_role(conn: &Connection, node_id: &str, role: &str) -> Result<usize> {
    conn.execute(
        "UPDATE nodes SET role = ? WHERE node_id = ?",
        (role, node_id),
    )
    .with_context(|| format!("setting role={role} for {node_id}"))
}

/// Set the `health` column for an existing node, returning whether
/// the value actually changed. The `health_reconciler` worker uses
/// the change bit to decide whether to fire the
/// `dev.mackes.MDE.Nebula.Status.PeerStateChanged` signal — emitting
/// only on transitions rather than on every reconcile tick.
///
/// Returns `Ok(false)` when the node row is missing OR when the
/// stored value already matches `health`. Returns `Ok(true)` when
/// the UPDATE wrote a new value.
///
/// # Errors
///
/// Returns an error when the read-then-update transaction fails.
pub fn set_node_health(conn: &Connection, node_id: &str, health: &str) -> Result<bool> {
    let prior: Option<String> = conn
        .query_row(
            "SELECT health FROM nodes WHERE node_id = ?",
            [node_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .with_context(|| format!("reading prior health for {node_id}"))?;
    let Some(prior) = prior else {
        return Ok(false);
    };
    if prior == health {
        return Ok(false);
    }
    let n = conn
        .execute(
            "UPDATE nodes SET health = ? WHERE node_id = ?",
            (health, node_id),
        )
        .with_context(|| format!("setting health={health} for {node_id}"))?;
    Ok(n > 0)
}

/// Set a node's `mde_version` by display name (PEERVER-4 mirror).
/// Matches on `nodes.name` (the peer hostname, the key in the
/// peer-files) rather than `node_id`. Returns whether a row changed.
///
/// # Errors
/// Returns an error if the UPDATE fails.
pub fn set_node_mde_version_by_name(
    conn: &Connection,
    name: &str,
    version: Option<&str>,
) -> Result<bool> {
    let n = conn
        .execute(
            "UPDATE nodes SET mde_version = ? WHERE name = ?",
            (version, name),
        )
        .with_context(|| format!("setting mde_version for {name}"))?;
    Ok(n > 0)
}

/// Replace a node's `public_key` (Phase 12.3.5 re-enrollment).
/// Updates `enrolled_at` to NOW. Does NOT touch lifecycle state, so
/// a previously-decommissioned node keeps its `decommissioned` role
/// until the operator explicitly re-promotes it.
///
/// # Errors
///
/// Returns an error when the update fails.
pub fn refresh_node_credentials(
    conn: &Connection,
    node_id: &str,
    new_public_key: &str,
) -> Result<usize> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE nodes SET public_key = ?, enrolled_at = ? WHERE node_id = ?",
        (new_public_key, &now, node_id),
    )
    .with_context(|| format!("refreshing credentials for {node_id}"))
}

/// Upsert a node row. New rows land with role=`peer` and
/// health=`unknown`; existing rows have their `name` + `public_key`
/// + `region` columns refreshed.
///
/// # Errors
///
/// Returns an error when the upsert fails.
pub fn upsert_node(
    conn: &Connection,
    node_id: &str,
    name: &str,
    public_key: &str,
    region: Option<&str>,
) -> Result<usize> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO nodes (node_id, name, public_key, enrolled_at, region) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(node_id) DO UPDATE SET \
            name = excluded.name, \
            public_key = excluded.public_key, \
            region = excluded.region",
        (node_id, name, public_key, &now, region),
    )
    .with_context(|| format!("upserting node {node_id}"))
}

fn encode_sha256_hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for &b in bytes {
        out.push(char::from(HEX[(b >> 4) as usize]));
        out.push(char::from(HEX[(b & 0xf) as usize]));
    }
    out
}

fn decode_sha256_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_applies_every_migration() {
        let conn = open_in_memory().expect("open");
        let n = applied_migration_count(&conn).expect("count");
        assert_eq!(usize::try_from(n).expect("count fits"), MIGRATIONS.len());
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_in_memory().expect("open");
        let before = applied_migration_count(&conn).expect("count");
        migrate(&conn).expect("re-migrate");
        let after = applied_migration_count(&conn).expect("count");
        assert_eq!(before, after, "re-running migrate must be a no-op");
    }

    #[test]
    fn nodes_table_accepts_only_known_roles() {
        let conn = open_in_memory().expect("open");
        // Bogus role rejected by CHECK constraint.
        let res = conn.execute(
            "INSERT INTO nodes (node_id, name, public_key, enrolled_at, role) \
             VALUES ('n1','one','pk','2026-01-01T00:00:00Z','grand-admiral')",
            [],
        );
        assert!(res.is_err(), "bogus role must violate CHECK constraint");
    }

    #[test]
    fn desired_config_state_machine_constraint() {
        let conn = open_in_memory().expect("open");
        let bad = conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at) \
             VALUES ('me','m','{}','rejected-by-policy','2026-01-01T00:00:00Z')",
            [],
        );
        assert!(
            bad.is_err(),
            "unknown deployment state must be rejected by CHECK"
        );
    }

    #[test]
    fn upsert_node_inserts_then_updates() {
        let conn = open_in_memory().expect("open");
        let n = upsert_node(&conn, "peer:alpha", "alpha", "pk1", Some("us-west")).expect("insert");
        assert_eq!(n, 1);
        let nodes = list_nodes(&conn).expect("list");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "peer:alpha");
        assert_eq!(nodes[0].public_key, "pk1");
        assert_eq!(nodes[0].region.as_deref(), Some("us-west"));
        // Re-upserting with a different key updates in place.
        let n2 = upsert_node(&conn, "peer:alpha", "alpha", "pk2", Some("us-east")).expect("update");
        assert_eq!(n2, 1);
        let nodes = list_nodes(&conn).expect("list");
        assert_eq!(nodes.len(), 1, "upsert must not duplicate");
        assert_eq!(nodes[0].public_key, "pk2");
        assert_eq!(nodes[0].region.as_deref(), Some("us-east"));
    }

    #[test]
    fn set_node_role_transitions_to_decommissioned() {
        let conn = open_in_memory().expect("open");
        upsert_node(&conn, "peer:beta", "beta", "pk", None).expect("seed");
        let n = set_node_role(&conn, "peer:beta", "decommissioned").expect("update");
        assert_eq!(n, 1);
        let nodes = list_nodes(&conn).expect("list");
        assert_eq!(nodes[0].role, "decommissioned");
    }

    #[test]
    fn set_node_health_returns_true_on_transition_and_false_on_noop() {
        let conn = open_in_memory().expect("open");
        upsert_node(&conn, "peer:delta", "delta", "pk", None).expect("seed");
        // Default health from migration is "unknown".
        assert!(
            set_node_health(&conn, "peer:delta", "healthy").expect("first set"),
            "first transition unknown→healthy must change",
        );
        assert!(
            !set_node_health(&conn, "peer:delta", "healthy").expect("noop set"),
            "second call with same value must not change",
        );
        assert!(
            set_node_health(&conn, "peer:delta", "unreachable").expect("flip"),
            "healthy→unreachable must change",
        );
        let row = list_nodes(&conn)
            .expect("list")
            .into_iter()
            .find(|n| n.node_id == "peer:delta")
            .expect("row exists");
        assert_eq!(row.health, "unreachable");
    }

    #[test]
    fn set_node_health_returns_false_when_node_missing() {
        let conn = open_in_memory().expect("open");
        assert!(
            !set_node_health(&conn, "peer:ghost", "healthy").expect("missing-node noop"),
            "no row to update must be a clean false (not an error)",
        );
    }

    #[test]
    fn refresh_node_credentials_swaps_public_key() {
        let conn = open_in_memory().expect("open");
        upsert_node(&conn, "peer:gamma", "gamma", "pk-old", None).expect("seed");
        let n = refresh_node_credentials(&conn, "peer:gamma", "pk-new").expect("update");
        assert_eq!(n, 1);
        let nodes = list_nodes(&conn).expect("list");
        assert_eq!(nodes[0].public_key, "pk-new");
    }

    #[test]
    fn insert_event_chains_hashes() {
        let mut conn = open_in_memory().expect("open");
        let seq1 = insert_event(&mut conn, "lifecycle", "peer:alpha", r#"{"a":1}"#).expect("e1");
        let seq2 = insert_event(&mut conn, "lifecycle", "peer:alpha", r#"{"b":2}"#).expect("e2");
        assert_eq!(seq2, seq1 + 1);
        let rows = load_audit_rows(&conn).expect("load");
        assert_eq!(rows.len(), 2);
        // Chain verifies end-to-end.
        match crate::audit::verify(&rows) {
            crate::audit::VerifyOutcome::Intact { verified, .. } => assert_eq!(verified, 2),
            other => panic!("expected Intact, got {other:?}"),
        }
    }

    #[test]
    fn encode_then_decode_sha256_hex_round_trips() {
        let bytes: [u8; 32] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98,
            0x76, 0x54, 0x32, 0x10,
        ];
        let hex = encode_sha256_hex(&bytes);
        assert_eq!(hex.len(), 64);
        let decoded = decode_sha256_hex(&hex).expect("decode");
        assert_eq!(bytes, decoded);
    }

    #[test]
    fn decode_sha256_hex_rejects_bad_length_or_chars() {
        assert!(decode_sha256_hex("").is_none());
        assert!(decode_sha256_hex("xx").is_none());
        // 64 chars but bogus alphabet.
        let bogus: String = std::iter::repeat('z').take(64).collect();
        assert!(decode_sha256_hex(&bogus).is_none());
    }
}

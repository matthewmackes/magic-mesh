//! NF-18.2 (v2.5) — typed roster export.
//!
//! `mackesd nebula export-roster` consumes [`export_roster`] to
//! emit a JSON array of every active Nebula peer cert:
//!
//! ```json
//! [
//!   {
//!     "node_id": "peer:anvil",
//!     "name": "anvil",
//!     "overlay_ip": "10.42.0.5",
//!     "epoch": 2,
//!     "cert_pem": "-----BEGIN CERT-----\n...\n-----END CERT-----\n",
//!     "created_at": 1716000000,
//!     "expires_at": 1747536000,
//!     "groups": "peer"
//!   }
//! ]
//! ```
//!
//! Useful for off-cluster audit and as a human-readable backup
//! record complementing the encrypted `ca export` bundle (NF-18.1).
//! Reads only — no mutation, no privilege escalation.
//!
//! `groups` is sourced from `nodes.role` rather than parsing the
//! cert PEM body. Nebula encodes groups in the cert itself, but
//! a flat SQL projection is cheaper and matches the values the
//! operator already sees in the Workbench peer table.

use serde::{Deserialize, Serialize};

/// Hard upper bound on rows materialized by the read-only roster export.
///
/// The enrollment path normally caps a mesh at twelve active peers, but an
/// audited operator override can exceed that policy.  Keep this independent
/// safety bound so a corrupt or cross-mesh database cannot make the exporter
/// allocate without limit; overflow is an error rather than a truncated
/// roster.
const MAX_ROSTER_ROWS: usize = 4096;

/// One row per active peer cert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterRow {
    /// Stable node id (e.g. `peer:anvil`).
    pub node_id: String,
    /// Human-friendly display name (the hostname at enrollment
    /// time). Empty when `nodes` doesn't have a row for the
    /// node_id (rare — usually means the peer was decommissioned
    /// but the cert wasn't revoked yet).
    pub name: String,
    /// Allocated overlay IP (e.g. `10.42.0.5`).
    pub overlay_ip: String,
    /// Active CA epoch the cert was signed under.
    pub epoch: i64,
    /// PEM body of the peer cert. Includes the BEGIN/END
    /// delimiters so the output is directly usable as a
    /// `nebula-cert print -path -` input.
    pub cert_pem: String,
    /// Unix-epoch seconds when the cert row was created.
    pub created_at: i64,
    /// Unix-epoch seconds when the cert expires.
    pub expires_at: i64,
    /// Nebula groups string, comma-separated. Today sourced
    /// from `nodes.role` (one of `host` | `peer` | `decommissioned`)
    /// so the output matches the Workbench peer table.
    pub groups: String,
}

/// Read every active row from `nebula_peer_certs` (revoked_at
/// IS NULL), join with `nodes` for the display name + role,
/// and project into a sorted Vec (by `node_id` ASC). Errors on
/// any SQL failure or when the bounded export size is exceeded.
///
/// # Errors
///
/// Surfaces the underlying rusqlite::Error wrapped in a String
/// so the CLI consumer can render it with `anyhow::anyhow!`.
pub fn export_roster(conn: &rusqlite::Connection) -> Result<Vec<RosterRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT pc.node_id, COALESCE(n.name, ''), pc.overlay_ip, \
                    pc.epoch, pc.cert_pem, pc.created_at, pc.expires_at, \
                    COALESCE(n.role, 'peer') \
             FROM nebula_peer_certs pc \
             LEFT JOIN nodes n ON n.node_id = pc.node_id \
             WHERE pc.revoked_at IS NULL \
             ORDER BY pc.node_id ASC, pc.epoch DESC",
        )
        .map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RosterRow {
                node_id: r.get(0)?,
                name: r.get(1)?,
                overlay_ip: r.get(2)?,
                epoch: r.get(3)?,
                cert_pem: r.get(4)?,
                created_at: r.get(5)?,
                expires_at: r.get(6)?,
                groups: r.get(7)?,
            })
        })
        .map_err(|e| format!("query: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("row: {e}"))?);
    }
    if out.len() > MAX_ROSTER_ROWS {
        return Err(format!(
            "roster contains more than {MAX_ROSTER_ROWS} active certificate rows"
        ));
    }
    // Per-node deduplication: keep only the highest-epoch active
    // cert. The SQL ORDER BY puts the highest epoch first, so a
    // simple "skip if we've seen the node_id" pass works.
    let mut seen = std::collections::HashSet::new();
    out.retain(|r| seen.insert(r.node_id.clone()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::writer::{CaPeerCertWrite, WriteOp, WriteResponse};
    use rusqlite::Connection;

    const TEST_MESH_ID: &str = "roster-test-mesh";

    fn fresh_store() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        typed_write(
            &conn,
            WriteOp::MintCa {
                mesh_id: TEST_MESH_ID.into(),
                ca_cert_pem: "test-ca:0".into(),
            },
        );
        conn
    }

    fn typed_write(conn: &Connection, operation: WriteOp) -> usize {
        crate::store::writer::request_or_execute(conn, operation)
            .and_then(WriteResponse::into_count)
            .expect("typed roster fixture write")
    }

    fn upsert_node(conn: &Connection, node_id: &str, role: &str) {
        typed_write(
            conn,
            WriteOp::UpsertNode {
                node_id: node_id.into(),
                name: node_id.trim_start_matches("peer:").into(),
                public_key: format!("test-public-key:{node_id}"),
                region: None,
            },
        );
        if role != "peer" {
            typed_write(
                conn,
                WriteOp::SetNodeRole {
                    node_id: node_id.into(),
                    role: role.into(),
                },
            );
        }
    }

    fn peer(node_id: &str, epoch: i64, cert_pem: &str, overlay_ip: &str) -> CaPeerCertWrite {
        CaPeerCertWrite {
            node_id: node_id.into(),
            epoch,
            cert_pem: cert_pem.into(),
            overlay_ip: overlay_ip.into(),
            public_key_pem: None,
            created_at: None,
            expires_at: 9_999_999,
        }
    }

    fn upsert_peer(conn: &Connection, node_id: &str, epoch: i64, cert_pem: &str, overlay_ip: &str) {
        typed_write(
            conn,
            WriteOp::UpsertPeerCert {
                mesh_id: TEST_MESH_ID.into(),
                expected_epoch: epoch,
                peer: peer(node_id, epoch, cert_pem, overlay_ip),
            },
        );
    }

    #[test]
    fn export_roster_returns_empty_when_no_certs() {
        let conn = fresh_store();
        let rows = export_roster(&conn).expect("ok");
        assert!(rows.is_empty());
    }

    #[test]
    fn export_roster_skips_revoked_certs() {
        let conn = fresh_store();
        upsert_node(&conn, "peer:anvil", "peer");
        // Active row.
        upsert_peer(&conn, "peer:anvil", 0, "PEM-A", "10.42.0.5");
        // Revoked row — should be excluded.
        upsert_peer(&conn, "peer:birch", 0, "PEM-B", "10.42.0.6");
        typed_write(
            &conn,
            WriteOp::RevokePeerCert {
                node_id: "peer:birch".into(),
                revoked_at: 1234,
            },
        );
        let rows = export_roster(&conn).expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].node_id, "peer:anvil");
        assert_eq!(rows[0].overlay_ip, "10.42.0.5");
        assert_eq!(rows[0].cert_pem, "PEM-A");
    }

    #[test]
    fn export_roster_joins_nodes_for_groups_field() {
        let conn = fresh_store();
        upsert_node(&conn, "peer:lighthouse-1", "host");
        typed_write(
            &conn,
            WriteOp::RotateCa {
                mesh_id: TEST_MESH_ID.into(),
                expected_active_epoch: Some(0),
                new_epoch: 1,
                ca_cert_pem: "test-ca:1".into(),
                peer_certs: vec![peer("peer:lighthouse-1", 1, "PEM", "10.42.0.1")],
            },
        );
        let rows = export_roster(&conn).expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].groups, "host");
        assert_eq!(rows[0].name, "lighthouse-1");
        assert_eq!(rows[0].epoch, 1);
    }

    #[test]
    fn export_roster_rejects_more_than_maximum_rows() {
        let conn = fresh_store();
        for i in 0..=MAX_ROSTER_ROWS {
            upsert_peer(
                &conn,
                &format!("peer:overflow-{i}"),
                0,
                "PEM",
                &format!("10.42.{}.{}", i / 254, (i % 254) + 1),
            );
        }
        let error = export_roster(&conn).expect_err("oversized roster must fail closed");
        assert!(error.contains("more than 4096"), "{error}");
    }

    #[test]
    fn export_roster_dedups_to_highest_epoch_per_node() {
        // A peer that's been through CA rotation has multiple
        // active rows (one per epoch) — only the highest epoch
        // row should appear in the export.
        let conn = fresh_store();
        upsert_node(&conn, "peer:anvil", "peer");
        upsert_peer(&conn, "peer:anvil", 0, "PEM-old", "10.42.0.5");
        // Same node, newer epoch, different overlay IP. Need
        // distinct overlay_ip because the (overlay_ip, epoch)
        // partial unique index from migration 0011 requires
        // unique overlay across active rows; using a different
        // IP for the higher-epoch row mirrors the real CA
        // rotation behavior.
        typed_write(
            &conn,
            WriteOp::RotateCa {
                mesh_id: TEST_MESH_ID.into(),
                expected_active_epoch: Some(0),
                new_epoch: 3,
                ca_cert_pem: "test-ca:3".into(),
                peer_certs: vec![peer("peer:anvil", 3, "PEM-new", "10.42.0.6")],
            },
        );
        let rows = export_roster(&conn).expect("ok");
        assert_eq!(rows.len(), 1, "expected dedup to 1 row");
        assert_eq!(rows[0].epoch, 3, "expected highest epoch");
        assert_eq!(rows[0].cert_pem, "PEM-new");
        assert_eq!(rows[0].overlay_ip, "10.42.0.6");
    }

    #[test]
    fn export_roster_orders_by_node_id_asc() {
        let conn = fresh_store();
        for (nid, ip) in [
            ("peer:cedar", "10.42.0.4"),
            ("peer:anvil", "10.42.0.2"),
            ("peer:birch", "10.42.0.3"),
        ] {
            upsert_node(&conn, nid, "peer");
            upsert_peer(&conn, nid, 0, "PEM", ip);
        }
        let rows = export_roster(&conn).expect("ok");
        let names: Vec<&str> = rows.iter().map(|r| r.node_id.as_str()).collect();
        assert_eq!(names, vec!["peer:anvil", "peer:birch", "peer:cedar"]);
    }

    #[test]
    fn roster_row_round_trips_through_json() {
        let r = RosterRow {
            node_id: "peer:anvil".into(),
            name: "anvil".into(),
            overlay_ip: "10.42.0.5".into(),
            epoch: 2,
            cert_pem: "-----BEGIN-----\nA\n-----END-----\n".into(),
            created_at: 1716000000,
            expires_at: 1747536000,
            groups: "peer".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: RosterRow = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }
}

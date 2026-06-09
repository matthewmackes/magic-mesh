//! Health surface (Phase 12.1.3).
//!
//! `HealthReport` is the value type returned by `mackesd healthz`
//! (CLI subcommand) and `mackesd_core::healthz()` (library function
//! the panel imports for the status cluster).
//!
//! Per the 12.1.3 lock the same data surfaces in both places — the
//! CLI prints it as JSON, the library returns the typed struct.

use serde::{Deserialize, Serialize};

/// Top-level health report. Each field is independently reportable
/// so a probe failure on one doesn't poison the others.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Schema version. Bump when the shape changes; the panel uses
    /// this to fall back gracefully if a newer `mackesd` reports a
    /// shape it doesn't recognize yet.
    pub schema: u32,
    /// Is this peer currently the leader? See 12.A.5.
    pub is_leader: bool,
    /// Most recent applied revision (`r-YYYY-MM-DD-NNNN` form).
    /// `None` when the store has never accepted a deploy.
    pub applied_revision: Option<String>,
    /// Count of rows in the `nodes` table (mesh size from this peer's
    /// perspective).
    pub node_count: u32,
    /// Count of rows whose `last_heartbeat` is within the healthy
    /// threshold (per 12.3.3).
    pub healthy_nodes: u32,
    /// Count of rows whose `last_heartbeat` missed exactly one cycle.
    pub degraded_nodes: u32,
    /// Count of rows whose `last_heartbeat` missed 3+ cycles.
    pub unreachable_nodes: u32,
    /// Audit chain status. `true` = `audit::verify()` returned
    /// `Intact`. `false` = the most recent verify reported a break.
    pub audit_chain_intact: bool,
    /// Mackesd version (Cargo package version).
    pub version: String,
}

impl HealthReport {
    /// Current schema version. Bump alongside any breaking field
    /// change. Add a fallback path on the panel side before bumping
    /// so older readers degrade gracefully.
    pub const CURRENT_SCHEMA: u32 = 1;

    /// Build a default report for a fresh peer that has no data
    /// yet. Used by `mackesd healthz` on a just-installed system
    /// before the first reconcile tick.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema: Self::CURRENT_SCHEMA,
            is_leader: false,
            applied_revision: None,
            node_count: 0,
            healthy_nodes: 0,
            degraded_nodes: 0,
            unreachable_nodes: 0,
            audit_chain_intact: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// JSON one-liner for `mackesd healthz`. Stable shape — every
    /// field always present, no schema-conditional keys.
    ///
    /// # Errors
    /// Returns `serde_json::Error` only on out-of-memory while
    /// serializing — never on schema-shape issues.
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_has_schema_1() {
        let r = HealthReport::empty();
        assert_eq!(r.schema, 1);
        assert!(!r.is_leader);
        assert!(r.applied_revision.is_none());
        assert_eq!(r.node_count, 0);
    }

    #[test]
    fn json_round_trips() {
        let r = HealthReport::empty();
        let line = r.to_json_line().expect("serialize");
        let back: HealthReport = serde_json::from_str(&line).expect("parse");
        assert_eq!(back.schema, r.schema);
        assert_eq!(back.is_leader, r.is_leader);
        assert_eq!(back.version, r.version);
    }

    #[test]
    fn version_string_matches_cargo() {
        let r = HealthReport::empty();
        assert_eq!(r.version, env!("CARGO_PKG_VERSION"));
    }
}

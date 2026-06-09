//! Structured logging helpers (Phase 12.1.4).
//!
//! The `mackesd` binary initializes a `tracing` subscriber that
//! emits JSON lines. Every log line carries `correlation_id`,
//! `node_id`, `revision_id`, `span`, `level`. This module ships
//! the helper that opens a correlation-scoped span — the worker
//! loop opens one span per tick + every log line inside the tick
//! inherits the same correlation id.

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter for fresh correlation IDs. Reset on process
/// restart. Cheap, never blocks, never collides within a single
/// `mackesd` run (which is what correlation ids need).
static NEXT_CORRELATION: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh correlation id for a new tracing scope.
#[must_use]
pub fn next_correlation_id() -> u64 {
    NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed)
}

/// Stable schema for log line context. The subscriber renders each
/// field as a top-level JSON key for grep-ability.
#[derive(Debug, Clone)]
pub struct LogContext {
    /// Monotonic correlation id (per-tick).
    pub correlation_id: u64,
    /// Optional node id (None when the log line is about the mesh
    /// as a whole rather than a specific peer).
    pub node_id: Option<String>,
    /// Optional applied-revision id.
    pub revision_id: Option<String>,
}

impl LogContext {
    /// New context with a freshly-allocated correlation id + no
    /// per-node / per-revision scope.
    #[must_use]
    pub fn fresh() -> Self {
        Self {
            correlation_id: next_correlation_id(),
            node_id: None,
            revision_id: None,
        }
    }

    /// Attach a node id (consumes self, returns new context).
    #[must_use]
    pub fn with_node(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    /// Attach a revision id.
    #[must_use]
    pub fn with_revision(mut self, revision_id: impl Into<String>) -> Self {
        self.revision_id = Some(revision_id.into());
        self
    }

    /// Render as JSON for inclusion in a log line. Stable shape so
    /// downstream tooling (`jq`, log shipper) can rely on it.
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "correlation_id": self.correlation_id,
            "node_id": self.node_id,
            "revision_id": self.revision_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_ids_are_unique_per_call() {
        let a = next_correlation_id();
        let b = next_correlation_id();
        let c = next_correlation_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert!(b > a && c > b);
    }

    #[test]
    fn fresh_context_starts_unscoped() {
        let ctx = LogContext::fresh();
        assert!(ctx.node_id.is_none());
        assert!(ctx.revision_id.is_none());
        assert!(ctx.correlation_id > 0);
    }

    #[test]
    fn builder_attaches_node_and_revision() {
        let ctx = LogContext::fresh()
            .with_node("peer:a")
            .with_revision("r-2026-05-19-0001");
        assert_eq!(ctx.node_id.as_deref(), Some("peer:a"));
        assert_eq!(ctx.revision_id.as_deref(), Some("r-2026-05-19-0001"));
    }

    #[test]
    fn json_value_carries_every_field() {
        let ctx = LogContext::fresh()
            .with_node("peer:b")
            .with_revision("r-2026-05-19-0007");
        let v = ctx.to_json_value();
        assert!(v["correlation_id"].is_number());
        assert_eq!(v["node_id"], "peer:b");
        assert_eq!(v["revision_id"], "r-2026-05-19-0007");
    }
}

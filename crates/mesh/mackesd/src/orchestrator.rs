//! Phase 2.6 — operation orchestrator state machine.
//!
//! Owns the lifecycle of one Send-To operation from arrival to
//! audit-log close. The state machine:
//!
//!   ```text
//!   Pending → Validating → Executing → Verifying → Completed
//!           ↘            ↘            ↘
//!           Rejected     Failed       Failed
//!   ```
//!
//! Inputs / outputs are pure data so the orchestrator's policy is
//! testable without touching the network or the filesystem:
//!
//!   * [`Orchestrator::new`] takes the SQLite handle (or, in
//!     tests, an in-memory event log).
//!   * [`Orchestrator::accept`] takes a [`Request`] (per the
//!     pre-flight module) + a validated path set from
//!     [`crate::path_safety::PathPolicy::validate`]; returns an
//!     [`OperationId`] + the matching [`AuditId`] (paired so the
//!     audit row + the live progress stream key off the same id).
//!   * [`Orchestrator::advance`] is the reducer the worker pool
//!     calls when a stage completes; it transitions the state +
//!     emits a [`ProgressEvent`].
//!   * [`Orchestrator::operation`] returns the read-only
//!     snapshot the panel + reconciler consume.
//!
//! `dev.mackes.MDE.Shell.Send` is the live D-Bus surface; this
//! module is the engine behind it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::path_safety::{PathError, PathPolicy};
use crate::preflight::{
    preflight, rows_allow_send, ConflictPolicyLite, Request as PreflightRequest, SendModeLite,
};

/// Stable identifier for one Send-To operation. Monotonically
/// allocated by [`Orchestrator::accept`]; the panel + the
/// reconciler key off this.
pub type OperationId = u64;

/// Audit-log row id. Equal to the operation id at creation time
/// — kept distinct so future per-step audit rows (validate,
/// execute, verify) can share the same parent op while having
/// their own ids.
pub type AuditId = u64;

/// State machine stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Just arrived; queued for validation.
    Pending,
    /// Pre-flight checks running.
    Validating,
    /// Validation passed; queued for execution.
    Executing,
    /// Execution finished; verifying checksum / target.
    Verifying,
    /// Verification passed; final audit row written.
    Completed,
    /// Pre-flight refused the request.
    Rejected,
    /// Execute or verify hit an error.
    Failed,
}

impl Stage {
    /// Whether the orchestrator can still transition out of this
    /// stage. `Completed`, `Rejected`, `Failed` are terminal.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Rejected | Self::Failed)
    }
}

/// One progress event emitted whenever the orchestrator advances.
/// The D-Bus surface forwards these onto the
/// `dev.mackes.MDE.Shell.Progress` signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    /// Op the event belongs to.
    pub op_id: OperationId,
    /// Stage the op transitioned INTO.
    pub stage: Stage,
    /// Free-form message for the audit / log (empty for clean
    /// transitions).
    pub message: String,
}

/// Read-only view of one operation. Panels + reconciler iterate
/// these via [`Orchestrator::operations_sorted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationView {
    /// Op id (matches the audit id at create time).
    pub op_id: OperationId,
    /// Audit id.
    pub audit_id: AuditId,
    /// Current stage.
    pub stage: Stage,
    /// Sources (canonicalised paths).
    pub sources: Vec<PathBuf>,
    /// Destination label.
    pub destination_label: String,
    /// Send mode.
    pub mode: SendModeLite,
    /// Conflict policy.
    pub conflict: ConflictPolicyLite,
    /// Last progress message; empty on the happy path.
    pub last_message: String,
}

/// Errors the orchestrator surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorError {
    /// One of the sources failed path-safety validation.
    PathRejected(PathError),
    /// Pre-flight checks blocked the request. The first row to
    /// fail is surfaced for the UI.
    PreflightBlocked {
        /// Identifier of the first failing check row.
        check: String,
        /// Caller-visible message.
        message: String,
    },
    /// Tried to advance an op that's already terminal.
    AlreadyTerminal {
        /// Op id.
        op_id: OperationId,
        /// Stage it was in.
        stage: Stage,
    },
    /// Tried to advance an op that doesn't exist.
    UnknownOperation(OperationId),
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathRejected(e) => write!(f, "orchestrator: {e}"),
            Self::PreflightBlocked { check, message } => {
                write!(
                    f,
                    "orchestrator: preflight check {check} blocked: {message}"
                )
            }
            Self::AlreadyTerminal { op_id, stage } => write!(
                f,
                "orchestrator: op {op_id} is already terminal at {stage:?}"
            ),
            Self::UnknownOperation(id) => {
                write!(f, "orchestrator: unknown op {id}")
            }
        }
    }
}

impl std::error::Error for OrchestratorError {}

/// In-process orchestrator. Allocates ids, tracks live operations,
/// records every transition into an in-memory event log. The
/// SQLite-backed persistence is the same shape but lives behind a
/// future trait swap when 2.7's BLAKE3+SHA-256 storage lands.
#[derive(Debug, Default)]
pub struct Orchestrator {
    next_id: AtomicU64,
    operations: HashMap<OperationId, OperationView>,
    events: Vec<ProgressEvent>,
}

impl Orchestrator {
    /// Construct an empty orchestrator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            operations: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// Accept a Send-To request. Runs path safety + pre-flight;
    /// allocates an op id + audit id; records the initial
    /// `Pending` event.
    ///
    /// # Errors
    ///
    /// * [`OrchestratorError::PathRejected`] — one source failed
    ///   validation.
    /// * [`OrchestratorError::PreflightBlocked`] — pre-flight
    ///   refused the request.
    pub fn accept(
        &mut self,
        request: PreflightRequest,
        policy: &PathPolicy,
    ) -> Result<OperationId, OrchestratorError> {
        // Path safety on every source first.
        let mut canonical: Vec<PathBuf> = Vec::with_capacity(request.sources.len());
        for src in &request.sources {
            let p = policy
                .validate(src)
                .map_err(OrchestratorError::PathRejected)?;
            canonical.push(p);
        }
        // Pre-flight battery.
        let rows = preflight(&request, policy);
        if !rows_allow_send(&rows) {
            let blocker = rows
                .iter()
                .find(|r| matches!(r.status, crate::preflight::CheckStatus::Block))
                .expect("rows_allow_send=false implies a Block row");
            return Err(OrchestratorError::PreflightBlocked {
                check: blocker.id.to_string(),
                message: blocker.message.clone(),
            });
        }

        let op_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let audit_id = op_id;
        let view = OperationView {
            op_id,
            audit_id,
            stage: Stage::Pending,
            sources: canonical,
            destination_label: request.destination_label,
            mode: request.mode,
            conflict: request.conflict,
            last_message: String::new(),
        };
        self.operations.insert(op_id, view);
        self.events.push(ProgressEvent {
            op_id,
            stage: Stage::Pending,
            message: String::new(),
        });
        Ok(op_id)
    }

    /// Advance an op to the next stage. Returns the new stage; the
    /// caller decides whether to schedule the next worker step.
    ///
    /// Transition table:
    ///   * Pending     → Validating
    ///   * Validating  → Executing | Rejected (with message)
    ///   * Executing   → Verifying | Failed (with message)
    ///   * Verifying   → Completed | Failed (with message)
    ///
    /// Passing `next_failed=true` short-circuits to the matching
    /// failure terminal state.
    ///
    /// # Errors
    ///
    /// * [`OrchestratorError::UnknownOperation`] when `op_id` was
    ///   never accepted.
    /// * [`OrchestratorError::AlreadyTerminal`] when the op
    ///   already reached `Completed` / `Rejected` / `Failed`.
    pub fn advance(
        &mut self,
        op_id: OperationId,
        next_failed: bool,
        message: impl Into<String>,
    ) -> Result<Stage, OrchestratorError> {
        let message = message.into();
        let op = self
            .operations
            .get_mut(&op_id)
            .ok_or(OrchestratorError::UnknownOperation(op_id))?;
        if op.stage.is_terminal() {
            return Err(OrchestratorError::AlreadyTerminal {
                op_id,
                stage: op.stage,
            });
        }
        let next = next_stage(op.stage, next_failed);
        op.stage = next;
        op.last_message.clone_from(&message);
        self.events.push(ProgressEvent {
            op_id,
            stage: next,
            message,
        });
        Ok(next)
    }

    /// Read-only view of one operation.
    #[must_use]
    pub fn operation(&self, op_id: OperationId) -> Option<&OperationView> {
        self.operations.get(&op_id)
    }

    /// Every operation, sorted by op id ascending.
    #[must_use]
    pub fn operations_sorted(&self) -> Vec<&OperationView> {
        let mut v: Vec<&OperationView> = self.operations.values().collect();
        v.sort_by_key(|o| o.op_id);
        v
    }

    /// Every emitted event in arrival order. Test + audit
    /// consumers iterate this; the live D-Bus signal forwards
    /// them.
    #[must_use]
    pub fn events(&self) -> &[ProgressEvent] {
        &self.events
    }

    /// Number of live + terminal operations tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// `true` when no operations have been accepted yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

/// Pure-fn transition table. Public so unit tests can pin every
/// transition.
#[must_use]
pub fn next_stage(current: Stage, failed: bool) -> Stage {
    match (current, failed) {
        (Stage::Pending, _) => Stage::Validating,
        (Stage::Validating, false) => Stage::Executing,
        (Stage::Validating, true) => Stage::Rejected,
        (Stage::Executing, false) => Stage::Verifying,
        (Stage::Executing, true) => Stage::Failed,
        (Stage::Verifying, false) => Stage::Completed,
        (Stage::Verifying, true) => Stage::Failed,
        // Terminal stages stay where they are.
        (Stage::Completed, _) => Stage::Completed,
        (Stage::Rejected, _) => Stage::Rejected,
        (Stage::Failed, _) => Stage::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_safety::AllowedRoot;
    use std::fs;
    use tempfile::tempdir;

    fn policy_for(root: &std::path::Path) -> PathPolicy {
        let mut p = PathPolicy::empty();
        p.allow(AllowedRoot::new(root, "scratch").expect("canonicalise"));
        p
    }

    fn happy_request(root: &std::path::Path) -> PreflightRequest {
        let src = root.join("a.txt");
        fs::write(&src, b"hello").unwrap();
        PreflightRequest {
            sources: vec![src],
            destination_label: "pine".into(),
            total_bytes: 5,
            destination_free_bytes: 1_000_000,
            destination_last_seen_ms: 100,
            rollback_available_for_target: true,
            target_exists: false,
            mode: SendModeLite::Copy,
            conflict: ConflictPolicyLite::Ask,
        }
    }

    #[test]
    fn accept_returns_monotonic_ids() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut o = Orchestrator::new();
        let id1 = o.accept(happy_request(tmp.path()), &policy).unwrap();
        let id2 = o.accept(happy_request(tmp.path()), &policy).unwrap();
        assert!(id2 > id1);
    }

    #[test]
    fn accept_rejects_traversal_request() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut req = happy_request(tmp.path());
        req.sources = vec![tmp.path().join("..").join("escape")];
        let mut o = Orchestrator::new();
        let err = o.accept(req, &policy).unwrap_err();
        assert!(matches!(err, OrchestratorError::PathRejected(_)));
    }

    #[test]
    fn accept_blocks_when_preflight_fails() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut req = happy_request(tmp.path());
        // Force the disk-space check to block.
        req.destination_free_bytes = 1;
        req.total_bytes = 1_000_000;
        let mut o = Orchestrator::new();
        let err = o.accept(req, &policy).unwrap_err();
        match err {
            OrchestratorError::PreflightBlocked { check, .. } => {
                assert_eq!(check, "disk-space");
            }
            other => panic!("expected PreflightBlocked, got {other:?}"),
        }
    }

    #[test]
    fn happy_path_walks_pending_to_completed() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut o = Orchestrator::new();
        let id = o.accept(happy_request(tmp.path()), &policy).unwrap();
        assert_eq!(o.operation(id).unwrap().stage, Stage::Pending);

        assert_eq!(o.advance(id, false, "").unwrap(), Stage::Validating);
        assert_eq!(o.advance(id, false, "").unwrap(), Stage::Executing);
        assert_eq!(o.advance(id, false, "").unwrap(), Stage::Verifying);
        assert_eq!(o.advance(id, false, "").unwrap(), Stage::Completed);
    }

    #[test]
    fn next_stage_table_covers_every_pair() {
        // Failing in Validating → Rejected.
        assert_eq!(next_stage(Stage::Validating, true), Stage::Rejected);
        // Failing in Executing → Failed.
        assert_eq!(next_stage(Stage::Executing, true), Stage::Failed);
        // Failing in Verifying → Failed.
        assert_eq!(next_stage(Stage::Verifying, true), Stage::Failed);
        // Terminal stages stay put.
        assert_eq!(next_stage(Stage::Completed, false), Stage::Completed);
        assert_eq!(next_stage(Stage::Rejected, true), Stage::Rejected);
        assert_eq!(next_stage(Stage::Failed, false), Stage::Failed);
    }

    #[test]
    fn advance_after_completed_errors() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut o = Orchestrator::new();
        let id = o.accept(happy_request(tmp.path()), &policy).unwrap();
        for _ in 0..4 {
            o.advance(id, false, "").unwrap();
        }
        let err = o.advance(id, false, "after the fact").unwrap_err();
        assert!(matches!(err, OrchestratorError::AlreadyTerminal { .. }));
    }

    #[test]
    fn advance_unknown_op_errors() {
        let mut o = Orchestrator::new();
        let err = o.advance(999, false, "").unwrap_err();
        assert!(matches!(err, OrchestratorError::UnknownOperation(999)));
    }

    #[test]
    fn validating_can_short_circuit_to_rejected_with_message() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut o = Orchestrator::new();
        let id = o.accept(happy_request(tmp.path()), &policy).unwrap();
        o.advance(id, false, "").unwrap();
        let stage = o.advance(id, true, "policy refused").unwrap();
        assert_eq!(stage, Stage::Rejected);
        assert_eq!(o.operation(id).unwrap().last_message, "policy refused");
    }

    #[test]
    fn events_are_emitted_in_order() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut o = Orchestrator::new();
        let id = o.accept(happy_request(tmp.path()), &policy).unwrap();
        o.advance(id, false, "").unwrap();
        o.advance(id, false, "").unwrap();
        let events = o.events();
        // Pending + 2 transitions.
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].stage, Stage::Pending);
        assert_eq!(events[1].stage, Stage::Validating);
        assert_eq!(events[2].stage, Stage::Executing);
        for e in events {
            assert_eq!(e.op_id, id);
        }
    }

    #[test]
    fn operations_sorted_returns_by_op_id() {
        let tmp = tempdir().unwrap();
        let policy = policy_for(tmp.path());
        let mut o = Orchestrator::new();
        let a = o.accept(happy_request(tmp.path()), &policy).unwrap();
        let b = o.accept(happy_request(tmp.path()), &policy).unwrap();
        let c = o.accept(happy_request(tmp.path()), &policy).unwrap();
        let ids: Vec<_> = o.operations_sorted().iter().map(|o| o.op_id).collect();
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn stage_is_terminal_correctly() {
        assert!(!Stage::Pending.is_terminal());
        assert!(!Stage::Validating.is_terminal());
        assert!(!Stage::Executing.is_terminal());
        assert!(!Stage::Verifying.is_terminal());
        assert!(Stage::Completed.is_terminal());
        assert!(Stage::Rejected.is_terminal());
        assert!(Stage::Failed.is_terminal());
    }

    #[test]
    fn orchestrator_error_display_includes_context() {
        let e = OrchestratorError::UnknownOperation(7);
        assert!(format!("{e}").contains("7"));
        let e = OrchestratorError::AlreadyTerminal {
            op_id: 3,
            stage: Stage::Completed,
        };
        assert!(format!("{e}").contains("3"));
        assert!(format!("{e}").contains("Completed"));
    }
}

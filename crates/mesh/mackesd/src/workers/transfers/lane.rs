//! TRANSFERS-1 — the injectable `LaneRunner` execution seam.
//!
//! The queue/ledger/verb spine owns lifecycle; it does NOT know how to move a byte.
//! Execution is delegated to a [`LaneRunner`] — the seam the per-protocol lanes
//! (TRANSFERS-2..6: sftp / rsync / wget / node / music) implement. TRANSFERS-1 ships
//! the trait plus [`GatedLaneRunner`], the **honest typed gate**: it runs no tool
//! and fabricates no success (§7) — it returns a [`LaneOutcome::Failed`] naming the
//! lane that has to land. Swapping in a real, method-dispatching `LaneRunner` is all
//! a lane wave has to do; the spine is unchanged.

#![cfg(feature = "async-services")]

use super::job::TransferJob;

/// What a lane reports when its run finishes.
///
/// There is no `Progress` variant here: live progress is written onto
/// [`TransferJob::progress`] by a lane as it runs; a `LaneOutcome` is the FINAL word
/// (the queue maps it to `Done`/`Failed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneOutcome {
    /// The move completed (and, when `policy.verify`, verified) — terminal `Done`.
    Done,
    /// The move ended in an honest failure — the reason is surfaced on the job's
    /// `error` (§7). Also the shape the [`GatedLaneRunner`] returns for every method.
    Failed {
        /// A human-readable, non-fabricated failure reason.
        error: String,
    },
}

impl LaneOutcome {
    /// Build a failure outcome from anything string-like.
    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self::Failed {
            error: error.into(),
        }
    }
}

/// The seam every lane implements.
///
/// `run` executes ONE job to completion (or honest failure) and returns its
/// [`LaneOutcome`]; the worker spawns it on a task and applies the outcome when it
/// finishes. A running task is aborted on `cancel`/`pause` (tokio task-abort → the
/// lane's `tokio::process` child is killed on drop), so lanes need no explicit cancel
/// channel in TRANSFERS-1.
#[async_trait::async_trait]
pub trait LaneRunner: Send + Sync {
    /// Execute `job` and report the outcome. Must not panic — a lane surfaces every
    /// failure as [`LaneOutcome::Failed`] with an honest reason.
    async fn run(&self, job: &TransferJob) -> LaneOutcome;
}

/// The TRANSFERS-1 default: no lane is wired yet, so every method fails honestly,
/// naming the lane that must land (§7 — never a fake success, never fake progress).
#[derive(Debug, Default, Clone, Copy)]
pub struct GatedLaneRunner;

#[async_trait::async_trait]
impl LaneRunner for GatedLaneRunner {
    async fn run(&self, job: &TransferJob) -> LaneOutcome {
        LaneOutcome::failed(format!(
            "the `{}` transfer lane is not yet wired \u{2014} TRANSFERS-2..6 implement execution",
            job.method
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::transfers::job::{Method, TransferPolicy};

    #[tokio::test]
    async fn gated_lane_fails_every_method_honestly() {
        let lane = GatedLaneRunner;
        for m in Method::ALL {
            let job = TransferJob::new("/a", "/b", m, TransferPolicy::default());
            let outcome = lane.run(&job).await;
            // The gate must never fake success (\u{a7}7).
            assert!(
                matches!(outcome, LaneOutcome::Failed { .. }),
                "the gate returned Done for {m}"
            );
            if let LaneOutcome::Failed { error } = outcome {
                assert!(error.contains(m.as_str()), "names the lane: {error}");
                assert!(
                    error.contains("TRANSFERS-2..6"),
                    "points at the lanes: {error}"
                );
            }
        }
    }
}

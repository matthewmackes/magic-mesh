//! TRANSFERS-1 — the queue engine: the state machine + the parallel cap over the
//! persistent [`Ledger`].
//!
//! This is the synchronous heart the acceptance rests on (submit→list, the state
//! machine, cap enforcement, restart recovery) — no async, no lanes. The worker
//! ([`super::TransfersWorker`]) drives it: it applies inbox verbs, calls
//! [`TransferQueue::claim_next`] to fill up to the cap, and [`TransferQueue::complete`]
//! when a lane task finishes. Every mutation is written straight through to the
//! ledger, so the daemon holds no authoritative in-memory state to lose on restart.

#![cfg(feature = "async-services")]

use std::io;
use std::path::Path;

use super::job::{TransferJob, TransferState, Transition};
use super::lane::LaneOutcome;
use super::ledger::Ledger;

/// Why a control verb could not be applied to a job (the honest, typed refusal the
/// CLI/GUI render — never a silent no-op).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// No job with this id in the ledger.
    NotFound(String),
    /// The verb is illegal from the job's current state (e.g. resume a Running job).
    IllegalTransition {
        /// The job id.
        id: String,
        /// The state the job is actually in.
        from: TransferState,
        /// The verb that was refused.
        verb: &'static str,
    },
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "no transfer `{id}` in the ledger"),
            Self::IllegalTransition { id, from, verb } => {
                write!(f, "cannot {verb} transfer `{id}` in state {from}")
            }
        }
    }
}

impl std::error::Error for QueueError {}

/// The queue over a persistent ledger, bounded by a parallel `cap` (Q12).
#[derive(Debug, Clone)]
pub struct TransferQueue {
    ledger: Ledger,
    cap: usize,
}

impl TransferQueue {
    /// Open the queue over `store_root` with a parallel `cap` (>= 1).
    ///
    /// **Crash recovery:** any job left `Running` by a previous daemon (it was
    /// executing when the process died, so it is NOT actually running now) is reset
    /// to `Queued` so it is re-attempted — never left as a phantom Running slot that
    /// would wedge the cap forever.
    ///
    /// # Errors
    /// Fails if the ledger directory can't be opened.
    pub fn open(store_root: &Path, cap: usize) -> io::Result<Self> {
        let q = Self {
            ledger: Ledger::open(store_root)?,
            cap: cap.max(1),
        };
        q.recover_orphaned_running();
        Ok(q)
    }

    /// The configured parallel cap.
    #[must_use]
    pub const fn cap(&self) -> usize {
        self.cap
    }

    /// Reset every `Running` record to `Queued` (see [`Self::open`]).
    fn recover_orphaned_running(&self) {
        for mut job in self.ledger.load_all() {
            if job.state == TransferState::Running {
                job.set_state(TransferState::Queued);
                if let Err(e) = self.ledger.upsert(&job) {
                    tracing::warn!(
                        target: "mackesd::transfers",
                        id = %job.id, error = %e,
                        "could not recover an orphaned Running job to Queued",
                    );
                }
            }
        }
    }

    /// Accept a (client-minted, Queued) job into the ledger.
    ///
    /// # Errors
    /// A ledger write failure.
    pub fn submit(&self, mut job: TransferJob) -> io::Result<String> {
        // Normalize: a submit always enters Queued (the client sets it, but the
        // daemon is the authority — never trust an inbound Running/Done state).
        if job.state != TransferState::Queued {
            job.set_state(TransferState::Queued);
        }
        let id = job.id.clone();
        self.ledger.upsert(&job)?;
        Ok(id)
    }

    /// Every job, ledger order (submit time then id).
    #[must_use]
    pub fn list(&self) -> Vec<TransferJob> {
        self.ledger.load_all()
    }

    /// One job by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<TransferJob> {
        self.ledger.get(id)
    }

    /// How many jobs are currently `Running` (the live slot count the cap bounds).
    #[must_use]
    pub fn running_count(&self) -> usize {
        self.ledger
            .load_all()
            .iter()
            .filter(|j| j.state == TransferState::Running)
            .count()
    }

    /// Remove a job entirely — the design's `cancel` (there is no `Cancelled`
    /// state; a cancel discards the row and frees any slot it held). Legal from any
    /// state, including terminal (a cancel of a Done/Failed row clears history).
    ///
    /// # Errors
    /// [`QueueError::NotFound`] if no such job; a ledger IO failure surfaces as
    /// `NotFound` only when the record truly vanished, otherwise it is returned.
    pub fn cancel(&self, id: &str) -> Result<(), QueueError> {
        if self.ledger.get(id).is_none() {
            return Err(QueueError::NotFound(id.to_string()));
        }
        self.ledger.remove(id).map_err(|e| {
            tracing::warn!(target: "mackesd::transfers", id, error = %e, "cancel remove failed");
            QueueError::NotFound(id.to_string())
        })
    }

    /// Hold a Queued or Running job (operator `pause`).
    ///
    /// # Errors
    /// [`QueueError::NotFound`] / [`QueueError::IllegalTransition`].
    pub fn pause(&self, id: &str) -> Result<(), QueueError> {
        self.transition(id, Transition::Pause, "pause", TransferState::Paused)
    }

    /// Re-arm a Paused job (operator `resume`) back to Queued.
    ///
    /// # Errors
    /// [`QueueError::NotFound`] / [`QueueError::IllegalTransition`].
    pub fn resume(&self, id: &str) -> Result<(), QueueError> {
        self.transition(id, Transition::Resume, "resume", TransferState::Queued)
    }

    /// Shared guard-and-apply for the simple state transitions (pause/resume).
    fn transition(
        &self,
        id: &str,
        verb: Transition,
        verb_name: &'static str,
        to: TransferState,
    ) -> Result<(), QueueError> {
        let mut job = self
            .ledger
            .get(id)
            .ok_or_else(|| QueueError::NotFound(id.to_string()))?;
        if !job.state.can(verb) {
            return Err(QueueError::IllegalTransition {
                id: id.to_string(),
                from: job.state,
                verb: verb_name,
            });
        }
        job.set_state(to);
        self.ledger
            .upsert(&job)
            .map_err(|_| QueueError::NotFound(id.to_string()))?;
        Ok(())
    }

    /// Claim the next runnable job **iff a slot is free**, transitioning it to
    /// `Running` and persisting it. Returns `None` when the cap is reached or no
    /// job is `Queued`. This is the single point cap enforcement lives — the worker
    /// loops on it to fill up to the cap each tick.
    #[must_use]
    pub fn claim_next(&self) -> Option<TransferJob> {
        let all = self.ledger.load_all();
        let running = all
            .iter()
            .filter(|j| j.state == TransferState::Running)
            .count();
        if running >= self.cap {
            return None;
        }
        // Oldest Queued first (load_all is already time-ordered).
        let mut job = all.into_iter().find(|j| j.state == TransferState::Queued)?;
        job.set_state(TransferState::Running);
        match self.ledger.upsert(&job) {
            Ok(()) => Some(job),
            Err(e) => {
                tracing::warn!(target: "mackesd::transfers", id = %job.id, error = %e, "claim upsert failed");
                None
            }
        }
    }

    /// Apply a lane's final [`LaneOutcome`] to a `Running` job → `Done`/`Failed`.
    /// A no-op if the job is no longer Running (it was paused/cancelled while the
    /// task was in flight) — the late outcome is honestly dropped, not force-applied.
    ///
    /// # Errors
    /// A ledger write failure.
    pub fn complete(&self, id: &str, outcome: &LaneOutcome) -> io::Result<()> {
        let Some(mut job) = self.ledger.get(id) else {
            return Ok(());
        };
        if job.state != TransferState::Running {
            return Ok(());
        }
        match outcome {
            LaneOutcome::Done => {
                job.progress = Some(100);
                job.set_state(TransferState::Done);
            }
            LaneOutcome::Failed { error } => job.fail(error.clone()),
        }
        self.ledger.upsert(&job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::transfers::job::{Method, TransferPolicy};

    fn q(cap: usize) -> (tempfile::TempDir, TransferQueue) {
        let tmp = tempfile::tempdir().unwrap();
        let queue = TransferQueue::open(tmp.path(), cap).unwrap();
        (tmp, queue)
    }

    fn job(source: &str) -> TransferJob {
        TransferJob::new(source, "/dest", Method::Rsync, TransferPolicy::default())
    }

    #[test]
    fn submit_then_list_round_trips() {
        let (_t, queue) = q(2);
        let id = queue.submit(job("/a")).unwrap();
        let listed = queue.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].state, TransferState::Queued);
        assert_eq!(queue.get(&id).unwrap().source, "/a");
    }

    #[test]
    fn cap_enforced_at_claim() {
        let (_t, queue) = q(2);
        let a = queue.submit(job("/a")).unwrap();
        let b = queue.submit(job("/b")).unwrap();
        let _c = queue.submit(job("/c")).unwrap();
        // Two slots → two claims; the third is refused until one frees.
        let c1 = queue.claim_next().expect("slot 1");
        let c2 = queue.claim_next().expect("slot 2");
        assert!(queue.claim_next().is_none(), "cap reached — no third claim");
        assert_eq!(queue.running_count(), 2);
        // FIFO: the two oldest were claimed.
        assert_eq!([c1.id.as_str(), c2.id.as_str()], [a.as_str(), b.as_str()]);
        // Complete one → a slot frees → the third claims.
        queue.complete(&c1.id, &LaneOutcome::Done).unwrap();
        assert_eq!(queue.running_count(), 1);
        assert!(
            queue.claim_next().is_some(),
            "a freed slot admits the third"
        );
        assert_eq!(queue.running_count(), 2);
    }

    #[test]
    fn state_machine_pause_resume_cancel() {
        let (_t, queue) = q(2);
        let id = queue.submit(job("/a")).unwrap();
        // Pause a Queued job, then it must not be claimable.
        queue.pause(&id).unwrap();
        assert_eq!(queue.get(&id).unwrap().state, TransferState::Paused);
        assert!(queue.claim_next().is_none(), "a Paused job is not runnable");
        // Pausing again is an illegal transition (honest refusal, not a silent no-op).
        assert!(matches!(
            queue.pause(&id),
            Err(QueueError::IllegalTransition { verb: "pause", .. })
        ));
        // Resume → Queued → claimable.
        queue.resume(&id).unwrap();
        assert_eq!(queue.get(&id).unwrap().state, TransferState::Queued);
        let running = queue.claim_next().expect("re-armed job runs");
        assert_eq!(running.state, TransferState::Running);
        // Resume of a non-Paused job is illegal.
        assert!(matches!(
            queue.resume(&id),
            Err(QueueError::IllegalTransition { verb: "resume", .. })
        ));
        // Pause a Running job holds it and frees the slot.
        queue.pause(&id).unwrap();
        assert_eq!(queue.running_count(), 0);
        // Cancel removes it entirely (no Cancelled state).
        queue.cancel(&id).unwrap();
        assert!(queue.get(&id).is_none());
        assert!(queue.list().is_empty());
    }

    #[test]
    fn verbs_on_a_missing_id_are_typed_not_found() {
        let (_t, queue) = q(2);
        assert!(matches!(queue.pause("nope"), Err(QueueError::NotFound(_))));
        assert!(matches!(queue.resume("nope"), Err(QueueError::NotFound(_))));
        assert!(matches!(queue.cancel("nope"), Err(QueueError::NotFound(_))));
    }

    #[test]
    fn complete_maps_outcomes_and_ignores_non_running() {
        let (_t, queue) = q(2);
        let id = queue.submit(job("/a")).unwrap();
        let running = queue.claim_next().unwrap();
        assert_eq!(running.id, id);
        // A Failed outcome carries the honest reason (§7).
        queue
            .complete(&id, &LaneOutcome::failed("host unreachable"))
            .unwrap();
        let done = queue.get(&id).unwrap();
        assert_eq!(done.state, TransferState::Failed);
        assert_eq!(done.error.as_deref(), Some("host unreachable"));
        // A late outcome against an already-terminal job is dropped, not forced.
        queue.complete(&id, &LaneOutcome::Done).unwrap();
        assert_eq!(queue.get(&id).unwrap().state, TransferState::Failed);
    }

    #[test]
    fn ledger_persists_across_a_simulated_restart_and_recovers_running() {
        let tmp = tempfile::tempdir().unwrap();
        let (id_running, id_queued) = {
            let queue = TransferQueue::open(tmp.path(), 2).unwrap();
            let r = queue.submit(job("/running")).unwrap();
            let qd = queue.submit(job("/queued")).unwrap();
            // Drive one into Running, then "crash" (drop the queue).
            let claimed = queue.claim_next().unwrap();
            assert_eq!(claimed.id, r);
            assert_eq!(queue.get(&r).unwrap().state, TransferState::Running);
            (r, qd)
        };
        // Restart over the same store: both records survive, and the orphaned
        // Running job is recovered to Queued (it was not actually running).
        let restarted = TransferQueue::open(tmp.path(), 2).unwrap();
        let all = restarted.list();
        assert_eq!(all.len(), 2, "both jobs survive the restart");
        assert_eq!(
            restarted.get(&id_running).unwrap().state,
            TransferState::Queued,
            "an orphaned Running job recovers to Queued"
        );
        assert_eq!(
            restarted.get(&id_queued).unwrap().state,
            TransferState::Queued
        );
        assert_eq!(restarted.running_count(), 0);
    }
}

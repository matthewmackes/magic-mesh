//! The surface's file-operation queue — the FILEMGR-2 [`OpQueue`] wired in
//! (FILEMGR-8).
//!
//! Every long file operation the shell fires (a drag-and-drop copy or move
//! between panes, a delete) is submitted here, runs on the queue's background
//! worker thread against a single owned [`FileOps`], and reports **real** live
//! progress back over a channel — never a fake spinner. The surface owns one
//! [`Ops`]; it [`submit`](Ops::submit)s an [`OpKind`] and [`pump`](Ops::pump)s
//! the event stream each frame to refresh the on-screen progress strip.
//!
//! Reuse, not re-implementation (governance §6): the copy/move/delete engine,
//! the conflict resolver, the pause/cancel control and the progress/ETA maths
//! are all the shipped `mde-files::opqueue` types. This module only tracks the
//! *live UI state* of the ops in flight so the view can draw them.
//!
//! ## The interactive conflict dialog (FILEMGR-11)
//!
//! Every op runs with the FILEMGR-2 [`ChannelResolver`]: when the engine hits a
//! name collision the **worker** thread blocks on a channel while the surface
//! presents the Overwrite/Skip/Keep-both + apply-to-all dialog and replies. The
//! surface polls each in-flight op's prompt channel in [`pump`](Ops::pump), holds
//! the outstanding [`ConflictPrompt`] as [`ActiveOp::pending`], and answers it via
//! [`answer_conflict`](Ops::answer_conflict). This replaces the interim
//! non-interactive keep-both resolver FILEMGR-8 shipped as an honest deferral —
//! the collision is now the user's call, not an auto-rename.

use mde_files::backend::OpId;
use mde_files::fileops::FileOps;
use mde_files::opqueue::{
    channel_resolver, Conflict, ConflictChoice, ConflictPrompt, OpControl, OpEvent, OpKind,
    OpOutcome, OpQueue, Progress, QueuedOp,
};
use std::sync::mpsc::Receiver;

/// The live UI state of one queued operation, folded from the queue's
/// [`OpEvent`] stream. The surface renders these as the bottom progress strip.
pub struct ActiveOp {
    /// The op's stable id (matches the [`Progress`]/[`OpOutcome`] `op_id`).
    pub op_id: OpId,
    /// A human label for the strip (e.g. `"Moving 3 items → Downloads"`).
    pub label: String,
    /// The pause/cancel handle — a clone of the one the worker polls, so the
    /// strip's buttons drive the running op.
    pub control: OpControl,
    /// The most recent progress snapshot, `None` until the first tick.
    pub progress: Option<Progress>,
    /// The final outcome once the op finishes (or is cancelled); `None` while
    /// still running.
    pub outcome: Option<OpOutcome>,
    /// FILEMGR-11 — the conflict-prompt channel the worker raises collisions on.
    /// The engine blocks on a reply per prompt, so at most one is outstanding at
    /// a time (drained into [`pending`](Self::pending)).
    prompts: Receiver<ConflictPrompt>,
    /// The collision currently awaiting the user's answer, if any. Owning the
    /// [`ConflictPrompt`] keeps the worker parked until [`ConflictPrompt::answer`]
    /// (or a drop, which the resolver treats as a fail-safe Skip).
    pending: Option<ConflictPrompt>,
}

impl ActiveOp {
    /// `true` once the op has finished (or been cancelled) — the strip shows a
    /// terminal state with a dismiss button instead of pause/cancel.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.outcome.is_some()
    }

    /// The collision this op is currently blocked on, if the user hasn't answered
    /// it yet — the [`Conflict`] the FILEMGR-11 dialog renders.
    #[must_use]
    pub fn pending_conflict(&self) -> Option<&Conflict> {
        self.pending.as_ref().map(|p| &p.conflict)
    }
}

/// The surface's operation queue: one background worker over a single owned
/// [`FileOps`], plus the live [`ActiveOp`] list the view draws.
///
/// Field order matters for a clean shutdown: `active` is declared **before**
/// `queue` so it drops first — releasing any outstanding [`ConflictPrompt`] (its
/// reply channel) so a worker parked on an unanswered collision fail-safes to
/// Skip and finishes, *before* [`OpQueue`]'s own `Drop` joins that worker. The
/// explicit [`Drop`] below makes that guarantee independent of field order too.
pub struct Ops {
    active: Vec<ActiveOp>,
    queue: OpQueue,
    events: std::sync::mpsc::Receiver<OpEvent>,
    next_id: OpId,
}

impl Drop for Ops {
    fn drop(&mut self) {
        // Unblock any worker parked on an unanswered conflict prompt before the
        // OpQueue's Drop joins it: dropping each pending prompt closes its reply
        // channel, which the ChannelResolver reads as a fail-safe Skip, so the op
        // runs to completion and the worker exits its recv loop. Without this a
        // shutdown mid-collision would deadlock on the join.
        self.active.clear();
    }
}

impl Ops {
    /// Spawn the queue over `fileops`. Production passes
    /// [`mde_files::fileops::LiveFileOps`]; tests pass a
    /// [`mde_files::fileops::FakeFileOps`] so the whole submit → run → report
    /// path is exercised with zero disk I/O.
    #[must_use]
    pub fn spawn<F: FileOps + Send + 'static>(fileops: F) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let queue = OpQueue::spawn(fileops, tx);
        Self {
            active: Vec::new(),
            queue,
            events: rx,
            next_id: 1,
        }
    }

    /// Submit `kind` to the worker, tracking it as an [`ActiveOp`] with `label`.
    /// Returns the assigned op id. A collision blocks the worker on the returned
    /// op's prompt channel until the surface answers the FILEMGR-11 conflict
    /// dialog. If the worker has shut down the op is recorded as an immediate
    /// honest failure (never silently dropped).
    pub fn submit(&mut self, kind: OpKind, label: impl Into<String>) -> OpId {
        let op_id = self.next_id;
        self.next_id += 1;
        let control = OpControl::new();
        // FILEMGR-11 — the interactive resolver: the worker raises each collision
        // on `prompts` and blocks for a reply; the surface owns the receiver.
        let (resolver, prompts) = channel_resolver();
        let queued = QueuedOp {
            op_id,
            kind,
            control: control.clone(),
            resolver: Box::new(resolver),
        };
        let outcome = if self.queue.submit(queued).is_err() {
            Some(OpOutcome {
                op_id,
                cancelled: false,
                items_completed: 0,
                items_skipped: 0,
                files_done: 0,
                bytes_done: 0,
                error: Some("operation queue is not running".to_string()),
            })
        } else {
            None
        };
        self.active.push(ActiveOp {
            op_id,
            label: label.into(),
            control,
            progress: None,
            outcome,
            prompts,
            pending: None,
        });
        op_id
    }

    /// Drain every pending queue event into the [`ActiveOp`] list without
    /// blocking, and return the ids of ops that **finished** during this pump so
    /// the surface can reload the affected panes. Called once per frame.
    ///
    /// Also drains each in-flight op's conflict-prompt channel (FILEMGR-11): a
    /// raised collision becomes the op's [`pending`](ActiveOp::pending) prompt for
    /// the dialog to render. Non-blocking — a `try_recv`, never a wait.
    pub fn pump(&mut self) -> Vec<OpId> {
        let mut finished = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                OpEvent::Started(_) => {}
                OpEvent::Progress(progress) => {
                    if let Some(op) = self.find_mut(progress.op_id) {
                        op.progress = Some(progress);
                    }
                }
                OpEvent::Finished(outcome) => {
                    let id = outcome.op_id;
                    if let Some(op) = self.find_mut(id) {
                        op.outcome = Some(outcome);
                    }
                    finished.push(id);
                }
            }
        }
        // FILEMGR-11 — surface any newly-raised collision. The worker blocks per
        // prompt, so an op with one already outstanding won't produce another.
        for op in &mut self.active {
            if op.pending.is_none() {
                if let Ok(prompt) = op.prompts.try_recv() {
                    op.pending = Some(prompt);
                }
            }
        }
        finished
    }

    /// The first in-flight op that is blocked on an unanswered collision — the
    /// op id and the [`Conflict`] the FILEMGR-11 dialog renders. `None` when no
    /// op is waiting on a decision.
    #[must_use]
    pub fn pending_conflict(&self) -> Option<(OpId, &Conflict)> {
        self.active
            .iter()
            .find_map(|op| op.pending_conflict().map(|c| (op.op_id, c)))
    }

    /// `true` when any op is blocked on a collision awaiting the user (drives the
    /// dialog's repaint heartbeat).
    #[must_use]
    pub fn any_pending_conflict(&self) -> bool {
        self.active.iter().any(|op| op.pending.is_some())
    }

    /// Answer the collision `op_id` is blocked on, unparking its worker. The
    /// [`ConflictChoice`] carries the Overwrite/Skip/Keep-both resolution and the
    /// apply-to-all flag; the FILEMGR-2 engine makes the choice sticky when
    /// apply-to-all is set, so it won't ask again this op. A no-op if that op has
    /// no outstanding prompt.
    pub fn answer_conflict(&mut self, op_id: OpId, choice: ConflictChoice) {
        if let Some(op) = self.find_mut(op_id) {
            if let Some(prompt) = op.pending.take() {
                prompt.answer(choice);
            }
        }
    }

    /// Cancel op `op_id`: signal its [`OpControl`] and drop any outstanding
    /// prompt. Dropping the prompt closes its reply channel, which the resolver
    /// reads as a fail-safe Skip — so a worker parked on a collision unblocks and
    /// then observes the cancel at its next checkpoint (no frozen op).
    pub fn cancel(&mut self, op_id: OpId) {
        if let Some(op) = self.find_mut(op_id) {
            op.control.cancel();
            op.pending = None;
        }
    }

    /// The ops currently on the strip (running and freshly-finished).
    #[must_use]
    pub fn active(&self) -> &[ActiveOp] {
        &self.active
    }

    /// `true` when at least one op is still running (drives the "reload on
    /// completion" repaint request and the strip's visibility).
    #[must_use]
    pub fn any_running(&self) -> bool {
        self.active.iter().any(|op| !op.is_done())
    }

    /// Remove a finished op from the strip (the dismiss button). A running op is
    /// left alone — cancel it first.
    pub fn dismiss(&mut self, op_id: OpId) {
        self.active.retain(|op| op.op_id != op_id || !op.is_done());
    }

    fn find_mut(&mut self, op_id: OpId) -> Option<&mut ActiveOp> {
        self.active.iter_mut().find(|op| op.op_id == op_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_files::fileops::FakeFileOps;
    use mde_files::opqueue::Resolution;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// A fake FS with `/src/f.txt` and an empty `/dst` — the shape a
    /// pane-to-pane transfer runs over.
    fn scratch() -> FakeFileOps {
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/src")).expect("mkdir /src");
        fs.create_dir(Path::new("/dst")).expect("mkdir /dst");
        fs.seed_file("/src/f.txt", b"payload").expect("seed");
        fs
    }

    /// Pump until the given op reports a terminal outcome or the deadline hits.
    fn drain_until_done(ops: &mut Ops, op_id: OpId) {
        // logic-timing, not motion (test poll loop — bounded timeout + pump cadence)
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            ops.pump();
            if ops
                .active()
                .iter()
                .find(|o| o.op_id == op_id)
                .is_some_and(ActiveOp::is_done)
            {
                return;
            }
            assert!(Instant::now() < deadline, "op {op_id} never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn submit_runs_a_copy_on_the_worker_and_reports_completion() {
        let mut ops = Ops::spawn(scratch());
        let id = ops.submit(
            OpKind::Copy {
                items: vec![PathBuf::from("/src/f.txt")],
                dest_dir: PathBuf::from("/dst"),
            },
            "Copying 1 item → dst",
        );
        assert!(ops.any_running() || ops.active()[0].is_done());
        drain_until_done(&mut ops, id);
        let op = ops
            .active()
            .iter()
            .find(|o| o.op_id == id)
            .expect("tracked");
        let outcome = op.outcome.as_ref().expect("finished");
        assert!(!outcome.cancelled && outcome.error.is_none());
        assert_eq!(outcome.items_completed, 1);
        assert_eq!(outcome.bytes_done, 7);
    }

    #[test]
    fn a_collision_blocks_on_a_prompt_then_the_answer_drives_completion() {
        // FILEMGR-11 — the interactive conflict round-trip through the surface's
        // Ops: /dst already holds f.txt, so the copy collides and the worker
        // parks on the prompt channel until the surface answers.
        let fs = scratch();
        fs.seed_file("/dst/f.txt", b"old").expect("seed collision");
        let mut ops = Ops::spawn(fs);
        let id = ops.submit(
            OpKind::Copy {
                items: vec![PathBuf::from("/src/f.txt")],
                dest_dir: PathBuf::from("/dst"),
            },
            "Copying 1 item → dst",
        );

        // Pump until the collision surfaces as this op's pending prompt.
        // logic-timing, not motion (test poll loop — bounded timeout + pump cadence)
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            ops.pump();
            if ops.pending_conflict().is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "collision never surfaced");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (blocked_id, conflict) = ops.pending_conflict().expect("a prompt is pending");
        assert_eq!(blocked_id, id);
        assert!(
            conflict.dst.ends_with("f.txt"),
            "the dialog names the clash"
        );
        assert!(ops.any_pending_conflict());

        // Answer keep-both → the worker unparks and the op completes one item.
        ops.answer_conflict(id, ConflictChoice::once(Resolution::KeepBoth));
        assert!(!ops.any_pending_conflict(), "the prompt was consumed");
        drain_until_done(&mut ops, id);
        let outcome = ops
            .active()
            .iter()
            .find(|o| o.op_id == id)
            .and_then(|o| o.outcome.as_ref())
            .expect("finished");
        assert!(!outcome.cancelled && outcome.error.is_none());
        assert_eq!(
            outcome.items_completed, 1,
            "keep-both copied the incoming file"
        );
    }

    #[test]
    fn answering_skip_reports_the_item_skipped() {
        let fs = scratch();
        fs.seed_file("/dst/f.txt", b"old").expect("seed collision");
        let mut ops = Ops::spawn(fs);
        let id = ops.submit(
            OpKind::Copy {
                items: vec![PathBuf::from("/src/f.txt")],
                dest_dir: PathBuf::from("/dst"),
            },
            "Copying 1 item → dst",
        );
        // logic-timing, not motion (test poll loop — bounded timeout + pump cadence)
        let deadline = Instant::now() + Duration::from_secs(5);
        while ops.pending_conflict().is_none() {
            ops.pump();
            assert!(Instant::now() < deadline, "collision never surfaced");
            std::thread::sleep(Duration::from_millis(5));
        }
        ops.answer_conflict(id, ConflictChoice::all(Resolution::Skip));
        drain_until_done(&mut ops, id);
        let outcome = ops
            .active()
            .iter()
            .find(|o| o.op_id == id)
            .and_then(|o| o.outcome.as_ref())
            .expect("finished");
        assert_eq!(outcome.items_skipped, 1, "skip left the destination alone");
        assert_eq!(outcome.items_completed, 0);
    }

    #[test]
    fn dismiss_drops_only_finished_ops() {
        let mut ops = Ops::spawn(scratch());
        let id = ops.submit(
            OpKind::Delete {
                items: vec![PathBuf::from("/src/f.txt")],
            },
            "Deleting 1 item",
        );
        drain_until_done(&mut ops, id);
        assert_eq!(ops.active().len(), 1);
        ops.dismiss(id);
        assert!(ops.active().is_empty(), "a finished op dismisses");
    }
}

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
//! The interactive Overwrite/Skip/Keep-both conflict **dialog** is FILEMGR-11;
//! until it lands, ops run with a non-interactive [`FixedResolution::keep_both`]
//! resolver — the safe, data-preserving default (a collision auto-renames the
//! incoming item rather than clobbering or blocking on a dialog the surface
//! can't yet show). That is an honest interim policy, not a stub: the op really
//! runs and really resolves every collision.

use mde_files::backend::OpId;
use mde_files::fileops::FileOps;
use mde_files::opqueue::{
    FixedResolution, OpControl, OpEvent, OpKind, OpOutcome, OpQueue, Progress, QueuedOp,
};

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
}

impl ActiveOp {
    /// `true` once the op has finished (or been cancelled) — the strip shows a
    /// terminal state with a dismiss button instead of pause/cancel.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.outcome.is_some()
    }
}

/// The surface's operation queue: one background worker over a single owned
/// [`FileOps`], plus the live [`ActiveOp`] list the view draws.
pub struct Ops {
    queue: OpQueue,
    events: std::sync::mpsc::Receiver<OpEvent>,
    active: Vec<ActiveOp>,
    next_id: OpId,
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
            queue,
            events: rx,
            active: Vec::new(),
            next_id: 1,
        }
    }

    /// Submit `kind` to the worker, tracking it as an [`ActiveOp`] with `label`.
    /// Returns the assigned op id. A collision auto-renames (keep-both) until the
    /// FILEMGR-11 conflict dialog lands. If the worker has shut down the op is
    /// recorded as an immediate honest failure (never silently dropped).
    pub fn submit(&mut self, kind: OpKind, label: impl Into<String>) -> OpId {
        let op_id = self.next_id;
        self.next_id += 1;
        let control = OpControl::new();
        let queued = QueuedOp {
            op_id,
            kind,
            control: control.clone(),
            resolver: Box::new(FixedResolution::keep_both()),
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
        });
        op_id
    }

    /// Drain every pending queue event into the [`ActiveOp`] list without
    /// blocking, and return the ids of ops that **finished** during this pump so
    /// the surface can reload the affected panes. Called once per frame.
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
        finished
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

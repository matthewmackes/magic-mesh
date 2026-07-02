//! FILEMGR-2 — the async op queue + conflict engine.
//!
//! Long file operations (bulk copy/move/delete over big trees) must not block
//! the Files surface, must show live progress, must be pausable + cancellable,
//! and must resolve name-collisions per item. This module is the render-agnostic
//! engine that does all of that on a worker thread, driving the FILEMGR-1
//! [`FileOps`] trait for every actual mutation — it never touches `std::fs`
//! itself (§6 glue-not-reimplementation; the surface is FILEMGR-8).
//!
//! ## The pieces
//!
//! * [`execute`] — the synchronous core. Given a [`FileOps`], an [`OpKind`], an
//!   [`OpControl`], a [`ConflictResolver`], and a progress sink, it runs the op
//!   to completion, emitting a [`Progress`] snapshot at every step and honouring
//!   pause/cancel at cooperative checkpoints. It is deterministic and fully unit
//!   tested against [`crate::fileops::FakeFileOps`].
//! * [`OpQueue`] — a background worker thread that drains submitted [`QueuedOp`]s
//!   one at a time, publishing [`OpEvent`]s over a channel. This is the "async op
//!   on a worker + a background queue" the lock requires; the UI submits and
//!   renders events without ever blocking.
//! * The conflict engine — per-item **Overwrite / Skip / Keep-both** with
//!   **apply-to-all**, and **recursive directory merge** (a colliding child
//!   re-runs the same resolution logic). See [`Resolution`] / [`ConflictResolver`].
//!
//! ## Cancel leaves no half-files
//!
//! A cancel stops at the next checkpoint and then **rolls back the in-flight
//! top-level item** — every path the engine freshly created for that item is
//! removed (children first), so a half-copied directory tree never survives a
//! cancel. Already-completed items and pre-existing merge targets are untouched.
//! The returned [`OpOutcome`] reports exactly what finished, so nothing
//! half-done is ever claimed complete.

use crate::archive::ArchiveFormat;
use crate::backend::OpId;
use crate::fileops::{free_duplicate_name, FileOps};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════
// The operation model.
// ═══════════════════════════════════════════════════════════════════════════

/// A long file operation the queue can run. Each variant carries absolute source
/// paths (as the surface resolved them) and, for transfers, the destination
/// directory the items are placed *into* (the classic "paste here" shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpKind {
    /// Copy each `item` into `dest_dir` (recursively for directories).
    Copy {
        items: Vec<PathBuf>,
        dest_dir: PathBuf,
    },
    /// Move each `item` into `dest_dir`. Implemented as copy-then-remove-source
    /// so it shares the conflict/merge/cancel semantics; the source is removed
    /// only after its copy fully succeeds, so a cancel never loses data.
    Move {
        items: Vec<PathBuf>,
        dest_dir: PathBuf,
    },
    /// Permanently delete each `item` (recursively). No trash, no undo — the
    /// confirm dialog upstream is the safeguard (lock 3/6). Cancellable between
    /// entries; already-deleted entries stay deleted (honestly reported).
    Delete { items: Vec<PathBuf> },
    /// Compress `items` (named relative to `base_dir`) into a new `archive` in
    /// the chosen `format` (FILEMGR-3). Built + progress-reported per member on
    /// the same queue; a cancel leaves no half-archive. See [`crate::archive`].
    Compress {
        items: Vec<PathBuf>,
        base_dir: PathBuf,
        archive: PathBuf,
        format: ArchiveFormat,
    },
    /// Extract every member of `archive` into `dest_dir` — the extract-here /
    /// extract-to verb (FILEMGR-3). Path-traversal-guarded; progress per member.
    Extract { archive: PathBuf, dest_dir: PathBuf },
}

impl OpKind {
    fn items(&self) -> &[PathBuf] {
        match self {
            OpKind::Copy { items, .. }
            | OpKind::Move { items, .. }
            | OpKind::Delete { items }
            | OpKind::Compress { items, .. } => items,
            // The archive is a single file, not a source-item set to scan.
            OpKind::Extract { .. } => &[],
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The conflict engine.
// ═══════════════════════════════════════════════════════════════════════════

/// A concrete per-item conflict decision. Unlike the stored, Send-To–oriented
/// [`crate::backend::ConflictPolicy`] (which includes an `Ask` state), a
/// `Resolution` is always an actionable choice — the engine never carries an
/// unresolved conflict forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Replace the destination. For two directories this **merges** (children
    /// re-run the conflict logic); for a file it is replaced in place.
    Overwrite,
    /// Leave the destination as-is; skip this item (and, for a directory, its
    /// whole subtree).
    Skip,
    /// Keep both: copy the source to a fresh auto-renamed sibling
    /// (`name copy.ext`, then `name copy 2.ext`, …) so nothing is lost.
    KeepBoth,
}

/// A name-collision the engine hit: the source it is copying and the existing
/// destination that is in the way, with each side's kind so the surface can word
/// the prompt ("Replace folder?" vs "Replace file?").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub src_is_dir: bool,
    pub dst_is_dir: bool,
}

/// The answer to a [`Conflict`]: a [`Resolution`] plus whether it applies to
/// every remaining conflict in this operation (the "apply to all" checkbox).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictChoice {
    pub resolution: Resolution,
    pub apply_to_all: bool,
}

impl ConflictChoice {
    #[must_use]
    pub fn once(resolution: Resolution) -> Self {
        Self {
            resolution,
            apply_to_all: false,
        }
    }

    #[must_use]
    pub fn all(resolution: Resolution) -> Self {
        Self {
            resolution,
            apply_to_all: true,
        }
    }
}

/// How the engine gets a decision for a [`Conflict`]. The real surface answers
/// interactively ([`ChannelResolver`]); headless callers and tests use a fixed
/// policy ([`FixedResolution`]) or a closure ([`FnResolver`]).
///
/// `resolve` may block (the channel resolver blocks the *worker* thread waiting
/// for the user) — never the UI thread, which owns the event loop.
pub trait ConflictResolver {
    fn resolve(&mut self, conflict: &Conflict) -> ConflictChoice;
}

/// A resolver that answers every conflict the same way. `apply_to_all` short-
/// circuits the engine so it stops asking after the first collision.
#[derive(Debug, Clone, Copy)]
pub struct FixedResolution(pub ConflictChoice);

impl FixedResolution {
    #[must_use]
    pub fn overwrite() -> Self {
        Self(ConflictChoice::all(Resolution::Overwrite))
    }
    #[must_use]
    pub fn skip() -> Self {
        Self(ConflictChoice::all(Resolution::Skip))
    }
    #[must_use]
    pub fn keep_both() -> Self {
        Self(ConflictChoice::all(Resolution::KeepBoth))
    }
}

impl ConflictResolver for FixedResolution {
    fn resolve(&mut self, _conflict: &Conflict) -> ConflictChoice {
        self.0
    }
}

/// A resolver backed by a closure — the surface passes one that consults its
/// own remembered policy; tests script per-path answers.
pub struct FnResolver<F>(pub F);

impl<F: FnMut(&Conflict) -> ConflictChoice> ConflictResolver for FnResolver<F> {
    fn resolve(&mut self, conflict: &Conflict) -> ConflictChoice {
        (self.0)(conflict)
    }
}

/// One collision handed to the UI by a [`ChannelResolver`]. The worker blocks
/// until the surface calls [`ConflictPrompt::answer`].
pub struct ConflictPrompt {
    pub conflict: Conflict,
    reply: Sender<ConflictChoice>,
}

impl ConflictPrompt {
    /// Answer the prompt, unblocking the worker. If the worker has gone away the
    /// send is dropped (the op was cancelled) — harmless.
    pub fn answer(self, choice: ConflictChoice) {
        let _ = self.reply.send(choice);
    }
}

/// The worker side of an interactive resolver. Each conflict is sent to the UI
/// as a [`ConflictPrompt`]; the worker blocks on the reply. Build with
/// [`channel_resolver`].
pub struct ChannelResolver {
    prompts: Sender<ConflictPrompt>,
}

impl ConflictResolver for ChannelResolver {
    fn resolve(&mut self, conflict: &Conflict) -> ConflictChoice {
        let (reply_tx, reply_rx) = channel();
        let prompt = ConflictPrompt {
            conflict: conflict.clone(),
            reply: reply_tx,
        };
        // If the UI side is gone, fail safe: skip (never silently clobber).
        if self.prompts.send(prompt).is_err() {
            return ConflictChoice::all(Resolution::Skip);
        }
        reply_rx
            .recv()
            .unwrap_or_else(|_| ConflictChoice::all(Resolution::Skip))
    }
}

/// Build an interactive resolver + the receiver the surface polls for
/// [`ConflictPrompt`]s. The resolver goes into the [`QueuedOp`]; the receiver
/// stays on the UI thread.
#[must_use]
pub fn channel_resolver() -> (ChannelResolver, Receiver<ConflictPrompt>) {
    let (tx, rx) = channel();
    (ChannelResolver { prompts: tx }, rx)
}

// ═══════════════════════════════════════════════════════════════════════════
// Pause / cancel control.
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlState {
    Running,
    Paused,
    Cancelled,
}

#[derive(Debug)]
struct ControlInner {
    state: Mutex<ControlState>,
    resume: Condvar,
}

/// A pause/resume/cancel handle shared between the UI (which drives it) and the
/// worker (which polls it at cooperative checkpoints). Cloning shares the same
/// underlying state, so a clone kept by the surface controls the running op.
#[derive(Debug, Clone)]
pub struct OpControl {
    inner: Arc<ControlInner>,
}

impl Default for OpControl {
    fn default() -> Self {
        Self::new()
    }
}

impl OpControl {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ControlInner {
                state: Mutex::new(ControlState::Running),
                resume: Condvar::new(),
            }),
        }
    }

    /// Pause the op at its next checkpoint. A pause requested mid-item takes
    /// effect between files, so an in-flight `copy_file` always finishes whole.
    pub fn pause(&self) {
        let mut st = guard(&self.inner.state);
        if *st == ControlState::Running {
            *st = ControlState::Paused;
        }
    }

    /// Resume a paused op. No-op if it was cancelled.
    pub fn resume(&self) {
        let mut st = guard(&self.inner.state);
        if *st == ControlState::Paused {
            *st = ControlState::Running;
        }
        drop(st);
        self.inner.resume.notify_all();
    }

    /// Cancel the op. It stops at the next checkpoint and rolls back the
    /// in-flight item. Also wakes a paused worker so a cancel-while-paused
    /// takes effect immediately.
    pub fn cancel(&self) {
        {
            let mut st = guard(&self.inner.state);
            *st = ControlState::Cancelled;
        }
        self.inner.resume.notify_all();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *guard(&self.inner.state) == ControlState::Cancelled
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        *guard(&self.inner.state) == ControlState::Paused
    }

    /// The worker's cooperative gate: returns `true` to proceed, `false` when
    /// cancelled. Blocks while paused (and returns `false` if cancelled during
    /// the pause). `pub(crate)` so the archive engine ([`crate::archive`]) shares
    /// the exact same pause/cancel checkpoint between members.
    pub(crate) fn proceed(&self) -> bool {
        let mut st = guard(&self.inner.state);
        loop {
            match *st {
                ControlState::Cancelled => return false,
                ControlState::Running => return true,
                ControlState::Paused => {
                    st = self
                        .inner
                        .resume
                        .wait(st)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
    }
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ═══════════════════════════════════════════════════════════════════════════
// Progress + outcome.
// ═══════════════════════════════════════════════════════════════════════════

/// A live snapshot emitted at every step. `files_*` count filesystem entries
/// (files, directories, symlinks) so an all-directories tree still shows
/// motion; `bytes_*` count regular-file payload only, which is what drives a
/// meaningful throughput/ETA. `*_skipped` are entries the conflict resolver
/// chose to skip — they count toward "examined" but not toward copied bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub op_id: OpId,
    pub files_total: u64,
    pub files_done: u64,
    pub files_skipped: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub bytes_skipped: u64,
    /// The entry currently being acted on, when one is in flight.
    pub current: Option<PathBuf>,
    /// Wall-clock time since the op started.
    pub elapsed: Duration,
}

impl Progress {
    /// Completion in `0.0..=1.0`, counting done + skipped against the total (so
    /// a run that skips collisions still reaches 1.0). An empty op is complete.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.files_total == 0 {
            return 1.0;
        }
        let handled = self.files_done + self.files_skipped;
        (handled as f32 / self.files_total as f32).clamp(0.0, 1.0)
    }

    /// Estimated time remaining, from the achieved rate. Prefers the byte rate
    /// (accurate for real transfers); falls back to the entry rate when no bytes
    /// have moved (deletes, empty files). `None` until there is enough signal.
    #[must_use]
    pub fn eta(&self) -> Option<Duration> {
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            return None;
        }
        if self.bytes_done > 0 {
            let remaining = self
                .bytes_total
                .saturating_sub(self.bytes_done)
                .saturating_sub(self.bytes_skipped);
            let rate = self.bytes_done as f64 / secs;
            if rate <= 0.0 {
                return None;
            }
            return Some(Duration::from_secs_f64(remaining as f64 / rate));
        }
        if self.files_done > 0 {
            let remaining = self
                .files_total
                .saturating_sub(self.files_done)
                .saturating_sub(self.files_skipped);
            let rate = self.files_done as f64 / secs;
            if rate <= 0.0 {
                return None;
            }
            return Some(Duration::from_secs_f64(remaining as f64 / rate));
        }
        None
    }
}

/// The honest result of a finished (or cancelled) op. `cancelled` + the counts
/// let the surface report true completion — a cancelled op's rolled-back
/// in-flight item is not counted in `items_completed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpOutcome {
    pub op_id: OpId,
    pub cancelled: bool,
    /// Top-level items fully completed.
    pub items_completed: u64,
    /// Top-level items skipped whole by the resolver.
    pub items_skipped: u64,
    /// Filesystem entries actually written/removed.
    pub files_done: u64,
    /// Regular-file bytes actually copied.
    pub bytes_done: u64,
    /// The first hard error that aborted the op, if any (typed IO error text).
    pub error: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// The synchronous engine.
// ═══════════════════════════════════════════════════════════════════════════

/// Internal stop signal that unwinds the recursion to the top-level item loop.
enum Stop {
    Cancelled,
    Failed(io::Error),
}

/// Whether a single item was copied or skipped by the resolver.
enum ItemResult {
    Copied,
    Skipped,
}

/// Run one operation to completion, synchronously, on the current thread. Every
/// mutation goes through `ops`; a [`Progress`] is handed to `sink` at each step;
/// `control` is polled at cooperative checkpoints; `resolver` decides
/// collisions. Returns the honest [`OpOutcome`].
pub fn execute(
    ops: &dyn FileOps,
    op: &OpKind,
    op_id: OpId,
    control: &OpControl,
    resolver: &mut dyn ConflictResolver,
    sink: &mut dyn FnMut(Progress),
) -> OpOutcome {
    // Archive ops (FILEMGR-3) run their own member-walk + progress engine; they
    // ride the same queue/OpEvent path but don't use the copy/move conflict
    // machinery (no `resolver`, no scan_items over source items).
    match op {
        OpKind::Compress {
            items,
            base_dir,
            archive,
            format,
        } => {
            return crate::archive::compress(
                ops, items, base_dir, archive, *format, control, op_id, sink,
            );
        }
        OpKind::Extract { archive, dest_dir } => {
            return crate::archive::extract(ops, archive, dest_dir, control, op_id, sink);
        }
        OpKind::Copy { .. } | OpKind::Move { .. } | OpKind::Delete { .. } => {}
    }

    let (files_total, bytes_total) = scan_items(ops, op.items());
    let mut ex = Exec {
        ops,
        control,
        resolver,
        sink,
        op_id,
        start: Instant::now(),
        files_total,
        bytes_total,
        files_done: 0,
        bytes_done: 0,
        files_skipped: 0,
        bytes_skipped: 0,
        sticky: None,
        created: Vec::new(),
    };
    ex.emit(None);
    let mut outcome = OpOutcome {
        op_id,
        cancelled: false,
        items_completed: 0,
        items_skipped: 0,
        files_done: 0,
        bytes_done: 0,
        error: None,
    };
    match op {
        OpKind::Copy { items, dest_dir } => ex.run_transfer(items, dest_dir, false, &mut outcome),
        OpKind::Move { items, dest_dir } => ex.run_transfer(items, dest_dir, true, &mut outcome),
        OpKind::Delete { items } => ex.run_delete(items, &mut outcome),
        // Archive ops early-returned above before the Exec was built.
        OpKind::Compress { .. } | OpKind::Extract { .. } => unreachable!("archive ops dispatched"),
    }
    outcome.files_done = ex.files_done;
    outcome.bytes_done = ex.bytes_done;
    ex.emit(None);
    outcome
}

/// Mutable execution state threaded through the recursion.
struct Exec<'a> {
    ops: &'a dyn FileOps,
    control: &'a OpControl,
    resolver: &'a mut dyn ConflictResolver,
    sink: &'a mut dyn FnMut(Progress),
    op_id: OpId,
    start: Instant,
    files_total: u64,
    bytes_total: u64,
    files_done: u64,
    bytes_done: u64,
    files_skipped: u64,
    bytes_skipped: u64,
    /// Sticky resolution once the user picked "apply to all".
    sticky: Option<Resolution>,
    /// Paths freshly created for the current in-flight top-level item, for
    /// cancel/error rollback. Cleared at the start of each item.
    created: Vec<PathBuf>,
}

impl Exec<'_> {
    fn emit(&mut self, current: Option<PathBuf>) {
        let progress = Progress {
            op_id: self.op_id,
            files_total: self.files_total,
            files_done: self.files_done,
            files_skipped: self.files_skipped,
            bytes_total: self.bytes_total,
            bytes_done: self.bytes_done,
            bytes_skipped: self.bytes_skipped,
            current,
            elapsed: self.start.elapsed(),
        };
        (self.sink)(progress);
    }

    /// Count one written/ensured entry as done and publish progress.
    fn advance(&mut self, path: &Path, bytes: u64) {
        self.files_done += 1;
        self.bytes_done += bytes;
        self.emit(Some(path.to_path_buf()));
    }

    /// Count a skipped subtree (its scanned entries + bytes) and publish.
    fn advance_skipped(&mut self, src: &Path) {
        let (n, b) = scan_one(self.ops, src);
        self.files_skipped += n;
        self.bytes_skipped += b;
        self.emit(Some(src.to_path_buf()));
    }

    fn resolution_for(&mut self, conflict: &Conflict) -> Resolution {
        if let Some(sticky) = self.sticky {
            return sticky;
        }
        let choice = self.resolver.resolve(conflict);
        if choice.apply_to_all {
            self.sticky = Some(choice.resolution);
        }
        choice.resolution
    }

    /// Undo the current in-flight item: remove every path we created for it,
    /// children first (best-effort — a NotFound during teardown is fine).
    fn rollback(&mut self) {
        for path in self.created.iter().rev() {
            let _ = self.ops.remove(path);
        }
        self.created.clear();
    }

    // ── copy / move ─────────────────────────────────────────────────────────

    fn run_transfer(
        &mut self,
        items: &[PathBuf],
        dest_dir: &Path,
        is_move: bool,
        outcome: &mut OpOutcome,
    ) {
        for item in items {
            self.created.clear();
            if !self.control.proceed() {
                outcome.cancelled = true;
                return;
            }
            let Some(name) = item.file_name() else {
                outcome.error = Some(format!("source has no file name: {}", item.display()));
                return;
            };
            let dst = dest_dir.join(name);
            match self.copy_into(item, &dst) {
                Ok(ItemResult::Copied) => {
                    if is_move {
                        if let Err(err) = self.ops.remove(item) {
                            outcome.error = Some(format!(
                                "move: copied but could not remove source {}: {err}",
                                item.display()
                            ));
                            return;
                        }
                    }
                    outcome.items_completed += 1;
                }
                Ok(ItemResult::Skipped) => outcome.items_skipped += 1,
                Err(Stop::Cancelled) => {
                    self.rollback();
                    outcome.cancelled = true;
                    return;
                }
                Err(Stop::Failed(err)) => {
                    self.rollback();
                    outcome.error = Some(format!("{}: {err}", item.display()));
                    return;
                }
            }
        }
    }

    /// Copy `src` to `dst`, resolving a collision at `dst` if one exists.
    fn copy_into(&mut self, src: &Path, dst: &Path) -> Result<ItemResult, Stop> {
        if !self.control.proceed() {
            return Err(Stop::Cancelled);
        }
        // Copying an entry onto itself is a no-op, and an "overwrite" of self
        // would destroy the source — guard it out honestly.
        if src == dst {
            return Ok(ItemResult::Skipped);
        }
        if self.ops.symlink_metadata(dst).is_err() {
            // No collision — straight fresh copy.
            self.copy_fresh(src, dst)?;
            return Ok(ItemResult::Copied);
        }
        let src_meta = self.ops.symlink_metadata(src).map_err(Stop::Failed)?;
        let dst_meta = self.ops.symlink_metadata(dst).map_err(Stop::Failed)?;
        let conflict = Conflict {
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
            src_is_dir: src_meta.is_dir,
            dst_is_dir: dst_meta.is_dir,
        };
        match self.resolution_for(&conflict) {
            Resolution::Skip => {
                self.advance_skipped(src);
                Ok(ItemResult::Skipped)
            }
            Resolution::KeepBoth => {
                let fresh = free_duplicate_name(dst, |c| self.ops.symlink_metadata(c).is_ok());
                self.copy_fresh(src, &fresh)?;
                Ok(ItemResult::Copied)
            }
            Resolution::Overwrite => {
                if src_meta.is_dir && dst_meta.is_dir {
                    // Merge: the destination directory stays; its pre-existing
                    // children are left alone, and each source child re-runs the
                    // conflict logic. The dir itself is already present, so count
                    // it as handled without re-creating it.
                    self.advance(dst, 0);
                    let mut children = self.ops.read_dir(src).map_err(Stop::Failed)?;
                    children.sort();
                    for child in children {
                        let cname = child_name(&child)?;
                        self.copy_into(&child, &dst.join(cname))?;
                    }
                    Ok(ItemResult::Copied)
                } else {
                    // Types differ or both are files: replace the destination.
                    self.ops.remove(dst).map_err(Stop::Failed)?;
                    self.copy_fresh(src, dst)?;
                    Ok(ItemResult::Copied)
                }
            }
        }
    }

    /// Copy `src` to a `dst` known not to exist, recursing into directories.
    /// Every entry it makes is tracked in `created` for rollback.
    fn copy_fresh(&mut self, src: &Path, dst: &Path) -> Result<(), Stop> {
        if !self.control.proceed() {
            return Err(Stop::Cancelled);
        }
        let meta = self.ops.symlink_metadata(src).map_err(Stop::Failed)?;
        if meta.is_symlink {
            let target = self.ops.read_link(src).map_err(Stop::Failed)?;
            self.ops.symlink(&target, dst).map_err(Stop::Failed)?;
            self.created.push(dst.to_path_buf());
            self.advance(dst, 0);
        } else if meta.is_dir {
            self.ops.create_dir(dst).map_err(Stop::Failed)?;
            self.created.push(dst.to_path_buf());
            self.advance(dst, 0);
            let mut children = self.ops.read_dir(src).map_err(Stop::Failed)?;
            children.sort();
            for child in children {
                let cname = child_name(&child)?;
                // A fresh destination has no children, so these never collide;
                // going through copy_into keeps one checkpoint/progress path.
                self.copy_into(&child, &dst.join(cname))?;
            }
        } else {
            let bytes = self.ops.copy_file(src, dst).map_err(Stop::Failed)?;
            self.created.push(dst.to_path_buf());
            self.advance(dst, bytes);
        }
        Ok(())
    }

    // ── delete ────────────────────────────────────────────────────────────────

    fn run_delete(&mut self, items: &[PathBuf], outcome: &mut OpOutcome) {
        let mut items = items.to_vec();
        items.sort();
        for item in items {
            if !self.control.proceed() {
                outcome.cancelled = true;
                return;
            }
            match self.delete_tree(&item) {
                Ok(()) => outcome.items_completed += 1,
                Err(Stop::Cancelled) => {
                    outcome.cancelled = true;
                    return;
                }
                Err(Stop::Failed(err)) => {
                    outcome.error = Some(format!("{}: {err}", item.display()));
                    return;
                }
            }
        }
    }

    /// Remove a path bottom-up so a cancel stops promptly and the reported count
    /// matches what was actually unlinked. Deletes are final (no rollback).
    fn delete_tree(&mut self, path: &Path) -> Result<(), Stop> {
        if !self.control.proceed() {
            return Err(Stop::Cancelled);
        }
        let meta = self.ops.symlink_metadata(path).map_err(Stop::Failed)?;
        if meta.is_dir {
            let mut children = self.ops.read_dir(path).map_err(Stop::Failed)?;
            children.sort();
            for child in children {
                self.delete_tree(&child)?;
            }
            self.ops.remove_dir_all(path).map_err(Stop::Failed)?;
            self.advance(path, 0);
        } else {
            self.ops.remove_file(path).map_err(Stop::Failed)?;
            self.advance(path, meta.len);
        }
        Ok(())
    }
}

fn child_name(child: &Path) -> Result<PathBuf, Stop> {
    child.file_name().map(PathBuf::from).ok_or_else(|| {
        Stop::Failed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "child has no name",
        ))
    })
}

/// Total (entries, bytes) across `items`, matching copy semantics: a symlink is
/// one entry / zero bytes, a directory one entry plus its subtree, a regular
/// file one entry plus its length. Missing sources contribute nothing (the
/// error surfaces when execution reaches them).
fn scan_items(ops: &dyn FileOps, items: &[PathBuf]) -> (u64, u64) {
    let mut files = 0;
    let mut bytes = 0;
    for item in items {
        let (n, b) = scan_one(ops, item);
        files += n;
        bytes += b;
    }
    (files, bytes)
}

fn scan_one(ops: &dyn FileOps, path: &Path) -> (u64, u64) {
    let Ok(meta) = ops.symlink_metadata(path) else {
        return (0, 0);
    };
    if meta.is_symlink {
        (1, 0)
    } else if meta.is_dir {
        let mut files = 1;
        let mut bytes = 0;
        if let Ok(children) = ops.read_dir(path) {
            for child in children {
                let (n, b) = scan_one(ops, &child);
                files += n;
                bytes += b;
            }
        }
        (files, bytes)
    } else {
        (1, meta.len)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The background queue.
// ═══════════════════════════════════════════════════════════════════════════

/// An op submitted to the [`OpQueue`], with the handles the surface keeps: its
/// stable id, its [`OpControl`] (for pause/cancel), and its conflict resolver.
pub struct QueuedOp {
    pub op_id: OpId,
    pub kind: OpKind,
    pub control: OpControl,
    pub resolver: Box<dyn ConflictResolver + Send>,
}

/// What the queue publishes about a running op. The surface renders these on its
/// event loop without ever blocking on the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpEvent {
    Started(OpId),
    Progress(Progress),
    Finished(OpOutcome),
}

/// A background worker thread that drains submitted [`QueuedOp`]s one at a time,
/// running each through [`execute`] against a single owned [`FileOps`] and
/// publishing [`OpEvent`]s. Dropping the queue closes it and joins the worker.
pub struct OpQueue {
    submit: Option<Sender<QueuedOp>>,
    worker: Option<JoinHandle<()>>,
}

impl OpQueue {
    /// Spawn the worker over `ops`, publishing every [`OpEvent`] on `events`.
    /// `ops` is moved onto the worker thread (so a `!Sync` fake works too).
    #[must_use]
    pub fn spawn<F>(ops: F, events: Sender<OpEvent>) -> Self
    where
        F: FileOps + Send + 'static,
    {
        let (submit, rx) = channel::<QueuedOp>();
        let worker = std::thread::Builder::new()
            .name("mde-files-opqueue".to_owned())
            .spawn(move || run_worker(&ops, &rx, &events))
            .expect("spawn mde-files op-queue worker");
        Self {
            submit: Some(submit),
            worker: Some(worker),
        }
    }

    /// Enqueue an op. It runs after any already-queued ops finish. Returns the
    /// op back as `Err` only if the worker has shut down.
    pub fn submit(&self, op: QueuedOp) -> Result<(), QueuedOp> {
        match &self.submit {
            Some(tx) => tx.send(op).map_err(|e| e.0),
            None => Err(op),
        }
    }
}

impl Drop for OpQueue {
    fn drop(&mut self) {
        // Close the submit channel so the worker's recv loop ends, then join.
        self.submit.take();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn run_worker(ops: &dyn FileOps, rx: &Receiver<QueuedOp>, events: &Sender<OpEvent>) {
    while let Ok(queued) = rx.recv() {
        let _ = events.send(OpEvent::Started(queued.op_id));
        let mut resolver = queued.resolver;
        let outcome = {
            let mut sink = |progress: Progress| {
                let _ = events.send(OpEvent::Progress(progress));
            };
            execute(
                ops,
                &queued.kind,
                queued.op_id,
                &queued.control,
                resolver.as_mut(),
                &mut sink,
            )
        };
        let _ = events.send(OpEvent::Finished(outcome));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileops::FakeFileOps;

    // ── fixtures ───────────────────────────────────────────────────────────

    /// A fake FS with `/dst` ready and a `/src` tree of the given files.
    fn scratch(files: &[(&str, &[u8])]) -> FakeFileOps {
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/src")).expect("mkdir /src");
        fs.create_dir(Path::new("/dst")).expect("mkdir /dst");
        for (path, body) in files {
            // Create any intermediate dirs under /src.
            let p = Path::new(path);
            if let Some(parent) = p.parent() {
                let _ = fs.create_dir_all(parent);
            }
            fs.seed_file(p, body).expect("seed");
        }
        fs
    }

    fn run(
        fs: &FakeFileOps,
        op: &OpKind,
        control: &OpControl,
        resolver: &mut dyn ConflictResolver,
    ) -> (OpOutcome, Vec<Progress>) {
        let mut seen = Vec::new();
        let outcome;
        {
            let mut sink = |p: Progress| seen.push(p);
            outcome = execute(fs, op, 1, control, resolver, &mut sink);
        }
        (outcome, seen)
    }

    // ── progress / ETA math ──────────────────────────────────────────────────

    #[test]
    fn eta_prefers_byte_rate_and_fraction_counts_skips() {
        let p = Progress {
            op_id: 1,
            files_total: 10,
            files_done: 4,
            files_skipped: 1,
            bytes_total: 1000,
            bytes_done: 400,
            bytes_skipped: 100,
            current: None,
            elapsed: Duration::from_secs(4),
        };
        // 400 bytes in 4s = 100 B/s; remaining = 1000-400-100 = 500 → 5s.
        assert_eq!(p.eta(), Some(Duration::from_secs(5)));
        // handled = done+skipped = 5 of 10.
        assert!((p.fraction() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn eta_falls_back_to_entry_rate_without_bytes() {
        let p = Progress {
            op_id: 1,
            files_total: 8,
            files_done: 2,
            files_skipped: 0,
            bytes_total: 0,
            bytes_done: 0,
            bytes_skipped: 0,
            current: None,
            elapsed: Duration::from_secs(2),
        };
        // 2 entries in 2s = 1/s; remaining 6 → 6s.
        assert_eq!(p.eta(), Some(Duration::from_secs(6)));
        assert_eq!(Progress::fraction(&p), 0.25);
    }

    #[test]
    fn eta_is_none_before_any_signal() {
        let p = Progress {
            op_id: 1,
            files_total: 5,
            files_done: 0,
            files_skipped: 0,
            bytes_total: 500,
            bytes_done: 0,
            bytes_skipped: 0,
            current: None,
            elapsed: Duration::ZERO,
        };
        assert_eq!(p.eta(), None);
        assert_eq!(p.fraction(), 0.0);
        // An empty op is complete.
        let empty = Progress {
            files_total: 0,
            ..p.clone()
        };
        assert_eq!(empty.fraction(), 1.0);
    }

    // ── copy reports files/bytes ─────────────────────────────────────────────

    #[test]
    fn copy_reports_files_and_bytes_and_completes() {
        let fs = scratch(&[
            ("/src/tree/a.txt", b"aaaa"),
            ("/src/tree/b.txt", b"bb"),
            ("/src/tree/sub/c.txt", b"cccccc"),
        ]);
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/tree")],
            dest_dir: PathBuf::from("/dst"),
        };
        let control = OpControl::new();
        let (outcome, seen) = run(&fs, &op, &control, &mut FixedResolution::overwrite());
        assert!(!outcome.cancelled && outcome.error.is_none());
        assert_eq!(outcome.items_completed, 1);
        // Entries: tree, sub, a, b, c = 5; bytes = 4+2+6 = 12.
        assert_eq!(outcome.files_done, 5);
        assert_eq!(outcome.bytes_done, 12);
        // The tree really landed.
        assert_eq!(fs.read("/dst/tree/a.txt").expect("read"), b"aaaa");
        assert_eq!(fs.read("/dst/tree/sub/c.txt").expect("read"), b"cccccc");
        // Progress was emitted and monotonic, ending at the totals.
        let last = seen.last().expect("progress emitted");
        assert_eq!(last.files_total, 5);
        assert_eq!(last.bytes_total, 12);
        assert!(seen.windows(2).all(|w| w[1].files_done >= w[0].files_done));
    }

    // ── conflicts: overwrite / skip / keep-both ──────────────────────────────

    #[test]
    fn conflict_overwrite_replaces_the_file() {
        let fs = scratch(&[("/src/f.txt", b"new")]);
        fs.seed_file("/dst/f.txt", b"old").expect("seed dst");
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/f.txt")],
            dest_dir: PathBuf::from("/dst"),
        };
        let (outcome, _) = run(
            &fs,
            &op,
            &OpControl::new(),
            &mut FixedResolution::overwrite(),
        );
        assert_eq!(outcome.items_completed, 1);
        assert_eq!(fs.read("/dst/f.txt").expect("read"), b"new");
    }

    #[test]
    fn conflict_skip_leaves_the_destination() {
        let fs = scratch(&[("/src/f.txt", b"new")]);
        fs.seed_file("/dst/f.txt", b"old").expect("seed dst");
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/f.txt")],
            dest_dir: PathBuf::from("/dst"),
        };
        let (outcome, _) = run(&fs, &op, &OpControl::new(), &mut FixedResolution::skip());
        assert_eq!(outcome.items_skipped, 1);
        assert_eq!(outcome.items_completed, 0);
        assert_eq!(fs.read("/dst/f.txt").expect("read"), b"old");
    }

    #[test]
    fn conflict_keep_both_makes_a_renamed_copy() {
        let fs = scratch(&[("/src/f.txt", b"new")]);
        fs.seed_file("/dst/f.txt", b"old").expect("seed dst");
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/f.txt")],
            dest_dir: PathBuf::from("/dst"),
        };
        let (outcome, _) = run(
            &fs,
            &op,
            &OpControl::new(),
            &mut FixedResolution::keep_both(),
        );
        assert_eq!(outcome.items_completed, 1);
        assert_eq!(
            fs.read("/dst/f.txt").expect("read"),
            b"old",
            "original kept"
        );
        assert_eq!(
            fs.read("/dst/f copy.txt").expect("read"),
            b"new",
            "renamed copy landed"
        );
    }

    #[test]
    fn apply_to_all_asks_once_then_reuses_the_choice() {
        let fs = scratch(&[("/src/a.txt", b"A"), ("/src/b.txt", b"B")]);
        fs.seed_file("/dst/a.txt", b"oldA").expect("seed");
        fs.seed_file("/dst/b.txt", b"oldB").expect("seed");
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/a.txt"), PathBuf::from("/src/b.txt")],
            dest_dir: PathBuf::from("/dst"),
        };
        let calls = std::cell::Cell::new(0u32);
        let mut resolver = FnResolver(|_c: &Conflict| {
            calls.set(calls.get() + 1);
            ConflictChoice::all(Resolution::Overwrite)
        });
        let (outcome, _) = run(&fs, &op, &OpControl::new(), &mut resolver);
        assert_eq!(outcome.items_completed, 2);
        assert_eq!(calls.get(), 1, "apply-to-all consults the resolver once");
        assert_eq!(fs.read("/dst/a.txt").expect("read"), b"A");
        assert_eq!(fs.read("/dst/b.txt").expect("read"), b"B");
    }

    // ── recursive directory merge ────────────────────────────────────────────

    #[test]
    fn overwrite_merges_directories_and_children_re_resolve() {
        // Destination dir already has {shared:"old", keep:"k"}; source dir has
        // {shared:"new", extra:"e"}. A per-child scripted resolver overwrites
        // the colliding `shared` but the merge must preserve `keep` and add
        // `extra`.
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/src")).expect("mkdir");
        fs.create_dir(Path::new("/dst")).expect("mkdir");
        fs.create_dir(Path::new("/src/dir")).expect("mkdir");
        fs.seed_file("/src/dir/shared", b"new").expect("seed");
        fs.seed_file("/src/dir/extra", b"e").expect("seed");
        fs.create_dir(Path::new("/dst/dir")).expect("mkdir");
        fs.seed_file("/dst/dir/shared", b"old").expect("seed");
        fs.seed_file("/dst/dir/keep", b"k").expect("seed");

        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/dir")],
            dest_dir: PathBuf::from("/dst"),
        };
        // Overwrite the dir (→ merge) and the colliding `shared` file.
        let mut resolver = FnResolver(|_c: &Conflict| ConflictChoice::once(Resolution::Overwrite));
        let (outcome, _) = run(&fs, &op, &OpControl::new(), &mut resolver);
        assert_eq!(outcome.items_completed, 1);
        assert_eq!(
            fs.read("/dst/dir/shared").expect("read"),
            b"new",
            "replaced"
        );
        assert_eq!(fs.read("/dst/dir/keep").expect("read"), b"k", "preserved");
        assert_eq!(fs.read("/dst/dir/extra").expect("read"), b"e", "added");
    }

    #[test]
    fn merge_with_skip_only_skips_the_colliding_child() {
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/src")).expect("mkdir");
        fs.create_dir(Path::new("/dst")).expect("mkdir");
        fs.create_dir(Path::new("/src/dir")).expect("mkdir");
        fs.seed_file("/src/dir/shared", b"new").expect("seed");
        fs.seed_file("/src/dir/extra", b"e").expect("seed");
        fs.create_dir(Path::new("/dst/dir")).expect("mkdir");
        fs.seed_file("/dst/dir/shared", b"old").expect("seed");

        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/dir")],
            dest_dir: PathBuf::from("/dst"),
        };
        // Merge the dir, but skip the colliding `shared`; `extra` has no
        // collision so it is copied regardless of the skip policy.
        let mut resolver = FnResolver(|c: &Conflict| {
            if c.dst.ends_with("dir") {
                ConflictChoice::once(Resolution::Overwrite)
            } else {
                ConflictChoice::once(Resolution::Skip)
            }
        });
        let (outcome, _) = run(&fs, &op, &OpControl::new(), &mut resolver);
        assert_eq!(outcome.items_completed, 1);
        assert_eq!(fs.read("/dst/dir/shared").expect("read"), b"old", "skipped");
        assert_eq!(fs.read("/dst/dir/extra").expect("read"), b"e", "added");
    }

    // ── cancel leaves no half-files ──────────────────────────────────────────

    #[test]
    fn cancel_rolls_back_the_in_flight_item() {
        let fs = scratch(&[
            ("/src/tree/a", b"a"),
            ("/src/tree/b", b"b"),
            ("/src/tree/c", b"c"),
            ("/src/tree/d", b"d"),
            ("/src/tree/e", b"e"),
        ]);
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/tree")],
            dest_dir: PathBuf::from("/dst"),
        };
        let control = OpControl::new();
        // Cancel mid-item: after 3 entries (the dir + a + b) have completed.
        let trigger = control.clone();
        let outcome;
        {
            let mut sink = |p: Progress| {
                if p.files_done == 3 {
                    trigger.cancel();
                }
            };
            outcome = execute(
                &fs,
                &op,
                7,
                &control,
                &mut FixedResolution::overwrite(),
                &mut sink,
            );
        }
        assert!(outcome.cancelled);
        assert_eq!(outcome.items_completed, 0, "nothing claimed complete");
        // The partial destination was rolled back — no half-tree survives.
        assert!(!fs.exists("/dst/tree"), "in-flight item cleaned up");
        assert!(!fs.exists("/dst/tree/a"));
        // The source is fully intact.
        assert_eq!(fs.read("/src/tree/a").expect("read"), b"a");
        assert_eq!(fs.read("/src/tree/e").expect("read"), b"e");
    }

    #[test]
    fn cancel_between_items_keeps_completed_ones() {
        let fs = scratch(&[("/src/one", b"1"), ("/src/two", b"2")]);
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/one"), PathBuf::from("/src/two")],
            dest_dir: PathBuf::from("/dst"),
        };
        let control = OpControl::new();
        let trigger = control.clone();
        let outcome;
        {
            // Cancel right after the first file completes (files_done == 1).
            let mut sink = |p: Progress| {
                if p.files_done == 1 {
                    trigger.cancel();
                }
            };
            outcome = execute(
                &fs,
                &op,
                1,
                &control,
                &mut FixedResolution::overwrite(),
                &mut sink,
            );
        }
        assert!(outcome.cancelled);
        assert_eq!(outcome.items_completed, 1, "the finished item is kept");
        assert_eq!(fs.read("/dst/one").expect("read"), b"1");
        assert!(!fs.exists("/dst/two"), "the un-started item never appeared");
    }

    // ── move ─────────────────────────────────────────────────────────────────

    #[test]
    fn move_removes_source_after_a_successful_copy() {
        let fs = scratch(&[("/src/f", b"data")]);
        let op = OpKind::Move {
            items: vec![PathBuf::from("/src/f")],
            dest_dir: PathBuf::from("/dst"),
        };
        let (outcome, _) = run(
            &fs,
            &op,
            &OpControl::new(),
            &mut FixedResolution::overwrite(),
        );
        assert_eq!(outcome.items_completed, 1);
        assert_eq!(fs.read("/dst/f").expect("read"), b"data");
        assert!(!fs.exists("/src/f"), "source removed");
    }

    #[test]
    fn cancelled_move_never_deletes_the_source() {
        let fs = scratch(&[
            ("/src/tree/a", b"a"),
            ("/src/tree/b", b"b"),
            ("/src/tree/c", b"c"),
        ]);
        let op = OpKind::Move {
            items: vec![PathBuf::from("/src/tree")],
            dest_dir: PathBuf::from("/dst"),
        };
        let control = OpControl::new();
        let trigger = control.clone();
        let outcome;
        {
            let mut sink = |p: Progress| {
                if p.files_done == 2 {
                    trigger.cancel();
                }
            };
            outcome = execute(
                &fs,
                &op,
                1,
                &control,
                &mut FixedResolution::overwrite(),
                &mut sink,
            );
        }
        assert!(outcome.cancelled);
        assert!(!fs.exists("/dst/tree"), "partial copy rolled back");
        // Source survives a cancelled move — no data loss.
        assert_eq!(fs.read("/src/tree/a").expect("read"), b"a");
        assert_eq!(fs.read("/src/tree/c").expect("read"), b"c");
    }

    // ── delete ───────────────────────────────────────────────────────────────

    #[test]
    fn delete_removes_a_tree_and_reports_counts() {
        let fs = scratch(&[("/src/tree/a", b"a"), ("/src/tree/sub/b", b"bb")]);
        let op = OpKind::Delete {
            items: vec![PathBuf::from("/src/tree")],
        };
        let (outcome, _) = run(&fs, &op, &OpControl::new(), &mut FixedResolution::skip());
        assert_eq!(outcome.items_completed, 1);
        assert!(!fs.exists("/src/tree"));
        // Entries removed: a, b, sub, tree = 4; bytes freed = 1+2 = 3.
        assert_eq!(outcome.files_done, 4);
        assert_eq!(outcome.bytes_done, 3);
    }

    #[test]
    fn delete_cancels_between_items() {
        let fs = scratch(&[("/src/one", b"1"), ("/src/two", b"2")]);
        let op = OpKind::Delete {
            items: vec![PathBuf::from("/src/one"), PathBuf::from("/src/two")],
        };
        let control = OpControl::new();
        let trigger = control.clone();
        let outcome;
        {
            let mut sink = |p: Progress| {
                if p.files_done == 1 {
                    trigger.cancel();
                }
            };
            outcome = execute(
                &fs,
                &op,
                1,
                &control,
                &mut FixedResolution::skip(),
                &mut sink,
            );
        }
        assert!(outcome.cancelled);
        // `/src/one` sorts first and was deleted; `/src/two` remains.
        assert!(!fs.exists("/src/one"));
        assert!(fs.exists("/src/two"));
    }

    // ── the background queue (worker thread) ──────────────────────────────────

    #[test]
    fn queue_runs_ops_on_a_worker_thread() {
        let fs = scratch(&[("/src/f.txt", b"payload")]);
        let (etx, erx) = channel::<OpEvent>();
        let queue = OpQueue::spawn(fs, etx);
        let op = QueuedOp {
            op_id: 42,
            kind: OpKind::Copy {
                items: vec![PathBuf::from("/src/f.txt")],
                dest_dir: PathBuf::from("/dst"),
            },
            control: OpControl::new(),
            resolver: Box::new(FixedResolution::overwrite()),
        };
        assert!(queue.submit(op).is_ok());

        // Drain events until Finished.
        let mut finished = None;
        let mut started = false;
        while let Ok(ev) = erx.recv_timeout(Duration::from_secs(5)) {
            match ev {
                OpEvent::Started(id) => {
                    assert_eq!(id, 42);
                    started = true;
                }
                OpEvent::Progress(p) => assert_eq!(p.op_id, 42),
                OpEvent::Finished(o) => {
                    finished = Some(o);
                    break;
                }
            }
        }
        assert!(started, "Started emitted");
        let outcome = finished.expect("Finished emitted");
        assert!(!outcome.cancelled && outcome.error.is_none());
        assert_eq!(outcome.items_completed, 1);
        assert_eq!(outcome.bytes_done, 7);
    }

    #[test]
    fn channel_resolver_answers_an_interactive_conflict() {
        let fs = scratch(&[("/src/f.txt", b"new")]);
        fs.seed_file("/dst/f.txt", b"old").expect("seed");
        let (etx, erx) = channel::<OpEvent>();
        let queue = OpQueue::spawn(fs, etx);
        let (resolver, prompts) = channel_resolver();

        // A responder stands in for the UI: keep-both, apply to all.
        let responder = std::thread::spawn(move || {
            if let Ok(prompt) = prompts.recv_timeout(Duration::from_secs(5)) {
                assert!(prompt.conflict.dst.ends_with("f.txt"));
                prompt.answer(ConflictChoice::all(Resolution::KeepBoth));
            }
        });

        let op = QueuedOp {
            op_id: 5,
            kind: OpKind::Copy {
                items: vec![PathBuf::from("/src/f.txt")],
                dest_dir: PathBuf::from("/dst"),
            },
            control: OpControl::new(),
            resolver: Box::new(resolver),
        };
        assert!(queue.submit(op).is_ok());

        let mut outcome = None;
        while let Ok(ev) = erx.recv_timeout(Duration::from_secs(5)) {
            if let OpEvent::Finished(o) = ev {
                outcome = Some(o);
                break;
            }
        }
        responder.join().expect("responder");
        let outcome = outcome.expect("finished");
        assert_eq!(outcome.items_completed, 1);
    }

    #[test]
    fn pause_blocks_progress_until_resume() {
        let fs = scratch(&[
            ("/src/tree/a", b"a"),
            ("/src/tree/b", b"b"),
            ("/src/tree/c", b"c"),
        ]);
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/tree")],
            dest_dir: PathBuf::from("/dst"),
        };
        let control = OpControl::new();
        control.pause();

        let (ptx, prx) = channel::<Progress>();
        let worker_control = control.clone();
        let handle = std::thread::spawn(move || {
            let mut resolver = FixedResolution::overwrite();
            let mut sink = |p: Progress| {
                let _ = ptx.send(p);
            };
            execute(&fs, &op, 1, &worker_control, &mut resolver, &mut sink)
        });

        // The initial snapshot (files_done == 0) is emitted before the first
        // checkpoint; after that the paused worker must make no progress.
        let first = prx.recv().expect("initial snapshot");
        assert_eq!(first.files_done, 0);
        assert!(
            prx.recv_timeout(Duration::from_millis(250)).is_err(),
            "no further progress while paused"
        );

        control.resume();
        let outcome = handle.join().expect("join worker");
        assert!(!outcome.cancelled);
        assert_eq!(outcome.items_completed, 1);
    }

    #[test]
    fn cancel_while_paused_stops_the_op() {
        let fs = scratch(&[("/src/tree/a", b"a"), ("/src/tree/b", b"b")]);
        let op = OpKind::Copy {
            items: vec![PathBuf::from("/src/tree")],
            dest_dir: PathBuf::from("/dst"),
        };
        let control = OpControl::new();
        control.pause();
        let worker_control = control.clone();
        let handle = std::thread::spawn(move || {
            let mut resolver = FixedResolution::overwrite();
            let mut sink = |_p: Progress| {};
            execute(&fs, &op, 1, &worker_control, &mut resolver, &mut sink)
        });
        // Cancel a paused op — the Condvar wake lets it exit promptly.
        control.cancel();
        let outcome = handle.join().expect("join");
        assert!(outcome.cancelled);
        assert_eq!(outcome.items_completed, 0);
    }

    // ── archive ops ride the same queue (FILEMGR-3) ──────────────────────────

    #[test]
    fn queue_runs_compress_then_extract() {
        use crate::archive::ArchiveFormat;
        use std::collections::HashMap;

        let fs = scratch(&[("/src/proj/a.txt", b"alpha"), ("/src/proj/b.txt", b"bravo")]);
        fs.create_dir(Path::new("/out")).expect("mkdir /out");
        let (etx, erx) = channel::<OpEvent>();
        let queue = OpQueue::spawn(fs, etx);

        let compress_op = QueuedOp {
            op_id: 1,
            kind: OpKind::Compress {
                items: vec![PathBuf::from("/src/proj")],
                base_dir: PathBuf::from("/src"),
                archive: PathBuf::from("/out/proj.zip"),
                format: ArchiveFormat::Zip,
            },
            control: OpControl::new(),
            resolver: Box::new(FixedResolution::overwrite()),
        };
        assert!(queue.submit(compress_op).is_ok(), "submit compress");
        let extract_op = QueuedOp {
            op_id: 2,
            kind: OpKind::Extract {
                archive: PathBuf::from("/out/proj.zip"),
                dest_dir: PathBuf::from("/dst"),
            },
            control: OpControl::new(),
            resolver: Box::new(FixedResolution::overwrite()),
        };
        assert!(queue.submit(extract_op).is_ok(), "submit extract");

        // The queue runs them in order (compress, then extract of what it wrote).
        let mut finished: HashMap<OpId, OpOutcome> = HashMap::new();
        while finished.len() < 2 {
            match erx.recv_timeout(Duration::from_secs(5)) {
                Ok(OpEvent::Finished(o)) => {
                    finished.insert(o.op_id, o);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let comp = finished.get(&1).expect("compress finished");
        assert!(comp.error.is_none(), "compress: {:?}", comp.error);
        assert_eq!(comp.items_completed, 1);
        let ext = finished.get(&2).expect("extract finished");
        assert!(ext.error.is_none(), "extract: {:?}", ext.error);
        assert_eq!(ext.items_completed, 1);
        // 3 members extracted: proj, proj/a.txt, proj/b.txt.
        assert_eq!(ext.files_done, 3);
    }
}

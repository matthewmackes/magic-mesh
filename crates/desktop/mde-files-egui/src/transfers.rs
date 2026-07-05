//! TRANSFERS-8 — the Files surface's **transfers client** (the desktop half of the
//! TRANSFERS-1 `mackesd` `transfers` worker contract).
//!
//! Design: `docs/design/transfers-surface.md` (locks Q1/Q13/Q16). The worker owns
//! the queue + lifecycle daemon-side (§9 — the GUI renders, the daemon executes);
//! this surface only **submits** typed verbs and **reads** the published ledger.
//! Per §6 the desktop tier never depends on `mackesd`: the payloads are a JSON
//! boundary, so the surface owns **local serde mirrors** of the worker's wire
//! shapes ([`TransferJob`] / [`TransferVerb`] / [`Method`] / …), exactly as
//! [`crate::mesh_mount`] mirrors the FILEMGR-5 worker's structs.
//!
//! Unlike mesh-mount (which rides the `mde-bus` `Persist`), the transfers worker's
//! transport is a **node-local file store** — the same store its `mackesd transfer`
//! CLI drives (TRANSFERS-1 `verb.rs` / `ledger.rs`):
//!
//! * **Read** `<store_root>/ledger/<id>.json` — every job's durable record. A cheap
//!   local directory scan, never a peer probe, so an absent worker can neither hang
//!   a read nor fabricate a job ([`FileTransfers::jobs`]).
//! * **Write** `<store_root>/inbox/<seq>.json` — one typed [`TransferVerb`] per file,
//!   drained + applied by the worker each tick (the daemon stays the single ledger
//!   writer, §9 one-state). Submit / cancel / pause / resume all ride this inbox
//!   ([`FileTransfers::dispatch`]); `list` is served directly off the ledger.
//!
//! `<store_root>` resolves from the SAME env the worker + CLI use
//! ([`default_store_root`] — `$MDE_HOME`/`$MACKESD_HOME` `+ /transfers`, else
//! `/var/lib/mde/transfers`), so the GUI, the CLI, and the daemon share one queue.
//! The [`TransfersClient`] seam is injectable so the model is unit-tested headless
//! (a fake) while production talks the file store ([`FileTransfers`]).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ── the protocol lane (mirror of the worker's `job::Method`) ────────────────────

/// The protocol lane a job routes to (Q4).
///
/// A local mirror of the worker's `Method` wire tag
/// (`#[serde(rename_all = "snake_case")]`), cross-checked against the worker's
/// tokens in tests so the surface never depends on `mackesd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// `sftp`/`ssh` to a foreign host.
    Sftp,
    /// `rsync --partial --bwlimit` mirror.
    Rsync,
    /// `wget -c --limit-rate` HTTP download.
    Http,
    /// A browser-enqueued download or scrape output (Q8/Q17 — browser-originated,
    /// never a manual pick; present so the ledger's browser rows decode).
    BrowserDownload,
    /// A node→node move staged through the mesh-share so Syncthing replicates (Q6).
    Node,
    /// A drop into the shared Navidrome music library dir (Q9).
    Music,
}

impl Method {
    /// Every method, in a stable order (mirrors the worker's `Method::ALL`).
    pub const ALL: [Self; 6] = [
        Self::Sftp,
        Self::Rsync,
        Self::Http,
        Self::BrowserDownload,
        Self::Node,
        Self::Music,
    ];

    /// The methods a user can *manually* originate in the New Transfer dialog —
    /// every lane except [`BrowserDownload`](Self::BrowserDownload), which is only
    /// ever enqueued by the browser (Q8/Q17), never hand-typed here.
    pub const MANUAL: [Self; 5] = [Self::Sftp, Self::Rsync, Self::Http, Self::Node, Self::Music];

    /// The canonical lowercase wire token (matches the worker's serde form).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Rsync => "rsync",
            Self::Http => "http",
            Self::BrowserDownload => "browser_download",
            Self::Node => "node",
            Self::Music => "music",
        }
    }

    /// A short human label for a picker / a ledger row's method chip.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sftp => "SFTP",
            Self::Rsync => "rsync",
            Self::Http => "HTTP",
            Self::BrowserDownload => "Browser",
            Self::Node => "Node → Node",
            Self::Music => "Music Library",
        }
    }
}

// ── the job state (mirror of the worker's `job::TransferState`) ─────────────────

/// The live state of a job — the worker's five-state machine (there is no
/// `Cancelled`: a cancel REMOVES the row). A local mirror of the wire tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Accepted, waiting for a free slot (below the parallel cap).
    Queued,
    /// A lane is actively executing it.
    Running,
    /// Held by an operator `pause`.
    Paused,
    /// Completed successfully (terminal).
    Done,
    /// Ended in an honest failure — the reason is on [`TransferJob::error`].
    Failed,
}

impl TransferState {
    /// The lowercase wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    /// A short human label for the state chip.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    /// Still in flight (not yet Done/Failed) — the "active" set the tab surfaces
    /// first and the pause/resume/cancel controls act on.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }

    /// A terminal outcome (Done / Failed) — Clear-completed removes these.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }

    /// `pause` is legal from Queued or Running (the worker's state-machine rule).
    #[must_use]
    pub const fn can_pause(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    /// `resume` is legal only from Paused.
    #[must_use]
    pub const fn can_resume(self) -> bool {
        matches!(self, Self::Paused)
    }
}

// ── the policy knobs (mirror of the worker's `job::TransferPolicy`) ─────────────

/// The per-job policy knobs (Q12 throttle + Q15 integrity). Both default off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferPolicy {
    /// Optional per-job bandwidth cap, passed to the tool verbatim (Q12). `None` is
    /// unthrottled; omitted from the wire when unset (mirrors the worker).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bwlimit: Option<String>,
    /// Verify integrity on completion (Q15 — size + checksum). Off by default.
    #[serde(default)]
    pub verify: bool,
}

// ── the job envelope (mirror of the worker's `job::TransferJob`) ────────────────

/// One typed transfer — the envelope the worker's queue + ledger share.
///
/// A local mirror of the wire record; `#[serde(default)]` on the optional fields
/// keeps a lean worker record (no `error`/`progress`/`bwlimit`) decoding cleanly,
/// and serializing one back produces the byte-identical shape the worker's inbox
/// parses (cross-checked in tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferJob {
    /// Stable job id (client-minted at submit — the ledger filename).
    pub id: String,
    /// Where the bytes come from (a path, a URL, a `host:path`, a peer).
    pub source: String,
    /// Where the bytes land (a path, a peer, the Music Library, a `host:path`).
    pub dest: String,
    /// The lane that will execute it (Q4).
    pub method: Method,
    /// The Q12/Q15 policy knobs.
    #[serde(default)]
    pub policy: TransferPolicy,
    /// The live state.
    pub state: TransferState,
    /// The honest failure reason when `state == Failed` (§7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Live progress percent (0..=100) when a lane reports one; `None` until then
    /// — the surface renders a determinate bar ONLY when this is `Some` (§7 — never
    /// a fabricated percentage; the design's progress-parsing risk note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// Wall-clock ms when the job was submitted.
    pub created_ms: u64,
    /// Wall-clock ms of the last state change.
    pub updated_ms: u64,
}

impl TransferJob {
    /// Mint a fresh Queued job (client side). The worker re-normalizes state to
    /// Queued on submit, but minting it Queued keeps the optimistic local echo
    /// honest until the ledger confirms. The id mirrors the worker's
    /// `<created_ms>-<seq>-<suffix>` filename-safe shape (a process-monotonic seq +
    /// a sub-ms nonce keep it unique across the GUI/CLI/daemon that all mint ids —
    /// no `rand` dep needed on the airgapped farm).
    #[must_use]
    pub fn new(
        source: impl Into<String>,
        dest: impl Into<String>,
        method: Method,
        policy: TransferPolicy,
    ) -> Self {
        let now = now_ms();
        Self {
            id: mint_id(now),
            source: source.into(),
            dest: dest.into(),
            method,
            policy,
            state: TransferState::Queued,
            error: None,
            progress: None,
            created_ms: now,
            updated_ms: now,
        }
    }

    /// A short display of the source → dest route for a ledger row / a status note.
    #[must_use]
    pub fn route(&self) -> String {
        format!("{} \u{2192} {}", self.source, self.dest)
    }
}

// ── the typed verb set (mirror of the worker's `verb::TransferVerb`) ────────────

/// The typed verb set (Q14) — a local mirror of the worker's `TransferVerb`.
///
/// Tagged `#[serde(rename_all = "snake_case", tag = "verb", content = "arg")]`, so
/// the surface serializes the byte-identical inbox shape the worker's `take_verbs`
/// drains. `Submit` carries the whole client-minted job; the lifecycle verbs carry
/// a job id. `List` is a pure read served off the ledger (never inboxed) — it is in
/// the enum for contract completeness only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verb", content = "arg")]
pub enum TransferVerb {
    /// `transfer.submit(job)` — enqueue a new job.
    Submit(TransferJob),
    /// `transfer.cancel(id)` — remove a job (also frees any slot it held).
    Cancel(String),
    /// `transfer.pause(id)` — hold a Queued/Running job.
    Pause(String),
    /// `transfer.resume(id)` — re-arm a Paused job.
    Resume(String),
    /// `transfer.list` — a pure read (served off the ledger, not inboxed).
    List,
}

impl TransferVerb {
    /// The verb token (the inbox filename suffix + logs) — matches the worker's
    /// `TransferVerb::name`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Submit(_) => "submit",
            Self::Cancel(_) => "cancel",
            Self::Pause(_) => "pause",
            Self::Resume(_) => "resume",
            Self::List => "list",
        }
    }
}

// ── store-root resolution + the file transport (mirror of the worker) ───────────

/// Wall-clock milliseconds since the epoch (the id seed + the inbox seq floor).
#[must_use]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// `<created_ms>-<seq>-<nonce>` — mirrors the worker's `job::mint_id` filename
/// shape. The `created_ms` prefix orders the ledger; the process-monotonic `seq`
/// breaks a same-ms tie; the sub-ms nonce keeps ids unique across the GUI/CLI/daemon.
fn mint_id(now: u64) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{now:013}-{seq:012x}-{nonce:08x}")
}

/// The node-LOCAL transfers store root — the SAME path the worker + CLI resolve.
///
/// Mirrors `mackesd_core::workers::transfers::default_store_root` so the GUI shares
/// their ledger + inbox: `$MDE_HOME`/`$MACKESD_HOME` `+ /transfers`, else
/// `/var/lib/mde/transfers`. Coupled by convention on purpose (§6 — never a
/// `mackesd` dep): a GUI launched with the daemon's env reaches its queue;
/// otherwise both fall to the shared `/var/lib/mde` default.
#[must_use]
pub fn default_store_root() -> PathBuf {
    if let Ok(home) = std::env::var("MDE_HOME") {
        return PathBuf::from(home).join("transfers");
    }
    if let Ok(home) = std::env::var("MACKESD_HOME") {
        return PathBuf::from(home).join("transfers");
    }
    PathBuf::from("/var/lib/mde/transfers")
}

/// The inbox directory the surface writes verbs into and the worker drains.
#[must_use]
fn inbox_dir(store_root: &Path) -> PathBuf {
    store_root.join("inbox")
}

/// The ledger directory the worker publishes records into and the surface reads.
#[must_use]
fn ledger_dir(store_root: &Path) -> PathBuf {
    store_root.join("ledger")
}

/// Monotonic per-process inbox seq — mirrors the worker's `verb::next_seq` so the
/// `{:020}` filename sorts in submission order (submit before a later cancel).
fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = now_ms();
    (ms << 16) | (SEQ.fetch_add(1, Ordering::Relaxed) & 0xFFFF)
}

/// Hand a mutating verb to the daemon: one atomic `<seq>-<name>.json` under
/// `inbox/` (temp + rename), exactly the shape the worker's `take_verbs` drains.
///
/// # Errors
/// Serialization or IO failures (no store dir, no write access).
fn write_verb(store_root: &Path, verb: &TransferVerb) -> io::Result<()> {
    let dir = inbox_dir(store_root);
    std::fs::create_dir_all(&dir)?;
    let stem = format!("{:020}-{}", next_seq(), verb.name());
    let body =
        serde_json::to_string(verb).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join(format!(".{stem}.json.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, dir.join(format!("{stem}.json")))
}

/// Read every ledger record, sorted `(created_ms, id)` — the worker's stable FIFO
/// order (the tab re-orders for display via [`display_order`]). Junk / half-written
/// / unparseable files are skipped, never a failed read (mirrors the worker's
/// `Ledger::load_all`).
#[must_use]
fn load_ledger(store_root: &Path) -> Vec<TransferJob> {
    let dir = ledger_dir(store_root);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(job) = serde_json::from_str::<TransferJob>(&data) {
                out.push(job);
            }
        }
    }
    out.sort_by(|a, b| {
        a.created_ms
            .cmp(&b.created_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

// ── the client seam ─────────────────────────────────────────────────────────────

/// The transfers client seam: read the worker's ledger, submit a typed verb.
/// Injectable so the model is unit-tested headless (a fake) while production talks
/// the file store ([`FileTransfers`]).
pub trait TransfersClient {
    /// Every job in the ledger, FIFO order. Non-blocking — a local directory scan,
    /// never a peer probe (an absent worker reads empty, never hangs).
    fn jobs(&self) -> Vec<TransferJob>;

    /// Whether a transfers worker has ever run on this node (its ledger dir
    /// exists) — the honest signal that distinguishes "no jobs yet" from "no worker
    /// here" for the `EmptyState` (§7).
    fn worker_present(&self) -> bool;

    /// Hand a mutating verb to the daemon (via the inbox). `Err` carries an honest,
    /// operator-readable reason; it never blocks on a peer.
    ///
    /// # Errors
    /// A human-readable string when the verb can't be written to the store.
    fn dispatch(&self, verb: &TransferVerb) -> Result<(), String>;
}

/// The live file-store-backed client — a synchronous local ledger read / inbox
/// write against the SAME store the worker + CLI use.
///
/// Holds only the resolved store root. Degrades honestly to an empty read / an
/// error when there's no store — never a panic, never a hang.
pub struct FileTransfers {
    store_root: PathBuf,
}

impl FileTransfers {
    /// Resolve the store root from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            store_root: default_store_root(),
        }
    }

    /// Construct with an explicit store root (tests point this at a tempdir).
    #[must_use]
    pub fn with_root(store_root: PathBuf) -> Self {
        Self { store_root }
    }
}

impl TransfersClient for FileTransfers {
    fn jobs(&self) -> Vec<TransferJob> {
        load_ledger(&self.store_root)
    }

    fn worker_present(&self) -> bool {
        ledger_dir(&self.store_root).is_dir()
    }

    fn dispatch(&self, verb: &TransferVerb) -> Result<(), String> {
        // `list` is a pure read — the worker treats an inboxed `list` as a no-op, so
        // there is nothing to write (the surface reads the ledger directly instead).
        if matches!(verb, TransferVerb::List) {
            return Ok(());
        }
        write_verb(&self.store_root, verb).map_err(|e| {
            format!(
                "Couldn't hand the {} request to the transfers worker: {e}",
                verb.name()
            )
        })
    }
}

// ── the destination registry (Q10 — auto-only + typed-per-job) ──────────────────

/// What kind of standing destination a [`TransferTarget`] is — the row's icon/tone
/// plus the honest label.
///
/// Arbitrary hosts / URLs are NOT pins; they're typed per-job in the New Transfer
/// dialog (Q10), so they never appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A mesh peer from the live roster (node→node via the mesh-share, Q6).
    Peer,
    /// The shared Navidrome music library dir (Q9).
    Music,
    /// The Syncthing-replicated mesh share (Q6).
    MeshShare,
}

/// One auto-registered transfer destination (Q10) — named + routable.
///
/// A drop or a right-click "Send to →" onto it mints a [`TransferJob`] with this
/// `dest` + `method`. The two standing targets (Music / Mesh Share) carry a
/// symbolic, self-describing `dest` the worker's lane resolves to the real
/// replicated path (Q6/Q9 — the GUI never fabricates a local path; the daemon owns
/// resolution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferTarget {
    /// The display label (a peer host, "Music Library", "Mesh Share").
    pub label: String,
    /// The [`TransferJob::dest`] string this target submits with.
    pub dest: String,
    /// The lane a job to this target routes through.
    pub method: Method,
    /// The kind (icon/tone + honest description).
    pub kind: TargetKind,
}

/// Build the auto-only destination registry (Q10).
///
/// From the reachable roster plus the two standing node-state targets: `peers` is
/// `(id, host)` for each reachable peer (the model reads the live roster). Music +
/// Mesh Share are always offered — they're standing node-state destinations
/// (Q6/Q9), resolved daemon-side.
#[must_use]
pub fn build_targets(peers: &[(String, String)]) -> Vec<TransferTarget> {
    let mut out = vec![
        TransferTarget {
            label: "Music Library".to_string(),
            // Symbolic: the music lane auto-registers the real Navidrome library
            // path (Q9); the GUI names the intent, the daemon resolves the path.
            dest: "music:library".to_string(),
            method: Method::Music,
            kind: TargetKind::Music,
        },
        TransferTarget {
            label: "Mesh Share".to_string(),
            // Symbolic: the node lane stages into the Syncthing mesh-share so it
            // replicates (Q6); the daemon owns the `/mnt/mesh-storage` path.
            dest: "mesh-share:".to_string(),
            method: Method::Node,
            kind: TargetKind::MeshShare,
        },
    ];
    for (id, host) in peers {
        out.push(TransferTarget {
            label: host.clone(),
            // A node→node move addressed by peer id; the node lane stages it via
            // the mesh-share so both peers rebooting can't lose it (Q6).
            dest: format!("peer:{id}"),
            method: Method::Node,
            kind: TargetKind::Peer,
        });
    }
    out
}

// ── the ledger view: filters + newest-relevant ordering ─────────────────────────

/// The Transfers tab's state filter (the `MenuBar`'s View-by-state, Q16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateFilter {
    /// Every job.
    #[default]
    All,
    /// In-flight only (Queued / Running / Paused).
    Active,
    /// Queued only.
    Queued,
    /// Running only.
    Running,
    /// Paused only.
    Paused,
    /// Completed only.
    Done,
    /// Failed only.
    Failed,
}

impl StateFilter {
    /// Every filter, in menu order.
    pub const ALL: [Self; 7] = [
        Self::All,
        Self::Active,
        Self::Queued,
        Self::Running,
        Self::Paused,
        Self::Done,
        Self::Failed,
    ];

    /// The menu label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Done => "Completed",
            Self::Failed => "Failed",
        }
    }

    /// Does a job in `state` pass this filter?
    #[must_use]
    pub const fn keep(self, state: TransferState) -> bool {
        match self {
            Self::All => true,
            Self::Active => state.is_active(),
            Self::Queued => matches!(state, TransferState::Queued),
            Self::Running => matches!(state, TransferState::Running),
            Self::Paused => matches!(state, TransferState::Paused),
            Self::Done => matches!(state, TransferState::Done),
            Self::Failed => matches!(state, TransferState::Failed),
        }
    }
}

/// The combined ledger filter — a state predicate plus an optional method
/// restriction (the `MenuBar`'s two View filters, Q16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferFilter {
    /// Filter by lifecycle state.
    pub state: StateFilter,
    /// Filter to a single lane, or `None` for every method.
    pub method: Option<Method>,
}

impl TransferFilter {
    /// Does `job` pass both predicates?
    #[must_use]
    pub fn keep(&self, job: &TransferJob) -> bool {
        self.state.keep(job.state) && self.method.is_none_or(|m| m == job.method)
    }
}

/// The tab's **newest-relevant** display order (design — "live progress list").
///
/// Filter, then surface the in-flight jobs first and, within each group, the most
/// recently touched first. Pure, so the ordering is unit-tested without egui.
#[must_use]
pub fn display_order(jobs: &[TransferJob], filter: &TransferFilter) -> Vec<TransferJob> {
    let mut kept: Vec<TransferJob> = jobs.iter().filter(|j| filter.keep(j)).cloned().collect();
    kept.sort_by(|a, b| {
        // Active jobs first (the operator's live work), then newest `updated_ms`,
        // then id for a stable tie-break.
        b.state
            .is_active()
            .cmp(&a.state.is_active())
            .then_with(|| b.updated_ms.cmp(&a.updated_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    kept
}

/// A per-state tally of the ledger — drives the `MenuBar`'s gating (Pause-all needs
/// a pausable job, Clear-completed needs a terminal one) and the status chip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerCounts {
    /// Jobs eligible for `pause` (Queued or Running).
    pub pausable: usize,
    /// Jobs eligible for `resume` (Paused).
    pub resumable: usize,
    /// Terminal jobs eligible for Clear-completed (Done or Failed).
    pub terminal: usize,
    /// In-flight jobs (Queued/Running/Paused) — the active badge count.
    pub active: usize,
    /// The total job count.
    pub total: usize,
}

impl LedgerCounts {
    /// Tally a ledger snapshot.
    #[must_use]
    pub fn of(jobs: &[TransferJob]) -> Self {
        let mut c = Self::default();
        for j in jobs {
            c.total += 1;
            if j.state.can_pause() {
                c.pausable += 1;
            }
            if j.state.can_resume() {
                c.resumable += 1;
            }
            if j.state.is_terminal() {
                c.terminal += 1;
            }
            if j.state.is_active() {
                c.active += 1;
            }
        }
        c
    }
}

// ── the New Transfer dialog's render-agnostic state (Q13) ───────────────────────

/// The New Transfer dialog's entry state (Q13) — source / dest / method + policy.
///
/// Pure data (no egui) so the whole compile-a-job path is unit-tested headless. A
/// prefilled dialog (opened from a drop / a "Send to →" with the destination
/// already chosen) starts with `dest`/`method` set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTransferForm {
    /// The source (a local path, a URL, a `host:path`, a peer — lane-parsed).
    pub source: String,
    /// The destination (a path, a peer, the Music Library, a `host:path`).
    pub dest: String,
    /// The lane the job routes to.
    pub method: Method,
    /// An optional bandwidth cap token (Q12), e.g. `2m` / `500k`. Blank = unset.
    pub bwlimit: String,
    /// Verify integrity on completion (Q15).
    pub verify: bool,
}

impl Default for NewTransferForm {
    fn default() -> Self {
        Self {
            source: String::new(),
            dest: String::new(),
            method: Method::Rsync,
            bwlimit: String::new(),
            verify: false,
        }
    }
}

impl NewTransferForm {
    /// A blank form seeded with a chosen `dest` + `method` (the drop / "Send to →"
    /// entry points open the dialog pre-pointed at the destination; the user only
    /// fills the source).
    #[must_use]
    pub fn to(dest: impl Into<String>, method: Method) -> Self {
        Self {
            dest: dest.into(),
            method,
            ..Self::default()
        }
    }

    /// Whether the form can be submitted right now (both endpoints non-blank) —
    /// drives the dialog's Submit-button enable state.
    #[must_use]
    pub fn runnable(&self) -> bool {
        !self.source.trim().is_empty() && !self.dest.trim().is_empty()
    }

    /// Compile the form into a client-minted [`TransferJob`], or `None` when an
    /// endpoint is blank (the honest guard behind the disabled Submit button).
    #[must_use]
    pub fn to_job(&self) -> Option<TransferJob> {
        if !self.runnable() {
            return None;
        }
        let bw = self.bwlimit.trim();
        let policy = TransferPolicy {
            bwlimit: (!bw.is_empty()).then(|| bw.to_string()),
            verify: self.verify,
        };
        Some(TransferJob::new(
            self.source.trim(),
            self.dest.trim(),
            self.method,
            policy,
        ))
    }
}

// ── a test double, shared by the model + view test suites ───────────────────────

#[cfg(test)]
pub(crate) mod test_support {
    use super::{TransferJob, TransferVerb, TransfersClient};
    use std::sync::{Arc, Mutex};

    /// An in-memory [`TransfersClient`] for headless tests: canned ledger jobs, a
    /// worker-present flag, and a recorded dispatch log — so a test asserts the
    /// exact verb the surface emitted without a live store or worker. `Clone`
    /// shares the log (an `Arc`), so a test keeps a probe handle after boxing a
    /// clone into the model.
    #[derive(Clone)]
    pub struct FakeTransfers {
        jobs: Vec<TransferJob>,
        present: bool,
        dispatched: Arc<Mutex<Vec<TransferVerb>>>,
    }

    impl FakeTransfers {
        /// A fresh fake: an empty ledger, a present worker, an empty dispatch log.
        pub fn new() -> Self {
            Self {
                jobs: Vec::new(),
                present: true,
                dispatched: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Seed the canned ledger the fake serves from [`TransfersClient::jobs`].
        #[must_use]
        pub fn with_jobs(mut self, jobs: Vec<TransferJob>) -> Self {
            self.jobs = jobs;
            self
        }

        /// Set the worker-present flag (an absent worker for the `EmptyState` test).
        #[must_use]
        pub fn present(mut self, present: bool) -> Self {
            self.present = present;
            self
        }

        /// The verbs dispatched so far, in order.
        pub fn verbs(&self) -> Vec<TransferVerb> {
            self.dispatched
                .lock()
                .expect("dispatch log mutex poisoned")
                .clone()
        }

        /// How many verbs were dispatched.
        pub fn dispatch_count(&self) -> usize {
            self.dispatched
                .lock()
                .expect("dispatch log mutex poisoned")
                .len()
        }
    }

    impl TransfersClient for FakeTransfers {
        fn jobs(&self) -> Vec<TransferJob> {
            self.jobs.clone()
        }

        fn worker_present(&self) -> bool {
            self.present
        }

        fn dispatch(&self, verb: &TransferVerb) -> Result<(), String> {
            self.dispatched
                .lock()
                .expect("dispatch log mutex poisoned")
                .push(verb.clone());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_in(state: TransferState, method: Method, updated: u64) -> TransferJob {
        let mut j = TransferJob::new("/a", "/b", method, TransferPolicy::default());
        j.state = state;
        j.updated_ms = updated;
        j
    }

    // ── wire compatibility (the §6 mirror MUST match the worker's bytes) ─────────

    #[test]
    fn method_tokens_match_the_worker_wire_form() {
        // These MUST equal mackesd_core::workers::transfers::Method's serde tags.
        assert_eq!(Method::Sftp.as_str(), "sftp");
        assert_eq!(Method::Rsync.as_str(), "rsync");
        assert_eq!(Method::Http.as_str(), "http");
        assert_eq!(Method::BrowserDownload.as_str(), "browser_download");
        assert_eq!(Method::Node.as_str(), "node");
        assert_eq!(Method::Music.as_str(), "music");
        for m in Method::ALL {
            let json = serde_json::to_string(&m).unwrap();
            assert_eq!(json, format!("\"{}\"", m.as_str()));
            assert_eq!(serde_json::from_str::<Method>(&json).unwrap(), m);
        }
    }

    #[test]
    fn a_worker_ledger_record_decodes_and_reencodes_identically() {
        // A lean worker record (no error/progress/bwlimit) as `Ledger::upsert`
        // writes it — the mirror must decode it, and re-encoding a submit verb must
        // produce the tagged shape the worker's `take_verbs` parses.
        let raw = r#"{
            "id":"1715000000000-000000000000-0000abcd",
            "source":"/src","dest":"peer:oak","method":"rsync",
            "policy":{"verify":false},"state":"running",
            "progress":42,"created_ms":1715000000000,"updated_ms":1715000000500
        }"#;
        let job: TransferJob = serde_json::from_str(raw).expect("decodes the worker record");
        assert_eq!(job.method, Method::Rsync);
        assert_eq!(job.state, TransferState::Running);
        assert_eq!(job.progress, Some(42));
        assert!(job.error.is_none() && job.policy.bwlimit.is_none());

        // A submit verb serializes to the worker's tagged `{"verb":"submit","arg":{…}}`.
        let verb = TransferVerb::Submit(job);
        let json = serde_json::to_string(&verb).unwrap();
        assert!(json.contains("\"verb\":\"submit\""), "tagged: {json}");
        assert!(json.contains("\"arg\":{"));
        // …and round-trips back through the mirror (symmetry with the worker).
        assert_eq!(serde_json::from_str::<TransferVerb>(&json).unwrap(), verb);

        // The lifecycle verbs carry a bare id string in `arg`.
        let cancel = TransferVerb::Cancel("id-1".into());
        let json = serde_json::to_string(&cancel).unwrap();
        assert!(json.contains("\"verb\":\"cancel\"") && json.contains("\"arg\":\"id-1\""));
    }

    #[test]
    fn default_policy_omits_bwlimit_on_the_wire() {
        let json = serde_json::to_string(&TransferPolicy::default()).unwrap();
        assert!(
            !json.contains("bwlimit"),
            "unthrottled omits bwlimit: {json}"
        );
        assert!(json.contains("\"verify\":false"));
    }

    // ── the file transport: inbox write + ledger read round-trip ─────────────────

    #[test]
    fn dispatch_writes_a_drainable_inbox_verb() {
        let tmp = tempfile::tempdir().unwrap();
        let client = FileTransfers::with_root(tmp.path().to_path_buf());
        let job = TransferJob::new("/s", "/d", Method::Http, TransferPolicy::default());
        client
            .dispatch(&TransferVerb::Submit(job.clone()))
            .expect("submit writes an inbox verb");
        // Exactly one `*.json` landed under inbox/ (no lingering .tmp), and it
        // parses back as the submit verb the worker's `take_verbs` would drain.
        let files: Vec<_> = std::fs::read_dir(inbox_dir(tmp.path()))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .filter(|p| !p.file_name().unwrap().to_str().unwrap().starts_with('.'))
            .collect();
        assert_eq!(files.len(), 1, "one drainable inbox file");
        let body = std::fs::read_to_string(&files[0]).unwrap();
        let parsed: TransferVerb = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, TransferVerb::Submit(job));
        // `list` is a pure read — dispatching it writes nothing.
        client.dispatch(&TransferVerb::List).unwrap();
        let count = std::fs::read_dir(inbox_dir(tmp.path()))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .filter(|e| !e.file_name().to_str().unwrap().starts_with('.'))
            .count();
        assert_eq!(count, 1, "list is not inboxed");
    }

    #[test]
    fn jobs_reads_the_ledger_and_worker_presence_tracks_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let client = FileTransfers::with_root(tmp.path().to_path_buf());
        // No ledger dir yet → no worker has run here, and an empty read (never a hang).
        assert!(!client.worker_present());
        assert!(client.jobs().is_empty());
        // Simulate the worker publishing two records (one junk file is skipped).
        let dir = ledger_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = job_in(TransferState::Done, Method::Rsync, 100);
        a.created_ms = 100;
        let mut b = job_in(TransferState::Running, Method::Http, 50);
        b.created_ms = 200; // later submit than `a`
        std::fs::write(
            dir.join(format!("{}.json", a.id)),
            serde_json::to_string_pretty(&a).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join(format!("{}.json", b.id)),
            serde_json::to_string_pretty(&b).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "junk").unwrap();
        std::fs::write(dir.join(".half.json.tmp"), "{ torn").unwrap();
        assert!(client.worker_present(), "the ledger dir marks a worker ran");
        let jobs = client.jobs();
        assert_eq!(jobs.len(), 2, "two records, junk skipped");
        // Ledger order is by created_ms: `a` (100) before `b` (200).
        assert_eq!(jobs[0].id, a.id);
    }

    // ── the destination registry (Q10) ───────────────────────────────────────────

    #[test]
    fn targets_are_the_two_standing_plus_one_per_reachable_peer() {
        let targets = build_targets(&[
            ("oak-id".into(), "oak".into()),
            ("pine-id".into(), "pine".into()),
        ]);
        // Music + Mesh Share are always offered; then one Node target per peer.
        assert_eq!(targets.len(), 4);
        assert_eq!(targets[0].kind, TargetKind::Music);
        assert_eq!(targets[0].method, Method::Music);
        assert_eq!(targets[1].kind, TargetKind::MeshShare);
        assert_eq!(targets[1].method, Method::Node);
        assert_eq!(targets[2].kind, TargetKind::Peer);
        assert_eq!(targets[2].dest, "peer:oak-id");
        assert_eq!(targets[2].method, Method::Node);
        // Peerless still offers the two standing node-state destinations.
        assert_eq!(build_targets(&[]).len(), 2);
    }

    // ── filters + newest-relevant ordering ────────────────────────────────────────

    #[test]
    fn filter_keeps_by_state_and_method() {
        let running = job_in(TransferState::Running, Method::Rsync, 10);
        let done = job_in(TransferState::Done, Method::Http, 20);
        assert!(
            TransferFilter::default().keep(&running),
            "All keeps everything"
        );
        let active = TransferFilter {
            state: StateFilter::Active,
            method: None,
        };
        assert!(active.keep(&running) && !active.keep(&done));
        let http_only = TransferFilter {
            state: StateFilter::All,
            method: Some(Method::Http),
        };
        assert!(http_only.keep(&done) && !http_only.keep(&running));
    }

    #[test]
    fn display_order_surfaces_active_first_then_newest() {
        let jobs = vec![
            job_in(TransferState::Done, Method::Rsync, 100),
            job_in(TransferState::Running, Method::Http, 50),
            job_in(TransferState::Queued, Method::Node, 90),
            job_in(TransferState::Failed, Method::Sftp, 200),
        ];
        let ordered = display_order(&jobs, &TransferFilter::default());
        let states: Vec<TransferState> = ordered.iter().map(|j| j.state).collect();
        // Active first (Queued@90 before Running@50 — newer updated_ms wins within
        // the active group), then the terminal jobs newest-first (Failed@200, Done@100).
        assert_eq!(
            states,
            vec![
                TransferState::Queued,
                TransferState::Running,
                TransferState::Failed,
                TransferState::Done,
            ]
        );
    }

    #[test]
    fn ledger_counts_tally_the_control_gates() {
        let jobs = vec![
            job_in(TransferState::Queued, Method::Rsync, 1),
            job_in(TransferState::Running, Method::Http, 2),
            job_in(TransferState::Paused, Method::Node, 3),
            job_in(TransferState::Done, Method::Music, 4),
            job_in(TransferState::Failed, Method::Sftp, 5),
        ];
        let c = LedgerCounts::of(&jobs);
        assert_eq!(c.pausable, 2, "Queued + Running are pausable");
        assert_eq!(c.resumable, 1, "Paused is resumable");
        assert_eq!(c.terminal, 2, "Done + Failed are terminal");
        assert_eq!(c.active, 3, "Queued + Running + Paused are active");
        assert_eq!(c.total, 5);
    }

    // ── the New Transfer form (Q13) ───────────────────────────────────────────────

    #[test]
    fn new_transfer_form_compiles_a_job_only_when_complete() {
        let mut form = NewTransferForm::default();
        assert!(
            !form.runnable() && form.to_job().is_none(),
            "blank isn't runnable"
        );
        form.source = "  /home/me/clip.mp3  ".into();
        form.dest = " music:library ".into();
        form.method = Method::Music;
        form.bwlimit = "2m".into();
        form.verify = true;
        assert!(form.runnable());
        let job = form.to_job().expect("a complete form compiles a job");
        assert_eq!(job.source, "/home/me/clip.mp3", "endpoints are trimmed");
        assert_eq!(job.dest, "music:library");
        assert_eq!(job.method, Method::Music);
        assert_eq!(job.policy.bwlimit.as_deref(), Some("2m"));
        assert!(job.policy.verify);
        assert_eq!(job.state, TransferState::Queued);
    }

    #[test]
    fn prefilled_form_points_at_the_destination() {
        let form = NewTransferForm::to("peer:oak-id", Method::Node);
        assert_eq!(form.dest, "peer:oak-id");
        assert_eq!(form.method, Method::Node);
        assert!(form.source.is_empty() && !form.runnable());
    }
}

//! BOOKMARKS-2 — the mackesd **bookmarks worker** (the mesh-synced bookmark
//! collection over the encrypted Syncthing share).
//!
//! Design: `docs/design/mesh-bookmarks.md` (locks Q8/Q17–Q24, Q64, Q90/Q91).
//! Builds on the landed pure `mde-bookmarks` crate — the [`Collection`] LWW-CRDT,
//! the [`Op`]/[`OpKind`] set, the [`HlcClock`], and [`key_between`] fractional
//! order — and adds the mesh-side *plumbing* the pure crate deliberately omits:
//! persistence, the Syncthing op segments, replay-merge, snapshot/prune, and the
//! Bus surface.
//!
//! ## What this worker owns
//!
//! * **Per-node append-only op segments** (lock Q17). Every node writes ops only
//!   into its own segment (`<root>/bookmarks/<node>/segment.jsonl`, one JSON
//!   [`Op`] per line); no two nodes ever write the same file, so Syncthing never
//!   sees a write conflict. A peer *reads* every node's segment.
//! * **Replay-merge** (locks Q19/Q22). Fold every peer's `(snapshot ⊕ segment)`
//!   through the existing [`Collection`] CRDT into one converged tree. The merge
//!   is commutative/associative/idempotent, so any node that has seen the same op
//!   set converges to the byte-identical [`Collection`] (the convergence
//!   property, proven by [`tests::two_nodes_converge_after_replay_merge`]).
//! * **Snapshot + prune** (lock Q20). Periodically fold this node's own tail into
//!   its own snapshot [`Collection`] (superseded LWW register writes collapse to
//!   the winner) and truncate the tail — bounded growth. A fresh node converges
//!   by replaying every node's `snapshot ⊕ tail` (no bootstrap RPC, lock Q19).
//! * **In-memory index + periodic-flush durability** (lock Q90). The live
//!   converged tree is held in memory for a snappy [`state/bookmarks/collection`]
//!   publish; every op is appended to the node-local segment on write and the
//!   [`HlcClock`] is persisted, so a restart replays the local store and resumes
//!   exactly where it left off.
//! * **Offline-first** (lock Q91). Edits apply to the in-memory index and the
//!   node-local segment *immediately*, even when the shared Syncthing folder is
//!   down — the mirror to the share is simply skipped and the published
//!   [`SyncStatus`] reports `syncing: false` + the offline backlog. When the
//!   share reappears the next flush mirrors the backlog out and merges peers back
//!   in (silent CRDT converge — no operator action).
//!
//! ## §6 / §7 posture — nothing faked
//!
//! Unlike the sshfs `mesh_mount` worker (which honestly gates a genuinely
//! node-only FUSE transport behind a typed `Gated` error), this worker has **no
//! external transport to fake**: Syncthing replication is done by the daemon out
//! of band, and the worker's whole job is real, testable file I/O against a
//! directory — it runs unchanged on a headless farm/CI box. The one
//! environmental condition is whether the canonical shared mount
//! (`/mnt/mesh-storage`) is actually present, which is the existing
//! [`crate::shared_root_writable`] guard (AUDIT-MESH-15): when it is not, the
//! worker keeps editing locally and honestly publishes an **offline**
//! [`SyncStatus`] — never a faked converge, never a write into a bare
//! unprovisioned mount. The reused Syncthing seam is the same
//! `<workgroup_root>` = `/mnt/mesh-storage` share the `ssh_pubkey_gossip` /
//! `chat` workers already publish per-node files into (§6, single substrate).
//!
//! Author attribution (lock Q64): the worker only ever mints ops for the local
//! authenticated user ([`resolve_user`]) on the local node, and that `(user,
//! node)` [`Author`] survives the merge.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mde_bookmarks::{key_between, Author, Collection, Hlc, HlcClock, Item, Op, OpKind, Source};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use uuid::Uuid;

use super::{ShutdownToken, Worker};

/// The `action/bookmarks/` RPC domain prefix this worker drains.
///
/// A request topic is `action/bookmarks/<verb>` — the `<verb>` is the topic's
/// verb slot (`action/<domain>/+`, §9), and the body carries the typed payload
/// ([`parse_action`]).
pub const ACTION_PREFIX: &str = "action/bookmarks/";

/// Retained-latest topic carrying the converged [`Collection`] snapshot the
/// `Surface::Bookmarks` UI (BOOKMARKS-4) renders.
pub const STATE_COLLECTION: &str = "state/bookmarks/collection";

/// Retained-latest topic carrying the [`SyncStatus`] (the freshness / "not
/// syncing" indicator, locks Q21/Q91).
pub const STATE_SYNC: &str = "state/bookmarks/sync";

/// Retained-latest topic carrying the most recent daemon-owned bookmark
/// dead-link check. This is operational state, not CRDT bookmark content.
pub const STATE_LINK_CHECK: &str = "state/bookmarks/link-check";

/// Event topic emitted for every operator-requested bookmark dead-link check.
pub const EVENT_LINK_CHECK: &str = "event/bookmarks/link-check";

/// The share subdirectory the per-node bookmark stores live under
/// (`<root>/bookmarks/<node>/…`).
///
/// One directory per node keeps every node a single-writer of its own files
/// (lock Q17 — no Syncthing conflicts).
pub const BOOKMARKS_SUBDIR: &str = "bookmarks";

/// This node's append-only op segment file name (JSON-lines).
pub const SEGMENT_FILE: &str = "segment.jsonl";

/// This node's folded snapshot file name (a serialized [`Collection`]).
pub const SNAPSHOT_FILE: &str = "snapshot.json";

/// This node's persisted [`HlcClock`] file name — kept node-local (never
/// mirrored to the share) so op stamps stay monotonic across a restart.
pub const CLOCK_FILE: &str = "clock.json";

/// Default poll/flush cadence. Bookmark edits are human-driven + rare, so a 3 s
/// tick keeps convergence imperceptible without polling storms.
pub const DEFAULT_TICK: Duration = Duration::from_secs(3);

/// Default number of flush ticks between a snapshot+prune pass (~1 min at the
/// default cadence). Prune also fires early once the tail crosses
/// [`DEFAULT_TAIL_THRESHOLD`].
pub const DEFAULT_PRUNE_EVERY: u32 = 20;

/// Fold the tail into the snapshot once it grows past this many ops, so a burst
/// of edits can't unbound the segment between periodic prunes (lock Q20).
pub const DEFAULT_TAIL_THRESHOLD: usize = 256;

/// The `action/bookmarks/<verb>` slot used for daemon-owned dead-link checks.
const CHECK_LINKS_VERB: &str = "check-links";

/// Hard cap per link-check request. The worker is operator-triggered but still
/// bounded so a stale imported collection cannot turn into a network storm.
const MAX_LINK_CHECKS: usize = 64;

/// Maximum diagnostic detail stored per checked URL.
const MAX_LINK_DETAIL: usize = 160;

/// A wall-clock source (ms since the Unix epoch). Injected so the model stays
/// pure and tests drive a deterministic fake clock.
type NowFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Injected URL probe. Production uses a bounded fixed-argv `curl`; tests use a
/// deterministic pure map so the suite never depends on the public network.
type LinkProbeFn = Arc<dyn Fn(&str) -> LinkProbeOutcome + Send + Sync>;

// ── the typed Bus action ────────────────────────────────────────────────────

/// The typed body of an `action/bookmarks/<verb>` request, minted into a real
/// [`Op`] by the worker.
///
/// There is no free-text/command variant (§9): the closed set mirrors the
/// [`OpKind`] surface the design pins (add / edit / move / delete / add-folder /
/// rename).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookmarkAction {
    /// Add a bookmark leaf. The id + fractional order key are minted worker-side
    /// unless an optimistic UI supplies its already-applied values.
    Add {
        /// Optional client-supplied id.
        id: Option<Uuid>,
        /// Parent folder, or `None` for top level.
        parent: Option<Uuid>,
        /// Optional client-supplied fractional order key.
        order_key: Option<String>,
        /// Target URL.
        url: String,
        /// Display title (falls back to the URL when empty).
        title: String,
        /// Tags to keep.
        tags: Vec<String>,
        /// Free-text notes.
        notes: String,
        /// Origin (defaults to [`Source::Manual`]).
        source: Option<Source>,
    },
    /// Edit one or more bookmark fields; a `None` field is left untouched.
    Edit {
        /// The bookmark to edit.
        id: Uuid,
        /// New URL, if changed.
        url: Option<String>,
        /// New title, if changed.
        title: Option<String>,
        /// New tag set, if changed.
        tags: Option<Vec<String>>,
        /// New notes, if changed.
        notes: Option<String>,
    },
    /// Reparent and/or reorder an item — one op (lock Q3). `before`/`after` name
    /// the sibling ids the item should land between when no explicit order key is
    /// supplied.
    Move {
        /// The item to move.
        id: Uuid,
        /// The new parent, or `None` for top level.
        parent: Option<Uuid>,
        /// Optional client-supplied fractional order key.
        order_key: Option<String>,
        /// The sibling that should sort *before* the moved item, if any.
        before: Option<Uuid>,
        /// The sibling that should sort *after* the moved item, if any.
        after: Option<Uuid>,
    },
    /// Delete an item (LWW on the `deleted` register — lock Q4).
    Delete {
        /// The item to delete.
        id: Uuid,
    },
    /// Add a folder. The id + order key are minted worker-side unless an
    /// optimistic UI supplies its already-applied values.
    AddFolder {
        /// Optional client-supplied id.
        id: Option<Uuid>,
        /// Parent folder, or `None` for top level.
        parent: Option<Uuid>,
        /// Optional client-supplied fractional order key.
        order_key: Option<String>,
        /// The folder name.
        name: String,
    },
    /// Rename a folder.
    Rename {
        /// The folder to rename.
        id: Uuid,
        /// The new name.
        name: String,
    },
}

// The per-verb request payloads. Private: the wire shape is an implementation
// detail of `parse_action`; the rest of the worker speaks `BookmarkAction`.
#[derive(serde::Deserialize)]
struct AddReq {
    #[serde(default)]
    id: Option<Uuid>,
    #[serde(default)]
    parent: Option<Uuid>,
    #[serde(default)]
    order_key: Option<String>,
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    source: Option<Source>,
}

#[derive(serde::Deserialize)]
struct EditReq {
    id: Uuid,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(serde::Deserialize)]
struct MoveReq {
    id: Uuid,
    #[serde(default)]
    parent: Option<Uuid>,
    #[serde(default)]
    order_key: Option<String>,
    #[serde(default)]
    before: Option<Uuid>,
    #[serde(default)]
    after: Option<Uuid>,
}

#[derive(serde::Deserialize)]
struct IdReq {
    id: Uuid,
}

#[derive(serde::Deserialize)]
struct AddFolderReq {
    #[serde(default)]
    id: Option<Uuid>,
    #[serde(default)]
    parent: Option<Uuid>,
    #[serde(default)]
    order_key: Option<String>,
    name: String,
}

#[derive(serde::Deserialize)]
struct RenameReq {
    id: Uuid,
    name: String,
}

/// Parse a typed [`BookmarkAction`] from the topic's `<verb>` slot + JSON body.
///
/// An empty body is treated as `{}` so verbs whose fields are all optional
/// (none, currently) still parse; a verb whose required field is absent surfaces
/// as a typed error rather than a silent no-op.
///
/// # Errors
/// An unknown verb or a body missing a required field (e.g. `id` on `edit`)
/// returns a human-readable message.
pub fn parse_action(verb: &str, body: &str) -> Result<BookmarkAction, String> {
    let body = body.trim();
    let json = if body.is_empty() { "{}" } else { body };
    let malformed = |e: serde_json::Error| format!("malformed `{verb}` bookmarks request: {e}");
    match verb {
        "add" => {
            let r: AddReq = serde_json::from_str(json).map_err(malformed)?;
            Ok(BookmarkAction::Add {
                id: r.id,
                parent: r.parent,
                order_key: r.order_key,
                url: r.url,
                title: r.title,
                tags: r.tags,
                notes: r.notes,
                source: r.source,
            })
        }
        "edit" => {
            let r: EditReq = serde_json::from_str(json).map_err(malformed)?;
            Ok(BookmarkAction::Edit {
                id: r.id,
                url: r.url,
                title: r.title,
                tags: r.tags,
                notes: r.notes,
            })
        }
        "move" => {
            let r: MoveReq = serde_json::from_str(json).map_err(malformed)?;
            Ok(BookmarkAction::Move {
                id: r.id,
                parent: r.parent,
                order_key: r.order_key,
                before: r.before,
                after: r.after,
            })
        }
        "delete" => {
            let r: IdReq = serde_json::from_str(json).map_err(malformed)?;
            Ok(BookmarkAction::Delete { id: r.id })
        }
        // Accept both the hyphen (topic-friendly) and underscore spellings.
        "add-folder" | "add_folder" => {
            let r: AddFolderReq = serde_json::from_str(json).map_err(malformed)?;
            Ok(BookmarkAction::AddFolder {
                id: r.id,
                parent: r.parent,
                order_key: r.order_key,
                name: r.name,
            })
        }
        "rename" => {
            let r: RenameReq = serde_json::from_str(json).map_err(malformed)?;
            Ok(BookmarkAction::Rename {
                id: r.id,
                name: r.name,
            })
        }
        other => Err(format!("unknown bookmarks action verb `{other}`")),
    }
}

// ── the published sync status ────────────────────────────────────────────────

/// The per-node sync health published to [`STATE_SYNC`] — the surface's
/// freshness / "not syncing" indicator (locks Q21/Q91) + the Workbench fleet
/// view (lock Q48).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncStatus {
    /// This node's id.
    pub node: String,
    /// Whether the shared Syncthing folder is present + writable this tick.
    pub share_reachable: bool,
    /// Whether ops are currently reaching the mesh (mirrors `share_reachable`);
    /// the UI shows a subtle "not syncing" pip when false (lock Q91).
    pub syncing: bool,
    /// How many *other* nodes' segments this node is merging.
    pub peers: usize,
    /// Live item count in the converged collection.
    pub items: usize,
    /// Local ops appended since the last successful mirror to the share — the
    /// offline backlog (0 when fully synced).
    pub pending_local_ops: usize,
    /// Wall-clock epoch millis of the last flush.
    pub last_flush_ms: u64,
    /// Wall-clock epoch millis of the last successful mirror to the share, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_mirror_ms: Option<u64>,
}

/// Operator request body for `action/bookmarks/check-links`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkCheckRequest {
    /// Optional specific bookmark ids to check. Empty means "check the current
    /// converged collection's bookmarks up to the request limit".
    pub ids: Vec<Uuid>,
    /// Per-request cap, clamped to [`MAX_LINK_CHECKS`].
    pub limit: usize,
}

#[derive(serde::Deserialize)]
struct LinkCheckReq {
    #[serde(default)]
    ids: Vec<Uuid>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Per-URL link health class published by the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkHealth {
    /// HTTP returned a success/redirect class status.
    Alive,
    /// HTTP returned a client/server failure class status.
    Dead,
    /// The URL scheme is not checkable by this HTTP(S)-only worker.
    Unsupported,
    /// The probe failed before an HTTP status was available.
    Error,
}

/// Result of one URL probe before bookmark metadata is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkProbeOutcome {
    /// Coarse link health.
    pub health: LinkHealth,
    /// HTTP status code, when the transport reached the server.
    pub http_status: Option<u16>,
    /// Short human-readable diagnostic.
    pub detail: String,
}

impl LinkProbeOutcome {
    fn new(health: LinkHealth, http_status: Option<u16>, detail: impl Into<String>) -> Self {
        Self {
            health,
            http_status,
            detail: truncate_detail(&detail.into()),
        }
    }
}

/// One bookmark's dead-link check record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkCheckRecord {
    /// Bookmark id.
    pub id: Uuid,
    /// Bookmark URL.
    pub url: String,
    /// Bookmark title at check time.
    pub title: String,
    /// Coarse link health.
    pub health: LinkHealth,
    /// HTTP status code, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Short human-readable diagnostic.
    pub detail: String,
}

/// Published result of a bookmark dead-link check pass.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkCheckStatus {
    /// Fixed op marker so downstream UIs can validate the payload.
    pub op: String,
    /// This node's id.
    pub node: String,
    /// Wall-clock epoch millis of the check.
    pub checked_at_ms: u64,
    /// Total records checked.
    pub checked: usize,
    /// Number of alive links.
    pub alive: usize,
    /// Number of dead links.
    pub dead: usize,
    /// Number of unsupported-scheme links.
    pub unsupported: usize,
    /// Number of probe errors.
    pub errors: usize,
    /// Whether the request hit the worker cap.
    pub truncated: bool,
    /// Per-bookmark bounded results.
    pub records: Vec<LinkCheckRecord>,
}

/// Parse `action/bookmarks/check-links`.
///
/// # Errors
/// Returns a typed message for malformed JSON.
pub fn parse_link_check_request(body: &str) -> Result<LinkCheckRequest, String> {
    let body = body.trim();
    let json = if body.is_empty() { "{}" } else { body };
    let r: LinkCheckReq = serde_json::from_str(json)
        .map_err(|e| format!("malformed `check-links` bookmarks request: {e}"))?;
    let requested = r.limit.unwrap_or(MAX_LINK_CHECKS);
    Ok(LinkCheckRequest {
        ids: r.ids,
        limit: requested.clamp(1, MAX_LINK_CHECKS),
    })
}

// ── pure store helpers (path building + (de)serialization) ───────────────────

/// The `<root>/bookmarks/` directory.
#[must_use]
fn bookmarks_dir(root: &Path) -> PathBuf {
    root.join(BOOKMARKS_SUBDIR)
}

/// The `<root>/bookmarks/<node>/` directory.
#[must_use]
fn node_dir(root: &Path, node: &str) -> PathBuf {
    bookmarks_dir(root).join(node)
}

/// The `<root>/bookmarks/<node>/segment.jsonl` path.
#[must_use]
fn segment_path(root: &Path, node: &str) -> PathBuf {
    node_dir(root, node).join(SEGMENT_FILE)
}

/// The `<root>/bookmarks/<node>/snapshot.json` path.
#[must_use]
fn snapshot_path(root: &Path, node: &str) -> PathBuf {
    node_dir(root, node).join(SNAPSHOT_FILE)
}

/// The `<root>/bookmarks/<node>/clock.json` path.
#[must_use]
fn clock_path(root: &Path, node: &str) -> PathBuf {
    node_dir(root, node).join(CLOCK_FILE)
}

/// Serialize an op tail to JSON-lines (one [`Op`] per line). A value that fails
/// to serialize is skipped rather than poisoning the whole segment.
#[must_use]
fn serialize_segment(ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        if let Ok(line) = serde_json::to_string(op) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Parse a JSON-lines op segment, skipping blank/corrupt lines (never panics on
/// peer-supplied data — a malformed line is dropped, the rest still merge).
#[must_use]
fn parse_segment(text: &str) -> Vec<Op> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Op>(l).ok())
        .collect()
}

/// Load a node's folded snapshot [`Collection`], or an empty one when absent /
/// corrupt.
#[must_use]
fn load_snapshot(path: &Path) -> Collection {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Collection>(&s).ok())
        .unwrap_or_default()
}

/// Load a persisted [`HlcClock`], if present + parseable.
#[must_use]
fn load_clock(path: &Path) -> Option<HlcClock> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// Read one node's on-disk contribution as a converged [`Collection`]
/// (`snapshot ⊕ segment`) plus the max [`Hlc`] seen in its segment (for the
/// receiver's clock-observe step).
#[must_use]
fn read_node_state(dir: &Path) -> (Collection, Option<Hlc>) {
    let mut coll = load_snapshot(&dir.join(SNAPSHOT_FILE));
    let ops = parse_segment(&std::fs::read_to_string(dir.join(SEGMENT_FILE)).unwrap_or_default());
    let max_hlc = ops.iter().map(|o| o.hlc.clone()).max();
    coll.apply_all(&ops);
    (coll, max_hlc)
}

/// Mint the fractional order key for a *new* item appended to the end of
/// `parent`'s children (lock Q3).
#[must_use]
fn append_order_key(index: &Collection, parent: Option<Uuid>) -> String {
    let kids = index.children(parent);
    let last = kids.last().map(|it| it.order_key().to_string());
    key_between(last.as_deref(), None)
}

/// Mint the order key for a moved item landing between the `before`/`after`
/// siblings (resolving their current keys from the index). With neither bound it
/// appends to the end of `parent`.
#[must_use]
fn move_order_key(
    index: &Collection,
    parent: Option<Uuid>,
    before: Option<Uuid>,
    after: Option<Uuid>,
) -> String {
    let key_of = |id: Uuid| index.item(id).map(|it| it.order_key().to_string());
    let before_key = before.and_then(key_of);
    let after_key = after.and_then(key_of);
    match (before_key, after_key) {
        (b, Some(a)) => key_between(b.as_deref(), Some(&a)),
        (Some(b), None) => key_between(Some(&b), None),
        (None, None) => append_order_key(index, parent),
    }
}

/// Whether this worker can probe `url` directly.
fn is_http_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Production bookmark link probe: a bounded fixed-argument `curl` invocation.
///
/// The URL is passed as one argv element (never through a shell). `--range 0-0`
/// keeps successful probes small while still using GET, which avoids the common
/// `HEAD`-not-supported false negative.
fn probe_link_with_curl(url: &str) -> LinkProbeOutcome {
    if !is_http_url(url) {
        return LinkProbeOutcome::new(
            LinkHealth::Unsupported,
            None,
            "only http:// and https:// bookmark URLs are checked",
        );
    }
    let output = Command::new("curl")
        .args([
            "-L",
            "-sS",
            "--max-time",
            "5",
            "--range",
            "0-0",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            url,
        ])
        .output();
    match output {
        Ok(out) => {
            let code = String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|c| *c != 0);
            if let Some(status) = code {
                return LinkProbeOutcome::new(
                    classify_http_status(status),
                    Some(status),
                    format!("http {status}"),
                );
            }
            let detail = String::from_utf8_lossy(&out.stderr);
            LinkProbeOutcome::new(
                LinkHealth::Error,
                None,
                if detail.trim().is_empty() {
                    format!("curl exited {}", out.status)
                } else {
                    format!("curl exited {}: {}", out.status, detail.trim())
                },
            )
        }
        Err(e) => LinkProbeOutcome::new(LinkHealth::Error, None, format!("curl spawn failed: {e}")),
    }
}

fn classify_http_status(status: u16) -> LinkHealth {
    match status {
        200..=399 => LinkHealth::Alive,
        400..=599 => LinkHealth::Dead,
        _ => LinkHealth::Error,
    }
}

fn truncate_detail(detail: &str) -> String {
    let trimmed = detail.trim();
    if trimmed.len() <= MAX_LINK_DETAIL {
        return trimmed.to_string();
    }
    let mut end = MAX_LINK_DETAIL;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

// ── the worker ───────────────────────────────────────────────────────────────

/// BOOKMARKS-2 — the mesh-synced bookmarks worker.
pub struct BookmarksWorker {
    /// This node's id (the segment owner + op-stamp node).
    node: String,
    /// The local authenticated user ops are attributed to (lock Q64).
    user: String,
    /// The node-local durable root (always writable — offline-first + restart
    /// durability). Holds this node's authoritative own store.
    local_root: PathBuf,
    /// The shared Syncthing root (`/mnt/mesh-storage`): this node mirrors its own
    /// store here when writable and reads every peer's store from here.
    share_root: PathBuf,
    /// The per-node HLC generator (persisted node-local for restart monotonicity).
    clock: HlcClock,
    /// The live converged tree (the in-memory index, lock Q90).
    index: Collection,
    /// This node's own folded snapshot (its ops up to the last prune).
    own_snapshot: Collection,
    /// This node's own ops since the last prune (the append-only tail).
    own_tail: Vec<Op>,
    /// Own ops appended since the last successful mirror (the offline backlog).
    pending: usize,
    /// Peer count observed on the last rebuild (for the published status).
    peer_count: usize,
    /// Flush ticks since the last snapshot+prune.
    prune_counter: u32,
    /// Wall-clock ms of the last flush.
    last_flush_ms: u64,
    /// Wall-clock ms of the last successful mirror, if any.
    last_mirror_ms: Option<u64>,
    /// Poll/flush cadence.
    tick: Duration,
    /// Flush ticks between snapshot+prune passes.
    prune_every: u32,
    /// Fold-early tail threshold.
    tail_threshold: usize,
    /// Per-topic request cursors (`action/bookmarks/<verb>` → last ULID).
    cursors: HashMap<String, String>,
    /// Injected wall clock (tests use a deterministic fake).
    now_fn: NowFn,
    /// Injected bookmark URL health probe.
    link_probe: LinkProbeFn,
    /// Test seam to force the share up/down (offline-first tests). `None` in
    /// production → the real [`crate::shared_root_writable`] guard.
    share_gate: Option<Arc<AtomicBool>>,
    /// Bus spool root override (tests point this at a tempdir).
    bus_root_override: Option<PathBuf>,
}

impl BookmarksWorker {
    /// Construct with production defaults. `local_root` is a node-local durable
    /// dir ([`resolve_local_root`]); `share_root` is the mesh workgroup root
    /// (`/mnt/mesh-storage`).
    #[must_use]
    pub fn new(node: String, user: String, local_root: PathBuf, share_root: PathBuf) -> Self {
        let clock = HlcClock::new(node.clone());
        Self {
            node,
            user,
            local_root,
            share_root,
            clock,
            index: Collection::new(),
            own_snapshot: Collection::new(),
            own_tail: Vec::new(),
            pending: 0,
            peer_count: 0,
            prune_counter: 0,
            last_flush_ms: 0,
            last_mirror_ms: None,
            tick: DEFAULT_TICK,
            prune_every: DEFAULT_PRUNE_EVERY,
            tail_threshold: DEFAULT_TAIL_THRESHOLD,
            cursors: HashMap::new(),
            now_fn: Arc::new(default_now),
            link_probe: Arc::new(probe_link_with_curl),
            share_gate: None,
            bus_root_override: None,
        }
    }

    /// Inject a deterministic wall clock (tests).
    #[must_use]
    pub fn with_now_fn(mut self, now: NowFn) -> Self {
        self.now_fn = now;
        self
    }

    /// Inject a deterministic link probe (tests).
    #[must_use]
    pub fn with_link_probe(mut self, probe: LinkProbeFn) -> Self {
        self.link_probe = probe;
        self
    }

    /// Inject a share-availability gate (offline-first tests).
    #[must_use]
    pub fn with_share_gate(mut self, gate: Arc<AtomicBool>) -> Self {
        self.share_gate = Some(gate);
        self
    }

    /// Override the poll/flush cadence (tests use a short value).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Read-only view of the live converged collection (the UI reads the
    /// published snapshot; this is the in-process accessor + a test seam).
    #[must_use]
    pub const fn collection(&self) -> &Collection {
        &self.index
    }

    /// The current published sync status (also the test accessor).
    #[must_use]
    pub fn status(&self) -> SyncStatus {
        let reachable = self.share_writable();
        SyncStatus {
            node: self.node.clone(),
            share_reachable: reachable,
            syncing: reachable,
            peers: self.peer_count,
            items: self.index.len(),
            pending_local_ops: self.pending,
            last_flush_ms: self.last_flush_ms,
            last_mirror_ms: self.last_mirror_ms,
        }
    }

    fn now_ms(&self) -> u64 {
        (self.now_fn)()
    }

    /// Whether the shared folder is present + writable this tick. The test gate
    /// wins when set; otherwise the AUDIT-MESH-15 canonical-mount guard.
    fn share_writable(&self) -> bool {
        self.share_gate.as_ref().map_or_else(
            || crate::shared_root_writable(&self.share_root),
            |g| g.load(Ordering::SeqCst),
        )
    }

    /// Restore this node's authoritative own store from `local_root` (offline-
    /// proof), reseat the clock for restart monotonicity, then rebuild the index
    /// (folding in any peers already present in the share).
    fn load(&mut self) {
        self.own_snapshot = load_snapshot(&snapshot_path(&self.local_root, &self.node));
        self.own_tail = parse_segment(
            &std::fs::read_to_string(segment_path(&self.local_root, &self.node))
                .unwrap_or_default(),
        );
        // A persisted clock already dominates every op it ever minted; on a fresh
        // store, seed from the tail's max stamp so the first new op still sorts
        // after the reloaded history.
        if let Some(clock) = load_clock(&clock_path(&self.local_root, &self.node)) {
            self.clock = clock;
        } else {
            let mut clock = HlcClock::new(self.node.clone());
            if let Some(max) = self.own_tail.iter().map(|o| o.hlc.clone()).max() {
                let _ = clock.observe(&max, self.now_ms());
            }
            self.clock = clock;
        }
        self.rebuild_index();
    }

    /// Mint a real [`Op`] for a typed action: stamp it with the next HLC, attach
    /// the local `(user, node)` author, and resolve ids/order-keys from the live
    /// index.
    fn mint(&mut self, action: BookmarkAction) -> Op {
        let now = self.now_ms();
        let kind = match action {
            BookmarkAction::Add {
                id,
                parent,
                order_key,
                url,
                title,
                tags,
                notes,
                source,
            } => {
                let order_key = order_key.unwrap_or_else(|| append_order_key(&self.index, parent));
                let title = if title.trim().is_empty() {
                    url.clone()
                } else {
                    title
                };
                OpKind::AddBookmark {
                    id: id.unwrap_or_else(Uuid::new_v4),
                    parent,
                    order_key,
                    url,
                    title,
                    favicon_ref: None,
                    tags,
                    notes,
                    added: now,
                    source: source.unwrap_or_default(),
                }
            }
            BookmarkAction::Edit {
                id,
                url,
                title,
                tags,
                notes,
            } => OpKind::EditBookmark {
                id,
                url,
                title,
                favicon_ref: None,
                tags,
                notes,
            },
            BookmarkAction::Move {
                id,
                parent,
                order_key,
                before,
                after,
            } => {
                let order_key =
                    order_key.unwrap_or_else(|| move_order_key(&self.index, parent, before, after));
                OpKind::MoveItem {
                    id,
                    parent,
                    order_key,
                }
            }
            BookmarkAction::Delete { id } => OpKind::DeleteItem { id },
            BookmarkAction::AddFolder {
                id,
                parent,
                order_key,
                name,
            } => {
                let order_key = order_key.unwrap_or_else(|| append_order_key(&self.index, parent));
                OpKind::AddFolder {
                    id: id.unwrap_or_else(Uuid::new_v4),
                    name,
                    parent,
                    order_key,
                }
            }
            BookmarkAction::Rename { id, name } => OpKind::RenameFolder { id, name },
        };
        let hlc = self.clock.tick(now);
        Op::new(hlc, Author::new(self.user.clone(), self.node.clone()), kind)
    }

    /// Apply a typed action locally: mint the op, fold it into the in-memory
    /// index *immediately* (offline-first), append it to the node-local segment
    /// (durability), and track the offline backlog. Returns the minted op.
    fn apply_action(&mut self, action: BookmarkAction) -> Op {
        let op = self.mint(action);
        self.index.apply(&op);
        self.own_tail.push(op.clone());
        self.pending = self.pending.saturating_add(1);
        self.append_local(&op);
        self.persist_clock();
        op
    }

    /// Append one op to the node-local segment (per-op durability). Best-effort +
    /// logged — a write failure never drops the in-memory edit.
    fn append_local(&self, op: &Op) {
        let dir = node_dir(&self.local_root, &self.node);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let Ok(line) = serde_json::to_string(op) else {
            return;
        };
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(segment_path(&self.local_root, &self.node))
        {
            Ok(mut f) => {
                if let Err(e) = writeln!(f, "{line}") {
                    tracing::warn!(target: "mackesd::bookmarks", error = %e, "segment append failed");
                }
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::bookmarks", error = %e, "segment open failed");
            }
        }
    }

    /// Persist this node's authoritative own store to `local_root` (snapshot +
    /// tail + clock). Idempotent full rewrite.
    fn persist_own_local(&self) {
        let dir = node_dir(&self.local_root, &self.node);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Ok(s) = serde_json::to_string(&self.own_snapshot) {
            let _ = std::fs::write(snapshot_path(&self.local_root, &self.node), s);
        }
        let _ = std::fs::write(
            segment_path(&self.local_root, &self.node),
            serialize_segment(&self.own_tail),
        );
        self.persist_clock();
    }

    /// Persist the HLC clock node-local (never shared).
    fn persist_clock(&self) {
        let dir = node_dir(&self.local_root, &self.node);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Ok(s) = serde_json::to_string(&self.clock) {
            let _ = std::fs::write(clock_path(&self.local_root, &self.node), s);
        }
    }

    /// Mirror this node's own store into the shared Syncthing folder so peers can
    /// replay it. A no-op (returns false) while the share is down — the offline
    /// backlog stays pending until it reappears (lock Q91). NEVER writes into a
    /// bare unprovisioned canonical mount (AUDIT-MESH-15).
    fn mirror_to_share(&mut self) -> bool {
        if !self.share_writable() {
            return false;
        }
        self.persist_own_local();
        let dst = node_dir(&self.share_root, &self.node);
        if std::fs::create_dir_all(&dst).is_err() {
            return false;
        }
        let snap_ok = std::fs::copy(
            snapshot_path(&self.local_root, &self.node),
            snapshot_path(&self.share_root, &self.node),
        )
        .is_ok();
        let seg_ok = std::fs::copy(
            segment_path(&self.local_root, &self.node),
            segment_path(&self.share_root, &self.node),
        )
        .is_ok();
        if snap_ok && seg_ok {
            self.pending = 0;
            self.last_mirror_ms = Some(self.now_ms());
            true
        } else {
            false
        }
    }

    /// Rebuild the in-memory index: fold this node's own `snapshot ⊕ tail` with
    /// every peer's on-disk contribution (replay-merge, locks Q19/Q22). Also
    /// observes peers' clocks so a later local op never sorts before a seen peer
    /// stamp (HLC receive step, lock Q5).
    fn rebuild_index(&mut self) {
        let mut idx = self.own_snapshot.clone();
        idx.apply_all(&self.own_tail);
        let now = self.now_ms();
        let mut peers = 0usize;
        let dir = bookmarks_dir(&self.share_root);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let Some(node) = name.to_str() else {
                    continue;
                };
                if node == self.node {
                    continue;
                }
                let (peer, max_hlc) = read_node_state(&path);
                idx.merge(&peer);
                if let Some(h) = max_hlc {
                    let _ = self.clock.observe(&h, now);
                }
                peers += 1;
            }
        }
        self.peer_count = peers;
        self.index = idx;
    }

    /// One convergence pass: mirror own state out, then merge peers back in.
    /// Split from [`Self::flush`] so tests exercise convergence without a Bus.
    fn sync(&mut self) {
        let _ = self.mirror_to_share();
        self.rebuild_index();
        self.last_flush_ms = self.now_ms();
    }

    /// Fold this node's tail into its own snapshot (superseded LWW writes
    /// collapse) and truncate the tail — bounded growth (lock Q20). Then persist
    /// + re-mirror the compacted store.
    fn snapshot_prune(&mut self) {
        if self.own_tail.is_empty() {
            return;
        }
        let tail = std::mem::take(&mut self.own_tail);
        self.own_snapshot.apply_all(&tail);
        self.persist_own_local();
        let _ = self.mirror_to_share();
    }

    /// Publish the converged collection snapshot + the sync status.
    fn publish_state(&self, persist: &Persist) {
        if let Ok(body) = serde_json::to_string(&self.index) {
            if let Err(e) = persist.write(STATE_COLLECTION, Priority::Default, None, Some(&body)) {
                tracing::warn!(target: "mackesd::bookmarks", error = %e, "collection publish failed");
            }
        }
        if let Ok(body) = serde_json::to_string(&self.status()) {
            if let Err(e) = persist.write(STATE_SYNC, Priority::Default, None, Some(&body)) {
                tracing::warn!(target: "mackesd::bookmarks", error = %e, "sync publish failed");
            }
        }
    }

    /// Build and run one bounded dead-link check over the current converged
    /// collection. This never mints bookmark ops: link health is observational
    /// state and can change independently of the synced bookmark tree.
    fn run_link_check(&self, request: &LinkCheckRequest) -> LinkCheckStatus {
        let mut candidates = Vec::new();
        if request.ids.is_empty() {
            for item in self.index.items() {
                if let Item::Bookmark(b) = item {
                    candidates.push((b.id, b.url, b.title));
                }
            }
        } else {
            for id in &request.ids {
                if let Some(Item::Bookmark(b)) = self.index.item(*id) {
                    candidates.push((b.id, b.url, b.title));
                }
            }
        }
        let truncated = candidates.len() > request.limit;
        let records = candidates
            .into_iter()
            .take(request.limit)
            .map(|(id, url, title)| {
                let outcome = if is_http_url(&url) {
                    (self.link_probe)(&url)
                } else {
                    LinkProbeOutcome::new(
                        LinkHealth::Unsupported,
                        None,
                        "only http:// and https:// bookmark URLs are checked",
                    )
                };
                LinkCheckRecord {
                    id,
                    url,
                    title,
                    health: outcome.health,
                    http_status: outcome.http_status,
                    detail: outcome.detail,
                }
            })
            .collect::<Vec<_>>();
        let alive = records
            .iter()
            .filter(|r| r.health == LinkHealth::Alive)
            .count();
        let dead = records
            .iter()
            .filter(|r| r.health == LinkHealth::Dead)
            .count();
        let unsupported = records
            .iter()
            .filter(|r| r.health == LinkHealth::Unsupported)
            .count();
        let errors = records
            .iter()
            .filter(|r| r.health == LinkHealth::Error)
            .count();
        LinkCheckStatus {
            op: "bookmarks_link_check".to_string(),
            node: self.node.clone(),
            checked_at_ms: self.now_ms(),
            checked: records.len(),
            alive,
            dead,
            unsupported,
            errors,
            truncated,
            records,
        }
    }

    /// Publish the latest dead-link check as both retained state and an event.
    fn publish_link_check(&self, persist: &Persist, status: &LinkCheckStatus) {
        let Ok(body) = serde_json::to_string(status) else {
            return;
        };
        if let Err(e) = persist.write(STATE_LINK_CHECK, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::bookmarks", error = %e, "link-check state publish failed");
        }
        if let Err(e) = persist.write(EVENT_LINK_CHECK, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::bookmarks", error = %e, "link-check event publish failed");
        }
    }

    /// Flush = one sync pass + publish (the tick body's convergence half).
    fn flush(&mut self, persist: &Persist) {
        self.sync();
        self.publish_state(persist);
    }

    /// Drain net-new requests across every `action/bookmarks/<verb>` topic,
    /// applying each typed action locally. Publishes immediately when any edit
    /// landed so the surface reflects a local edit without waiting for the flush.
    fn drain_requests(&mut self, persist: &Persist) {
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::bookmarks", error = %e, "list_topics failed");
                return;
            }
        };
        let mut changed = false;
        for topic in topics
            .into_iter()
            .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
        {
            let verb = topic[ACTION_PREFIX.len()..].to_string();
            let cursor = self.cursors.get(&topic).cloned();
            let msgs = match persist.list_since(&topic, cursor.as_deref()) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(target: "mackesd::bookmarks", topic, error = %e, "list_since failed");
                    continue;
                }
            };
            for msg in msgs {
                self.cursors.insert(topic.clone(), msg.ulid.clone());
                if verb == CHECK_LINKS_VERB {
                    match parse_link_check_request(msg.body.as_deref().unwrap_or_default()) {
                        Ok(request) => {
                            let status = self.run_link_check(&request);
                            self.publish_link_check(persist, &status);
                        }
                        Err(e) => {
                            tracing::warn!(target: "mackesd::bookmarks", error = %e, "bad link-check request");
                        }
                    }
                    continue;
                }
                match parse_action(&verb, msg.body.as_deref().unwrap_or_default()) {
                    Ok(action) => {
                        self.apply_action(action);
                        changed = true;
                    }
                    Err(e) => {
                        tracing::warn!(target: "mackesd::bookmarks", verb = %verb, error = %e, "bad request");
                    }
                }
            }
        }
        if changed {
            self.publish_state(persist);
        }
    }

    /// Seed each request topic's cursor at its tail so a restart doesn't replay +
    /// re-apply already-processed requests (the ops are already in the store).
    fn seed_cursors(&mut self, persist: &Persist) {
        if let Ok(topics) = persist.list_topics() {
            for topic in topics
                .into_iter()
                .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
            {
                if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                    self.cursors.insert(topic, ulid);
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for BookmarksWorker {
    fn name(&self) -> &'static str {
        "bookmarks"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::bookmarks", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::bookmarks", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        self.load();
        self.seed_cursors(&persist);
        self.flush(&persist); // publish the initial converged state
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.drain_requests(&persist);
                    self.flush(&persist);
                    self.prune_counter = self.prune_counter.saturating_add(1);
                    if self.prune_counter >= self.prune_every
                        || self.own_tail.len() >= self.tail_threshold
                    {
                        self.prune_counter = 0;
                        self.snapshot_prune();
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        // Clean shutdown: persist the authoritative own store + a final mirror so
        // a restart resumes exactly, and peers get this session's tail.
        self.persist_own_local();
        let _ = self.mirror_to_share();
        Ok(())
    }
}

/// Resolve the node-local durable bookmarks root
/// (`<XDG_DATA_HOME>/mde/bookmarks`, or `/var/lib/mde/bookmarks` headless).
///
/// Kept node-local so offline edits + the HLC clock survive a restart regardless
/// of the shared mount.
#[must_use]
pub fn resolve_local_root() -> PathBuf {
    dirs::data_dir().map_or_else(
        || PathBuf::from("/var/lib/mde/bookmarks"),
        |d| d.join("mde").join("bookmarks"),
    )
}

/// Resolve the local authenticated user ops are attributed to (lock Q64).
///
/// `$MDE_MESH_USER` (the shell's explicit override) → `$USER`/`$LOGNAME` → a
/// stable `operator` fallback. The worker only ever stamps ops for this user.
#[must_use]
pub fn resolve_user() -> String {
    for key in ["MDE_MESH_USER", "USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }
    }
    "operator".to_string()
}

/// Wall-clock epoch millis (the production [`NowFn`]).
fn default_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    // ── a deterministic fake wall clock shared across nodes ─────────────────

    fn fake_clock(start: u64) -> (Arc<AtomicU64>, NowFn) {
        let cell = Arc::new(AtomicU64::new(start));
        let reader = cell.clone();
        let now: NowFn = Arc::new(move || reader.load(Ordering::SeqCst));
        (cell, now)
    }

    fn worker(node: &str, user: &str, local: &Path, share: &Path, now: NowFn) -> BookmarksWorker {
        BookmarksWorker::new(
            node.to_string(),
            user.to_string(),
            local.to_path_buf(),
            share.to_path_buf(),
        )
        .with_now_fn(now)
    }

    fn add(url: &str, title: &str) -> BookmarkAction {
        BookmarkAction::Add {
            id: None,
            parent: None,
            order_key: None,
            url: url.to_string(),
            title: title.to_string(),
            tags: vec![],
            notes: String::new(),
            source: None,
        }
    }

    fn test_link_probe() -> LinkProbeFn {
        Arc::new(|url| {
            if url.contains("alive") {
                LinkProbeOutcome::new(LinkHealth::Alive, Some(204), "http 204")
            } else if url.contains("dead") {
                LinkProbeOutcome::new(LinkHealth::Dead, Some(404), "http 404")
            } else {
                LinkProbeOutcome::new(LinkHealth::Error, None, "connection refused")
            }
        })
    }

    fn title_of(coll: &Collection, id: Uuid) -> Option<String> {
        match coll.item(id)? {
            mde_bookmarks::Item::Bookmark(b) => Some(b.title),
            mde_bookmarks::Item::Folder(_) => None,
        }
    }

    fn find_by_title(coll: &Collection, title: &str) -> Option<Uuid> {
        coll.items().into_iter().find_map(|it| match it {
            mde_bookmarks::Item::Bookmark(b) if b.title == title => Some(b.id),
            _ => None,
        })
    }

    // ── request parsing ─────────────────────────────────────────────────────

    #[test]
    fn parse_action_covers_every_verb() {
        let id = Uuid::from_u128(1);
        let supplied = Uuid::from_u128(2);
        let idj = id.to_string();
        let supplied_j = supplied.to_string();
        match parse_action(
            "add",
            &format!(r#"{{"id":"{supplied_j}","url":"https://x","title":"X","order_key":"m"}}"#),
        )
        .unwrap()
        {
            BookmarkAction::Add { id, order_key, .. } => {
                assert_eq!(id, Some(supplied));
                assert_eq!(order_key.as_deref(), Some("m"));
            }
            other => panic!("unexpected action: {other:?}"),
        }
        assert!(matches!(
            parse_action("edit", &format!(r#"{{"id":"{idj}","title":"Y"}}"#)).unwrap(),
            BookmarkAction::Edit { .. }
        ));
        match parse_action("move", &format!(r#"{{"id":"{idj}","order_key":"n"}}"#)).unwrap() {
            BookmarkAction::Move { order_key, .. } => assert_eq!(order_key.as_deref(), Some("n")),
            other => panic!("unexpected action: {other:?}"),
        }
        assert!(matches!(
            parse_action("delete", &format!(r#"{{"id":"{idj}"}}"#)).unwrap(),
            BookmarkAction::Delete { .. }
        ));
        match parse_action(
            "add-folder",
            &format!(r#"{{"id":"{supplied_j}","name":"F","order_key":"p"}}"#),
        )
        .unwrap()
        {
            BookmarkAction::AddFolder { id, order_key, .. } => {
                assert_eq!(id, Some(supplied));
                assert_eq!(order_key.as_deref(), Some("p"));
            }
            other => panic!("unexpected action: {other:?}"),
        }
        assert!(matches!(
            parse_action("add_folder", r#"{"name":"F"}"#).unwrap(),
            BookmarkAction::AddFolder { .. }
        ));
        assert!(matches!(
            parse_action("rename", &format!(r#"{{"id":"{idj}","name":"G"}}"#)).unwrap(),
            BookmarkAction::Rename { .. }
        ));
    }

    #[test]
    fn parse_action_rejects_unknown_verb_and_missing_field() {
        assert!(parse_action("frobnicate", "{}").is_err());
        // `edit` without an id is a typed error, never a silent no-op.
        assert!(parse_action("edit", r#"{"title":"x"}"#).is_err());
        // `add` without a url is malformed.
        assert!(parse_action("add", "{}").is_err());
    }

    #[test]
    fn parse_link_check_request_defaults_and_clamps_the_limit() {
        let id = Uuid::from_u128(42);
        let req = parse_link_check_request("{}").unwrap();
        assert!(req.ids.is_empty());
        assert_eq!(req.limit, MAX_LINK_CHECKS);

        let req = parse_link_check_request(&format!(r#"{{"ids":["{id}"],"limit":9999}}"#)).unwrap();
        assert_eq!(req.ids, vec![id]);
        assert_eq!(req.limit, MAX_LINK_CHECKS);

        let req = parse_link_check_request(r#"{"limit":0}"#).unwrap();
        assert_eq!(req.limit, 1);
        assert!(parse_link_check_request("{").is_err());
    }

    #[test]
    fn link_check_classifies_alive_dead_unsupported_and_errors() {
        let (_c, now) = fake_clock(7000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let mut w =
            worker("n1", "u", local.path(), share.path(), now).with_link_probe(test_link_probe());
        w.apply_action(add("https://alive.example/", "Alive"));
        w.apply_action(add("https://dead.example/", "Dead"));
        w.apply_action(add("gopher://old.example/", "Old"));
        w.apply_action(add("https://error.example/", "Error"));

        let status = w.run_link_check(&parse_link_check_request("{}").unwrap());
        assert_eq!(status.op, "bookmarks_link_check");
        assert_eq!(status.node, "n1");
        assert_eq!(status.checked_at_ms, 7000);
        assert_eq!(status.checked, 4);
        assert_eq!(status.alive, 1);
        assert_eq!(status.dead, 1);
        assert_eq!(status.unsupported, 1);
        assert_eq!(status.errors, 1);
        assert!(!status.truncated);
        assert!(status
            .records
            .iter()
            .any(|r| r.title == "Dead" && r.http_status == Some(404)));
        assert!(status
            .records
            .iter()
            .any(|r| r.title == "Old" && r.health == LinkHealth::Unsupported));
    }

    #[test]
    fn client_supplied_ids_and_order_keys_are_preserved() {
        let (_c, now) = fake_clock(1000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let mut w = worker("n1", "u", local.path(), share.path(), now);
        let folder = Uuid::from_u128(0xabc1);
        let bookmark = Uuid::from_u128(0xabc2);

        w.apply_action(BookmarkAction::AddFolder {
            id: Some(folder),
            parent: None,
            order_key: Some("folder-key".into()),
            name: "Client Folder".into(),
        });
        w.apply_action(BookmarkAction::Add {
            id: Some(bookmark),
            parent: Some(folder),
            order_key: Some("bookmark-key".into()),
            url: "https://client.example".into(),
            title: "Client Bookmark".into(),
            tags: vec![],
            notes: String::new(),
            source: None,
        });

        let mde_bookmarks::Item::Folder(f) = w.collection().item(folder).expect("folder") else {
            panic!("expected folder");
        };
        assert_eq!(f.order_key, "folder-key");
        let mde_bookmarks::Item::Bookmark(b) = w.collection().item(bookmark).expect("bookmark")
        else {
            panic!("expected bookmark");
        };
        assert_eq!(b.parent, Some(folder));
        assert_eq!(b.order_key, "bookmark-key");

        w.apply_action(BookmarkAction::Move {
            id: bookmark,
            parent: None,
            order_key: Some("moved-key".into()),
            before: None,
            after: None,
        });
        let mde_bookmarks::Item::Bookmark(b) = w.collection().item(bookmark).expect("bookmark")
        else {
            panic!("expected bookmark");
        };
        assert_eq!(b.parent, None);
        assert_eq!(b.order_key, "moved-key");
    }

    #[test]
    fn link_check_request_publishes_status_without_minting_bookmark_ops() {
        let (_c, now) = fake_clock(9000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut w =
            worker("n1", "u", local.path(), share.path(), now).with_link_probe(test_link_probe());
        w.apply_action(add("https://alive.example/", "Alive"));
        w.apply_action(add("https://dead.example/", "Dead"));
        let alive = find_by_title(w.collection(), "Alive").unwrap();
        let dead = find_by_title(w.collection(), "Dead").unwrap();
        let tail_before = w.own_tail.len();

        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let body = format!(r#"{{"ids":["{alive}","{dead}"],"limit":1}}"#);
        persist
            .write(
                &format!("{ACTION_PREFIX}{CHECK_LINKS_VERB}"),
                Priority::Default,
                None,
                Some(&body),
            )
            .unwrap();
        w.drain_requests(&persist);

        assert_eq!(
            w.own_tail.len(),
            tail_before,
            "link checks publish operational state, not bookmark CRDT ops"
        );
        let state = persist.list_since(STATE_LINK_CHECK, None).unwrap();
        assert_eq!(state.len(), 1);
        let body = state[0].body.as_deref().unwrap();
        let status: LinkCheckStatus = serde_json::from_str(body).unwrap();
        assert_eq!(status.checked, 1);
        assert!(status.truncated);
        assert_eq!(status.alive, 1);
        let events = persist.list_since(EVENT_LINK_CHECK, None).unwrap();
        assert_eq!(events.len(), 1);
    }

    // ── segment (de)serialization is lossless + corruption-tolerant ─────────

    #[test]
    fn segment_round_trips_and_skips_corrupt_lines() {
        let (_c, now) = fake_clock(100);
        let dir = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let mut w = worker("n1", "u", dir.path(), share.path(), now);
        w.apply_action(add("https://a", "A"));
        w.apply_action(add("https://b", "B"));
        let text = serialize_segment(&w.own_tail);
        // A corrupt line in the middle is dropped, the good ops still parse.
        let poisoned = format!("{{ not json\n{text}");
        let ops = parse_segment(&poisoned);
        assert_eq!(ops.len(), 2);
    }

    // ── offline-first: edits apply with no share / no flush ─────────────────

    #[test]
    fn edits_apply_immediately_offline_and_survive_restart() {
        let (_c, now) = fake_clock(1000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let gate = Arc::new(AtomicBool::new(false)); // share DOWN
        let mut w = worker("n1", "alice", local.path(), share.path(), now.clone())
            .with_share_gate(gate.clone());
        w.apply_action(add("https://a", "A"));
        w.apply_action(add("https://b", "B"));
        // Applied to the live index without any flush/share.
        assert_eq!(w.collection().len(), 2);
        // Offline: a sync is a no-op to the share; the backlog is pending.
        w.sync();
        assert!(!w.status().syncing);
        assert_eq!(w.status().pending_local_ops, 2);
        assert!(!share.path().join("bookmarks/n1").exists());

        // Restart: a fresh worker over the same local root replays the store.
        let mut w2 =
            worker("n1", "alice", local.path(), share.path(), now).with_share_gate(gate.clone());
        w2.load();
        assert_eq!(w2.collection().len(), 2, "local store survives restart");

        // Reconnect: the share comes up, the next sync mirrors the backlog out.
        gate.store(true, Ordering::SeqCst);
        w2.sync();
        assert!(w2.status().syncing);
        assert_eq!(w2.status().pending_local_ops, 0);
        assert!(share
            .path()
            .join("bookmarks/n1")
            .join(SEGMENT_FILE)
            .exists());
    }

    // ── the crux: two-node convergence via replay-merge ─────────────────────

    #[test]
    fn two_nodes_converge_after_replay_merge() {
        // One shared fake Syncthing folder; two nodes each with their own local
        // store; one deterministic wall clock shared by both (equal wall times
        // exercise the HLC node-id tiebreak).
        let (clk, now) = fake_clock(1000);
        let share = tempfile::tempdir().unwrap();
        let la = tempfile::tempdir().unwrap();
        let lb = tempfile::tempdir().unwrap();
        let mut a = worker("A", "alice", la.path(), share.path(), now.clone());
        let mut b = worker("B", "bob", lb.path(), share.path(), now.clone());

        // Round 1 — concurrent adds on each node.
        a.apply_action(add("https://a1", "A1"));
        b.apply_action(add("https://b1", "B1"));
        // Converge: each mirrors then reads the other (idempotent, so a couple of
        // interleaved passes settle it).
        a.sync();
        b.sync();
        a.sync();
        b.sync();
        assert_eq!(
            a.collection(),
            b.collection(),
            "both nodes see the union after replay-merge"
        );
        assert_eq!(a.collection().len(), 2);

        // Round 2 — a CONCURRENT edit of the SAME item at equal wall time, which
        // must resolve deterministically by the HLC node-id tiebreak.
        let a1 = find_by_title(a.collection(), "A1").expect("A1 present on both");
        clk.store(2000, Ordering::SeqCst);
        a.apply_action(BookmarkAction::Edit {
            id: a1,
            url: None,
            title: Some("from-A".into()),
            tags: None,
            notes: None,
        });
        b.apply_action(BookmarkAction::Edit {
            id: a1,
            url: None,
            title: Some("from-B".into()),
            tags: None,
            notes: None,
        });
        a.sync();
        b.sync();
        a.sync();
        b.sync();
        assert_eq!(
            a.collection(),
            b.collection(),
            "concurrent edits converge to identical collections"
        );
        // node "B" > "A" at equal (wall, counter) → B's write wins on both.
        assert_eq!(title_of(a.collection(), a1).as_deref(), Some("from-B"));
        assert_eq!(title_of(b.collection(), a1).as_deref(), Some("from-B"));
    }

    // ── snapshot/prune bounds growth without breaking convergence ───────────

    #[test]
    fn snapshot_prune_bounds_the_tail_and_still_converges() {
        let (_c, now) = fake_clock(1000);
        let share = tempfile::tempdir().unwrap();
        let la = tempfile::tempdir().unwrap();
        let lb = tempfile::tempdir().unwrap();
        let mut a = worker("A", "alice", la.path(), share.path(), now.clone());
        let mut b = worker("B", "bob", lb.path(), share.path(), now.clone());

        for i in 0..10 {
            a.apply_action(add(&format!("https://a/{i}"), &format!("A{i}")));
        }
        let before = a.collection().clone();
        assert_eq!(a.own_tail.len(), 10);
        a.snapshot_prune();
        // The tail folded into the snapshot; the converged view is unchanged.
        assert!(a.own_tail.is_empty(), "tail pruned");
        assert_eq!(
            &before,
            a.collection(),
            "prune preserves the converged state"
        );

        // A fresh peer converges by replaying A's snapshot ⊕ (empty) tail.
        a.sync();
        b.sync();
        a.sync();
        assert_eq!(a.collection(), b.collection());
        assert_eq!(b.collection().len(), 10);
    }

    // ── folders + fractional order + move ───────────────────────────────────

    #[test]
    fn add_folder_move_and_reorder_mint_real_ops() {
        let (_c, now) = fake_clock(1000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let mut w = worker("n1", "u", local.path(), share.path(), now);
        w.apply_action(BookmarkAction::AddFolder {
            id: None,
            parent: None,
            order_key: None,
            name: "Imported".into(),
        });
        let folder = w
            .collection()
            .items()
            .into_iter()
            .find_map(|it| match it {
                mde_bookmarks::Item::Folder(f) => Some(f.id),
                mde_bookmarks::Item::Bookmark(_) => None,
            })
            .expect("folder created");
        // Two bookmarks at top level, then move one into the folder.
        w.apply_action(add("https://x", "X"));
        w.apply_action(add("https://y", "Y"));
        let x = find_by_title(w.collection(), "X").unwrap();
        w.apply_action(BookmarkAction::Move {
            id: x,
            parent: Some(folder),
            order_key: None,
            before: None,
            after: None,
        });
        let kids = w.collection().children(Some(folder));
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].id(), x);
        // Sibling order keys are distinct + strictly ordered (no renumber storm).
        w.apply_action(add("https://z1", "Z1"));
        w.apply_action(add("https://z2", "Z2"));
        let tops = w.collection().children(None);
        for pair in tops.windows(2) {
            assert!(pair[0].order_key() < pair[1].order_key());
        }
    }

    #[test]
    fn worker_name_is_locked() {
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let (_c, now) = fake_clock(0);
        let w = worker("n1", "u", local.path(), share.path(), now);
        assert_eq!(w.name(), "bookmarks");
    }
}

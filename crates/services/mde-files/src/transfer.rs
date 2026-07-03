//! FILEMGR-7 — direct peer-to-peer transfer (routing + the queued transfer).
//!
//! Design: `docs/design/file-manager-full.md` (lock 16). A file copied from
//! node **A** to node **B** must move **directly** A→B and NOT double-hop
//! through the browsing node **C** (which mounted both over sshfs). This module
//! is the render-agnostic engine that decides the route and drives it:
//!
//! * **Endpoint classification** ([`classify_endpoint`]) — a path under the
//!   FILEMGR-5 mesh-mount tree (`<runtime>/mde-mesh/<host>/…`) is a *peer*
//!   endpoint (host + mount-relative path); anything else is *local*.
//! * **The routing decision** ([`route_transfer`]) — the load-bearing fold the
//!   acceptance criteria pin: **two distinct peers ⇒ [`TransferRoute::Direct`]**
//!   (peer-side helper: A rsyncs straight to B over the overlay); a local end,
//!   or the same peer on both ends, ⇒ **[`TransferRoute::Relay`]** (the plain
//!   FILEMGR-2 [`crate::opqueue`] copy over the two sshfs mounts — the honest
//!   "sshfs-relay fallback when no direct path").
//! * **The queued transfer** — the direct leg reuses the FILEMGR-2
//!   [`Progress`] shape via [`DirectProgress`] so a direct A→B transfer renders
//!   as *one queued transfer with real progress*, identical to a relay/local op
//!   in the op-queue panel. The rsync `--info=progress2` line parser
//!   ([`parse_progress2_line`]) folds live byte/file counts into that shape.
//!
//! ## §7 — the live peer leg is honestly gated, never faked
//!
//! The routing + plan + progress folds are **pure** and unit-tested here. The
//! one leg that genuinely needs a running mesh — dispatching the direct request
//! to the mackesd peer-side helper (`action/mesh-transfer/direct`) and having it
//! rsync A→B over the overlay — is behind the `dbus` feature and returns a typed
//! [`TransferError`]; a [`TransferError::Gated`] / [`TransferError::Unavailable`]
//! is the honest "no direct path" that [`TransferError::should_relay`] routes to
//! the sshfs relay fallback (mirroring FILEMGR-5's mesh_mount typed `Gated`).
//! It NEVER fabricates a completed transfer.
//!
//! The desktop surface (FILEMGR-9/12, `mde-files-egui`) owns the drag-onto-node
//! wiring; it calls the clean API here: [`MeshLayout::classify`] +
//! [`route_transfer`] to choose the path, [`relay_op`] to build the queue op for
//! the relay/local items, and [`DirectRequest`] to dispatch the direct ones.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::backend::OpId;
use crate::fileops::FileOps;
use crate::opqueue::{OpKind, Progress};

/// The stable mesh-mount tree segment under the desktop runtime dir. MUST match
/// the mackesd `mesh_mount` worker's mountpoint layout
/// (`<runtime>/mde-mesh/<host>`) so a mounted peer path classifies correctly.
pub const MESH_MOUNT_DIR: &str = "mde-mesh";

// ═══════════════════════════════════════════════════════════════════════════
// Endpoint classification.
// ═══════════════════════════════════════════════════════════════════════════

/// One end of a transfer, resolved from a filesystem path the surface handed us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A plain local path (this node's own filesystem, or the mesh-browse root
    /// itself which isn't a peer mount).
    Local(PathBuf),
    /// A path inside peer `host`'s sshfs mount, with the path **relative to that
    /// peer's mountpoint** (`""` = the mount root). The remote absolute path is
    /// resolved mesh-side by the peer helper (which knows the mount's scope).
    Peer {
        /// Short peer hostname (the roster key + `<host>.mesh` overlay name).
        host: String,
        /// Path relative to the peer's mountpoint.
        mount_rel: PathBuf,
    },
}

impl Endpoint {
    /// The peer hostname when this endpoint is a mounted peer path.
    #[must_use]
    pub fn peer_host(&self) -> Option<&str> {
        match self {
            Endpoint::Peer { host, .. } => Some(host),
            Endpoint::Local(_) => None,
        }
    }
}

/// Classify `path` against the mesh-mount root `mesh_root`
/// (`<runtime>/mde-mesh`). A path of the shape `<mesh_root>/<host>[/<rest>]` is a
/// [`Endpoint::Peer`]; `mesh_root` itself (the browse root, no host segment) and
/// everything else is [`Endpoint::Local`]. Pure — the load-bearing parse the
/// routing decision rides on.
#[must_use]
pub fn classify_endpoint(path: &Path, mesh_root: &Path) -> Endpoint {
    if let Ok(rel) = path.strip_prefix(mesh_root) {
        let mut comps = rel.components();
        // The first component after the mesh root is the peer host; a `Normal`
        // component excludes `.`/`..`/root, so a crafted `mde-mesh/../etc` never
        // masquerades as a peer.
        if let Some(Component::Normal(host)) = comps.next() {
            return Endpoint::Peer {
                host: host.to_string_lossy().into_owned(),
                mount_rel: comps.as_path().to_path_buf(),
            };
        }
    }
    Endpoint::Local(path.to_path_buf())
}

/// Knows the sshfs mount root so it can classify paths + name a peer's
/// mountpoint. Constructed once from the desktop runtime base
/// (`XDG_RUNTIME_DIR`, `/run/user/<uid>`).
#[derive(Debug, Clone)]
pub struct MeshLayout {
    mesh_root: PathBuf,
}

impl MeshLayout {
    /// Build the layout for a desktop runtime base (`/run/user/<uid>`).
    #[must_use]
    pub fn new(runtime_base: &Path) -> Self {
        Self {
            mesh_root: runtime_base.join(MESH_MOUNT_DIR),
        }
    }

    /// The mesh-mount root (`<runtime>/mde-mesh`).
    #[must_use]
    pub fn mesh_root(&self) -> &Path {
        &self.mesh_root
    }

    /// The stable mountpoint for `host` (`<mesh_root>/<host>`) — the path the
    /// surface uses as a drag-onto-node destination directory.
    #[must_use]
    pub fn mountpoint(&self, host: &str) -> PathBuf {
        self.mesh_root.join(host)
    }

    /// Classify a path into a [`Endpoint`].
    #[must_use]
    pub fn classify(&self, path: &Path) -> Endpoint {
        classify_endpoint(path, &self.mesh_root)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The routing decision.
// ═══════════════════════════════════════════════════════════════════════════

/// Copy vs move — the transfer mode carried through the route + into the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    /// Copy the source, leaving it in place.
    Copy,
    /// Move the source (copy-then-remove — the peer helper removes the source
    /// only after the copy fully succeeds, so a failure never loses data).
    Move,
}

impl TransferMode {
    /// Stable wire tag for the direct-request body + logs.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
        }
    }
}

/// Why a transfer falls back to the sshfs relay (the plain [`crate::opqueue`]
/// copy over the local mounts) instead of the direct A→B peer path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayReason {
    /// At least one end is a local (non-mesh) path — a straight sshfs read/write
    /// through this node is the only path (local↔node).
    LocalEndpoint,
    /// Both ends are on the SAME peer — no cross-node hop; a copy within one
    /// mount is a straight sshfs op (relaying A→A directly would be pointless).
    SamePeer,
}

impl RelayReason {
    /// Stable tag for logs + the transfer panel's "why relayed" hint.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::LocalEndpoint => "local-endpoint",
            Self::SamePeer => "same-peer",
        }
    }
}

/// A resolved direct A→B transfer: distinct source + destination peers, each
/// with a mount-relative path. The peer helper (mackesd) resolves each host's
/// remote absolute path from its live mount scope and rsyncs A→B over the
/// overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTransfer {
    /// Source peer host.
    pub src_host: String,
    /// Source item path, relative to the source peer's mountpoint.
    pub src_rel: PathBuf,
    /// Destination peer host.
    pub dst_host: String,
    /// Destination **directory** path, relative to the dest peer's mountpoint
    /// (the source item's basename lands inside it, the "paste here" shape).
    pub dst_rel: PathBuf,
    /// Copy vs move.
    pub mode: TransferMode,
}

/// The chosen route for one source → destination-directory transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferRoute {
    /// Direct peer-to-peer (A rsyncs straight to B).
    Direct(DirectTransfer),
    /// The sshfs relay fallback — the surface runs a normal [`crate::opqueue`]
    /// copy/move over the mounted paths.
    Relay(RelayReason),
}

impl TransferRoute {
    /// `true` for the direct peer-to-peer path.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        matches!(self, TransferRoute::Direct(_))
    }
}

/// Decide the route for copying/moving `src` **into** the directory `dst_dir`.
///
/// The rule (lock 16): a transfer whose two ends are **distinct mesh peers**
/// goes [`TransferRoute::Direct`]; every other shape — a local end on either
/// side, or the same peer on both ends — is a [`TransferRoute::Relay`]. Pure +
/// exhaustively unit-tested; this is the decision the acceptance criteria pin.
#[must_use]
pub fn route_transfer(src: &Endpoint, dst_dir: &Endpoint, mode: TransferMode) -> TransferRoute {
    match (src, dst_dir) {
        (
            Endpoint::Peer {
                host: src_host,
                mount_rel: src_rel,
            },
            Endpoint::Peer {
                host: dst_host,
                mount_rel: dst_rel,
            },
        ) => {
            if src_host == dst_host {
                // Same node: a copy within one mount, no cross-node hop.
                TransferRoute::Relay(RelayReason::SamePeer)
            } else {
                TransferRoute::Direct(DirectTransfer {
                    src_host: src_host.clone(),
                    src_rel: src_rel.clone(),
                    dst_host: dst_host.clone(),
                    dst_rel: dst_rel.clone(),
                    mode,
                })
            }
        }
        // A local end on either side: only the sshfs read/write through us works.
        _ => TransferRoute::Relay(RelayReason::LocalEndpoint),
    }
}

/// Build the FILEMGR-2 [`OpKind`] for the relay (or local) leg — the surface
/// submits this to its existing [`crate::opqueue::OpQueue`] so a relayed copy
/// gets the full progress/pause/cancel/conflict machinery for free.
#[must_use]
pub fn relay_op(items: Vec<PathBuf>, dst_dir: PathBuf, mode: TransferMode) -> OpKind {
    match mode {
        TransferMode::Copy => OpKind::Copy {
            items,
            dest_dir: dst_dir,
        },
        TransferMode::Move => OpKind::Move {
            items,
            dest_dir: dst_dir,
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The direct-transfer wire request (dispatched to the mackesd peer helper).
// ═══════════════════════════════════════════════════════════════════════════

/// The typed body of an `action/mesh-transfer/direct` request. The mackesd
/// peer-side helper resolves each host's remote absolute path from its live
/// mount scope and rsyncs A→B over the overlay (there is deliberately no
/// command/shell field — §9: only the two hosts, their mount-relative paths, and
/// the mode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectRequest {
    /// The queue id the surface assigned this transfer (echoed in progress).
    pub op_id: OpId,
    /// Source peer host.
    pub src_host: String,
    /// Source item, relative to the source mountpoint.
    pub src_rel: String,
    /// Destination peer host.
    pub dst_host: String,
    /// Destination directory, relative to the dest mountpoint.
    pub dst_rel: String,
    /// `"copy"` or `"move"`.
    pub mode: String,
}

impl DirectRequest {
    /// Build the wire request from a resolved [`DirectTransfer`] + a queue id.
    #[must_use]
    pub fn new(op_id: OpId, t: &DirectTransfer) -> Self {
        Self {
            op_id,
            src_host: t.src_host.clone(),
            src_rel: t.src_rel.to_string_lossy().into_owned(),
            dst_host: t.dst_host.clone(),
            dst_rel: t.dst_rel.to_string_lossy().into_owned(),
            mode: t.mode.tag().to_string(),
        }
    }

    /// Serialize to the JSON request body.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// The honest outcome of a completed direct transfer (parsed from the peer
/// helper's reply). Bytes/files are what rsync actually moved — never fabricated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectOutcome {
    /// Filesystem entries transferred.
    pub files: u64,
    /// Regular-file bytes transferred.
    pub bytes: u64,
}

/// A typed direct-transfer failure. `Unavailable`/`Gated` are the honest
/// "no direct path" states that fall back to the sshfs relay (lock 16);
/// `Failed` is a transfer that genuinely errored (surfaced, not relayed —
/// re-relaying a hard rsync error would just fail again, hiding the cause).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferError {
    /// The mackesd mesh-transfer responder isn't reachable (no daemon / no Bus /
    /// timed out). Fall back to the relay.
    Unavailable(String),
    /// The peer helper honestly refused: no `ssh`/`rsync` or no provisioned mesh
    /// key on the mesh side (§7 gate). Fall back to the relay.
    Gated(String),
    /// The direct transfer ran but failed (rsync/ssh error). Surfaced as-is.
    Failed(String),
}

impl TransferError {
    /// `true` when this failure should route to the sshfs relay fallback (the
    /// direct transport is simply unavailable here, not a genuine copy error).
    #[must_use]
    pub const fn should_relay(&self) -> bool {
        matches!(
            self,
            TransferError::Unavailable(_) | TransferError::Gated(_)
        )
    }
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "direct transport unavailable: {m}"),
            Self::Gated(m) => write!(f, "direct transfer gated: {m}"),
            Self::Failed(m) => write!(f, "direct transfer failed: {m}"),
        }
    }
}

impl std::error::Error for TransferError {}

/// Parse the peer helper's JSON reply into a typed outcome. `{"ok":true,…}` is a
/// [`DirectOutcome`]; `{"ok":false,"gated":true,…}` is [`TransferError::Gated`]
/// (relay fallback); any other `ok:false` is [`TransferError::Failed`]. Pure +
/// tested so the honest-gate → relay decision is exercised without a live mesh.
///
/// # Errors
/// A typed [`TransferError`] for any non-success reply (incl. an undecodable
/// body, which is treated as an unavailable transport).
pub fn parse_direct_reply(raw: &str) -> Result<DirectOutcome, TransferError> {
    let v: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| TransferError::Unavailable(format!("undecodable reply: {e}")))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(DirectOutcome {
            files: v
                .get("files")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            bytes: v
                .get("bytes")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        });
    }
    let msg = v
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("direct transfer rejected")
        .to_string();
    if v.get("gated").and_then(serde_json::Value::as_bool) == Some(true) {
        Err(TransferError::Gated(msg))
    } else {
        Err(TransferError::Failed(msg))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The queued transfer — folding live progress into the FILEMGR-2 shape.
// ═══════════════════════════════════════════════════════════════════════════

/// One live progress reading for a direct transfer: cumulative bytes + files the
/// peer helper has moved so far, and the entry in flight. The rsync
/// `--info=progress2` producer emits these; [`DirectProgress::apply`] folds them
/// into the FILEMGR-2 [`Progress`] shape.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransferTick {
    /// Cumulative regular-file bytes transferred so far.
    pub bytes_done: u64,
    /// Cumulative filesystem entries completed so far.
    pub files_done: u64,
    /// The entry currently in flight, when known.
    pub current: Option<PathBuf>,
}

/// Folds a direct transfer's live [`TransferTick`]s into the FILEMGR-2
/// [`Progress`] shape so a direct A→B transfer renders in the same op-queue
/// panel as a local/relay op — *one queued transfer with real progress*.
///
/// Totals are scanned from the (locally-mounted, hence stat-able) source before
/// the transfer starts, so the fraction/ETA are honest; the running counts are
/// monotonic (a re-ordered or duplicated tick never rewinds the bar) and clamped
/// to the totals.
#[derive(Debug, Clone)]
pub struct DirectProgress {
    op_id: OpId,
    files_total: u64,
    bytes_total: u64,
    files_done: u64,
    bytes_done: u64,
    current: Option<PathBuf>,
}

/// Scan a direct transfer's source items into `(files_total, bytes_total)` —
/// the same fold [`crate::opqueue`] uses for a local copy, so a direct
/// transfer's progress bar has honest totals. The mounted source is a local
/// path to `ops` (the sshfs worker did the mount), so this stats real sizes.
#[must_use]
pub fn scan_source_totals(ops: &dyn FileOps, items: &[PathBuf]) -> (u64, u64) {
    crate::opqueue::scan_items(ops, items)
}

impl DirectProgress {
    /// Start a direct-transfer progress fold with the pre-scanned source totals
    /// (see [`scan_source_totals`]).
    #[must_use]
    pub fn new(op_id: OpId, files_total: u64, bytes_total: u64) -> Self {
        Self {
            op_id,
            files_total,
            bytes_total,
            files_done: 0,
            bytes_done: 0,
            current: None,
        }
    }

    /// Fold one live reading, then return the current [`Progress`] snapshot.
    /// Counts are monotonic + clamped to the totals; `elapsed` is supplied by
    /// the caller (so the fold stays pure + testable).
    pub fn apply(&mut self, tick: &TransferTick, elapsed: Duration) -> Progress {
        self.bytes_done = self.bytes_done.max(tick.bytes_done).min(self.bytes_total);
        self.files_done = self.files_done.max(tick.files_done).min(self.files_total);
        self.current.clone_from(&tick.current);
        self.snapshot(elapsed)
    }

    /// Mark the transfer complete (every scanned entry/byte moved) + snapshot.
    /// Used to fold the peer helper's final honest byte/file count into a
    /// terminal 100% [`Progress`].
    pub fn complete(&mut self, outcome: &DirectOutcome, elapsed: Duration) -> Progress {
        // Trust the scanned totals for the bar, but never claim more moved than
        // the source held; the outcome's counts are the real transferred amount.
        self.bytes_done = outcome.bytes.min(self.bytes_total).max(self.bytes_done);
        self.files_done = outcome.files.min(self.files_total).max(self.files_done);
        // A wholly-successful transfer lands the bar at the totals.
        if outcome.files >= self.files_total {
            self.files_done = self.files_total;
            self.bytes_done = self.bytes_total;
        }
        self.current = None;
        self.snapshot(elapsed)
    }

    /// The current [`Progress`] snapshot (shares the exact shape a relay/local op
    /// emits, so the panel renders both identically).
    #[must_use]
    pub fn snapshot(&self, elapsed: Duration) -> Progress {
        Progress {
            op_id: self.op_id,
            files_total: self.files_total,
            files_done: self.files_done,
            files_skipped: 0,
            bytes_total: self.bytes_total,
            bytes_done: self.bytes_done,
            bytes_skipped: 0,
            current: self.current.clone(),
            elapsed,
        }
    }
}

/// Parse one rsync `--info=progress2` output line into a [`TransferTick`].
///
/// progress2 lines look like:
/// ```text
///    1,234,567  42%  1.05MB/s    0:00:03 (xfr#7, to-chk=12/40)
/// ```
/// The leading comma-grouped integer is cumulative bytes; `xfr#<n>` is the
/// count of files transferred so far. Returns `None` for a line without a
/// leading byte count (headers, blank lines) so a streaming parser can skip it.
/// Pure + tested — no rsync needed to exercise the fold.
#[must_use]
pub fn parse_progress2_line(line: &str) -> Option<TransferTick> {
    let trimmed = line.trim_start();
    // The first whitespace-delimited token is the cumulative byte count with
    // comma thousands-separators (rsync's locale-independent default grouping).
    let first = trimmed.split_whitespace().next()?;
    let digits: String = first.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || first.chars().any(|c| c != ',' && !c.is_ascii_digit()) {
        return None;
    }
    let bytes_done: u64 = digits.parse().ok()?;
    // `xfr#<n>` — files transferred so far (present on per-file progress lines).
    let files_done = line
        .split("xfr#")
        .nth(1)
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    Some(TransferTick {
        bytes_done,
        files_done,
        current: None,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// The live dispatch leg — honestly gated (behind `dbus`), never faked.
// ═══════════════════════════════════════════════════════════════════════════

/// The action topic the mackesd peer-side helper serves.
pub const DIRECT_ACTION_TOPIC: &str = "action/mesh-transfer/direct";

/// Bus client that dispatches a resolved [`DirectRequest`] to the mackesd
/// peer-side helper (`action/mesh-transfer/direct`) and folds its typed reply.
///
/// Mirrors [`crate::mesh_backend::MeshBackend`] / [`crate::bus_backend::BusBackend`]:
/// a tokio runtime + the resolved Bus data dir are held; each call opens a fresh
/// `Persist` (rusqlite isn't `Send`) inside `rt.block_on` and blocks the caller
/// until mackesd replies, bounded by a timeout so the GUI thread never freezes.
/// A missing daemon / Bus / responder surfaces as [`TransferError::Unavailable`]
/// — the honest "no direct path" the caller relays around.
#[cfg(feature = "dbus")]
pub struct TransferDispatch {
    rt: tokio::runtime::Runtime,
    bus_dir: PathBuf,
    call_timeout: Duration,
}

#[cfg(feature = "dbus")]
impl TransferDispatch {
    /// A direct A→B rsync can take a while; give the round-trip a generous
    /// ceiling (the peer helper streams no reply until it finishes).
    pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(600);

    /// Resolve the Bus data dir + build the runtime. `Err` (as
    /// [`TransferError::Unavailable`]) when no runtime/Bus dir resolves — the
    /// caller falls back to the relay.
    ///
    /// # Errors
    /// [`TransferError::Unavailable`] when the tokio runtime can't build or no
    /// Bus data dir resolves.
    pub fn connect() -> Result<Self, TransferError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| TransferError::Unavailable(format!("tokio runtime: {e}")))?;
        let bus_dir = mde_bus::client_data_dir()
            .ok_or_else(|| TransferError::Unavailable("no Bus data dir".into()))?;
        Ok(Self {
            rt,
            bus_dir,
            call_timeout: Self::DEFAULT_CALL_TIMEOUT,
        })
    }

    /// Override the per-call timeout (tests keep it short).
    #[must_use]
    pub fn with_call_timeout(mut self, t: Duration) -> Self {
        self.call_timeout = t;
        self
    }

    /// Dispatch `req` to the peer helper + fold the reply into a typed outcome.
    /// A no-responder / timeout is [`TransferError::Unavailable`]; the helper's
    /// own honest gate comes back as [`TransferError::Gated`]; both
    /// [`TransferError::should_relay`], so the caller drops to the sshfs relay.
    ///
    /// # Errors
    /// A typed [`TransferError`] on any non-success reply.
    pub fn dispatch(&self, req: &DirectRequest) -> Result<DirectOutcome, TransferError> {
        let body = req.to_json();
        let raw = self.rt.block_on(async {
            let persist = mde_bus::persist::Persist::open(self.bus_dir.clone())
                .map_err(|e| TransferError::Unavailable(format!("bus persist: {e}")))?;
            match mde_bus::rpc::request(
                &persist,
                DIRECT_ACTION_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
                self.call_timeout,
            )
            .await
            {
                Ok(reply) => reply.body.ok_or_else(|| {
                    TransferError::Unavailable(format!("{DIRECT_ACTION_TOPIC}: empty reply"))
                }),
                Err(e) => Err(TransferError::Unavailable(format!(
                    "{DIRECT_ACTION_TOPIC}: {e}"
                ))),
            }
        })?;
        parse_direct_reply(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/run/user/1000/mde-mesh")
    }

    // ── endpoint classification ──────────────────────────────────────────────

    #[test]
    fn classify_peer_path_splits_host_and_relative() {
        let e = classify_endpoint(&root().join("oak/docs/a.txt"), &root());
        assert_eq!(
            e,
            Endpoint::Peer {
                host: "oak".into(),
                mount_rel: PathBuf::from("docs/a.txt"),
            }
        );
        assert_eq!(e.peer_host(), Some("oak"));
    }

    #[test]
    fn classify_peer_mount_root_has_empty_relative() {
        let e = classify_endpoint(&root().join("oak"), &root());
        assert_eq!(
            e,
            Endpoint::Peer {
                host: "oak".into(),
                mount_rel: PathBuf::new(),
            }
        );
    }

    #[test]
    fn classify_local_path_is_local() {
        let e = classify_endpoint(Path::new("/home/matt/x.txt"), &root());
        assert_eq!(e, Endpoint::Local(PathBuf::from("/home/matt/x.txt")));
        assert_eq!(e.peer_host(), None);
    }

    #[test]
    fn classify_mesh_root_itself_is_local_not_a_peer() {
        // The browse root carries no host segment — it must not classify as a
        // (nameless) peer.
        assert_eq!(classify_endpoint(&root(), &root()), Endpoint::Local(root()));
    }

    #[test]
    fn classify_never_treats_dotdot_escape_as_a_peer() {
        // A path that strips to `../etc` under the mesh root must not become a
        // peer endpoint (the `Component::Normal` guard rejects it).
        let sneaky = root().join("../etc/passwd");
        assert!(matches!(
            classify_endpoint(&sneaky, &root()),
            Endpoint::Local(_)
        ));
    }

    #[test]
    fn layout_names_mountpoint_and_classifies() {
        let layout = MeshLayout::new(Path::new("/run/user/1000"));
        assert_eq!(layout.mountpoint("birch"), root().join("birch"));
        assert_eq!(
            layout.classify(&root().join("birch/media")),
            Endpoint::Peer {
                host: "birch".into(),
                mount_rel: PathBuf::from("media"),
            }
        );
    }

    // ── the routing decision (the acceptance-pinned fold) ────────────────────

    fn peer(host: &str, rel: &str) -> Endpoint {
        Endpoint::Peer {
            host: host.into(),
            mount_rel: PathBuf::from(rel),
        }
    }

    #[test]
    fn two_distinct_peers_route_direct() {
        let route = route_transfer(
            &peer("oak", "docs/a.txt"),
            &peer("birch", "incoming"),
            TransferMode::Copy,
        );
        assert!(route.is_direct());
        assert_eq!(
            route,
            TransferRoute::Direct(DirectTransfer {
                src_host: "oak".into(),
                src_rel: PathBuf::from("docs/a.txt"),
                dst_host: "birch".into(),
                dst_rel: PathBuf::from("incoming"),
                mode: TransferMode::Copy,
            })
        );
    }

    #[test]
    fn same_peer_relays_not_direct() {
        // A→A is a copy within one mount, no cross-node hop → relay (same-peer).
        let route = route_transfer(
            &peer("oak", "a.txt"),
            &peer("oak", "backup"),
            TransferMode::Move,
        );
        assert_eq!(route, TransferRoute::Relay(RelayReason::SamePeer));
        assert!(!route.is_direct());
    }

    #[test]
    fn local_source_relays() {
        let route = route_transfer(
            &Endpoint::Local(PathBuf::from("/home/matt/x")),
            &peer("birch", "incoming"),
            TransferMode::Copy,
        );
        assert_eq!(route, TransferRoute::Relay(RelayReason::LocalEndpoint));
    }

    #[test]
    fn local_destination_relays() {
        let route = route_transfer(
            &peer("oak", "a.txt"),
            &Endpoint::Local(PathBuf::from("/home/matt/dl")),
            TransferMode::Copy,
        );
        assert_eq!(route, TransferRoute::Relay(RelayReason::LocalEndpoint));
    }

    #[test]
    fn both_local_relays() {
        let route = route_transfer(
            &Endpoint::Local(PathBuf::from("/a")),
            &Endpoint::Local(PathBuf::from("/b")),
            TransferMode::Copy,
        );
        assert_eq!(route, TransferRoute::Relay(RelayReason::LocalEndpoint));
    }

    #[test]
    fn relay_op_builds_the_right_opkind() {
        let items = vec![PathBuf::from("/m/oak/a"), PathBuf::from("/m/oak/b")];
        let dst = PathBuf::from("/m/oak/dst");
        assert_eq!(
            relay_op(items.clone(), dst.clone(), TransferMode::Copy),
            OpKind::Copy {
                items: items.clone(),
                dest_dir: dst.clone(),
            }
        );
        assert_eq!(
            relay_op(items.clone(), dst.clone(), TransferMode::Move),
            OpKind::Move {
                items,
                dest_dir: dst,
            }
        );
    }

    // ── the direct request + reply ───────────────────────────────────────────

    #[test]
    fn direct_request_round_trips_through_json() {
        let t = DirectTransfer {
            src_host: "oak".into(),
            src_rel: PathBuf::from("docs/a.txt"),
            dst_host: "birch".into(),
            dst_rel: PathBuf::from("incoming"),
            mode: TransferMode::Move,
        };
        let req = DirectRequest::new(9, &t);
        assert_eq!(req.mode, "move");
        let back: DirectRequest = serde_json::from_str(&req.to_json()).expect("decode");
        assert_eq!(back, req);
        assert_eq!(back.op_id, 9);
        assert_eq!(back.src_rel, "docs/a.txt");
    }

    #[test]
    fn parse_reply_ok_is_an_outcome() {
        let out = parse_direct_reply(r#"{"ok":true,"files":3,"bytes":4096}"#).expect("ok");
        assert_eq!(
            out,
            DirectOutcome {
                files: 3,
                bytes: 4096
            }
        );
    }

    #[test]
    fn parse_reply_gated_falls_back_to_relay() {
        let err = parse_direct_reply(r#"{"ok":false,"gated":true,"error":"no rsync on host"}"#)
            .expect_err("gated");
        assert!(matches!(err, TransferError::Gated(_)));
        assert!(err.should_relay(), "a gated direct leg relays");
    }

    #[test]
    fn parse_reply_hard_failure_does_not_relay() {
        let err = parse_direct_reply(r#"{"ok":false,"error":"rsync: permission denied"}"#)
            .expect_err("f");
        assert!(matches!(err, TransferError::Failed(_)));
        assert!(
            !err.should_relay(),
            "a real rsync error surfaces, not relays"
        );
    }

    #[test]
    fn parse_reply_garbage_is_unavailable_and_relays() {
        let err = parse_direct_reply("not json").expect_err("garbage");
        assert!(matches!(err, TransferError::Unavailable(_)));
        assert!(err.should_relay());
    }

    // ── the queued-transfer progress folds ───────────────────────────────────

    #[test]
    fn direct_progress_folds_ticks_into_the_shared_shape() {
        let mut p = DirectProgress::new(7, 4, 1000);
        let first = p.apply(
            &TransferTick {
                bytes_done: 250,
                files_done: 1,
                current: Some(PathBuf::from("a.txt")),
            },
            Duration::from_secs(1),
        );
        assert_eq!(first.op_id, 7);
        assert_eq!(first.bytes_total, 1000);
        assert_eq!(first.bytes_done, 250);
        assert_eq!(first.files_done, 1);
        assert_eq!(first.current.as_deref(), Some(Path::new("a.txt")));
        // 250 B in 1 s → 750 remaining → 3 s ETA; fraction 1/4.
        assert_eq!(first.eta(), Some(Duration::from_secs(3)));
        assert!((first.fraction() - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn direct_progress_is_monotonic_and_clamped() {
        let mut p = DirectProgress::new(1, 2, 100);
        p.apply(
            &TransferTick {
                bytes_done: 80,
                files_done: 1,
                current: None,
            },
            Duration::from_secs(1),
        );
        // A late/duplicate tick with smaller counts must not rewind the bar…
        let snap = p.apply(
            &TransferTick {
                bytes_done: 10,
                files_done: 0,
                current: None,
            },
            Duration::from_secs(2),
        );
        assert_eq!(snap.bytes_done, 80, "monotonic: never rewinds");
        assert_eq!(snap.files_done, 1);
        // …and a runaway tick is clamped to the totals.
        let snap = p.apply(
            &TransferTick {
                bytes_done: 9_999,
                files_done: 9,
                current: None,
            },
            Duration::from_secs(3),
        );
        assert_eq!(snap.bytes_done, 100, "clamped to bytes_total");
        assert_eq!(snap.files_done, 2, "clamped to files_total");
    }

    #[test]
    fn direct_progress_complete_lands_at_the_totals() {
        let mut p = DirectProgress::new(1, 3, 900);
        p.apply(
            &TransferTick {
                bytes_done: 300,
                files_done: 1,
                current: None,
            },
            Duration::from_secs(1),
        );
        let done = p.complete(
            &DirectOutcome {
                files: 3,
                bytes: 900,
            },
            Duration::from_secs(3),
        );
        assert_eq!(done.files_done, 3);
        assert_eq!(done.bytes_done, 900);
        assert!(done.current.is_none());
        assert!((done.fraction() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_progress2_reads_bytes_and_xfr_count() {
        let tick =
            parse_progress2_line("    1,234,567  42%  1.05MB/s    0:00:03 (xfr#7, to-chk=12/40)")
                .expect("parsed");
        assert_eq!(tick.bytes_done, 1_234_567);
        assert_eq!(tick.files_done, 7);
    }

    #[test]
    fn parse_progress2_reads_a_plain_byte_line() {
        // A mid-file progress line with no xfr# marker yet.
        let tick = parse_progress2_line("      65,536  6%  600.00kB/s    0:00:10").expect("parsed");
        assert_eq!(tick.bytes_done, 65_536);
        assert_eq!(tick.files_done, 0);
    }

    #[test]
    fn parse_progress2_skips_non_progress_lines() {
        assert!(parse_progress2_line("sending incremental file list").is_none());
        assert!(parse_progress2_line("").is_none());
        assert!(parse_progress2_line("total size is 1,234  speedup is 1.00").is_none());
    }
}

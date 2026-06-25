//! Backend trait — the surface Artifact Manager renders against.
//!
//! Three concrete impls ship as of v4.0.1 AF-* (2026-05-23):
//!
//!   * [`DemoBackend`] — wraps `demo_data::*` so unit tests +
//!     the "panel boots in 200 ms" smoke gate render the curated
//!     dummy roster without any I/O.
//!   * [`LocalFsBackend`] — real local filesystem reads
//!     (`$HOME`, `$HOME/Downloads`, `$HOME/Documents`, …). Mesh
//!     methods return empty / "this node" placeholders so the
//!     manager still opens cleanly when `mackesd` isn't running.
//!   * [`RealBackend`] — composes `LocalFsBackend` for the local
//!     surface with a `BusBackend` for the mesh surface; falls
//!     back gracefully when `mackesd` is unreachable.
//!
//! The trait is sync + non-blocking — Iced calls each method
//! from its `view()` / `update()` callbacks, both of which run
//! on the GUI thread. The DBus paths inside `RealBackend` use
//! short timeouts + return empty `Vec`s on failure so the GUI
//! thread never blocks for I/O.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::bus_backend::BusBackend;
#[cfg(feature = "dbus")]
use crate::mesh_backend::{MeshBackend, MeshPeer, NebulaStatus};
use crate::model::{
    FileRow, LocalPin, Mime, Peer, PeerKind, PeerStatus, PinIcon, SelfNode, Transfer,
};

/// Stable identifier for a long-running transfer operation. Iced
/// renders the transfer drawer keyed by this.
pub type OpId = u64;

/// One destination of a Send-To. Per the Phase 3.x spec, a
/// destination is either a single peer, a peer group, a role, or a
/// "site" (a region). Today the demo backend only supports
/// per-peer destinations; the richer selectors land with the DBus
/// backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// One named peer.
    Peer(String),
    /// Peer group by name.
    Group(String),
    /// All peers carrying the given role.
    Role(String),
    /// All peers in the given region/site.
    Site(String),
}

/// Send-To mode per Phase 3.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMode {
    Copy,
    Move,
    Sync,
    Deploy,
    Stage,
}

/// Conflict policy per Phase 3.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Ask,
    Skip,
    Overwrite,
    Rename,
}

/// One audit-log row from the operation history (Phase 2.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub op_id: OpId,
    pub kind: &'static str,
    pub source: PathBuf,
    pub destination: Destination,
    pub mode: SendMode,
    pub bytes: u64,
    pub at_ms: i64,
    pub ok: bool,
}

/// Surface every backend implements. Pure abstraction over data +
/// operations — Iced never reaches past this trait.
pub trait Backend {
    /// Self-identity. Iced surfaces this in the breadcrumb + the
    /// header pill ("you are peer:anvil").
    fn self_node(&self) -> SelfNode;
    /// Mesh roster. Sidebar + Send-To picker iterate this.
    fn peers(&self) -> Vec<Peer>;
    /// AFM-RECONNECT — re-attempt any mesh/bus connection that wasn't live at
    /// startup and refresh the cached roster. Called periodically + before each
    /// snapshot so a GUI launched before `mackesd`'s responders were ready (the
    /// cold-boot race) populates its peers on its own instead of staying empty
    /// until restart. Default no-op for the local/demo backends.
    fn reconnect(&mut self) {}
    /// Files visible under a path. Empty path = the mesh overview.
    /// `peer:<id>` paths hit the mesh router; other paths hit the
    /// local FS.
    fn list(&self, path: &str) -> Vec<FileRow>;
    /// Audit history (newest first).
    fn audit_log(&self) -> Vec<AuditEntry>;
    /// Fire a Send-To. Demo backend records the audit row +
    /// returns a synthetic op id immediately; DBus backend
    /// returns once mded has accepted the request.
    fn send_to(
        &mut self,
        sources: &[PathBuf],
        destination: Destination,
        mode: SendMode,
        conflict: ConflictPolicy,
    ) -> Result<OpId, BackendError>;
    /// Roll back a completed operation (Phase 2.7). Returns the
    /// new audit row's op id.
    fn rollback(&mut self, op_id: OpId) -> Result<OpId, BackendError>;
    /// v4.x AF-mesh (2026-05-24) — live Nebula overlay status,
    /// or `None` when the backend has no mesh source (Demo,
    /// LocalFs) or mackesd isn't reachable. Default impl returns
    /// `None` so non-mesh backends compile unchanged.
    fn mesh_overlay(&self) -> Option<MeshOverlayBadge> {
        None
    }
}

/// Backend-surface errors. Surfaced to the UI as toasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// Source file doesn't exist.
    SourceMissing(PathBuf),
    /// Destination unknown / unreachable.
    DestinationUnreachable(Destination),
    /// Operation rejected by validation (Phase 2.5 path-safety, etc).
    Rejected(String),
    /// Op id not in history (rollback).
    NotFound(OpId),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceMissing(p) => write!(f, "source missing: {}", p.display()),
            Self::DestinationUnreachable(d) => write!(f, "destination unreachable: {d:?}"),
            Self::Rejected(reason) => write!(f, "rejected: {reason}"),
            Self::NotFound(id) => write!(f, "op {id} not found"),
        }
    }
}

impl std::error::Error for BackendError {}

/// Per-render snapshot of every list the Iced view functions
/// consume. Built once from the active `Box<dyn Backend>` at the
/// top of `App::view()` so individual view fns take one parameter
/// instead of the seven they'd otherwise need.
///
/// Added v4.0.1 AF-* (2026-05-23). Replaces the `crate::demo_data
/// as data` direct reads that the prototype shipped — every UI
/// fn now reads through this snapshot regardless of whether the
/// underlying backend is `DemoBackend`, `LocalFsBackend`, or
/// `RealBackend`.
#[derive(Debug, Clone, Default)]
pub struct BackendSnapshot {
    pub self_node: SelfNode,
    pub peers: Vec<Peer>,
    pub inbox: Vec<FileRow>,
    /// AFM-6 — files this node has sent to peers, projected from the send
    /// audit log (kind `send_to`). Empty until the operator sends something.
    pub outbox: Vec<FileRow>,
    pub downloads: Vec<FileRow>,
    pub local_pins: Vec<LocalPin>,
    pub local_recent: Vec<FileRow>,
    pub recent_transfers: Vec<Transfer>,
    /// v4.x AF-mesh (2026-05-24) — live Nebula overlay snapshot
    /// pulled from `dev.mackes.MDE.Nebula.Status::Status` at
    /// capture time. `None` when mackesd isn't running OR the
    /// backend isn't a `RealBackend`; views read this through
    /// `mesh_overlay()` so they degrade cleanly to the
    /// "no-mesh" rendering.
    pub mesh_overlay: Option<MeshOverlayBadge>,
}

/// Lightweight, UI-friendly projection of `dev.mackes.MDE.
/// Nebula.Status::Status`. Stripped to the fields the file
/// manager actually renders (titlebar pill + mesh-overview
/// banner) so the sidebar code doesn't have to know the
/// full wire schema.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MeshOverlayBadge {
    pub is_lighthouse: bool,
    pub ca_epoch: i64,
    pub mesh_id: String,
    pub peer_count: usize,
    pub active_transport: String,
}

impl BackendSnapshot {
    /// Build a snapshot by querying the active backend for every
    /// list the view tree might touch this render. The `peer_id`
    /// argument is `None` for the mesh-overview / inbox / etc.
    /// views and `Some(id)` when the user has navigated into a
    /// peer folder (so the snapshot can include the cached list
    /// later — for now per-peer files are fetched lazily by the
    /// view fn).
    #[must_use]
    pub fn capture(backend: &dyn Backend) -> Self {
        let self_node = backend.self_node();
        let peers = backend.peers();
        let inbox = backend.list("");
        let downloads = backend.list("downloads");
        // AFM-6 — Outbox: the files this node has sent, projected from the send
        // audit log. Newest first (audit_log returns reverse-chronological).
        let outbox: Vec<FileRow> = backend
            .audit_log()
            .into_iter()
            .filter(|a| a.kind == "send_to" && a.ok)
            .map(|a| {
                let name = a
                    .source
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| a.source.display().to_string());
                let peer = match a.destination {
                    Destination::Peer(p)
                    | Destination::Group(p)
                    | Destination::Role(p)
                    | Destination::Site(p) => p,
                };
                FileRow::local(name, crate::model::Mime::Doc, format!("{} B", a.bytes), "")
                    .with_mesh(peer)
            })
            .collect();
        let local_pins = local_pins_xdg(&std::env::var_os("HOME"));
        let local_recent = local_recent_from(&std::env::var_os("HOME"));
        // Transfers live in the audit log feed; until mackesd
        // ships a streaming Shell.AuditLog method the snapshot
        // pulls the in-memory backend audit and projects it.
        let recent_transfers = backend
            .audit_log()
            .into_iter()
            .take(5)
            .map(|a| crate::model::Transfer {
                dir: match a.kind {
                    "send_to" => crate::model::TxDir::Out,
                    _ => crate::model::TxDir::In,
                },
                name: a
                    .source
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| a.source.display().to_string()),
                peer: match a.destination {
                    Destination::Peer(p)
                    | Destination::Group(p)
                    | Destination::Role(p)
                    | Destination::Site(p) => p,
                },
                size: format!("{} B", a.bytes),
                age: String::new(),
            })
            .collect();
        let mesh_overlay = backend.mesh_overlay();
        Self {
            self_node,
            peers,
            inbox,
            outbox,
            downloads,
            local_pins,
            local_recent,
            recent_transfers,
            mesh_overlay,
        }
    }
}

/// Generate `LocalPin`s for the XDG user dirs that actually
/// exist on disk. Falls back to a curated set of paths when
/// `$HOME` is unset (test environments, daemonised launchers).
fn local_pins_xdg(home: &Option<std::ffi::OsString>) -> Vec<LocalPin> {
    let home_path: PathBuf = home
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut pins = vec![LocalPin {
        id: "home".into(),
        name: "Home".into(),
        path: home_path.display().to_string(),
        icon: PinIcon::Home,
    }];
    for (slug, label, sub, icon) in [
        ("docs", "Documents", "Documents", PinIcon::Doc2),
        ("pics", "Pictures", "Pictures", PinIcon::Image),
        ("music", "Music", "Music", PinIcon::Doc),
        ("videos", "Videos", "Videos", PinIcon::Player),
        ("code", "Code", "code", PinIcon::Rust),
    ] {
        let p = home_path.join(sub);
        if p.is_dir() {
            pins.push(LocalPin {
                id: slug.into(),
                name: label.into(),
                path: p.display().to_string(),
                icon,
            });
        }
    }
    pins.push(LocalPin {
        id: "root".into(),
        name: "Filesystem".into(),
        path: "/".into(),
        icon: PinIcon::Hdd,
    });
    pins
}

/// Read the 6 most-recently-modified entries in `$HOME` for the
/// "Recent locally-modified" section of the Local view. Returns
/// an empty Vec when `$HOME` isn't readable.
fn local_recent_from(home: &Option<std::ffi::OsString>) -> Vec<FileRow> {
    let Some(home) = home.as_ref() else {
        return Vec::new();
    };
    let rows = LocalFsBackend::list_dir(Path::new(home));
    rows.into_iter().take(6).collect()
}

/// v2.0.0 Phase 2.2 — in-memory `Backend` impl wrapping the demo
/// data. Used for headless tests + the panel-boot smoke gate.
pub struct DemoBackend {
    next_op_id: OpId,
    audit: Vec<AuditEntry>,
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoBackend {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_op_id: 1,
            audit: Vec::new(),
        }
    }

    fn alloc_id(&mut self) -> OpId {
        let id = self.next_op_id;
        self.next_op_id += 1;
        id
    }
}

impl Backend for DemoBackend {
    fn self_node(&self) -> SelfNode {
        crate::demo_data::self_node()
    }

    fn peers(&self) -> Vec<Peer> {
        crate::demo_data::peers()
    }

    fn list(&self, path: &str) -> Vec<FileRow> {
        match path {
            "" | "/" => crate::demo_data::inbox(),
            "downloads" => crate::demo_data::downloads(),
            "peer:pine" => crate::demo_data::pine_files(),
            "peer:birch" => crate::demo_data::birch_files(),
            "peer:oak" => crate::demo_data::oak_files(),
            _ => Vec::new(),
        }
    }

    fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit.iter().rev().cloned().collect()
    }

    fn send_to(
        &mut self,
        sources: &[PathBuf],
        destination: Destination,
        mode: SendMode,
        _conflict: ConflictPolicy,
    ) -> Result<OpId, BackendError> {
        if sources.is_empty() {
            return Err(BackendError::Rejected("empty source list".into()));
        }
        let id = self.alloc_id();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let total_bytes: u64 = sources
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        self.audit.push(AuditEntry {
            op_id: id,
            kind: "send_to",
            source: sources[0].clone(),
            destination,
            mode,
            bytes: total_bytes,
            at_ms: now_ms,
            ok: true,
        });
        Ok(id)
    }

    fn rollback(&mut self, op_id: OpId) -> Result<OpId, BackendError> {
        let original = self.audit.iter().find(|a| a.op_id == op_id).cloned();
        let Some(original) = original else {
            return Err(BackendError::NotFound(op_id));
        };
        let id = self.alloc_id();
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.audit.push(AuditEntry {
            op_id: id,
            kind: "rollback",
            source: original.source.clone(),
            destination: original.destination.clone(),
            mode: original.mode,
            bytes: original.bytes,
            at_ms: now_ms,
            ok: true,
        });
        Ok(id)
    }
}

/// v4.0.1 AF-* (2026-05-23) — real-filesystem `Backend` impl.
/// Reads `$HOME` and standard XDG dirs. Mesh methods return
/// `self`-only roster ("this node" with hostname) + empty peer
/// list so the manager opens cleanly without mackesd. The mesh
/// surface ships through [`RealBackend`].
pub struct LocalFsBackend {
    next_op_id: OpId,
    audit: Vec<AuditEntry>,
    home: PathBuf,
    hostname: String,
}

impl LocalFsBackend {
    #[must_use]
    pub fn new() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let hostname = std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "this-node".to_string());
        Self {
            next_op_id: 1,
            audit: Vec::new(),
            home,
            hostname,
        }
    }

    fn alloc_id(&mut self) -> OpId {
        let id = self.next_op_id;
        self.next_op_id += 1;
        id
    }

    /// Resolve a Files-app path to an absolute disk path. Accepts:
    ///   * `""` / `"/"` — INBOX-equivalent → returns the home dir.
    ///   * `"downloads"` → `$HOME/Downloads`.
    ///   * `"local:<slug>"` — a `LocalPin` id mapped to its real
    ///     path under `$HOME`.
    ///   * `"local:<slug>/<subpath>"` — a pin id followed by
    ///     a nested subdirectory (AF-mesh.3 subdir navigation).
    ///     Path-safety: any `..` segment in the subpath aborts the
    ///     resolve so the manager can never accidentally escape
    ///     the XDG root the operator clicked into.
    ///   * any other relative path — treated relative to `$HOME`.
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            return Some(self.home.clone());
        }
        if trimmed == "downloads" {
            return Some(self.home.join("Downloads"));
        }
        if let Some(rest) = trimmed.strip_prefix("local:") {
            // Split `<slug>` from the optional `/<subpath>` tail.
            let (slug, sub) = match rest.find('/') {
                Some(pos) => (&rest[..pos], &rest[pos + 1..]),
                None => (rest, ""),
            };
            let base = match slug {
                "home" => self.home.clone(),
                "docs" => self.home.join("Documents"),
                "pics" => self.home.join("Pictures"),
                "music" => self.home.join("Music"),
                "videos" => self.home.join("Videos"),
                "code" => self.home.join("code"),
                "downloads" => self.home.join("Downloads"),
                "root" => PathBuf::from("/"),
                _ => return None,
            };
            if sub.is_empty() {
                return Some(base);
            }
            // Path-safety: reject `..` segments outright. Empty
            // segments (from a stray `//`) get skipped silently.
            let mut joined = base;
            for segment in sub.split('/') {
                if segment.is_empty() {
                    continue;
                }
                if segment == ".." || segment.contains('\0') {
                    return None;
                }
                joined.push(segment);
            }
            return Some(joined);
        }
        // Already-absolute paths stay as-is; relative paths land
        // under $HOME so the app can never accidentally browse
        // outside the user's tree without a typed prefix.
        if path.starts_with('/') {
            Some(PathBuf::from(path))
        } else {
            Some(self.home.join(trimmed))
        }
    }

    /// Read a directory and return the entries as `FileRow`s,
    /// newest-first.
    pub fn list_dir(dir: &Path) -> Vec<FileRow> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut rows: Vec<(SystemTime, FileRow)> = Vec::new();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            // Hide dotfiles in the home roots but show them inside
            // explicit dotted paths (e.g., when the user navigates
            // to `.config`). Heuristic: only hide if we're at a
            // root-level listing; passing the parent through the
            // resolver isn't trivial here so just always show.
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let mime = mime_of(&entry.path(), meta.is_dir());
            let size = if meta.is_dir() {
                folder_summary(&entry.path())
            } else {
                fmt_bytes(meta.len())
            };
            let age = fmt_age(mtime);
            let abs_path = entry.path().to_string_lossy().into_owned();
            let display = if meta.is_dir() {
                format!("{name}/")
            } else {
                name
            };
            rows.push((
                mtime,
                FileRow::local(display, mime, size, age).with_path(abs_path),
            ));
        }
        // newest first
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        rows.into_iter().map(|(_, r)| r).collect()
    }
}

impl Default for LocalFsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for LocalFsBackend {
    fn self_node(&self) -> SelfNode {
        SelfNode {
            id: format!("self:{}", self.hostname),
            host: self.hostname.clone(),
            label: "this node".into(),
            addr: "—".into(),
            files: 0,
            shared: 0,
        }
    }

    fn peers(&self) -> Vec<Peer> {
        Vec::new()
    }

    fn list(&self, path: &str) -> Vec<FileRow> {
        match self.resolve(path) {
            Some(p) => Self::list_dir(&p),
            None => Vec::new(),
        }
    }

    fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit.iter().rev().cloned().collect()
    }

    fn send_to(
        &mut self,
        sources: &[PathBuf],
        destination: Destination,
        _mode: SendMode,
        _conflict: ConflictPolicy,
    ) -> Result<OpId, BackendError> {
        if sources.is_empty() {
            return Err(BackendError::Rejected("empty source list".into()));
        }
        // No mesh = every destination is unreachable until
        // RealBackend routes through DBus.
        Err(BackendError::DestinationUnreachable(destination))
    }

    fn rollback(&mut self, op_id: OpId) -> Result<OpId, BackendError> {
        let original = self.audit.iter().find(|a| a.op_id == op_id).cloned();
        let Some(original) = original else {
            return Err(BackendError::NotFound(op_id));
        };
        let id = self.alloc_id();
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.audit.push(AuditEntry {
            op_id: id,
            kind: "rollback",
            source: original.source.clone(),
            destination: original.destination.clone(),
            mode: original.mode,
            bytes: original.bytes,
            at_ms: now_ms,
            ok: true,
        });
        Ok(id)
    }
}

/// v4.0.1 AF-* — composed `Backend` that combines the local FS
/// surface with the mesh surface from mackesd. `peer:<id>` paths
/// + the `peers()` call route through DBus; everything else hits
/// the local FS. If DBus is unreachable the mesh surface comes
/// back empty (no panic, no fallback to demo data).
///
/// v4.x AF-mesh (2026-05-24) — adds the `MeshBackend` companion
/// that reads from `dev.mackes.MDE.Nebula.Status`. When
/// `MeshBackend` is connectable, the cached peer list comes from
/// the live Nebula roster instead of the older Fleet.Files reads.
/// The Fleet.Files `BusBackend` is retained for the
/// audit/transfer/list_peer surface that still ships from there.
pub struct RealBackend {
    local: LocalFsBackend,
    bus: Option<BusBackend>,
    mesh: Option<MeshBackend>,
    cached_self_node: SelfNode,
    cached_peers: Vec<Peer>,
    cached_mesh_overlay: Option<MeshOverlayBadge>,
}

impl RealBackend {
    /// AFM-RECONNECT — connect probe. Widened from 800 ms: at GUI launch right
    /// after boot, `mackesd`'s Fleet.Files / mesh responders can take a beat to
    /// come up, and an 800 ms miss left the Artifact Manager with no peers until
    /// a restart. `reconnect` retries with this same budget on a later tick.
    const CONNECT_TIMEOUT: Duration = Duration::from_millis(2500);

    #[must_use]
    pub fn new() -> Self {
        let local = LocalFsBackend::new();
        let bus = BusBackend::connect_with_timeout(Self::CONNECT_TIMEOUT).ok();
        let mesh = MeshBackend::connect_with_timeout(Self::CONNECT_TIMEOUT).ok();

        let cached_self_node = match mesh.as_ref().and_then(|m| m.nebula_self_node().ok()) {
            Some(n) => SelfNode {
                id: n.node_id,
                host: n.host,
                label: "this node".into(),
                addr: n.overlay_ip,
                files: 0,
                shared: 0,
            },
            None => match bus.as_ref().and_then(|d| d.self_node().ok()) {
                Some(s) => s,
                None => local.self_node(),
            },
        };

        // Prefer mesh peers (real Nebula reachability). Fall back
        // to Fleet.Files-cached peers, then to an empty list.
        let cached_peers = match mesh.as_ref().and_then(|m| m.mesh_peers().ok()) {
            Some(rows) => rows.into_iter().map(mesh_peer_to_peer).collect(),
            None => bus
                .as_ref()
                .and_then(|d| d.peers().ok())
                .unwrap_or_default(),
        };

        let cached_mesh_overlay = mesh
            .as_ref()
            .and_then(|m| m.nebula_status().ok())
            .map(nebula_status_to_badge);

        Self {
            local,
            bus,
            mesh,
            cached_self_node,
            cached_peers,
            cached_mesh_overlay,
        }
    }

    #[must_use]
    pub fn has_mesh(&self) -> bool {
        self.mesh.is_some() || self.bus.is_some()
    }

    /// AFM-RECONNECT — re-attempt the mesh/bus connections that weren't live at
    /// construction and refresh the cached roster. Cheap + idempotent when fully
    /// connected with peers (returns early); does real work only while a backend
    /// is missing or the roster is still empty, so it's safe to call on a tick.
    fn reconnect_now(&mut self) {
        let fully_up = self.mesh.is_some() && self.bus.is_some() && !self.cached_peers.is_empty();
        if fully_up {
            return;
        }
        if self.bus.is_none() {
            self.bus = BusBackend::connect_with_timeout(Self::CONNECT_TIMEOUT).ok();
        }
        if self.mesh.is_none() {
            self.mesh = MeshBackend::connect_with_timeout(Self::CONNECT_TIMEOUT).ok();
        }
        // Refresh the cached roster + identity from whichever backend is live
        // (prefer the Nebula mesh roster, fall back to Fleet.Files).
        if let Some(rows) = self.mesh.as_ref().and_then(|m| m.mesh_peers().ok()) {
            self.cached_peers = rows.into_iter().map(mesh_peer_to_peer).collect();
        } else if let Some(rows) = self.bus.as_ref().and_then(|d| d.peers().ok()) {
            self.cached_peers = rows;
        }
        if let Some(n) = self.mesh.as_ref().and_then(|m| m.nebula_self_node().ok()) {
            self.cached_self_node = SelfNode {
                id: n.node_id,
                host: n.host,
                label: "this node".into(),
                addr: n.overlay_ip,
                files: 0,
                shared: 0,
            };
        } else if let Some(s) = self.bus.as_ref().and_then(|d| d.self_node().ok()) {
            self.cached_self_node = s;
        }
        if let Some(badge) = self
            .mesh
            .as_ref()
            .and_then(|m| m.nebula_status().ok())
            .map(nebula_status_to_badge)
        {
            self.cached_mesh_overlay = Some(badge);
        }
    }
}

impl Default for RealBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for RealBackend {
    fn self_node(&self) -> SelfNode {
        self.cached_self_node.clone()
    }

    fn peers(&self) -> Vec<Peer> {
        self.cached_peers.clone()
    }

    fn reconnect(&mut self) {
        self.reconnect_now();
    }

    fn list(&self, path: &str) -> Vec<FileRow> {
        if let Some(peer) = path.strip_prefix("peer:") {
            return self
                .bus
                .as_ref()
                .and_then(|d| d.list_peer(peer).ok())
                .unwrap_or_default();
        }
        // AFM-5 — the empty path is the mesh Inbox ONLY: received files from the
        // Syncthing-replicated inbox over the Bus. When mackesd/Bus is
        // unavailable the inbox is empty — return an honest empty list, never
        // the local home directory. (The old fallback to `local.list("")`
        // resolved to `$HOME`, so an offline Bus made the Inbox view show the
        // operator's home directory as if those files had been received.)
        if path.is_empty() {
            return self.bus.as_ref().map(BusBackend::inbox).unwrap_or_default();
        }
        // E10 — Cloud-Files: the paired KDE-Connect device roster.
        if path == "cloud:" {
            return self
                .bus
                .as_ref()
                .map(BusBackend::cloud_devices)
                .unwrap_or_default();
        }
        self.local.list(path)
    }

    fn audit_log(&self) -> Vec<AuditEntry> {
        // Audit lives on the local backend for now; once mackesd
        // ships a Shell.AuditLog stream the entries will be
        // merged here.
        self.local.audit_log()
    }

    fn send_to(
        &mut self,
        sources: &[PathBuf],
        destination: Destination,
        mode: SendMode,
        conflict: ConflictPolicy,
    ) -> Result<OpId, BackendError> {
        // AUD-1 — route the send through mackesd's file-ops surface, which
        // copies the sources into the target peer's Syncthing-replicated
        // inbox (the real cross-mesh transport). With no mackesd, fall back
        // to the local-FS backend (records an audit row, rejects mesh dests).
        #[cfg(feature = "dbus")]
        if let Some(bus) = self.bus.as_ref() {
            return bus.send_to(sources, &destination, mode, conflict);
        }
        self.local.send_to(sources, destination, mode, conflict)
    }

    fn rollback(&mut self, op_id: OpId) -> Result<OpId, BackendError> {
        self.local.rollback(op_id)
    }

    fn mesh_overlay(&self) -> Option<MeshOverlayBadge> {
        self.cached_mesh_overlay.clone()
    }
}

// ----- mesh → existing-model bridges --------------------------

/// Convert a `MeshPeer` (Nebula shape) into the existing `Peer`
/// model the sidebar + cards already render.
#[cfg(feature = "dbus")]
pub fn mesh_peer_to_peer(mp: MeshPeer) -> Peer {
    let status = match mp.reachable.as_str() {
        "online" | "healthy" => PeerStatus::Online,
        "idle" | "degraded" => PeerStatus::Idle,
        _ => PeerStatus::Offline,
    };
    Peer {
        id: if mp.node_id.is_empty() {
            mp.host.clone()
        } else {
            mp.node_id.clone()
        },
        host: if mp.overlay_ip.is_empty() {
            mp.host.clone()
        } else {
            format!("{} · {}", mp.host, mp.overlay_ip)
        },
        label: mp.host,
        kind: PeerKind::Desktop,
        addr: mp.overlay_ip,
        status,
        latency: None,
        files: 0,
        shared: 0,
        last: String::new(),
    }
}

#[cfg(feature = "dbus")]
fn nebula_status_to_badge(s: NebulaStatus) -> MeshOverlayBadge {
    MeshOverlayBadge {
        is_lighthouse: s.is_lighthouse,
        ca_epoch: s.ca_epoch,
        mesh_id: s.mesh_id,
        peer_count: s.peer_count,
        active_transport: s.active_transport,
    }
}

// ---- helpers --------------------------------------------------

fn mime_of(p: &Path, is_dir: bool) -> Mime {
    if is_dir {
        return Mime::Folder;
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tiff" => Mime::Image,
        "pdf" => Mime::Pdf,
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" => Mime::Archive,
        "iso" | "qcow2" | "img" | "raw" => Mime::Disk,
        _ => Mime::Doc,
    }
}

fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} B")
    }
}

fn fmt_age(t: SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        format!("{} h", secs / 3600)
    } else if secs < 30 * 86_400 {
        format!("{} d", secs / 86_400)
    } else if secs < 365 * 86_400 {
        format!("{} mo", secs / (30 * 86_400))
    } else {
        format!("{} y", secs / (365 * 86_400))
    }
}

fn folder_summary(p: &Path) -> String {
    let count = std::fs::read_dir(p)
        .ok()
        .map(|it| it.flatten().count())
        .unwrap_or(0);
    if count == 0 {
        "— · empty".into()
    } else {
        format!("— · {count} items")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_backend_returns_demo_self_node() {
        let b = DemoBackend::new();
        let self_node = b.self_node();
        assert_eq!(self_node.host, "yew.mesh");
    }

    #[test]
    fn demo_backend_peers_match_demo_data() {
        let b = DemoBackend::new();
        assert_eq!(b.peers().len(), crate::demo_data::peers().len());
    }

    #[test]
    fn demo_backend_list_returns_inbox_for_empty_path() {
        let b = DemoBackend::new();
        let rows = b.list("");
        assert_eq!(rows.len(), crate::demo_data::inbox().len());
    }

    #[test]
    fn demo_backend_list_returns_per_peer_files() {
        let b = DemoBackend::new();
        assert!(!b.list("peer:pine").is_empty());
        assert!(!b.list("peer:birch").is_empty());
        assert!(!b.list("peer:oak").is_empty());
    }

    #[test]
    fn demo_backend_list_returns_empty_for_unknown_path() {
        let b = DemoBackend::new();
        assert!(b.list("not-a-real-path").is_empty());
    }

    #[test]
    fn demo_backend_audit_log_starts_empty() {
        let b = DemoBackend::new();
        assert!(b.audit_log().is_empty());
    }

    #[test]
    fn send_to_rejects_empty_source_list() {
        let mut b = DemoBackend::new();
        let r = b.send_to(
            &[],
            Destination::Peer("pine".into()),
            SendMode::Copy,
            ConflictPolicy::Ask,
        );
        assert!(matches!(r, Err(BackendError::Rejected(_))));
    }

    #[test]
    fn send_to_records_audit_row_and_returns_increasing_op_ids() {
        let mut b = DemoBackend::new();
        let id1 = b
            .send_to(
                &[PathBuf::from("/tmp/a")],
                Destination::Peer("pine".into()),
                SendMode::Copy,
                ConflictPolicy::Ask,
            )
            .expect("send_to");
        let id2 = b
            .send_to(
                &[PathBuf::from("/tmp/b")],
                Destination::Peer("birch".into()),
                SendMode::Move,
                ConflictPolicy::Overwrite,
            )
            .expect("send_to");
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        let log = b.audit_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].op_id, id2);
        assert_eq!(log[1].op_id, id1);
    }

    #[test]
    fn rollback_records_rollback_audit_row() {
        let mut b = DemoBackend::new();
        let original = b
            .send_to(
                &[PathBuf::from("/tmp/x")],
                Destination::Peer("oak".into()),
                SendMode::Copy,
                ConflictPolicy::Ask,
            )
            .expect("send_to");
        let rb = b.rollback(original).expect("rollback");
        assert_ne!(original, rb);
        let log = b.audit_log();
        assert_eq!(log[0].op_id, rb);
        assert_eq!(log[0].kind, "rollback");
    }

    #[test]
    fn rollback_unknown_id_returns_not_found() {
        let mut b = DemoBackend::new();
        let r = b.rollback(999);
        assert!(matches!(r, Err(BackendError::NotFound(999))));
    }

    #[test]
    fn backend_error_display_includes_context() {
        let e = BackendError::SourceMissing(PathBuf::from("/missing"));
        assert!(format!("{e}").contains("source missing"));
        assert!(format!("{e}").contains("/missing"));

        let e = BackendError::DestinationUnreachable(Destination::Peer("x".into()));
        assert!(format!("{e}").contains("destination"));

        let e = BackendError::NotFound(7);
        assert!(format!("{e}").contains("7"));
    }

    #[test]
    fn local_fs_backend_self_node_reads_hostname() {
        let b = LocalFsBackend::new();
        let s = b.self_node();
        assert!(!s.host.is_empty());
        assert_eq!(s.label, "this node");
    }

    #[test]
    fn local_fs_backend_no_peers_without_dbus() {
        let b = LocalFsBackend::new();
        assert!(b.peers().is_empty());
    }

    #[test]
    fn local_fs_backend_list_home_returns_something() {
        // Best-effort: the test env's $HOME exists. Even if it
        // doesn't, the result is just an empty Vec — never panics.
        let b = LocalFsBackend::new();
        let _rows = b.list("");
        // No assertion on contents — varies by environment.
    }

    #[test]
    fn local_fs_backend_unknown_local_slug_returns_empty() {
        let b = LocalFsBackend::new();
        assert!(b.list("local:does-not-exist").is_empty());
    }

    /// AFM-5 — the Inbox view (empty path) must NEVER fall back to the local
    /// home directory. With no Bus connected (the test env), `list("")` is the
    /// mesh inbox and must come back empty, not the operator's `$HOME` listing.
    #[test]
    fn real_backend_inbox_is_empty_not_home_without_bus() {
        let b = RealBackend::new();
        // No mackesd/Bus in the test env → honest empty inbox.
        if b.bus.is_none() {
            assert!(
                b.list("").is_empty(),
                "Inbox must be empty (not the home directory) when the Bus is down"
            );
        }
    }

    #[test]
    fn local_fs_backend_resolves_subpath_under_slug() {
        let b = LocalFsBackend::new();
        let p = b.resolve("local:docs/Projects/MDE").expect("resolves");
        assert!(p.ends_with("Documents/Projects/MDE"));
    }

    #[test]
    fn local_fs_backend_resolves_subpath_skips_double_slashes() {
        let b = LocalFsBackend::new();
        let p = b.resolve("local:docs//foo/bar").expect("resolves");
        assert!(p.ends_with("Documents/foo/bar"));
    }

    #[test]
    fn local_fs_backend_rejects_parent_traversal_in_subpath() {
        let b = LocalFsBackend::new();
        assert!(b.resolve("local:docs/..").is_none());
        assert!(b.resolve("local:docs/a/../etc").is_none());
    }

    #[test]
    fn local_fs_backend_rejects_null_byte_in_subpath() {
        let b = LocalFsBackend::new();
        assert!(b.resolve("local:docs/a\0b").is_none());
    }

    #[test]
    fn local_fs_backend_root_slug_with_subpath() {
        let b = LocalFsBackend::new();
        let p = b.resolve("local:root/etc/hostname").expect("resolves");
        assert_eq!(p, std::path::PathBuf::from("/etc/hostname"));
    }

    #[test]
    fn local_fs_backend_downloads_slug_with_subpath() {
        let b = LocalFsBackend::new();
        // New `downloads` slug under `local:` (AF-mesh.3 added).
        let p = b
            .resolve("local:downloads/incoming")
            .expect("resolves downloads");
        assert!(p.ends_with("Downloads/incoming"));
    }

    #[test]
    fn local_fs_backend_rejects_mesh_destination_without_dbus() {
        let mut b = LocalFsBackend::new();
        let r = b.send_to(
            &[PathBuf::from("/tmp/x")],
            Destination::Peer("pine".into()),
            SendMode::Copy,
            ConflictPolicy::Ask,
        );
        assert!(matches!(r, Err(BackendError::DestinationUnreachable(_))));
    }

    #[test]
    fn fmt_bytes_thresholds() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn mime_of_classifies_extensions() {
        assert_eq!(mime_of(Path::new("a.jpg"), false), Mime::Image);
        assert_eq!(mime_of(Path::new("a.pdf"), false), Mime::Pdf);
        assert_eq!(mime_of(Path::new("a.tar.gz"), false), Mime::Archive);
        assert_eq!(mime_of(Path::new("a.iso"), false), Mime::Disk);
        assert_eq!(mime_of(Path::new("a.txt"), false), Mime::Doc);
        assert_eq!(mime_of(Path::new("a"), true), Mime::Folder);
    }
}

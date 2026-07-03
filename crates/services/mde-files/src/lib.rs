//! MDE Files — mesh-first "Artifact Manager" for MCNF.
//!
//! Implementation contract: `docs/design/v2.0.0-mde-files/design-spec.md`.
//! Prototype: `docs/design/v2.0.0-mde-files/upstream-bundle/Artifact-Manager.html`.
//!
//! ## The three file bridges (SVC-5 / Q67 lock)
//!
//! mde-files reaches remote files over exactly three **co-equal**
//! bridges — none is "the real one", none is deprecated, and new
//! file-transfer features must consider all three:
//!
//! 1. **Mesh** — peer files over the Bus (`action/fleet-files/*`,
//!    [`bus_backend`]) + the Syncthing-replicated QNM dirs ([`mounts`]).
//!    The default path between enrolled peers.
//! 2. **SMB** — classic LAN shares ([`mounts`] / gio), for the NAS and
//!    non-mesh machines on the same network.
//! 3. **KDC** — phone/tablet files via the KDE-Connect-protocol host
//!    (`action/connect/*`), for paired mobile devices.

// ── Render-agnostic surface (always compiled) ───────────────────────────────
// The file/listing/transfer model + the Bus client. This subset carries no
// GUI-toolkit dependency — E12's `mde-files-egui` renders it on the egui
// harness. E12-14b stripped the Cosmic-era iced GUI + all modules that
// only that GUI used (archive, bookmarks, desktop, fileops, grid, mime,
// panels, properties, search, selection, smb, thumbnails, trash).
pub mod archive;
pub mod backend;
#[cfg(feature = "dbus")]
pub mod bus_backend;
pub mod fileops;
#[cfg(feature = "dbus")]
pub mod mesh_backend;
pub mod model;
pub mod opqueue;
// FILEMGR-4 — async recursive search: a streaming, cancellable traversal over the
// `FileOps` seam (name-glob + content grep + type/size/mtime filters) whose hits
// are `FileRow`s, so a result set renders as a normal file view and every op
// applies. Render-agnostic (no GUI toolkit) — `mde-files-egui` runs it off-thread.
pub mod search;
pub mod send_to;
// FILEMGR-7 — direct peer-to-peer transfer routing + the queued transfer. The
// pure routing/plan/progress folds are always compiled; the live Bus dispatch
// to the mackesd peer helper is honestly gated behind `dbus`.
pub mod transfer;

// ── E12-14b — the windowed Cosmic-era surface was stripped ──────────────────
// The Cosmic-era iced file-manager GUI (`app`/`views`/`widgets`/`icons`/`theme`/
// `loading`/`picker`/`prefs`/`cosmic_compat`/`mounts` + the `mde-files` binary)
// is retired. MCNF 12.0 "Quasar" renders Files as an egui panel
// (`mde-files-egui::files_panel`) inside `mde-shell-egui`, reusing the
// render-agnostic backend/model/send_to above. No `gui` feature remains.
pub use archive::{browse as browse_archive, compress, extract, ArchiveEntry, ArchiveFormat};
pub use backend::{
    AuditEntry, Backend, BackendError, ConflictPolicy, Destination, LocalFsBackend, OpId, SendMode,
};
pub use fileops::{FakeFileOps, FileOps, FileStat, LiveFileOps};
pub use model::{FileRow, Layout, Mime, Peer, PeerKind, PeerStatus, SelfNode, Tab, View};
pub use opqueue::{
    channel_resolver, execute, ChannelResolver, Conflict, ConflictChoice, ConflictPrompt,
    ConflictResolver, FixedResolution, FnResolver, OpControl, OpEvent, OpKind, OpOutcome, OpQueue,
    Progress, QueuedOp, Resolution,
};
pub use search::{
    run_search, CompiledQuery, ContentMode, ContentQuery, Filters, SearchError, SearchEvent,
    SearchQuery, SearchRun, SearchStats, TypeFilter,
};
pub use transfer::{
    classify_endpoint, parse_direct_reply, parse_progress2_line, relay_op, route_transfer,
    scan_source_totals, DirectOutcome, DirectProgress, DirectRequest, DirectTransfer, Endpoint,
    MeshLayout, RelayReason, TransferError, TransferMode, TransferRoute, TransferTick,
};

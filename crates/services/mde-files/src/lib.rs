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
pub mod backend;
#[cfg(feature = "dbus")]
pub mod bus_backend;
pub mod demo_data;
#[cfg(feature = "dbus")]
pub mod mesh_backend;
pub mod model;
pub mod send_to;

// ── E12-14b — the windowed Cosmic-era surface was stripped ──────────────────
// The Cosmic-era iced file-manager GUI (`app`/`views`/`widgets`/`icons`/`theme`/
// `loading`/`picker`/`prefs`/`cosmic_compat`/`mounts` + the `mde-files` binary)
// is retired. MCNF 12.0 "Quasar" renders Files as an egui panel
// (`mde-files-egui::files_panel`) inside `mde-shell-egui`, reusing the
// render-agnostic backend/model/send_to above. No `gui` feature remains.
pub use backend::{
    AuditEntry, Backend, BackendError, ConflictPolicy, DemoBackend, Destination, OpId, SendMode,
};
pub use model::{FileRow, Layout, Mime, Peer, PeerKind, PeerStatus, SelfNode, Tab, View};

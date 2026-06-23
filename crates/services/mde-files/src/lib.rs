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
//!    [`bus_backend`]) + the LizardFS-replicated QNM dirs ([`mounts`]).
//!    The default path between enrolled peers.
//! 2. **SMB** — classic LAN shares ([`mounts`] / gio), for the NAS and
//!    non-mesh machines on the same network.
//! 3. **KDC** — phone/tablet files via the KDE-Connect-protocol host
//!    (`action/connect/*`), for paired mobile devices.

pub mod a11y_labels;
pub mod app;
pub mod archive;
pub mod backend;
pub mod bookmarks;
#[cfg(feature = "dbus")]
pub mod bus_backend;
/// GUI-7 — libcosmic `.sty()` styling shims (see module docs).
pub mod cosmic_compat;
pub mod demo_data;
pub mod desktop;
pub mod fileops;
pub mod grid;
pub mod icons;
pub mod loading;
#[cfg(feature = "dbus")]
pub mod mesh_backend;
pub mod mime;
pub mod model;
pub mod mounts;
pub mod panels;
pub mod picker;
pub mod prefs;
pub mod properties;
pub mod search;
pub mod selection;
pub mod send_to;
pub mod smb;
pub mod theme;
pub mod thumbnails;
pub mod trash;
pub mod views;
pub mod widgets;

pub use app::{MdeFiles, Message};
pub use backend::{
    AuditEntry, Backend, BackendError, ConflictPolicy, DemoBackend, Destination, OpId, SendMode,
};
pub use model::{FileRow, Layout, Mime, Peer, PeerKind, PeerStatus, SelfNode, Tab, View};

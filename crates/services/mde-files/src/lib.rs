//! MDE Files — mesh-first "Artifact Manager" for the Mackes Desktop Environment.
//!
//! Implementation contract: `docs/design/v2.0.0-mde-files/design-spec.md`.
//! Prototype: `docs/design/v2.0.0-mde-files/upstream-bundle/Artifact-Manager.html`.

pub mod a11y_labels;
pub mod app;
pub mod archive;
pub mod backend;
pub mod bookmarks;
#[cfg(feature = "dbus")]
pub mod bus_backend;
pub mod demo_data;
pub mod desktop;
pub mod fileops;
pub mod grid;
pub mod icons;
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

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
// libcosmic dependency, so it compiles under `--no-default-features` (with
// `dbus`) for headless reuse — E12's `mde-files-egui` renders it on the egui
// harness instead of the libcosmic `app` below.
pub mod a11y_labels;
pub mod archive;
pub mod backend;
pub mod bookmarks;
#[cfg(feature = "dbus")]
pub mod bus_backend;
pub mod demo_data;
/// DENSITY-SYMMETRY — density-resolving Carbon metrics for the file listing.
pub mod density;
pub mod desktop;
pub mod fileops;
pub mod grid;
#[cfg(feature = "dbus")]
pub mod mesh_backend;
pub mod mime;
pub mod model;
pub mod panels;
pub mod properties;
pub mod search;
pub mod selection;
pub mod send_to;
pub mod smb;
pub mod thumbnails;
pub mod trash;

// ── Windowed libcosmic surface (feature = "gui") ────────────────────────────
// Every module that renders through libcosmic, plus `mounts` (which resolves its
// icon through `icons`). Gated so a `default-features = false` consumer never
// pulls the toolkit.
#[cfg(feature = "gui")]
pub mod app;
/// GUI-7 — libcosmic `.sty()` styling shims (see module docs).
#[cfg(feature = "gui")]
pub mod cosmic_compat;
#[cfg(feature = "gui")]
pub mod icons;
#[cfg(feature = "gui")]
pub mod loading;
#[cfg(feature = "gui")]
pub mod mounts;
#[cfg(feature = "gui")]
pub mod picker;
#[cfg(feature = "gui")]
pub mod prefs;
#[cfg(feature = "gui")]
pub mod theme;
#[cfg(feature = "gui")]
pub mod views;
#[cfg(feature = "gui")]
pub mod widgets;

#[cfg(feature = "gui")]
pub use app::{MdeFiles, Message};
pub use backend::{
    AuditEntry, Backend, BackendError, ConflictPolicy, DemoBackend, Destination, OpId, SendMode,
};
pub use model::{FileRow, Layout, Mime, Peer, PeerKind, PeerStatus, SelfNode, Tab, View};

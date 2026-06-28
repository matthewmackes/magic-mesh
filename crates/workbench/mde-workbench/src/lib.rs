//! MCNF Workbench — the operator console, an iced GUI in
//! the IBM Carbon look (§4, tokens single-sourced in `mde-theme`)
//! running on the Cosmic desktop (E11 pivot).
//!
//! Exposes [`App`], [`Message`], and [`View`]; the sidebar is the
//! five-plane nav (Peers Front Door · This Node · Controller ·
//! Network · Fleet · Provisioning — see [`model::nav_model`]).
//! [`single_instance`] guards against duplicate processes.
//!
//! All daemon reads ride the Mackes Bus (`action/<domain>/<verb>`
//! via `mde-bus`) or shell out to `mackesd`/`meshctl` — no
//! MDE-private D-Bus (§2).

pub mod app;
pub mod backend;
/// MESH-CONNECT-DIALOG-1 — shared, panel-agnostic UI components
/// (e.g. the connect/configure progress modal).
pub mod components;
pub mod controls;
pub mod cosmic_compat;
pub mod dbus;
pub mod header;
pub mod keyboard;
pub mod launcher;
pub mod live_theme;
/// SUBSTRATE-8 — read the mesh peer directory over `action/mesh/directory`.
pub mod mesh_directory;
pub mod model;
pub mod panel_chrome;
pub mod panels;
pub mod patternfly;
pub mod role;
pub mod sidebar;
pub mod single_instance;
/// UNIFY-1 — the global status strip (top chrome band) of the Unified Workbench.
pub mod status_strip;

pub use app::{App, Message};
pub use backend::{Backend, BackendError, DemoBackend, FileBackend, RemoteBackend};
pub use dbus::{
    poll_once as focus_poll_once, serve_bus as serve_focus_bus, slug_from_body, PendingFocus,
    ACTION_TOPIC,
};
pub use model::{nav_model, Group, NavEntry, Panel, View};
pub use single_instance::{acquire as acquire_single_instance, PrimaryStatus};

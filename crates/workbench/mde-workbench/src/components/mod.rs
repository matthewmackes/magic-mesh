//! Shared, panel-agnostic UI components. Each module ships a small
//! reusable widget (state + `view`) composed by multiple panels, in the
//! IBM Carbon look (§4, tokens single-sourced in `mde-theme`).

/// MESH-CONNECT-DIALOG-1 — the connect/configure progress modal
/// (pending → success / failure) reused by the Overview, Mesh Services,
/// and Music panels so every wired button shows real progress + a
/// terminal outcome.
pub mod connect_progress;

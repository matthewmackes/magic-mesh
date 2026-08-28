//! Shared library for the `mde-enroll` join shim and the `magic-setup`
//! full-lifecycle wizard (SETUP epic). Both binaries drive these pure,
//! I/O-free state machines + the action layer; the terminal event loops live
//! in `main.rs` (mde-enroll) and `bin/magic-setup.rs` (magic-setup).

pub mod app;
/// WL-FUNC-023 S6 leftover — renderer-neutral token/capsule projection.
pub mod commissioning_view;
pub mod lifecycle_controller;
/// WL-FUNC-023 S4 — the renderer-neutral lifecycle session projection.
pub mod lifecycle_view;
pub mod public_roster;
pub mod setup;
pub mod setup_action;
/// WL-FUNC-023 S17 leftover — Status/self-test uses the live grouped plane.
pub mod wizard_status;

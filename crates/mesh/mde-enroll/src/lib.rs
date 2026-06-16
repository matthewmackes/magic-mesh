//! Shared library for the `mde-enroll` join shim and the `magic-setup`
//! full-lifecycle wizard (SETUP epic). Both binaries drive these pure,
//! I/O-free state machines + the action layer; the terminal event loops live
//! in `main.rs` (mde-enroll) and `bin/magic-setup.rs` (magic-setup).

pub mod app;
pub mod setup;
pub mod setup_action;

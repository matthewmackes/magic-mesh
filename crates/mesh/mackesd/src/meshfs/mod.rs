//! MESHFS-14.1 (v5.0.0) — LizardFS state-snapshot for the
//! `mackesd state backup` bundle.
//!
//! The first inhabitant is [`snapshot`], which shells
//! `mfsmetadump` + `mfsadmin CS-LIST` to capture enough state
//! for a bare-peer restore (`mackesd state restore <bundle>`).
//! Future commits add the `preflight` headroom check + the
//! offline-staging replay engine (MESHFS-6.1).

pub mod headroom;
pub mod snapshot;

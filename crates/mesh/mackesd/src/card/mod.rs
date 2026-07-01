//! Universal cards subsystem (Portal-31) — folded into `mackesd_core`
//! from the retired standalone `mde-card` crate (E12-14c: once the
//! Workbench retired, the daemon was its sole consumer).
//!
//! Every Object surfaced by the mesh renders as a Card: apps, files,
//! peers, activities, contacts, containers, workspaces, tray items,
//! tagged zones, and notes share one schema ([`Card`], 12 fields,
//! R5-Q3) with stable mesh-merged IDs ([`stable_id_for`], R5-Q9),
//! composition via `children: Vec<Card>` (R5-Q10), and
//! forward-compatibility across mesh-version drift via
//! `metadata: BTreeMap<String, serde_json::Value>` (R10-Q37).
//!
//! GUI-4 (sweep H3) removed the unreached Portal-era surfaces: the
//! `migration` registry, `RenderMode`, and `TemplateSpec` — consumers
//! use `schema::{Card, CardKind}` + `probe::*` only.
//!
//! (`unsafe_code` is forbidden crate-wide in `mackesd_core`'s root, so
//! the standalone crate's `#![forbid(unsafe_code)]` carries over.)

pub mod id;
pub mod probe;
pub mod schema;

pub use id::stable_id_for;
pub use probe::{
    host_card, host_facts, service_card, service_facts, HostFacts, HostSource, ServiceFacts,
};
pub use schema::{Card, CardKind};

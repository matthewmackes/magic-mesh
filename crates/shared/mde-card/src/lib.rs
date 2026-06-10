//! `mde-card` — Universal cards subsystem (Portal-31).
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

#![forbid(unsafe_code)]

pub mod id;
pub mod probe;
pub mod schema;

pub use id::stable_id_for;
pub use probe::{
    host_card, host_facts, service_card, service_facts, HostFacts, HostSource, ServiceFacts,
};
pub use schema::{Card, CardKind};

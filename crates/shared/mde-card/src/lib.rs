//! `mde-card` — Universal cards subsystem (Portal-31).
//!
//! Every Object surfaced by the Portal renders as a Card: apps, files,
//! peers, activities, contacts, containers, workspaces, tray items,
//! tagged zones, and notes all share one schema and one render
//! pipeline. The five layers of Portal (Hub / Library / Control /
//! Compact / Wallpaper) just project the same Card stream through
//! different [`RenderMode`]s.
//!
//! Acceptance (matches `docs/PROJECT_WORKLIST.md` Portal-31):
//!   - 12-field schema (`Card`), R5-Q3.
//!   - 6 render modes (`RenderMode`), R5-Q2 / R5-Q7 / R5-Q17 / R5-Q21.
//!   - Stable mesh-merged IDs (`stable_id_for`), R5-Q9.
//!   - Composition via `children: Vec<Card>`, R5-Q10.
//!   - `schema_version: u32 = SCHEMA_VERSION` (= 1) with migration
//!     registry (`migrate`), R10-Q36.
//!   - Forward-compatible across mesh-version drift via
//!     `metadata: BTreeMap<String, serde_json::Value>` capturing
//!     fields unknown to this version, R10-Q37.

#![forbid(unsafe_code)]

pub mod id;
pub mod migration;
pub mod probe;
pub mod render_mode;
pub mod schema;

pub use id::stable_id_for;
pub use migration::{migrate, MigrationError, SCHEMA_VERSION};
pub use probe::{
    host_card, host_facts, service_card, service_facts, HostFacts, HostSource, ServiceFacts,
};
pub use render_mode::RenderMode;
pub use schema::{Card, CardKind, TemplateSpec};

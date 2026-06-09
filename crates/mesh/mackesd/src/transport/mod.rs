//! KDC2-1 — `mackesd::transport` module.
//!
//! Houses the daemon-side transport orchestration: the policy
//! loader (KDC2-1.11), eventually the `KdcTls` Transport impl
//! glue, the audit-chain integration (KDC2-1.12), and the
//! per-transport observability seams.
//!
//! The trait + scorer + types live in `mackes-transport`
//! (workspace crate) so this module's surface is intentionally
//! thin — it's the wiring layer between the trait crate and
//! mackesd's existing infrastructure (SQLite store, audit chain,
//! worker supervisor).

pub mod audit;
pub mod https443;
pub mod policy;

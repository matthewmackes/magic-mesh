//! Mackes Bus — mesh-wide notification + clipboard pub/sub bus.
//!
//! v6.x BUS-1..7 epic. Locked 2026-05-25 via 104-Q poll across 26
//! rounds; full design lock at `docs/design/v6.x-mackes-bus.md`.
//!
//! This crate is the in-process library used by the `mde-bus` binary,
//! by the `mackesd::workers::bus_supervisor` subprocess supervisor,
//! and (eventually) by `crates/mde-clipd/`. It exposes:
//!
//! * [`topic`] — slash-hierarchy topic names + MQTT-wildcard matcher
//!   (BUS-1.5).
//! * [`wildcard`] — the `+` / `#` wildcard match table itself, split
//!   so test surfaces stay focused (BUS-1.5).
//! * [`seed`] — first-run idempotent seed of the 12 curated default
//!   topics (BUS-1.6).
//! * [`template`] — Tera-backed message templating with mesh
//!   variables, `{{exec 'cmd'}}` shell exec, and `{{include 'path'}}`
//!   GFS file content (BUS-1.10).
//!
//! Later BUS-1.* tasks layer ntfy broker supervision (BUS-1.2),
//! mDNS-on-Nebula discovery (BUS-1.3), SQLite+file-tree persistence
//! (BUS-1.4), subscription manifest (BUS-1.7), the full CLI (BUS-1.8),
//! and the retention engine (BUS-1.9) on top of these primitives.

pub mod audit;
pub mod broker;
pub mod cli;
pub mod correlate;
pub mod discovery;
pub mod dnd;
pub mod hooks;
pub mod persist;
pub mod retention;
pub mod rpc;
pub mod seed;
pub mod subs;
pub mod surface;
pub mod template;
pub mod topic;
pub mod wildcard;

/// Default XDG-rooted base directory for the bus on-disk state:
/// `~/.local/share/mde/bus/`. Used by [`seed`] for the first-run
/// marker, by BUS-1.4 for the per-topic file tree, and by BUS-1.7 for
/// the subscription manifest.
///
/// Returns `None` when neither `$XDG_DATA_HOME` nor `$HOME` is set
/// (e.g. the daemon was launched from a context with no user home).
/// Callers should fall back to `/var/lib/mde/bus/` in that case.
#[must_use]
pub fn default_data_dir() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("mde").join("bus"))
}

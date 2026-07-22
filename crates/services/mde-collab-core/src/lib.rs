//! `mde-collab-core` — the headless collaboration core for the Communications
//! suite (WL-FUNC-011, Phase 1): the data-flow spine + the offline-first
//! convergence engine.
//!
//! It builds **on** the [`mde_collab_types`] Phase-0 contracts (never redefining
//! them) and is a pure library — no egui, no provider HTTP, no long-running I/O
//! loop (those are Phase 2 + the mackesd collab worker). Every I/O boundary is a
//! trait a test can back with memory, and all time, ids, and signing are
//! caller-injected, so the same command/event stream replays deterministically.
//!
//! # The pieces
//!
//! * [`apply_command`] / [`ApplyCtx`] — the command → signed-events pipeline:
//!   validates membership + Owner/Member permission + the 5-minute author
//!   edit/delete window, then mints, HLC-stamps, and signs the event(s). A
//!   denied action returns a typed [`CollabError`] — visible, never a silent
//!   no-op.
//! * [`ActorLog`] ([`MemoryActorLog`], [`FileActorLog`]) — the durable,
//!   idempotent, per-space actor log (the Syncthing-replicable unit).
//! * [`Projection`] — the transactional, idempotent, convergent SQLite
//!   projection backing the [`CollabReadModel`](mde_collab_types::CollabReadModel)
//!   shapes.
//! * [`CollabEngine`] — ties the pipeline, the domain fold, the projection, and
//!   the purge gate together; its [`merge`](CollabEngine::merge) is the
//!   order-independent, signature-checked convergence engine.
//! * [`BlobStore`] ([`MemoryBlobStore`], [`FsBlobStore`]) — the SHA-256
//!   content-addressed blob store; every fetch verifies hash **and** size.
//! * [`PurgeGate`] — convergent tombstones + all-members-acked payload purge.
//!
//! # Convergence guarantee
//!
//! Two engines that have accepted the same event *set* hold byte-identical
//! projected state, regardless of the order the events arrived, whether any
//! arrived more than once, and independent of any centre — because the
//! projection rebuilds each space by folding its full log in the canonical
//! total order `(clock, event_id)`, dropping unsigned/forged events. See
//! [`Projection::dump_tables`] for the fingerprint the convergence tests
//! compare.

#![forbid(unsafe_code)]

pub mod blob;
pub mod domain;
pub mod engine;
pub mod error;
pub mod import;
pub mod log;
pub mod pipeline;
pub mod projection;
pub mod purge;
pub mod signer;

#[cfg(test)]
mod tests;

pub use blob::{default_root, verify_bytes, BlobStore, FsBlobStore, MemoryBlobStore};
pub use domain::{canonical_sort, sort_key, DomainState};
pub use engine::{CollabEngine, MergeOutcome};
pub use error::{CollabError, Result};
pub use import::{
    EditorImport, EventSink, ImportMap, ImportReport, Importer, LogSink, MemorySink,
    IMPORT_MAP_VERSION,
};
pub use log::{ActorLog, FileActorLog, MemoryActorLog};
pub use pipeline::{apply_command, ApplyCtx, EDIT_WINDOW_MS};
pub use projection::Projection;
pub use purge::PurgeGate;
pub use signer::{Ed25519Signer, EventSigner, IdSource, RandomIds};

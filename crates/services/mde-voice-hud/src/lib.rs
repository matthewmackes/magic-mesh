//! `mde-voice-hud` — the libcosmic voice HUD **and** its render-agnostic SIP core.
//!
//! The crate ships two faces over one source tree (the lib + bin + `gui`-feature
//! split `mde-files` uses):
//!
//! * **This library** — the toolkit-free core: the pure-Rust SIP register/call
//!   state machine ([`sip`]), the RTP/G.711 [`media`] engine, the mesh peer
//!   [`roster`] loader, and the dialer-target [`resolve`] heuristic. It carries
//!   **no libcosmic dependency**, so it compiles under `--no-default-features`
//!   for headless reuse — E12's `mde-voice-egui` renders this core on the shared
//!   egui harness instead of the layer-shell HUD.
//! * **The `gui` binary** (`src/main.rs`, `required-features = ["gui"]`) — the
//!   libcosmic + wlr-layer-shell softphone HUD built on top of this core.
//!
//! Everything here is the shipped VOIP-27/28/29 logic; the egui surface is glue
//! over it, not a reimplementation (governance §6).

// Every public *item* (struct/enum/fn) here is documented, but the short,
// self-evident struct/variant FIELDS (`username`, `port`, `from`, `peer`, …) are
// not — these modules were authored as a binary's private modules where the
// field-level `missing_docs` lint never fired. Allow it on the newly-exposed
// library rather than bloating the shipped enums with `/// The caller.`-style
// noise, matching the sibling forked GUI service crate `mde-files`.
#![allow(missing_docs)]

pub mod media;
pub mod resolve;
pub mod roster;
pub mod sip;

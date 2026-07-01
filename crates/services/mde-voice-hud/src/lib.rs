//! `mde-voice-hud` — the render-agnostic SIP softphone core.
//!
//! This crate is the toolkit-free voice core: the pure-Rust SIP register/call
//! state machine ([`sip`]), the RTP/G.711 [`media`] engine, the mesh peer
//! [`roster`] loader, and the dialer-target [`resolve`] heuristic. It carries
//! **no libcosmic dependency**.
//!
//! E12-14b — the libcosmic + wlr-layer-shell softphone HUD binary (`src/main.rs`)
//! was stripped. MCNF 12.0 "Quasar" renders Voice as an egui panel
//! (`mde-voice-egui::voice_panel`, pumped by `voice_pump`) inside
//! `mde-shell-egui`, reusing this core — including the persistent SIP agent
//! ([`sip::run_agent`]) that the shell spawns once at start — on the shared egui
//! harness instead of the retired layer-shell HUD + its `--agent` autostart.
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

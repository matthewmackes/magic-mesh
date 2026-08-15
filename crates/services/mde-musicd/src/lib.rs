//! `mde-musicd` — native Airsonic music daemon for MDE (AIR-* / v6.1).
//!
//! AIR-4 slice: the Subsonic/Airsonic REST [`airsonic`] client + the
//! mesh-shared [`creds`] loader. The `mde-musicd` binary's `ping`
//! subcommand exercises both end-to-end (the runtime entry point that
//! makes these modules reachable per §0.12).
//!
//! Later AIR tasks layer on top:
//! * AIR-2/6 — the zbus `dev.mackes.MDE.Music` + MPRIS surfaces.
//! * AIR-5 — native gapless playback ([`engine`]: Symphonia decode →
//!   cpal/ALSA→PipeWire output), reachable via `mde-musicd play`.
//! * AIR-7/8 — mesh-shared cache + exclusive-playback handoff.

pub mod airsonic;
pub mod bus_responder;
pub mod cache;
pub mod cast;
pub mod clock_audio;
pub mod creds;
pub mod domain;
pub mod engine;
pub mod mpris;
pub mod queue;
pub mod reconnect;
pub mod seat_audio;
pub mod state;

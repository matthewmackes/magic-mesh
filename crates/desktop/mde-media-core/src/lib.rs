//! `mde-media-core` — the libmpv-backed engine + player core (MEDIA-1,
//! `docs/design/mesh-media-player.md`).
//!
//! MCNF's media player (MEDIA epic) is a VLC-class native player driven by
//! **mpv**. This crate is the load-bearing *engine core*: it wraps mpv behind a
//! narrow, injectable [`MediaEngine`] seam and drives a [`Player`] state machine
//! over it. No GUI lives here — the `mde-media-egui` surface (MEDIA-8) and the
//! AV-integration units (MEDIA-2/3/4) build on this.
//!
//! # The seam (§6 glue, testable without mpv)
//!
//! - [`MediaEngine`] is the one interface between the player and the decoder.
//!   Every method is a single mpv command / property / event — this is glue over
//!   mpv, **not** a reimplemented decoder.
//! - [`crate::mpv::MpvEngine`] (feature `mpv`) is the real implementation. It
//!   links the system `libmpv`, so it is OFF by default and honest-gated to a
//!   host that carries `mpv-libs-devel` — see the crate `Cargo.toml`.
//! - [`FakeMpv`] is a deterministic in-tree engine. It is what the unit tests and
//!   the default `media-smoke` binary drive, so the whole state machine is
//!   exercised with **no system libmpv** — the airgap-safe path.
//!
//! # The player
//!
//! [`Player`] owns the transport verbs (`load`/`play`/`pause`/`seek`/`stop`), the
//! authoritative [`PlayerState`] (`Idle`→`Loading`→`Playing`⇄`Paused`→
//! `Stopped`/`Ended`), the live position/duration, the enumerated [`Track`]s, and
//! an ordered [`PlayerEvent`] stream the surface renders from.
//!
//! ```
//! use mde_media_core::{FakeMpv, Player, PlayerState};
//!
//! let mut player = Player::new(FakeMpv::new().with_duration(90.0));
//! player.load("test://clip.mkv").expect("load");
//! player.pump(); // fold in the engine's FileLoaded
//! assert_eq!(player.state(), PlayerState::Playing);
//! player.pause().expect("pause");
//! assert_eq!(player.state(), PlayerState::Paused);
//! ```

// Pragmatic pedantic allows: the type names intentionally echo their module
// (`PlayerState` in `player`), and the pure getters are convenience accessors
// rather than a `#[must_use]`-critical API surface.
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

pub mod engine;
pub mod fake;
pub mod player;

#[cfg(feature = "mpv")]
pub mod mpv;

pub use engine::{EndReason, EngineError, EngineSignal, MediaEngine, Track, TrackKind};
pub use fake::FakeMpv;
pub use player::{Player, PlayerError, PlayerEvent, PlayerState};

//! The audio-mixer client seam — the DAW-authentic mixer's data source (lock 4).
//!
//! The real `PipeWire` graph binding is **E12-16's** work. Until it lands, the
//! bound impl here is [`UnboundMixer`]: every call answers a typed
//! [`SeatError::Unavailable`] naming `PipeWire` — the honest "no mixer yet" state
//! the System surface's Mixer section renders (§7), never fake strips at fake
//! levels. The [`MixerStrip`] / [`MixerStatus`] model + the [`MixerClient`] seam
//! are fixed now so E12-16 swaps only the impl. A strip's `origin` already models
//! the full lock-4 span (host session · local VM · mesh-remote stream).

use crate::error::{Backend, SeatError};

/// Where a mixer strip's audio comes from — the lock-4 span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripOrigin {
    /// The local host session (musicd / voice / a host app).
    HostSession,
    /// A local VM session, by its VM name.
    LocalVm(String),
    /// A mesh-remote peer's audio stream, by peer node id.
    MeshRemote(String),
}

/// One mixer channel strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixerStrip {
    /// A stable id for the strip (the `PipeWire` node id, once bound).
    pub id: String,
    /// Operator-facing name (application / VM / peer label).
    pub name: String,
    /// Where the audio originates.
    pub origin: StripOrigin,
    /// Volume 0–100.
    pub volume: u8,
    /// Muted.
    pub muted: bool,
}

/// The whole mixer state: the master strip plus every channel strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixerStatus {
    /// The master output strip.
    pub master: MixerStrip,
    /// Every channel strip (host / VM / mesh-remote).
    pub strips: Vec<MixerStrip>,
}

/// The mixer seam. Production impl (E12-16) drives the `PipeWire` graph; today
/// [`UnboundMixer`] is the honest not-yet-bound impl.
pub trait MixerClient: Send {
    /// Read the whole mixer state.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] until the `PipeWire` binding lands (E12-16) or
    /// when no `PipeWire` daemon is running.
    fn status(&self) -> Result<MixerStatus, SeatError>;

    /// Set a strip's volume (0–100).
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] until E12-16.
    fn set_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError>;

    /// Set a strip's mute.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] until E12-16.
    fn set_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError>;
}

/// The not-yet-bound mixer client: a typed [`SeatError::Unavailable`] for every
/// call (the real `PipeWire` binding is E12-16).
///
/// A deliberate honest seam — the Mixer section shows "audio graph not
/// available" rather than fake faders.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnboundMixer;

impl UnboundMixer {
    const REASON: &'static str = "the PipeWire graph binding lands in E12-16";

    fn unavailable() -> SeatError {
        SeatError::Unavailable {
            backend: Backend::PipeWire,
            reason: Self::REASON.to_owned(),
        }
    }
}

impl MixerClient for UnboundMixer {
    fn status(&self) -> Result<MixerStatus, SeatError> {
        Err(Self::unavailable())
    }

    fn set_volume(&self, _strip_id: &str, _volume: u8) -> Result<(), SeatError> {
        Err(Self::unavailable())
    }

    fn set_muted(&self, _strip_id: &str, _muted: bool) -> Result<(), SeatError> {
        Err(Self::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbound_mixer_is_honestly_unavailable_not_fake_strips() {
        let m = UnboundMixer;
        let e = m.status().expect_err("must not fabricate strips");
        assert_eq!(e.backend(), Backend::PipeWire);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
        assert!(matches!(
            m.set_volume("42", 60),
            Err(SeatError::Unavailable { .. })
        ));
        assert!(matches!(
            m.set_muted("42", true),
            Err(SeatError::Unavailable { .. })
        ));
    }

    #[test]
    fn strip_origin_models_the_full_lock4_span() {
        // The three origins the mixer must cover exist in the model now, so
        // E12-16 populates them rather than widening the type.
        let origins = [
            StripOrigin::HostSession,
            StripOrigin::LocalVm("win10".into()),
            StripOrigin::MeshRemote("nyc3".into()),
        ];
        assert_eq!(origins.len(), 3);
    }
}

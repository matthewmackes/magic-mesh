//! The DDC/CI client seam — external-monitor brightness over i2c-dev (lock 13,
//! the external-monitor half; internal panels are [`crate::backlight`]).
//!
//! The real i2c/DDC binding is **E12-18's** work. Until it lands, the bound impl
//! here is [`UnboundDdc`]: every call answers a typed [`SeatError::Unavailable`]
//! naming DDC/CI — the honest "not controllable yet on this build" state the
//! Display section renders (§7 / interlock 4), never a fake slider that pretends
//! to move an external monitor. The [`DdcDisplay`] model and the [`DdcClient`]
//! seam are fixed now so E12-18 only swaps the impl, not the surface.

use crate::error::{Backend, SeatError};

/// One DDC/CI-controllable external display (an i2c-attached monitor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdcDisplay {
    /// The i2c bus label (`i2c-<n>`) the monitor answers on.
    pub bus: String,
    /// The monitor model string from its EDID, when read.
    pub model: Option<String>,
    /// Current brightness (VCP 0x10), 0–100.
    pub brightness: u8,
}

/// The DDC/CI seam. Production impl (E12-18) drives i2c-dev; today [`UnboundDdc`]
/// is the honest not-yet-bound impl.
pub trait DdcClient: Send {
    /// Enumerate DDC/CI-controllable external monitors.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] until the i2c binding lands (E12-18) or when no
    /// i2c-dev bus answers DDC.
    fn displays(&self) -> Result<Vec<DdcDisplay>, SeatError>;

    /// Set an external monitor's brightness (VCP 0x10), 0–100.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] until E12-18; [`SeatError::OutOfRange`] if the
    /// percentage exceeds 100 once bound.
    fn set_brightness(&self, bus: &str, percent: u8) -> Result<(), SeatError>;
}

/// The not-yet-bound DDC client: answers a typed [`SeatError::Unavailable`] for
/// every call (the real i2c binding is E12-18).
///
/// This is a deliberate honest seam, not a stub that lies — the Display section
/// shows "external brightness: not controllable" rather than a dead control.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnboundDdc;

impl UnboundDdc {
    /// The reason string every call reports.
    const REASON: &'static str = "the i2c/DDC binding lands in E12-18";

    fn unavailable() -> SeatError {
        SeatError::Unavailable {
            backend: Backend::Ddc,
            reason: Self::REASON.to_owned(),
        }
    }
}

impl DdcClient for UnboundDdc {
    fn displays(&self) -> Result<Vec<DdcDisplay>, SeatError> {
        Err(Self::unavailable())
    }

    fn set_brightness(&self, _bus: &str, _percent: u8) -> Result<(), SeatError> {
        Err(Self::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbound_ddc_is_honestly_unavailable_not_a_lie() {
        let d = UnboundDdc;
        let e = d.displays().expect_err("must not fabricate monitors");
        assert_eq!(e.backend(), Backend::Ddc);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
        let e = d
            .set_brightness("i2c-4", 50)
            .expect_err("must not fake a write");
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
    }
}

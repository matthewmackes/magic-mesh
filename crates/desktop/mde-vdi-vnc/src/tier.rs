//! [`QualityTier`] → the RFB knobs this client actually has.
//!
//! RFB is client-steered **at runtime**: `SetPixelFormat` (RFC 6143 §7.5.1)
//! changes the wire pixel layout of every subsequent `FramebufferUpdate`,
//! `SetEncodings` (§7.5.2) restates the client's encoding preference, and —
//! because a server only sends updates in response to
//! `FramebufferUpdateRequest` — the request cadence is the client's own rate
//! control. All three apply mid-session over the live connection, so the VNC
//! backend's tiers are honestly [`TierApplication::Live`]
//! ([`VncTierSettings::APPLICATION`]).
//!
//! What each tier changes:
//!
//! * **Pixel depth** — 32-bpp true colour → 16-bpp RGB565 → 8-bpp BGR233: the
//!   real bandwidth knob, since every byte of every rectangle scales with it.
//! * **Update pacing** — [`VncTierSettings::update_interval_ms`], the minimum
//!   `FramebufferUpdateRequest` spacing the transport must honour: the
//!   RFB-native way to shed update rate on a weak link. It is also the only
//!   axis separating [`QualityTier::Compressed`] from
//!   [`QualityTier::Minimal`]: 8-bpp is the lightest layout the RFB
//!   true-colour model expresses, so the last rung slows the clock instead of
//!   pretending a lighter encoding exists.
//! * **Encoding preference** — always the compact-first list
//!   ([`PREFERRED_ENCODINGS`]): all four supported encodings are lossless, so
//!   there is nothing to trade per tier; the list is re-sent with every change
//!   so a tier announcement is complete and self-contained.

use crate::encoding::Encoding;
use crate::link::{QualityTier, TierApplication};
use crate::pixel::PixelFormat;

/// The compact-first encoding preference announced at every tier: prefer the
/// cheap on-screen copy, then the tiled/run-length encoders, with Raw last.
pub const PREFERRED_ENCODINGS: [Encoding; 4] = [
    Encoding::CopyRect,
    Encoding::Hextile,
    Encoding::Rre,
    Encoding::Raw,
];

/// The RFB-side settings one [`QualityTier`] maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VncTierSettings {
    /// Wire pixel layout to request via `SetPixelFormat`.
    pub pixel_format: PixelFormat,
    /// Encoding preference to (re-)announce via `SetEncodings`.
    pub encodings: [Encoding; 4],
    /// Minimum `FramebufferUpdateRequest` spacing the transport must honour.
    pub update_interval_ms: u64,
}

impl VncTierSettings {
    /// RFB applies tiers live, mid-session (see the module docs).
    pub const APPLICATION: TierApplication = TierApplication::Live;

    /// The settings for `tier`.
    #[must_use]
    pub const fn for_tier(tier: QualityTier) -> Self {
        let (pixel_format, update_interval_ms) = match tier {
            QualityTier::Full => (PixelFormat::rgba8888(), 16), // ~60 Hz
            QualityTier::Reduced => (PixelFormat::rgb565(), 33), // ~30 Hz
            QualityTier::Compressed => (PixelFormat::bgr233(), 66), // ~15 Hz
            QualityTier::Minimal => (PixelFormat::bgr233(), 200), // ~5 Hz
        };
        Self {
            pixel_format,
            encodings: PREFERRED_ENCODINGS,
            update_interval_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Encoding, VncTierSettings, PREFERRED_ENCODINGS};
    use crate::link::{QualityTier, TierApplication};
    use crate::pixel::PixelFormat;

    #[test]
    fn vnc_tiers_apply_live() {
        assert_eq!(VncTierSettings::APPLICATION, TierApplication::Live);
    }

    #[test]
    fn tiers_map_to_strictly_lighter_wire_settings() {
        let tiers = QualityTier::ALL.map(VncTierSettings::for_tier);
        assert_eq!(tiers[0].pixel_format, PixelFormat::rgba8888());
        assert_eq!(tiers[1].pixel_format, PixelFormat::rgb565());
        assert_eq!(tiers[2].pixel_format, PixelFormat::bgr233());
        assert_eq!(tiers[3].pixel_format, PixelFormat::bgr233());
        // Bytes per wire pixel never grow, pacing never speeds up.
        for pair in tiers.windows(2) {
            assert!(
                pair[1].pixel_format.bytes_per_pixel() <= pair[0].pixel_format.bytes_per_pixel()
            );
            assert!(pair[1].update_interval_ms >= pair[0].update_interval_ms);
        }
        // The last rung is pacing-only: 8-bpp is already the RFB floor.
        assert!(tiers[3].update_interval_ms > tiers[2].update_interval_ms);
    }

    #[test]
    fn every_tier_announces_the_compact_first_preference() {
        for tier in QualityTier::ALL {
            let s = VncTierSettings::for_tier(tier);
            assert_eq!(s.encodings, PREFERRED_ENCODINGS);
            assert!(s.pixel_format.is_supported(), "our decoder must decode it");
        }
        assert_eq!(PREFERRED_ENCODINGS[0], Encoding::CopyRect);
        assert_eq!(PREFERRED_ENCODINGS[3], Encoding::Raw);
    }
}

//! [`QualityTier`] → the RDP knobs the pinned `ironrdp` stack actually exposes.
//!
//! RDP negotiates its encoding surface during the **connection sequence**:
//! colour depth and bitmap codecs ride the GCC conference-create blocks and
//! the capability exchange, and the performance/compression flags ride the
//! Client Info PDU. The pinned `ironrdp` releases (connector 0.9 / session
//! 0.10 / pdu 0.8) expose exactly those connect-time knobs on
//! [`ironrdp_connector::Config`] — `bitmap` ([`BitmapConfig`]),
//! `performance_flags`, `compression_type` — and **no client-driven
//! mid-session re-negotiation**: a capability re-exchange
//! (Deactivation-Reactivation) is server-initiated, and the client-side
//! runtime PDUs `ironrdp` builds (input, frame-acknowledge, suppress-output,
//! refresh-rectangle) change *what* is requested, never how it is encoded.
//!
//! A tier change on the RDP backend is therefore honestly typed
//! [`TierApplication::OnReconnect`] ([`RdpTierSettings::APPLICATION`]):
//! [`crate::RdpSession`] moves its *target* tier and raises
//! `needs_reconnect`; the connect layer builds the next connection from
//! [`RdpTierSettings`] and then calls `mark_tier_applied`. Nothing pretends
//! to switch mid-session.
//!
//! One more honest bound: the pinned connector rejects any colour depth other
//! than 15/16/24/32 when building the GCC blocks, so the lightest expressible
//! depth is **15-bpp RGB555** — [`QualityTier::Minimal`] uses that, not 8-bpp.

use crate::link::{QualityTier, TierApplication};
use ironrdp_connector::BitmapConfig;
use ironrdp_pdu::rdp::capability_sets::{client_codecs_capabilities, BitmapCodecs};
use ironrdp_pdu::rdp::client_info::{CompressionType, PerformanceFlags};

/// The connect-time RDP settings one [`QualityTier`] maps to.
///
/// The (feature-gated) connect layer copies these onto
/// [`ironrdp_connector::Config`]: [`RdpTierSettings::bitmap_config`] →
/// `Config::bitmap`, [`RdpTierSettings::performance_flags`] →
/// `Config::performance_flags`, [`RdpTierSettings::bulk_compression`] →
/// `Config::compression_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RdpTierSettings {
    /// Preferred colour depth in the GCC core data (32 / 16 / 15 — the values
    /// the pinned connector accepts; 24 is skipped because it costs like 32
    /// without the `WANT_32_BPP_SESSION` fast path).
    pub color_depth: u32,
    /// Allow lossy bitmap compression (`BitmapConfig::lossy_compression`).
    pub lossy_bitmap_compression: bool,
    /// Advertise the `RemoteFX` codec. Off on the lightest tier so the server
    /// falls back to plain (bulk-compressed) bitmaps at the reduced depth.
    pub remotefx: bool,
    /// Client Info performance flags — server-side eye-candy the tier sheds.
    pub performance_flags: PerformanceFlags,
    /// Bulk (MPPC/NCRUSH/XCRUSH) compression to negotiate; richer levels on
    /// weaker links trade client CPU for wire bytes.
    pub bulk_compression: Option<CompressionType>,
}

impl RdpTierSettings {
    /// RDP applies tiers at connect time only (see the module docs): every
    /// tier change is gated on a reconnect, and the session API says so.
    pub const APPLICATION: TierApplication = TierApplication::OnReconnect;

    /// The settings for `tier`.
    #[must_use]
    pub fn for_tier(tier: QualityTier) -> Self {
        let disable_eyecandy = PerformanceFlags::DISABLE_WALLPAPER
            | PerformanceFlags::DISABLE_FULLWINDOWDRAG
            | PerformanceFlags::DISABLE_MENUANIMATIONS;
        let disable_everything = disable_eyecandy
            | PerformanceFlags::DISABLE_THEMING
            | PerformanceFlags::DISABLE_CURSOR_SHADOW
            | PerformanceFlags::DISABLE_CURSORSETTINGS;
        match tier {
            QualityTier::Full => Self {
                color_depth: 32,
                lossy_bitmap_compression: false,
                remotefx: true,
                performance_flags: PerformanceFlags::ENABLE_FONT_SMOOTHING
                    | PerformanceFlags::ENABLE_DESKTOP_COMPOSITION,
                bulk_compression: None,
            },
            QualityTier::Reduced => Self {
                color_depth: 16,
                lossy_bitmap_compression: true,
                remotefx: true,
                performance_flags: disable_eyecandy | PerformanceFlags::ENABLE_FONT_SMOOTHING,
                bulk_compression: Some(CompressionType::K64),
            },
            QualityTier::Compressed => Self {
                color_depth: 16,
                lossy_bitmap_compression: true,
                remotefx: true,
                performance_flags: disable_everything,
                bulk_compression: Some(CompressionType::Rdp6),
            },
            QualityTier::Minimal => Self {
                // 15-bpp RGB555 is the floor the pinned connector accepts.
                color_depth: 15,
                lossy_bitmap_compression: true,
                remotefx: false,
                performance_flags: disable_everything,
                bulk_compression: Some(CompressionType::Rdp61),
            },
        }
    }

    /// The [`ironrdp_connector::Config::bitmap`] value for this tier.
    #[must_use]
    pub fn bitmap_config(&self) -> BitmapConfig {
        BitmapConfig {
            lossy_compression: self.lossy_bitmap_compression,
            color_depth: self.color_depth,
            codecs: self.codecs(),
        }
    }

    /// The bitmap-codec capability set: `RemoteFX` advertised on the richer
    /// tiers, none (plain bitmaps) on [`QualityTier::Minimal`].
    #[must_use]
    pub fn codecs(&self) -> BitmapCodecs {
        let wanted: &[&str] = if self.remotefx {
            &["remotefx:on"]
        } else {
            &["remotefx:off"]
        };
        // The parser only errors on an unknown codec name; both inputs are
        // fixed known strings, so the fallback (advertise nothing = plain
        // bitmaps) is unreachable in practice but keeps this panic-free.
        client_codecs_capabilities(wanted).unwrap_or_else(|_| BitmapCodecs(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::{CompressionType, PerformanceFlags, RdpTierSettings, TierApplication};
    use crate::link::QualityTier;

    #[test]
    fn rdp_tiers_are_reconnect_gated() {
        assert_eq!(RdpTierSettings::APPLICATION, TierApplication::OnReconnect);
    }

    #[test]
    fn tiers_map_to_strictly_lighter_connect_settings() {
        let full = RdpTierSettings::for_tier(QualityTier::Full);
        let reduced = RdpTierSettings::for_tier(QualityTier::Reduced);
        let compressed = RdpTierSettings::for_tier(QualityTier::Compressed);
        let minimal = RdpTierSettings::for_tier(QualityTier::Minimal);

        assert_eq!(
            [32, 16, 16, 15],
            [
                full.color_depth,
                reduced.color_depth,
                compressed.color_depth,
                minimal.color_depth
            ],
            "depth only ever shrinks; 15 is the connector's floor"
        );
        assert!(!full.lossy_bitmap_compression, "full tier stays lossless");
        assert!(reduced.lossy_bitmap_compression);
        assert_eq!(full.bulk_compression, None);
        assert_eq!(reduced.bulk_compression, Some(CompressionType::K64));
        assert_eq!(compressed.bulk_compression, Some(CompressionType::Rdp6));
        assert_eq!(minimal.bulk_compression, Some(CompressionType::Rdp61));
        assert!(
            full.performance_flags
                .contains(PerformanceFlags::ENABLE_FONT_SMOOTHING),
            "full keeps the eye candy"
        );
        assert!(
            minimal
                .performance_flags
                .contains(PerformanceFlags::DISABLE_THEMING),
            "minimal sheds it"
        );
    }

    #[test]
    fn remotefx_is_advertised_on_rich_tiers_and_dropped_on_minimal() {
        assert!(
            !RdpTierSettings::for_tier(QualityTier::Full)
                .codecs()
                .0
                .is_empty(),
            "full advertises RemoteFX"
        );
        assert!(
            RdpTierSettings::for_tier(QualityTier::Minimal)
                .codecs()
                .0
                .is_empty(),
            "minimal advertises no codec (plain bitmaps)"
        );
    }

    #[test]
    fn bitmap_config_carries_the_tier() {
        let cfg = RdpTierSettings::for_tier(QualityTier::Reduced).bitmap_config();
        assert_eq!(cfg.color_depth, 16);
        assert!(cfg.lossy_compression);
        assert!(!cfg.codecs.0.is_empty());
    }
}

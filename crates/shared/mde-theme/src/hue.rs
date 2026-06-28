//! MESHMAP-2 (W2) — the stable per-node hue wheel.
//!
//! The mesh-map wallpaper + Peers Map color each node by a **stable, distinct
//! hue derived from its hostname** so a node reads the same color across
//! reboots and across every peer's map (an EtherApe-like identity color). §4
//! requires all color math live in `mde-theme` (no raw hex / scattered HSL
//! literals in the panels), so the hash→hue→RGB derivation is single-sourced
//! here.
//!
//! The saturation + lightness are **fixed Carbon-derived constants** ([`NODE_SAT`]
//! / [`NODE_LIGHT`]) chosen to sit on the same on-dark band as the Carbon support
//! ramp (Green 50 / Blue 50 / Yellow 30 read at roughly this S/L on Gray 100), so
//! a generated node hue is a sibling of the named tokens rather than a clashing
//! fully-saturated primary. Only the **hue** varies per node; saturation and
//! lightness are constant, which is what keeps the wheel visually coherent.

use crate::color::Rgba;

/// Node-hue saturation (0.0..=1.0).
///
/// A Carbon-restrained 0.62 — vivid enough to separate adjacent hues on Gray
/// 100 without the neon over-saturation Carbon avoids (the support ramp sits in
/// this band, e.g. Green 50 ≈ S 0.63).
pub const NODE_SAT: f32 = 0.62;

/// Node-hue lightness (0.0..=1.0).
///
/// 0.62 keeps every generated hue legible on the Gray 100 (#161616) ground and
/// bright enough for a 2 px particle dot, matching the on-dark lightness of the
/// Carbon support steps.
pub const NODE_LIGHT: f32 = 0.62;

/// FNV-1a hash of a hostname → a 32-bit seed (the same hash style the map's
/// `seed_angle` uses, so the seed is stable + cheap + dependency-free). Pure.
#[must_use]
pub fn hostname_seed(hostname: &str) -> u32 {
    hostname.bytes().fold(2_166_136_261_u32, |acc, b| {
        (acc ^ u32::from(b)).wrapping_mul(16_777_619)
    })
}

/// Map a 32-bit seed onto the hue wheel (0..360°), resolved to an `Rgba`.
///
/// Resolved at the fixed Carbon node saturation/lightness
/// ([`NODE_SAT`]/[`NODE_LIGHT`]). Deterministic + pure: the same seed always
/// yields the same color.
///
/// This is the single color source for a mesh-map node's identity hue (W2). The
/// presence (online/idle/offline) is rendered separately as the node's *ring*;
/// the hue is the node's *fill* + the color of the packet particles it sends.
#[must_use]
pub fn node_hue(seed: u32) -> Rgba {
    // Spread the seed across the wheel. Multiplying by the golden-ratio
    // conjugate (≈0.618 of 2^32) before taking the hue decorrelates nearby
    // seeds (adjacent hostnames land far apart on the wheel) — the classic
    // low-discrepancy hue assignment.
    let golden = seed.wrapping_mul(2_654_435_769); // ⌊φ⁻¹·2³²⌋
    let hue_deg = (golden as f32 / u32::MAX as f32) * 360.0;
    hsl_to_rgba(hue_deg, NODE_SAT, NODE_LIGHT)
}

/// Convenience: the stable node hue for a hostname (hash then wheel). Pure.
#[must_use]
pub fn node_hue_for(hostname: &str) -> Rgba {
    node_hue(hostname_seed(hostname))
}

/// HSL → `Rgba` (hue in degrees 0..360, saturation/lightness 0.0..=1.0).
///
/// Full opacity; the standard piecewise conversion, pure + branch-stable. Kept
/// here (not at a call site) so §4's "no scattered color math" holds.
#[must_use]
pub fn hsl_to_rgba(hue_deg: f32, sat: f32, light: f32) -> Rgba {
    let h = hue_deg.rem_euclid(360.0) / 360.0;
    let s = sat.clamp(0.0, 1.0);
    let l = light.clamp(0.0, 1.0);
    if s <= f32::EPSILON {
        let v = chan_u8(l);
        return Rgba::rgb(v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        s.mul_add(-l, l + s)
    };
    let p = 2.0f32.mul_add(l, -q);
    let r = hue_channel(p, q, h + 1.0 / 3.0);
    let g = hue_channel(p, q, h);
    let b = hue_channel(p, q, h - 1.0 / 3.0);
    Rgba::rgb(chan_u8(r), chan_u8(g), chan_u8(b))
}

/// Quantize a 0.0..=1.0 channel to an 8-bit value (round, then clamp into
/// range so the `as u8` cast is always well-defined).
fn chan_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// One channel of the HSL→RGB piecewise function (`t` is the hue-shifted point).
fn hue_channel(p: f32, q: f32, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        ((q - p) * 6.0).mul_add(t, p)
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        ((q - p) * (2.0 / 3.0 - t)).mul_add(6.0, p)
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn node_hue_is_deterministic() {
        // W2 — the same hostname always resolves to the same color (stable
        // across reboots / across every peer's map).
        for host in ["anvil", "forge", "pine", "lighthouse-nyc3", "self-laptop"] {
            assert_eq!(
                node_hue_for(host),
                node_hue_for(host),
                "{host} hue must be deterministic"
            );
        }
    }

    #[test]
    fn distinct_hostnames_get_distinct_hues() {
        // W2 — distinct nodes must read as distinct colors. Across a realistic
        // 12-node workgroup (the §8 envelope) every hue is unique.
        let hosts = [
            "anvil",
            "forge",
            "pine",
            "oak",
            "elm",
            "birch",
            "cedar",
            "maple",
            "ash",
            "willow",
            "lighthouse-nyc3",
            "lighthouse-fra1",
        ];
        let colors: HashSet<(u8, u8, u8)> = hosts
            .iter()
            .map(|h| {
                let c = node_hue_for(h);
                (c.r, c.g, c.b)
            })
            .collect();
        assert_eq!(
            colors.len(),
            hosts.len(),
            "every node in the workgroup envelope must get a distinct hue"
        );
    }

    #[test]
    fn fixed_saturation_and_lightness_band() {
        // The hues vary, but saturation/lightness stay on the Carbon band: no
        // generated hue is pure black/white or a blown-out primary. Assert the
        // mid-band lightness (no channel pinned at 0 or 255 across the wheel,
        // and not all-equal which would mean a gray).
        for deg in (0..360).step_by(17) {
            let c = hsl_to_rgba(deg as f32, NODE_SAT, NODE_LIGHT);
            let gray = c.r == c.g && c.g == c.b;
            assert!(!gray, "a saturated hue at {deg}° must not be gray");
            // Mid-band lightness keeps it off the pure extremes.
            assert!(
                c.r < 255 || c.g < 255 || c.b < 255,
                "hue at {deg}° not blown to white"
            );
        }
    }

    #[test]
    fn hsl_primaries_match_expected_rgb() {
        // Sanity-check the conversion against known HSL → RGB points.
        // Pure red at full sat / 50% light = (255,0,0).
        let red = hsl_to_rgba(0.0, 1.0, 0.5);
        assert_eq!((red.r, red.g, red.b), (255, 0, 0));
        // Pure green at 120°.
        let green = hsl_to_rgba(120.0, 1.0, 0.5);
        assert_eq!((green.r, green.g, green.b), (0, 255, 0));
        // Pure blue at 240°.
        let blue = hsl_to_rgba(240.0, 1.0, 0.5);
        assert_eq!((blue.r, blue.g, blue.b), (0, 0, 255));
        // Zero saturation → gray regardless of hue.
        let gray = hsl_to_rgba(200.0, 0.0, 0.5);
        assert_eq!(gray.r, gray.g);
        assert_eq!(gray.g, gray.b);
    }

    #[test]
    fn hue_wraps_negative_and_over_360() {
        // Hue is taken mod 360, so 360+30 == 30 and -30 == 330.
        assert_eq!(hsl_to_rgba(390.0, 0.6, 0.6), hsl_to_rgba(30.0, 0.6, 0.6));
        assert_eq!(hsl_to_rgba(-30.0, 0.6, 0.6), hsl_to_rgba(330.0, 0.6, 0.6));
    }
}

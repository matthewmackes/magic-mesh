//! Primitive RGBA color type. No Iced runtime dependency in the
//! default build; convert via `Rgba::into_iced_color()` when the
//! `iced` cargo feature is on.

use std::fmt;

/// 8-bit RGB + f32 alpha (0.0..=1.0). All palette and shadow
/// tokens are expressed as `Rgba`.
#[derive(Clone, Copy, PartialEq)]
pub struct Rgba {
    /// Red channel, 0..=255.
    pub r: u8,
    /// Green channel, 0..=255.
    pub g: u8,
    /// Blue channel, 0..=255.
    pub b: u8,
    /// Alpha, 0.0..=1.0.
    pub a: f32,
}

impl Rgba {
    /// Construct from 8-bit RGB with full opacity.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Construct from 8-bit RGBA.
    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Build a copy with the alpha replaced.
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Parse a `#rrggbb` hex string. Returns `None` on malformed
    /// input. `const` would be nice here but `from_str_radix` is
    /// not yet stable in const contexts; runtime parse is fine
    /// for tokens read once at startup.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Self::rgb(r, g, b))
    }
}

impl fmt::Debug for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Rgba(#{:02x}{:02x}{:02x} @ {:.2})",
            self.r, self.g, self.b, self.a
        )
    }
}

#[cfg(feature = "iced")]
impl Rgba {
    /// Convert to an Iced color value. Available when the `iced`
    /// feature is enabled.
    pub fn into_iced_color(self) -> iced_core::Color {
        iced_core::Color {
            r: (self.r as f32) / 255.0,
            g: (self.g as f32) / 255.0,
            b: (self.b as f32) / 255.0,
            a: self.a,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_round_trips() {
        let c = Rgba::from_hex("#5b6af5").unwrap();
        assert_eq!(c.r, 0x5b);
        assert_eq!(c.g, 0x6a);
        assert_eq!(c.b, 0xf5);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn from_hex_rejects_bad_length() {
        assert!(Rgba::from_hex("#5b6af").is_none());
        assert!(Rgba::from_hex("#5b6af55").is_none());
        assert!(Rgba::from_hex("").is_none());
    }

    #[test]
    fn with_alpha_preserves_channels() {
        let c = Rgba::rgb(0x5b, 0x6a, 0xf5).with_alpha(0.08);
        assert_eq!(c.r, 0x5b);
        assert_eq!(c.a, 0.08);
    }
}

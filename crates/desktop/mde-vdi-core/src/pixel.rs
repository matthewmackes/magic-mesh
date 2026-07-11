//! The shared RGBA8 desktop surface the VDI framebuffers are built on.
//!
//! Each transport owns a `Framebuffer` with protocol-specific mutation — RDP
//! stride-blits decoded bitmap rectangles, VNC accumulates RFB
//! Raw/CopyRect/RRE/Hextile rectangles, SPICE replaces the whole primary surface —
//! but they all **store** the desktop identically (`width × height × 4` canonical,
//! tightly-packed RGBA8) and all **convert** it to an [`egui::ColorImage`] the same
//! way. That shared storage + conversion is [`RgbaSurface`], so the opaque-black
//! init and the egui hand-off are written once. The divergent blits stay in each
//! crate and paint into the surface through [`RgbaSurface::rgba_mut`].

use crate::egui::ColorImage;

/// A persistent, tightly-packed RGBA8 desktop surface + the egui hand-off.
///
/// Construct it once at the desktop size with [`RgbaSurface::new`] (opaque black);
/// each transport's `Framebuffer` wraps one and paints decoded pixels into
/// [`rgba_mut`](RgbaSurface::rgba_mut). The buffer is always exactly
/// `width * height * 4` bytes, so [`to_color_image`](RgbaSurface::to_color_image) is
/// a straight hand-off to egui.
#[derive(Clone)]
pub struct RgbaSurface {
    width: usize,
    height: usize,
    /// Tightly packed RGBA8, exactly `width * height * 4` bytes.
    rgba: Vec<u8>,
}

impl RgbaSurface {
    /// Bytes per pixel — always 4 for the canonical RGBA8 store.
    pub const BYTES_PER_PIXEL: usize = 4;

    /// A new opaque-black surface of `width × height`.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        let mut rgba = vec![0u8; width * height * Self::BYTES_PER_PIXEL];
        // Opaque black: every 4th byte (alpha) = 0xFF.
        for a in rgba.iter_mut().skip(3).step_by(Self::BYTES_PER_PIXEL) {
            *a = 0xFF;
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    /// Surface width in pixels.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Surface height in pixels.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Surface size as egui's `[w, h]`.
    #[must_use]
    pub const fn size(&self) -> [usize; 2] {
        [self.width, self.height]
    }

    /// Borrow the raw canonical RGBA bytes (conversion / testing / zero-copy).
    #[must_use]
    pub fn rgba_bytes(&self) -> &[u8] {
        &self.rgba
    }

    /// Mutably borrow the raw canonical RGBA bytes — the target a transport's blit
    /// paints decoded pixels into. Exactly `width * height * 4` bytes.
    pub fn rgba_mut(&mut self) -> &mut [u8] {
        &mut self.rgba
    }

    /// Convert the current surface into an [`egui::ColorImage`] for upload to a
    /// `TextureHandle`. The stored buffer is already canonical RGBA, so this is a
    /// straight hand-off through egui's unmultiplied-RGBA constructor (opaque
    /// pixels are unaffected by premultiplication).
    #[must_use]
    pub fn to_color_image(&self) -> ColorImage {
        ColorImage::from_rgba_unmultiplied([self.width, self.height], &self.rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::RgbaSurface;
    use crate::egui::Color32;

    #[test]
    fn fresh_surface_is_opaque_black() {
        let s = RgbaSurface::new(2, 2);
        assert_eq!(s.size(), [2, 2]);
        let img = s.to_color_image();
        assert_eq!(img.size, [2, 2]);
        assert_eq!(img.pixels.len(), 4);
        for px in img.pixels {
            assert_eq!(px, Color32::from_rgb(0, 0, 0));
            assert_eq!(px.a(), 0xFF);
        }
    }

    #[test]
    fn rgba_mut_paints_through_to_the_color_image() {
        let mut s = RgbaSurface::new(1, 1);
        s.rgba_mut().copy_from_slice(&[0x30, 0x20, 0x10, 0xFF]);
        assert_eq!(s.rgba_bytes(), &[0x30, 0x20, 0x10, 0xFF]);
        assert_eq!(
            s.to_color_image().pixels[0],
            Color32::from_rgb(0x30, 0x20, 0x10)
        );
    }

    #[test]
    fn dimensions_report_the_constructed_size() {
        let s = RgbaSurface::new(7, 3);
        assert_eq!(s.width(), 7);
        assert_eq!(s.height(), 3);
        assert_eq!(s.rgba_bytes().len(), 7 * 3 * RgbaSurface::BYTES_PER_PIXEL);
    }
}

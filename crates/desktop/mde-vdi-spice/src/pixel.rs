//! Framebuffer storage + the SPICE surface → egui [`ColorImage`] conversion.
//!
//! This is the **decode-side egui surface** of the SPICE backend. A live session
//! hands the `spice-client` display channel's decoded primary surface (a whole
//! [`spice_client::DisplaySurface`] of tightly-packed pixels) to
//! [`Framebuffer::apply_surface`]; this module normalises it into a persistent
//! RGBA8 desktop [`Framebuffer`] and converts that into an [`egui::ColorImage`]
//! the shell uploads to a `TextureHandle` (lock 21 — render egui-native, no
//! external viewer, exactly like mde-vdi-rdp/-vnc).
//!
//! It is deliberately free of any transport dependency: it operates on raw pixel
//! bytes + a [`SurfaceFormat`], so the whole decode→egui conversion is unit-tested
//! on synthetic buffers with no runtime and no live connection (governance §7 —
//! the tested logic is real, not mocked). The live session feeds the very same
//! [`Framebuffer::apply_surface`] from `spice-client`'s decoded surface, so there
//! is no divergence between the tested path and the shipped path.

use crate::egui::ColorImage;

/// Byte layout of a decoded SPICE display surface.
///
/// `spice-client` decodes every wire image (raw / LZ / GLZ / QUIC) into its
/// [`DisplaySurface::data`](spice_client::DisplaySurface) as tightly-packed
/// **RGBA8** and tags the surface `format = 32`. That is the one layout the
/// shipped path delivers, so it is the canonical case; the enum still names the
/// 32-bpp `xRGB`/`ARGB` byte orders explicitly so a surface tagged otherwise is
/// normalised deterministically rather than mis-rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFormat {
    /// Red, Green, Blue, Alpha — `spice-client`'s decoded surface layout
    /// (`format = 32`). A straight hand-off to egui.
    Rgba,
    /// Blue, Green, Red, Alpha — the raw SPICE `_32_ARGB` little-endian order.
    Bgra,
    /// Blue, Green, Red, padding — the raw SPICE `_32_xRGB` little-endian order
    /// (the padding byte is forced opaque; the host desktop has no alpha).
    Bgrx,
}

impl SurfaceFormat {
    /// Bytes per pixel — always 4 for the 32-bpp surfaces SPICE delivers.
    pub const BYTES_PER_PIXEL: usize = 4;

    /// The [`SurfaceFormat`] for a raw `spice-client`
    /// [`DisplaySurface::format`](spice_client::DisplaySurface) tag.
    ///
    /// `spice-client` uses `32` for its decoded RGBA surfaces; the raw SPICE
    /// `SPICE_SURFACE_FMT_32_xRGB` (`32`) / `_32_ARGB` (`96`) wire tags are
    /// mapped to their little-endian byte orders for completeness. An unknown tag
    /// falls back to [`SurfaceFormat::Rgba`] (the shipped layout) so a surface is
    /// rendered rather than dropped.
    #[must_use]
    pub const fn from_tag(tag: u32) -> Self {
        match tag {
            96 => Self::Bgra,  // SPICE_SURFACE_FMT_32_ARGB (little-endian BGRA)
            129 => Self::Bgrx, // SPICE_SURFACE_FMT_32_xRGB variant (opaque)
            _ => Self::Rgba,   // 32 = spice-client's decoded RGBA (the ship path)
        }
    }

    /// Normalise one 4-byte source pixel to canonical `[r, g, b, a]`. `X` formats
    /// force alpha to `0xFF` (the host desktop has no transparency).
    #[inline]
    #[must_use]
    const fn to_rgba(self, px: [u8; 4]) -> [u8; 4] {
        match self {
            Self::Rgba => px,
            Self::Bgra => [px[2], px[1], px[0], px[3]],
            Self::Bgrx => [px[2], px[1], px[0], 0xFF],
        }
    }
}

/// Something wrong with a surface update.
///
/// Caught and surfaced rather than allowed to panic (governance: the workspace
/// denies `unwrap`/`panic` in shipped code; a malformed surface from the wire
/// must degrade, not crash).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferError {
    /// A surface dimension is zero — nothing to render.
    EmptySurface {
        /// The rejected `(width, height)`.
        size: (usize, usize),
    },
    /// The source slice is shorter than `width * height * 4` — a truncated
    /// surface.
    ShortSource {
        /// Bytes the source actually carried.
        got: usize,
        /// Bytes required for the declared surface.
        need: usize,
    },
}

impl core::fmt::Display for FramebufferError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptySurface { size } => write!(f, "empty SPICE surface {size:?}"),
            Self::ShortSource { got, need } => {
                write!(f, "truncated SPICE surface: {got} bytes, need {need}")
            }
        }
    }
}

impl std::error::Error for FramebufferError {}

/// A persistent RGBA8 desktop surface the display channel's frames accumulate
/// into.
///
/// Stored canonically as tightly-packed RGBA so [`Framebuffer::to_color_image`]
/// is a direct hand-off to egui. Construct it once at the negotiated desktop
/// size; replace it whole from each decoded surface with
/// [`Framebuffer::apply_surface`] (SPICE hands the display channel a whole
/// primary surface, not the RFB-style sub-rectangles VNC accumulates).
#[derive(Clone)]
pub struct Framebuffer {
    width: usize,
    height: usize,
    /// Tightly packed RGBA8, exactly `width * height * 4` bytes.
    rgba: Vec<u8>,
}

impl Framebuffer {
    /// A new opaque-black surface of `width × height`.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        let mut rgba = vec![0u8; width * height * SurfaceFormat::BYTES_PER_PIXEL];
        // Opaque black: every 4th byte (alpha) = 0xFF.
        for a in rgba
            .iter_mut()
            .skip(3)
            .step_by(SurfaceFormat::BYTES_PER_PIXEL)
        {
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

    /// Replace the whole surface from a decoded SPICE display surface: `w × h`
    /// pixels of `src` in `format`, resizing the framebuffer to `(w, h)` first.
    ///
    /// This is the single entry point the live display channel feeds and the
    /// unit tests drive — the surface `spice-client` decodes is already the whole
    /// primary framebuffer, so there is no sub-rectangle blit to accumulate.
    ///
    /// # Errors
    /// [`FramebufferError`] if a dimension is zero or `src` is shorter than
    /// `w * h * 4` — a malformed surface degrades rather than panicking.
    pub fn apply_surface(
        &mut self,
        w: usize,
        h: usize,
        format: SurfaceFormat,
        src: &[u8],
    ) -> Result<(), FramebufferError> {
        let bpp = SurfaceFormat::BYTES_PER_PIXEL;
        if w == 0 || h == 0 {
            return Err(FramebufferError::EmptySurface { size: (w, h) });
        }
        let need = w * h * bpp;
        if src.len() < need {
            return Err(FramebufferError::ShortSource {
                got: src.len(),
                need,
            });
        }
        if self.width != w || self.height != h {
            self.width = w;
            self.height = h;
            self.rgba = vec![0u8; need];
        }
        for (s, d) in src[..need]
            .chunks_exact(bpp)
            .zip(self.rgba.chunks_exact_mut(bpp))
        {
            // chunks_exact(4) yields exactly 4 bytes; copy into a fixed array so
            // the format normaliser has no fallible indexing.
            let px = [s[0], s[1], s[2], s[3]];
            d.copy_from_slice(&format.to_rgba(px));
        }
        Ok(())
    }

    /// Convert the current surface into an [`egui::ColorImage`] for upload to a
    /// `TextureHandle`. The stored buffer is already canonical RGBA, so this is a
    /// straight hand-off through egui's unmultiplied-RGBA constructor (opaque
    /// pixels are unaffected by premultiplication).
    #[must_use]
    pub fn to_color_image(&self) -> ColorImage {
        ColorImage::from_rgba_unmultiplied([self.width, self.height], &self.rgba)
    }

    /// Borrow the raw canonical RGBA bytes (testing / zero-copy callers).
    #[must_use]
    pub fn rgba_bytes(&self) -> &[u8] {
        &self.rgba
    }
}

#[cfg(test)]
mod tests {
    use super::{Framebuffer, FramebufferError, SurfaceFormat};
    use crate::egui::Color32;

    #[test]
    fn rgba_pixel_is_a_straight_handoff() {
        // spice-client's decoded surface layout — no swizzle.
        assert_eq!(
            SurfaceFormat::Rgba.to_rgba([0x30, 0x20, 0x10, 0x40]),
            [0x30, 0x20, 0x10, 0x40]
        );
    }

    #[test]
    fn bgra_and_bgrx_are_normalised() {
        // B=0x10 G=0x20 R=0x30 A=0x40  ->  R=0x30 G=0x20 B=0x10 A=0x40
        assert_eq!(
            SurfaceFormat::Bgra.to_rgba([0x10, 0x20, 0x30, 0x40]),
            [0x30, 0x20, 0x10, 0x40]
        );
        // X format forces opaque alpha regardless of the padding byte.
        assert_eq!(
            SurfaceFormat::Bgrx.to_rgba([0x10, 0x20, 0x30, 0x00]),
            [0x30, 0x20, 0x10, 0xFF]
        );
    }

    #[test]
    fn format_tag_maps_the_shipped_layout_to_rgba() {
        assert_eq!(SurfaceFormat::from_tag(32), SurfaceFormat::Rgba);
        assert_eq!(SurfaceFormat::from_tag(96), SurfaceFormat::Bgra);
        assert_eq!(SurfaceFormat::from_tag(129), SurfaceFormat::Bgrx);
        // Unknown tags fall back to the shipped RGBA layout, never dropped.
        assert_eq!(SurfaceFormat::from_tag(7), SurfaceFormat::Rgba);
    }

    #[test]
    fn fresh_framebuffer_is_opaque_black() {
        let fb = Framebuffer::new(2, 2);
        let img = fb.to_color_image();
        assert_eq!(img.size, [2, 2]);
        assert_eq!(img.pixels.len(), 4);
        for px in img.pixels {
            assert_eq!(px, Color32::from_rgb(0, 0, 0));
            assert_eq!(px.a(), 0xFF);
        }
    }

    #[test]
    fn rgba_surface_converts_to_expected_colorimage() {
        // 2x1 RGBA: pixel0 = pure red, pixel1 = pure blue.
        let mut fb = Framebuffer::new(2, 1);
        let src = [
            0xFF, 0x00, 0x00, 0xFF, // red
            0x00, 0x00, 0xFF, 0xFF, // blue
        ];
        fb.apply_surface(2, 1, SurfaceFormat::Rgba, &src)
            .expect("surface");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
    }

    #[test]
    fn applying_a_larger_surface_resizes() {
        let mut fb = Framebuffer::new(2, 2);
        let src = vec![0u8; 4 * 3 * 4]; // 4x3 RGBA
        fb.apply_surface(4, 3, SurfaceFormat::Rgba, &src)
            .expect("resize");
        assert_eq!(fb.size(), [4, 3]);
        assert_eq!(fb.to_color_image().size, [4, 3]);
    }

    #[test]
    fn bgra_surface_is_swizzled_on_apply() {
        let mut fb = Framebuffer::new(1, 1);
        // BGRA bytes for red: B=0 G=0 R=255 A=255.
        fb.apply_surface(1, 1, SurfaceFormat::Bgra, &[0x00, 0x00, 0xFF, 0xFF])
            .expect("surface");
        assert_eq!(fb.to_color_image().pixels[0], Color32::from_rgb(0xFF, 0, 0));
    }

    #[test]
    fn empty_surface_is_rejected() {
        let mut fb = Framebuffer::new(2, 2);
        let err = fb
            .apply_surface(0, 4, SurfaceFormat::Rgba, &[])
            .expect_err("must reject");
        assert!(matches!(err, FramebufferError::EmptySurface { .. }));
    }

    #[test]
    fn truncated_surface_is_rejected() {
        let mut fb = Framebuffer::new(4, 4);
        let too_small = [0u8; 8];
        let err = fb
            .apply_surface(4, 4, SurfaceFormat::Rgba, &too_small)
            .expect_err("must reject");
        assert!(matches!(err, FramebufferError::ShortSource { .. }));
    }
}

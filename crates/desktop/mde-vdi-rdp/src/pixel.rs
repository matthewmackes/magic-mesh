//! Framebuffer storage + the BGRA/RGBA → egui [`ColorImage`] conversion.
//!
//! This is the **decode-side egui surface** of the RDP backend. A live session
//! decodes bitmap / codec updates into rectangles of pixels (`ironrdp-graphics`
//! and `ironrdp-session`'s `DecodedImage` do the wire-level decode); this module
//! blits those rectangles into a persistent desktop [`Framebuffer`] and converts
//! the surface into an [`egui::ColorImage`] the shell uploads to a `TextureHandle`
//! (lock 21 — render egui-native, no external viewer).
//!
//! It is deliberately free of any `ironrdp` dependency: it operates on raw pixel
//! bytes + a [`PixelFormat`], so the whole decode→egui conversion is unit-tested
//! on synthetic buffers with no GPU and no live connection (governance §7 — the
//! tested logic is real, not mocked). The live session feeds the very same
//! [`Framebuffer::apply_rect`] from the decoder output, so there is no divergence
//! between the tested path and the shipped path.

use crate::egui::ColorImage;
use mde_vdi_core::RgbaSurface;

/// Byte order of a source pixel buffer coming off the RDP decoder.
///
/// RDP surfaces are overwhelmingly little-endian 32-bpp; the `X` variants carry a
/// don't-care padding byte where the `A` variants carry alpha. The host desktop
/// is opaque, so `X` padding is materialised as full alpha on conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// Blue, Green, Red, Alpha — the common RDP / `DecodedImage` byte order.
    Bgra,
    /// Blue, Green, Red, padding (forced opaque).
    Bgrx,
    /// Red, Green, Blue, Alpha.
    Rgba,
    /// Red, Green, Blue, padding (forced opaque).
    Rgbx,
}

impl PixelFormat {
    /// Bytes per pixel — always 4 for the 32-bpp surfaces RDP delivers.
    pub const BYTES_PER_PIXEL: usize = 4;

    /// Normalise one 4-byte source pixel to canonical `[r, g, b, a]`. `X` formats
    /// force alpha to `0xFF` (the host desktop has no transparency).
    #[inline]
    #[must_use]
    fn to_rgba(self, px: [u8; 4]) -> [u8; 4] {
        match self {
            Self::Bgra => [px[2], px[1], px[0], px[3]],
            Self::Bgrx => [px[2], px[1], px[0], 0xFF],
            Self::Rgba => px,
            Self::Rgbx => [px[0], px[1], px[2], 0xFF],
        }
    }
}

/// Something wrong with a rectangle update — caught and surfaced rather than
/// allowed to panic (governance: the workspace denies `unwrap`/`panic` in
/// shipped code; a malformed update from the wire must degrade, not crash).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferError {
    /// The update rectangle (partly) falls outside the framebuffer bounds.
    RectOutOfBounds {
        /// Rectangle origin + size that was rejected: `(x, y, w, h)`.
        rect: (usize, usize, usize, usize),
        /// Framebuffer size `(width, height)`.
        surface: (usize, usize),
    },
    /// The source slice is shorter than `height * stride` — a truncated update.
    ShortSource {
        /// Bytes the source actually carried.
        got: usize,
        /// Bytes required for the declared rectangle + stride.
        need: usize,
    },
    /// The source stride is narrower than one row of the rectangle.
    ShortStride {
        /// The declared stride in bytes.
        stride: usize,
        /// The minimum stride one row of the rectangle needs.
        min: usize,
    },
}

impl core::fmt::Display for FramebufferError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RectOutOfBounds { rect, surface } => write!(
                f,
                "update rect {rect:?} out of bounds for {surface:?} surface"
            ),
            Self::ShortSource { got, need } => {
                write!(f, "truncated update: {got} bytes, need {need}")
            }
            Self::ShortStride { stride, min } => {
                write!(f, "source stride {stride} narrower than row minimum {min}")
            }
        }
    }
}

impl std::error::Error for FramebufferError {}

/// A persistent RGBA8 desktop surface that accumulates rectangular updates.
///
/// Wraps the shared [`RgbaSurface`] (canonical tightly-packed RGBA, so
/// [`Framebuffer::to_color_image`] is a direct hand-off to egui) and adds the
/// RDP-specific stride blit. Construct it once at the negotiated desktop size; feed
/// decoded rectangles with [`Framebuffer::apply_rect`] (or the whole surface with
/// [`Framebuffer::apply_full`]).
#[derive(Clone)]
pub struct Framebuffer {
    surface: RgbaSurface,
}

impl Framebuffer {
    /// A new opaque-black surface of `width × height`.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            surface: RgbaSurface::new(width, height),
        }
    }

    /// Surface width in pixels.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.surface.width()
    }

    /// Surface height in pixels.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.surface.height()
    }

    /// Surface size as egui's `[w, h]`.
    #[must_use]
    pub const fn size(&self) -> [usize; 2] {
        self.surface.size()
    }

    /// Blit a decoded rectangle of `src` pixels (in `format`) at `(x, y)`.
    ///
    /// `src_stride` is the byte distance between source rows (≥ `w * 4`; codecs
    /// often hand back padded rows). The rectangle must lie fully inside the
    /// surface and `src` must hold `h * src_stride` bytes.
    ///
    /// # Errors
    /// [`FramebufferError`] if the rectangle escapes the surface, the stride is
    /// too narrow, or `src` is truncated — a malformed wire update degrades
    /// rather than panicking.
    pub fn apply_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        format: PixelFormat,
        src: &[u8],
        src_stride: usize,
    ) -> Result<(), FramebufferError> {
        let bpp = PixelFormat::BYTES_PER_PIXEL;
        let row_bytes = w * bpp;
        if w == 0 || h == 0 {
            return Ok(()); // an empty rectangle is a no-op, not an error.
        }
        if src_stride < row_bytes {
            return Err(FramebufferError::ShortStride {
                stride: src_stride,
                min: row_bytes,
            });
        }
        let (width, height) = (self.surface.width(), self.surface.height());
        if x + w > width || y + h > height {
            return Err(FramebufferError::RectOutOfBounds {
                rect: (x, y, w, h),
                surface: (width, height),
            });
        }
        let need = (h - 1) * src_stride + row_bytes;
        if src.len() < need {
            return Err(FramebufferError::ShortSource {
                got: src.len(),
                need,
            });
        }

        let dst_stride = width * bpp;
        let rgba = self.surface.rgba_mut();
        for row in 0..h {
            let src_row = &src[row * src_stride..row * src_stride + row_bytes];
            let dst_off = (y + row) * dst_stride + x * bpp;
            let dst_row = &mut rgba[dst_off..dst_off + row_bytes];
            for (s, d) in src_row.chunks_exact(bpp).zip(dst_row.chunks_exact_mut(bpp)) {
                // chunks_exact(4) yields exactly 4 bytes; copy into a fixed array
                // so the format normaliser has no fallible indexing.
                let px = [s[0], s[1], s[2], s[3]];
                d.copy_from_slice(&format.to_rgba(px));
            }
        }
        Ok(())
    }

    /// Replace the entire surface from a full-frame `src` (tightly packed, in
    /// `format`). Convenience over [`Framebuffer::apply_rect`] for a whole-desktop
    /// update.
    ///
    /// # Errors
    /// [`FramebufferError::ShortSource`] if `src` is smaller than the surface.
    pub fn apply_full(&mut self, format: PixelFormat, src: &[u8]) -> Result<(), FramebufferError> {
        let (width, height) = (self.surface.width(), self.surface.height());
        let stride = width * PixelFormat::BYTES_PER_PIXEL;
        self.apply_rect(0, 0, width, height, format, src, stride)
    }

    /// Convert the current surface into an [`egui::ColorImage`] for upload to a
    /// `TextureHandle`. The stored buffer is already canonical RGBA, so this is a
    /// straight hand-off through egui's unmultiplied-RGBA constructor (opaque
    /// pixels are unaffected by premultiplication).
    #[must_use]
    pub fn to_color_image(&self) -> ColorImage {
        self.surface.to_color_image()
    }

    /// Borrow the raw canonical RGBA bytes (testing / zero-copy callers).
    #[must_use]
    pub fn rgba_bytes(&self) -> &[u8] {
        self.surface.rgba_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::{Framebuffer, FramebufferError, PixelFormat};
    use crate::egui::Color32;

    #[test]
    fn bgra_pixel_normalises_to_rgba() {
        // B=0x10 G=0x20 R=0x30 A=0x40  ->  R=0x30 G=0x20 B=0x10 A=0x40
        assert_eq!(
            PixelFormat::Bgra.to_rgba([0x10, 0x20, 0x30, 0x40]),
            [0x30, 0x20, 0x10, 0x40]
        );
        // X format forces opaque alpha regardless of the padding byte.
        assert_eq!(
            PixelFormat::Bgrx.to_rgba([0x10, 0x20, 0x30, 0x00]),
            [0x30, 0x20, 0x10, 0xFF]
        );
        assert_eq!(
            PixelFormat::Rgba.to_rgba([0x30, 0x20, 0x10, 0x40]),
            [0x30, 0x20, 0x10, 0x40]
        );
        assert_eq!(
            PixelFormat::Rgbx.to_rgba([0x30, 0x20, 0x10, 0x00]),
            [0x30, 0x20, 0x10, 0xFF]
        );
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
    fn full_bgra_frame_converts_to_expected_colorimage() {
        // 2x1 BGRA: pixel0 = pure red, pixel1 = pure blue (in BGRA byte order).
        let mut fb = Framebuffer::new(2, 1);
        let src = [
            0x00, 0x00, 0xFF, 0xFF, // B=0 G=0 R=255 -> red
            0xFF, 0x00, 0x00, 0xFF, // B=255 G=0 R=0 -> blue
        ];
        fb.apply_full(PixelFormat::Bgra, &src).expect("full frame");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
    }

    #[test]
    fn sub_rect_blits_only_its_region() {
        // 3x3 surface; paint a 2x2 green rect at (1,1). The rest stays black.
        let mut fb = Framebuffer::new(3, 3);
        let green = [0x00u8, 0xFF, 0x00, 0xFF]; // BGRA green
        let rect: Vec<u8> = green.iter().copied().cycle().take(2 * 2 * 4).collect();
        fb.apply_rect(1, 1, 2, 2, PixelFormat::Bgra, &rect, 2 * 4)
            .expect("sub rect");
        let img = fb.to_color_image();
        let at = |x: usize, y: usize| img.pixels[y * 3 + x];
        assert_eq!(at(0, 0), Color32::from_rgb(0, 0, 0), "outside stays black");
        assert_eq!(
            at(0, 1),
            Color32::from_rgb(0, 0, 0),
            "left column untouched"
        );
        assert_eq!(at(1, 1), Color32::from_rgb(0, 0xFF, 0), "rect painted");
        assert_eq!(
            at(2, 2),
            Color32::from_rgb(0, 0xFF, 0),
            "rect corner painted"
        );
    }

    #[test]
    fn padded_stride_is_honoured() {
        // 1x2 rect with a 1-pixel pad byte-run per row (stride = 8, row = 4).
        let mut fb = Framebuffer::new(1, 2);
        let src = [
            0x00, 0x00, 0xFF, 0xFF, 0xAA, 0xAA, 0xAA, 0xAA, // row0 red + 4 pad bytes
            0xFF, 0x00, 0x00, 0xFF, 0xBB, 0xBB, 0xBB, 0xBB, // row1 blue + 4 pad bytes
        ];
        fb.apply_rect(0, 0, 1, 2, PixelFormat::Bgra, &src, 8)
            .expect("padded rect");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
    }

    #[test]
    fn out_of_bounds_rect_is_rejected() {
        let mut fb = Framebuffer::new(2, 2);
        let src = [0u8; 4 * 4];
        let err = fb
            .apply_rect(1, 1, 2, 2, PixelFormat::Bgra, &src, 8)
            .expect_err("must reject");
        assert!(matches!(err, FramebufferError::RectOutOfBounds { .. }));
    }

    #[test]
    fn truncated_source_is_rejected() {
        let mut fb = Framebuffer::new(4, 4);
        let too_small = [0u8; 8];
        let err = fb
            .apply_full(PixelFormat::Bgra, &too_small)
            .expect_err("must reject");
        assert!(matches!(err, FramebufferError::ShortSource { .. }));
    }

    #[test]
    fn narrow_stride_is_rejected() {
        let mut fb = Framebuffer::new(4, 1);
        let src = [0u8; 4 * 4];
        let err = fb
            .apply_rect(0, 0, 4, 1, PixelFormat::Bgra, &src, 8) // 8 < 16
            .expect_err("must reject");
        assert!(matches!(err, FramebufferError::ShortStride { .. }));
    }

    #[test]
    fn empty_rect_is_a_noop() {
        let mut fb = Framebuffer::new(2, 2);
        assert!(fb.apply_rect(0, 0, 0, 0, PixelFormat::Bgra, &[], 0).is_ok());
    }
}

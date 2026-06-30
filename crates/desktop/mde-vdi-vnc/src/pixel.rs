//! RFB pixel format + the framebuffer surface the RFB decoder paints into.
//!
//! This is the **decode target** of the VNC backend. The RFB wire decoder
//! ([`crate::encoding`]) turns Raw / `CopyRect` / RRE / Hextile rectangle updates
//! into pixels; this module owns the [`PixelFormat`] that says how one wire pixel
//! is laid out and the persistent [`Framebuffer`] those rectangles accumulate
//! into, which converts to an [`egui::ColorImage`] the shell uploads to a
//! `TextureHandle` (lock 21 — render egui-native, no external viewer).
//!
//! It is deliberately free of any wire-reader dependency: [`PixelFormat::decode`]
//! turns one already-read pixel value into canonical RGBA, and the framebuffer
//! ops take already-decoded RGBA / framebuffer coordinates. The whole surface is
//! unit-tested on synthetic data with no GPU and no live connection (governance
//! §7 — the tested logic is real, not mocked); the live session feeds the very
//! same [`Framebuffer`] from the decoder output, so the tested path and the
//! shipped path do not diverge.

use crate::egui::ColorImage;

/// Bytes in a canonical RGBA pixel — the framebuffer is stored RGBA8.
pub(crate) const RGBA_BYTES: usize = 4;

/// Scale one colour channel from its `max`-bit range to 8 bits (rounded). A zero
/// `max` (an illegal channel) yields 0 rather than dividing by zero.
#[inline]
#[allow(
    clippy::cast_possible_truncation,
    reason = "the rounded ratio is always <= 255, so the u8 cast cannot truncate"
)]
fn scale8(component: u32, max: u16) -> u8 {
    if max == 0 {
        return 0;
    }
    let m = u32::from(max);
    (((component & m) * 255 + m / 2) / m).min(255) as u8
}

/// How a server lays out one pixel on the RFB wire (RFC 6143 §7.4 `PIXEL_FORMAT`).
///
/// A true-colour pixel is `bits_per_pixel / 8` bytes read in the server's byte
/// order ([`PixelFormat::big_endian`]); each colour channel is then
/// `(value >> shift) & max`, scaled from its `max` range up to 8 bits. The common
/// case is 32-bpp depth-24 with 8-bit channels, but 16-bpp (e.g. RGB565) and
/// 8-bpp true-colour fall out of the same shift/max maths.
///
/// Palette (non-true-colour) surfaces need a server colour map that arrives in a
/// separate `SetColourMapEntries` message; this decoder is true-colour only and
/// [`PixelFormat::is_supported`] rejects the rest rather than mis-decode it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    /// Bits in one wire pixel — 8, 16, or 32.
    pub bits_per_pixel: u8,
    /// Significant colour depth (informational; channels come from the maxes).
    pub depth: u8,
    /// `true` if the wire pixel is big-endian.
    pub big_endian: bool,
    /// `true` for a true-colour surface (the only kind this decoder handles).
    pub true_color: bool,
    /// Maximum value of the red channel (e.g. 255 for 8-bit, 31 for 5-bit).
    pub red_max: u16,
    /// Maximum value of the green channel.
    pub green_max: u16,
    /// Maximum value of the blue channel.
    pub blue_max: u16,
    /// Right-shift to the red channel's least-significant bit.
    pub red_shift: u8,
    /// Right-shift to the green channel's least-significant bit.
    pub green_shift: u8,
    /// Right-shift to the blue channel's least-significant bit.
    pub blue_shift: u8,
}

impl PixelFormat {
    /// The canonical 32-bpp depth-24 little-endian true-colour format (8-bit
    /// channels). In memory a pixel is `[blue, green, red, pad]` — the format most
    /// MCNF guests (and the XAPI/`Xvnc` consoles) negotiate.
    #[must_use]
    pub const fn rgba8888() -> Self {
        Self {
            bits_per_pixel: 32,
            depth: 24,
            big_endian: false,
            true_color: true,
            red_max: 255,
            green_max: 255,
            blue_max: 255,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        }
    }

    /// The 16-bpp little-endian RGB565 true-colour format — a common low-bandwidth
    /// fallback. Red/blue are 5-bit (max 31), green 6-bit (max 63).
    #[must_use]
    pub const fn rgb565() -> Self {
        Self {
            bits_per_pixel: 16,
            depth: 16,
            big_endian: false,
            true_color: true,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
        }
    }

    /// Bytes in one wire pixel (`bits_per_pixel / 8`), 1 / 2 / 4.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        self.bits_per_pixel as usize / 8
    }

    /// Whether this decoder can handle the format: a true-colour surface at 8 / 16
    /// / 32 bpp with non-zero channel maxes. Anything else (palette, odd bpp) is
    /// rejected up front so the decode path never silently mis-renders.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        let bpp_ok = matches!(self.bits_per_pixel, 8 | 16 | 32);
        let maxes_ok = self.red_max != 0 && self.green_max != 0 && self.blue_max != 0;
        bpp_ok && self.true_color && maxes_ok
    }

    /// Normalise one already-read wire pixel `value` to canonical opaque
    /// `[r, g, b, 0xFF]`. The host desktop is opaque, so alpha is forced full.
    #[must_use]
    pub fn decode(self, value: u32) -> [u8; RGBA_BYTES] {
        let r = value >> self.red_shift;
        let g = value >> self.green_shift;
        let b = value >> self.blue_shift;
        [
            scale8(r, self.red_max),
            scale8(g, self.green_max),
            scale8(b, self.blue_max),
            0xFF,
        ]
    }
}

/// Something wrong with a framebuffer operation — caught and surfaced.
///
/// Surfaced rather than allowed to panic (governance: the workspace denies
/// `unwrap`/`panic` in shipped code; a malformed update off the wire must
/// degrade, not crash).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferError {
    /// A rectangle (partly) falls outside the framebuffer bounds.
    RectOutOfBounds {
        /// Rectangle origin + size that was rejected: `(x, y, w, h)`.
        rect: (usize, usize, usize, usize),
        /// Framebuffer size `(width, height)`.
        surface: (usize, usize),
    },
    /// The supplied RGBA slice is shorter than the rectangle needs.
    ShortSource {
        /// Bytes the source actually carried.
        got: usize,
        /// Bytes required for the rectangle (`w * h * 4`).
        need: usize,
    },
}

impl core::fmt::Display for FramebufferError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RectOutOfBounds { rect, surface } => {
                write!(f, "rect {rect:?} out of bounds for {surface:?} surface")
            }
            Self::ShortSource { got, need } => {
                write!(f, "truncated blit: {got} bytes, need {need}")
            }
        }
    }
}

impl std::error::Error for FramebufferError {}

/// A persistent RGBA8 desktop surface that accumulates RFB rectangle updates.
///
/// Stored canonically as tightly-packed RGBA so [`Framebuffer::to_color_image`]
/// is a direct hand-off to egui. Construct it at the server's framebuffer size;
/// feed it decoded pixels with [`Framebuffer::blit_rgba`], solid fills with
/// [`Framebuffer::fill_rect`], and intra-surface copies with
/// [`Framebuffer::copy_rect`] (the `CopyRect` encoding).
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
        let mut rgba = vec![0u8; width * height * RGBA_BYTES];
        // Opaque black: every 4th byte (alpha) = 0xFF.
        for a in rgba.iter_mut().skip(3).step_by(RGBA_BYTES) {
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

    /// Reject a rectangle that escapes the surface.
    const fn check_bounds(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) -> Result<(), FramebufferError> {
        if x + w > self.width || y + h > self.height {
            return Err(FramebufferError::RectOutOfBounds {
                rect: (x, y, w, h),
                surface: (self.width, self.height),
            });
        }
        Ok(())
    }

    /// Fill the `w × h` rectangle at `(x, y)` with a solid RGBA pixel (RRE /
    /// Hextile background + subrectangles, solid `CopyRect` sources, etc.).
    ///
    /// # Errors
    /// [`FramebufferError::RectOutOfBounds`] if the rectangle escapes the surface.
    pub fn fill_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        rgba: [u8; RGBA_BYTES],
    ) -> Result<(), FramebufferError> {
        if w == 0 || h == 0 {
            return Ok(());
        }
        self.check_bounds(x, y, w, h)?;
        for row in 0..h {
            let base = ((y + row) * self.width + x) * RGBA_BYTES;
            for col in 0..w {
                let off = base + col * RGBA_BYTES;
                self.rgba[off..off + RGBA_BYTES].copy_from_slice(&rgba);
            }
        }
        Ok(())
    }

    /// Blit a tightly-packed RGBA rectangle (`w * h * 4` bytes, row-major) at
    /// `(x, y)` — the decoded output of a Raw rectangle / Hextile raw tile.
    ///
    /// # Errors
    /// [`FramebufferError::RectOutOfBounds`] if the rectangle escapes the surface,
    /// or [`FramebufferError::ShortSource`] if `src` is too short.
    pub fn blit_rgba(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[u8],
    ) -> Result<(), FramebufferError> {
        if w == 0 || h == 0 {
            return Ok(());
        }
        self.check_bounds(x, y, w, h)?;
        let row_bytes = w * RGBA_BYTES;
        let need = row_bytes * h;
        if src.len() < need {
            return Err(FramebufferError::ShortSource {
                got: src.len(),
                need,
            });
        }
        for row in 0..h {
            let src_off = row * row_bytes;
            let dst_off = ((y + row) * self.width + x) * RGBA_BYTES;
            self.rgba[dst_off..dst_off + row_bytes]
                .copy_from_slice(&src[src_off..src_off + row_bytes]);
        }
        Ok(())
    }

    /// Copy the `w × h` rectangle at `(src_x, src_y)` to `(dst_x, dst_y)` — the
    /// `CopyRect` encoding. The source region is snapshotted first, so the copy is
    /// correct even when source and destination overlap.
    ///
    /// # Errors
    /// [`FramebufferError::RectOutOfBounds`] if either rectangle escapes the
    /// surface.
    pub fn copy_rect(
        &mut self,
        src_x: usize,
        src_y: usize,
        dst_x: usize,
        dst_y: usize,
        w: usize,
        h: usize,
    ) -> Result<(), FramebufferError> {
        if w == 0 || h == 0 {
            return Ok(());
        }
        self.check_bounds(src_x, src_y, w, h)?;
        self.check_bounds(dst_x, dst_y, w, h)?;
        let row_bytes = w * RGBA_BYTES;
        // Snapshot the source rows first so overlap is handled correctly.
        let mut tmp = vec![0u8; row_bytes * h];
        for row in 0..h {
            let s = ((src_y + row) * self.width + src_x) * RGBA_BYTES;
            tmp[row * row_bytes..row * row_bytes + row_bytes]
                .copy_from_slice(&self.rgba[s..s + row_bytes]);
        }
        for row in 0..h {
            let d = ((dst_y + row) * self.width + dst_x) * RGBA_BYTES;
            self.rgba[d..d + row_bytes]
                .copy_from_slice(&tmp[row * row_bytes..row * row_bytes + row_bytes]);
        }
        Ok(())
    }

    /// Resize the surface to `width × height`, preserving the overlapping
    /// top-left region (the `DesktopSize` pseudo-encoding the live layer applies).
    /// New area is opaque black.
    pub fn resize(&mut self, width: usize, height: usize) {
        if width == self.width && height == self.height {
            return;
        }
        let mut next = Self::new(width, height);
        let cw = self.width.min(width);
        let ch = self.height.min(height);
        for row in 0..ch {
            let s = (row * self.width) * RGBA_BYTES;
            let d = (row * width) * RGBA_BYTES;
            next.rgba[d..d + cw * RGBA_BYTES].copy_from_slice(&self.rgba[s..s + cw * RGBA_BYTES]);
        }
        *self = next;
    }

    /// Convert the current surface into an [`egui::ColorImage`] for upload to a
    /// `TextureHandle`. The stored buffer is already canonical RGBA, so this is a
    /// straight hand-off through egui's unmultiplied-RGBA constructor.
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
    use super::{Framebuffer, FramebufferError, PixelFormat};
    use crate::egui::Color32;

    #[test]
    fn rgba8888_decodes_primary_colours() {
        let f = PixelFormat::rgba8888();
        // In-memory little-endian bytes [B, G, R, pad]; value = pad<<24|R<<16|G<<8|B.
        let red = u32::from_le_bytes([0x00, 0x00, 0xFF, 0x00]);
        let green = u32::from_le_bytes([0x00, 0xFF, 0x00, 0x00]);
        let blue = u32::from_le_bytes([0xFF, 0x00, 0x00, 0x00]);
        assert_eq!(f.decode(red), [0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(f.decode(green), [0x00, 0xFF, 0x00, 0xFF]);
        assert_eq!(f.decode(blue), [0x00, 0x00, 0xFF, 0xFF]);
    }

    #[test]
    fn rgb565_scales_channels_to_8_bit() {
        let f = PixelFormat::rgb565();
        // Full red (5-bit max 31) -> 0xFF; full green (6-bit max 63) -> 0xFF.
        let full_red: u32 = 0x1F << 11;
        let full_green: u32 = 0x3F << 5;
        let full_blue: u32 = 0x1F;
        assert_eq!(f.decode(full_red), [0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(f.decode(full_green), [0x00, 0xFF, 0x00, 0xFF]);
        assert_eq!(f.decode(full_blue), [0x00, 0x00, 0xFF, 0xFF]);
        // Mid red (16/31) scales to 132 = round(16*255/31).
        assert_eq!(f.decode(16 << 11)[0], 132);
    }

    #[test]
    fn support_predicate_rejects_palette_and_odd_bpp() {
        assert!(PixelFormat::rgba8888().is_supported());
        assert!(PixelFormat::rgb565().is_supported());
        let palette = PixelFormat {
            true_color: false,
            ..PixelFormat::rgba8888()
        };
        assert!(!palette.is_supported());
        let odd = PixelFormat {
            bits_per_pixel: 24,
            ..PixelFormat::rgba8888()
        };
        assert!(!odd.is_supported());
        let zero_max = PixelFormat {
            green_max: 0,
            ..PixelFormat::rgba8888()
        };
        assert!(!zero_max.is_supported());
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
    fn fill_rect_paints_only_its_region() {
        let mut fb = Framebuffer::new(3, 3);
        fb.fill_rect(1, 1, 2, 2, [0x00, 0xFF, 0x00, 0xFF])
            .expect("fill");
        let img = fb.to_color_image();
        let at = |x: usize, y: usize| img.pixels[y * 3 + x];
        assert_eq!(at(0, 0), Color32::from_rgb(0, 0, 0), "outside stays black");
        assert_eq!(at(1, 1), Color32::from_rgb(0, 0xFF, 0), "rect painted");
        assert_eq!(at(2, 2), Color32::from_rgb(0, 0xFF, 0), "rect corner");
        assert_eq!(
            at(0, 1),
            Color32::from_rgb(0, 0, 0),
            "left column untouched"
        );
    }

    #[test]
    fn blit_rgba_writes_packed_pixels() {
        let mut fb = Framebuffer::new(2, 1);
        let src = [0xFF, 0x00, 0x00, 0xFF, 0x00, 0x00, 0xFF, 0xFF]; // red, blue
        fb.blit_rgba(0, 0, 2, 1, &src).expect("blit");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
    }

    #[test]
    fn copy_rect_duplicates_a_region() {
        let mut fb = Framebuffer::new(4, 1);
        fb.fill_rect(0, 0, 1, 1, [0xFF, 0x00, 0x00, 0xFF])
            .expect("seed");
        fb.copy_rect(0, 0, 2, 0, 1, 1).expect("copy");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[2], Color32::from_rgb(0xFF, 0, 0), "copied");
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0), "gap untouched");
    }

    #[test]
    fn copy_rect_handles_overlap() {
        // Seed pixel0 red, pixel1 blue; copy [0..2] right by one. Overlap must not
        // smear pixel0 into pixel2 via an in-place left-to-right copy.
        let mut fb = Framebuffer::new(3, 1);
        fb.fill_rect(0, 0, 1, 1, [0xFF, 0x00, 0x00, 0xFF])
            .expect("p0");
        fb.fill_rect(1, 0, 1, 1, [0x00, 0x00, 0xFF, 0xFF])
            .expect("p1");
        fb.copy_rect(0, 0, 1, 0, 2, 1).expect("overlap copy");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[1], Color32::from_rgb(0xFF, 0, 0), "p0 -> p1");
        assert_eq!(img.pixels[2], Color32::from_rgb(0, 0, 0xFF), "p1 -> p2");
    }

    #[test]
    fn resize_preserves_top_left() {
        let mut fb = Framebuffer::new(2, 2);
        fb.fill_rect(0, 0, 1, 1, [0xFF, 0x00, 0x00, 0xFF])
            .expect("seed");
        fb.resize(4, 4);
        assert_eq!(fb.size(), [4, 4]);
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0), "kept");
        assert_eq!(img.pixels[5], Color32::from_rgb(0, 0, 0), "new area black");
    }

    #[test]
    fn out_of_bounds_and_short_source_are_rejected() {
        let mut fb = Framebuffer::new(2, 2);
        assert!(matches!(
            fb.fill_rect(1, 1, 2, 2, [0; 4]),
            Err(FramebufferError::RectOutOfBounds { .. })
        ));
        assert!(matches!(
            fb.blit_rgba(0, 0, 2, 2, &[0u8; 4]),
            Err(FramebufferError::ShortSource { got: 4, need: 16 })
        ));
        assert!(matches!(
            fb.copy_rect(1, 1, 0, 0, 2, 2),
            Err(FramebufferError::RectOutOfBounds { .. })
        ));
    }

    #[test]
    fn empty_rect_is_a_noop() {
        let mut fb = Framebuffer::new(2, 2);
        assert!(fb.fill_rect(0, 0, 0, 0, [0; 4]).is_ok());
        assert!(fb.blit_rgba(0, 0, 0, 0, &[]).is_ok());
        assert!(fb.copy_rect(0, 0, 0, 0, 0, 0).is_ok());
    }
}

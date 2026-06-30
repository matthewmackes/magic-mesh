//! The pure-Rust RFB rectangle decoder: wire bytes → [`Framebuffer`] pixels.
//!
//! A `FramebufferUpdate` (RFC 6143 §7.6.1) is a count of rectangles, each a
//! header (`x, y, w, h, encoding`) followed by encoding-specific payload. This
//! module owns the decoder for the four classic, dependency-free encodings every
//! desktop path exercises:
//!
//! * **Raw** (0) — pixels verbatim in the negotiated [`PixelFormat`].
//! * **`CopyRect`** (1) — copy a region already on screen (scrolls / window moves).
//! * **RRE** (2) — a background fill plus coloured subrectangles.
//! * **Hextile** (5) — 16×16 tiles, each raw or background/foreground subrects.
//!
//! These are integer-only decoders with no external crate — the whole reason
//! lock 21 keeps VNC the *universal* fallback. They are unit-tested against
//! synthetic byte streams here (governance §7: the decode logic is real, not
//! mocked); the live session feeds [`decode_framebuffer_update`] straight off the
//! socket, so the tested path and the shipped path do not diverge. The
//! zlib/JPEG-bearing encodings (Tight / ZRLE / TRLE) and the resize/cursor
//! pseudo-encodings are the live/advanced layer and are reported as
//! [`DecodeError::UnsupportedEncoding`] rather than silently mis-decoded.

use crate::pixel::{Framebuffer, FramebufferError, PixelFormat};

/// Why an RFB byte stream could not be decoded — surfaced, never panicked
/// (governance: a malformed update off the wire must degrade, not crash).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The stream ended mid-field — a truncated update.
    UnexpectedEof,
    /// An encoding this unit-path decoder does not implement (Tight / ZRLE /
    /// TRLE / a pseudo-encoding). Carries the wire encoding number.
    UnsupportedEncoding(i32),
    /// The negotiated pixel format is not a supported true-colour layout.
    UnsupportedFormat,
    /// A decoded rectangle did not fit the framebuffer.
    Framebuffer(FramebufferError),
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "truncated RFB update"),
            Self::UnsupportedEncoding(code) => write!(f, "unsupported RFB encoding {code}"),
            Self::UnsupportedFormat => write!(f, "unsupported (non-true-colour) pixel format"),
            Self::Framebuffer(e) => write!(f, "framebuffer: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<FramebufferError> for DecodeError {
    fn from(e: FramebufferError) -> Self {
        Self::Framebuffer(e)
    }
}

/// A forward byte cursor over an RFB message body.
///
/// Provides the big-endian integer reads the protocol uses. Every read is
/// bounds-checked into [`DecodeError::UnexpectedEof`], so a short buffer degrades
/// rather than panics.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Wrap a byte slice.
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Bytes consumed so far.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// Take `n` bytes, advancing the cursor.
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    /// Read one byte.
    ///
    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if the stream is exhausted.
    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    /// Read a big-endian `u16`.
    ///
    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than 2 bytes remain.
    pub fn read_u16_be(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// Read a big-endian `u32`.
    ///
    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than 4 bytes remain.
    pub fn read_u32_be(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a big-endian `i32` (the signed encoding number).
    ///
    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than 4 bytes remain.
    pub fn read_i32_be(&mut self) -> Result<i32, DecodeError> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Borrow the next `n` bytes.
    ///
    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than `n` bytes remain.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        self.take(n)
    }

    /// Read one pixel of `bytes_per_pixel` bytes into a `u32`, honouring the
    /// server byte order. `bytes_per_pixel` is 1 / 2 / 4 (≤ 4, so no overflow).
    ///
    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if the pixel is truncated.
    pub fn read_pixel(
        &mut self,
        bytes_per_pixel: usize,
        big_endian: bool,
    ) -> Result<u32, DecodeError> {
        let bytes = self.take(bytes_per_pixel)?;
        let mut value = 0u32;
        if big_endian {
            for &b in bytes {
                value = (value << 8) | u32::from(b);
            }
        } else {
            for (i, &b) in bytes.iter().enumerate() {
                value |= u32::from(b) << (8 * i);
            }
        }
        Ok(value)
    }
}

/// The RFB encoding of a rectangle. Only the four implemented encodings get a
/// named variant; anything else is [`Encoding::Other`] and decodes to
/// [`DecodeError::UnsupportedEncoding`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Raw pixels (0).
    Raw,
    /// `CopyRect` — on-screen region copy (1).
    CopyRect,
    /// Rise-and-Run-length Encoding (2).
    Rre,
    /// Hextile — 16×16 tiles (5).
    Hextile,
    /// Any other / pseudo-encoding, by wire number.
    Other(i32),
}

impl Encoding {
    /// Classify a wire encoding number.
    #[must_use]
    pub const fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Raw,
            1 => Self::CopyRect,
            2 => Self::Rre,
            5 => Self::Hextile,
            other => Self::Other(other),
        }
    }

    /// The wire encoding number.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Raw => 0,
            Self::CopyRect => 1,
            Self::Rre => 2,
            Self::Hextile => 5,
            Self::Other(v) => v,
        }
    }
}

/// A rectangle header from a `FramebufferUpdate`: a region plus its encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rectangle {
    /// Left edge in framebuffer pixels.
    pub x: u16,
    /// Top edge in framebuffer pixels.
    pub y: u16,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// The wire encoding number.
    pub encoding: i32,
}

/// Parse the 12-byte rectangle header (`x, y, w, h` as big-endian `u16`, then the
/// `i32` encoding).
///
/// # Errors
/// [`DecodeError::UnexpectedEof`] on a truncated header.
pub fn parse_rectangle_header(reader: &mut Reader) -> Result<Rectangle, DecodeError> {
    Ok(Rectangle {
        x: reader.read_u16_be()?,
        y: reader.read_u16_be()?,
        width: reader.read_u16_be()?,
        height: reader.read_u16_be()?,
        encoding: reader.read_i32_be()?,
    })
}

/// Parse the 16-byte `PIXEL_FORMAT` structure (RFC 6143 §7.4) — the layout a
/// `ServerInit` / `SetPixelFormat` carries.
///
/// # Errors
/// [`DecodeError::UnexpectedEof`] on a truncated structure.
pub fn parse_pixel_format(reader: &mut Reader) -> Result<PixelFormat, DecodeError> {
    let bits_per_pixel = reader.read_u8()?;
    let depth = reader.read_u8()?;
    let big_endian = reader.read_u8()? != 0;
    let true_color = reader.read_u8()? != 0;
    let red_max = reader.read_u16_be()?;
    let green_max = reader.read_u16_be()?;
    let blue_max = reader.read_u16_be()?;
    let red_shift = reader.read_u8()?;
    let green_shift = reader.read_u8()?;
    let blue_shift = reader.read_u8()?;
    let _padding = reader.read_bytes(3)?;
    Ok(PixelFormat {
        bits_per_pixel,
        depth,
        big_endian,
        true_color,
        red_max,
        green_max,
        blue_max,
        red_shift,
        green_shift,
        blue_shift,
    })
}

/// Read `count` pixels in `format` from `reader` into a tightly-packed RGBA
/// buffer. The reader is checked to hold the whole run first so a malformed
/// header cannot trigger a huge speculative allocation.
fn decode_pixels(
    reader: &mut Reader,
    format: PixelFormat,
    count: usize,
) -> Result<Vec<u8>, DecodeError> {
    let bpp = format.bytes_per_pixel();
    if reader.remaining() < count * bpp {
        return Err(DecodeError::UnexpectedEof);
    }
    let mut rgba = Vec::with_capacity(count * 4);
    for _ in 0..count {
        let value = reader.read_pixel(bpp, format.big_endian)?;
        rgba.extend_from_slice(&format.decode(value));
    }
    Ok(rgba)
}

// Hextile subencoding mask bits (RFC 6143 §7.7.4).
const HEXTILE_RAW: u8 = 0x01;
const HEXTILE_BG: u8 = 0x02;
const HEXTILE_FG: u8 = 0x04;
const HEXTILE_ANY_SUBRECTS: u8 = 0x08;
const HEXTILE_SUBRECTS_COLOURED: u8 = 0x10;
const HEXTILE_TILE: usize = 16;

/// Decode one rectangle's payload into the framebuffer.
///
/// `format` must be a supported true-colour layout ([`PixelFormat::is_supported`])
/// — checked once at the message level by [`decode_framebuffer_update`].
///
/// # Errors
/// [`DecodeError::UnsupportedEncoding`] for an encoding this decoder does not
/// implement, [`DecodeError::UnexpectedEof`] on truncated payload, or
/// [`DecodeError::Framebuffer`] if a decoded region escapes the surface.
pub fn decode_rect(
    rect: &Rectangle,
    reader: &mut Reader,
    fb: &mut Framebuffer,
    format: PixelFormat,
) -> Result<(), DecodeError> {
    let x = usize::from(rect.x);
    let y = usize::from(rect.y);
    let w = usize::from(rect.width);
    let h = usize::from(rect.height);

    match Encoding::from_i32(rect.encoding) {
        Encoding::Raw => {
            let rgba = decode_pixels(reader, format, w * h)?;
            fb.blit_rgba(x, y, w, h, &rgba)?;
        }
        Encoding::CopyRect => {
            let src_x = usize::from(reader.read_u16_be()?);
            let src_y = usize::from(reader.read_u16_be()?);
            fb.copy_rect(src_x, src_y, x, y, w, h)?;
        }
        Encoding::Rre => decode_rre(rect, reader, fb, format)?,
        Encoding::Hextile => decode_hextile(rect, reader, fb, format)?,
        Encoding::Other(code) => return Err(DecodeError::UnsupportedEncoding(code)),
    }
    Ok(())
}

/// RRE: a background fill plus `n` coloured subrectangles (RFC 6143 §7.7.2).
fn decode_rre(
    rect: &Rectangle,
    reader: &mut Reader,
    fb: &mut Framebuffer,
    format: PixelFormat,
) -> Result<(), DecodeError> {
    let bpp = format.bytes_per_pixel();
    let x = usize::from(rect.x);
    let y = usize::from(rect.y);
    let n = reader.read_u32_be()?;
    let background = format.decode(reader.read_pixel(bpp, format.big_endian)?);
    fb.fill_rect(
        x,
        y,
        usize::from(rect.width),
        usize::from(rect.height),
        background,
    )?;
    for _ in 0..n {
        let colour = format.decode(reader.read_pixel(bpp, format.big_endian)?);
        let sx = usize::from(reader.read_u16_be()?);
        let sy = usize::from(reader.read_u16_be()?);
        let sw = usize::from(reader.read_u16_be()?);
        let sh = usize::from(reader.read_u16_be()?);
        fb.fill_rect(x + sx, y + sy, sw, sh, colour)?;
    }
    Ok(())
}

/// Hextile: 16×16 tiles, each raw or background/foreground + subrects, with the
/// background/foreground carried over from the previous tile when not re-sent
/// (RFC 6143 §7.7.4).
fn decode_hextile(
    rect: &Rectangle,
    reader: &mut Reader,
    fb: &mut Framebuffer,
    format: PixelFormat,
) -> Result<(), DecodeError> {
    let bpp = format.bytes_per_pixel();
    let ox = usize::from(rect.x);
    let oy = usize::from(rect.y);
    let w = usize::from(rect.width);
    let h = usize::from(rect.height);
    let mut background = [0u8, 0, 0, 0xFF];
    let mut foreground = [0u8, 0, 0, 0xFF];

    let mut ty = 0;
    while ty < h {
        let th = HEXTILE_TILE.min(h - ty);
        let mut tx = 0;
        while tx < w {
            let tw = HEXTILE_TILE.min(w - tx);
            let mask = reader.read_u8()?;

            if mask & HEXTILE_RAW != 0 {
                let rgba = decode_pixels(reader, format, tw * th)?;
                fb.blit_rgba(ox + tx, oy + ty, tw, th, &rgba)?;
                tx += tw;
                continue;
            }
            if mask & HEXTILE_BG != 0 {
                background = format.decode(reader.read_pixel(bpp, format.big_endian)?);
            }
            if mask & HEXTILE_FG != 0 {
                foreground = format.decode(reader.read_pixel(bpp, format.big_endian)?);
            }
            fb.fill_rect(ox + tx, oy + ty, tw, th, background)?;

            if mask & HEXTILE_ANY_SUBRECTS != 0 {
                let coloured = mask & HEXTILE_SUBRECTS_COLOURED != 0;
                let subrects = reader.read_u8()?;
                for _ in 0..subrects {
                    let colour = if coloured {
                        format.decode(reader.read_pixel(bpp, format.big_endian)?)
                    } else {
                        foreground
                    };
                    let xy = reader.read_u8()?;
                    let wh = reader.read_u8()?;
                    let sx = usize::from(xy >> 4);
                    let sy = usize::from(xy & 0x0F);
                    let sw = usize::from((wh >> 4) + 1);
                    let sh = usize::from((wh & 0x0F) + 1);
                    fb.fill_rect(ox + tx + sx, oy + ty + sy, sw, sh, colour)?;
                }
            }
            tx += tw;
        }
        ty += th;
    }
    Ok(())
}

/// Decode a whole `FramebufferUpdate` body into the framebuffer, returning the
/// number of rectangles applied.
///
/// The caller has already consumed the 1-byte message type (0); `reader` is
/// positioned at the padding byte, so the body is
/// `[padding(1), rect-count(u16), rectangles...]`.
///
/// # Errors
/// [`DecodeError::UnsupportedFormat`] if `format` is not true-colour,
/// [`DecodeError::UnsupportedEncoding`] for an unimplemented encoding,
/// [`DecodeError::UnexpectedEof`] on truncation, or [`DecodeError::Framebuffer`]
/// for an out-of-bounds rectangle.
pub fn decode_framebuffer_update(
    reader: &mut Reader,
    fb: &mut Framebuffer,
    format: PixelFormat,
) -> Result<u16, DecodeError> {
    if !format.is_supported() {
        return Err(DecodeError::UnsupportedFormat);
    }
    let _padding = reader.read_u8()?;
    let count = reader.read_u16_be()?;
    for _ in 0..count {
        let rect = parse_rectangle_header(reader)?;
        decode_rect(&rect, reader, fb, format)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_framebuffer_update, decode_rect, parse_pixel_format, parse_rectangle_header,
        DecodeError, Encoding, Reader, Rectangle,
    };
    use crate::egui::Color32;
    use crate::pixel::{Framebuffer, PixelFormat};

    // rgba8888 little-endian pixel bytes for a primary colour.
    fn px(r: u8, g: u8, b: u8) -> [u8; 4] {
        [b, g, r, 0] // [B, G, R, pad]
    }

    #[test]
    fn reader_reads_big_endian_and_pixels() {
        let mut r = Reader::new(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(r.read_u16_be().expect("u16"), 0x1234);
        assert_eq!(r.remaining(), 2);
        // Little-endian 16-bit pixel from the remaining [0x56,0x78] = 0x7856.
        assert_eq!(r.read_pixel(2, false).expect("px"), 0x7856);
        assert!(matches!(r.read_u8(), Err(DecodeError::UnexpectedEof)));
    }

    #[test]
    fn read_pixel_respects_endianness() {
        let mut le = Reader::new(&[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(le.read_pixel(4, false).expect("le"), 0xDDCC_BBAA);
        let mut be = Reader::new(&[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(be.read_pixel(4, true).expect("be"), 0xAABB_CCDD);
    }

    #[test]
    fn encoding_classification_roundtrips() {
        assert_eq!(Encoding::from_i32(0), Encoding::Raw);
        assert_eq!(Encoding::from_i32(1), Encoding::CopyRect);
        assert_eq!(Encoding::from_i32(2), Encoding::Rre);
        assert_eq!(Encoding::from_i32(5), Encoding::Hextile);
        assert_eq!(Encoding::from_i32(-239), Encoding::Other(-239));
        assert_eq!(Encoding::Hextile.code(), 5);
        assert_eq!(Encoding::Other(7).code(), 7);
    }

    #[test]
    fn raw_rect_decodes_into_framebuffer() {
        let mut fb = Framebuffer::new(2, 1);
        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            encoding: 0,
        };
        let mut payload = Vec::new();
        payload.extend_from_slice(&px(0xFF, 0, 0)); // red
        payload.extend_from_slice(&px(0, 0, 0xFF)); // blue
        let mut r = Reader::new(&payload);
        decode_rect(&rect, &mut r, &mut fb, PixelFormat::rgba8888()).expect("raw");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
    }

    #[test]
    fn copyrect_copies_existing_region() {
        let mut fb = Framebuffer::new(4, 1);
        fb.fill_rect(0, 0, 1, 1, [0x00, 0xFF, 0x00, 0xFF])
            .expect("seed green");
        let rect = Rectangle {
            x: 2,
            y: 0,
            width: 1,
            height: 1,
            encoding: 1,
        };
        // CopyRect payload: src-x = 0, src-y = 0.
        let payload = [0x00, 0x00, 0x00, 0x00];
        let mut r = Reader::new(&payload);
        decode_rect(&rect, &mut r, &mut fb, PixelFormat::rgba8888()).expect("copyrect");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[2], Color32::from_rgb(0, 0xFF, 0));
    }

    #[test]
    fn rre_fills_background_then_subrect() {
        let mut fb = Framebuffer::new(4, 4);
        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            encoding: 2,
        };
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_be_bytes()); // one subrect
        payload.extend_from_slice(&px(0, 0, 0xFF)); // background blue
        payload.extend_from_slice(&px(0xFF, 0, 0)); // subrect red
        payload.extend_from_slice(&1u16.to_be_bytes()); // sx
        payload.extend_from_slice(&1u16.to_be_bytes()); // sy
        payload.extend_from_slice(&2u16.to_be_bytes()); // sw
        payload.extend_from_slice(&2u16.to_be_bytes()); // sh
        let mut r = Reader::new(&payload);
        decode_rect(&rect, &mut r, &mut fb, PixelFormat::rgba8888()).expect("rre");
        let img = fb.to_color_image();
        let at = |x: usize, y: usize| img.pixels[y * 4 + x];
        assert_eq!(at(0, 0), Color32::from_rgb(0, 0, 0xFF), "background");
        assert_eq!(at(1, 1), Color32::from_rgb(0xFF, 0, 0), "subrect");
        assert_eq!(at(2, 2), Color32::from_rgb(0xFF, 0, 0), "subrect corner");
        assert_eq!(at(3, 3), Color32::from_rgb(0, 0, 0xFF), "background again");
    }

    #[test]
    fn hextile_background_foreground_and_subrect() {
        let mut fb = Framebuffer::new(2, 2);
        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            encoding: 5,
        };
        // One tile (2x2): mask = BG|FG|ANY_SUBRECTS (0x02|0x04|0x08 = 0x0E).
        let mut payload = Vec::new();
        payload.push(0x0E);
        payload.extend_from_slice(&px(0, 0, 0xFF)); // background blue
        payload.extend_from_slice(&px(0, 0xFF, 0)); // foreground green
        payload.push(1); // one subrect
        payload.push(0x00); // xy: x=0, y=0
        payload.push(0x00); // wh: w=1, h=1
        let mut r = Reader::new(&payload);
        decode_rect(&rect, &mut r, &mut fb, PixelFormat::rgba8888()).expect("hextile");
        let img = fb.to_color_image();
        let at = |x: usize, y: usize| img.pixels[y * 2 + x];
        assert_eq!(at(0, 0), Color32::from_rgb(0, 0xFF, 0), "fg subrect");
        assert_eq!(at(1, 0), Color32::from_rgb(0, 0, 0xFF), "bg");
        assert_eq!(at(1, 1), Color32::from_rgb(0, 0, 0xFF), "bg");
    }

    #[test]
    fn hextile_raw_tile() {
        let mut fb = Framebuffer::new(2, 1);
        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            encoding: 5,
        };
        let mut payload = Vec::new();
        payload.push(0x01); // RAW tile
        payload.extend_from_slice(&px(0xFF, 0, 0));
        payload.extend_from_slice(&px(0, 0xFF, 0));
        let mut r = Reader::new(&payload);
        decode_rect(&rect, &mut r, &mut fb, PixelFormat::rgba8888()).expect("hextile raw");
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0xFF, 0));
    }

    #[test]
    fn full_framebuffer_update_applies_every_rect() {
        let mut fb = Framebuffer::new(2, 1);
        // Body: padding, count=1, rect(0,0,2,1,Raw), then two raw pixels.
        let mut body = Vec::new();
        body.push(0x00); // padding
        body.extend_from_slice(&1u16.to_be_bytes()); // one rect
        body.extend_from_slice(&0u16.to_be_bytes()); // x
        body.extend_from_slice(&0u16.to_be_bytes()); // y
        body.extend_from_slice(&2u16.to_be_bytes()); // w
        body.extend_from_slice(&1u16.to_be_bytes()); // h
        body.extend_from_slice(&0i32.to_be_bytes()); // Raw
        body.extend_from_slice(&px(0xFF, 0, 0));
        body.extend_from_slice(&px(0, 0, 0xFF));
        let mut r = Reader::new(&body);
        let n =
            decode_framebuffer_update(&mut r, &mut fb, PixelFormat::rgba8888()).expect("update");
        assert_eq!(n, 1);
        let img = fb.to_color_image();
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
    }

    #[test]
    fn pixel_format_parse_roundtrips_standard() {
        // The 16-byte PIXEL_FORMAT for the canonical 32bpp LE true-colour layout.
        let bytes = [
            32, 24, 0, 1, // bpp, depth, big-endian=0, true-colour=1
            0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, // r/g/b max = 255 each (BE u16)
            16, 8, 0, // r/g/b shift
            0, 0, 0, // padding
        ];
        let mut r = Reader::new(&bytes);
        let f = parse_pixel_format(&mut r).expect("format");
        assert_eq!(f, PixelFormat::rgba8888());
        assert!(f.is_supported());
    }

    #[test]
    fn unsupported_encoding_is_reported_not_panicked() {
        let mut fb = Framebuffer::new(2, 2);
        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            encoding: 16, // ZRLE — not in the unit path
        };
        let mut r = Reader::new(&[]);
        assert_eq!(
            decode_rect(&rect, &mut r, &mut fb, PixelFormat::rgba8888()),
            Err(DecodeError::UnsupportedEncoding(16))
        );
    }

    #[test]
    fn truncated_payload_is_reported_not_panicked() {
        let mut fb = Framebuffer::new(2, 1);
        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            encoding: 0,
        };
        // Only one pixel's worth of bytes for a two-pixel raw rect.
        let mut r = Reader::new(&[0, 0, 0xFF, 0]);
        assert_eq!(
            decode_rect(&rect, &mut r, &mut fb, PixelFormat::rgba8888()),
            Err(DecodeError::UnexpectedEof)
        );
    }

    #[test]
    fn non_true_colour_format_is_rejected_at_message_level() {
        let mut fb = Framebuffer::new(2, 2);
        let palette = PixelFormat {
            true_color: false,
            ..PixelFormat::rgba8888()
        };
        let body = [0u8, 0, 0]; // padding + zero rects (never reached)
        let mut r = Reader::new(&body);
        assert_eq!(
            decode_framebuffer_update(&mut r, &mut fb, palette),
            Err(DecodeError::UnsupportedFormat)
        );
    }

    #[test]
    fn rectangle_header_parses() {
        let bytes = [0, 1, 0, 2, 0, 3, 0, 4, 0, 0, 0, 5];
        let mut r = Reader::new(&bytes);
        let rect = parse_rectangle_header(&mut r).expect("header");
        assert_eq!(
            rect,
            Rectangle {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
                encoding: 5,
            }
        );
    }
}

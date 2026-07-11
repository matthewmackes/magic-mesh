//! Per-frame damage tracking + the partial-upload slice math shared by every VDI
//! transport.
//!
//! A live desktop changes a *little* between frames — a blinking cursor, a moved
//! window, a scrolling line — but the naive upload path re-sends the **whole**
//! framebuffer to the GPU on every changed frame (`TextureHandle::set`). The
//! decoders already blit their updates rectangle-by-rectangle, so the exact region
//! that changed is known at decode time; this module is the transport-neutral home
//! for carrying those rectangles up to the shell so it can update just the changed
//! sub-rectangles with `TextureHandle::set_partial` (perf-7).
//!
//! The types here are deliberately pure — no egui context, no GPU. A
//! [`DamageLog`] accumulates the rectangles a `Framebuffer` painted since the last
//! frame; [`FrameDamage`] is what a session hands the shell alongside the
//! [`ColorImage`]; [`sub_color_image`] slices one sub-rectangle out of the full
//! frame (exactly the `(offset, image)` pair `set_partial` wants); and
//! [`paint_sub_image`] is the CPU-side model of `set_partial` used to *prove* — in
//! plain unit tests, no seat required — that partial-uploading the damage rects is
//! pixel-identical to a full upload (governance §7: the tested logic is the shipped
//! logic).
//!
//! The correctness contract is simple and is what the tests assert: as long as the
//! damage rectangles cover every changed pixel (which the decoders guarantee — they
//! blit exactly those rectangles), applying them via [`sub_color_image`] +
//! [`paint_sub_image`] onto the previous frame reproduces the full frame byte for
//! byte, while leaving untouched regions untouched. Any path that cannot promise
//! that (a resize, a whole-surface replace, an unknown-geometry batch, the first
//! frame) degrades to [`FrameDamage::Full`] — never a skipped or partial upload
//! that a full `set` would have done.

use crate::egui::{Color32, ColorImage};

/// A changed rectangle of the desktop, in surface pixels (top-left origin).
///
/// Coordinates are the decoder framebuffer's own pixel grid — the same grid the
/// [`ColorImage`] handed to the shell is in — so a rectangle indexes straight into
/// that image with no scaling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageRect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl DamageRect {
    /// A rectangle of `w × h` pixels with its top-left corner at `(x, y)`.
    #[must_use]
    pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }

    /// Left edge (x origin) in surface pixels.
    #[must_use]
    pub const fn x(&self) -> usize {
        self.x
    }

    /// Top edge (y origin) in surface pixels.
    #[must_use]
    pub const fn y(&self) -> usize {
        self.y
    }

    /// Width in pixels.
    #[must_use]
    pub const fn w(&self) -> usize {
        self.w
    }

    /// Height in pixels.
    #[must_use]
    pub const fn h(&self) -> usize {
        self.h
    }

    /// Top-left corner as egui's `[x, y]` (the `pos` `set_partial` wants).
    #[must_use]
    pub const fn offset(&self) -> [usize; 2] {
        [self.x, self.y]
    }

    /// A zero-area rectangle carries no pixels — an empty (no-op) update.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Intersect this rectangle with a `width × height` surface, returning the
    /// visible part — or `None` if it is empty or falls entirely outside.
    ///
    /// This only ever *shrinks* the rectangle (trimming a right/bottom overhang),
    /// so a clamped rectangle always lies fully inside the surface and can index it
    /// without a bounds panic. A rectangle whose origin is already off the surface
    /// has no visible part and yields `None`.
    #[must_use]
    pub fn clamped(&self, width: usize, height: usize) -> Option<Self> {
        if self.is_empty() || self.x >= width || self.y >= height {
            return None;
        }
        let w = self.w.min(width - self.x);
        let h = self.h.min(height - self.y);
        if w == 0 || h == 0 {
            None
        } else {
            Some(Self {
                x: self.x,
                y: self.y,
                w,
                h,
            })
        }
    }
}

/// What changed in a frame the session hands the shell — the upload hint.
///
/// [`FrameDamage::Full`] means "upload the whole image" (the safe default: the
/// first frame, a resize, a whole-surface replace, or any path whose changed region
/// is not reliably known). [`FrameDamage::Rects`] carries the exact changed
/// rectangles for a partial upload; it is only produced when every changed pixel is
/// covered by the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameDamage {
    /// Every pixel may have changed — the shell must do a full `set`.
    Full,
    /// Only these (non-empty) rectangles changed — the shell may `set_partial`
    /// each one. Never empty (an empty change produces no frame at all).
    Rects(Vec<DamageRect>),
}

/// Accumulates the damage rectangles a `Framebuffer` painted since the last frame.
///
/// A transport's `Framebuffer` calls [`push`](DamageLog::push) for each rectangle it
/// blits and [`mark_full`](DamageLog::mark_full) for a whole-surface replace / resize;
/// the session drains it once per emitted frame with [`take`](DamageLog::take). The
/// log is a pure *hint*: it never gates whether a frame is emitted (the session's
/// own dirty flag does that), so if it ever disagrees with the dirty flag the shell
/// still falls back to a correct full upload.
#[derive(Clone, Debug, Default)]
pub struct DamageLog {
    /// A whole-surface change happened — the rect list is meaningless, upload all.
    full: bool,
    /// The changed rectangles accumulated since the last drain (when not `full`).
    rects: Vec<DamageRect>,
}

impl DamageLog {
    /// The most rectangles worth tracking before collapsing to a full upload.
    /// Past this the per-rect `set_partial` overhead + the `Vec` growth stop paying
    /// for themselves versus one `set`, and a whole-desktop repaint (which produces
    /// many rectangles) is exactly the case a full upload handles best.
    pub const MAX_RECTS: usize = 64;

    /// A clean log — nothing changed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the whole surface changed (a resize / full-frame replace / a batch of
    /// unknown geometry). Supersedes any accumulated rectangles: once full, the
    /// rectangle list is irrelevant and is dropped.
    pub fn mark_full(&mut self) {
        self.full = true;
        self.rects.clear();
    }

    /// Record one painted rectangle. Empty rectangles are ignored; once the log is
    /// full (or the rectangle list would exceed [`MAX_RECTS`](DamageLog::MAX_RECTS))
    /// it collapses to a full upload rather than track more.
    pub fn push(&mut self, rect: DamageRect) {
        if self.full {
            return;
        }
        if rect.is_empty() {
            return;
        }
        if self.rects.len() >= Self::MAX_RECTS {
            self.mark_full();
            return;
        }
        self.rects.push(rect);
    }

    /// Whether anything has been recorded since the last drain.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.full || !self.rects.is_empty()
    }

    /// Drop everything without producing a [`FrameDamage`] — used to keep the log in
    /// step with the dirty flag when a frame is consumed by the damage-less path.
    pub fn clear(&mut self) {
        self.full = false;
        self.rects.clear();
    }

    /// Take the accumulated damage, resetting the log to clean.
    ///
    /// Returns `None` when nothing changed, [`FrameDamage::Full`] after a
    /// whole-surface change, or [`FrameDamage::Rects`] with the exact changed
    /// rectangles otherwise.
    pub fn take(&mut self) -> Option<FrameDamage> {
        if self.full {
            self.full = false;
            self.rects.clear();
            return Some(FrameDamage::Full);
        }
        if self.rects.is_empty() {
            return None;
        }
        Some(FrameDamage::Rects(std::mem::take(&mut self.rects)))
    }
}

/// Slice the sub-rectangle `rect` out of the full frame `full` as its own
/// [`ColorImage`] plus the top-left offset to place it at — exactly the
/// `(pos, image)` pair `TextureHandle::set_partial` consumes.
///
/// The rectangle is clamped to `full`'s bounds first, so an over-hanging rectangle
/// is trimmed rather than panicking; a rectangle with no visible part yields `None`
/// (the caller simply skips that upload). The returned image's pixels are copied
/// row by row out of `full`, so it is exactly the pixels `full` shows in that
/// region.
#[must_use]
pub fn sub_color_image(full: &ColorImage, rect: DamageRect) -> Option<([usize; 2], ColorImage)> {
    let [fw, fh] = full.size;
    let rect = rect.clamped(fw, fh)?;
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    let mut pixels = Vec::with_capacity(w * h);
    for row in 0..h {
        let start = (y + row) * fw + x;
        pixels.extend_from_slice(&full.pixels[start..start + w]);
    }
    Some((
        [x, y],
        ColorImage {
            size: [w, h],
            pixels,
        },
    ))
}

/// The CPU-side model of `TextureHandle::set_partial`: copy `sub` into the flat
/// `dst` pixel buffer (`dst_width` pixels per row) at `offset`.
///
/// The shell hands the very same `(offset, sub)` pair — produced by
/// [`sub_color_image`] — to egui's GPU `set_partial`; this function is what the
/// unit tests apply against a simulated texture buffer to prove the two paths agree.
/// It is defensively bounds-checked: a row (or a rectangle) that would spill past
/// the destination is skipped rather than panicking, matching `set_partial`'s
/// requirement that the sub-image fit inside the texture.
pub fn paint_sub_image(
    dst: &mut [Color32],
    dst_width: usize,
    offset: [usize; 2],
    sub: &ColorImage,
) {
    let [ox, oy] = offset;
    let [sw, sh] = sub.size;
    if dst_width == 0 || sw == 0 || sh == 0 || ox + sw > dst_width {
        return;
    }
    let dst_height = dst.len() / dst_width;
    for row in 0..sh {
        let dy = oy + row;
        if dy >= dst_height {
            break;
        }
        let d0 = dy * dst_width + ox;
        let s0 = row * sw;
        dst[d0..d0 + sw].copy_from_slice(&sub.pixels[s0..s0 + sw]);
    }
}

#[cfg(test)]
mod tests {
    use super::{paint_sub_image, sub_color_image, DamageLog, DamageRect, FrameDamage};
    use crate::egui::{Color32, ColorImage};

    /// A deterministic W×H test frame — a cheap hash per pixel so neighbouring
    /// pixels differ (a solid image would hide slice/offset bugs). Built from raw
    /// opaque RGBA bytes through egui's own constructor (no hand-minted colours).
    fn pattern(w: usize, h: usize, seed: u32) -> ColorImage {
        let mut rgba = Vec::with_capacity(w * h * 4);
        for i in 0..w * h {
            let v = i
                .wrapping_mul(2_654_435_761)
                .wrapping_add((seed as usize).wrapping_mul(0x9E37_79B9));
            let b = v.to_le_bytes();
            rgba.extend_from_slice(&[b[0], b[1], b[2], 0xFF]);
        }
        ColorImage::from_rgba_unmultiplied([w, h], &rgba)
    }

    /// Overwrite the pixels of `rect` in `img` with a solid colour (a synthetic
    /// "this region changed" edit).
    fn fill_rect(img: &mut ColorImage, rect: DamageRect, color: Color32) {
        let w = img.size[0];
        for yy in rect.y()..rect.y() + rect.h() {
            for xx in rect.x()..rect.x() + rect.w() {
                img.pixels[yy * w + xx] = color;
            }
        }
    }

    // ── DamageRect / clamp math ──────────────────────────────────────────────

    #[test]
    fn clamp_keeps_an_in_bounds_rect_whole() {
        let r = DamageRect::new(2, 3, 4, 5);
        assert_eq!(r.clamped(20, 20), Some(r));
    }

    #[test]
    fn clamp_trims_a_right_bottom_overhang() {
        // A rect running off the right/bottom edge is shrunk to what fits.
        let r = DamageRect::new(8, 8, 5, 5);
        let c = r.clamped(10, 11).expect("partly visible");
        assert_eq!((c.x(), c.y(), c.w(), c.h()), (8, 8, 2, 3));
    }

    #[test]
    fn clamp_rejects_empty_and_fully_outside() {
        assert_eq!(DamageRect::new(0, 0, 0, 4).clamped(10, 10), None, "zero w");
        assert_eq!(DamageRect::new(0, 0, 4, 0).clamped(10, 10), None, "zero h");
        assert_eq!(DamageRect::new(10, 0, 2, 2).clamped(10, 10), None, "x off");
        assert_eq!(DamageRect::new(0, 10, 2, 2).clamped(10, 10), None, "y off");
    }

    #[test]
    fn offset_is_the_top_left_corner() {
        assert_eq!(DamageRect::new(7, 9, 3, 3).offset(), [7, 9]);
    }

    // ── sub_color_image slicing ──────────────────────────────────────────────

    #[test]
    fn sub_image_copies_the_exact_region() {
        let full = pattern(6, 4, 1);
        let (offset, sub) = sub_color_image(&full, DamageRect::new(1, 1, 3, 2)).expect("visible");
        assert_eq!(offset, [1, 1]);
        assert_eq!(sub.size, [3, 2]);
        // Every sub pixel equals the matching full pixel.
        for row in 0..2 {
            for col in 0..3 {
                let full_px = full.pixels[(1 + row) * 6 + (1 + col)];
                assert_eq!(sub.pixels[row * 3 + col], full_px, "px ({col},{row})");
            }
        }
    }

    #[test]
    fn sub_image_of_an_overhanging_rect_is_trimmed() {
        let full = pattern(4, 4, 2);
        let (offset, sub) = sub_color_image(&full, DamageRect::new(2, 2, 9, 9)).expect("visible");
        assert_eq!(offset, [2, 2]);
        assert_eq!(sub.size, [2, 2], "trimmed to the surface");
    }

    #[test]
    fn sub_image_of_a_fully_outside_rect_is_none() {
        let full = pattern(4, 4, 3);
        assert_eq!(sub_color_image(&full, DamageRect::new(4, 0, 2, 2)), None);
        assert_eq!(sub_color_image(&full, DamageRect::new(0, 0, 0, 0)), None);
    }

    #[test]
    fn a_full_screen_rect_slices_the_whole_image() {
        // The "full-screen damage degrades to a full upload" case: the sub-image is
        // the entire frame, so set_partial([0,0], whole) == set(whole).
        let full = pattern(5, 3, 4);
        let (offset, sub) = sub_color_image(&full, DamageRect::new(0, 0, 5, 3)).expect("visible");
        assert_eq!(offset, [0, 0]);
        assert_eq!(sub.size, [5, 3]);
        assert_eq!(sub.pixels, full.pixels, "identical to the whole frame");
    }

    // ── the equivalence proof ────────────────────────────────────────────────

    #[test]
    fn partial_upload_of_damage_rects_equals_a_full_upload() {
        let (w, h) = (20, 12);
        // The frame currently on the GPU (the previous upload).
        let prev = pattern(w, h, 1);
        // The next frame: identical to prev EXCEPT inside three changed rects.
        let mut next = prev.clone();
        let rects = [
            DamageRect::new(2, 3, 5, 4),   // interior block
            DamageRect::new(10, 0, 6, 6),  // touches the top edge
            DamageRect::new(0, 10, 20, 2), // a full-width bottom band
        ];
        // A distinct fill colour, taken from an unrelated pattern pixel so no colour
        // is hand-minted here (the value itself is irrelevant to the equivalence).
        let marker = pattern(1, 1, 777).pixels[0];
        for r in rects {
            fill_rect(&mut next, r, marker);
        }

        // Path A — a full `set`: the texture becomes the whole next frame.
        let full_upload = next.pixels.clone();

        // Path B — a partial upload: start from the previous frame on the GPU and
        // set_partial each damage rect out of the next frame.
        let mut partial_upload = prev.pixels.clone();
        for r in rects {
            let (offset, sub) = sub_color_image(&next, r).expect("visible rect");
            paint_sub_image(&mut partial_upload, w, offset, &sub);
        }

        // Pixel-identical: the partial path reproduced the full frame exactly.
        assert_eq!(partial_upload, full_upload, "partial == full");
        // And an untouched pixel (0,0) still carries the previous frame's value —
        // which equals the next frame there, since only the rects changed.
        assert_eq!(partial_upload[0], prev.pixels[0]);
        assert_eq!(
            prev.pixels[0], next.pixels[0],
            "corner really was unchanged"
        );
    }

    #[test]
    fn overlapping_damage_rects_still_reproduce_the_full_frame() {
        // Damage lists can overlap (a decoder may repaint intersecting regions); the
        // last write wins and must still match the full frame. `next` differs from
        // `prev` ONLY inside the union of the rects, honouring the coverage
        // precondition (every changed pixel is covered by some rect).
        let (w, h) = (8, 8);
        let prev = pattern(w, h, 5);
        let rects = [DamageRect::new(0, 0, 5, 5), DamageRect::new(3, 3, 4, 4)];
        let color_a = pattern(1, 1, 111).pixels[0];
        let color_b = pattern(1, 1, 222).pixels[0];

        // Paint the full next frame rect by rect (in order), so the overlap ends up
        // `color_b` and the changed region is exactly the rects' union.
        let mut next = prev.clone();
        fill_rect(&mut next, rects[0], color_a);
        fill_rect(&mut next, rects[1], color_b);

        // The partial path applies the same rects, in the same order, onto `prev`.
        let mut tex = prev.pixels.clone();
        for r in rects {
            let (offset, sub) = sub_color_image(&next, r).expect("visible");
            paint_sub_image(&mut tex, w, offset, &sub);
        }
        assert_eq!(tex, next.pixels, "overlap resolves to the full frame");
        assert_eq!(
            tex[3 * w + 3],
            color_b,
            "the later rect wins in the overlap"
        );
    }

    #[test]
    fn paint_sub_image_skips_an_out_of_bounds_blit() {
        // Defensive: a sub-image that would spill past the buffer is a no-op, not a
        // panic (set_partial requires the sub fit; this models that contract safely).
        let mut buf = vec![Color32::BLACK; 4 * 4];
        let sub = ColorImage {
            size: [3, 3],
            pixels: vec![Color32::WHITE; 9],
        };
        paint_sub_image(&mut buf, 4, [3, 3], &sub); // 3+3 > 4 wide
        assert!(buf.iter().all(|p| *p == Color32::BLACK), "nothing written");
    }

    // ── DamageLog accumulation ───────────────────────────────────────────────

    #[test]
    fn a_fresh_log_is_clean_and_takes_nothing() {
        let mut log = DamageLog::new();
        assert!(!log.is_dirty());
        assert_eq!(log.take(), None);
    }

    #[test]
    fn pushed_rects_accumulate_and_drain_once() {
        let mut log = DamageLog::new();
        log.push(DamageRect::new(1, 1, 2, 2));
        log.push(DamageRect::new(4, 4, 1, 1));
        log.push(DamageRect::new(0, 0, 0, 0)); // empty → ignored
        assert!(log.is_dirty());
        assert_eq!(
            log.take(),
            Some(FrameDamage::Rects(vec![
                DamageRect::new(1, 1, 2, 2),
                DamageRect::new(4, 4, 1, 1),
            ]))
        );
        // Draining resets to clean.
        assert!(!log.is_dirty());
        assert_eq!(log.take(), None);
    }

    #[test]
    fn mark_full_supersedes_rects() {
        let mut log = DamageLog::new();
        log.push(DamageRect::new(1, 1, 2, 2));
        log.mark_full();
        assert_eq!(log.take(), Some(FrameDamage::Full));
        // A rect pushed after a full mark is dropped until the next drain.
        log.mark_full();
        log.push(DamageRect::new(0, 0, 1, 1));
        assert_eq!(log.take(), Some(FrameDamage::Full));
    }

    #[test]
    fn too_many_rects_collapse_to_full() {
        let mut log = DamageLog::new();
        for i in 0..(DamageLog::MAX_RECTS + 5) {
            log.push(DamageRect::new(i, 0, 1, 1));
        }
        assert_eq!(
            log.take(),
            Some(FrameDamage::Full),
            "capped to a full upload"
        );
    }

    #[test]
    fn clear_drops_damage_without_a_frame() {
        let mut log = DamageLog::new();
        log.push(DamageRect::new(1, 1, 2, 2));
        log.clear();
        assert!(!log.is_dirty());
        assert_eq!(log.take(), None);
    }
}

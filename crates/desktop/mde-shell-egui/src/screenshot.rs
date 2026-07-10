//! Headless screenshot capture for `mde-shell-egui` tests — turns a real egui
//! frame into an actual PNG on disk, with **no GPU, no EGL/GL context, and no
//! DRM device**. Test-only tooling: the whole module is gated `#[cfg(test)]` at
//! its `mod screenshot;` declaration in `main.rs`, so none of it (nor its `png`
//! encode path) ships in the production binary.
//!
//! # Why this exists
//!
//! Five WIN7 chrome-redesign units (WIN7-1..5, see `docs/WORKLIST.md`) landed
//! real layout/visual changes to this crate verified only by layout-assertion
//! tests (`ctx.read_response(id).rect` checks) and accesskit output. Every one
//! of them explicitly flagged that no screenshot/pixel-diff harness exists here
//! — the most recent called it "an accumulating gap... worth a dedicated
//! live-seat smoke pass before the epic is considered presentable." This module
//! is that harness's minimal real form: not a live-seat smoke pass (still
//! valuable, still separate), but a way for a *test* — or a human running one —
//! to actually see what a shell state renders as, on any headless box.
//!
//! # How it works
//!
//! Every render test in this crate already drives the same two real egui calls
//! the DRM runner itself makes each frame (`mde_egui::drm::run_drm`):
//!
//! ```ignore
//! let full_output = ctx.run(raw_input, |ctx| ui(ctx));
//! let clipped = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
//! ```
//!
//! `tessellate` already does the hard part: it turns every painted `Shape`
//! (rects, text, strokes, images, …) into real `epaint::Mesh` triangle soup —
//! positions, UVs into the font/image atlas, and per-vertex premultiplied
//! colors. What's missing is turning THAT into pixels. Production does it with
//! `egui_glow` (a real GLES context over GBM/EGL on a DRM render node, see
//! `mde_egui::drm::run_drm`) or, off the DRM seat, `eframe`'s `wgpu` backend —
//! both real GPU backends, neither usable headless on this workspace's farm
//! build VMs (verified live for this unit: no `/dev/dri` render node, and no
//! software Vulkan/GL ICD installed — see the WIN7-SHOT-1 commit for exactly
//! what was tried).
//!
//! `epaint` itself ships no rasterizer — verified against the vendored 0.31.1
//! source: `tessellator.rs` stops at triangles; there is no pixel-producing
//! module anywhere in the crate. Turning a `Mesh` into pixels is *always* the
//! backend's job. So this module IS a backend: a minimal software one. For
//! every `ClippedPrimitive::Mesh` this walks each triangle with a textbook
//! barycentric edge-function rasterizer, samples the SAME texture atlas data a
//! real backend would have uploaded to the GPU (`FullOutput::textures_delta`,
//! folded through [`Atlas`] in exactly the order `epaint`'s own doc comment
//! requires: `set` before painting, `free` after), and composites with the same
//! premultiplied-alpha "over" operator a fragment shader would. No `unsafe`, no
//! native library, no new dependency: every type used here (`egui`/`epaint`,
//! the `png` encoder) is already a real dependency of this crate for other
//! reasons.
//!
//! This is deliberately NOT pixel-perfect against a real GPU backend — no
//! mipmapping, a plainer coverage path than `egui_glow`'s shader. It is a
//! **verification tool**, not a renderer to ship: its job is "does this frame
//! paint the content a human would recognize," which it proves far more
//! directly than a layout-rect or an accesskit-tree assertion ever could.
//!
//! # Using it
//!
//! Drive frames through a [`Capture`] session using the exact `(ctx, RawInput,
//! ui closure)` shape every `drive`/`run` test helper in this crate already
//! uses (`start_menu.rs`, `dock.rs`, …) — swapping a bare `ctx.run(..)` for a
//! captured one is a one-line change:
//!
//! ```ignore
//! let ctx = egui::Context::default();
//! Style::install(&ctx);
//! let mut shell = Shell::new_for_ctx(&ctx);
//! let input = egui::RawInput {
//!     screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 800.0))),
//!     ..Default::default()
//! };
//! let canvas = screenshot::Capture::new().frame(&ctx, input, |ctx| shell.render(ctx));
//! assert!(!canvas.is_blank());
//! canvas.write_png("/tmp/shell.png").unwrap();
//! ```
//!
//! A [`Capture`] session's [`Atlas`] accumulates texture uploads ACROSS calls
//! to [`Capture::frame`], so a shot taken after one or more warm-up/settle
//! frames (the idiom this crate already uses everywhere a panel needs a frame
//! to latch, e.g. `s.toggle(); run(&ctx, .., 1);` in `start_menu.rs`) still
//! sees the real font atlas + icons on the frame you finally save. A *fresh*
//! atlas per shot would miss them: egui only re-uploads a texture on the frame
//! it actually changes, so capturing frame N in isolation from frame 1 (where
//! the font atlas was actually uploaded) would silently render every glyph as
//! nothing. Reuse ONE `Capture` across every frame of a fixture, settle frames
//! included, and only write out the PNG for the one you actually want to keep.
//!
//! # Known limitation
//!
//! `ClippedPrimitive::Callback` — a raw `egui_glow`/`egui-wgpu` paint callback,
//! an app opting into hand-written GPU draw calls inside its egui frame — has
//! no generic software equivalent and is honestly skipped, not faked. Nothing
//! in the shell chrome / Start Menu / dock path uses one (those surfaces paint
//! only `Shape`s through the normal `Ui`/`Painter` API); a future surface that
//! DID use a paint callback would show a gap exactly where it drew, not a
//! silent wrong pixel.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)] // A software rasterizer is inherently pixel-coordinate <-> float arithmetic;
   // mirrors the same allow `mde_egui::drm`'s `norm` closure carries for the
   // identical reason (real, bounded UI/display coordinates, never adversarial
   // input), rather than threading `try_from`/`.round() as` through every line.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

use mde_egui::egui::{
    self, epaint::Primitive, Color32, ColorImage, ImageData, Mesh, TextureId, TexturesDelta,
};

/// The canvas's initial fill — a neutral near-black distinct from every real
/// Carbon-token surface color this shell paints (all lighter), so a genuinely
/// blank capture is visually obvious in the PNG itself, not just caught by
/// [`Canvas::is_blank`].
const CLEAR: Color32 = Color32::from_rgb(18, 18, 18);

/// A software-rasterized frame: fully-opaque RGBA8 (see the compositing note on
/// [`Capture::frame`]), row-major top-to-bottom, sized in PHYSICAL pixels
/// (`screen_rect points * pixels_per_point` — real HiDPI shells fold a
/// fractional `pixels_per_point` in, see `mde_egui::drm::run_drm`'s SURFACE-7
/// scale detect; this canvas honors it exactly the way a real backend's
/// framebuffer would).
pub(crate) struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
}

impl Canvas {
    fn blank(width: usize, height: usize) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            width,
            height,
            pixels: vec![CLEAR; width * height],
        }
    }

    pub(crate) const fn width(&self) -> usize {
        self.width
    }

    pub(crate) const fn height(&self) -> usize {
        self.height
    }

    /// Whether every pixel is identical — the same "a real, non-degenerate
    /// frame came back" gate `mde_media_core::VideoFrame::is_blank` proved
    /// BUG-VIDEO-1's pixel path with this same session, applied here to the
    /// rasterizer's own output: a wired-but-broken raster path (or a shell that
    /// painted nothing) leaves the canvas exactly [`CLEAR`] everywhere; real
    /// content — even a single flat-colored panel — breaks the uniformity.
    pub(crate) fn is_blank(&self) -> bool {
        self.pixels
            .first()
            .is_none_or(|first| self.pixels.iter().all(|p| p == first))
    }

    /// Write this canvas as a PNG, creating its parent directory if needed.
    ///
    /// Encoded RGB (no alpha channel): every stored pixel is fully opaque by
    /// construction (see [`Capture::frame`]'s compositing note), so an alpha
    /// channel would carry no information — dropping it also sidesteps any
    /// premultiplied-vs-straight ambiguity a viewer might apply to it.
    ///
    /// # Errors
    /// Any I/O or encode failure (a missing/unwritable path, a PNG encoder
    /// error) surfaces as the real `io::Error` rather than a panic.
    pub(crate) fn write_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut encoder = png::Encoder::new(
            BufWriter::new(File::create(path)?),
            self.width as u32,
            self.height as u32,
        );
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        let mut bytes = Vec::with_capacity(self.width * self.height * 3);
        for p in &self.pixels {
            bytes.extend_from_slice(&[p.r(), p.g(), p.b()]);
        }
        writer.write_image_data(&bytes).map_err(io::Error::other)?;
        Ok(())
    }
}

/// The CPU-side mirror of every texture a real backend would have uploaded to
/// the GPU — `TextureId` keyed, exactly like `egui_glow::Painter`'s own atlas.
/// Folds [`TexturesDelta`] in over a session so a capture taken after warm-up
/// frames still has the font atlas + every image uploaded on an earlier frame
/// (see the module doc's "using it" section).
#[derive(Default)]
struct Atlas {
    textures: HashMap<TextureId, ColorImage>,
}

impl Atlas {
    /// Apply every `set` (full or partial) — call BEFORE rasterizing the frame
    /// that produced `delta`, matching `FullOutput::textures_delta`'s doc.
    fn apply_set(&mut self, delta: &TexturesDelta) {
        for (id, image_delta) in &delta.set {
            let [w, h] = image_delta.image.size();
            let pixels: Vec<Color32> = match &image_delta.image {
                ImageData::Color(img) => img.pixels.clone(),
                // The font atlas ships as single-channel coverage; `srgba_pixels`
                // is `epaint::FontImage`'s own documented conversion to the
                // premultiplied white-with-alpha a backend actually uploads.
                ImageData::Font(img) => img.srgba_pixels(None).collect(),
            };
            match image_delta.pos {
                None => {
                    self.textures.insert(
                        *id,
                        ColorImage {
                            size: [w, h],
                            pixels,
                        },
                    );
                }
                Some([px, py]) => {
                    let Some(existing) = self.textures.get_mut(id) else {
                        // A patch for a texture this session never saw a full
                        // upload for — shouldn't happen (egui always fully
                        // uploads a texture before it ever patches it), and
                        // honestly dropped rather than guessed at if it did.
                        continue;
                    };
                    let ew = existing.size[0];
                    let eh = existing.size[1];
                    for row in 0..h {
                        let dst_y = py + row;
                        if dst_y >= eh {
                            break;
                        }
                        let take = w.min(ew.saturating_sub(px));
                        let src_start = row * w;
                        let dst_start = dst_y * ew + px;
                        existing.pixels[dst_start..dst_start + take]
                            .copy_from_slice(&pixels[src_start..src_start + take]);
                    }
                }
            }
        }
    }

    /// Drop every freed id — call AFTER rasterizing (the other half of the
    /// `set`-before / `free`-after contract).
    fn apply_free(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// Bilinear-sample `id` at normalized `uv` — matching `TextureOptions::
    /// LINEAR`, egui's default magnification filter for both the font atlas and
    /// user images, so glyph edges look like the real thing rather than a
    /// blocky nearest-neighbor stand-in.
    ///
    /// An unknown id (never uploaded this session) resolves to opaque WHITE —
    /// the multiplicative identity for [`modulate`], which is exactly correct
    /// for the overwhelmingly common case (a solid-fill triangle sampling the
    /// atlas's white texel) and an honest, visible-if-wrong fallback otherwise.
    fn sample(&self, id: TextureId, uv: egui::Pos2) -> Color32 {
        let Some(tex) = self.textures.get(&id) else {
            return Color32::WHITE;
        };
        let [w, h] = tex.size;
        if w == 0 || h == 0 {
            return Color32::TRANSPARENT;
        }
        let (wf, hf) = (w as f32, h as f32);
        let fx = (uv.x * wf - 0.5).clamp(0.0, wf - 1.0);
        let fy = (uv.y * hf - 0.5).clamp(0.0, hf - 1.0);
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(w - 1);
        let y1 = (y0 + 1).min(h - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let at = |x: usize, y: usize| tex.pixels[y * w + x];
        let top = lerp2(at(x0, y0), at(x1, y0), tx);
        let bot = lerp2(at(x0, y1), at(x1, y1), tx);
        lerp2(top, bot, ty)
    }
}

/// A capture session: one persistent [`Atlas`] threaded across as many frames
/// as a fixture needs to drive (see the module doc's "using it" section).
#[derive(Default)]
pub(crate) struct Capture {
    atlas: Atlas,
}

impl Capture {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Run one real egui frame — `ctx.run` + `ctx.tessellate`, the exact pair
    /// `mde_egui::drm::run_drm` calls each frame — and rasterize it to a fresh
    /// [`Canvas`].
    ///
    /// Compositing note: the canvas starts fully OPAQUE ([`CLEAR`], alpha
    /// 255), and premultiplied "over" onto a fully-opaque destination is
    /// provably fully-opaque again (`a_out = src.a + dst.a*(1-src.a) = src.a +
    /// (255-src.a) = 255` for any `src`) — so the whole canvas stays opaque
    /// forever, by induction over every triangle painted. That's what makes
    /// [`Canvas::write_png`]'s alpha-free RGB encode lossless rather than a
    /// simplification.
    pub(crate) fn frame(
        &mut self,
        ctx: &egui::Context,
        input: egui::RawInput,
        mut ui: impl FnMut(&egui::Context),
    ) -> Canvas {
        let output = ctx.run(input, &mut ui);
        self.atlas.apply_set(&output.textures_delta);

        let screen = ctx.screen_rect();
        let ppp = output.pixels_per_point;
        let width = (screen.width() * ppp).round().max(1.0) as usize;
        let height = (screen.height() * ppp).round().max(1.0) as usize;
        let mut canvas = Canvas::blank(width, height);

        let clipped = ctx.tessellate(output.shapes, ppp);
        for cp in &clipped {
            let Primitive::Mesh(mesh) = &cp.primitive else {
                continue; // Callback primitive — see the module doc's known limitation.
            };
            rasterize_mesh(&mut canvas, mesh, cp.clip_rect, ppp, &self.atlas);
        }

        self.atlas.apply_free(&output.textures_delta);
        canvas
    }
}

/// Rasterize every triangle of `mesh` into `canvas`, clipped to `clip_rect`
/// (converted points -> physical pixels by `ppp`, exactly like the vertex
/// positions below).
fn rasterize_mesh(
    canvas: &mut Canvas,
    mesh: &Mesh,
    clip_rect: egui::Rect,
    ppp: f32,
    atlas: &Atlas,
) {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        return;
    }
    let canvas_w = canvas.width;
    let canvas_h = canvas.height;
    let clip_x0 = (clip_rect.left() * ppp).max(0.0);
    let clip_y0 = (clip_rect.top() * ppp).max(0.0);
    let clip_x1 = (clip_rect.right() * ppp).min(canvas_w as f32);
    let clip_y1 = (clip_rect.bottom() * ppp).min(canvas_h as f32);
    if clip_x1 <= clip_x0 || clip_y1 <= clip_y0 {
        return;
    }

    for tri in mesh.indices.chunks_exact(3) {
        let v0 = &mesh.vertices[tri[0] as usize];
        let v1 = &mesh.vertices[tri[1] as usize];
        let v2 = &mesh.vertices[tri[2] as usize];
        let p0 = egui::pos2(v0.pos.x * ppp, v0.pos.y * ppp);
        let p1 = egui::pos2(v1.pos.x * ppp, v1.pos.y * ppp);
        let p2 = egui::pos2(v2.pos.x * ppp, v2.pos.y * ppp);

        let area = edge(p0, p1, p2);
        if area.abs() < 1e-6 {
            continue; // degenerate (zero-area) triangle
        }

        let min_x = p0.x.min(p1.x).min(p2.x).floor().max(clip_x0);
        let min_y = p0.y.min(p1.y).min(p2.y).floor().max(clip_y0);
        let max_x = p0.x.max(p1.x).max(p2.x).ceil().min(clip_x1);
        let max_y = p0.y.max(p1.y).max(p2.y).ceil().min(clip_y1);
        if max_x <= min_x || max_y <= min_y {
            continue;
        }

        for y in (min_y as usize)..(max_y as usize) {
            for x in (min_x as usize)..(max_x as usize) {
                let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge(p1, p2, p);
                let w1 = edge(p2, p0, p);
                let w2 = edge(p0, p1, p);
                // `Mesh::indices`' own doc: "egui is NOT consistent with what
                // winding order it uses, so turn off backface culling" — accept
                // either sign as long as all three edge tests agree.
                let inside =
                    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
                if !inside {
                    continue;
                }
                let b0 = w0 / area;
                let b1 = w1 / area;
                let b2 = w2 / area;
                let uv = egui::pos2(
                    b0 * v0.uv.x + b1 * v1.uv.x + b2 * v2.uv.x,
                    b0 * v0.uv.y + b1 * v1.uv.y + b2 * v2.uv.y,
                );
                let vertex_color = lerp3(v0.color, b0, v1.color, b1, v2.color, b2);
                let texel = atlas.sample(mesh.texture_id, uv);
                let src = modulate(texel, vertex_color);
                if src.a() == 0 {
                    continue;
                }
                let idx = y * canvas_w + x;
                canvas.pixels[idx] = over(canvas.pixels[idx], src);
            }
        }
    }
}

/// The 2D edge function: positive/negative/zero exactly as `c` is left-of,
/// right-of, or on the directed line `a -> b`. Its magnitude is twice the
/// signed area of triangle `(a, b, c)` — used both as the barycentric
/// denominator and, per-pixel, as the three (consistently-signed-when-inside)
/// numerators.
fn edge(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// Linear interpolation between two premultiplied colors.
fn lerp2(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| {
        (f32::from(x) + (f32::from(y) - f32::from(x)) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// Barycentric interpolation across three premultiplied vertex colors — the
/// software equivalent of GPU Gouraud shading, exactly what egui's own
/// antialiasing feather (a per-vertex alpha ramp on the outer edge of a shape)
/// depends on to look smooth rather than a hard-edged silhouette.
fn lerp3(c0: Color32, w0: f32, c1: Color32, w1: f32, c2: Color32, w2: f32) -> Color32 {
    let mix = |a: u8, b: u8, c: u8| {
        (f32::from(a) * w0 + f32::from(b) * w1 + f32::from(c) * w2)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_premultiplied(
        mix(c0.r(), c1.r(), c2.r()),
        mix(c0.g(), c1.g(), c2.g()),
        mix(c0.b(), c1.b(), c2.b()),
        mix(c0.a(), c1.a(), c2.a()),
    )
}

/// Componentwise premultiplied modulation (`texture_sample * vertex_color`) —
/// the same multiply a GPU fragment shader does; both inputs are already
/// premultiplied sRGB8, so this is a plain per-channel product scaled back to
/// the `0..=255` range.
fn modulate(tex: Color32, vertex: Color32) -> Color32 {
    let mix = |a: u8, b: u8| ((u16::from(a) * u16::from(b) + 127) / 255) as u8;
    Color32::from_rgba_premultiplied(
        mix(tex.r(), vertex.r()),
        mix(tex.g(), vertex.g()),
        mix(tex.b(), vertex.b()),
        mix(tex.a(), vertex.a()),
    )
}

/// Premultiplied-alpha "over": `dst = src + dst * (1 - src.a)`. Also correctly
/// handles egui's "additive" colors (`src.a == 0`, per `Color32`'s own doc) as
/// a natural special case — `inv` is `255` then, so this reduces to `dst =
/// src + dst`, the additive-blend formula, with no separate branch needed.
fn over(dst: Color32, src: Color32) -> Color32 {
    let inv = 255 - u16::from(src.a());
    let mix = |d: u8| ((u16::from(d) * inv) / 255) as u8;
    Color32::from_rgba_premultiplied(
        src.r().saturating_add(mix(dst.r())),
        src.g().saturating_add(mix(dst.g())),
        src.b().saturating_add(mix(dst.b())),
        src.a().saturating_add(mix(dst.a())),
    )
}

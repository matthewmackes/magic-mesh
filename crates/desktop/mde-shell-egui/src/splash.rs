//! Construct boot-splash: the Construct identity painted while the
//! shell initializes, with real init progress animated along the loading bar
//! (`docs/design/construct-branding.md`, locks 8 + 11).
//!
//! The visible composition is native text from [`mde_theme::brand::logo`], so
//! the Construct product name is not tied to legacy raster wordmark art.
//!
//! **The bar animates honest progress, never a timer.** The three milestones are
//! the real work the shell does before its first dock frame (see the boot driver
//! in `main.rs`):
//!
//! 1. [`Milestone::Seat`] — the DRM/KMS seat + wgpu renderer came up (`run_drm`
//!    finishes that init before any frame callback can run);
//! 2. [`Milestone::Surfaces`] — `Shell::new_for_ctx` returned, so every surface
//!    backend is constructed (music worker, media core, files browser, voice SIP
//!    agent, the terminal's real PTY, …);
//! 3. [`Milestone::MeshSnapshot`] — the shell's first poll of the world-readable
//!    mesh-status snapshot completed (the same `/run/mde/mesh-status.json` fold
//!    the chrome bar runs on its cadence; an absent snapshot on a fresh host
//!    completes the poll honestly rather than hanging boot).
//!
//! When the embedded artwork matches the legacy measured bar geometry, the
//! one-time decode can rebuild that bar for animation **from the artwork's own
//! pixels** (no colours are invented, §4). New Construct artwork does not expose
//! that measured geometry, so it uses the native token progress bar instead:
//! background, Construct labels, track, and fill all come from `Style`.
//!
//! The splash owns the screen until every milestone lands **and** the eased bar
//! reaches full, then dismisses: the first dock frame replaces it.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, …); the boot driver in main.rs consumes them"
)]

use mde_egui::egui::{self, Align2, Color32, FontId, Rect, TextureHandle, TextureOptions};
use mde_egui::{Motion, MotionPreset, Style};

use crate::chooser::decode_png_rgba;

/// The official Construct boot-splash artwork (lock 11), embedded like the BRAND-1 lockup
/// so the splash renders with no filesystem / RPM-path dependency.
const ARTWORK: &[u8] = include_bytes!("../../../../assets/brand/CONSTRUCT-WALLPAPER1.png");

// ─────────────────── legacy measured-bar geometry ───────────────────
//
// Coordinates INTO the previous measured splash artwork, in its native pixels.
// Guarded by [`ART_W`]×[`ART_H`]: the current Construct artwork falls back to
// the native token fill (no harvested bar animation) instead of misreading
// pixels.

/// The artwork's native width in pixels.
const ART_W: usize = 1672;
/// The artwork's native height in pixels.
const ART_H: usize = 941;
/// The loading-bar track's left edge.
const TRACK_X0: usize = 648;
/// The loading-bar track's right edge (exclusive).
const TRACK_X1: usize = 1024;
/// Top of the bar band (the fill + head-dot glow rows), inclusive.
const BAND_Y0: usize = 824;
/// Bottom of the bar band, exclusive.
const BAND_Y1: usize = 844;
/// The baked gradient's last pure column (exclusive) — the head-dot glow starts
/// here, so the gradient resample stops before it.
const GRAD_X1: usize = 846;
/// The head-dot sprite's columns (the bright dot + its glow).
const HEAD_X0: usize = 846;
/// The head-dot sprite's right edge (exclusive).
const HEAD_X1: usize = 867;
/// The columns rewritten to the empty track (the baked fill + dot, with margin
/// for the glow bleed).
const REWRITE_X0: usize = 646;
/// Right edge (exclusive) of the rewritten span — everything beyond is already
/// the artwork's empty track.
const REWRITE_X1: usize = 872;
/// A column safely inside the artwork's empty track, sampled per-row as the
/// template the rewritten span copies (the artwork's own "0 %" appearance).
const TRACK_TPL_X: usize = 950;
/// The egui-memory animation key easing the drawn fill toward the banked
/// milestone fraction (through the shared `Motion` table, lock 10 idiom).
const EASE_KEY: &str = "construct-splash-progress";

/// The eased fill fraction at which the full bar counts as visually settled and
/// the splash may dismiss.
const EASE_DONE: f32 = 0.999;

// ──────────────────────────── milestones ────────────────────────────

/// The real init milestones the shell completes before its first dock frame —
/// each is banked by the boot driver in `main.rs` the moment the actual work
/// finishes (never a timer, §7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Milestone {
    /// The DRM/KMS (or windowed) seat + wgpu renderer are up — proven by the
    /// first frame callback running at all (`run_drm` completes that init
    /// before it can call back).
    Seat,
    /// `Shell::new_for_ctx` returned: every surface backend the shell owns is
    /// constructed (music worker, media core, files browser, voice SIP agent,
    /// the terminal's real PTY, …).
    Surfaces,
    /// The shell's first mesh-status snapshot poll completed — the same
    /// world-readable `/run/mde/mesh-status.json` fold the chrome bar runs.
    /// An absent snapshot completes the poll honestly (boot never hangs on it).
    MeshSnapshot,
}

/// How many milestones a boot has — the bar's denominator.
const MILESTONE_COUNT: usize = 3;

impl Milestone {
    /// This milestone's slot in the done-set.
    const fn index(self) -> usize {
        match self {
            Self::Seat => 0,
            Self::Surfaces => 1,
            Self::MeshSnapshot => 2,
        }
    }
}

// ──────────────────────────── the splash ────────────────────────────

/// The prepared artwork, uploaded once on the first paint.
struct SplashArt {
    /// The animated bar: the full-width gradient fill strip + the head-dot
    /// sprite. `None` when the embedded artwork isn't the measured asset
    /// (fail-soft to a native token fill).
    bar: Option<BarArt>,
}

/// The two bar textures the progress overlay draws.
struct BarArt {
    /// The baked gradient resampled across the whole track — drawn UV-clipped
    /// to the eased progress fraction.
    fill: TextureHandle,
    /// The luminance-keyed head dot, riding the fill's leading edge.
    head: TextureHandle,
}

/// The artwork's decode/upload lifecycle — resolved exactly once, on the first
/// paint.
#[derive(Default)]
enum ArtState {
    /// Not yet decoded (before the first paint).
    #[default]
    Pending,
    /// Decoded + uploaded.
    Ready(SplashArt),
    /// The embedded asset failed to decode — fail-soft to the native token
    /// progress fill, never a panic (§7). Kept so the decode is never
    /// re-attempted per frame.
    Missing,
}

/// The boot-splash state: the once-decoded artwork, the banked milestones, and
/// the last eased bar fraction (which gates dismissal).
#[derive(Default)]
pub(crate) struct Splash {
    /// The artwork, lazily prepared on the first paint.
    art: ArtState,
    /// Which milestones have completed.
    done: [bool; MILESTONE_COUNT],
    /// The eased fill fraction the bar last painted.
    eased: f32,
}

impl Splash {
    /// Bank a completed milestone (idempotent — re-banking is a no-op).
    pub(crate) const fn complete(&mut self, milestone: Milestone) {
        self.done[milestone.index()] = true;
    }

    /// Whether a milestone has been banked.
    pub(crate) const fn is_complete(&self, milestone: Milestone) -> bool {
        self.done[milestone.index()]
    }

    /// The banked progress fraction — completed milestones over the total. The
    /// bar's *target*; the drawn fill eases toward it.
    #[allow(
        clippy::cast_precision_loss,
        reason = "milestone counts are tiny; the usize→f32 fraction is exact"
    )]
    fn progress(&self) -> f32 {
        let completed = self.done.iter().filter(|d| **d).count();
        completed as f32 / MILESTONE_COUNT as f32
    }

    /// Whether every milestone has completed (init is done).
    pub(crate) fn finished(&self) -> bool {
        self.done.iter().all(|d| *d)
    }

    /// Whether the splash has fully played out — init finished AND the eased
    /// bar reached full — so the first dock frame may replace it.
    pub(crate) fn dismissed(&self) -> bool {
        self.finished() && self.eased >= EASE_DONE
    }

    /// Paint one full-screen splash frame: the shell field, Construct identity,
    /// and the progress overlay.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        // Ease the drawn fill toward the banked fraction through the shared
        // Motion table (a fresh context starts at the target, so the bar never
        // rewinds; each later bank glides).
        self.eased = Motion::animate_scalar(ctx, EASE_KEY, self.progress(), MotionPreset::Page)
            .value()
            .clamp(0.0, 1.0);
        let eased = self.eased;
        let bar = self.art(ctx).and_then(|art| art.bar.as_ref());

        egui::CentralPanel::default().show(ctx, |ui| {
            let free = ui.max_rect();
            let painter = ui.painter().clone();
            painter.rect_filled(free, 0.0, Style::BG);

            let center = free.center();
            let title_y = center.y - Style::SP_XL * 1.55;
            painter.text(
                egui::pos2(center.x, title_y),
                Align2::CENTER_CENTER,
                mde_theme::brand::logo::PRODUCT_NAME,
                FontId::proportional(Style::DISPLAY * 2.0),
                Style::TEXT,
            );
            painter.text(
                egui::pos2(center.x, title_y + Style::SP_XL),
                Align2::CENTER_CENTER,
                mde_theme::brand::logo::SOFTWARE_STUDIO,
                FontId::proportional(Style::TITLE),
                Style::TEXT_DIM,
            );
            painter.text(
                egui::pos2(center.x, title_y + Style::SP_XL * 2.05),
                Align2::CENTER_CENTER,
                mde_theme::brand::logo::PRODUCT_RELEASE,
                FontId::proportional(Style::SMALL),
                Style::TEXT_DIM,
            );

            let track_w = (free.width() - Style::SP_XL * 2.0).max(96.0).min(520.0);
            let track = Rect::from_center_size(
                egui::pos2(center.x, center.y + Style::SP_XL * 2.7),
                egui::vec2(track_w, Style::SP_M),
            );
            painter.rect_filled(track, Style::RADIUS, Style::SURFACE);
            painter.rect_stroke(
                track,
                Style::RADIUS,
                Style::hairline(),
                egui::StrokeKind::Inside,
            );

            let head_x = track.width().mul_add(eased, track.left());
            if eased > 0.0 {
                let fill = Rect::from_min_max(track.min, egui::pos2(head_x, track.max.y));
                if let Some(bar) = bar {
                    egui::Image::new(egui::load::SizedTexture::new(bar.fill.id(), fill.size()))
                        .uv(Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(eased, 1.0),
                        ))
                        .paint_at(ui, fill);
                } else {
                    painter.rect_filled(fill, Style::RADIUS, Style::ACCENT);
                }
            }

            let head_center = egui::pos2(head_x, track.center().y);
            if let Some(bar) = bar {
                let head_size = egui::vec2(track.height() * head_aspect(), track.height());
                let head = Rect::from_center_size(head_center, head_size);
                egui::Image::new(egui::load::SizedTexture::new(bar.head.id(), head.size()))
                    .paint_at(ui, head);
            } else {
                painter.circle_filled(head_center, track.height() * 0.48, Style::ACCENT);
            }
        });
    }

    /// The prepared artwork, decoded + uploaded exactly once on the first paint
    /// (the resolved-or-failed result is kept, so neither the 1.6 MP decode nor
    /// the upload ever repeats).
    fn art(&mut self, ctx: &egui::Context) -> Option<&SplashArt> {
        if matches!(self.art, ArtState::Pending) {
            self.art = upload(ctx).map_or(ArtState::Missing, ArtState::Ready);
        }
        match &self.art {
            ArtState::Ready(art) => Some(art),
            ArtState::Pending | ArtState::Missing => None,
        }
    }
}

// ──────────────────────────── geometry helpers ────────────────────────────

/// A native artwork dimension as `f32` (exact — the artwork is far below 2²⁴).
#[allow(
    clippy::cast_precision_loss,
    reason = "artwork pixel coordinates are far below f32's exact-integer range"
)]
const fn art_dim(px: usize) -> f32 {
    px as f32
}

/// The head-dot sprite's aspect ratio (width over height).
const fn head_aspect() -> f32 {
    art_dim(HEAD_X1 - HEAD_X0) / art_dim(BAND_Y1 - BAND_Y0)
}

// ──────────────────────────── artwork preparation ────────────────────────────

/// Decode the embedded artwork and upload the prepared textures. `None` (never
/// a panic) if the asset can't decode — the caller fails soft to the bare
/// Carbon field (§7).
fn upload(ctx: &egui::Context) -> Option<SplashArt> {
    let artwork = decode_png_rgba(ARTWORK)?;
    let (_base, bar) = prepare(&artwork);
    Some(SplashArt {
        bar: bar.map(|(fill, head)| BarArt {
            fill: ctx.load_texture("construct-splash-fill", fill, TextureOptions::LINEAR),
            head: ctx.load_texture("construct-splash-head", head, TextureOptions::LINEAR),
        }),
    })
}

/// Rebuild the artwork's baked bar for animation, **from its own pixels only**:
///
/// * the **base** is the artwork with the baked fill + head-dot span rewritten
///   to the empty track (each band row copies the artwork's own track template
///   column), so the bar starts honestly empty;
/// * the **fill strip** is the baked blue→magenta gradient resampled across the
///   full track width, drawn UV-clipped to the progress fraction;
/// * the **head sprite** is the baked head dot, luminance-keyed to alpha so its
///   glow blends over fill and track alike at any progress.
///
/// A non-measured artwork (dimension guard) yields no bar — the caller paints
/// the native token fill instead of misreading pixel coordinates.
fn prepare(
    art: &egui::ColorImage,
) -> (
    egui::ColorImage,
    Option<(egui::ColorImage, egui::ColorImage)>,
) {
    let mut base = art.clone();
    if art.size != [ART_W, ART_H] {
        return (base, None);
    }

    // The empty track: rewrite the baked fill + dot band from the artwork's own
    // track template column.
    for y in BAND_Y0..BAND_Y1 {
        let template = art.pixels[y * ART_W + TRACK_TPL_X];
        base.pixels[y * ART_W + REWRITE_X0..y * ART_W + REWRITE_X1].fill(template);
    }

    // The full-width fill strip: the baked gradient columns resampled across
    // the whole track span (nearest — the gradient is smooth).
    let strip_w = TRACK_X1 - TRACK_X0;
    let strip_h = BAND_Y1 - BAND_Y0;
    let mut fill = egui::ColorImage::new([strip_w, strip_h], Color32::TRANSPARENT);
    for i in 0..strip_w {
        let src_x = TRACK_X0 + i * (GRAD_X1 - TRACK_X0) / strip_w;
        for j in 0..strip_h {
            fill.pixels[j * strip_w + i] = art.pixels[(BAND_Y0 + j) * ART_W + src_x];
        }
    }

    // The head-dot sprite, luminance-keyed: alpha follows the brightest channel
    // so the dot's dark surround goes transparent and its glow blends.
    let head_w = HEAD_X1 - HEAD_X0;
    let mut head = egui::ColorImage::new([head_w, strip_h], Color32::TRANSPARENT);
    for j in 0..strip_h {
        for i in 0..head_w {
            let p = art.pixels[(BAND_Y0 + j) * ART_W + HEAD_X0 + i];
            // Luminance-key the head-dot alpha in the shared kit — the dark
            // surround fades out, the glow blends (§4: no colour minted here).
            head.pixels[j * head_w + i] = Style::key_alpha_to_luma(p);
        }
    }

    (base, Some((fill, head)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2};

    /// The embedded Construct artwork decodes at its native size and uses the
    /// native token progress fill when the source image does not expose a
    /// measured progress-bar band.
    #[test]
    fn the_embedded_construct_artwork_decodes_and_uses_native_progress_fill() {
        let art = decode_png_rgba(ARTWORK).expect("the embedded artwork decodes");
        assert_eq!(art.size, [1408, 768], "native Construct artwork size");

        let (base, bar) = prepare(&art);
        assert_eq!(base.size, art.size);
        assert!(
            bar.is_none(),
            "unknown artwork geometry must not be sampled"
        );
    }

    /// A swapped (non-measured) artwork must NOT be misread through the fixed
    /// pixel geometry — it falls back to the plain image, no bar.
    #[test]
    fn a_non_measured_artwork_yields_no_bar() {
        let other = egui::ColorImage::new([64, 64], Color32::BLACK);
        let (base, bar) = prepare(&other);
        assert_eq!(base.size, [64, 64]);
        assert!(bar.is_none(), "fixed geometry applied to unknown artwork");
    }

    /// Progress is the banked milestone fraction — it advances only as real
    /// milestones complete, is idempotent per milestone, and finishes exactly
    /// when all three have landed (never a timer).
    #[test]
    #[allow(clippy::float_cmp, reason = "exact fractions of a 3-way split")]
    fn progress_advances_across_the_real_milestones() {
        let mut s = Splash::default();
        assert_eq!(s.progress(), 0.0);
        assert!(!s.finished());

        s.complete(Milestone::Seat);
        assert!(s.is_complete(Milestone::Seat));
        assert_eq!(s.progress(), 1.0 / 3.0);

        // Idempotent: re-banking the same milestone moves nothing.
        s.complete(Milestone::Seat);
        assert_eq!(s.progress(), 1.0 / 3.0);

        s.complete(Milestone::Surfaces);
        assert_eq!(s.progress(), 2.0 / 3.0);
        assert!(!s.finished(), "finished before the snapshot poll");

        s.complete(Milestone::MeshSnapshot);
        assert_eq!(s.progress(), 1.0);
        assert!(s.finished());
    }

    /// Drive headless splash frames through the same `Context::run` →
    /// `tessellate` path the DRM runner uses: the splash paints Construct
    /// identity + progress (real draw primitives), holds the screen while
    /// milestones are outstanding, and dismisses once init completes and the
    /// eased bar settles — the first dock frame replaces it.
    #[test]
    fn splash_renders_then_dismisses_when_init_completes() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut splash = Splash::default();

        let frame = |splash: &mut Splash, time: f64| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
                time: Some(time),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| splash.show(ctx));
            ctx.tessellate(out.shapes, out.pixels_per_point)
        };

        // First boot frame: the splash paints Construct identity + progress and is
        // nowhere near dismissed.
        let prims = frame(&mut splash, 0.0);
        assert!(!prims.is_empty(), "the splash painted no draw primitives");
        assert!(!splash.dismissed(), "dismissed before any milestone");

        // Milestones land mid-boot; the splash still owns the screen while the
        // eased bar is in flight.
        splash.complete(Milestone::Seat);
        splash.complete(Milestone::Surfaces);
        frame(&mut splash, 0.05);

        splash.complete(Milestone::MeshSnapshot);
        assert!(splash.finished());

        // Once the ease has fully settled across normal frames, the splash
        // dismisses and hands the screen to the first dock frame. A single long
        // time jump intentionally does not fast-forward the shared motion carrier.
        frame(&mut splash, 0.1);
        let mut prims = Vec::new();
        for frame_idx in 7..40 {
            prims = frame(&mut splash, f64::from(frame_idx) / 60.0);
        }
        assert!(
            !prims.is_empty(),
            "the settling splash frame painted nothing"
        );
        assert!(
            splash.dismissed(),
            "init complete + bar settled, yet the splash still owns the screen"
        );
    }

    /// The visible release line the splash paints stays independent from the
    /// internal build semver/codename.
    #[test]
    fn the_splash_version_line_is_the_visible_product_release() {
        assert_eq!(mde_theme::brand::logo::PRODUCT_RELEASE, "Release 1.0 BETA");
    }
}

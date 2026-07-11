//! QBRAND-4 — the DRM **boot-splash**: the official Quazar artwork painted while
//! the shell initializes, with real init progress animated along the artwork's
//! own loading bar (`docs/design/quasar-branding.md`, locks 8 + 11).
//!
//! The image is the operator-locked `MDE-QUAZAR-WALLPAPER1.png` — the centered
//! mesh-node mark + "MDE Quazar" wordmark with a loading bar composed into the
//! artwork near the bottom. The splash letterboxes it on the Carbon field
//! (scale-to-fit, aspect preserved, never stretched) and renders
//! [`mde_theme::brand::build::version_line()`] beneath the wordmark in the dim
//! Carbon text token.
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
//! The artwork bakes its bar at a fixed decorative fraction, so the one-time
//! decode rebuilds it for animation **from the artwork's own pixels** (no colours
//! are invented, §4): the baked fill + head-dot band is rewritten to the empty
//! track (sampled from the artwork's track), the baked gradient is resampled
//! across the full track width into a fill strip, and the head dot becomes a
//! luminance-keyed sprite that rides the fill's leading edge. The strip is drawn
//! UV-clipped to the eased progress fraction. Everything the shell *adds* — the
//! field behind the letterbox and the version line — is a `Style` token.
//!
//! The splash owns the screen until every milestone lands **and** the eased bar
//! reaches full, then dismisses: the first dock frame replaces it.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, …); the boot driver in main.rs consumes them"
)]

use mde_egui::egui::{self, Align2, Color32, FontId, Rect, TextureHandle, TextureOptions};
use mde_egui::{Motion, Style};

use crate::chooser::decode_png_rgba;

/// The official boot-splash artwork (lock 11), embedded like the BRAND-1 lockup
/// so the splash renders with no filesystem / RPM-path dependency.
const ARTWORK: &[u8] = include_bytes!("../../../../assets/brand/MDE-QUAZAR-WALLPAPER1.png");

// ─────────────────── the artwork's measured geometry ───────────────────
//
// Coordinates INTO the official artwork, in its native pixels — data about the
// locked asset (measured from `MDE-QUAZAR-WALLPAPER1.png`), not theme metrics.
// Guarded by [`ART_W`]×[`ART_H`]: a swapped artwork falls back to the plain
// letterboxed image (no bar animation) instead of misreading pixels.

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
/// The version line's centre row — the open band between the wordmark's subtitle
/// (ends ≈ row 776) and the bar band (starts row 824).
const VERSION_CY: usize = 800;

/// The egui-memory animation key easing the drawn fill toward the banked
/// milestone fraction (through the shared `Motion` table, lock 10 idiom).
const EASE_KEY: &str = "qbrand4-splash-progress";

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
    /// The full artwork with the bar band rewritten to the empty track.
    base: TextureHandle,
    /// The animated bar: the full-width gradient fill strip + the head-dot
    /// sprite. `None` when the embedded artwork isn't the measured asset
    /// (fail-soft to the plain image).
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
    /// The embedded asset failed to decode — fail-soft to the bare Carbon
    /// field + version line, never a panic (§7). Kept so the decode is never
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

    /// Paint one full-screen splash frame: the Carbon field, the letterboxed
    /// artwork, the progress overlay along the artwork's bar, and the version
    /// line beneath the wordmark.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        // Ease the drawn fill toward the banked fraction through the shared
        // Motion table (a fresh context starts at the target, so the bar never
        // rewinds; each later bank glides).
        self.eased = ctx
            .animate_value_with_time(egui::Id::new(EASE_KEY), self.progress(), Motion::SLOW)
            .clamp(0.0, 1.0);
        let eased = self.eased;
        let art = self.art(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            let free = ui.max_rect();
            // The Carbon field behind the letterbox — a Style token, never a
            // raw hex (§4). A painter clone so `Image::paint_at` can borrow `ui`.
            let painter = ui.painter().clone();
            painter.rect_filled(free, 0.0, Style::BG);

            // Fail-soft: an undecodable embedded asset still boots to an honest
            // Carbon field + version line (§7) while the milestones play out.
            let Some(art) = art else {
                painter.text(
                    free.center(),
                    Align2::CENTER_CENTER,
                    mde_theme::brand::build::version_line(),
                    FontId::proportional(Style::SMALL),
                    Style::TEXT_DIM,
                );
                return;
            };

            // Letterbox: scale to fit, preserve aspect, never stretch.
            let img = letterbox(free, art_size());
            egui::Image::new(egui::load::SizedTexture::new(art.base.id(), img.size()))
                .paint_at(ui, img);

            // The progress overlay along the artwork's own bar region: the
            // resampled gradient strip UV-clipped to the eased fraction, the
            // head dot at its leading edge.
            if let Some(bar) = &art.bar {
                let band = map_rect(img, TRACK_X0, BAND_Y0, TRACK_X1, BAND_Y1);
                let head_x = band.width().mul_add(eased, band.min.x);
                if eased > 0.0 {
                    let fill = Rect::from_min_max(band.min, egui::pos2(head_x, band.max.y));
                    egui::Image::new(egui::load::SizedTexture::new(bar.fill.id(), fill.size()))
                        .uv(Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(eased, 1.0),
                        ))
                        .paint_at(ui, fill);
                }
                let head_w = band.height() * head_aspect();
                let head = Rect::from_center_size(
                    egui::pos2(head_x, band.center().y),
                    egui::vec2(head_w, band.height()),
                );
                egui::Image::new(egui::load::SizedTexture::new(bar.head.id(), head.size()))
                    .paint_at(ui, head);
            }

            // The version line beneath the artwork's wordmark, in the dim Carbon
            // token, scaled with the artwork so the composition holds at any
            // resolution.
            let font = Style::SMALL * (img.height() / art_dim(ART_H));
            painter.text(
                egui::pos2(
                    img.center().x,
                    img.height().mul_add(band_frac(VERSION_CY), img.top()),
                ),
                Align2::CENTER_CENTER,
                mde_theme::brand::build::version_line(),
                FontId::proportional(font),
                Style::TEXT_DIM,
            );
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

/// The artwork's native size as a vector.
const fn art_size() -> egui::Vec2 {
    egui::vec2(art_dim(ART_W), art_dim(ART_H))
}

/// A native artwork dimension as `f32` (exact — the artwork is far below 2²⁴).
#[allow(
    clippy::cast_precision_loss,
    reason = "artwork pixel coordinates are far below f32's exact-integer range"
)]
const fn art_dim(px: usize) -> f32 {
    px as f32
}

/// A native artwork row as a fraction of the artwork height.
const fn band_frac(y: usize) -> f32 {
    art_dim(y) / art_dim(ART_H)
}

/// The head-dot sprite's aspect ratio (width over height).
const fn head_aspect() -> f32 {
    art_dim(HEAD_X1 - HEAD_X0) / art_dim(BAND_Y1 - BAND_Y0)
}

/// Centre `size` inside `free` scaled to fit — aspect preserved, letterboxed on
/// whichever axis has slack, never stretched.
fn letterbox(free: Rect, size: egui::Vec2) -> Rect {
    let scale = (free.width() / size.x).min(free.height() / size.y);
    Rect::from_center_size(free.center(), size * scale)
}

/// Map a native-artwork pixel rect into the painted (letterboxed) image rect.
fn map_rect(img: Rect, x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    let sx = img.width() / art_dim(ART_W);
    let sy = img.height() / art_dim(ART_H);
    Rect::from_min_max(
        egui::pos2(
            art_dim(x0).mul_add(sx, img.left()),
            art_dim(y0).mul_add(sy, img.top()),
        ),
        egui::pos2(
            art_dim(x1).mul_add(sx, img.left()),
            art_dim(y1).mul_add(sy, img.top()),
        ),
    )
}

// ──────────────────────────── artwork preparation ────────────────────────────

/// Decode the embedded artwork and upload the prepared textures. `None` (never
/// a panic) if the asset can't decode — the caller fails soft to the bare
/// Carbon field (§7).
fn upload(ctx: &egui::Context) -> Option<SplashArt> {
    let artwork = decode_png_rgba(ARTWORK)?;
    let (base, bar) = prepare(&artwork);
    Some(SplashArt {
        base: ctx.load_texture("qbrand4-splash-base", base, TextureOptions::LINEAR),
        bar: bar.map(|(fill, head)| BarArt {
            fill: ctx.load_texture("qbrand4-splash-fill", fill, TextureOptions::LINEAR),
            head: ctx.load_texture("qbrand4-splash-head", head, TextureOptions::LINEAR),
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
/// the plain letterboxed image instead of misreading pixel coordinates.
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

    /// The embedded official artwork decodes at its measured native size and
    /// prepares the animated bar — proving the asset is the locked
    /// `MDE-QUAZAR-WALLPAPER1.png` (lock 11), not a stray or re-exported file
    /// the measured geometry would misread.
    #[test]
    fn the_embedded_artwork_decodes_and_prepares_the_bar() {
        let art = decode_png_rgba(ARTWORK).expect("the embedded artwork decodes");
        assert_eq!(art.size, [ART_W, ART_H], "native artwork size");

        let (base, bar) = prepare(&art);
        let (fill, head) = bar.expect("the measured artwork yields the animated bar");
        assert_eq!(fill.size, [TRACK_X1 - TRACK_X0, BAND_Y1 - BAND_Y0]);
        assert_eq!(head.size, [HEAD_X1 - HEAD_X0, BAND_Y1 - BAND_Y0]);

        // The base's bar starts empty: where the baked gradient sat, the pixel
        // now matches the dim track (no blue/magenta fill left behind).
        let mid_fill = base.pixels[833 * ART_W + 700];
        assert!(
            mid_fill.r() < 60 && mid_fill.b() < 60,
            "baked fill still visible in the base: {mid_fill:?}"
        );

        // The fill strip carries the artwork's gradient across the FULL track:
        // blue-dominant at the far left, magenta at the far right.
        let row = 833 - BAND_Y0;
        let left = fill.pixels[row * fill.size[0] + 2];
        let right = fill.pixels[row * fill.size[0] + fill.size[0] - 2];
        assert!(
            left.b() > 150 && left.b() > left.r(),
            "left of strip: {left:?}"
        );
        assert!(right.r() > 150, "right of strip: {right:?}");

        // The head sprite's centre is the bright dot (opaque); its corner glow
        // is keyed toward transparent.
        let centre = head.pixels[row * head.size[0] + head.size[0] / 2];
        assert!(centre.a() > 200, "head dot centre not opaque: {centre:?}");
        let corner = head.pixels[0];
        assert!(corner.a() < 80, "head sprite corner not keyed: {corner:?}");
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
    /// `tessellate` path the DRM runner uses: the splash paints the artwork +
    /// version line (real draw primitives), holds the screen while milestones
    /// are outstanding, and dismisses once init completes and the eased bar
    /// settles — the first dock frame replaces it.
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

        // First boot frame: the splash paints (artwork + version line) and is
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

        // Once the ease has fully settled (well past Motion::SLOW), the splash
        // dismisses and hands the screen to the first dock frame.
        frame(&mut splash, 0.1);
        let prims = frame(&mut splash, 5.0);
        assert!(
            !prims.is_empty(),
            "the settling splash frame painted nothing"
        );
        assert!(
            splash.dismissed(),
            "init complete + bar settled, yet the splash still owns the screen"
        );
    }

    /// The version line the splash paints is the QBRAND-1 build identity —
    /// semver + the locked "Quazar" codename (lock 9).
    #[test]
    fn the_splash_version_line_is_the_brand_build_line() {
        let line = mde_theme::brand::build::version_line();
        assert!(
            line.contains("\"Quazar\""),
            "not the locked codename: {line}"
        );
    }
}

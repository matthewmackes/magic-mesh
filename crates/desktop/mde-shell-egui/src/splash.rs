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

use std::fs;

use mde_egui::egui::{self, Align2, Color32, Rect, TextureHandle, TextureOptions};
use mde_egui::{Motion, MotionPreset, Style, TypographyRole};

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
const BOOT_STATUS_FILE: &str = "/run/mde/boot-status.tsv";

#[derive(Clone, Debug, PartialEq, Eq)]
struct BootService {
    unit: String,
    label: String,
    active: String,
    sub: String,
    result: String,
}

fn boot_services() -> Vec<BootService> {
    let Ok(raw) = fs::read_to_string(BOOT_STATUS_FILE) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let unit = fields.next()?;
            if unit == "version" {
                return None;
            }
            Some(BootService {
                unit: unit.to_owned(),
                label: fields.next()?.to_owned(),
                active: fields.next()?.to_owned(),
                sub: fields.next()?.to_owned(),
                result: fields.next()?.to_owned(),
            })
        })
        .take(16)
        .collect()
}

/// Token-derived geometry for the responsive boot-service roster. Keeping the
/// fold pure makes compact/wide behavior deterministic without a render fixture
/// having to rediscover the layout from painted pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BootServiceLayout {
    compact: bool,
    row_h: f32,
    gap: f32,
    meter_w: f32,
    label_chars: usize,
}

fn boot_service_layout(width: f32) -> BootServiceLayout {
    // Seventeen XL steps plus one M step preserves the established 560 pt collapse
    // boundary while making its relationship to the shared spacing ladder
    // explicit. The dimensions below likewise preserve the existing rendered
    // geometry exactly, but no longer form a local spacing/type scale.
    let compact = width < Style::SP_XL * 17.0 + Style::SP_M;
    BootServiceLayout {
        compact,
        row_h: if compact {
            Style::HEADING + Style::SP_XS * 0.25
        } else {
            Style::SP_L
        },
        gap: if compact {
            Style::SP_XS * 0.75
        } else {
            Style::SP_XS
        },
        meter_w: if compact {
            Style::HEADING * 2.0 + Style::SP_XS * 0.5
        } else {
            Style::SP_L * 3.0 + Style::SP_XS
        },
        label_chars: if compact { 24 } else { usize::MAX },
    }
}

fn boot_service_style(ctx: &egui::Context, service: &BootService, blink: bool) -> (char, Color32) {
    match service.active.as_str() {
        "active" => ('✓', Style::resolve_color(ctx, Style::SUPPORT_SUCCESS)),
        "failed" if blink => ('✕', Style::resolve_color(ctx, Style::SUPPORT_ERROR)),
        "failed" => (
            '✕',
            Style::resolve_color(ctx, Style::SUPPORT_ERROR).gamma_multiply(0.65),
        ),
        "skipped" | "inactive" => ('·', Style::resolve_color(ctx, Style::TEXT_DIM)),
        _ => ('◌', Style::resolve_color(ctx, Style::SUPPORT_WARNING)),
    }
}

fn paint_boot_services(
    ctx: &egui::Context,
    painter: &egui::Painter,
    free: Rect,
    track: Rect,
    services: &[BootService],
) {
    if services.is_empty() {
        return;
    }
    let layout = boot_service_layout(free.width());
    let surface = Style::resolve_color(ctx, Style::SURFACE);
    let background = Style::resolve_color(ctx, Style::BG);
    let text = Style::resolve_color(ctx, Style::TEXT);
    let text_dim = Style::resolve_color(ctx, Style::TEXT_DIM);
    let top = track.bottom() + Style::SP_M;
    let available_h = (free.bottom() - top - Style::SP_M).max(0.0);
    let max_rows = (available_h / (layout.row_h + layout.gap)).floor() as usize;
    let count = services.len().min(max_rows.max(1));
    let blink = (ctx.input(|input| input.time) * 2.0) as u64 % 2 == 0;
    for (index, service) in services.iter().take(count).enumerate() {
        let y = top + index as f32 * (layout.row_h + layout.gap);
        let row = Rect::from_min_max(
            egui::pos2(free.left() + Style::SP_M, y),
            egui::pos2(free.right() - Style::SP_M, y + layout.row_h),
        );
        painter.rect_filled(row, Style::RADIUS_S, surface);
        let (glyph, color) = boot_service_style(ctx, service, blink);
        painter.text(
            egui::pos2(row.left() + Style::SP_S, row.center().y),
            Align2::LEFT_CENTER,
            glyph,
            Style::typography_font(TypographyRole::Label),
            color,
        );
        let label_max = (row.width() - layout.meter_w - Style::SP_XL * 1.5).max(Style::SP_L);
        let label = if layout.compact {
            service
                .label
                .chars()
                .take(layout.label_chars)
                .collect::<String>()
        } else {
            service.label.clone()
        };
        let galley = painter.layout_job(Style::typography_job(
            &label,
            TypographyRole::Caption,
            text,
            label_max,
        ));
        painter.galley(
            egui::pos2(
                row.left() + Style::SP_XL,
                row.center().y - galley.size().y / 2.0,
            ),
            galley,
            text,
        );
        let meter = Rect::from_min_max(
            egui::pos2(
                row.right() - layout.meter_w - Style::SP_S,
                row.top() + layout.row_h * 0.33,
            ),
            egui::pos2(
                row.right() - Style::SP_S,
                row.bottom() - layout.row_h * 0.33,
            ),
        );
        let filled = service.active == "active";
        painter.rect_filled(meter, Style::RADIUS_S, background);
        if filled {
            painter.rect_filled(meter, Style::RADIUS_S, color.gamma_multiply(0.8));
        } else if service.active != "failed" && service.active != "skipped" {
            let phase = ((ctx.input(|input| input.time) * 6.0) as usize + index) % 6;
            let segment_w = (meter.width() / 6.0 - 1.0).max(1.0);
            let segment = Rect::from_min_max(
                egui::pos2(meter.left() + phase as f32 * (segment_w + 1.0), meter.top()),
                egui::pos2(
                    (meter.left() + (phase as f32 + 1.0) * (segment_w + 1.0) - 1.0)
                        .min(meter.right()),
                    meter.bottom(),
                ),
            );
            painter.rect_filled(segment, Style::RADIUS_S, color);
        }
        if service.active != "active" && service.active != "skipped" {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
    if count < services.len() {
        painter.text(
            egui::pos2(free.center().x, free.bottom() - Style::SP_XS),
            Align2::CENTER_BOTTOM,
            format!("+{} more node services", services.len() - count),
            Style::typography_font(TypographyRole::Caption),
            text_dim,
        );
    }
}

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

    /// The systemd feed is allowed to settle only when every boot-relevant
    /// discovered node service is active, intentionally skipped,
    /// inactive/oneshot-complete, or failed and therefore visible as a
    /// terminal red result. Periodic timer jobs are deliberately excluded:
    /// their short `activating` window must never hold the desktop splash open
    /// after the initial boot graph has completed. An absent feed is treated as
    /// settled so recovery and minimal images never deadlock the graphical
    /// shell on an optional status helper.
    pub(crate) fn services_settled(&self) -> bool {
        boot_services().iter().all(|service| {
            if matches!(
                service.unit.as_str(),
                "mesh-health.service" | "mesh-status.service"
            ) {
                return true;
            }
            matches!(
                service.active.as_str(),
                "active" | "skipped" | "inactive" | "failed"
            )
        })
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
                Style::typography_font(TypographyRole::Display),
                Style::TEXT,
            );
            painter.text(
                egui::pos2(center.x, title_y + Style::SP_XL),
                Align2::CENTER_CENTER,
                mde_theme::brand::logo::SOFTWARE_STUDIO,
                Style::typography_font(TypographyRole::Headline),
                Style::TEXT_DIM,
            );
            painter.text(
                egui::pos2(center.x, title_y + Style::SP_XL * 2.05),
                Align2::CENTER_CENTER,
                mde_theme::brand::logo::PRODUCT_RELEASE,
                Style::typography_font(TypographyRole::Caption),
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

            paint_boot_services(ctx, &painter, free, track, &boot_services());
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

    /// UX-009: the boot roster has one shared-style geometry fold at both
    /// responsive widths, and semantic ink follows the installed appearance.
    #[test]
    fn splash_boot_service_layout_is_shared_style_responsive_and_appearance_aware() {
        let compact = boot_service_layout(Style::SP_XL * 10.0);
        let wide = boot_service_layout(Style::SP_XL * 20.0);

        assert_eq!(
            compact,
            BootServiceLayout {
                compact: true,
                row_h: Style::HEADING + Style::SP_XS * 0.25,
                gap: Style::SP_XS * 0.75,
                meter_w: Style::HEADING * 2.0 + Style::SP_XS * 0.5,
                label_chars: 24,
            }
        );
        assert_eq!(
            wide,
            BootServiceLayout {
                compact: false,
                row_h: Style::SP_L,
                gap: Style::SP_XS,
                meter_w: Style::SP_L * 3.0 + Style::SP_XS,
                label_chars: usize::MAX,
            }
        );
        assert!(compact.meter_w < wide.meter_w);

        let service = BootService {
            unit: "mde-shell.service".to_owned(),
            label: "Construct shell".to_owned(),
            active: "active".to_owned(),
            sub: "running".to_owned(),
            result: "success".to_owned(),
        };
        let dark = egui::Context::default();
        Style::install(&dark);
        let light = egui::Context::default();
        Style::install_color_scheme_with_density(
            &light,
            mde_egui::StyleColorScheme::Light,
            mde_egui::Density::Mouse,
        );
        let (_, dark_tone) = boot_service_style(&dark, &service, false);
        let (_, light_tone) = boot_service_style(&light, &service, false);
        assert_eq!(
            dark_tone,
            Style::resolve_color(&dark, Style::SUPPORT_SUCCESS)
        );
        assert_eq!(
            light_tone,
            Style::resolve_color(&light, Style::SUPPORT_SUCCESS)
        );
        assert_ne!(
            Style::resolve_color(&dark, Style::SURFACE),
            Style::resolve_color(&light, Style::SURFACE),
            "the roster must not pin the dark surface in Light appearance"
        );
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
        assert_eq!(
            mde_theme::brand::logo::PRODUCT_RELEASE,
            concat!("Release ", env!("CARGO_PKG_VERSION"))
        );
    }
}

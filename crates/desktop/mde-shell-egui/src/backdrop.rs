//! The shell's desktop backdrop: the bottom-most layer is the shell background
//! colour, with the watermark and any honest status text above it.
//!
//! Under E12 "Construct" the shell IS the desktop, so "the empty desktop" is the
//! Desktop surface with nothing brokered in: the [`crate::vdi`] no-desktop state and
//! the [`crate::discovery`]/[`crate::chooser`] empty root desktop. Both paint the
//! this one helper, with any honest status relocated to a small block low on the
//! field (§7 honesty preserved).
//!
//! NAVBAR-W10-3 (`docs/design/workbench-navbar.md` lock W12) rides this same layer:
//! a Windows-10-activation-style **ghost watermark** — the product mark, the brand
//! version line, and the node identity — right-aligned in the field's bottom-right
//! corner, a margin above where the taskbar mounts. Faded [`Style::TEXT_DIM`] ink
//! with no backing, so it reads like the Win10 "Activate Windows" text: visible,
//! never competing. It paints wherever the backdrop paints; the role is honestly
//! omitted when no `role.toml` is pinned (§7).
//!
//! NAVBAR-W10-6 (lock W12b) makes that watermark a **live link to the About surface**:
//! the union of the three ghost lines is one hover/click target — hover brightens the
//! ink one token step (the affordance; still no tooltip, W6) and a click latches an
//! [`Surface::About`] nav-request into egui memory, which the shell drains through its
//! one nav ([`take_nav_request`]). The backdrop layer owns no surface state, so the
//! request rides the same egui-memory seam the wallpaper cache does — the `take_*`
//! drain idiom the picker/desktop faces (`chooser::take_connect` /
//! `vdi::take_return_to_chrome`) already use.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, …); the shell's surfaces consume them"
)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, FontId, Rect, TextureHandle, TextureOptions, Vec2};
use mde_egui::{Motion, Style};

use mde_theme::brand;
use serde::{Deserialize, Serialize};

use mackes_mesh_types::peers::default_workgroup_root;

use crate::chooser::decode_png_rgba;
use crate::dock::Surface;

/// The installed wallpaper directory — QBRAND drops the Construct wallpapers
/// here in the RPM. The selected one is loaded from disk at runtime.
const WALLPAPER_DIR: &str = "/usr/share/backgrounds/magic-mesh";

/// The default wallpaper, embedded so a fresh / dev shell (or a host missing the
/// installed set) always has a backdrop with no filesystem / RPM-path dependency.
/// Only `WALLPAPER4` is carried — embedding all five would add ~6 MB to the binary,
/// so the rest load from [`WALLPAPER_DIR`] (disk-honest, §4).
const DEFAULT_WALLPAPER: &[u8] =
    include_bytes!("../../../../assets/brand/CONSTRUCT-WALLPAPER4.png");

/// The share subdirectory the per-seat wallpaper prefs record lives under
/// (`<root>/wallpaper-prefs/<identity>/<seat>.json`) — the same layout
/// `chooser/chooser_prefs.rs` uses for the Chooser prefs.
const PREFS_SUBDIR: &str = "wallpaper-prefs";

/// How often [`wallpaper_texture`] re-reads the persisted selection off disk. A
/// human-paced throttle so a wallpaper roamed from another seat surfaces within a
/// few seconds without a per-frame disk scan.
const PREF_REFRESH: Duration = Duration::from_secs(3);

/// The dim Carbon scrim faded in as a surface/window covers the desktop, so
/// foreground content (a populated Chooser grid, a session) stays legible over the
/// artwork. The empty desktop shows the wallpaper at full strength (lock #12).
const COVERED_SCRIM: f32 = 0.55;

/// The status block's vertical anchor as a fraction of the field height — low on the
/// field, clear of the artwork's centred wordmark.
const STATUS_Y_FRAC: f32 = 0.74;

/// The Carbon backing opacity behind the honest status block, so it reads over any
/// wallpaper region (§4 token, never a raw hex).
const STATUS_BACKING_ALPHA: f32 = 0.5;

/// The watermark's product mark — the first, slightly larger ghost line.
const WATERMARK_PRODUCT: &str = mde_theme::brand::logo::PRODUCT_NAME;

/// The ghost emphasis of the watermark ink: [`Style::TEXT_DIM`] faded further to
/// the Win10 "Activate Windows" register — visible over the artwork, never
/// competing with content (§4: a faded token, never a raw hex).
const WATERMARK_GHOST: f32 = 0.6;

/// The stable egui id of the watermark's one hover/click target (lock W12b) — the
/// union of the three ghost lines, so the whole mark is a single About affordance
/// with no dead zone between the lines.
const WATERMARK_LINK_ID: &str = "shell-watermark-about-link";

/// The egui-memory key the watermark's [`Surface::About`] nav-request latches under
/// (lock W12b). The backdrop layer owns no surface state, so a click stows the target
/// here for the shell to drain next frame ([`take_nav_request`]) — the same memory
/// seam the wallpaper cache rides.
const NAV_REQUEST_KEY: &str = "shell-backdrop-nav-request";

/// The pinned deployment role file — `mde-role`'s canonical path. The watermark
/// reads it directly (only the pinned token is wanted for the node line, not the
/// fail-closed role gate), honoring the same `MDE_ROLE_PATH` override
/// `mde_role::default_role_path` honors.
const ROLE_PATH: &str = "/var/lib/mde/role.toml";

/// The shared cover-scrim animation key. Exactly one backdrop paints per frame (the
/// shell shows one central view at a time), so a single key yields a continuous eased
/// crossfade across every empty↔covered transition.
const CROSSFADE_KEY: &str = "shell-desktop-backdrop-coverage";

/// The egui-memory key the resolved wallpaper (choice + decoded texture) is cached
/// under, so the selection read + decode/upload happen off the throttle, never per
/// frame.
const WALLPAPER_CACHE_KEY: &str = "shell-desktop-wallpaper";

/// Whether the display the backdrop paints on is empty (the wallpaper at full
/// strength) or covered by a surface/window (dimmed under the cover scrim). Resolved
/// per display by the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Coverage {
    /// Nothing covers the display — the wallpaper at full strength.
    Empty,
    /// A surface/window is open — the wallpaper dims under the cover scrim so the
    /// foreground content reads.
    Covered,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusAnchor {
    Low,
    Center,
}

/// One of the five generated Construct wallpapers (placement lock #12). `Four` is the
/// default; all five ship in the RPM as a selectable set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wallpaper {
    /// `CONSTRUCT-WALLPAPER1.png`.
    One,
    /// `CONSTRUCT-WALLPAPER2.png`.
    Two,
    /// `CONSTRUCT-WALLPAPER3.png`.
    Three,
    /// `CONSTRUCT-WALLPAPER4.png` — the default desktop wallpaper (lock #12).
    Four,
    /// `CONSTRUCT-WALLPAPER5.png`.
    Five,
}

impl Wallpaper {
    /// The full selectable set, in order — the picker's option list.
    pub(crate) const ALL: [Self; 5] = [Self::One, Self::Two, Self::Three, Self::Four, Self::Five];

    /// The default desktop wallpaper (placement lock #12).
    pub(crate) const DEFAULT: Self = Self::Four;

    /// This wallpaper's 1-based index (1–5) — the persisted register + asset suffix.
    const fn index(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Five => 5,
        }
    }

    /// The wallpaper a 1-based index names, or `None` for an out-of-range value (a
    /// corrupt/legacy record degrades to the default, never a panic).
    const fn from_index(index: u8) -> Option<Self> {
        match index {
            1 => Some(Self::One),
            2 => Some(Self::Two),
            3 => Some(Self::Three),
            4 => Some(Self::Four),
            5 => Some(Self::Five),
            _ => None,
        }
    }

    /// The picker caption.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::One => "Wallpaper 1",
            Self::Two => "Wallpaper 2",
            Self::Three => "Wallpaper 3",
            Self::Four => "Wallpaper 4 (default)",
            Self::Five => "Wallpaper 5",
        }
    }

    /// The asset filename under [`WALLPAPER_DIR`].
    const fn filename(self) -> &'static str {
        match self {
            Self::One => "CONSTRUCT-WALLPAPER1.png",
            Self::Two => "CONSTRUCT-WALLPAPER2.png",
            Self::Three => "CONSTRUCT-WALLPAPER3.png",
            Self::Four => "CONSTRUCT-WALLPAPER4.png",
            Self::Five => "CONSTRUCT-WALLPAPER5.png",
        }
    }

    /// The installed absolute path this wallpaper loads from at runtime.
    fn installed_path(self) -> PathBuf {
        Path::new(WALLPAPER_DIR).join(self.filename())
    }
}

/// Paint the shell-colour backdrop as the bottom-most layer of the current panel:
/// the solid Carbon §4 field, an eased cover scrim when a surface/window is open,
/// and — when `status` is given — an honest status block placed low on the field.
///
/// Call this FIRST in the panel body: it draws through the painter and consumes no
/// layout, so the panel's other widgets lay out over it (a covered display's grid
/// floats above the scrim).
pub(crate) fn show(ui: &egui::Ui, coverage: Coverage, status: Option<(&str, &str)>) {
    show_with_status_anchor(ui, coverage, status, StatusAnchor::Low);
}

/// Paint the backdrop with its honest status block centered in the workspace.
/// Used by the Desktop chooser's empty state, where the status itself is the
/// primary content rather than a secondary note under a session/backdrop.
pub(crate) fn show_centered_status(
    ui: &egui::Ui,
    coverage: Coverage,
    status: Option<(&str, &str)>,
) {
    show_with_status_anchor(ui, coverage, status, StatusAnchor::Center);
}

fn show_with_status_anchor(
    ui: &egui::Ui,
    coverage: Coverage,
    status: Option<(&str, &str)>,
    status_anchor: StatusAnchor,
) {
    let free = ui.max_rect();

    // The Carbon §4 field is the background. No wallpaper image is painted here.
    let painter = ui.painter().clone();
    painter.rect_filled(free, 0.0, Style::BG);

    // Ease the cover scrim toward the coverage target (a continuous crossfade across
    // every empty↔covered transition).
    let empty = coverage == Coverage::Empty;
    let reveal = Motion::animate(ui.ctx(), CROSSFADE_KEY, empty, Motion::SLOW);
    let scrim = (1.0 - reveal) * COVERED_SCRIM;
    if scrim > f32::EPSILON {
        painter.rect_filled(free, 0.0, Style::BG.gamma_multiply(scrim));
    }

    // NAVBAR-W10-3 (lock W12) — the brand watermark: three ghost lines in the
    // bottom-right, clear of the taskbar. Painted over the scrim so its ghost
    // weight holds on empty and covered displays alike. NAVBAR-W10-6 (lock W12b):
    // it's a live About link — `ui` carries the hover/click interaction.
    paint_watermark(ui, &painter, free, ui.ctx().screen_rect());

    // Any honest status (the empty-desktop copy, a gated-transport note) — a small
    // block low on the field, over a subtle backing so it reads over the artwork.
    if let Some((title, detail)) = status {
        paint_status(&painter, free, title, detail, status_anchor);
    }

    // Keep the frame alive while the cover-scrim crossfade is mid-flight.
    if reveal > 0.001 && reveal < 0.999 {
        ui.ctx().request_repaint();
    }
}

/// The UV rect that cover-fills a texture of size `tex` into a target of size `free`:
/// the aspect is preserved and the overflow axis is centre-cropped (the image is
/// scaled to *cover* the target, never stretched). A degenerate zero dimension maps
/// to the full texture rather than dividing by zero.
const fn cover_uv(free: Vec2, tex: Vec2) -> Rect {
    let full = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if free.x <= 0.0 || free.y <= 0.0 || tex.x <= 0.0 || tex.y <= 0.0 {
        return full;
    }
    let target = free.x / free.y;
    let image = tex.x / tex.y;
    if image > target {
        // The image is wider than the target — crop it horizontally.
        let w = target / image;
        let x0 = (1.0 - w) / 2.0;
        Rect::from_min_max(egui::pos2(x0, 0.0), egui::pos2(x0 + w, 1.0))
    } else {
        // The image is taller than (or as wide as) the target — crop it vertically.
        let h = image / target;
        let y0 = (1.0 - h) / 2.0;
        Rect::from_min_max(egui::pos2(0.0, y0), egui::pos2(1.0, y0 + h))
    }
}

/// Paint the status title + detail centred low on the field, over a subtle Carbon
/// backing so the honest status stays legible over any wallpaper region (§4). The
/// detail wraps to the free width so a long caption never runs off-panel.
fn paint_status(
    painter: &egui::Painter,
    free: Rect,
    title: &str,
    detail: &str,
    anchor: StatusAnchor,
) {
    let center_x = free.center().x;
    let title_galley = painter.layout_no_wrap(
        title.to_owned(),
        FontId::proportional(Style::BODY),
        Style::TEXT,
    );
    let wrap = Style::SP_XL.mul_add(-2.0, free.width()).max(Style::SP_XL);
    let detail_galley = painter.layout(
        detail.to_owned(),
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
        wrap,
    );

    let block_w = title_galley.size().x.max(detail_galley.size().x);
    let block_h = title_galley.size().y + Style::SP_XS + detail_galley.size().y;
    let top = match anchor {
        StatusAnchor::Low => free.height().mul_add(STATUS_Y_FRAC, free.top()),
        StatusAnchor::Center => free.center().y - block_h / 2.0,
    };
    let backing = Rect::from_center_size(
        egui::pos2(center_x, top + block_h / 2.0),
        egui::vec2(
            Style::SP_L.mul_add(2.0, block_w),
            Style::SP_S.mul_add(2.0, block_h),
        ),
    );
    painter.rect_filled(
        backing,
        Style::RADIUS,
        Style::BG.gamma_multiply(STATUS_BACKING_ALPHA),
    );

    let title_pos = egui::pos2(center_x - title_galley.size().x / 2.0, top);
    let detail_pos = egui::pos2(
        center_x - detail_galley.size().x / 2.0,
        top + title_galley.size().y + Style::SP_XS,
    );
    painter.galley(title_pos, title_galley, Style::TEXT);
    painter.galley(detail_pos, detail_galley, Style::TEXT_DIM);
}

/// NAVBAR-W10-3 (lock W12) — paint the brand watermark: the Win10-activation-style
/// ghost block, three right-aligned lines stacked in the field's bottom-right
/// corner, a margin above where the taskbar mounts. The product mark sits slightly
/// larger over the version + node lines; all three in faded [`Style::TEXT_DIM`]
/// ink with no backing, so the mark reads over the artwork without competing.
///
/// NAVBAR-W10-6 (lock W12b) makes the block a **live About link**: the union of the
/// three text rects is one hover/click target — hover brightens the ink one token
/// step ([`watermark_ink`]; no tooltip, W6) and a click latches an
/// [`Surface::About`] nav-request the shell drains ([`take_nav_request`]).
fn paint_watermark(ui: &egui::Ui, painter: &egui::Painter, free: Rect, screen: Rect) {
    let [product, version, node] = watermark();
    // Lay the three lines out once at a placeholder colour (a galley's size is
    // ink-independent), so a single layout serves both the resting ghost and the
    // hover-brightened link — the ink is applied as a paint-time override below.
    let galleys = [
        painter.layout_no_wrap(
            product.clone(),
            FontId::proportional(Style::BODY),
            egui::Color32::PLACEHOLDER,
        ),
        painter.layout_no_wrap(
            version.clone(),
            FontId::proportional(Style::SMALL),
            egui::Color32::PLACEHOLDER,
        ),
        painter.layout_no_wrap(
            node.clone(),
            FontId::proportional(Style::SMALL),
            egui::Color32::PLACEHOLDER,
        ),
    ];

    // The taskbar mounts at the screen's bottom edge, so in the mounted shell the
    // backdrop's own rect already ends at the bar's top (the bottom panel is
    // reserved first); on a bar-less display (a headless frame) the screen-based
    // bound still keeps the watermark clear of where the bar sits. `min` takes
    // whichever bound is higher on screen — the mark is always above the bar.
    let right = free.right() - Style::SP_L;
    let mut bottom = free.bottom().min(screen.bottom() - crate::dock::TASKBAR_H) - Style::SP_L;

    // Place each line bottom-up (node at the anchor, version above it, product on
    // top), recording its paint position — the layout is byte-for-byte the W10-3
    // stack, split out only so the union of the line rects can be one click target.
    let mut positions = [egui::Pos2::ZERO; 3];
    for (galley, pos) in galleys.iter().zip(positions.iter_mut()).rev() {
        let size = galley.size();
        bottom -= size.y;
        *pos = egui::pos2(right - size.x, bottom);
        bottom -= Style::SP_XS;
    }

    // The union of the three text rects is the single hover/click region (W12b — the
    // click region IS the text, never a dead zone around or between the lines).
    let mut region = Rect::NOTHING;
    for (galley, &pos) in galleys.iter().zip(positions.iter()) {
        region = region.union(Rect::from_min_size(pos, galley.size()));
    }

    // The watermark is a live link to About (W12b): sense a click over the text
    // region, brighten the ink one token step on hover (the affordance, no tooltip),
    // and latch the About nav-request the shell drains on click.
    let response = ui.interact(
        region,
        egui::Id::new(WATERMARK_LINK_ID),
        egui::Sense::click(),
    );
    if response.clicked() {
        request_nav(ui.ctx(), Surface::About);
    }
    let ink = watermark_ink(response.hovered());
    for (galley, &pos) in galleys.iter().zip(positions.iter()) {
        painter.galley_with_override_text_color(pos, galley.clone(), ink);
    }
}

/// The watermark ink for the current hover state (lock W12b): the resting Win10-
/// activation ghost ([`Style::TEXT_DIM`] faded by [`WATERMARK_GHOST`]), brightened
/// one token step to the full [`Style::TEXT_DIM`] on hover — the affordance that the
/// mark is a live About link, with no tooltip (W6). Pure — unit-tested without egui.
fn watermark_ink(hovered: bool) -> egui::Color32 {
    if hovered {
        Style::TEXT_DIM
    } else {
        Style::TEXT_DIM.gamma_multiply(WATERMARK_GHOST)
    }
}

/// Latch a nav-request into egui memory for the shell to drain (lock W12b). The
/// backdrop layer owns no surface state, so the watermark link stows its target
/// here — the same memory seam the wallpaper cache rides — rather than reaching the
/// shell nav directly.
fn request_nav(ctx: &egui::Context, surface: Surface) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(NAV_REQUEST_KEY), surface));
}

/// Take the backdrop's pending nav-request (the W12b watermark→About link), if any —
/// a one-shot drain mirroring `chooser::ChooserState::take_connect`: the shell calls
/// this each frame and routes the returned [`Surface`] through its own nav, and a
/// second call in the same frame yields `None`. Returns `None` when nothing was
/// clicked.
#[allow(
    dead_code,
    reason = "the shell drains this each frame to route the W12b watermark→About \
              click; wiring it is the one-line main.rs nav hookup, held out of this \
              unit to avoid the concurrent Surface edit main.rs is carrying"
)]
pub(crate) fn take_nav_request(ctx: &egui::Context) -> Option<Surface> {
    let id = egui::Id::new(NAV_REQUEST_KEY);
    ctx.data_mut(|d| {
        let taken = d.get_temp::<Surface>(id);
        if taken.is_some() {
            d.remove::<Surface>(id);
        }
        taken
    })
}

// ─────────────────────────── the resolved-texture cache ───────────────────────────

/// The resolved wallpaper cached in egui memory: the selected [`Wallpaper`], its
/// decoded texture (`None` when even the embedded default failed to decode — the bare
/// Carbon field then stands), and when the selection was last read off disk.
#[derive(Clone)]
struct WallpaperCache {
    /// The selection this texture was decoded for.
    choice: Wallpaper,
    /// The decoded, uploaded texture, or `None` on a total decode failure (§7).
    texture: Option<TextureHandle>,
    /// When the selection was last re-read off disk — the [`PREF_REFRESH`] clock.
    checked_at: Instant,
}

/// The decoded wallpaper texture for the current selection, cached in egui memory so
/// the selection read + decode/upload happen off the [`PREF_REFRESH`] throttle, never
/// per frame. `None` (never a panic) when even the embedded default can't decode, so
/// the caller fails soft to the bare Carbon field (§7).
fn wallpaper_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let id = egui::Id::new(WALLPAPER_CACHE_KEY);
    let cached = ctx.data_mut(|d| d.get_temp::<WallpaperCache>(id));
    let now = Instant::now();

    // Within the refresh window the cached selection stands, so neither the disk read
    // nor a re-decode runs — the fast per-frame path returns the cached texture (even
    // a cached `None`, so a total decode miss isn't retried every frame).
    if let Some(c) = &cached {
        if c.checked_at.elapsed() < PREF_REFRESH {
            return c.texture.clone();
        }
    }

    // The throttle elapsed (or the first paint): re-read the selection. Reuse the
    // uploaded texture when the choice is unchanged; re-decode only on a real change.
    // The decode/upload happens OUTSIDE the egui data lock — `load_texture`
    // read-locks the context that `data_mut` write-locks (the known `parking_lot`
    // trap; cf. `chooser::ThumbnailCache`), so we resolve first, then cache.
    let choice = resolve_choice();
    let texture = match &cached {
        Some(c) if c.choice == choice => c.texture.clone(),
        _ => decode_wallpaper(ctx, choice),
    };
    ctx.data_mut(|d| {
        d.insert_temp(
            id,
            WallpaperCache {
                choice,
                texture: texture.clone(),
                checked_at: now,
            },
        );
    });
    texture
}

/// Decode + upload the wallpaper for `choice`: the installed asset under
/// [`WALLPAPER_DIR`] first, then the embedded [`DEFAULT_WALLPAPER`] as the only
/// bundled fallback, else `None` (the bare Carbon field). Reuses
/// [`crate::chooser::decode_png_rgba`] (the RGB/RGBA `png` path the splash decodes
/// its artwork on). Linear sampling — the wallpaper is scaled to fill, which reads
/// crisper than nearest.
fn decode_wallpaper(ctx: &egui::Context, choice: Wallpaper) -> Option<TextureHandle> {
    let image = fs::read(choice.installed_path())
        .ok()
        .as_deref()
        .and_then(decode_png_rgba)
        .or_else(|| decode_png_rgba(DEFAULT_WALLPAPER))?;
    Some(ctx.load_texture(
        format!("qbrand11-wallpaper-{}", choice.index()),
        image,
        TextureOptions::LINEAR,
    ))
}

/// The persisted wallpaper selection, or the default when nothing is stored / the
/// workgroup volume isn't provisioned (honest offline → the default backdrop).
fn resolve_choice() -> Wallpaper {
    WallpaperStore::open_default()
        .load()
        .unwrap_or(Wallpaper::DEFAULT)
}

/// Select `choice` as the desktop wallpaper: persist it (a no-op when the workgroup
/// volume isn't provisioned) and update the live egui-memory cache immediately so the
/// desktop reflects it this session even offline. The System surface's picker drives
/// this.
pub(crate) fn select_wallpaper(ctx: &egui::Context, choice: Wallpaper) {
    // Persist — inert when the workgroup root isn't provisioned (honest offline); the
    // cache update below still makes the choice hold session-local.
    let _ = WallpaperStore::open_default().save(choice, unix_millis());
    let texture = decode_wallpaper(ctx, choice);
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::new(WALLPAPER_CACHE_KEY),
            WallpaperCache {
                choice,
                texture,
                checked_at: Instant::now(),
            },
        );
    });
    ctx.request_repaint();
}

/// The currently-selected wallpaper — the live cache when the desktop has painted,
/// else the persisted selection. The System surface's picker shows this as current.
pub(crate) fn selected_wallpaper(ctx: &egui::Context) -> Wallpaper {
    ctx.data_mut(|d| d.get_temp::<WallpaperCache>(egui::Id::new(WALLPAPER_CACHE_KEY)))
        .map_or_else(resolve_choice, |c| c.choice)
}

// ─────────────────────────── the per-seat persisted store ───────────────────────────

/// One seat's persisted wallpaper selection — the JSON record written to its own file
/// under the workgroup root.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WallpaperRecord {
    /// The seat that wrote this record (the single writer of its own file).
    #[serde(default)]
    seat: String,
    /// The selected wallpaper's 1-based index (1–5).
    choice: u8,
    /// Wall-clock epoch millis of the write — the newest across seats wins the fold.
    #[serde(default)]
    updated_ms: u64,
}

/// The mesh-synced wallpaper store — one JSON file per seat under the workgroup root,
/// the same single-writer-per-file idiom `chooser/chooser_prefs.rs` (CHOOSER-9) and
/// the mesh peer records use, so the selection roams between seats with no new
/// transport. Reads fold every seat file and take the newest selection.
struct WallpaperStore {
    /// The Syncthing-replicated workgroup root the prefs file lives under.
    root: PathBuf,
}

impl WallpaperStore {
    /// A store rooted at `root` (tests point this at a tempdir).
    const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// A store over the canonical workgroup root
    /// ([`mackes_mesh_types::peers::default_workgroup_root`] — never a hardcoded
    /// `/mnt/mesh-storage`, the same seam CHOOSER-9 reuses).
    fn open_default() -> Self {
        Self::new(default_workgroup_root())
    }

    /// Whether the workgroup root is actually present. The store writes only under an
    /// existing root — never creating a bare unprovisioned mount — so a seat with no
    /// mesh volume is a silent no-op rather than a fabricated synced record.
    fn is_ready(&self) -> bool {
        self.root.is_dir()
    }

    /// The `<root>/wallpaper-prefs/<identity>/` directory.
    fn identity_dir(&self) -> PathBuf {
        self.root
            .join(PREFS_SUBDIR)
            .join(sanitize(&resolve_identity()))
    }

    /// The folded selection across every seat file: the record with the newest
    /// `updated_ms` wins (id tiebreak by later iteration). Malformed / half-written /
    /// temp files are skipped (never fatal), and a missing directory yields `None`.
    fn load(&self) -> Option<Wallpaper> {
        let entries = fs::read_dir(self.identity_dir()).ok()?;
        let mut best: Option<(u64, u8)> = None;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue; // an in-flight atomic-write temp file
            }
            let Ok(data) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(rec) = serde_json::from_str::<WallpaperRecord>(&data) else {
                continue;
            };
            if best.is_none_or(|(ms, _)| rec.updated_ms >= ms) {
                best = Some((rec.updated_ms, rec.choice));
            }
        }
        best.and_then(|(_, choice)| Wallpaper::from_index(choice))
    }

    /// Write this seat's selection (atomic temp + rename). A silent `Ok(())` no-op
    /// when the root is not provisioned ([`is_ready`](Self::is_ready)).
    ///
    /// # Errors
    /// The [`io::Error`] if the directory cannot be created or the file cannot be
    /// written / renamed.
    fn save(&self, choice: Wallpaper, now_ms: u64) -> io::Result<()> {
        if !self.is_ready() {
            return Ok(());
        }
        let dir = self.identity_dir();
        fs::create_dir_all(&dir)?;
        let seat = resolve_seat();
        let safe_seat = sanitize(&seat);
        let final_path = dir.join(format!("{safe_seat}.json"));
        let tmp_path = dir.join(format!(".{safe_seat}.json.tmp"));
        let rec = WallpaperRecord {
            seat,
            choice: choice.index(),
            updated_ms: now_ms,
        };
        let json = serde_json::to_string_pretty(&rec)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }
}

// ─── environment resolution (mirrors chooser_prefs; that module is private) ───

/// Reduce an identity / seat id to a safe single path component
/// (`[A-Za-z0-9_-]`, everything else → `_`; never empty).
fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Resolve the mesh identity the selection roams under: `$MDE_MESH_USER` → `$USER` /
/// `$LOGNAME` → a stable `operator` fallback (the same precedence CHOOSER-9 uses, so
/// every identity-bound record agrees).
fn resolve_identity() -> String {
    for key in ["MDE_MESH_USER", "USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    "operator".to_owned()
}

/// Resolve this seat's id: `$MDE_MESH_SEAT` → `$HOSTNAME` → `/etc/hostname` → a
/// stable `seat` fallback (the per-file writer, like a peer record's hostname).
fn resolve_seat() -> String {
    for key in ["MDE_MESH_SEAT", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    if let Ok(host) = fs::read_to_string("/etc/hostname") {
        let host = host.trim();
        if !host.is_empty() {
            return host.to_owned();
        }
    }
    "seat".to_owned()
}

/// The three watermark lines, resolved once per process ([`OnceLock`]): the
/// hostname is boot-stable and the role pin is install-time (upgrade-only in
/// `mde-role`), so a per-frame re-read would be pure disk churn — a re-pin
/// surfaces on the next shell start.
fn watermark() -> &'static [String; 3] {
    static LINES: OnceLock<[String; 3]> = OnceLock::new();
    LINES
        .get_or_init(|| watermark_lines(&crate::discovery::local_peer(), resolve_role().as_deref()))
}

/// Fold the watermark's three ghost lines (lock W12): the product mark, the visible
/// release line ([`brand::logo::PRODUCT_RELEASE`]), and the node identity —
/// `<host> · <role>`, or the bare hostname when no role is pinned (honest
/// omission, §7).
fn watermark_lines(host: &str, role: Option<&str>) -> [String; 3] {
    let node = role.map_or_else(|| host.to_owned(), |role| format!("{host} · {role}"));
    [
        WATERMARK_PRODUCT.to_owned(),
        brand::logo::PRODUCT_RELEASE.to_owned(),
        node,
    ]
}

/// The pinned deployment role for the watermark's node line, or `None` — the role
/// honestly omitted — when [`ROLE_PATH`] is absent/unreadable or names nothing
/// (§7; the watermark never guesses a tier the way `mde-role` callers never
/// default one).
fn resolve_role() -> Option<String> {
    let path =
        std::env::var_os("MDE_ROLE_PATH").map_or_else(|| PathBuf::from(ROLE_PATH), PathBuf::from);
    role_from_toml(&fs::read_to_string(path).ok()?)
}

/// Extract the pinned role token from a `role.toml` body: the unquoted value of
/// the first `role = <value>` line, `#` comments and blank lines skipped —
/// mirrors `mde-role`'s tolerant line parse, minus the enum gate (the watermark
/// shows the pinned token verbatim; validity is the role gate's business).
fn role_from_toml(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(|line| {
            let value = line.strip_prefix("role")?.trim_start().strip_prefix('=')?;
            let value = value.trim().trim_matches('"').trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
}

/// Wall-clock epoch millis — the record timestamp the save stamps writes with.
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "exact uv/layout arithmetic on exact inputs"
)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2};

    /// Render one headless 960×640 frame of the backdrop and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives,
    /// minus the GPU. Returns whether it drew primitives and whether a wallpaper
    /// texture was cached.
    fn run(coverage: Coverage, status: Option<(&str, &str)>) -> (bool, bool) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, coverage, status));
        });
        let cached = ctx.data_mut(|d| {
            d.get_temp::<WallpaperCache>(egui::Id::new(WALLPAPER_CACHE_KEY))
                .and_then(|c| c.texture)
                .is_some()
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        (!prims.is_empty(), cached)
    }

    fn temp_root(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mde-qbrand11-{tag}-{n}"))
    }

    #[test]
    fn the_embedded_default_wallpaper_decodes_to_its_native_size() {
        // Proves the embedded default (the bundled fallback) is a real, correctly
        // shaped image — not a stray/missing/mis-encoded file.
        let img = decode_png_rgba(DEFAULT_WALLPAPER).expect("the embedded default decodes");
        assert_eq!(img.size, [1408, 768], "native Construct wallpaper size");
    }

    #[test]
    fn an_empty_desktop_paints_the_shell_color_without_wallpaper_cache() {
        // The empty path: the shell field fills + a status block below. No
        // wallpaper image is decoded or cached for the background.
        let (drew, cached) = run(
            Coverage::Empty,
            Some((
                "No desktop connected",
                "Broker a VM desktop — it renders here.",
            )),
        );
        assert!(
            drew,
            "the empty shell-colour backdrop produced no draw primitives"
        );
        assert!(
            !cached,
            "the shell-colour background must not cache a wallpaper texture"
        );
    }

    #[test]
    fn a_covered_desktop_still_paints_shell_color_under_the_scrim() {
        // The covered path: the shell field still fills behind whatever content
        // covers the display, with no status block or wallpaper cache.
        let (drew, cached) = run(Coverage::Covered, None);
        assert!(
            drew,
            "the covered shell-colour backdrop produced no draw primitives"
        );
        assert!(
            !cached,
            "the covered shell-colour backdrop must not cache a wallpaper texture"
        );
    }

    #[test]
    fn centered_status_places_the_empty_desktop_copy_in_the_workspace_center() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let status = Some((
            "No desktops discovered",
            "No mesh peer, LAN endpoint, or local VM is advertising a desktop.",
        ));

        let low = ctx.run(input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, Coverage::Empty, status));
        });
        let centered = ctx.run(input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_centered_status(ui, Coverage::Empty, status);
            });
        });
        let title_y = |out: &egui::FullOutput| {
            frame_text(&out.shapes)
                .into_iter()
                .find_map(|(pos, text)| (text == "No desktops discovered").then_some(pos.y))
                .expect("status title should paint")
        };

        let low_y = title_y(&low);
        let centered_y = title_y(&centered);
        assert!(
            centered_y > 250.0 && centered_y < 360.0,
            "centered empty status should sit near the workspace middle: {centered_y}"
        );
        assert!(
            centered_y + 100.0 < low_y,
            "centered status should move up from the old low anchor: centered={centered_y}, low={low_y}"
        );
    }

    #[test]
    fn the_wallpaper_texture_is_decoded_once_and_reused() {
        // Two resolves on the same context hand back the SAME uploaded texture — the
        // decode+upload is not repeated per call.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let first = wallpaper_texture(&ctx).expect("decodes once").id();
        let second = wallpaper_texture(&ctx).expect("served from cache").id();
        assert_eq!(
            first, second,
            "the wallpaper must be uploaded once and reused"
        );
    }

    #[test]
    fn the_default_wallpaper_is_number_four() {
        // Placement lock #12: WALLPAPER4 is the default.
        assert_eq!(Wallpaper::DEFAULT, Wallpaper::Four);
        assert_eq!(Wallpaper::DEFAULT.index(), 4);
    }

    #[test]
    fn the_wallpaper_index_round_trips_and_rejects_out_of_range() {
        for w in Wallpaper::ALL {
            assert_eq!(
                Wallpaper::from_index(w.index()),
                Some(w),
                "{w:?} round-trips"
            );
        }
        assert!(Wallpaper::from_index(0).is_none(), "0 is out of range");
        assert!(Wallpaper::from_index(6).is_none(), "6 is out of range");
    }

    #[test]
    fn cover_uv_centre_crops_the_overflowing_axis() {
        // A wide (2:1) image into a square target crops horizontally to a centred
        // half-width; the vertical axis stays full.
        let uv = cover_uv(vec2(100.0, 100.0), vec2(200.0, 100.0));
        assert!((uv.min.x - 0.25).abs() < 1e-4, "left crop: {uv:?}");
        assert!((uv.max.x - 0.75).abs() < 1e-4, "right crop: {uv:?}");
        assert_eq!(uv.min.y, 0.0);
        assert_eq!(uv.max.y, 1.0);

        // A tall (1:2) image into a square target crops vertically instead.
        let uv = cover_uv(vec2(100.0, 100.0), vec2(100.0, 200.0));
        assert!((uv.min.y - 0.25).abs() < 1e-4, "top crop: {uv:?}");
        assert_eq!(uv.min.x, 0.0);
        assert_eq!(uv.max.x, 1.0);

        // A degenerate zero dimension maps to the full texture (no divide-by-zero).
        let uv = cover_uv(vec2(0.0, 100.0), vec2(200.0, 100.0));
        assert_eq!(uv, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
    }

    #[test]
    fn the_store_round_trips_a_selection_and_newest_write_wins() {
        let dir = temp_root("store");
        fs::create_dir_all(&dir).expect("mkroot");
        let store = WallpaperStore::new(dir.clone());
        assert!(store.is_ready());
        assert!(
            store.load().is_none(),
            "no record yet → the default resolves"
        );

        store.save(Wallpaper::Three, 1_000).expect("save three");
        assert_eq!(store.load(), Some(Wallpaper::Three));

        // The same seat re-saves; the newer write wins (its file is overwritten).
        store.save(Wallpaper::One, 2_000).expect("save one");
        assert_eq!(store.load(), Some(Wallpaper::One));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_fold_takes_the_newest_seat_and_skips_a_corrupt_file() {
        let dir = temp_root("fold");
        let store = WallpaperStore::new(dir.clone());
        let idir = store.identity_dir();
        fs::create_dir_all(&idir).expect("mkdir");
        fs::write(
            idir.join("alpha.json"),
            r#"{"seat":"alpha","choice":2,"updated_ms":10}"#,
        )
        .expect("write alpha");
        fs::write(
            idir.join("beta.json"),
            r#"{"seat":"beta","choice":5,"updated_ms":30}"#,
        )
        .expect("write beta");
        fs::write(
            idir.join("gamma.json"),
            r#"{"seat":"gamma","choice":1,"updated_ms":20}"#,
        )
        .expect("write gamma");
        assert_eq!(
            store.load(),
            Some(Wallpaper::Five),
            "the newest updated_ms wins"
        );

        // A corrupt file is skipped, never fatal.
        fs::write(idir.join("bad.json"), "{ not json").expect("write bad");
        assert_eq!(store.load(), Some(Wallpaper::Five), "corrupt skipped");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unprovisioned_store_is_inert() {
        let store = WallpaperStore::new(PathBuf::from("/no/such/mesh/root"));
        assert!(!store.is_ready());
        store.save(Wallpaper::Two, 1).expect("inert save is Ok");
        assert!(store.load().is_none(), "nothing folded from a missing root");
    }

    #[test]
    fn selecting_a_wallpaper_updates_the_live_cache_immediately() {
        // Even when the workgroup volume is unprovisioned (the save is inert), the
        // live cache holds the new choice this session (honest offline).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        select_wallpaper(&ctx, Wallpaper::Two);
        assert_eq!(selected_wallpaper(&ctx), Wallpaper::Two);
        select_wallpaper(&ctx, Wallpaper::Five);
        assert_eq!(selected_wallpaper(&ctx), Wallpaper::Five);
    }

    // ──────────────────── NAVBAR-W10-3 — the brand watermark ────────────────────

    /// Every glyph run in a frame's shape list, with its paint position — the
    /// headless proof the watermark text actually reaches the paint list
    /// (`Shape::Vec` recursed; the same shapes [`run`] tessellates).
    fn frame_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Pos2, String)> {
        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(egui::Pos2, String)>) {
            match shape {
                egui::epaint::Shape::Text(t) => out.push((t.pos, t.galley.text().to_owned())),
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn the_watermark_folds_product_version_and_node() {
        let [product, version, node] = watermark_lines("eagle", Some("workstation"));
        assert_eq!(product, WATERMARK_PRODUCT);
        assert_eq!(
            version,
            brand::logo::PRODUCT_RELEASE.to_owned(),
            "the visible release line is the brand constant, never a re-derived string"
        );
        assert_eq!(node, "eagle · workstation");
    }

    #[test]
    fn the_watermark_omits_an_absent_role() {
        // No pinned role → the bare hostname, never a guessed tier (§7).
        let [_, _, node] = watermark_lines("eagle", None);
        assert_eq!(node, "eagle");
    }

    #[test]
    fn the_role_token_parses_from_a_role_toml_body() {
        assert_eq!(
            role_from_toml("# pinned at install\nrole = \"workstation\"\nmedia = true\n"),
            Some("workstation".to_owned())
        );
        assert_eq!(
            role_from_toml("role=lighthouse"),
            Some("lighthouse".to_owned()),
            "unquoted / unspaced values parse"
        );
        assert_eq!(
            role_from_toml("# role = \"workstation\""),
            None,
            "a commented-out line never pins a role"
        );
        assert_eq!(
            role_from_toml("role = \"\""),
            None,
            "an empty value is no role"
        );
        assert_eq!(role_from_toml(""), None, "an empty body is no role");
    }

    #[test]
    fn the_backdrop_paints_the_watermark_bottom_right_above_the_bar() {
        // A headless 960×640 frame with no status block: the only text on the
        // backdrop is the watermark itself. All three live lines must reach the
        // paint list, anchored in the bottom-right quadrant and wholly above where
        // the taskbar mounts — and the frame must still tessellate non-empty.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, Coverage::Empty, None));
        });

        let texts = frame_text(&out.shapes);
        for line in watermark() {
            let hits: Vec<_> = texts.iter().filter(|(_, t)| t == line).collect();
            assert!(!hits.is_empty(), "watermark line {line:?} was not painted");
            for (pos, _) in hits {
                assert!(
                    pos.x > 480.0 && pos.y > 320.0,
                    "line {line:?} must anchor bottom-right, painted at {pos:?}"
                );
                assert!(
                    pos.y < 640.0 - crate::dock::TASKBAR_H,
                    "line {line:?} must sit above the taskbar, painted at {pos:?}"
                );
            }
        }

        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the watermark backdrop produced no draw primitives"
        );
    }

    // ─────────────── NAVBAR-W10-6 — the watermark→About link (W12b) ───────────────

    /// A primary-button press/release event at `pos` (the egui click model: press one
    /// frame, release the next), mirroring the tray/dock click tests.
    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// Every watermark line's painted **override** colour this frame — the ink the
    /// hover state resolved to (`galley_with_override_text_color` stamps it onto the
    /// `TextShape`). Keyed by the line text so a test can assert per line.
    fn watermark_inks(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    if let Some(c) = t.override_text_color {
                        out.push((t.galley.text().to_owned(), c));
                    }
                }
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Run one headless 960×640 backdrop frame (empty desktop, no status) feeding
    /// `events`, returning the frame output for shape inspection.
    fn watermark_frame(ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, Coverage::Empty, None));
        })
    }

    #[test]
    fn watermark_ink_brightens_one_token_step_on_hover() {
        // Lock W12b — the resting ink is the ghosted TEXT_DIM (the Win10-activation
        // register); hover firms it up one token step to the full TEXT_DIM, the
        // affordance that the mark is a live link (still no tooltip, W6).
        let rest = watermark_ink(false);
        let hot = watermark_ink(true);
        assert_eq!(rest, Style::TEXT_DIM.gamma_multiply(WATERMARK_GHOST));
        assert_eq!(hot, Style::TEXT_DIM);
        assert_ne!(rest, hot, "hover must change the ink");
        // "Brighter" = no channel drops and at least one rises (a ghost is a gamma
        // fade toward black, so un-ghosting only raises the channels).
        assert!(
            hot.r() >= rest.r()
                && hot.g() >= rest.g()
                && hot.b() >= rest.b()
                && (hot.r() > rest.r() || hot.g() > rest.g() || hot.b() > rest.b()),
            "hover ink {hot:?} must be brighter than the ghost {rest:?}"
        );
    }

    #[test]
    fn hovering_the_watermark_brightens_the_painted_ink() {
        // The painted proof: pointer off the mark → every line paints the ghost ink;
        // pointer over the mark's rect → every line paints the brightened link ink,
        // so the W12b hover affordance actually reaches the paint list.
        let ctx = egui::Context::default();
        Style::install(&ctx);

        // Settle the interact region, then read its centre back by the stable id.
        let _ = watermark_frame(&ctx, Vec::new());
        let region = ctx
            .read_response(egui::Id::new(WATERMARK_LINK_ID))
            .expect("the watermark link region is registered")
            .rect;
        let center = region.center();

        // Off the mark (pointer parked top-left): the resting ghost ink on all three.
        let cold = watermark_frame(&ctx, vec![egui::Event::PointerMoved(pos2(4.0, 4.0))]);
        let ghost = Style::TEXT_DIM.gamma_multiply(WATERMARK_GHOST);
        let cold_inks = watermark_inks(&cold.shapes);
        assert_eq!(cold_inks.len(), 3, "three watermark lines paint");
        assert!(
            cold_inks.iter().all(|(_, c)| *c == ghost),
            "un-hovered lines paint the ghost ink: {cold_inks:?}"
        );

        // Over the mark: every line brightens one token step to full TEXT_DIM.
        let hot = watermark_frame(&ctx, vec![egui::Event::PointerMoved(center)]);
        let hot_inks = watermark_inks(&hot.shapes);
        assert_eq!(hot_inks.len(), 3, "three watermark lines paint");
        assert!(
            hot_inks.iter().all(|(_, c)| *c == Style::TEXT_DIM),
            "hovered lines brighten to TEXT_DIM: {hot_inks:?}"
        );
    }

    #[test]
    fn clicking_the_watermark_requests_the_about_surface() {
        // Lock W12b — a click anywhere on the three-line block latches a
        // Surface::About nav-request the shell drains; the drain is one-shot (a second
        // take yields None), and nothing is pending before a click.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        assert_eq!(take_nav_request(&ctx), None, "no request before a click");

        // Settle the region, read its centre, then click it (press one frame, release
        // the next — the egui click model the tray/dock tests use).
        let _ = watermark_frame(&ctx, Vec::new());
        let center = ctx
            .read_response(egui::Id::new(WATERMARK_LINK_ID))
            .expect("the watermark link region is registered")
            .rect
            .center();
        let _ = watermark_frame(
            &ctx,
            vec![egui::Event::PointerMoved(center), press(center, true)],
        );
        let _ = watermark_frame(&ctx, vec![press(center, false)]);

        assert_eq!(
            take_nav_request(&ctx),
            Some(Surface::About),
            "clicking the watermark routes to About"
        );
        assert_eq!(
            take_nav_request(&ctx),
            None,
            "the nav-request drain is one-shot"
        );
    }
}

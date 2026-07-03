//! QBRAND-11 — the shell's **desktop wallpaper backdrop**: the official Quazar
//! artwork painted as the bottom-most desktop layer (`docs/design/quasar-branding.md`,
//! placement lock #12 — `MDE-QUAZAR-WALLPAPER4.png` is the default, all five ship
//! selectable).
//!
//! Under E12 "Quasar" the shell IS the desktop, so "the empty desktop" is the
//! Desktop surface with nothing brokered in: the [`crate::vdi`] no-desktop state and
//! the [`crate::discovery`]/[`crate::chooser`] empty root desktop. Both paint the
//! wallpaper through this one helper, with any honest status relocated to a small
//! block low on the field (§7 honesty preserved).
//!
//! The wallpaper resolves at runtime from the installed set QBRAND-9 drops under
//! [`WALLPAPER_DIR`], decoded ONCE to a cached texture (the decode + upload is shared
//! by every empty path, never re-run per frame) and cover-filled to the display —
//! aspect preserved, the overflow axis cropped via UV, never stretched. The selected
//! wallpaper persists per seat and follows the mesh identity through the same
//! `chooser/chooser_prefs.rs` (CHOOSER-9) idiom. Only the default `WALLPAPER4` is
//! embedded (`include_bytes!`) as a bundled fallback — embedding all five would add
//! ~6 MB to the binary, so the rest load from disk (disk-honest, §4). All colour the
//! shell *adds* — the Carbon field beneath, the cover scrim, the status backing — is
//! a `mde-theme`/`Style` token, never a raw hex (§4).

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, …); the shell's surfaces consume them"
)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{self, FontId, Rect, TextureHandle, TextureOptions, Vec2};
use mde_egui::{Motion, Style};

use serde::{Deserialize, Serialize};

use mackes_mesh_types::peers::default_workgroup_root;

use crate::chooser::decode_png_rgba;

/// The installed wallpaper directory — QBRAND-9 drops all five official wallpapers
/// here in the RPM. The selected one is loaded from disk at runtime.
const WALLPAPER_DIR: &str = "/usr/share/backgrounds/magic-mesh";

/// The default wallpaper, embedded so a fresh / dev shell (or a host missing the
/// installed set) always has a backdrop with no filesystem / RPM-path dependency.
/// Only `WALLPAPER4` is carried — embedding all five would add ~6 MB to the binary,
/// so the rest load from [`WALLPAPER_DIR`] (disk-honest, §4).
const DEFAULT_WALLPAPER: &[u8] =
    include_bytes!("../../../../assets/brand/MDE-QUAZAR-WALLPAPER4.png");

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

/// One of the five official Quazar wallpapers (placement lock #12). `Four` is the
/// default; all five ship in the RPM as a selectable set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wallpaper {
    /// `MDE-QUAZAR-WALLPAPER1.png`.
    One,
    /// `MDE-QUAZAR-WALLPAPER2.png`.
    Two,
    /// `MDE-QUAZAR-WALLPAPER3.png`.
    Three,
    /// `MDE-QUAZAR-WALLPAPER4.png` — the default desktop wallpaper (lock #12).
    Four,
    /// `MDE-QUAZAR-WALLPAPER5.png`.
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
            Self::One => "MDE-QUAZAR-WALLPAPER1.png",
            Self::Two => "MDE-QUAZAR-WALLPAPER2.png",
            Self::Three => "MDE-QUAZAR-WALLPAPER3.png",
            Self::Four => "MDE-QUAZAR-WALLPAPER4.png",
            Self::Five => "MDE-QUAZAR-WALLPAPER5.png",
        }
    }

    /// The installed absolute path this wallpaper loads from at runtime.
    fn installed_path(self) -> PathBuf {
        Path::new(WALLPAPER_DIR).join(self.filename())
    }
}

/// Paint the desktop wallpaper as the bottom-most layer of the current panel: the
/// solid Carbon §4 field (the ultimate fallback), the selected wallpaper cover-filled
/// over it, an eased cover scrim when a surface/window is open, and — when `status` is
/// given — an honest status block placed low on the field.
///
/// Call this FIRST in the panel body: it draws through the painter and consumes no
/// layout, so the panel's other widgets lay out over it (a covered display's grid
/// floats above the scrim).
pub(crate) fn show(ui: &egui::Ui, coverage: Coverage, status: Option<(&str, &str)>) {
    let free = ui.max_rect();

    // The Carbon §4 field is painted first as the ultimate fallback: if neither the
    // selected wallpaper nor the embedded default decodes, this honest solid field
    // still stands (§7). A painter clone so `Image::paint_at` can still borrow `ui`.
    let painter = ui.painter().clone();
    painter.rect_filled(free, 0.0, Style::BG);

    // Ease the cover scrim toward the coverage target (a continuous crossfade across
    // every empty↔covered transition).
    let empty = coverage == Coverage::Empty;
    let reveal = Motion::animate(ui.ctx(), CROSSFADE_KEY, empty, Motion::SLOW);

    // The desktop wallpaper, cover-filled to the display — aspect preserved, the
    // overflow axis cropped via UV, never stretched. A missing/undecodable set falls
    // through to the embedded default, then to the bare Carbon field above (§7).
    if let Some(texture) = wallpaper_texture(ui.ctx()) {
        let uv = cover_uv(free.size(), texture.size_vec2());
        egui::Image::new(egui::load::SizedTexture::new(texture.id(), free.size()))
            .uv(uv)
            .paint_at(ui, free);
        let scrim = (1.0 - reveal) * COVERED_SCRIM;
        if scrim > f32::EPSILON {
            painter.rect_filled(free, 0.0, Style::BG.gamma_multiply(scrim));
        }
    }

    // Any honest status (the empty-desktop copy, a gated-transport note) — a small
    // block low on the field, over a subtle backing so it reads over the artwork.
    if let Some((title, detail)) = status {
        paint_status(&painter, free, title, detail);
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
fn paint_status(painter: &egui::Painter, free: Rect, title: &str, detail: &str) {
    let center_x = free.center().x;
    let top = free.height().mul_add(STATUS_Y_FRAC, free.top());
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
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives, minus
    /// the GPU. Returns whether it drew primitives and whether the wallpaper texture
    /// was cached (proving the one decode+upload happened).
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
        assert_eq!(img.size, [1672, 941], "native WALLPAPER4 size");
    }

    #[test]
    fn an_empty_desktop_paints_the_wallpaper_and_caches_it() {
        // The empty path: the wallpaper fills + a status block below, and the one
        // texture upload is cached for reuse.
        let (drew, cached) = run(
            Coverage::Empty,
            Some((
                "No desktop connected",
                "Broker a VM desktop — it renders here.",
            )),
        );
        assert!(
            drew,
            "the empty wallpaper backdrop produced no draw primitives"
        );
        assert!(
            cached,
            "the wallpaper texture was not cached after the first paint"
        );
    }

    #[test]
    fn a_covered_desktop_still_paints_the_wallpaper_under_the_scrim() {
        // The covered path: the wallpaper still fills (dimmed under the scrim) behind
        // whatever content covers the display, with no status block.
        let (drew, cached) = run(Coverage::Covered, None);
        assert!(
            drew,
            "the covered wallpaper backdrop produced no draw primitives"
        );
        assert!(
            cached,
            "the wallpaper texture was not cached on the covered path"
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
}

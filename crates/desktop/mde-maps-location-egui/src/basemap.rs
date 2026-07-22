//! Offline raster basemap — `MBTiles` → egui texture slippy-tile layer (P2).
//!
//! The seat ships a region bundle under
//! `<client_data_dir>/maps/<region>/<region>.mbtiles` (see
//! [`mde_bus::client_data_dir`]); this module reads it and paints a real
//! Web-Mercator tile layer under the Drive/Map HUD, replacing the old
//! procedural grid + hard-coded road splines.
//!
//! Design constraints (from `docs/design/maps-worldclass-plan.md`):
//!   * **Raster, not vector** — a textured quad works on both the DRM seat's
//!     `egui_glow` (GLES) backend and the windowed `wgpu` backend. A wgpu
//!     paint-callback vector map would silently break the in-vehicle seat.
//!   * **`MBTiles` is TMS row order** — the on-disk `tile_row` counts from the
//!     south, so `y_tms = (1 << z) - 1 - y_xyz`.
//!   * **Fail-soft** — a seat with no region installed shows an honest
//!     "No offline map data" panel and never panics.
//!
//! Textures are memoized in `ctx` temp memory keyed by `(region, z, x, y)`,
//! mirroring the [`mde_egui::carbon`] texture-cache pattern: a tile decodes and
//! uploads once, then every later frame reuses the handle. Misses are cached
//! too, so an absent tile is not re-queried each frame.

use std::path::{Path, PathBuf};

use mde_egui::egui::{
    self, Color32, Context, FontId, Painter, Pos2, Rect, TextureHandle, TextureOptions,
};
use mde_egui::Style;
use rusqlite::{Connection, OpenFlags};

use crate::model::MapViewState;

/// Web-Mercator latitude clamp — the projection is undefined beyond the poles,
/// so every conversion clamps to the standard slippy-map limit.
const MERCATOR_LAT_LIMIT: f64 = 85.051_128_78;

/// Native tile edge in pixels (OSM/`MBTiles` standard). One tile uploads as a
/// 256×256 texture and paints into a logical rect scaled by the fractional zoom.
const TILE_SIZE: f64 = 256.0;

/// Hard cap on the number of tiles drawn in a single frame — a guard against a
/// pathological viewport/zoom asking for thousands of tiles. The visible window
/// is normally a few dozen; this only trips on degenerate input.
const MAX_TILES_PER_FRAME: usize = 256;

/// Fractional Web-Mercator tile coordinate `(x, y)` for `lon`/`lat` at zoom `z`.
///
/// `x` runs west→east in `[0, 2^z)`, `y` runs north→south in `[0, 2^z)`. The
/// integer floor of each is the XYZ tile index; the fraction locates a point
/// inside the tile. Latitude is clamped to the Mercator limit so the transform
/// stays finite.
#[must_use]
pub fn tile_frac(lon: f64, lat: f64, z: u8) -> (f64, f64) {
    let n = f64::from(1u32 << z);
    let lat = lat.clamp(-MERCATOR_LAT_LIMIT, MERCATOR_LAT_LIMIT);
    let lat_rad = lat.to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

/// Region metadata parsed from the `MBTiles` `metadata` table.
#[derive(Debug, Clone, PartialEq)]
pub struct RawMeta {
    /// Map centre (from the `center` row, or the bounds midpoint).
    pub center_lat: f64,
    /// Map centre longitude.
    pub center_lon: f64,
    /// Lowest zoom level present.
    pub min_zoom: u8,
    /// Highest zoom level present.
    pub max_zoom: u8,
    /// Coverage bounds (WGS84).
    pub min_lon: f64,
    /// Coverage bounds.
    pub min_lat: f64,
    /// Coverage bounds.
    pub max_lon: f64,
    /// Coverage bounds.
    pub max_lat: f64,
}

impl RawMeta {
    /// Whether `lat`/`lon` falls inside the region's coverage bounds.
    #[must_use]
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        lon >= self.min_lon && lon <= self.max_lon && lat >= self.min_lat && lat <= self.max_lat
    }
}

/// Cached, cheap-to-clone description of the installed offline basemap region.
#[derive(Debug, Clone)]
pub struct BasemapMeta {
    /// Region directory name (e.g. `east-texas`) — the texture-cache key prefix.
    pub region: String,
    /// Absolute path to the region's `.mbtiles` file.
    pub mbtiles: PathBuf,
    /// Parsed region metadata.
    pub raw: RawMeta,
}

/// The maps root the seat reads region bundles from: `MDE_MAPS_DIR` when set (a
/// test/override hook), else `<client_data_dir>/maps`.
fn maps_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MDE_MAPS_DIR") {
        return Some(PathBuf::from(dir));
    }
    mde_bus::client_data_dir().map(|d| d.join("maps"))
}

/// The first `*.mbtiles` file directly inside `dir`, if any.
fn mbtiles_in(dir: &Path) -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mbtiles"))
        })
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// Resolve the installed region directory.
///
/// The first sub-directory of the maps root that carries a `.mbtiles` or a
/// `gazetteer.sqlite`. Returns `None` when no data is installed (the honest
/// offline fallback).
#[must_use]
pub fn region_dir() -> Option<PathBuf> {
    let root = maps_root()?;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .find(|d| mbtiles_in(d).is_some() || d.join("gazetteer.sqlite").exists())
}

/// Open an `MBTiles`/`SQLite` file read-only. Fail-soft: `None` when it cannot open.
fn open_ro(path: &Path) -> Option<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Parse the `metadata` table of an `MBTiles` file. `None` when the file is
/// missing/unreadable or lacks the fields needed to place tiles.
#[must_use]
pub fn read_meta(mbtiles: &Path) -> Option<RawMeta> {
    let conn = open_ro(mbtiles)?;
    let mut stmt = conn.prepare("SELECT name, value FROM metadata").ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()?;

    let mut min_zoom: Option<u8> = None;
    let mut max_zoom: Option<u8> = None;
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    let mut center: Option<(f64, f64)> = None;
    for row in rows.flatten() {
        let (name, value) = row;
        match name.as_str() {
            "minzoom" => min_zoom = value.trim().parse().ok(),
            "maxzoom" => max_zoom = value.trim().parse().ok(),
            "bounds" => bounds = parse_bounds(&value),
            "center" => center = parse_center(&value),
            _ => {}
        }
    }

    let (min_lon, min_lat, max_lon, max_lat) = bounds?;
    let (center_lon, center_lat) = center.unwrap_or_else(|| {
        (
            f64::midpoint(min_lon, max_lon),
            f64::midpoint(min_lat, max_lat),
        )
    });
    Some(RawMeta {
        center_lat,
        center_lon,
        min_zoom: min_zoom.unwrap_or(0),
        max_zoom: max_zoom.unwrap_or(19),
        min_lon,
        min_lat,
        max_lon,
        max_lat,
    })
}

/// Parse an `MBTiles` `bounds` value `minlon,minlat,maxlon,maxlat`.
fn parse_bounds(value: &str) -> Option<(f64, f64, f64, f64)> {
    let parts: Vec<f64> = value
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    match parts.as_slice() {
        [a, b, c, d] => Some((*a, *b, *c, *d)),
        _ => None,
    }
}

/// Parse an `MBTiles` `center` value `lon,lat[,zoom]` → `(lon, lat)`.
fn parse_center(value: &str) -> Option<(f64, f64)> {
    let parts: Vec<f64> = value
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    match parts.as_slice() {
        [lon, lat] | [lon, lat, _] => Some((*lon, *lat)),
        _ => None,
    }
}

/// Read one XYZ tile's PNG bytes from an already-open `MBTiles` connection,
/// converting the XYZ `y` to the on-disk TMS row. `None` when the tile is absent.
#[must_use]
pub fn read_tile_conn(conn: &Connection, z: u8, x: u32, y_xyz: u32) -> Option<Vec<u8>> {
    let y_tms = ((1u32 << z) - 1).checked_sub(y_xyz)?;
    conn.query_row(
        "SELECT tile_data FROM tiles \
         WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
        rusqlite::params![z, x, y_tms],
        |row| row.get::<_, Vec<u8>>(0),
    )
    .ok()
}

/// Convenience reader that opens the file itself (used by the tests + one-shot
/// callers). Production paint opens the connection once per frame instead.
#[must_use]
pub fn read_tile(mbtiles: &Path, z: u8, x: u32, y_xyz: u32) -> Option<Vec<u8>> {
    let conn = open_ro(mbtiles)?;
    read_tile_conn(&conn, z, x, y_xyz)
}

/// Decode PNG tile bytes into an egui image. `None` on undecodable bytes.
fn decode_tile(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// Resolve + cache the installed region metadata in `ctx` temp memory.
///
/// Absence is *not* cached (re-discovered each frame) so a region installed
/// mid-session appears without a restart; a resolved region is cached so the
/// filesystem + metadata parse happen once.
#[must_use]
pub fn cached_meta(ctx: &Context) -> Option<BasemapMeta> {
    let key = egui::Id::new("mde-maps-basemap-meta");
    if let Some(meta) = ctx.data_mut(|d| d.get_temp::<BasemapMeta>(key)) {
        return Some(meta);
    }
    let dir = region_dir()?;
    let mbtiles = mbtiles_in(&dir)?;
    let raw = read_meta(&mbtiles)?;
    let region = dir.file_name().map_or_else(
        || "region".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let meta = BasemapMeta {
        region,
        mbtiles,
        raw,
    };
    ctx.data_mut(|d| d.insert_temp(key, meta.clone()));
    Some(meta)
}

/// A live Web-Mercator screen projection for one paint pass.
///
/// It turns any `lat`/`lon` into a screen point consistent with the tiles
/// painted this frame, so pins + straight-line previews land on the real map.
#[derive(Debug, Clone, Copy)]
pub struct Projection {
    origin: Pos2,
    cfx: f64,
    cfy: f64,
    tile_px: f64,
    tile_z: u8,
    n_tiles: i64,
}

impl Projection {
    /// Build the projection for `rect` centred on `center` (`lat`, `lon`) at the
    /// view's zoom/pan. The integer tile zoom is the rounded view zoom clamped to
    /// the region's available levels; fractional zoom scales the tile size so
    /// pinch/step zoom stays smooth.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn new(rect: Rect, map: &MapViewState, center: (f64, f64), meta: &RawMeta) -> Self {
        let zoom = if map.zoom.is_finite() { map.zoom } else { 13.0 };
        let tile_z =
            (zoom.round() as i32).clamp(i32::from(meta.min_zoom), i32::from(meta.max_zoom)) as u8;
        let scale = (f64::from(zoom) - f64::from(tile_z)).exp2();
        let tile_px = (TILE_SIZE * scale).max(1.0);
        let (cfx, cfy) = tile_frac(center.1, center.0, tile_z);
        let pan_x = clamp_pan(map.pan[0]);
        let pan_y = clamp_pan(map.pan[1]);
        let origin = egui::pos2(rect.center().x + pan_x, rect.center().y + pan_y);
        Self {
            origin,
            cfx,
            cfy,
            tile_px,
            tile_z,
            n_tiles: 1i64 << tile_z,
        }
    }

    /// Project a geographic point to a screen position.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn project(&self, lat: f64, lon: f64) -> Pos2 {
        let (fx, fy) = tile_frac(lon, lat, self.tile_z);
        egui::pos2(
            self.origin.x + ((fx - self.cfx) * self.tile_px) as f32,
            self.origin.y + ((fy - self.cfy) * self.tile_px) as f32,
        )
    }

    /// Screen rect covered by XYZ tile `(x, y)`. A one-pixel bleed hides the
    /// hairline seam between neighbouring tiles under bilinear sampling.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn tile_rect(&self, x: i64, y: i64) -> Rect {
        let tl = egui::pos2(
            self.origin.x + ((x as f64 - self.cfx) * self.tile_px) as f32,
            self.origin.y + ((y as f64 - self.cfy) * self.tile_px) as f32,
        );
        Rect::from_min_size(
            tl,
            egui::vec2(self.tile_px as f32 + 1.0, self.tile_px as f32 + 1.0),
        )
    }

    /// The XYZ tiles whose rects intersect `rect`, clamped to both the world
    /// wrap `[0, 2^z)` and the region's coverage bounds (so empty out-of-region
    /// tiles are never queried). Capped at [`MAX_TILES_PER_FRAME`].
    #[allow(clippy::cast_possible_truncation, clippy::similar_names)]
    fn visible_tiles(&self, rect: Rect, meta: &RawMeta) -> Vec<(i64, i64)> {
        let fx_at = |sx: f32| self.cfx + f64::from(sx - self.origin.x) / self.tile_px;
        let fy_at = |sy: f32| self.cfy + f64::from(sy - self.origin.y) / self.tile_px;

        let mut x0 = fx_at(rect.left()).floor() as i64 - 1;
        let mut x1 = fx_at(rect.right()).floor() as i64 + 1;
        let mut y0 = fy_at(rect.top()).floor() as i64 - 1;
        let mut y1 = fy_at(rect.bottom()).floor() as i64 + 1;

        // Clamp to the region's own tile extent so we never point-query tiles the
        // bundle does not contain.
        let (bx_min, by_min) = self.tile_index(meta.max_lat, meta.min_lon); // NW corner
        let (bx_max, by_max) = self.tile_index(meta.min_lat, meta.max_lon); // SE corner
        x0 = x0.max(bx_min).max(0);
        x1 = x1.min(bx_max).min(self.n_tiles - 1);
        y0 = y0.max(by_min).max(0);
        y1 = y1.min(by_max).min(self.n_tiles - 1);

        let mut tiles = Vec::new();
        let mut y = y0;
        while y <= y1 && tiles.len() < MAX_TILES_PER_FRAME {
            let mut x = x0;
            while x <= x1 && tiles.len() < MAX_TILES_PER_FRAME {
                tiles.push((x, y));
                x += 1;
            }
            y += 1;
        }
        tiles
    }

    /// Integer XYZ tile index containing a geographic point.
    #[allow(clippy::cast_possible_truncation)]
    fn tile_index(&self, lat: f64, lon: f64) -> (i64, i64) {
        let (fx, fy) = tile_frac(lon, lat, self.tile_z);
        (fx.floor() as i64, fy.floor() as i64)
    }
}

/// Clamp a raw pan offset to a finite, bounded value (matches `scene_point`).
const fn clamp_pan(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-4096.0, 4096.0)
    } else {
        0.0
    }
}

/// Fetch a tile texture from the `ctx` cache, decoding + uploading on a miss.
/// The `conn` is opened lazily on the first miss and reused for the rest of the
/// frame. Both hits and misses are cached, so an absent tile is queried once.
fn tile_texture(
    ctx: &Context,
    meta: &BasemapMeta,
    conn: &mut Option<Connection>,
    z: u8,
    x: i64,
    y: i64,
) -> Option<TextureHandle> {
    let key = egui::Id::new(("mde-maps-tile", meta.region.as_str(), z, x, y));
    if let Some(cached) = ctx.data_mut(|d| d.get_temp::<Option<TextureHandle>>(key)) {
        return cached;
    }
    if conn.is_none() {
        *conn = open_ro(&meta.mbtiles);
    }
    let handle = conn
        .as_ref()
        .and_then(|c| read_tile_conn(c, z, u32::try_from(x).ok()?, u32::try_from(y).ok()?))
        .as_deref()
        .and_then(decode_tile)
        .map(|img| {
            ctx.load_texture(
                format!("maps-tile-{}-{z}-{x}-{y}", meta.region),
                img,
                TextureOptions::LINEAR,
            )
        });
    ctx.data_mut(|d| d.insert_temp(key, handle.clone()));
    handle
}

/// Paint the slippy-tile basemap for `rect`.
///
/// Centred on `center` (`lat`, `lon`) when supplied, else on the region
/// centroid. Returns the [`Projection`] used (so the caller can place geo pins /
/// route previews on top), or `None` when no region is installed — in which case
/// an honest "no data" panel is painted.
#[must_use]
pub fn paint_basemap(
    painter: &Painter,
    rect: Rect,
    map: &MapViewState,
    center: Option<(f64, f64)>,
) -> Option<Projection> {
    let ctx = painter.ctx();
    let Some(meta) = cached_meta(ctx) else {
        paint_no_data(painter, rect);
        return None;
    };

    // Centre on the live fix when it is inside coverage; otherwise the region
    // centroid, so the map always shows installed data (an indoor seat or an
    // off-map fix still renders East Texas rather than a blank void).
    let center = match center {
        Some((lat, lon)) if meta.raw.contains(lat, lon) => (lat, lon),
        _ => (meta.raw.center_lat, meta.raw.center_lon),
    };

    let proj = Projection::new(rect, map, center, &meta.raw);
    let tiles = proj.visible_tiles(rect, &meta.raw);

    let clip = rect.intersect(painter.clip_rect());
    let tile_painter = painter.with_clip_rect(clip);
    let uv = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
    let mut conn: Option<Connection> = None;
    for (x, y) in tiles {
        if let Some(tex) = tile_texture(ctx, &meta, &mut conn, proj.tile_z, x, y) {
            tile_painter.image(tex.id(), proj.tile_rect(x, y), uv, Color32::WHITE);
        }
    }
    Some(proj)
}

/// The honest offline fallback: a centred two-line panel when no region bundle
/// is installed. Colours read from the shared [`Style`] (no map-content leak).
fn paint_no_data(painter: &Painter, rect: Rect) {
    let c = rect.center();
    if !c.x.is_finite() || !c.y.is_finite() {
        return;
    }
    painter.text(
        egui::pos2(c.x, c.y - 10.0),
        egui::Align2::CENTER_BOTTOM,
        "No offline map data",
        FontId::proportional(Style::TITLE),
        Style::TEXT,
    );
    painter.text(
        egui::pos2(c.x, c.y + 10.0),
        egui::Align2::CENTER_TOP,
        "Install a region bundle to see the map",
        FontId::proportional(Style::BODY),
        Style::TEXT_DIM,
    );
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)] // tests fail by panicking, with context; coord→tile casts are exact here
mod tests {
    use super::*;

    /// A 2×2 solid PNG, encoded through the in-tree `image` crate.
    fn png_bytes(rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba(rgba));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// Build a synthetic one-tile `MBTiles` fixture at `path` (z12 tile 957/1661
    /// over the East-Texas centroid), matching the real bundle's schema + TMS
    /// row order — so tests never depend on the real `/root/mcnf-offline-mapdata`.
    fn synth_mbtiles(path: &Path) -> Vec<u8> {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT, value TEXT);
             CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);
             CREATE UNIQUE INDEX tile_index ON tiles (zoom_level, tile_column, tile_row);",
        )
        .unwrap();
        for (n, v) in [
            ("format", "png"),
            ("minzoom", "12"),
            ("maxzoom", "12"),
            ("bounds", "-96.4,31.7,-95.3,32.6"),
            ("center", "-95.849,32.168,12"),
        ] {
            conn.execute(
                "INSERT INTO metadata VALUES (?1, ?2)",
                rusqlite::params![n, v],
            )
            .unwrap();
        }
        // The centroid (32.168, -95.849) sits in XYZ tile (957, 1661) at z12; the
        // `MBTiles` row is TMS: (1<<12)-1 - 1661 = 2434.
        let png = png_bytes([20, 40, 60, 255]);
        conn.execute(
            "INSERT INTO tiles VALUES (12, 957, 2434, ?1)",
            rusqlite::params![png],
        )
        .unwrap();
        png
    }

    #[test]
    fn tile_frac_places_the_centroid_in_the_expected_z12_tile() {
        // Athens / East-Texas centroid.
        let (fx, fy) = tile_frac(-95.849, 32.168, 12);
        assert_eq!(fx.floor() as u32, 957);
        assert_eq!(fy.floor() as u32, 1661);
    }

    #[test]
    fn tile_frac_matches_known_null_island_and_greenwich() {
        // (0,0) sits at the centre of the world at any zoom.
        let (fx, fy) = tile_frac(0.0, 0.0, 1);
        assert!((fx - 1.0).abs() < 1e-9 && (fy - 1.0).abs() < 1e-9);
    }

    #[test]
    fn read_meta_parses_bounds_center_and_zoom() {
        let dir = tempfile::tempdir().unwrap();
        let mb = dir.path().join("synth.mbtiles");
        synth_mbtiles(&mb);
        let meta = read_meta(&mb).unwrap();
        assert_eq!(meta.min_zoom, 12);
        assert_eq!(meta.max_zoom, 12);
        assert!((meta.center_lat - 32.168).abs() < 1e-6);
        assert!(meta.contains(32.168, -95.849));
        assert!(!meta.contains(40.44, -79.99)); // Pittsburgh — outside coverage.
    }

    #[test]
    fn read_tile_honours_tms_row_order() {
        let dir = tempfile::tempdir().unwrap();
        let mb = dir.path().join("synth.mbtiles");
        let png = synth_mbtiles(&mb);
        // XYZ y=1661 must resolve to TMS row 2434 and return the inserted PNG.
        let got = read_tile(&mb, 12, 957, 1661).expect("tile present");
        assert_eq!(got, png);
        // A neighbouring (absent) tile returns None, not an error.
        assert!(read_tile(&mb, 12, 957, 1662).is_none());
        // The bytes decode back to a 2×2 image.
        let img = decode_tile(&png).unwrap();
        assert_eq!(img.size, [2, 2]);
    }

    #[test]
    fn read_tile_missing_file_is_none_not_panic() {
        assert!(read_tile(Path::new("/nonexistent/none.mbtiles"), 12, 1, 1).is_none());
        assert!(read_meta(Path::new("/nonexistent/none.mbtiles")).is_none());
    }

    #[test]
    fn projection_round_trips_the_centre_to_the_rect_centre() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1024.0, 768.0));
        let mut map = MapViewState::simulated();
        map.zoom = 12.0;
        map.pan = [0.0, 0.0];
        let meta = RawMeta {
            center_lat: 32.168,
            center_lon: -95.849,
            min_zoom: 12,
            max_zoom: 12,
            min_lon: -96.4,
            min_lat: 31.7,
            max_lon: -95.3,
            max_lat: 32.6,
        };
        let proj = Projection::new(rect, &map, (meta.center_lat, meta.center_lon), &meta);
        let p = proj.project(meta.center_lat, meta.center_lon);
        assert!((p.x - rect.center().x).abs() < 0.5);
        assert!((p.y - rect.center().y).abs() < 0.5);
        // A point one tile east projects one tile-width to the right.
        let east = proj.project(
            meta.center_lat,
            meta.center_lon + 360.0 / f64::from(1u32 << 12),
        );
        assert!((east.x - rect.center().x - 256.0).abs() < 1.0);
    }

    #[test]
    fn visible_tiles_clamp_to_region_bounds() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1024.0, 768.0));
        let mut map = MapViewState::simulated();
        map.zoom = 12.0;
        let meta = RawMeta {
            center_lat: 32.168,
            center_lon: -95.849,
            min_zoom: 12,
            max_zoom: 12,
            min_lon: -96.4,
            min_lat: 31.7,
            max_lon: -95.3,
            max_lat: 32.6,
        };
        let proj = Projection::new(rect, &map, (meta.center_lat, meta.center_lon), &meta);
        let tiles = proj.visible_tiles(rect, &meta);
        assert!(!tiles.is_empty());
        // Every returned tile is inside the region's z12 tile extent.
        let (x_nw, y_nw) = proj.tile_index(meta.max_lat, meta.min_lon);
        let (x_se, y_se) = proj.tile_index(meta.min_lat, meta.max_lon);
        for (x, y) in tiles {
            assert!(x >= x_nw && x <= x_se, "x {x} out of [{x_nw},{x_se}]");
            assert!(y >= y_nw && y <= y_se, "y {y} out of [{y_nw},{y_se}]");
        }
    }
}

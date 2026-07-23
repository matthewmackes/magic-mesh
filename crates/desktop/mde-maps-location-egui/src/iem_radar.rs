//! IEM/NWS NEXRAD animated radar-tile layer and bounded PNG painter.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;

use base64::Engine as _;
use mackes_mesh_types::iem_radar::{IemRadarFrame, IemRadarSnapshot, IemRadarTile, ATTRIBUTION};
use mde_egui::egui::{
    self, Color32, FontId, Painter, Pos2, Rect, Stroke, TextureHandle, TextureOptions,
};
use mde_egui::Style;

/// Radar producer time older than twenty minutes is never painted as live.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 20 * 60 * 1_000;
/// Animation frame dwell time.
pub const FRAME_DWELL_MS: i64 = 900;

const MAX_PNG_BYTES: usize = 96 * 1024;
const MAX_PNG_BASE64_BYTES: usize = 4 * MAX_PNG_BYTES.div_ceil(3);
const TILE_EDGE: u32 = 256;
const MAX_DECODE_ALLOC: u64 = 2 * 1024 * 1024;
const MAX_TEXTURES: usize = 24;

/// Retained complete radar animation and local animation preference.
#[derive(Debug, Clone)]
pub struct IemRadarLayerState {
    /// Latest exact producer-frame set.
    pub snapshot: Option<IemRadarSnapshot>,
    /// Animate history oldest-to-newest; false pins the newest frame.
    pub animate: bool,
}

impl Default for IemRadarLayerState {
    fn default() -> Self {
        Self {
            snapshot: None,
            animate: true,
        }
    }
}

impl IemRadarLayerState {
    /// Replace the previous frame set wholesale.
    pub fn fold(&mut self, snapshot: IemRadarSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Exact newest composite age from IEM producer metadata.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.frames.first())
            .map(|frame| now_ms.saturating_sub(frame.valid_at_ms).max(0))
    }

    /// Whether the exact producer time exceeds the safety threshold.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Whether last-good tiles were retained after fix/refresh loss.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.starts_with("IEM radar paused:"))
        })
    }

    /// Required active-layer attribution.
    #[must_use]
    pub const fn attribution() -> &'static str {
        ATTRIBUTION
    }
}

/// Observable painter facts used by headless regressions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Decoded raster tiles painted for the selected frame.
    pub tiles: usize,
    /// Selected newest-to-oldest frame index.
    pub frame_index: usize,
    /// Whether the honest age badge painted.
    pub badge: bool,
    /// Whether every tile was forced to non-live opacity.
    pub non_live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextureKey {
    valid_at_ms: i64,
    z: u8,
    x: u32,
    y: u32,
}

#[derive(Clone, Default)]
struct TextureCache {
    entries: HashMap<TextureKey, Option<TextureHandle>>,
    order: VecDeque<TextureKey>,
}

/// Paint one exact Web-Mercator radar frame below vector overlays.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &IemRadarLayerState,
    now_ms: i64,
    mut project: F,
) -> PaintStats
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return PaintStats::default();
    }
    let non_live = layer.stale(now_ms) || layer.paused();
    let mut stats = PaintStats {
        non_live,
        ..PaintStats::default()
    };
    if let Some(snapshot) = &layer.snapshot {
        if let Some((frame_index, frame)) = selected_frame(snapshot, layer.animate, now_ms) {
            stats.frame_index = frame_index;
            let tile_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
            for tile in &frame.tiles {
                let Some(texture) = cached_texture(painter.ctx(), frame, tile) else {
                    continue;
                };
                let Some(tile_rect) = projected_tile_rect(tile, &mut project) else {
                    continue;
                };
                if !tile_rect.is_finite() || !tile_rect.intersects(rect) {
                    continue;
                }
                tile_painter.image(
                    texture.id(),
                    tile_rect,
                    Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE.gamma_multiply(if non_live { 0.18 } else { 0.48 }),
                );
                stats.tiles += 1;
            }
        }
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn selected_frame(
    snapshot: &IemRadarSnapshot,
    animate: bool,
    now_ms: i64,
) -> Option<(usize, &IemRadarFrame)> {
    if snapshot.frames.is_empty() {
        return None;
    }
    let index = if animate && snapshot.frames.len() > 1 {
        let step = usize::try_from(now_ms.max(0) / FRAME_DWELL_MS).unwrap_or(0);
        snapshot.frames.len() - 1 - step % snapshot.frames.len()
    } else {
        0
    };
    Some((index, &snapshot.frames[index]))
}

fn projected_tile_rect<F>(tile: &IemRadarTile, project: &mut F) -> Option<Rect>
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    let n = f64::from(1_u32.checked_shl(u32::from(tile.z))?);
    if tile.x >= n as u32 || tile.y >= n as u32 {
        return None;
    }
    let west = f64::from(tile.x) / n * 360.0 - 180.0;
    let east = f64::from(tile.x + 1) / n * 360.0 - 180.0;
    let north = tile_y_lat(f64::from(tile.y), n);
    let south = tile_y_lat(f64::from(tile.y + 1), n);
    let northwest = project(north, west)?;
    let southeast = project(south, east)?;
    if northwest.any_nan() || southeast.any_nan() {
        return None;
    }
    Some(Rect::from_min_max(
        egui::pos2(northwest.x.min(southeast.x), northwest.y.min(southeast.y)),
        egui::pos2(northwest.x.max(southeast.x), northwest.y.max(southeast.y)),
    ))
}

fn tile_y_lat(y: f64, n: f64) -> f64 {
    (std::f64::consts::PI * (1.0 - 2.0 * y / n))
        .sinh()
        .atan()
        .to_degrees()
}

fn cached_texture(
    ctx: &egui::Context,
    frame: &IemRadarFrame,
    tile: &IemRadarTile,
) -> Option<TextureHandle> {
    let cache_id = egui::Id::new("iem-radar-texture-cache");
    let key = TextureKey {
        valid_at_ms: frame.valid_at_ms,
        z: tile.z,
        x: tile.x,
        y: tile.y,
    };
    if let Some(cached) = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<TextureCache>(cache_id)
            .entries
            .get(&key)
            .cloned()
    }) {
        return cached;
    }
    let texture = decode_tile(&tile.png_base64).map(|image| {
        ctx.load_texture(
            format!(
                "iem-radar-{}-{}-{}-{}",
                frame.valid_at_ms, tile.z, tile.x, tile.y
            ),
            image,
            TextureOptions::LINEAR,
        )
    });
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<TextureCache>(cache_id);
        cache.entries.insert(key.clone(), texture.clone());
        cache.order.retain(|existing| existing != &key);
        cache.order.push_back(key);
        while cache.order.len() > MAX_TEXTURES {
            if let Some(expired) = cache.order.pop_front() {
                cache.entries.remove(&expired);
            }
        }
    });
    texture
}

fn decode_tile(value: &str) -> Option<egui::ColorImage> {
    if value.len() > MAX_PNG_BASE64_BYTES {
        return None;
    }
    let png = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    if png.len() > MAX_PNG_BYTES || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(TILE_EDGE);
    limits.max_image_height = Some(TILE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().ok()?.to_rgba8();
    if decoded.width() != TILE_EDGE || decoded.height() != TILE_EDGE {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [TILE_EDGE as usize, TILE_EDGE as usize],
        decoded.as_raw(),
    ))
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &IemRadarLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("NEXRAD · no data".to_string(), Style::TEXT_DIM),
        (Some(_), Some(age)) if layer.paused() => {
            (format!("NEXRAD · PAUSED · {}", age_label(age)), Style::WARN)
        }
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => {
            (format!("NEXRAD · STALE {}", age_label(age)), Style::WARN)
        }
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "NEXRAD · {} · {} frames · degraded",
                age_label(age),
                snapshot.frames.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => {
            let quorum = snapshot
                .frames
                .first()
                .and_then(|frame| frame.radar_quorum.as_deref())
                .map_or_else(String::new, |quorum| format!(" · {quorum} radars"));
            (
                format!(
                    "NEXRAD · {} · {} frames{quorum}",
                    age_label(age),
                    snapshot.frames.len()
                ),
                Style::TEXT,
            )
        }
        (Some(_), None) => ("NEXRAD · no producer time".to_string(), Style::WARN),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 6.0,
        ),
        galley.size() + pad * 2.0,
    );
    painter.rect_filled(badge, Style::RADIUS_S, Style::BG.gamma_multiply(0.86));
    painter.rect_stroke(
        badge,
        Style::RADIUS_S,
        Stroke::new(1.0, tone.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );
    painter.galley(badge.left_top() + pad, galley, tone);
}

fn age_label(age_ms: i64) -> String {
    let minutes = age_ms.max(0) / 60_000;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h", minutes / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_base64() -> String {
        let image = image::RgbaImage::from_pixel(256, 256, image::Rgba([10, 120, 220, 128]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("png");
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    }

    fn snapshot(now_ms: i64) -> IemRadarSnapshot {
        let mut snapshot = IemRadarSnapshot::empty("rig-1", now_ms, 42.36, -71.06);
        for index in 0..3_u8 {
            snapshot.frames.push(IemRadarFrame {
                valid_at_ms: now_ms - i64::from(index) * 300_000,
                nominal_minutes_ago: index * 5,
                radar_quorum: Some("143/147".to_string()),
                tiles: vec![IemRadarTile {
                    z: 6,
                    x: 19,
                    y: 23,
                    png_base64: png_base64(),
                }],
            });
        }
        snapshot
    }

    #[test]
    fn strict_decoder_rejects_non_png_and_wrong_dimensions() {
        assert!(decode_tile("aGVsbG8=").is_none());
        assert!(decode_tile(&"A".repeat(MAX_PNG_BASE64_BYTES + 1)).is_none());
        let image = decode_tile(&png_base64()).expect("valid tile");
        assert_eq!(image.size, [256, 256]);
    }

    #[test]
    fn animation_cycles_oldest_to_newest_and_static_pins_latest() {
        let snapshot = snapshot(1_000_000);
        assert_eq!(selected_frame(&snapshot, false, 0).expect("frame").0, 0);
        assert_eq!(selected_frame(&snapshot, true, 0).expect("frame").0, 2);
        assert_eq!(
            selected_frame(&snapshot, true, FRAME_DWELL_MS)
                .expect("frame")
                .0,
            1
        );
    }

    #[test]
    fn painter_decodes_projects_and_dims_immediately_when_paused() {
        let now_ms = 1_000_000;
        let mut layer = IemRadarLayerState::default();
        layer.animate = false;
        layer.fold(snapshot(now_ms));
        let context = egui::Context::default();
        let mut live = PaintStats::default();
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                live = paint_layer(ui.painter(), rect, &layer, now_ms, |lat, lon| {
                    Some(egui::pos2(
                        rect.center().x + ((lon + 71.0) * 10.0) as f32,
                        rect.center().y - ((lat - 42.0) * 10.0) as f32,
                    ))
                });
            });
        });
        assert_eq!(live.tiles, 1);
        assert!(!live.non_live);
        layer
            .snapshot
            .as_mut()
            .expect("snapshot")
            .gaps
            .push("IEM radar paused: no fresh fix".to_string());
        let mut paused = PaintStats::default();
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                paused = paint_layer(ui.painter(), rect, &layer, now_ms, |lat, lon| {
                    Some(egui::pos2(
                        rect.center().x + ((lon + 71.0) * 10.0) as f32,
                        rect.center().y - ((lat - 42.0) * 10.0) as f32,
                    ))
                });
            });
        });
        assert!(paused.non_live);
    }
}

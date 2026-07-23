//! Caltrans CWWP2 traffic-camera map layer and bounded JPEG still painter.

use std::io::Cursor;

use base64::Engine as _;
use mackes_mesh_types::caltrans_camera::{
    CaltransCamera, CaltransCameraSnapshot, CameraThumbnail, ATTRIBUTION,
};
use mde_egui::egui::{
    self, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, TextureHandle, TextureOptions,
};
use mde_egui::Style;

/// Three missed one-minute refreshes make the whole layer visibly stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 3 * 60 * 1_000;
/// Current stills fade after two advertised one-minute refreshes.
pub const THUMBNAIL_STALE_AFTER_MS: i64 = 2 * 60 * 1_000;
/// Old stills disappear rather than lingering indefinitely.
pub const THUMBNAIL_DROP_AFTER_MS: i64 = 10 * 60 * 1_000;

const MAX_JPEG_BYTES: usize = 128 * 1024;
const MAX_JPEG_BASE64_BYTES: usize = 4 * MAX_JPEG_BYTES.div_ceil(3);
const MAX_IMAGE_DIMENSION: u32 = 2_048;
const MAX_DECODE_ALLOC: u64 = 16 * 1024 * 1024;
const CARD_WIDTH: f32 = 152.0;
const CARD_HEIGHT: f32 = 116.0;
const IMAGE_HEIGHT: f32 = 82.0;

/// Retained complete vehicle-scoped camera snapshot.
#[derive(Debug, Clone, Default)]
pub struct CaltransCameraLayerState {
    /// Latest bounded camera set, absent before the adapter publishes.
    pub snapshot: Option<CaltransCameraSnapshot>,
}

impl CaltransCameraLayerState {
    /// Replace the previous point-scoped set wholesale.
    pub fn fold(&mut self, snapshot: CaltransCameraSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Age since the adapter most recently validated the catalog.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| now_ms.saturating_sub(snapshot.fetched_at_ms).max(0))
    }

    /// Whether validated camera data is older than three refresh windows.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Whether last-good data was retained after refresh or vehicle-fix loss.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.starts_with("Caltrans cameras paused:"))
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
    /// Nearby camera pins painted inside the viewport.
    pub markers: usize,
    /// Decoded current-still cards painted.
    pub cards: usize,
    /// Whether the honest state badge painted.
    pub badge: bool,
    /// Whether every marker/card was forced to a non-live tone.
    pub non_live: bool,
}

#[derive(Clone)]
struct CachedThumbnail {
    observed_at_ms: i64,
    texture: Option<TextureHandle>,
}

/// Paint nearby camera pins, up to three current stills, and freshness state.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &CaltransCameraLayerState,
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
    let marker_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let mut stats = PaintStats {
        non_live,
        ..PaintStats::default()
    };
    if let Some(snapshot) = &layer.snapshot {
        for camera in &snapshot.cameras {
            if !valid_coordinate(camera.latitude, camera.longitude) {
                continue;
            }
            let Some(point) = project(camera.latitude, camera.longitude) else {
                continue;
            };
            if point.any_nan() || !rect.expand(18.0).contains(point) {
                continue;
            }
            paint_camera_pin(&marker_painter, point, camera, non_live);
            stats.markers += 1;
        }
        for camera in snapshot
            .cameras
            .iter()
            .filter(|camera| camera.thumbnail.is_some())
        {
            if stats.cards >= 3 {
                break;
            }
            let thumbnail = camera.thumbnail.as_ref().expect("filtered above");
            let age_ms = now_ms.saturating_sub(thumbnail.observed_at_ms).max(0);
            if age_ms > THUMBNAIL_DROP_AFTER_MS {
                continue;
            }
            let Some(texture) =
                cached_thumbnail(painter.ctx(), snapshot.district, camera, thumbnail)
            else {
                continue;
            };
            paint_thumbnail_card(
                &marker_painter,
                rect,
                camera,
                &texture,
                stats.cards,
                non_live || age_ms > THUMBNAIL_STALE_AFTER_MS,
                age_ms,
            );
            stats.cards += 1;
        }
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn valid_coordinate(latitude: f64, longitude: f64) -> bool {
    latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
}

fn paint_camera_pin(painter: &Painter, point: Pos2, camera: &CaltransCamera, non_live: bool) {
    let tone = if non_live || !camera.in_service {
        Style::TEXT_DIM
    } else {
        Style::ACCENT_HI
    };
    painter.circle_filled(point, 8.0, Style::BG.gamma_multiply(0.93));
    painter.circle_stroke(point, 8.0, Stroke::new(1.5, tone));
    painter.rect_filled(
        Rect::from_center_size(point, egui::vec2(8.0, 5.5)),
        1.5,
        tone.gamma_multiply(0.8),
    );
    painter.circle_filled(point, 1.8, Style::BG);
}

fn cached_thumbnail(
    ctx: &egui::Context,
    district: u8,
    camera: &CaltransCamera,
    thumbnail: &CameraThumbnail,
) -> Option<TextureHandle> {
    let key = egui::Id::new(("caltrans-camera-thumbnail", district, camera.id.as_str()));
    if let Some(cached) = ctx.data_mut(|data| data.get_temp::<CachedThumbnail>(key)) {
        if cached.observed_at_ms == thumbnail.observed_at_ms {
            return cached.texture;
        }
    }
    let texture = decode_thumbnail(&thumbnail.jpeg_base64).map(|image| {
        ctx.load_texture(
            format!("caltrans-d{district}-{}", camera.id),
            image,
            TextureOptions::LINEAR,
        )
    });
    ctx.data_mut(|data| {
        data.insert_temp(
            key,
            CachedThumbnail {
                observed_at_ms: thumbnail.observed_at_ms,
                texture: texture.clone(),
            },
        );
    });
    texture
}

fn decode_thumbnail(value: &str) -> Option<egui::ColorImage> {
    if value.len() > MAX_JPEG_BASE64_BYTES {
        return None;
    }
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    if jpeg.len() > MAX_JPEG_BYTES
        || !jpeg.starts_with(&[0xFF, 0xD8])
        || !jpeg.ends_with(&[0xFF, 0xD9])
    {
        return None;
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(jpeg), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().ok()?.to_rgba8();
    if decoded.width() == 0 || decoded.height() == 0 {
        return None;
    }
    let size = [decoded.width() as usize, decoded.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        decoded.as_raw(),
    ))
}

fn paint_thumbnail_card(
    painter: &Painter,
    viewport: Rect,
    camera: &CaltransCamera,
    texture: &TextureHandle,
    row: usize,
    non_live: bool,
    age_ms: i64,
) {
    let card = Rect::from_min_size(
        egui::pos2(
            viewport.left() + Style::SP_S,
            viewport.top() + Style::SP_S + row as f32 * (CARD_HEIGHT + Style::SP_XS),
        ),
        egui::vec2(CARD_WIDTH, CARD_HEIGHT),
    )
    .intersect(viewport);
    if card.width() < 40.0 || card.height() < 40.0 {
        return;
    }
    let tone = if non_live {
        Style::TEXT_DIM
    } else {
        Style::ACCENT_HI
    };
    painter.rect_filled(card, Style::RADIUS_S, Style::BG.gamma_multiply(0.93));
    painter.rect_stroke(
        card,
        Style::RADIUS_S,
        Stroke::new(1.0, tone.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    let image_rect = Rect::from_min_max(
        card.left_top() + egui::vec2(4.0, 4.0),
        egui::pos2(
            card.right() - 4.0,
            (card.top() + IMAGE_HEIGHT).min(card.bottom() - 4.0),
        ),
    );
    painter.image(
        texture.id(),
        image_rect,
        Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
        if non_live {
            Color32::WHITE.gamma_multiply(0.45)
        } else {
            Color32::WHITE
        },
    );
    let route = camera.route.as_deref().unwrap_or("Camera");
    painter.text(
        egui::pos2(card.left() + 6.0, image_rect.bottom() + 5.0),
        Align2::LEFT_TOP,
        format!("{route} · {}", camera.name),
        FontId::proportional(Style::SMALL),
        tone,
    );
    painter.text(
        egui::pos2(card.left() + 6.0, card.bottom() - 5.0),
        Align2::LEFT_BOTTOM,
        format!("{} · {:.1} nm", age_label(age_ms), camera.distance_nm),
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &CaltransCameraLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("Caltrans cameras · no data".to_string(), Style::TEXT_DIM),
        (Some(_), Some(age)) if layer.paused() => (
            format!("Caltrans cameras · PAUSED · {}", age_label(age)),
            Style::WARN,
        ),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("Caltrans cameras · STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "Caltrans cameras · {} · {} nearby · degraded",
                age_label(age),
                snapshot.cameras.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "Caltrans cameras · {} · {} nearby",
                age_label(age),
                snapshot.cameras.len()
            ),
            Style::TEXT,
        ),
        (Some(_), None) => ("Caltrans cameras · no timestamp".to_string(), Style::WARN),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 5.0,
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
    let seconds = age_ms.max(0) / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m", seconds / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_base64() -> String {
        let image = image::DynamicImage::new_rgb8(4, 3);
        let mut bytes = Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .expect("jpeg");
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    }

    fn snapshot(now_ms: i64) -> CaltransCameraSnapshot {
        let mut snapshot = CaltransCameraSnapshot::empty("rig-1", 3, now_ms, 38.481, -121.511);
        snapshot.cameras.push(CaltransCamera {
            id: "1".to_string(),
            name: "Hwy 5 at Pocket".to_string(),
            nearby_place: Some("Sacramento".to_string()),
            county: Some("Sacramento".to_string()),
            route: Some("I-5".to_string()),
            direction: Some("Median".to_string()),
            latitude: 38.481,
            longitude: -121.511,
            distance_nm: 0.1,
            in_service: true,
            record_at_ms: Some(now_ms),
            image_url: "https://cwwp2.dot.ca.gov/data/d3/cctv/image/hwy5atpocket/hwy5atpocket.jpg"
                .to_string(),
            image_update_minutes: Some(1),
            thumbnail: Some(CameraThumbnail {
                observed_at_ms: now_ms,
                jpeg_base64: jpeg_base64(),
            }),
        });
        snapshot
    }

    #[test]
    fn bounded_jpeg_decoder_rejects_non_jpeg_and_oversized_base64() {
        assert!(decode_thumbnail("aGVsbG8=").is_none());
        assert!(decode_thumbnail(&"A".repeat(MAX_JPEG_BASE64_BYTES + 1)).is_none());
        let image = decode_thumbnail(&jpeg_base64()).expect("valid jpeg");
        assert_eq!(image.size, [4, 3]);
    }

    #[test]
    fn painter_renders_real_still_and_dims_immediately_when_paused() {
        let now_ms = 1_000_000;
        let mut layer = CaltransCameraLayerState::default();
        layer.fold(snapshot(now_ms));
        let context = egui::Context::default();
        let mut live = PaintStats::default();
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                live = paint_layer(ui.painter(), rect, &layer, now_ms, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(live.markers, 1);
        assert_eq!(live.cards, 1);
        assert!(!live.non_live);

        layer
            .snapshot
            .as_mut()
            .expect("snapshot")
            .gaps
            .push("Caltrans cameras paused: vehicle fix unavailable".to_string());
        let mut paused = PaintStats::default();
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                paused = paint_layer(ui.painter(), rect, &layer, now_ms, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert!(paused.non_live);
        assert_eq!(paused.cards, 1);
    }
}

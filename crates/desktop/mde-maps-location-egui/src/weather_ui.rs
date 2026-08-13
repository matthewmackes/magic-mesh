//! Render-agnostic Weather presentation state for the Maps workspace.
//!
//! The daemon remains the only weather authority. This module only folds the
//! four canonical retained projections into a bounded, generation-consistent
//! view model; it performs no network or persistence work.

use std::io::Cursor;

use base64::Engine as _;
use mackes_mesh_types::location::{
    EffectiveLocationSnapshot, EffectiveLocationState, SetWeatherLocationRequest,
    WeatherLocationMode, WEATHER_LOCATION_SCHEMA_VERSION,
};
use mackes_mesh_types::weather::{
    AtmosphericFieldKind, AtmosphericMapSnapshot, AtmosphericViewport, CurrentWeatherSnapshot,
    LocalDaySummary, SetWeatherMapViewportRequest, Temperature, TemperatureUnit,
    WeatherAvailability, WeatherForecastSnapshot, WeatherMapViewportState, ATMOSPHERIC_FIELD_EDGE,
    WEATHER_CONTRACT_SCHEMA_VERSION,
};
use mde_egui::egui::{self, Color32, Painter, Rect, TextureOptions};

const MAX_DECODED_PNG_BYTES: usize = 2 * 1024 * 1024;
const MIN_MANUAL_SEARCH_CHARS: usize = 2;
const MAX_MANUAL_SEARCH_BYTES: usize = 4 * 1024;

/// Honest state of the explicit-submit offline manual-location search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ManualLocationSearchStatus {
    /// No search has been submitted.
    #[default]
    Idle,
    /// An explicit request is queued for off-render execution.
    Pending,
    /// One or more verified offline rows are available.
    Results,
    /// Search completed without an admissible row.
    NoResults,
    /// The offline authority is missing or unreadable.
    Unavailable,
}

impl ManualLocationSearchStatus {
    /// Honest operator-facing summary.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Enter a place and select Search",
            Self::Pending => "Searching the offline gazetteer…",
            Self::Results => "Verified offline results",
            Self::NoResults => "No verified weather locations found",
            Self::Unavailable => "Offline location search unavailable",
        }
    }
}

/// Forecast range selected in Weather mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WeatherRange {
    /// Current observations only.
    #[default]
    Current,
    /// Today.
    OneDay,
    /// Three local days.
    ThreeDay,
    /// Five local days.
    FiveDay,
}

impl WeatherRange {
    pub const ALL: [Self; 4] = [Self::Current, Self::OneDay, Self::ThreeDay, Self::FiveDay];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::OneDay => "1D",
            Self::ThreeDay => "3D",
            Self::FiveDay => "5D",
        }
    }

    const fn day_count(self) -> usize {
        match self {
            Self::Current => 0,
            Self::OneDay => 1,
            Self::ThreeDay => 3,
            Self::FiveDay => 5,
        }
    }
}

/// Exactly one daemon-provided atmospheric field shown over the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WeatherField {
    #[default]
    Temperature,
    Wind,
    Cloud,
}

impl WeatherField {
    pub const ALL: [Self; 3] = [Self::Temperature, Self::Wind, Self::Cloud];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Temperature => "Temperature",
            Self::Wind => "Wind",
            Self::Cloud => "Cloud",
        }
    }

    pub(crate) const fn contract_kind(self) -> AtmosphericFieldKind {
        match self {
            Self::Temperature => AtmosphericFieldKind::Temperature,
            Self::Wind => AtmosphericFieldKind::Wind,
            Self::Cloud => AtmosphericFieldKind::CloudCover,
        }
    }
}

#[derive(Debug, Clone)]
struct DecodedAtmosphericField {
    host: String,
    location_generation: u64,
    viewport: AtmosphericViewport,
    rendered_at_ms: i64,
    kind: AtmosphericFieldKind,
    image: egui::ColorImage,
}

/// Honest coarse state rendered beside each projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherTruth {
    Fresh,
    Stale,
    Unavailable,
}

impl WeatherTruth {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fresh => "Fresh",
            Self::Stale => "Stale",
            Self::Unavailable => "Unavailable",
        }
    }
}

fn availability_truth(value: Option<&WeatherAvailability>) -> WeatherTruth {
    match value {
        Some(WeatherAvailability::Fresh) => WeatherTruth::Fresh,
        Some(WeatherAvailability::Stale { .. }) => WeatherTruth::Stale,
        Some(WeatherAvailability::Unavailable { .. }) | None => WeatherTruth::Unavailable,
    }
}

/// The single Weather state owned by Maps.
#[derive(Debug, Clone, Default)]
pub struct WeatherUiState {
    /// Whether the map-first Weather controls are visible.
    pub active: bool,
    /// Current/forecast range selection.
    pub range: WeatherRange,
    /// Mutually exclusive atmospheric field selection.
    pub field: WeatherField,
    location: Option<EffectiveLocationSnapshot>,
    current: Option<CurrentWeatherSnapshot>,
    forecast: Option<WeatherForecastSnapshot>,
    atmosphere: Option<AtmosphericMapSnapshot>,
    viewport: Option<WeatherMapViewportState>,
    decoded_fields: Vec<DecodedAtmosphericField>,
    pending_viewport: Option<SetWeatherMapViewportRequest>,
    last_queued_tile: Option<(u64, u8, u32, u32)>,
    last_viewport_generation: u64,
    /// Editable text only. Search I/O is explicitly submitted and consumed by
    /// the model before rendering the next frame.
    pub manual_search_query: String,
    manual_search_results: Vec<crate::geocode::WeatherGeoResult>,
    manual_search_note: Option<String>,
    manual_search_status: ManualLocationSearchStatus,
    pending_manual_search: Option<String>,
    pending_location_action: Option<SetWeatherLocationRequest>,
}

impl WeatherUiState {
    /// Queue an explicit-submit offline search. This performs no disk or Bus I/O.
    pub fn submit_manual_search(&mut self) {
        let query = self.manual_search_query.trim().to_string();
        if query.chars().count() < MIN_MANUAL_SEARCH_CHARS || query.len() > MAX_MANUAL_SEARCH_BYTES
        {
            self.manual_search_results.clear();
            self.manual_search_note = Some(format!(
                "Enter between {MIN_MANUAL_SEARCH_CHARS} characters and {MAX_MANUAL_SEARCH_BYTES} bytes"
            ));
            self.manual_search_status = ManualLocationSearchStatus::NoResults;
            self.pending_manual_search = None;
            return;
        }
        self.manual_search_query = query.clone();
        self.manual_search_results.clear();
        self.manual_search_note = None;
        self.manual_search_status = ManualLocationSearchStatus::Pending;
        self.pending_manual_search = Some(query);
    }

    pub(crate) fn take_pending_manual_search(&mut self) -> Option<String> {
        self.pending_manual_search.take()
    }

    pub(crate) fn complete_manual_search(
        &mut self,
        query: &str,
        outcome: crate::geocode::WeatherGeocodeOutcome,
    ) {
        if self.manual_search_query.trim() != query {
            return;
        }
        self.manual_search_results = outcome.results;
        self.manual_search_note = outcome.note;
        self.manual_search_status = if !self.manual_search_results.is_empty() {
            ManualLocationSearchStatus::Results
        } else if self
            .manual_search_note
            .as_deref()
            .is_some_and(|note| note.contains("unavailable"))
        {
            ManualLocationSearchStatus::Unavailable
        } else {
            ManualLocationSearchStatus::NoResults
        };
    }

    /// Admitted results for the most recently completed query.
    #[must_use]
    pub fn manual_search_results(&self) -> &[crate::geocode::WeatherGeoResult] {
        &self.manual_search_results
    }

    /// Current search lifecycle state.
    #[must_use]
    pub fn manual_search_status(&self) -> ManualLocationSearchStatus {
        self.manual_search_status
    }

    /// Provider/result note suitable for direct rendering.
    #[must_use]
    pub fn manual_search_message(&self) -> &str {
        self.manual_search_note
            .as_deref()
            .unwrap_or_else(|| self.manual_search_status.label())
    }

    /// Queue the exact typed manual preference action for one admitted result.
    /// Bus publication happens in the model refresh, outside paint.
    pub fn select_manual_result(&mut self, index: usize, issued_at_ms: i64) {
        let Some(result) = self.manual_search_results.get(index) else {
            return;
        };
        let expected_generation = self
            .location
            .as_ref()
            .map_or(0, |snapshot| snapshot.generation);
        let request = SetWeatherLocationRequest {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            request_id: format!("maps-manual-{expected_generation}-{issued_at_ms}"),
            expected_generation,
            issued_at_ms,
            mode: WeatherLocationMode::Manual,
            manual_place: Some(result.verified_place(issued_at_ms)),
        };
        if request.validate_at(issued_at_ms).is_ok() {
            self.pending_location_action = Some(request);
        }
    }

    /// Typed manual-location action awaiting off-render Bus publication.
    #[must_use]
    pub fn pending_location_action(&self) -> Option<&SetWeatherLocationRequest> {
        self.pending_location_action.as_ref()
    }

    /// Clear a published action only when its exact request ID still wins.
    pub fn clear_pending_location_action(&mut self, request_id: &str) {
        if self
            .pending_location_action
            .as_ref()
            .is_some_and(|request| request.request_id == request_id)
        {
            self.pending_location_action = None;
        }
    }

    /// Replace the retained projection set as one generation-scoped fold.
    /// Missing, cross-host, or generation-mismatched children become honestly
    /// unavailable instead of retaining data for a previous location.
    pub fn fold(
        &mut self,
        host: &str,
        location: Option<EffectiveLocationSnapshot>,
        current: Option<CurrentWeatherSnapshot>,
        forecast: Option<WeatherForecastSnapshot>,
        atmosphere: Option<AtmosphericMapSnapshot>,
        viewport: Option<WeatherMapViewportState>,
    ) {
        let location = location.filter(|snapshot| snapshot.host == host);
        let generation = location.as_ref().map(|snapshot| snapshot.generation);
        self.current = current.filter(|snapshot| {
            snapshot.host == host && Some(snapshot.location_generation) == generation
        });
        self.forecast = forecast.filter(|snapshot| {
            snapshot.host == host && Some(snapshot.location_generation) == generation
        });
        self.viewport = viewport.filter(|snapshot| {
            snapshot.host == host && Some(snapshot.location_generation) == generation
        });
        self.atmosphere = atmosphere.filter(|snapshot| {
            snapshot.host == host
                && Some(snapshot.location_generation) == generation
                && self
                    .viewport
                    .as_ref()
                    .is_none_or(|viewport| viewport.viewport == snapshot.viewport)
        });
        self.last_viewport_generation = self
            .last_viewport_generation
            .max(generation.unwrap_or(0))
            .max(
                self.viewport
                    .as_ref()
                    .map_or(0, |state| state.viewport.generation),
            )
            .max(
                self.atmosphere
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.viewport.generation),
            );
        if self
            .pending_viewport
            .as_ref()
            .is_some_and(|request| Some(request.expected_location_generation) != generation)
        {
            self.pending_viewport = None;
            self.last_queued_tile = None;
        }
        // A manual-location request is a compare-and-set against the exact
        // location projection the operator selected from.  Once a newer (or
        // missing/foreign) projection is folded, retrying that old request can
        // no longer represent the selection the UI displayed.  Revoke it here
        // instead of relying on the daemon to reject an obsolete generation on
        // every subsequent refresh.
        if self
            .pending_location_action
            .as_ref()
            .is_some_and(|request| Some(request.expected_generation) != generation)
        {
            self.pending_location_action = None;
        }
        self.refresh_decoded_fields();
        self.location = location;
    }

    /// Queue a latest-wins action when the interactive map moves to a new XYZ
    /// tile. Disk publication is deliberately performed by the model refresh,
    /// never by the renderer.
    pub fn queue_interactive_viewport(
        &mut self,
        host: &str,
        zoom: f32,
        pan: [f32; 2],
        issued_at_ms: i64,
    ) {
        let Some((location_generation, latitude, longitude)) = self.location_point() else {
            return;
        };
        let Some((tile_zoom, x, y)) = interactive_tile(latitude, longitude, zoom, pan) else {
            return;
        };
        let tile = (location_generation, tile_zoom, x, y);
        if self.last_queued_tile == Some(tile) {
            return;
        }
        self.last_viewport_generation = self
            .last_viewport_generation
            .max(location_generation)
            .saturating_add(1);
        let viewport = AtmosphericViewport {
            generation: self.last_viewport_generation,
            zoom: tile_zoom,
            x,
            y,
            pixel_width: ATMOSPHERIC_FIELD_EDGE,
            pixel_height: ATMOSPHERIC_FIELD_EDGE,
        };
        self.pending_viewport = Some(SetWeatherMapViewportRequest {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            request_id: format!(
                "maps-{location_generation}-{}-{tile_zoom}-{x}-{y}",
                viewport.generation
            ),
            target_host: host.to_string(),
            expected_location_generation: location_generation,
            viewport,
            issued_at_ms,
        });
        self.last_queued_tile = Some(tile);
    }

    #[must_use]
    pub fn pending_viewport(&self) -> Option<&SetWeatherMapViewportRequest> {
        self.pending_viewport.as_ref()
    }

    pub fn clear_pending_viewport(&mut self, request_id: &str) {
        if self
            .pending_viewport
            .as_ref()
            .is_some_and(|request| request.request_id == request_id)
        {
            self.pending_viewport = None;
        }
    }

    fn location_point(&self) -> Option<(u64, f64, f64)> {
        let snapshot = self.location.as_ref()?;
        let location = match &snapshot.state {
            EffectiveLocationState::Available { location }
            | EffectiveLocationState::Stale { location, .. } => location,
            EffectiveLocationState::Unavailable { .. } => return None,
        };
        Some((
            snapshot.generation,
            location.point.latitude,
            location.point.longitude,
        ))
    }

    fn refresh_decoded_fields(&mut self) {
        let Some(snapshot) = self.atmosphere.as_ref() else {
            self.decoded_fields.clear();
            return;
        };
        if matches!(
            snapshot.availability,
            WeatherAvailability::Unavailable { .. }
        ) {
            self.decoded_fields.clear();
            return;
        }
        let same_snapshot = self.decoded_fields.first().is_some_and(|field| {
            field.host == snapshot.host
                && field.location_generation == snapshot.location_generation
                && field.viewport == snapshot.viewport
                && field.rendered_at_ms == snapshot.rendered_at_ms
        });
        if same_snapshot {
            return;
        }
        self.decoded_fields = snapshot
            .fields
            .iter()
            .filter_map(|field| {
                decode_png(&field.png_base64).map(|image| DecodedAtmosphericField {
                    host: snapshot.host.clone(),
                    location_generation: snapshot.location_generation,
                    viewport: snapshot.viewport.clone(),
                    rendered_at_ms: snapshot.rendered_at_ms,
                    kind: field.kind,
                    image,
                })
            })
            .collect();
    }

    /// Paint the selected admitted field. Returns false for unavailable data or
    /// any race between the decoded image and the current authority.
    #[must_use]
    pub fn paint_selected(&self, painter: &Painter, rect: Rect) -> bool {
        let Some(snapshot) = self.atmosphere.as_ref() else {
            return false;
        };
        let Some(field) = self
            .decoded_fields
            .iter()
            .find(|field| field.kind == self.field.contract_kind())
        else {
            return false;
        };
        if field.host != snapshot.host
            || field.location_generation != snapshot.location_generation
            || field.viewport != snapshot.viewport
            || field.rendered_at_ms != snapshot.rendered_at_ms
        {
            return false;
        }
        let key = egui::Id::new((
            "maps-weather-atmosphere",
            field.host.as_str(),
            field.location_generation,
            field.viewport.generation,
            field.rendered_at_ms,
            match field.kind {
                AtmosphericFieldKind::Temperature => 0_u8,
                AtmosphericFieldKind::Wind => 1,
                AtmosphericFieldKind::CloudCover => 2,
            },
        ));
        let texture = painter
            .ctx()
            .data_mut(|data| data.get_temp::<egui::TextureHandle>(key))
            .unwrap_or_else(|| {
                let texture = painter.ctx().load_texture(
                    format!(
                        "maps-weather-{}-{}-{}",
                        field.location_generation, field.viewport.generation, field.rendered_at_ms
                    ),
                    field.image.clone(),
                    TextureOptions::LINEAR,
                );
                painter
                    .ctx()
                    .data_mut(|data| data.insert_temp(key, texture.clone()));
                texture
            });
        painter.image(
            texture.id(),
            rect.intersect(painter.clip_rect()),
            Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE.gamma_multiply(0.68),
        );
        true
    }

    #[must_use]
    pub fn location_truth(&self) -> WeatherTruth {
        match self.location.as_ref().map(|snapshot| &snapshot.state) {
            Some(EffectiveLocationState::Available { .. }) => WeatherTruth::Fresh,
            Some(EffectiveLocationState::Stale { .. }) => WeatherTruth::Stale,
            Some(EffectiveLocationState::Unavailable { .. }) | None => WeatherTruth::Unavailable,
        }
    }

    #[must_use]
    pub fn location_label(&self) -> &str {
        match self.location.as_ref().map(|snapshot| &snapshot.state) {
            Some(EffectiveLocationState::Available { location })
            | Some(EffectiveLocationState::Stale { location, .. }) => &location.label,
            Some(EffectiveLocationState::Unavailable { .. }) | None => "Location unavailable",
        }
    }

    #[must_use]
    pub fn current_truth(&self) -> WeatherTruth {
        availability_truth(self.current.as_ref().map(|snapshot| &snapshot.availability))
    }

    #[must_use]
    pub fn forecast_truth(&self) -> WeatherTruth {
        availability_truth(
            self.forecast
                .as_ref()
                .map(|snapshot| &snapshot.availability),
        )
    }

    #[must_use]
    pub fn atmosphere_truth(&self) -> WeatherTruth {
        let Some(snapshot) = &self.atmosphere else {
            return WeatherTruth::Unavailable;
        };
        let truth = availability_truth(Some(&snapshot.availability));
        if truth != WeatherTruth::Unavailable
            && snapshot
                .fields
                .iter()
                .any(|field| field.kind == self.field.contract_kind())
        {
            truth
        } else {
            WeatherTruth::Unavailable
        }
    }

    #[must_use]
    pub fn selected_viewport(&self) -> Option<&AtmosphericViewport> {
        self.atmosphere.as_ref().map(|snapshot| &snapshot.viewport)
    }

    #[must_use]
    pub fn current_summary(&self) -> String {
        let Some(conditions) = self
            .current
            .as_ref()
            .and_then(|snapshot| snapshot.conditions.as_ref())
        else {
            return "Current conditions unavailable".to_string();
        };
        let condition = conditions
            .provider_text
            .as_deref()
            .unwrap_or("Conditions reported");
        conditions.temperature.map_or_else(
            || condition.to_string(),
            |temperature| format!("{condition} · {}", format_temperature(temperature)),
        )
    }

    #[must_use]
    pub fn visible_days(&self) -> &[LocalDaySummary] {
        let count = self.range.day_count();
        let Some(forecast) = &self.forecast else {
            return &[];
        };
        &forecast.daily[..forecast.daily.len().min(count)]
    }

    /// Stable, de-duplicated credits from the projections currently rendered.
    #[must_use]
    pub fn attribution(&self) -> String {
        let mut labels = Vec::new();
        for attribution in self
            .current
            .iter()
            .flat_map(|snapshot| &snapshot.attributions)
            .chain(
                self.forecast
                    .iter()
                    .flat_map(|snapshot| &snapshot.attributions),
            )
            .chain(
                self.atmosphere
                    .iter()
                    .flat_map(|snapshot| &snapshot.attributions),
            )
        {
            if !labels.iter().any(|label| label == &attribution.label) {
                labels.push(attribution.label.clone());
            }
        }
        if labels.is_empty() {
            "Weather source unavailable".to_string()
        } else {
            labels.join(" · ")
        }
    }
}

fn interactive_tile(
    latitude: f64,
    longitude: f64,
    zoom: f32,
    pan: [f32; 2],
) -> Option<(u8, u32, u32)> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !zoom.is_finite()
        || pan.iter().any(|value| !value.is_finite())
        || !(-85.051_128_78..=85.051_128_78).contains(&latitude)
        || !(-180.0..180.0).contains(&longitude)
    {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let tile_zoom = (zoom.round() as i32).clamp(2, 12) as u8;
    let n = f64::from(1_u32 << tile_zoom);
    let lat = latitude.to_radians();
    let center_x = (longitude + 180.0) / 360.0 * n;
    let center_y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    let scale = (f64::from(zoom) - f64::from(tile_zoom)).exp2();
    let tile_px = f64::from(ATMOSPHERIC_FIELD_EDGE) * scale;
    let x = (center_x - f64::from(pan[0].clamp(-600.0, 600.0)) / tile_px).floor();
    let y = (center_y - f64::from(pan[1].clamp(-600.0, 600.0)) / tile_px).floor();
    if !x.is_finite() || !y.is_finite() || y < 0.0 || y >= n {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((tile_zoom, (x.rem_euclid(n)) as u32, y as u32))
}

fn decode_png(value: &str) -> Option<egui::ColorImage> {
    if value.len() > MAX_DECODED_PNG_BYTES.saturating_mul(4).saturating_div(3) + 4 {
        return None;
    }
    let png = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    if png.len() > MAX_DECODED_PNG_BYTES || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(u32::from(ATMOSPHERIC_FIELD_EDGE));
    limits.max_image_height = Some(u32::from(ATMOSPHERIC_FIELD_EDGE));
    limits.max_alloc = Some(4 * u64::from(ATMOSPHERIC_FIELD_EDGE).pow(2));
    reader.limits(limits);
    let image = reader.decode().ok()?.to_rgba8();
    if image.dimensions()
        != (
            u32::from(ATMOSPHERIC_FIELD_EDGE),
            u32::from(ATMOSPHERIC_FIELD_EDGE),
        )
    {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [usize::from(ATMOSPHERIC_FIELD_EDGE); 2],
        image.as_raw(),
    ))
}

#[must_use]
pub fn format_temperature(value: Temperature) -> String {
    let unit = match value.unit {
        TemperatureUnit::Celsius => "°C",
        TemperatureUnit::Fahrenheit => "°F",
    };
    format!("{:.0}{unit}", value.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::location::{
        EffectiveLocationProvenance, EffectiveWeatherLocation, WeatherCoverage,
        WeatherLocationMode, WEATHER_LOCATION_SCHEMA_VERSION,
    };
    use mackes_mesh_types::nws_alert::GeoPoint;

    fn location(host: &str, generation: u64) -> EffectiveLocationSnapshot {
        EffectiveLocationSnapshot {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            host: host.to_string(),
            generation,
            mode: WeatherLocationMode::Manual,
            produced_at_ms: 1,
            state: EffectiveLocationState::Available {
                location: EffectiveWeatherLocation {
                    label: "Boston, MA".to_string(),
                    point: GeoPoint {
                        latitude: 42.36,
                        longitude: -71.06,
                    },
                    time_zone: "America/New_York".to_string(),
                    coverage: WeatherCoverage::NwsUnitedStates,
                    provenance: EffectiveLocationProvenance::ManualVerifiedPlace {
                        place_id: "boston".to_string(),
                    },
                    source_observed_at_ms: None,
                },
            },
        }
    }

    #[test]
    fn generation_change_retracts_old_weather_children() {
        let mut state = WeatherUiState::default();
        let mut current: CurrentWeatherSnapshot = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "host": "rig-1", "location_generation": 4,
            "producer_at_ms": 1, "fetched_at_ms": 1,
            "availability": {"state": "unavailable", "reason": "observation_unavailable"},
            "attributions": []
        }))
        .unwrap();
        state.fold(
            "rig-1",
            Some(location("rig-1", 4)),
            Some(current.clone()),
            None,
            None,
            None,
        );
        assert_eq!(state.current_truth(), WeatherTruth::Unavailable);
        current.location_generation = 4;
        state.fold(
            "rig-1",
            Some(location("rig-1", 5)),
            Some(current),
            None,
            None,
            None,
        );
        assert_eq!(state.current_summary(), "Current conditions unavailable");
        assert_eq!(state.location_label(), "Boston, MA");
    }

    #[test]
    fn selectors_are_closed_and_exclusive() {
        let mut state = WeatherUiState::default();
        state.field = WeatherField::Wind;
        state.range = WeatherRange::FiveDay;
        assert_eq!(state.field, WeatherField::Wind);
        assert_eq!(state.range, WeatherRange::FiveDay);
        assert_eq!(WeatherField::ALL.len(), 3);
        assert_eq!(WeatherRange::ALL.len(), 4);
    }

    #[test]
    fn interactive_viewport_is_latest_wins_and_tile_bounded() {
        let mut state = WeatherUiState::default();
        state.fold("rig-1", Some(location("rig-1", 7)), None, None, None, None);
        state.queue_interactive_viewport("rig-1", 13.0, [0.0, 0.0], 10);
        let first = state.pending_viewport().cloned().expect("first action");
        assert_eq!(first.expected_location_generation, 7);
        assert_eq!(first.viewport.zoom, 12);
        assert_eq!(first.viewport.pixel_width, ATMOSPHERIC_FIELD_EDGE);

        state.queue_interactive_viewport("rig-1", 13.0, [0.0, 0.0], 11);
        assert_eq!(state.pending_viewport(), Some(&first));

        state.queue_interactive_viewport("rig-1", 13.0, [600.0, 0.0], 12);
        let latest = state.pending_viewport().expect("latest action");
        assert!(latest.viewport.generation > first.viewport.generation);
        assert_ne!(latest.viewport.x, first.viewport.x);
    }

    #[test]
    fn manual_search_is_explicit_bounded_and_latest_wins() {
        let mut state = WeatherUiState::default();
        state.manual_search_query = "Boston".into();
        assert!(state.take_pending_manual_search().is_none());
        state.submit_manual_search();
        assert_eq!(
            state.manual_search_status(),
            ManualLocationSearchStatus::Pending
        );
        assert_eq!(
            state.take_pending_manual_search().as_deref(),
            Some("Boston")
        );

        state.manual_search_query = "Providence".into();
        state.complete_manual_search(
            "Boston",
            crate::geocode::WeatherGeocodeOutcome {
                results: vec![],
                note: Some("No verified weather locations found".into()),
            },
        );
        assert_eq!(
            state.manual_search_status(),
            ManualLocationSearchStatus::Pending
        );

        state.manual_search_query = "x".repeat(MAX_MANUAL_SEARCH_BYTES + 1);
        state.submit_manual_search();
        assert!(state.take_pending_manual_search().is_none());
        assert_eq!(
            state.manual_search_status(),
            ManualLocationSearchStatus::NoResults
        );
    }

    #[test]
    fn selected_offline_result_queues_exact_typed_manual_action() {
        let mut state = WeatherUiState::default();
        state.fold("rig-1", Some(location("rig-1", 7)), None, None, None, None);
        state.manual_search_query = "Boston".into();
        state.complete_manual_search(
            "Boston",
            crate::geocode::WeatherGeocodeOutcome {
                results: vec![crate::geocode::WeatherGeoResult {
                    place_id: "offline-boston-ma".into(),
                    label: "Boston, MA".into(),
                    latitude: 42.36,
                    longitude: -71.06,
                    time_zone: "America/New_York".into(),
                    coverage: WeatherCoverage::NwsUnitedStates,
                }],
                note: None,
            },
        );
        state.select_manual_result(0, 50);
        let action = state.pending_location_action().expect("manual action");
        assert_eq!(action.expected_generation, 7);
        assert_eq!(action.mode, WeatherLocationMode::Manual);
        let place = action.manual_place.as_ref().expect("verified place");
        assert_eq!(place.place_id, "offline-boston-ma");
        assert_eq!(place.time_zone, "America/New_York");
        action.validate_at(50).expect("typed request");
    }

    #[test]
    fn location_generation_change_revokes_pending_manual_action() {
        let mut state = WeatherUiState::default();
        state.fold("rig-1", Some(location("rig-1", 7)), None, None, None, None);
        state.manual_search_query = "Boston".into();
        state.complete_manual_search(
            "Boston",
            crate::geocode::WeatherGeocodeOutcome {
                results: vec![crate::geocode::WeatherGeoResult {
                    place_id: "offline-boston-ma".into(),
                    label: "Boston, MA".into(),
                    latitude: 42.36,
                    longitude: -71.06,
                    time_zone: "America/New_York".into(),
                    coverage: WeatherCoverage::NwsUnitedStates,
                }],
                note: None,
            },
        );
        state.select_manual_result(0, 50);
        assert!(state.pending_location_action().is_some());

        state.fold("rig-1", Some(location("rig-1", 8)), None, None, None, None);

        assert!(state.pending_location_action().is_none());
    }

    #[test]
    fn admitted_png_paints_and_viewport_race_is_refused() {
        use mackes_mesh_types::weather::{AtmosphericFieldImage, WeatherMapViewportSource};

        let png = {
            let image = image::RgbaImage::from_pixel(
                u32::from(ATMOSPHERIC_FIELD_EDGE),
                u32::from(ATMOSPHERIC_FIELD_EDGE),
                image::Rgba([240, 20, 10, 180]),
            );
            let mut bytes = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .expect("png");
            base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
        };
        let viewport = AtmosphericViewport {
            generation: 8,
            zoom: 7,
            x: 37,
            y: 47,
            pixel_width: ATMOSPHERIC_FIELD_EDGE,
            pixel_height: ATMOSPHERIC_FIELD_EDGE,
        };
        let viewport_state = WeatherMapViewportState {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: "rig-1".into(),
            location_generation: 7,
            viewport: viewport.clone(),
            source: WeatherMapViewportSource::MapsAction,
            admitted_at_ms: 10,
        };
        let atmosphere = AtmosphericMapSnapshot {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: "rig-1".into(),
            location_generation: 7,
            location_point: GeoPoint {
                latitude: 42.36,
                longitude: -71.06,
            },
            viewport: viewport.clone(),
            rendered_at_ms: 10,
            fetched_at_ms: 10,
            availability: WeatherAvailability::Fresh,
            fields: vec![AtmosphericFieldImage {
                kind: AtmosphericFieldKind::Temperature,
                provider_service_path: AtmosphericFieldKind::Temperature
                    .nowcoast_product()
                    .0
                    .into(),
                provider_layer_name: AtmosphericFieldKind::Temperature
                    .nowcoast_product()
                    .1
                    .into(),
                pixel_width: ATMOSPHERIC_FIELD_EDGE,
                pixel_height: ATMOSPHERIC_FIELD_EDGE,
                png_base64: png,
            }],
            gaps: vec![],
            attributions: vec![],
        };
        let mut state = WeatherUiState::default();
        state.fold(
            "rig-1",
            Some(location("rig-1", 7)),
            None,
            None,
            Some(atmosphere.clone()),
            Some(viewport_state),
        );
        let ctx = egui::Context::default();
        let mut painted = false;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                painted = state.paint_selected(ui.painter(), ui.max_rect());
            });
        });
        assert!(painted);
        assert!(output.shapes.iter().any(|shape| {
            matches!(shape.shape, egui::Shape::Mesh(ref mesh) if mesh.texture_id != egui::TextureId::default())
        }));

        let mut raced_viewport = viewport;
        raced_viewport.generation = 9;
        state.fold(
            "rig-1",
            Some(location("rig-1", 7)),
            None,
            None,
            Some(atmosphere),
            Some(WeatherMapViewportState {
                viewport: raced_viewport,
                ..state.viewport.clone().expect("viewport")
            }),
        );
        assert_eq!(state.atmosphere_truth(), WeatherTruth::Unavailable);
    }
}

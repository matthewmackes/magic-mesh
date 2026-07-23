//! Render-agnostic state for the Maps & Location workspace.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

/// Poll cadence for retained live Bus mirrors (PERF-5). The shell calls
/// [`MapsLocationSurface::refresh_from_bus`] every frame (~60 Hz); re-reading the
/// Bus spool off disk that often is pure waste for latest-wins mirrors. Gating
/// to 2 Hz keeps the vehicle fold live and cheaply picks up slower overlay feeds.
const BUS_REFRESH: Duration = Duration::from_millis(500);

/// Maximum age of a vehicle-gateway mirror that may still drive instrument or
/// safety state. The MG90 adapter normally publishes at ~1 Hz; five missed
/// updates is long enough to tolerate jitter without letting a retained Bus
/// snapshot impersonate a live vehicle indefinitely.
const VEHICLE_TELEMETRY_STALE_AFTER_S: f32 = 5.0;

/// The simulator-active gap note seeded by [`MapsLocationSurface::simulated`].
///
/// Named as a constant (not an inline literal) so the live-mirror fold in
/// [`MapsLocationSurface::refresh_from_vehicle`] can retract exactly this note
/// once a real `state/vehicle/<node>` mirror exists, without a fragile string
/// duplicated across two call sites.
const SIMULATED_MG90_GAP_NOTE: &str =
    "Real MG90 discovery/auth/status adapters are skeleton seams; simulator is active.";

/// The production-constructor gap note for "no vehicle-gateway mirror yet".
/// Seeded by [`MapsLocationSurface::live`] and retracted by
/// [`MapsLocationSurface::refresh_from_vehicle`] the moment a real
/// `state/vehicle/<node>` mirror folds in.
const AWAITING_MIRROR_GAP_NOTE: &str = "Awaiting live `state/vehicle` mirror — no MG90 vehicle-gateway adapter has published for this node yet.";

/// Workspace tabs in the order requested by the product directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkspaceTab {
    /// Default in-motion navigation view.
    #[default]
    Drive,
    /// Airspace — real-time wardriving radar (WiFi / cell / BT around the vehicle).
    Airspace,
    /// Full map exploration and layer control.
    Map,
    /// Trips, routes, saved places, replay, and export.
    RoutesTrips,
    /// Ford 2020 Police Interceptor vehicle telemetry.
    Vehicle,
    /// MG90 WAN/cellular/connectivity view.
    Connectivity,
    /// Serial recovery, GPIO, USB, Ethernet, CAN/OBD.
    DevicesIo,
    /// Primary-source selection and health diagnostics.
    LocationSources,
    /// First-time direct-Ethernet setup and reset guardrails.
    Mg90Setup,
    /// Native MG90 setting descriptors and pending changes.
    Mg90Settings,
    /// Firmware lifecycle and serial recovery workflows.
    FirmwareRecovery,
}

impl WorkspaceTab {
    /// All tabs in stable product order (primary surfaces first, then the
    /// sections nested under **Advanced**). Kept as the flat 11-section list so
    /// "every section reachable" iteration still covers the whole workspace.
    /// The former Simulator control tab was removed from production entirely
    /// (WL-UX-007/S1, operator directive 2026-07-22): production only shows
    /// MG90-mirror data, so there is no simulator surface to control.
    pub const ALL: [Self; 11] = [
        Self::Drive,
        Self::Airspace,
        Self::Map,
        Self::RoutesTrips,
        Self::Vehicle,
        Self::Connectivity,
        Self::DevicesIo,
        Self::LocationSources,
        Self::Mg90Setup,
        Self::Mg90Settings,
        Self::FirmwareRecovery,
    ];

    /// Primary top-level surfaces — the clean first-level nav a driver reaches
    /// for. The technical/config sections are nested under [`Self::ADVANCED`]
    /// (progressive disclosure), so the main rail stays uncluttered.
    pub const PRIMARY: [Self; 4] = [Self::Drive, Self::Airspace, Self::Map, Self::RoutesTrips];

    /// Technical / configuration sections nested under the top-level **Advanced**
    /// entry. Off the primary nav by design — reached by expanding Advanced.
    pub const ADVANCED: [Self; 7] = [
        Self::Vehicle,
        Self::Connectivity,
        Self::DevicesIo,
        Self::LocationSources,
        Self::Mg90Setup,
        Self::Mg90Settings,
        Self::FirmwareRecovery,
    ];

    /// Whether this section lives under the top-level **Advanced** menu (the
    /// technical / configuration sections) rather than the primary nav.
    #[must_use]
    pub const fn is_advanced(self) -> bool {
        matches!(
            self,
            Self::Vehicle
                | Self::Connectivity
                | Self::DevicesIo
                | Self::LocationSources
                | Self::Mg90Setup
                | Self::Mg90Settings
                | Self::FirmwareRecovery
        )
    }

    /// Human label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Drive => "Drive",
            Self::Airspace => "Airspace",
            Self::Map => "Map",
            Self::RoutesTrips => "Routes & Trips",
            Self::Vehicle => "Vehicle",
            Self::Connectivity => "Connectivity",
            Self::DevicesIo => "Devices & I/O",
            Self::LocationSources => "Location Sources",
            Self::Mg90Setup => "MG90 Setup",
            Self::Mg90Settings => "MG90 Settings",
            Self::FirmwareRecovery => "Firmware & Recovery",
        }
    }
}

/// Whole workspace state.
#[derive(Debug, Clone)]
pub struct MapsLocationSurface {
    /// Selected workspace tab.
    pub active: WorkspaceTab,
    /// Airspace — the real-time wardriving radar state (WiFi/cell/BT around the
    /// vehicle). Live-only; simulated feed until the MG90 `airspace` worker lands.
    pub airspace: crate::airspace::AirspaceState,
    /// Whether the pre-drive route-preview screen is showing over the Drive tab.
    pub route_preview: bool,
    /// Whether the "Where to?" destination-search screen is showing over Drive.
    pub destination_search: bool,
    /// Live text in the "Where to?" search field (P1). Drives the offline
    /// geocoder; empty until the driver types.
    pub destination_query: String,
    /// Ranked offline-geocoder results for the current [`Self::destination_query`].
    pub geocode_results: Vec<crate::geocode::GeoResult>,
    /// A human note shown in place of results (no gazetteer / no match).
    pub geocode_note: Option<String>,
    /// The query [`Self::geocode_results`] were computed for, so the geocoder
    /// only re-runs when the typed text actually changes.
    last_geocode_query: Option<String>,
    /// One-shot: focus the search field on the frame the search screen opens.
    request_search_focus: bool,
    /// Whether the "You have arrived" screen is showing over the Drive tab.
    pub arrived: bool,
    /// Whether turn-by-turn guidance is in the off-route "Recalculating…" state.
    pub off_route: bool,
    /// Whether the top-level **Advanced** submenu is expanded in the nav rail.
    /// Pure nav-view state (progressive disclosure): the technical/config
    /// sections stay hidden until the driver opens Advanced. The submenu also
    /// auto-reveals whenever the active section is one of the Advanced sections,
    /// so the selected item is always visible — see [`Self::advanced_open`].
    pub advanced_expanded: bool,
    /// Whether the (test-only) simulator fixture seeded this surface. Always
    /// `false` on the production [`Self::live`] path — only the cfg-gated
    /// [`Self::simulated`] fixture sets it, and the un-hideable Car-Mode
    /// "SIMULATED DATA" ribbon keys off it, so the ribbon is unreachable in
    /// production by construction. PLATFORM-INTERFACES P8/Q33.
    pub simulator_enabled: bool,
    /// Current map view model.
    pub map: MapViewState,
    /// Offline map package manager state.
    pub offline_maps: OfflineMapManagerState,
    /// Routing/search abstraction state.
    pub local_navigation: LocalNavigationState,
    /// MG90 local-management state.
    pub mg90: Mg90State,
    /// Location-source manager.
    pub locations: LocationManager,
    /// Trip recorder and export model.
    pub trips: TripRecorderState,
    /// Dead-zone recorder/overlay state.
    pub dead_zones: DeadZoneState,
    /// Vehicle profile and telemetry.
    pub vehicle: VehicleState,
    /// GPIO/CAN/USB/serial device state.
    pub devices: DeviceIoState,
    /// Firmware lifecycle model.
    pub firmware: FirmwareWorkflow,
    /// Encrypted vault readiness model.
    pub vault: EncryptedVaultState,
    /// Known real-hardware gaps for this vertical slice.
    pub real_hardware_gaps: Vec<String>,
    /// Throttle stamp for the per-frame `refresh_from_bus` Bus reads (PERF-5).
    /// Covers both the vehicle and overlay latest-wins mirrors.
    last_bus_poll: Option<Instant>,
}

impl MapsLocationSurface {
    /// Build the PRODUCTION workspace state — honest-empty everywhere.
    ///
    /// PLATFORM-INTERFACES P8/Q33 (operator directive 2026-07-22, WL-UX-007/S1):
    /// production shows ONLY MG90-mirror-derived data (`state/vehicle/<node>`
    /// via [`Self::refresh_from_bus`] / [`Self::refresh_from_vehicle`]) or real
    /// on-disk artifacts (the deployed `MBTiles` basemap/gazetteer). Every layer
    /// with no live source renders an honest empty state — never a fabricated
    /// contact, telemetry reading, trip, dead zone, device, firmware check, or
    /// destination:
    ///
    /// * locations — the MG90 GNSS primary is armed but source-less (`No fix`,
    ///   null coordinates, disconnected) until a mirror folds in;
    /// * airspace — zero contacts ("no scanner feed", not fake radar) until the
    ///   MG90 airspace worker lands;
    /// * vehicle — absent telemetry whose confidence label never claims live;
    /// * trips / dead zones — empty, with the real recording seams
    ///   ([`Self::record_dead_zone_from_current_status`]) still functional;
    /// * offline maps — whatever region bundle is REALLY installed on disk
    ///   (probed fail-soft), else the honest not-installed state;
    /// * mg90 / devices / firmware — offline-until-mirror, nothing discovered.
    #[must_use]
    pub fn live() -> Self {
        let offline_maps = OfflineMapManagerState::live();
        let map = MapViewState::live(!offline_maps.installed_regions.is_empty());
        Self {
            active: WorkspaceTab::Drive,
            airspace: crate::airspace::AirspaceState::live(),
            route_preview: false,
            destination_search: false,
            destination_query: String::new(),
            geocode_results: Vec::new(),
            geocode_note: None,
            last_geocode_query: None,
            request_search_focus: false,
            arrived: false,
            off_route: false,
            advanced_expanded: false,
            simulator_enabled: false,
            map,
            offline_maps,
            local_navigation: LocalNavigationState::live(),
            mg90: Mg90State::live(),
            locations: LocationManager::live(),
            trips: TripRecorderState::live(),
            dead_zones: DeadZoneState::live(),
            vehicle: VehicleState::awaiting_gateway(),
            devices: DeviceIoState::live(),
            firmware: FirmwareWorkflow::live(),
            vault: EncryptedVaultState::ready_for_local_admin(),
            real_hardware_gaps: vec![
                AWAITING_MIRROR_GAP_NOTE.to_string(),
                "MG90 airspace worker (WiFi survey / cell scan / BT) is not wired; Airspace has no scanner feed."
                    .to_string(),
                "Valhalla offline routing is not wired; chosen destinations preview as straight-line only."
                    .to_string(),
                "CAN/OBD, GPIO, serial, firmware upload, and factory reset workflows are UI/model complete but not wired to hardware."
                    .to_string(),
            ],
            last_bus_poll: None,
        }
    }

    /// Build the first vertical slice in simulator mode — TEST FIXTURE ONLY.
    ///
    /// Compiled only for this crate's own tests and for dependents that opt in
    /// via the dev-only `sim-fixture` feature (`mde-shell-egui` enables it from
    /// `[dev-dependencies]`). No production build carries this constructor, so
    /// no production path can boot on the fabricated seed. PLATFORM-INTERFACES
    /// P8/Q33; operator directive 2026-07-22.
    #[cfg(any(test, feature = "sim-fixture"))]
    #[must_use]
    pub fn simulated() -> Self {
        Self {
            active: WorkspaceTab::Drive,
            airspace: crate::airspace::AirspaceState::simulated(),
            route_preview: false,
            destination_search: false,
            destination_query: String::new(),
            geocode_results: Vec::new(),
            geocode_note: None,
            last_geocode_query: None,
            request_search_focus: false,
            arrived: false,
            off_route: false,
            advanced_expanded: false,
            simulator_enabled: true,
            map: MapViewState::simulated(),
            offline_maps: OfflineMapManagerState::simulated_default(),
            local_navigation: LocalNavigationState::simulated(),
            mg90: Mg90State::simulated(),
            locations: LocationManager::simulated(),
            trips: TripRecorderState::simulated(),
            dead_zones: DeadZoneState::simulated(),
            vehicle: VehicleState::ford_interceptor_2020(),
            devices: DeviceIoState::simulated(),
            firmware: FirmwareWorkflow::simulated(),
            vault: EncryptedVaultState::ready_for_local_admin(),
            real_hardware_gaps: vec![
                SIMULATED_MG90_GAP_NOTE.to_string(),
                "Valhalla and Nominatim are represented as local-only backend contracts; no live daemon is launched by this slice."
                    .to_string(),
                "gpsd, CAN/OBD, GPIO, serial, firmware upload, and factory reset workflows are UI/model complete but not wired to hardware."
                    .to_string(),
                "Traffic, weather, and satellite providers expose graceful unavailable states until configured."
                    .to_string(),
            ],
            last_bus_poll: None,
        }
    }

    /// One-line warning when the selected primary source is unhealthy.
    #[must_use]
    pub fn primary_location_warning(&self) -> Option<String> {
        self.locations.primary_warning()
    }

    /// Open the "Where to?" destination-search screen over the Drive tab.
    ///
    /// Clears any terminal arrival state so search is always reachable, matching
    /// the Google-Maps / Waze "search from anywhere" entry affordance.
    pub fn open_destination_search(&mut self) {
        self.active = WorkspaceTab::Drive;
        self.arrived = false;
        self.destination_search = true;
        // Fresh slate: clear the field + any stale results, and request focus so
        // the shell auto-raises the OSK (Car/Tablet) onto an empty search box.
        self.destination_query.clear();
        self.geocode_results.clear();
        self.geocode_note = None;
        self.last_geocode_query = None;
        self.request_search_focus = true;
    }

    /// Take the one-shot "focus the search field" request — `true` exactly once
    /// per [`Self::open_destination_search`], so focus is requested on the frame
    /// the screen opens without stealing focus on every later frame.
    pub fn take_search_focus(&mut self) -> bool {
        std::mem::take(&mut self.request_search_focus)
    }

    /// Re-run the offline geocoder when the query text changed since last frame.
    ///
    /// Fail-soft: a query shorter than two chars clears the list; a missing
    /// gazetteer or read error yields an empty list plus an explanatory note
    /// (never a panic). Cheap to call every frame — it early-returns unless the
    /// trimmed text actually changed.
    pub fn refresh_geocode(&mut self) {
        const MIN_QUERY_CHARS: usize = 2;
        const RESULT_LIMIT: usize = 24;
        let query = self.destination_query.trim().to_string();
        if self.last_geocode_query.as_deref() == Some(query.as_str()) {
            return;
        }
        self.last_geocode_query = Some(query.clone());
        if query.chars().count() < MIN_QUERY_CHARS {
            self.geocode_results.clear();
            self.geocode_note = None;
            return;
        }
        let outcome = crate::geocode::geocode(&query, RESULT_LIMIT);
        self.geocode_results = outcome.results;
        self.geocode_note = outcome.note;
    }

    /// Choose a live geocoder result: promote it to a real pinned destination at
    /// the head of the list and advance to route preview. Out-of-range → no-op.
    pub fn choose_geo_result(&mut self, idx: usize) {
        let Some(result) = self.geocode_results.get(idx).cloned() else {
            return;
        };
        let dest = {
            let from = self.locations.primary_sample();
            Destination::from_geo(&result, from)
        };
        self.local_navigation.destinations.insert(0, dest);
        self.choose_destination(0);
    }

    /// Choose a destination from the search screen and advance to route preview.
    ///
    /// Out-of-range indices leave the selected destination unchanged but still
    /// advance to preview, so the call is always crash-safe.
    pub fn choose_destination(&mut self, idx: usize) {
        self.local_navigation.select_destination(idx);
        self.destination_search = false;
        self.arrived = false;
        self.off_route = false;
        self.route_preview = true;
    }

    /// Begin turn-by-turn guidance on the selected route option — the target of
    /// the route-preview **Start** button. Applies the chosen option to the active
    /// route, leaves the preview, and marks guidance as running so the Drive HUD
    /// paints the maneuver banner / ETA sheet / speed sign (not the idle prompt).
    ///
    /// Honest no-op when no route options exist: without a routing engine there
    /// is no route, so guidance never starts on a fabricated empty maneuver
    /// banner (PLATFORM-INTERFACES Q33).
    pub fn start_navigation(&mut self) {
        if self.local_navigation.route_options.is_empty() {
            return;
        }
        let selected = self.local_navigation.selected_route;
        self.local_navigation.apply_route_option(selected);
        self.local_navigation.navigating = true;
        self.route_preview = false;
        self.arrived = false;
    }

    /// Enter the "You have arrived" screen (the arrival path). TEST FIXTURE
    /// ONLY — production has no arrival-detection source yet, so no production
    /// UI reaches this transition.
    #[cfg(any(test, feature = "sim-fixture"))]
    pub fn simulate_arrival(&mut self) {
        self.active = WorkspaceTab::Drive;
        self.destination_search = false;
        self.route_preview = false;
        self.off_route = false;
        self.arrived = true;
        // Arrival ends guidance: the Drive HUD returns to its idle state after.
        self.local_navigation.navigating = false;
    }

    /// Leave any navigation-flow overlay and return to the idle Drive HUD.
    pub fn end_navigation(&mut self) {
        self.arrived = false;
        self.destination_search = false;
        self.route_preview = false;
        self.off_route = false;
        self.local_navigation.navigating = false;
    }

    /// Toggle the off-route / recalculating guidance state (dev toggle).
    pub fn toggle_off_route(&mut self) {
        self.off_route = !self.off_route;
    }

    /// Whether the top-level **Advanced** submenu is revealed in the nav rail —
    /// either because the driver expanded it, or because the active section
    /// already lives under Advanced (so the selected item is always visible).
    #[must_use]
    pub const fn advanced_open(&self) -> bool {
        self.advanced_expanded || self.active.is_advanced()
    }

    /// Toggle the top-level **Advanced** submenu's expanded state.
    pub const fn toggle_advanced(&mut self) {
        self.advanced_expanded = !self.advanced_expanded;
    }

    /// Compute whether the current state can provide offline turn-by-turn use.
    #[must_use]
    pub fn offline_navigation_status(&self) -> OfflineNavigationStatus {
        OfflineNavigationStatus::from_surface(self)
    }

    /// Simulator scenario: the selected source stops updating. TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    pub fn simulate_stale_primary_location(&mut self) {
        if let Some(source) = self
            .locations
            .sources
            .iter_mut()
            .find(|source| source.kind == self.locations.primary)
        {
            source.status = SourceStatus::Stale;
            source.sample.update_age_s = 18.0;
            source
                .diagnostics
                .insert("scenario".to_string(), "stale primary source".to_string());
        }
    }

    /// Simulator scenario: no usable offline map bundle is loaded. TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    pub fn simulate_no_offline_maps(&mut self) {
        self.offline_maps.used_gb = 0.0;
        self.offline_maps.installed_regions.clear();
        self.offline_maps
            .available_regions
            .push("Default state/province region queued for reinstall".to_string());
    }

    /// Restore simulator data to an offline-navigation-ready state. TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    pub fn simulate_ready_offline_navigation(&mut self) {
        self.locations = LocationManager::simulated();
        self.offline_maps = OfflineMapManagerState::simulated_default();
        self.mg90.setup_step = SetupStep::Ready;
        self.mg90.authenticated = true;
    }

    /// Simulator scenario: the active cellular path degrades enough to record a
    /// route dead zone. TEST FIXTURE ONLY — the underlying
    /// [`Self::record_dead_zone_from_current_status`] seam is the REAL recorder
    /// and stays production-compiled.
    #[cfg(any(test, feature = "sim-fixture"))]
    pub fn simulate_cellular_dead_zone(&mut self) -> bool {
        self.mg90.status.cellular_a.signal_dbm = -116;
        self.mg90.status.cellular_a.healthy = false;
        self.mg90.status.packet_loss_percent = 14.0;
        self.mg90.status.latency_ms = 260;
        self.mg90.status.link_quality = "dead-zone candidate".to_string();
        self.record_dead_zone_from_current_status()
    }

    /// Append a dead-zone record from the current primary location and active MG90 link.
    ///
    /// Returns `false` when the current cellular state is good or no location/link is available.
    pub fn record_dead_zone_from_current_status(&mut self) -> bool {
        let severity = self.mg90.status.dead_zone_severity();
        if severity == DeadZoneSeverity::Good {
            return false;
        }
        let Some(sample) = self.locations.primary_sample().cloned() else {
            return false;
        };
        // A dead zone pins to the map at the current position; without a real GNSS
        // lock there is no honest coordinate to record (a null-island `0, 0` point
        // would be fabricated).
        if !sample.has_fix() {
            return false;
        }
        let Some(link) = self.mg90.status.active_cellular_link() else {
            return false;
        };

        let outage_duration_s = match severity {
            DeadZoneSeverity::Good => 0,
            DeadZoneSeverity::Weak => 5,
            DeadZoneSeverity::Degraded => 18,
            DeadZoneSeverity::Outage => 30,
        };
        self.dead_zones.zones.push(DeadZoneRecord {
            position: format!("{:.4}, {:.4}", sample.latitude, sample.longitude),
            selected_wan: self.mg90.status.active_wan.clone(),
            carrier: link.carrier.clone(),
            technology: link.technology.clone(),
            signal_dbm: link.signal_dbm,
            packet_loss_percent: self.mg90.status.packet_loss_percent,
            latency_ms: self.mg90.status.latency_ms,
            outage_duration_s,
            severity,
        });
        self.dead_zones.refresh_route_risk();
        true
    }

    /// True when the motion guard should warn before dangerous changes.
    #[must_use]
    pub fn moving(&self) -> bool {
        self.locations
            .primary_sample()
            .is_some_and(|sample| !sample.stale() && sample.moving())
            || (self.vehicle.telemetry.is_live() && self.vehicle.telemetry.moving)
            || self.mg90.ignition_on
    }

    /// Build the setting-change execution plan used by MG90 Settings.
    #[must_use]
    pub fn setting_change_plan(&self, setting_id: &str) -> Option<SettingChangePlan> {
        let setting = self
            .mg90
            .settings
            .iter()
            .find(|descriptor| descriptor.id == setting_id)?;
        Some(SettingChangePlan::for_setting(setting, self.moving()))
    }

    /// Fold a live `state/vehicle/<node>` mirror onto this surface's LIVE models
    /// — the real MG90 (a.k.a. "Rolling Node") behind the beautiful HUD.
    ///
    /// `WanStatus` -> `Mg90Status` (+ both `CellularLink`s); the `GpsFix` ->
    /// the **MG90 GNSS** `LocationSource`'s `LocationSample`; `VehicleTelem` ->
    /// `VehicleTelemetry`. This is an additive fold over the simulator seed,
    /// never a full replacement: fields the wire type doesn't carry
    /// (`Mg90Status::data_transferred`, the MG90 setup/settings/backup seams,
    /// …) are left as-is so a live gateway with a partial mirror still shows the
    /// cockpit's other seams honestly.
    ///
    /// The key behaviour: when the mirror is `online`, the MG90 GNSS source is
    /// made **primary** and the "Simulator" chip drops — so the Drive HUD's
    /// GNSS source and the Location Sources tab read MG90/GNSS, not Simulator.
    /// `has_fix` is respected (no lock still shows the HUD's "Acquiring GPS"
    /// state), but the source LABEL is MG90 the moment a live gateway exists.
    pub fn refresh_from_vehicle(&mut self, v: &mackes_mesh_types::vehicle::VehicleState) {
        let mirror_age_s = mirror_age_s(v.published_at_ms);

        // WanStatus -> Mg90Status.
        let status = &mut self.mg90.status;
        status.active_wan = v.wan.active_wan.clone();
        status.cellular_a = cellular_link_from_wire(&v.wan.cellular_a);
        status.cellular_b = cellular_link_from_wire(&v.wan.cellular_b);
        status.wifi_state = v.wan.wifi_state.clone();
        status.ethernet_state = v.wan.ethernet_state.clone();
        status.vpn_state = v.wan.vpn_state.clone();
        status.failover_events = v.wan.failover_events;
        status.latency_ms = v.wan.latency_ms;
        status.packet_loss_percent = v.wan.packet_loss_percent;
        status.link_quality = v.wan.link_quality.clone();

        // Auto-select MG90 GNSS as the primary location source once a live
        // gateway exists, and retire the global "Simulator" indicator. Assigned
        // directly (not via `set_primary`, which gates on health) so a no-lock
        // gateway still switches the SOURCE LABEL to MG90 while the HUD's own
        // `has_fix` gate keeps showing "Acquiring GPS".
        if v.online {
            self.locations.primary = LocationSourceKind::Mg90Gnss;
            self.simulator_enabled = false;
        }

        // GpsFix -> the MG90 GNSS source's LocationSample (found by kind, so the
        // live fold lands on MG90 regardless of the current primary). HDOP has
        // no exact meters conversion; ~5 m per HDOP unit is the commonly-cited
        // civilian-GNSS UERE estimate — an honest approximation, not precision.
        if let Some(source) = self
            .locations
            .sources
            .iter_mut()
            .find(|s| s.kind == LocationSourceKind::Mg90Gnss)
        {
            let gps = &v.gps;
            source.sample = LocationSample {
                fix_type: gps.fix_type.clone(),
                latitude: gps.latitude,
                longitude: gps.longitude,
                accuracy_m: gps.hdop * 5.0,
                speed_mph: gps.speed_mph,
                heading_deg: gps.heading_deg,
                altitude_m: gps.altitude_m,
                satellites: Some(gps.satellites),
                update_rate_hz: gps.update_rate_hz,
                // A retained mirror cannot make an old GNSS sample look young:
                // the effective sample age is at least the mirror's wall-clock
                // age, even when the gateway's last payload said `age_s = 0`.
                update_age_s: gps.age_s.max(mirror_age_s),
            };
            if v.online {
                source.status = SourceStatus::Connected;
                source.diagnostics.insert(
                    "mode".to_string(),
                    format!(
                        "live vehicle-gateway mirror ({} {})",
                        v.model, v.mgos_version
                    ),
                );
            }
        }

        // VehicleTelem -> VehicleTelemetry. Optional OBD fields (fuel/odometer/
        // coolant) preserve the prior value when the mirror reports `None` — an
        // unsupported PID is not the same as a zero reading.
        let telem = &v.telem;
        let telemetry = &mut self.vehicle.telemetry;
        telemetry.speed_mph = telem.speed_mph;
        telemetry.rpm = telem.rpm;
        if let Some(coolant_c) = telem.coolant_c {
            telemetry.coolant_c = coolant_c;
        }
        telemetry.battery_v = telem.battery_v;
        if telem.fuel_percent.is_some() {
            telemetry.fuel_percent = telem.fuel_percent;
        }
        telemetry.dtc_count = telem.dtc_count;
        telemetry.ignition_on = telem.ignition_on;
        telemetry.moving = telem.moving;
        if telem.odometer_mi.is_some() {
            telemetry.odometer_mi = telem.odometer_mi;
        }
        telemetry.runtime_min = telem.runtime_min;
        telemetry.internal_temp_c = Some(telem.internal_temp_c);
        telemetry.confidence = if v.online {
            format!(
                "live vehicle-gateway mirror ({} {})",
                v.model, v.mgos_version
            )
        } else {
            "vehicle-gateway mirror reports the adapter offline".to_string()
        };
        telemetry.last_update_age_s = mirror_age_s;

        // Retract the seed's "no mirror yet" / "simulator is active" gaps now a
        // live mirror exists and fold the adapter's own honest gap report in
        // their place.
        self.real_hardware_gaps
            .retain(|g| g != SIMULATED_MG90_GAP_NOTE && g != AWAITING_MIRROR_GAP_NOTE);
        if v.gaps.is_empty() {
            let note = format!(
                "Live vehicle-gateway mirror active for node `{}` ({} {}).",
                v.host, v.model, v.mgos_version
            );
            if !self.real_hardware_gaps.contains(&note) {
                self.real_hardware_gaps.insert(0, note);
            }
        } else {
            for gap in &v.gaps {
                let note = format!("Vehicle-gateway adapter gap: {gap}");
                if !self.real_hardware_gaps.contains(&note) {
                    self.real_hardware_gaps.push(note);
                }
            }
        }
    }

    /// Read retained vehicle + overlay mirrors off the Bus (fail-soft, honest
    /// off-mesh no-op) and fold them into the cockpit.
    ///
    /// When no mirror is retained yet — no spool, no adapter worker running, or
    /// the topic is simply empty — this leaves the simulated seed exactly as it
    /// was, `real_hardware_gaps` note included: the honest offline fallback, not
    /// an error. Opens the store per call (no cached `Connection`) rather than
    /// reaching into the shell's crate-private `BusReader` seam, matching that
    /// seam's own fail-soft idiom for a cross-crate caller.
    pub fn refresh_from_bus(&mut self, node: &str) {
        // PERF-5: the shell calls this every frame (~60 Hz); gate the Bus spool
        // read + decode to ~2 Hz. The gateway refreshes the mirror ~1 Hz, so a more
        // frequent read is pure waste — the cockpit keeps drawing the last fold
        // between polls (latest-wins, byte-identical result).
        if self
            .last_bus_poll
            .is_some_and(|t| t.elapsed() < BUS_REFRESH)
        {
            return;
        }
        self.last_bus_poll = Some(Instant::now());
        // Open the SQLite-backed Bus spool once per refresh.  The ten overlay
        // lanes plus the vehicle mirror are all latest-wins reads; opening a
        // separate Persist handle for each lane multiplied connection/schema
        // work eleven-fold on every poll (and was especially visible on the
        // smallest workstation instances).  A single borrowed handle keeps
        // the fail-soft behavior while making the fold genuinely cheap.
        let Some(root) = mde_bus::client_data_dir() else {
            return;
        };
        let Ok(persist) = mde_bus::persist::Persist::open(root) else {
            return;
        };

        self.refresh_from_persist(&persist, node);
    }

    /// Fold one already-open Bus spool into the cockpit.
    ///
    /// This is kept separate from [`Self::refresh_from_bus`] so the live
    /// mirror contract can be exercised against a deterministic SQLite spool
    /// without mutating process-global `MDE_BUS_ROOT` or depending on a
    /// workstation's real daemon. Production still reaches this seam through
    /// the fail-soft, cadence-gated method above.
    fn refresh_from_persist(&mut self, persist: &mde_bus::persist::Persist, node: &str) {
        if let Some(mirror) = read_vehicle_mirror(persist, node) {
            self.refresh_from_vehicle(&mirror);
        }
        if let Some(snapshot) = read_earthquake_mirror(persist, node) {
            self.refresh_from_earthquakes(snapshot);
        }
        if let Some(snapshot) = read_nws_alert_mirror(persist, node) {
            self.refresh_from_nws_alerts(snapshot);
        }
        if let Some(snapshot) = read_aircraft_mirror(persist, node) {
            self.refresh_from_aircraft(snapshot);
        }
        if let Some(snapshot) = read_transit_mirror(persist, node) {
            self.refresh_from_transit(snapshot);
        }
        if let Some(snapshot) = read_nws_forecast_mirror(persist, node) {
            self.refresh_from_nws_forecast(snapshot);
        }
        if let Some(snapshot) = read_caltrans_camera_mirror(persist, node) {
            self.refresh_from_caltrans_cameras(snapshot);
        }
        if let Some(snapshot) = read_iem_radar_mirror(persist, node) {
            self.refresh_from_iem_radar(snapshot);
        }
        if let Some(snapshot) = read_wildfire_mirror(persist, node) {
            self.refresh_from_wildfire(snapshot);
        }
        if let Some(snapshot) = read_traffic_mirror(persist, node) {
            self.refresh_from_traffic(snapshot);
        }
        if let Some(snapshot) = read_air_quality_mirror(persist, node) {
            self.refresh_from_air_quality(snapshot);
        }
    }

    /// Fold a complete USGS snapshot. Whole-snapshot replacement is deliberate:
    /// it retracts upstream-deleted events and applies revisions by id/update.
    pub fn refresh_from_earthquakes(
        &mut self,
        snapshot: mackes_mesh_types::earthquake::EarthquakeSnapshot,
    ) {
        self.map.earthquakes.fold(snapshot);
    }

    /// Fold a complete point-scoped NWS active-alert set.
    pub fn refresh_from_nws_alerts(
        &mut self,
        snapshot: mackes_mesh_types::nws_alert::NwsAlertSnapshot,
    ) {
        self.map.nws_alerts.fold(snapshot);
    }

    /// Fold a complete vehicle-scoped adsb.lol low-altitude aircraft set.
    pub fn refresh_from_aircraft(
        &mut self,
        snapshot: mackes_mesh_types::aircraft::AircraftSnapshot,
    ) {
        self.map.aircraft.fold(snapshot);
    }

    /// Fold a complete vehicle-scoped MBTA GTFS-Realtime set.
    pub fn refresh_from_transit(&mut self, snapshot: mackes_mesh_types::transit::TransitSnapshot) {
        self.map.transit.fold(snapshot);
    }

    /// Fold a complete vehicle-scoped NWS hourly drive-ahead forecast.
    pub fn refresh_from_nws_forecast(
        &mut self,
        snapshot: mackes_mesh_types::nws_forecast::NwsForecastSnapshot,
    ) {
        self.map.nws_forecast.fold(snapshot);
    }

    /// Fold a complete vehicle-scoped Caltrans CWWP2 camera set.
    pub fn refresh_from_caltrans_cameras(
        &mut self,
        snapshot: mackes_mesh_types::caltrans_camera::CaltransCameraSnapshot,
    ) {
        self.map.caltrans_cameras.fold(snapshot);
    }

    /// Fold a complete local-tile IEM/NWS NEXRAD animation.
    pub fn refresh_from_iem_radar(
        &mut self,
        snapshot: mackes_mesh_types::iem_radar::IemRadarSnapshot,
    ) {
        self.map.iem_radar.fold(snapshot);
    }

    /// Fold a complete vehicle-centred NIFC WFIGS perimeter set.
    pub fn refresh_from_wildfire(
        &mut self,
        snapshot: mackes_mesh_types::wildfire::WildfireSnapshot,
    ) {
        self.map.wildfire.fold(snapshot);
    }

    /// Fold a complete vehicle-centred NCDOT current-event set.
    pub fn refresh_from_traffic(&mut self, snapshot: mackes_mesh_types::traffic::TrafficSnapshot) {
        self.map.traffic_events.fold(snapshot);
    }

    /// Fold complete credential/configuration state plus current AirNow stations.
    pub fn refresh_from_air_quality(
        &mut self,
        snapshot: mackes_mesh_types::air_quality::AirQualitySnapshot,
    ) {
        self.map.air_quality.fold(snapshot);
    }

    /// The Auto Mode home's **Vehicle**-tile glance line: a live telematics
    /// summary when the MG90 gateway is the primary location source, else `None`
    /// (the home then shows a plain descriptor, never a simulated reading). Speed
    /// while moving, otherwise the gateway's live battery voltage — the two facts
    /// a driver glances for.
    #[must_use]
    pub fn vehicle_glance(&self) -> Option<String> {
        if self.locations.primary != LocationSourceKind::Mg90Gnss
            || !self.vehicle.telemetry.is_live()
        {
            return None;
        }
        let t = &self.vehicle.telemetry;
        if t.moving && t.speed_mph > 0.5 {
            Some(format!("{:.0} mph", t.speed_mph))
        } else if t.battery_v > 0.1 {
            Some(format!("MG90 · {:.1} V", t.battery_v))
        } else {
            Some("MG90 linked".to_string())
        }
    }

    /// Open the cockpit directly on its **Vehicle** telematics tab — the target of
    /// the Auto Mode home's Vehicle tile, so it lands on telematics rather than the
    /// default Drive HUD.
    pub fn focus_vehicle_tab(&mut self) {
        self.active = WorkspaceTab::Vehicle;
    }

    /// Open the cockpit on the **Airspace** wardriving radar (and arm scanning) —
    /// the target of the Airspace keyboard action + feature-bar item.
    pub fn focus_airspace_tab(&mut self) {
        self.active = WorkspaceTab::Airspace;
        self.airspace.active = true;
    }
}

/// `mackes_mesh_types::vehicle::CellLink` -> the cockpit's `CellularLink` —
/// same six fields, different crate.
fn cellular_link_from_wire(link: &mackes_mesh_types::vehicle::CellLink) -> CellularLink {
    CellularLink {
        sim_state: link.sim_state.clone(),
        carrier: link.carrier.clone(),
        signal_dbm: link.signal_dbm,
        technology: link.technology.clone(),
        wan_ip: link.wan_ip.clone(),
        healthy: link.healthy,
    }
}

/// Wall-clock age of a `published_at_ms` mirror stamp, seconds. Falls back to
/// `0.0` if the system clock is somehow before the stamp — never panics.
fn mirror_age_s(published_at_ms: i64) -> f32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(published_at_ms, |d| {
            i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
        });
    ((now_ms - published_at_ms).max(0) as f32) / 1000.0
}

/// Open the Bus fail-soft and decode the newest `state/vehicle/<node>` mirror
/// body — the same "resolve `client_data_dir`, open `Persist` fail-soft,
/// newest row, `serde_json` decode" seam the shell's own per-host readers use,
/// embedded locally since that seam is crate-private to `mde-shell-egui`.
fn read_vehicle_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::vehicle::VehicleState> {
    let topic = mackes_mesh_types::vehicle::vehicle_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained keyless-USGS overlay snapshot, fail-soft when the adapter
/// is disabled, the Bus is absent, or the latest payload is malformed.
fn read_earthquake_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::earthquake::EarthquakeSnapshot> {
    let topic = mackes_mesh_types::earthquake::earthquake_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained NWS active-alert snapshot, fail-soft when the opt-in
/// adapter has no fresh vehicle fix or has not published yet.
fn read_nws_alert_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::nws_alert::NwsAlertSnapshot> {
    let topic = mackes_mesh_types::nws_alert::nws_alert_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained adsb.lol aircraft snapshot, fail-soft when the adapter
/// is disabled, lacks a qualified vehicle fix, or has not published yet.
fn read_aircraft_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::aircraft::AircraftSnapshot> {
    let topic = mackes_mesh_types::aircraft::aircraft_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained MBTA transit snapshot, fail-soft when disabled/absent.
fn read_transit_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::transit::TransitSnapshot> {
    let topic = mackes_mesh_types::transit::transit_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained NWS hourly snapshot, including explicit no-fix state.
fn read_nws_forecast_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::nws_forecast::NwsForecastSnapshot> {
    let topic = mackes_mesh_types::nws_forecast::nws_forecast_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained Caltrans camera snapshot, fail-soft when disabled/absent.
fn read_caltrans_camera_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::caltrans_camera::CaltransCameraSnapshot> {
    let topic = mackes_mesh_types::caltrans_camera::caltrans_camera_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained IEM/NWS NEXRAD snapshot, fail-soft when disabled/absent.
fn read_iem_radar_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::iem_radar::IemRadarSnapshot> {
    let topic = mackes_mesh_types::iem_radar::iem_radar_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained keyless NIFC WFIGS perimeter snapshot, fail-soft when
/// the opt-in adapter is disabled, has no fresh fix, or has not published yet.
fn read_wildfire_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::wildfire::WildfireSnapshot> {
    let topic = mackes_mesh_types::wildfire::wildfire_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode the retained keyless NCDOT traffic snapshot, fail-soft outside North
/// Carolina, while disabled, or before the first publish.
fn read_traffic_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::traffic::TrafficSnapshot> {
    let topic = mackes_mesh_types::traffic::traffic_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Decode retained AirNow state, including the explicit missing-key state.
fn read_air_quality_mirror(
    persist: &mde_bus::persist::Persist,
    node: &str,
) -> Option<mackes_mesh_types::air_quality::AirQualitySnapshot> {
    let topic = mackes_mesh_types::air_quality::air_quality_state_topic(node);
    read_latest_json(persist, &topic)
}

/// Read and decode one retained latest-wins payload from an already-open Bus
/// spool.  A missing row, absent body, SQL failure, or malformed JSON all
/// intentionally collapse to `None`: one unhealthy feed must not prevent the
/// remaining map layers from folding during the same poll.
fn read_latest_json<T: DeserializeOwned>(
    persist: &mde_bus::persist::Persist,
    topic: &str,
) -> Option<T> {
    let body = persist.read_latest(topic).ok().flatten()?.body?;
    serde_json::from_str(&body).ok()
}

impl Default for MapsLocationSurface {
    fn default() -> Self {
        Self::live()
    }
}

/// Coarse readiness level for the offline navigation core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineNavigationReadiness {
    /// Essential offline routing inputs are present.
    Ready,
    /// Offline routing can run, but an operator-facing warning is active.
    Degraded,
    /// Offline routing should not claim turn-by-turn readiness.
    Blocked,
}

impl OfflineNavigationReadiness {
    /// Human label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Degraded => "Degraded",
            Self::Blocked => "Blocked",
        }
    }
}

/// Render-agnostic status for native offline navigation.
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineNavigationStatus {
    /// Coarse readiness.
    pub readiness: OfflineNavigationReadiness,
    /// Selected primary location source.
    pub primary_source: LocationSourceKind,
    /// Loaded offline region, if any.
    pub loaded_region: Option<String>,
    /// Coverage percentage for the loaded region.
    pub coverage_percent: Option<u8>,
    /// Used offline-map storage.
    pub used_gb: f32,
    /// Offline-map storage cap.
    pub cap_gb: u32,
    /// Hard blockers that prevent an honest offline-navigation-ready claim.
    pub blockers: Vec<String>,
    /// Warnings that still allow offline routing.
    pub warnings: Vec<String>,
    /// Informational notes for optional providers or simulator fixtures.
    pub notes: Vec<String>,
}

impl OfflineNavigationStatus {
    fn from_surface(surface: &MapsLocationSurface) -> Self {
        let mut blockers = Vec::new();
        let mut warnings = Vec::new();
        let mut notes = Vec::new();

        match surface.locations.primary_source() {
            Some(source) => {
                if source.status != SourceStatus::Connected {
                    blockers.push(format!(
                        "{} is {}.",
                        source.kind.label(),
                        source.status.label()
                    ));
                }
                if source.sample.stale() {
                    blockers.push(format!(
                        "{} update is stale at {:.1} s.",
                        source.kind.label(),
                        source.sample.update_age_s
                    ));
                } else if !source.sample.healthy() {
                    blockers.push(format!(
                        "{} accuracy is {:.1} m; route guidance requires <= 5.0 m.",
                        source.kind.label(),
                        source.sample.accuracy_m
                    ));
                }
            }
            None => blockers.push(format!(
                "Primary location source {} is missing.",
                surface.locations.primary.label()
            )),
        }

        if !blockers.is_empty() {
            let alternatives = surface.locations.healthy_alternatives();
            if !alternatives.is_empty() {
                let labels: Vec<&str> = alternatives.iter().map(|kind| kind.label()).collect();
                warnings.push(format!(
                    "Healthy equal peer available: {}; manual switch required because automatic failover is off.",
                    labels.join(", ")
                ));
            }
        }

        let loaded_region = surface.offline_maps.loaded_region();
        if let Some(region) = loaded_region {
            if region.coverage_percent < 100 {
                warnings.push(format!(
                    "{} offline coverage is {}%.",
                    region.name, region.coverage_percent
                ));
            }
        } else {
            blockers.push("No loaded offline map region is available.".to_string());
        }

        if surface.offline_maps.used_gb > surface.offline_maps.storage_cap_gb as f32 {
            blockers.push(format!(
                "Offline maps use {:.1} GB, above the {} GB cap.",
                surface.offline_maps.used_gb, surface.offline_maps.storage_cap_gb
            ));
        } else if surface.offline_maps.storage_ratio() >= 0.9 {
            warnings.push(format!(
                "Offline map storage is {:.0}% of the {} GB cap.",
                surface.offline_maps.storage_ratio() * 100.0,
                surface.offline_maps.storage_cap_gb
            ));
        }

        for provider in [
            &surface.offline_maps.map_provider,
            &surface.local_navigation.routing,
            &surface.local_navigation.geocoder,
        ] {
            if !provider.local_only_core || provider.graceful_unavailable {
                blockers.push(format!(
                    "{} is not ready for local-only offline use.",
                    provider.abstraction
                ));
            }
        }

        if surface.mg90.setup_step < SetupStep::OfflineMapsVerified {
            blockers.push(format!(
                "MG90 setup has not verified offline maps; current step is {}.",
                surface.mg90.setup_step.label()
            ));
        } else if surface.mg90.setup_step < SetupStep::Ready {
            warnings.push(format!(
                "MG90 setup is not fully complete; current step is {}.",
                surface.mg90.setup_step.label()
            ));
        }

        if !surface.mg90.authenticated {
            blockers.push("MG90 local management is not authenticated.".to_string());
        }

        for provider in [
            &surface.local_navigation.traffic,
            &surface.local_navigation.weather,
            &surface.local_navigation.satellite,
        ] {
            if provider.graceful_unavailable {
                notes.push(format!(
                    "{} is optional and degrades gracefully when no provider is configured.",
                    provider.abstraction
                ));
            }
        }

        if surface.simulator_enabled {
            notes.push(
                "Simulator fixture supplies route, source, and offline-map data without MG90 hardware."
                    .to_string(),
            );
        }

        let readiness = if blockers.is_empty() {
            if warnings.is_empty() {
                OfflineNavigationReadiness::Ready
            } else {
                OfflineNavigationReadiness::Degraded
            }
        } else {
            OfflineNavigationReadiness::Blocked
        };

        Self {
            readiness,
            primary_source: surface.locations.primary,
            loaded_region: loaded_region.map(|region| region.name.clone()),
            coverage_percent: loaded_region.map(|region| region.coverage_percent),
            used_gb: surface.offline_maps.used_gb,
            cap_gb: surface.offline_maps.storage_cap_gb,
            blockers,
            warnings,
            notes,
        }
    }

    /// Whether turn-by-turn offline routing may be claimed.
    #[must_use]
    pub fn can_claim_turn_by_turn(&self) -> bool {
        self.readiness != OfflineNavigationReadiness::Blocked
    }
}

/// Native map viewport state.
#[derive(Debug, Clone)]
pub struct MapViewState {
    /// Dark map styling enabled.
    pub dark_mode: bool,
    /// Simulated zoom level.
    pub zoom: f32,
    /// Simulated pan offset in egui points.
    pub pan: [f32; 2],
    /// Rotation in degrees.
    pub rotation_deg: f32,
    /// Pitch in degrees.
    pub pitch_deg: f32,
    /// Whether the route line is visible.
    pub route_visible: bool,
    /// Whether traffic overlay is visible.
    pub traffic_overlay: bool,
    /// Whether weather overlay is visible.
    pub weather_overlay: bool,
    /// Whether cellular dead-zone overlay is visible.
    pub dead_zone_overlay: bool,
    /// Whether GNSS quality overlay is visible.
    pub gnss_overlay: bool,
    /// Whether the ambient USGS earthquake layer is visible. Off by default.
    pub earthquake_overlay: bool,
    /// Latest normalized USGS earthquake snapshot.
    pub earthquakes: crate::earthquake::EarthquakeLayerState,
    /// Whether the safety-relevant NWS active-alert layer is visible.
    pub nws_alert_overlay: bool,
    /// Latest point-scoped NWS active-alert snapshot.
    pub nws_alerts: crate::nws_alert::NwsAlertLayerState,
    /// Whether the driver-relevant low-altitude aircraft layer is visible.
    pub aircraft_overlay: bool,
    /// Latest point-scoped adsb.lol aircraft snapshot and label preference.
    pub aircraft: crate::aircraft::AircraftLayerState,
    /// Whether nearby MBTA GTFS-Realtime vehicles are visible.
    pub transit_overlay: bool,
    /// Latest point-filtered MBTA vehicle set and label preference.
    pub transit: crate::transit::TransitLayerState,
    /// Whether NWS hourly current/drive-ahead guidance is visible.
    pub nws_forecast_overlay: bool,
    /// Latest current/drive-ahead NWS hourly samples.
    pub nws_forecast: crate::nws_forecast::NwsForecastLayerState,
    /// Whether nearby Caltrans CWWP2 traffic cameras are visible.
    pub caltrans_camera_overlay: bool,
    /// Latest vehicle-scoped Caltrans camera set and bounded current stills.
    pub caltrans_cameras: crate::caltrans_camera::CaltransCameraLayerState,
    /// Whether the safety-relevant IEM/NWS NEXRAD layer is visible.
    pub iem_radar_overlay: bool,
    /// Latest exact producer-timed local radar animation.
    pub iem_radar: crate::iem_radar::IemRadarLayerState,
    /// Whether the safety-relevant NIFC WFIGS wildfire perimeters are visible.
    pub wildfire_overlay: bool,
    /// Latest vehicle-centred current wildfire perimeter set.
    pub wildfire: crate::wildfire::WildfireLayerState,
    /// Whether the regional NCDOT TIMS traffic-event layer is visible.
    pub traffic_event_overlay: bool,
    /// Latest vehicle-centred current NCDOT event set.
    pub traffic_events: crate::traffic::TrafficLayerState,
    /// Whether the ambient AirNow station AQI layer is visible. Off by default.
    pub air_quality_overlay: bool,
    /// Latest AirNow credential/configuration state and nearby station set.
    pub air_quality: crate::air_quality::AirQualityLayerState,
    /// Attribution string shown on every map view.
    pub attribution: String,
}

impl MapViewState {
    /// The production map viewport (dark, region-zoom, no pan) over the REAL
    /// `MBTiles` basemap loader path. `region_installed` keys the attribution
    /// line: OSM credit only when OSM-derived tiles are actually on disk, else
    /// the honest "no offline map data installed".
    #[must_use]
    pub fn live(region_installed: bool) -> Self {
        Self {
            dark_mode: true,
            zoom: 13.0,
            pan: [0.0, 0.0],
            rotation_deg: 18.0,
            pitch_deg: 34.0,
            route_visible: true,
            // No live traffic/weather provider exists; these stay off (and the
            // scene never paints fabricated overlay geometry regardless).
            traffic_overlay: false,
            weather_overlay: false,
            dead_zone_overlay: true,
            gnss_overlay: true,
            earthquake_overlay: false,
            earthquakes: crate::earthquake::EarthquakeLayerState::default(),
            // Safety layers default on in Drive; no-data remains explicitly badged.
            nws_alert_overlay: true,
            nws_alerts: crate::nws_alert::NwsAlertLayerState::default(),
            aircraft_overlay: false,
            aircraft: crate::aircraft::AircraftLayerState::default(),
            transit_overlay: false,
            transit: crate::transit::TransitLayerState::default(),
            nws_forecast_overlay: false,
            nws_forecast: crate::nws_forecast::NwsForecastLayerState::default(),
            caltrans_camera_overlay: false,
            caltrans_cameras: crate::caltrans_camera::CaltransCameraLayerState::default(),
            iem_radar_overlay: true,
            iem_radar: crate::iem_radar::IemRadarLayerState::default(),
            wildfire_overlay: true,
            wildfire: crate::wildfire::WildfireLayerState::default(),
            traffic_event_overlay: false,
            traffic_events: crate::traffic::TrafficLayerState::default(),
            air_quality_overlay: false,
            air_quality: crate::air_quality::AirQualityLayerState::default(),
            attribution: if region_installed {
                "OpenStreetMap contributors | local offline package".to_string()
            } else {
                "no offline map data installed".to_string()
            },
        }
    }

    /// The simulator-fixture viewport seed. TEST FIXTURE ONLY — public so the
    /// `basemap` projection tests can build a viewport without the whole surface.
    #[cfg(any(test, feature = "sim-fixture"))]
    #[must_use]
    pub fn simulated() -> Self {
        Self {
            dark_mode: true,
            zoom: 13.0,
            pan: [0.0, 0.0],
            rotation_deg: 18.0,
            pitch_deg: 34.0,
            route_visible: true,
            traffic_overlay: true,
            weather_overlay: true,
            dead_zone_overlay: true,
            gnss_overlay: true,
            earthquake_overlay: false,
            earthquakes: crate::earthquake::EarthquakeLayerState::default(),
            nws_alert_overlay: true,
            nws_alerts: crate::nws_alert::NwsAlertLayerState::default(),
            aircraft_overlay: false,
            aircraft: crate::aircraft::AircraftLayerState::default(),
            transit_overlay: false,
            transit: crate::transit::TransitLayerState::default(),
            nws_forecast_overlay: false,
            nws_forecast: crate::nws_forecast::NwsForecastLayerState::default(),
            caltrans_camera_overlay: false,
            caltrans_cameras: crate::caltrans_camera::CaltransCameraLayerState::default(),
            iem_radar_overlay: true,
            iem_radar: crate::iem_radar::IemRadarLayerState::default(),
            wildfire_overlay: true,
            wildfire: crate::wildfire::WildfireLayerState::default(),
            traffic_event_overlay: false,
            traffic_events: crate::traffic::TrafficLayerState::default(),
            air_quality_overlay: false,
            air_quality: crate::air_quality::AirQualityLayerState::default(),
            attribution: "OpenStreetMap contributors | local offline package | simulated route"
                .to_string(),
        }
    }

    /// Attribution with active live overlays appended. Each provider credit is
    /// tied to its layer toggle even before data arrives, so a no-data/stale
    /// state is never mistaken for an unattributed alternate source.
    #[must_use]
    pub fn attribution_line(&self) -> String {
        let mut attribution = self.attribution.clone();
        if self.earthquake_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::earthquake::EarthquakeLayerState::attribution());
        }
        if self.nws_alert_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::nws_alert::NwsAlertLayerState::attribution());
        }
        if self.aircraft_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::aircraft::AircraftLayerState::attribution());
        }
        if self.transit_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::transit::TransitLayerState::attribution());
        }
        if self.nws_forecast_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::nws_forecast::NwsForecastLayerState::attribution());
        }
        if self.caltrans_camera_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::caltrans_camera::CaltransCameraLayerState::attribution());
        }
        if self.iem_radar_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::iem_radar::IemRadarLayerState::attribution());
        }
        if self.wildfire_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::wildfire::WildfireLayerState::attribution());
        }
        if self.traffic_event_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::traffic::TrafficLayerState::attribution());
        }
        if self.air_quality_overlay {
            attribution.push_str(" | ");
            attribution.push_str(crate::air_quality::AirQualityLayerState::attribution());
        }
        attribution
    }
}

/// Offline map manager first-slice state.
#[derive(Debug, Clone)]
pub struct OfflineMapManagerState {
    /// Default state/province-level region label.
    pub default_region: String,
    /// Storage cap in GB.
    pub storage_cap_gb: u32,
    /// Used storage in GB.
    pub used_gb: f32,
    /// Installed regions.
    pub installed_regions: Vec<OfflineMapRegion>,
    /// Pending/downloadable regions.
    pub available_regions: Vec<String>,
    /// OpenStreetMap-derived provider contract.
    pub map_provider: ProviderContract,
}

impl OfflineMapManagerState {
    /// The production offline-map manager: reflect the region bundle that is
    /// REALLY installed on disk (the same `<maps root>/<region>/*.mbtiles`
    /// layout the basemap loader paints from), else the honest not-installed
    /// state. Never fabricates a region, size, or queued download.
    fn live() -> Self {
        Self::from_installed(crate::basemap::region_dir())
    }

    /// Build the manager over an optionally-installed region directory (split
    /// from [`Self::live`] so tests can exercise both branches without touching
    /// the process environment).
    fn from_installed(region: Option<std::path::PathBuf>) -> Self {
        let mut state = Self {
            default_region: String::new(),
            storage_cap_gb: 25,
            used_gb: 0.0,
            installed_regions: Vec::new(),
            available_regions: Vec::new(),
            map_provider: ProviderContract {
                abstraction: "Map Provider API".to_string(),
                first_backend: "OpenStreetMap-derived data".to_string(),
                local_only_core: true,
                graceful_unavailable: false,
            },
        };
        if let Some(dir) = region {
            let name = dir.file_name().map_or_else(
                || "region".to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let size_gb = dir_size_gb(&dir);
            state.default_region.clone_from(&name);
            state.used_gb = size_gb;
            state.installed_regions.push(OfflineMapRegion {
                name,
                status: RegionStatus::Loaded,
                size_gb,
                // The bundle covers the whole of its own declared MBTiles
                // bounds — coverage here describes the installed package, not a
                // fabricated continental claim.
                coverage_percent: 100,
                updated: "installed offline bundle".to_string(),
            });
        }
        state
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated_default() -> Self {
        Self {
            default_region: "Default state/province region".to_string(),
            storage_cap_gb: 25,
            used_gb: 3.8,
            installed_regions: vec![OfflineMapRegion {
                name: "Default state/province region".to_string(),
                status: RegionStatus::Loaded,
                size_gb: 3.8,
                coverage_percent: 100,
                updated: "simulated offline bundle".to_string(),
            }],
            available_regions: vec![
                "Neighboring state/province".to_string(),
                "Cross-border corridor".to_string(),
            ],
            map_provider: ProviderContract {
                abstraction: "Map Provider API".to_string(),
                first_backend: "OpenStreetMap-derived data".to_string(),
                local_only_core: true,
                graceful_unavailable: false,
            },
        }
    }

    fn loaded_region(&self) -> Option<&OfflineMapRegion> {
        self.installed_regions
            .iter()
            .filter(|region| region.status == RegionStatus::Loaded)
            .max_by_key(|region| region.coverage_percent)
    }

    fn storage_ratio(&self) -> f32 {
        if self.storage_cap_gb == 0 {
            return 1.0;
        }
        self.used_gb / self.storage_cap_gb as f32
    }
}

/// Total size of the files directly inside `dir`, in GB. Fail-soft: unreadable
/// entries count as zero (an honest under-report, never a fabrication).
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // display value
fn dir_size_gb(dir: &std::path::Path) -> f32 {
    let bytes: u64 = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(std::fs::Metadata::is_file)
        .map(|m| m.len())
        .sum();
    (bytes as f64 / 1_073_741_824.0) as f32
}

/// Installed offline region.
#[derive(Debug, Clone)]
pub struct OfflineMapRegion {
    /// Region display name.
    pub name: String,
    /// Load/download status.
    pub status: RegionStatus,
    /// Package size.
    pub size_gb: f32,
    /// Coverage percentage.
    pub coverage_percent: u8,
    /// Last update label.
    pub updated: String,
}

/// Offline region state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionStatus {
    /// Loaded and usable for offline navigation.
    Loaded,
    /// Download queued when internet is available.
    Queued,
    /// Provider unavailable.
    Unavailable,
}

/// Provider/backend abstraction contract.
#[derive(Debug, Clone)]
pub struct ProviderContract {
    /// Abstraction seam name.
    pub abstraction: String,
    /// First backend selected by product directive.
    pub first_backend: String,
    /// Whether the core v1 path is local-only.
    pub local_only_core: bool,
    /// Whether the provider is gracefully unavailable.
    pub graceful_unavailable: bool,
}

/// Local routing/search state.
#[derive(Debug, Clone)]
pub struct LocalNavigationState {
    /// Routing abstraction.
    pub routing: ProviderContract,
    /// Geocoder abstraction.
    pub geocoder: ProviderContract,
    /// Traffic provider abstraction.
    pub traffic: ProviderContract,
    /// Weather provider abstraction.
    pub weather: ProviderContract,
    /// Satellite provider abstraction.
    pub satellite: ProviderContract,
    /// Active simulated route.
    pub active_route: RoutePlan,
    /// Recent/favorite destinations.
    pub destinations: Vec<Destination>,
    /// Selectable route options shown on the pre-drive route-preview screen.
    pub route_options: Vec<RouteOption>,
    /// Index of the currently selected route option.
    pub selected_route: usize,
    /// Index of the destination the preview / arrival screens summarize.
    pub selected_destination: usize,
    /// Whether turn-by-turn guidance to a chosen destination is actually running.
    ///
    /// `false` is the honest idle state — no destination picked, so the Drive HUD
    /// shows a calm "search to start" prompt instead of a fabricated maneuver
    /// banner / ETA sheet / traffic pills for a route the driver never chose. Set
    /// `true` the moment the operator taps **Start** on the route preview.
    pub navigating: bool,
}

impl LocalNavigationState {
    /// The production navigation state: real provider contracts, an unplanned
    /// route, and NO destinations beyond what live geocoding adds — the presets
    /// ("Home", "Precinct HQ", …) were fixture data and never ship.
    fn live() -> Self {
        Self {
            routing: ProviderContract {
                abstraction: "Routing API".to_string(),
                first_backend: "Valhalla".to_string(),
                local_only_core: true,
                // Not wired yet: reads as a blocker in the readiness model, so
                // turn-by-turn is never claimed on a route that cannot exist.
                graceful_unavailable: true,
            },
            geocoder: ProviderContract {
                abstraction: "Geocoder API".to_string(),
                // The REAL offline gazetteer (`geocode.rs`) — wired and local.
                first_backend: "offline gazetteer (FTS5)".to_string(),
                local_only_core: true,
                graceful_unavailable: false,
            },
            traffic: ProviderContract {
                abstraction: "Traffic API".to_string(),
                first_backend: "no provider configured".to_string(),
                local_only_core: false,
                graceful_unavailable: true,
            },
            weather: ProviderContract {
                abstraction: "Weather API".to_string(),
                first_backend: "no provider configured".to_string(),
                local_only_core: false,
                graceful_unavailable: true,
            },
            satellite: ProviderContract {
                abstraction: "Satellite API".to_string(),
                first_backend: "no provider configured".to_string(),
                local_only_core: false,
                graceful_unavailable: true,
            },
            active_route: RoutePlan::none(),
            destinations: Vec::new(),
            route_options: Vec::new(),
            selected_route: 0,
            selected_destination: 0,
            navigating: false,
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        Self {
            routing: ProviderContract {
                abstraction: "Routing API".to_string(),
                first_backend: "Valhalla".to_string(),
                local_only_core: true,
                graceful_unavailable: false,
            },
            geocoder: ProviderContract {
                abstraction: "Geocoder API".to_string(),
                first_backend: "Nominatim".to_string(),
                local_only_core: true,
                graceful_unavailable: false,
            },
            traffic: ProviderContract {
                abstraction: "Traffic API".to_string(),
                first_backend: "configured live traffic provider".to_string(),
                local_only_core: false,
                graceful_unavailable: true,
            },
            weather: ProviderContract {
                abstraction: "Weather API".to_string(),
                first_backend: "configured weather provider".to_string(),
                local_only_core: false,
                graceful_unavailable: true,
            },
            satellite: ProviderContract {
                abstraction: "Satellite API".to_string(),
                first_backend: "configured imagery provider".to_string(),
                local_only_core: false,
                graceful_unavailable: true,
            },
            active_route: RoutePlan {
                current_road: "US-30 W".to_string(),
                next_maneuver: "Keep right toward patrol staging".to_string(),
                distance_to_maneuver_mi: 0.4,
                eta: "14:32".to_string(),
                remaining_time_min: 18,
                remaining_distance_mi: 11.6,
                alternatives: 2,
                traffic_alert: "Slowdown +4 min ahead".to_string(),
                weather_alert: "Heavy rain intersects route in 9 mi".to_string(),
            },
            destinations: vec![
                Destination {
                    label: "Home".to_string(),
                    category: "home".to_string(),
                    distance_mi: 5.4,
                    address: "742 Ridgeview Terrace".to_string(),
                    lat: None,
                    lon: None,
                },
                Destination {
                    label: "Precinct HQ".to_string(),
                    category: "work".to_string(),
                    distance_mi: 3.2,
                    address: "1200 Public Safety Blvd".to_string(),
                    lat: None,
                    lon: None,
                },
                Destination {
                    label: "Hospital entrance".to_string(),
                    category: "recent".to_string(),
                    distance_mi: 8.7,
                    address: "500 Medical Center Dr, Emergency".to_string(),
                    lat: None,
                    lon: None,
                },
                Destination {
                    label: "Command post".to_string(),
                    category: "favorite".to_string(),
                    distance_mi: 14.1,
                    address: "US-30 W Mile 214, staging area".to_string(),
                    lat: None,
                    lon: None,
                },
                Destination {
                    label: "Motor pool fuel".to_string(),
                    category: "fuel".to_string(),
                    distance_mi: 2.1,
                    address: "88 Motor Pool Rd".to_string(),
                    lat: None,
                    lon: None,
                },
                Destination {
                    label: "Market St Diner".to_string(),
                    category: "food".to_string(),
                    distance_mi: 4.3,
                    address: "210 Market St".to_string(),
                    lat: None,
                    lon: None,
                },
                Destination {
                    label: "Union St Garage".to_string(),
                    category: "parking".to_string(),
                    distance_mi: 1.6,
                    address: "5th St & Union, Level 2".to_string(),
                    lat: None,
                    lon: None,
                },
            ],
            route_options: vec![
                RouteOption {
                    label: "Fastest".to_string(),
                    via: "US-30 W".to_string(),
                    eta: "14:32".to_string(),
                    remaining_time_min: 18,
                    remaining_distance_mi: 11.6,
                    traffic: RouteTraffic::Slow,
                },
                RouteOption {
                    label: "Less traffic".to_string(),
                    via: "PA-51 S".to_string(),
                    eta: "14:39".to_string(),
                    remaining_time_min: 25,
                    remaining_distance_mi: 13.2,
                    traffic: RouteTraffic::Clear,
                },
            ],
            selected_route: 0,
            selected_destination: 0,
            navigating: false,
        }
    }

    /// The destination the route-preview and arrival screens summarize.
    ///
    /// Falls back to the first destination when the selected index is out of
    /// range, so the summary is always populated (crash-safe).
    #[must_use]
    pub fn active_destination(&self) -> Option<&Destination> {
        self.destinations
            .get(self.selected_destination)
            .or_else(|| self.destinations.first())
    }

    /// Select a destination by index. Out-of-range indices are ignored, so the
    /// call is always crash-safe.
    pub fn select_destination(&mut self, idx: usize) {
        if idx < self.destinations.len() {
            self.selected_destination = idx;
        }
    }

    /// Index of the first destination whose category matches `category`, if any.
    #[must_use]
    pub fn destination_in_category(&self, category: &str) -> Option<usize> {
        self.destinations
            .iter()
            .position(|destination| destination.category.eq_ignore_ascii_case(category))
    }

    /// Apply a route option's summary onto the active route.
    ///
    /// Called when the operator taps an option on the route-preview screen.
    /// Out-of-range indices are ignored, so the call is always crash-safe.
    pub fn apply_route_option(&mut self, idx: usize) {
        let Some(option) = self.route_options.get(idx).cloned() else {
            return;
        };
        self.selected_route = idx;
        self.active_route.eta = option.eta;
        self.active_route.remaining_time_min = option.remaining_time_min;
        self.active_route.remaining_distance_mi = option.remaining_distance_mi;
        self.active_route.current_road = option.via;
        self.active_route.traffic_alert = match option.traffic {
            RouteTraffic::Clear => String::new(),
            RouteTraffic::Slow => "Slowdown +4 min ahead".to_string(),
            RouteTraffic::Heavy => "Heavy traffic ahead".to_string(),
        };
    }
}

impl RoutePlan {
    /// The honest "no route planned" state — every field empty/zero. The map
    /// scene paints no route ribbon and the HUD stays on the idle prompt until
    /// a real plan exists ([`Self::is_planned`]).
    #[must_use]
    pub fn none() -> Self {
        Self {
            current_road: String::new(),
            next_maneuver: String::new(),
            distance_to_maneuver_mi: 0.0,
            eta: String::new(),
            remaining_time_min: 0,
            remaining_distance_mi: 0.0,
            alternatives: 0,
            traffic_alert: String::new(),
            weather_alert: String::new(),
        }
    }

    /// Whether an actual route plan exists (vs. the empty [`Self::none`] state).
    /// Gates the map's route-ribbon paint so an unplanned surface never draws a
    /// fabricated route.
    #[must_use]
    pub fn is_planned(&self) -> bool {
        !self.current_road.trim().is_empty() || !self.eta.trim().is_empty()
    }
}

/// Active route summary.
#[derive(Debug, Clone)]
pub struct RoutePlan {
    /// Current road name.
    pub current_road: String,
    /// Next turn instruction.
    pub next_maneuver: String,
    /// Distance to next maneuver.
    pub distance_to_maneuver_mi: f32,
    /// ETA clock label.
    pub eta: String,
    /// Remaining minutes.
    pub remaining_time_min: u32,
    /// Remaining miles.
    pub remaining_distance_mi: f32,
    /// Number of alternate routes.
    pub alternatives: u8,
    /// Traffic alert strip text.
    pub traffic_alert: String,
    /// Weather alert strip text.
    pub weather_alert: String,
}

/// Saved/recent destination.
#[derive(Debug, Clone)]
pub struct Destination {
    /// Label.
    pub label: String,
    /// Category.
    pub category: String,
    /// Distance from current location.
    pub distance_mi: f32,
    /// Street address / locality line shown in the route-preview summary.
    pub address: String,
    /// Geographic pin latitude — present for live geocoder results, `None` for
    /// the preset/simulated rows (which have no real coordinate). A destination
    /// with a pin draws on the real basemap + gets a straight-line preview.
    pub lat: Option<f64>,
    /// Geographic pin longitude — see [`Self::lat`].
    pub lon: Option<f64>,
}

impl Destination {
    /// Build a destination from a live offline-geocoder result, computing the
    /// straight-line distance from the current fix when one is available.
    #[must_use]
    pub fn from_geo(result: &crate::geocode::GeoResult, from: Option<&LocationSample>) -> Self {
        let distance_mi = from.filter(|s| s.has_fix()).map_or(0.0, |s| {
            haversine_mi(s.latitude, s.longitude, result.lat, result.lon)
        });
        Self {
            label: result.title(),
            category: "search".to_string(),
            distance_mi,
            address: result.subtitle(),
            lat: Some(result.lat),
            lon: Some(result.lon),
        }
    }

    /// The geographic pin `(lat, lon)`, if this destination has one.
    #[must_use]
    pub fn geo(&self) -> Option<(f64, f64)> {
        match (self.lat, self.lon) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        }
    }
}

/// Great-circle distance between two WGS84 points, in miles (the straight-line
/// "as the crow flies" preview distance, not a routed distance).
#[must_use]
#[allow(clippy::cast_possible_truncation)] // f64 metres → f32 miles: display value
pub fn haversine_mi(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f32 {
    const EARTH_MI: f64 = 3958.7613;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    (EARTH_MI * c) as f32
}

/// Coarse traffic condition on a route option, shown as an OK/WARN/DANGER dot on
/// the route-preview cards (Waze/Google-Maps grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteTraffic {
    /// Light/clear traffic — green.
    Clear,
    /// Slower than usual — amber.
    Slow,
    /// Heavy/stopped traffic — red.
    Heavy,
}

impl RouteTraffic {
    /// Human label for the route-option traffic line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clear => "Light traffic",
            Self::Slow => "Slower than usual",
            Self::Heavy => "Heavy traffic",
        }
    }
}

/// One selectable route on the pre-drive route-preview screen. Alternates are
/// mocked from the active route so the preview has a "fastest / less-traffic"
/// choice even when the routing seam only returns a single plan.
#[derive(Debug, Clone)]
pub struct RouteOption {
    /// Short option label ("Fastest", "Less traffic").
    pub label: String,
    /// Primary road the option runs on ("US-30 W").
    pub via: String,
    /// Arrival clock label.
    pub eta: String,
    /// Total minutes for this option.
    pub remaining_time_min: u32,
    /// Total miles for this option.
    pub remaining_distance_mi: f32,
    /// Traffic condition dot.
    pub traffic: RouteTraffic,
}

/// MG90 model/status.
#[derive(Debug, Clone)]
pub struct Mg90State {
    /// Managed device count. v1 intentionally manages exactly one.
    pub managed_devices: u8,
    /// Direct Ethernet is the required management path.
    pub direct_ethernet_only: bool,
    /// Current setup wizard step.
    pub setup_step: SetupStep,
    /// Discovered hardware model.
    pub model: Mg90Model,
    /// Capability profile detected from model/MGOS.
    pub capabilities: Mg90Capabilities,
    /// Authentication state.
    pub authenticated: bool,
    /// Ignition/input signal state.
    pub ignition_on: bool,
    /// Factory reset workflow.
    pub reset: FactoryResetWorkflow,
    /// Native setting registry.
    pub settings: Vec<Mg90SettingDescriptor>,
    /// Versioned restore points.
    pub backups: Vec<BackupRecord>,
    /// Local status dashboard.
    pub status: Mg90Status,
}

impl Mg90State {
    /// The production MG90 state: offline-until-mirror. Nothing discovered, no
    /// fabricated capability profile, setting descriptors, or backups; the
    /// direct-Ethernet management contract and factory-reset guardrails are
    /// real config, not data, and stay.
    fn live() -> Self {
        Self {
            managed_devices: 0,
            direct_ethernet_only: true,
            setup_step: SetupStep::NotConnected,
            // Placeholder family until real discovery reads the model; the view
            // dashes the model label while `setup_step < Mg90Discovered`.
            model: Mg90Model::FiveG,
            capabilities: Mg90Capabilities {
                lte_a: false,
                five_g: false,
                mgos_version: String::new(),
                gnss: false,
                gpio: false,
                serial_recovery: false,
                firmware_management: false,
            },
            authenticated: false,
            ignition_on: false,
            reset: FactoryResetWorkflow::awaiting_backup(),
            settings: Vec::new(),
            backups: Vec::new(),
            status: Mg90Status::offline(),
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        let settings = sample_settings();
        Self {
            managed_devices: 1,
            direct_ethernet_only: true,
            setup_step: SetupStep::Ready,
            model: Mg90Model::FiveG,
            capabilities: Mg90Capabilities {
                lte_a: true,
                five_g: true,
                mgos_version: "MGOS simulated capability profile".to_string(),
                gnss: true,
                gpio: true,
                serial_recovery: true,
                firmware_management: true,
            },
            authenticated: true,
            ignition_on: true,
            reset: FactoryResetWorkflow::guarded(),
            settings,
            backups: vec![BackupRecord {
                id: "baseline-0001".to_string(),
                reason: "Baseline backup created before first local status verification"
                    .to_string(),
                encrypted: true,
                restore_point: true,
                created: "simulated now".to_string(),
            }],
            status: Mg90Status::simulated(),
        }
    }

    /// Advance the offline setup wizard in simulator mode. TEST FIXTURE ONLY —
    /// the production wizard advances only when real discovery/auth seams land.
    #[cfg(any(test, feature = "sim-fixture"))]
    pub fn advance_setup_simulated(&mut self) {
        self.setup_step = self.setup_step.next();
        if matches!(self.setup_step, SetupStep::Authenticated | SetupStep::Ready) {
            self.authenticated = true;
        }
    }
}

/// Supported MG90 hardware model families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mg90Model {
    /// MG90 LTE-A.
    LteA,
    /// MG90 5G.
    FiveG,
}

impl Mg90Model {
    /// Label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::LteA => "Sierra Wireless AirLink MG90 LTE-A",
            Self::FiveG => "Sierra Wireless AirLink MG90 5G",
        }
    }
}

/// Detected MG90 feature set.
#[derive(Debug, Clone)]
pub struct Mg90Capabilities {
    /// LTE-A support.
    pub lte_a: bool,
    /// 5G support.
    pub five_g: bool,
    /// Detected MGOS label.
    pub mgos_version: String,
    /// GNSS capability.
    pub gnss: bool,
    /// GPIO capability.
    pub gpio: bool,
    /// Serial recovery available.
    pub serial_recovery: bool,
    /// Firmware lifecycle supported.
    pub firmware_management: bool,
}

/// Setup wizard states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SetupStep {
    /// MG90 not connected.
    NotConnected,
    /// Ethernet link detected.
    EthernetDetected,
    /// MG90 discovered on direct Ethernet.
    Mg90Discovered,
    /// Credentials entered.
    CredentialsEntered,
    /// Authenticated.
    Authenticated,
    /// Baseline backup created.
    BaselineBackupCreated,
    /// Local status verified.
    LocalStatusVerified,
    /// GNSS verified.
    GnssVerified,
    /// Offline maps verified.
    OfflineMapsVerified,
    /// Routing verified.
    RoutingVerified,
    /// Ready.
    Ready,
}

impl SetupStep {
    /// All setup steps.
    pub const ALL: [Self; 11] = [
        Self::NotConnected,
        Self::EthernetDetected,
        Self::Mg90Discovered,
        Self::CredentialsEntered,
        Self::Authenticated,
        Self::BaselineBackupCreated,
        Self::LocalStatusVerified,
        Self::GnssVerified,
        Self::OfflineMapsVerified,
        Self::RoutingVerified,
        Self::Ready,
    ];

    /// Label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotConnected => "Not connected",
            Self::EthernetDetected => "Ethernet detected",
            Self::Mg90Discovered => "MG90 discovered",
            Self::CredentialsEntered => "Credentials entered",
            Self::Authenticated => "Authenticated",
            Self::BaselineBackupCreated => "Baseline backup created",
            Self::LocalStatusVerified => "Local status verified",
            Self::GnssVerified => "GNSS verified",
            Self::OfflineMapsVerified => "Offline maps verified",
            Self::RoutingVerified => "Routing verified",
            Self::Ready => "Ready",
        }
    }

    /// The wizard's next step — only the cfg-gated simulator advance uses this
    /// today; the real discovery/auth seams will drive it when they land.
    #[cfg(any(test, feature = "sim-fixture"))]
    const fn next(self) -> Self {
        match self {
            Self::NotConnected => Self::EthernetDetected,
            Self::EthernetDetected => Self::Mg90Discovered,
            Self::Mg90Discovered => Self::CredentialsEntered,
            Self::CredentialsEntered => Self::Authenticated,
            Self::Authenticated => Self::BaselineBackupCreated,
            Self::BaselineBackupCreated => Self::LocalStatusVerified,
            Self::LocalStatusVerified => Self::GnssVerified,
            Self::GnssVerified => Self::OfflineMapsVerified,
            Self::OfflineMapsVerified => Self::RoutingVerified,
            Self::RoutingVerified | Self::Ready => Self::Ready,
        }
    }
}

/// Local MG90 status dashboard.
#[derive(Debug, Clone)]
pub struct Mg90Status {
    /// Active WAN label.
    pub active_wan: String,
    /// Cellular A.
    pub cellular_a: CellularLink,
    /// Cellular B.
    pub cellular_b: CellularLink,
    /// Wi-Fi state.
    pub wifi_state: String,
    /// Ethernet state.
    pub ethernet_state: String,
    /// VPN state.
    pub vpn_state: String,
    /// Data transferred.
    pub data_transferred: String,
    /// Failover event count.
    pub failover_events: u32,
    /// Latency.
    pub latency_ms: u32,
    /// Packet loss.
    pub packet_loss_percent: f32,
    /// Link quality label.
    pub link_quality: String,
}

impl Mg90Status {
    /// The production "no gateway yet" status: every link/interface field empty
    /// or zero, both cellular links absent and unhealthy. The views dash empty
    /// strings and treat non-negative dBm as "no signal", so nothing here reads
    /// as a live uplink. [`MapsLocationSurface::refresh_from_vehicle`] overwrites
    /// this wholesale from the wire mirror.
    fn offline() -> Self {
        let absent_link = || CellularLink {
            sim_state: String::new(),
            carrier: String::new(),
            signal_dbm: 0,
            technology: String::new(),
            wan_ip: String::new(),
            healthy: false,
        };
        Self {
            active_wan: String::new(),
            cellular_a: absent_link(),
            cellular_b: absent_link(),
            wifi_state: String::new(),
            ethernet_state: String::new(),
            vpn_state: String::new(),
            data_transferred: String::new(),
            failover_events: 0,
            latency_ms: 0,
            packet_loss_percent: 0.0,
            link_quality: String::new(),
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        Self {
            active_wan: "Cellular A".to_string(),
            cellular_a: CellularLink {
                sim_state: "ready".to_string(),
                carrier: "FirstNet".to_string(),
                signal_dbm: -72,
                technology: "5G/LTE-A".to_string(),
                wan_ip: "100.92.14.8".to_string(),
                healthy: true,
            },
            cellular_b: CellularLink {
                sim_state: "standby".to_string(),
                carrier: "Carrier B".to_string(),
                signal_dbm: -94,
                technology: "LTE".to_string(),
                wan_ip: "not active".to_string(),
                healthy: false,
            },
            wifi_state: "disabled for management".to_string(),
            ethernet_state: "direct cable link up".to_string(),
            vpn_state: "local status unavailable".to_string(),
            data_transferred: "1.4 GB down / 220 MB up".to_string(),
            failover_events: 1,
            latency_ms: 42,
            packet_loss_percent: 0.3,
            link_quality: "good".to_string(),
        }
    }

    /// Active cellular link, when the selected WAN is cellular.
    #[must_use]
    pub fn active_cellular_link(&self) -> Option<&CellularLink> {
        match self.active_wan.as_str() {
            "Cellular A" => Some(&self.cellular_a),
            "Cellular B" => Some(&self.cellular_b),
            _ => None,
        }
    }

    /// Classify the current active link for route dead-zone recording.
    #[must_use]
    pub fn dead_zone_severity(&self) -> DeadZoneSeverity {
        let Some(link) = self.active_cellular_link() else {
            return DeadZoneSeverity::Good;
        };
        if !link.healthy || self.packet_loss_percent >= 20.0 || link.signal_dbm <= -118 {
            DeadZoneSeverity::Outage
        } else if self.packet_loss_percent >= 5.0
            || self.latency_ms >= 200
            || link.signal_dbm <= -110
        {
            DeadZoneSeverity::Degraded
        } else if self.packet_loss_percent >= 1.0
            || self.latency_ms >= 120
            || link.signal_dbm <= -100
        {
            DeadZoneSeverity::Weak
        } else {
            DeadZoneSeverity::Good
        }
    }
}

/// Cellular link status.
#[derive(Debug, Clone)]
pub struct CellularLink {
    /// SIM state.
    pub sim_state: String,
    /// Carrier.
    pub carrier: String,
    /// Signal in dBm.
    pub signal_dbm: i32,
    /// Network technology.
    pub technology: String,
    /// WAN IP.
    pub wan_ip: String,
    /// Link health.
    pub healthy: bool,
}

/// Factory reset guardrail model.
#[derive(Debug, Clone)]
pub struct FactoryResetWorkflow {
    /// Backup is required before reset.
    pub backup_required: bool,
    /// Backup has completed.
    pub backup_completed: bool,
    /// Typed confirmation phrase.
    pub confirmation_phrase: String,
    /// Phrase entered by the user.
    pub typed_confirmation: String,
    /// Reconnect workflow text.
    pub reconnect_workflow: Vec<String>,
}

impl FactoryResetWorkflow {
    /// The production guardrail state: reset stays disarmed because NO backup
    /// has actually completed yet (the fixture's `backup_completed: true` was a
    /// fabricated claim).
    fn awaiting_backup() -> Self {
        Self {
            backup_completed: false,
            ..Self::template()
        }
    }

    /// TEST FIXTURE ONLY — a guarded workflow whose backup already completed.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn guarded() -> Self {
        Self::template()
    }

    /// The shared guardrail template (backup required, phrase set, workflow
    /// steps spelled out) — config, not data.
    fn template() -> Self {
        Self {
            backup_required: true,
            backup_completed: true,
            confirmation_phrase: "RESET MG90".to_string(),
            typed_confirmation: String::new(),
            reconnect_workflow: vec![
                "Wait for MG90 reboot".to_string(),
                "Keep direct Ethernet connected".to_string(),
                "Rediscover local address".to_string(),
                "Re-authenticate".to_string(),
                "Restore or reconfigure".to_string(),
                "Create new baseline backup".to_string(),
            ],
        }
    }

    /// Whether reset can be armed.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.backup_completed && self.typed_confirmation == self.confirmation_phrase
    }
}

/// Native MG90 setting descriptor.
#[derive(Debug, Clone)]
pub struct Mg90SettingDescriptor {
    /// Stable setting id.
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Category.
    pub category: Mg90SettingCategory,
    /// Value type.
    pub value_type: SettingValueType,
    /// Read method.
    pub read_method: Mg90ManagementMethod,
    /// Write method.
    pub write_method: Mg90ManagementMethod,
    /// Reboot requirement.
    pub requires_reboot: bool,
    /// Management disconnect risk.
    pub may_disconnect_management: bool,
    /// Rollback support.
    pub supports_rollback: bool,
    /// Validation rules.
    pub validation: Vec<ValidationRule>,
}

/// MG90 setting categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mg90SettingCategory {
    /// Overview.
    Overview,
    /// Cellular and SIM.
    CellularSim,
    /// Wi-Fi.
    Wifi,
    /// Ethernet.
    Ethernet,
    /// WAN policies.
    WanPolicies,
    /// LAN/DHCP/VLAN.
    LanDhcpVlan,
    /// Firewall.
    Firewall,
    /// VPN.
    Vpn,
    /// GNSS.
    Gnss,
    /// Serial recovery.
    SerialRecovery,
    /// GPIO.
    Gpio,
    /// Services.
    Services,
    /// Security.
    Security,
    /// Diagnostics.
    Diagnostics,
    /// Logs.
    Logs,
    /// Backup and restore.
    BackupRestore,
    /// Original Local Configuration Interface fallback.
    OriginalLciFallback,
}

impl Mg90SettingCategory {
    /// All native MG90 setting categories in product order.
    pub const ALL: [Self; 17] = [
        Self::Overview,
        Self::CellularSim,
        Self::Wifi,
        Self::Ethernet,
        Self::WanPolicies,
        Self::LanDhcpVlan,
        Self::Firewall,
        Self::Vpn,
        Self::Gnss,
        Self::SerialRecovery,
        Self::Gpio,
        Self::Services,
        Self::Security,
        Self::Diagnostics,
        Self::Logs,
        Self::BackupRestore,
        Self::OriginalLciFallback,
    ];

    /// Label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::CellularSim => "Cellular & SIM",
            Self::Wifi => "Wi-Fi",
            Self::Ethernet => "Ethernet",
            Self::WanPolicies => "WAN Policies",
            Self::LanDhcpVlan => "LAN / DHCP / VLAN",
            Self::Firewall => "Firewall",
            Self::Vpn => "VPN",
            Self::Gnss => "GNSS",
            Self::SerialRecovery => "Serial Recovery",
            Self::Gpio => "GPIO",
            Self::Services => "Services",
            Self::Security => "Security",
            Self::Diagnostics => "Diagnostics",
            Self::Logs => "Logs",
            Self::BackupRestore => "Backup & Restore",
            Self::OriginalLciFallback => "Original LCI Fallback",
        }
    }
}

/// Setting value kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValueType {
    /// Boolean.
    Boolean,
    /// Integer.
    Integer,
    /// Text.
    Text,
    /// Enum choices.
    Enum(Vec<String>),
}

/// Management method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mg90ManagementMethod {
    /// Local MG90 API over direct Ethernet.
    LocalApi,
    /// Local configuration interface fallback.
    LocalConfigurationInterface,
    /// Serial recovery console only.
    SerialRecoveryConsole,
    /// Simulator method.
    Simulator,
    /// Unsupported on this capability profile.
    Unsupported,
}

/// Validation rule descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationRule {
    /// Rule label.
    pub label: String,
}

/// Guarded setting-change plan.
#[derive(Debug, Clone)]
pub struct SettingChangePlan {
    /// Setting id.
    pub setting_id: String,
    /// Required ordered steps.
    pub steps: Vec<String>,
    /// Warn but do not block while moving.
    pub moving_warning: bool,
    /// Backup requirement.
    pub backup_required: bool,
    /// Rollback possible.
    pub rollback_supported: bool,
}

impl SettingChangePlan {
    fn for_setting(setting: &Mg90SettingDescriptor, moving: bool) -> Self {
        let mut steps = vec![
            "Validate pending value".to_string(),
            "Create versioned backup".to_string(),
            "Apply change".to_string(),
            "Read back current value".to_string(),
            "Verify direct-Ethernet management path".to_string(),
            "Write audit entry".to_string(),
        ];
        if setting.supports_rollback {
            steps.insert(5, "Rollback if verification fails".to_string());
        }
        Self {
            setting_id: setting.id.clone(),
            steps,
            moving_warning: moving,
            backup_required: true,
            rollback_supported: setting.supports_rollback,
        }
    }
}

/// TEST FIXTURE ONLY — sample MG90 setting descriptors for the simulator seed.
#[cfg(any(test, feature = "sim-fixture"))]
fn sample_settings() -> Vec<Mg90SettingDescriptor> {
    vec![
        Mg90SettingDescriptor {
            id: "gnss.primary".to_string(),
            display_name: "MG90 GNSS publish rate".to_string(),
            category: Mg90SettingCategory::Gnss,
            value_type: SettingValueType::Enum(vec![
                "1 Hz".to_string(),
                "5 Hz".to_string(),
                "10 Hz".to_string(),
            ]),
            read_method: Mg90ManagementMethod::Simulator,
            write_method: Mg90ManagementMethod::Simulator,
            requires_reboot: false,
            may_disconnect_management: false,
            supports_rollback: true,
            validation: vec![ValidationRule {
                label: "supported by detected MGOS capability".to_string(),
            }],
        },
        Mg90SettingDescriptor {
            id: "wan.policy".to_string(),
            display_name: "WAN failover policy".to_string(),
            category: Mg90SettingCategory::WanPolicies,
            value_type: SettingValueType::Enum(vec![
                "cellular_a_primary".to_string(),
                "cellular_b_primary".to_string(),
                "best_quality".to_string(),
            ]),
            read_method: Mg90ManagementMethod::Simulator,
            write_method: Mg90ManagementMethod::Simulator,
            requires_reboot: false,
            may_disconnect_management: true,
            supports_rollback: true,
            validation: vec![ValidationRule {
                label: "direct Ethernet remains reachable".to_string(),
            }],
        },
        Mg90SettingDescriptor {
            id: "security.password".to_string(),
            display_name: "Local admin password".to_string(),
            category: Mg90SettingCategory::Security,
            value_type: SettingValueType::Text,
            read_method: Mg90ManagementMethod::Simulator,
            write_method: Mg90ManagementMethod::Simulator,
            requires_reboot: false,
            may_disconnect_management: false,
            supports_rollback: false,
            validation: vec![ValidationRule {
                label: "vault write succeeds before device write".to_string(),
            }],
        },
    ]
}

/// Backup/restore-point record.
#[derive(Debug, Clone)]
pub struct BackupRecord {
    /// Backup id.
    pub id: String,
    /// Reason/audit label.
    pub reason: String,
    /// Encrypted-at-rest flag.
    pub encrypted: bool,
    /// Restore-point flag.
    pub restore_point: bool,
    /// Created timestamp label.
    pub created: String,
}

/// Location source manager.
#[derive(Debug, Clone)]
pub struct LocationManager {
    /// Primary source selected by the user.
    pub primary: LocationSourceKind,
    /// Sources are equal peers; v1 never auto-failovers.
    pub auto_failover: bool,
    /// Source records.
    pub sources: Vec<LocationSource>,
}

impl LocationManager {
    /// The production source manager: exactly one source — the MG90 GNSS
    /// primary, armed but source-less (no fix, null coordinates, disconnected)
    /// until a live `state/vehicle` mirror folds in. No fabricated gpsd /
    /// manual / simulator peers.
    fn live() -> Self {
        Self {
            primary: LocationSourceKind::Mg90Gnss,
            auto_failover: false,
            sources: vec![LocationSource::awaiting_live(LocationSourceKind::Mg90Gnss)],
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        Self {
            primary: LocationSourceKind::Mg90Gnss,
            auto_failover: false,
            sources: vec![
                // The PRIMARY MG90 GNSS starts with no lock: the cockpit must not
                // read as a real vehicle sitting in Pittsburgh before a live fix
                // has been folded. The dedicated Simulator source keeps a fix so
                // the Simulator section still has a demonstrable position.
                LocationSource::acquiring(LocationSourceKind::Mg90Gnss),
                LocationSource::sample(LocationSourceKind::UsbGpsd, 4.6, 1.7, true),
                LocationSource::sample(LocationSourceKind::ManualTest, 0.0, 0.0, true),
                LocationSource::sample(LocationSourceKind::Simulator, 2.8, 0.3, true),
            ],
        }
    }

    /// Set primary source manually.
    pub fn set_primary(&mut self, kind: LocationSourceKind) {
        if self
            .sources
            .iter()
            .any(|source| source.kind == kind && source.manual_switch_ready())
        {
            self.primary = kind;
        }
    }

    /// Primary sample.
    #[must_use]
    pub fn primary_sample(&self) -> Option<&LocationSample> {
        self.primary_source().map(|source| &source.sample)
    }

    /// Primary source record.
    #[must_use]
    pub fn primary_source(&self) -> Option<&LocationSource> {
        self.sources
            .iter()
            .find(|source| source.kind == self.primary)
    }

    /// Warning if primary source is unhealthy.
    #[must_use]
    pub fn primary_warning(&self) -> Option<String> {
        let source = self.primary_source()?;
        source.health_issue().map(|issue| {
            format!(
                "{} unhealthy: {issue}; accuracy {:.1} m, update age {:.1} s",
                source.kind.label(),
                source.sample.accuracy_m,
                source.sample.update_age_s
            )
        })
    }

    /// Healthy alternatives for one-click manual switch.
    #[must_use]
    pub fn healthy_alternatives(&self) -> Vec<LocationSourceKind> {
        self.sources
            .iter()
            .filter(|source| source.kind != self.primary && source.manual_switch_ready())
            .map(|source| source.kind)
            .collect()
    }
}

/// Location source kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocationSourceKind {
    /// MG90 GNSS.
    Mg90Gnss,
    /// USB GPS through gpsd.
    UsbGpsd,
    /// Manual test location.
    ManualTest,
    /// Simulator location.
    Simulator,
}

impl LocationSourceKind {
    /// Label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mg90Gnss => "MG90 GNSS",
            Self::UsbGpsd => "USB GPS via gpsd",
            Self::ManualTest => "Manual test location",
            Self::Simulator => "Simulator location",
        }
    }
}

/// One location source row.
#[derive(Debug, Clone)]
pub struct LocationSource {
    /// Source kind.
    pub kind: LocationSourceKind,
    /// Source status.
    pub status: SourceStatus,
    /// Connected device label.
    pub connected_device: String,
    /// Raw diagnostics.
    pub diagnostics: BTreeMap<String, String>,
    /// Latest sample.
    pub sample: LocationSample,
}

impl LocationSource {
    /// The production "awaiting live feed" source: NO position lock, null
    /// coordinates (`!has_fix()`), and honestly **disconnected** — before a
    /// `state/vehicle` mirror exists we cannot claim the device link is up.
    /// [`MapsLocationSurface::refresh_from_vehicle`] flips it Connected and
    /// fills the sample the moment a live mirror folds in. The Drive HUD paints
    /// "Acquiring GPS" and every GPS tile reads "—" until then (Q33).
    fn awaiting_live(kind: LocationSourceKind) -> Self {
        let mut diagnostics = BTreeMap::new();
        diagnostics.insert("adapter".to_string(), kind.label().to_string());
        diagnostics.insert(
            "mode".to_string(),
            "awaiting live state/vehicle mirror".to_string(),
        );
        diagnostics.insert("fix".to_string(), "acquiring — no lock".to_string());
        Self {
            kind,
            status: SourceStatus::Disconnected,
            connected_device: match kind {
                LocationSourceKind::Mg90Gnss => "MG90 local GNSS".to_string(),
                LocationSourceKind::UsbGpsd => "gpsd tcp://127.0.0.1:2947 skeleton".to_string(),
                LocationSourceKind::ManualTest => "operator-entered point".to_string(),
                LocationSourceKind::Simulator => "route simulator".to_string(),
            },
            diagnostics,
            sample: LocationSample {
                fix_type: "No fix".to_string(),
                latitude: 0.0,
                longitude: 0.0,
                accuracy_m: 0.0,
                speed_mph: 0.0,
                heading_deg: 0.0,
                altitude_m: 0.0,
                satellites: None,
                update_rate_hz: 0.0,
                update_age_s: 0.0,
            },
        }
    }

    /// TEST FIXTURE ONLY — a connected source with a demo Pittsburgh fix.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn sample(
        kind: LocationSourceKind,
        accuracy_m: f32,
        update_age_s: f32,
        connected: bool,
    ) -> Self {
        let mut diagnostics = BTreeMap::new();
        diagnostics.insert("adapter".to_string(), kind.label().to_string());
        diagnostics.insert("mode".to_string(), "simulated".to_string());
        Self {
            kind,
            status: if connected {
                SourceStatus::Connected
            } else {
                SourceStatus::Disconnected
            },
            connected_device: match kind {
                LocationSourceKind::Mg90Gnss => "MG90 local GNSS".to_string(),
                LocationSourceKind::UsbGpsd => "gpsd tcp://127.0.0.1:2947 skeleton".to_string(),
                LocationSourceKind::ManualTest => "operator-entered point".to_string(),
                LocationSourceKind::Simulator => "route simulator".to_string(),
            },
            diagnostics,
            sample: LocationSample {
                fix_type: "3D".to_string(),
                latitude: 40.4406,
                longitude: -79.9959,
                accuracy_m,
                speed_mph: 27.0,
                heading_deg: 284.0,
                altitude_m: 311.0,
                satellites: Some(14),
                update_rate_hz: 1.0,
                update_age_s,
            },
        }
    }

    /// TEST FIXTURE ONLY — a source that is connected but holds **no position
    /// lock yet**: the fixture's stand-in for the MG90 GNSS before a live
    /// `state/vehicle` mirror has been folded. Its sample is `!has_fix()`, so
    /// the Drive HUD paints "Acquiring GPS" and the instrument strip's GPS
    /// tiles read "—" / "No fix" instead of a hard-coded coordinate in a city
    /// the vehicle is not in. The device itself is Connected (link up), so it
    /// stays a healthy peer.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn acquiring(kind: LocationSourceKind) -> Self {
        let mut source = Self::sample(kind, 0.0, 0.0, true);
        source.sample = LocationSample {
            fix_type: "No fix".to_string(),
            latitude: 0.0,
            longitude: 0.0,
            accuracy_m: 0.0,
            speed_mph: 0.0,
            heading_deg: 0.0,
            altitude_m: 0.0,
            satellites: None,
            update_rate_hz: 0.0,
            update_age_s: 0.0,
        };
        source
            .diagnostics
            .insert("fix".to_string(), "acquiring — no lock".to_string());
        source
    }

    /// True when this source is safe to select manually as the primary source.
    #[must_use]
    pub fn manual_switch_ready(&self) -> bool {
        self.health_issue().is_none()
    }

    /// Operator-facing readiness reason for the manual primary switch button.
    #[must_use]
    pub fn manual_switch_reason(&self) -> String {
        self.health_issue().unwrap_or_else(|| {
            format!(
                "ready: connected with {:.1} m accuracy and {:.1} s update age",
                self.sample.accuracy_m, self.sample.update_age_s
            )
        })
    }

    fn health_issue(&self) -> Option<String> {
        if self.status != SourceStatus::Connected {
            return Some(format!("source is {}", self.status.label()));
        }
        if self.sample.stale() {
            return Some(format!(
                "update is stale at {:.1} s",
                self.sample.update_age_s
            ));
        }
        if !self.sample.healthy() {
            return Some(format!(
                "accuracy {:.1} m exceeds 5.0 m",
                self.sample.accuracy_m
            ));
        }
        None
    }
}

/// Source connection status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    /// Connected.
    Connected,
    /// Disconnected.
    Disconnected,
    /// Stale.
    Stale,
    /// Unhealthy.
    Unhealthy,
}

impl SourceStatus {
    /// Human label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Stale => "stale",
            Self::Unhealthy => "unhealthy",
        }
    }
}

/// Location sample.
#[derive(Debug, Clone)]
pub struct LocationSample {
    /// Fix type.
    pub fix_type: String,
    /// Latitude.
    pub latitude: f64,
    /// Longitude.
    pub longitude: f64,
    /// Accuracy in meters.
    pub accuracy_m: f32,
    /// Speed in mph.
    pub speed_mph: f32,
    /// Heading in degrees.
    pub heading_deg: f32,
    /// Altitude in meters.
    pub altitude_m: f32,
    /// Satellite count.
    pub satellites: Option<u8>,
    /// Update rate in Hz.
    pub update_rate_hz: f32,
    /// Age of latest update in seconds.
    pub update_age_s: f32,
}

impl LocationSample {
    /// v1 health rule.
    #[must_use]
    pub fn healthy(&self) -> bool {
        self.accuracy_m <= 5.0 && self.update_age_s <= 5.0
    }

    /// Stale rule.
    #[must_use]
    pub fn stale(&self) -> bool {
        self.update_age_s > 5.0
    }

    /// Motion rule.
    #[must_use]
    pub fn moving(&self) -> bool {
        self.speed_mph > 1.0
    }

    /// Whether this sample represents a real position fix.
    ///
    /// The driving HUD uses this to decide between the live vehicle chevron and
    /// the honest "Acquiring GPS" state. A sample counts as fixed when its
    /// `fix_type` reports an actual 2D/3D/DGPS/RTK lock (not empty, "no fix" or
    /// spelling variants such as "no-fix"/"nofix", or "none") and the reported
    /// coordinate is not the degenerate null island
    /// `0, 0`. Guarding on both keeps a half-populated sample from feeding a
    /// zero/NaN-adjacent position into HUD layout.
    #[must_use]
    pub fn has_fix(&self) -> bool {
        let fix = self.fix_type.trim();
        let fix_ok = !fix.is_empty()
            && !fix.eq_ignore_ascii_case("no fix")
            && !fix.eq_ignore_ascii_case("no-fix")
            && !fix.eq_ignore_ascii_case("none")
            && !fix.eq_ignore_ascii_case("0")
            && !fix.eq_ignore_ascii_case("nofix");
        let coord_ok = self.latitude.is_finite()
            && self.longitude.is_finite()
            && (self.latitude.abs() > f64::EPSILON || self.longitude.abs() > f64::EPSILON);
        fix_ok && coord_ok
    }
}

/// Trip recorder state.
#[derive(Debug, Clone)]
pub struct TripRecorderState {
    /// Retention days.
    pub retention_days: u32,
    /// Breadcrumbs.
    pub breadcrumbs: Vec<Breadcrumb>,
    /// Export formats.
    pub export_formats: Vec<TripExportFormat>,
    /// History encrypted at rest.
    pub encrypted_at_rest: bool,
}

impl TripRecorderState {
    /// The production trip recorder: retention/export/encryption CONFIG intact,
    /// zero breadcrumbs — nothing has been recorded yet, so nothing shows.
    fn live() -> Self {
        Self {
            retention_days: 30,
            breadcrumbs: Vec::new(),
            export_formats: TripExportFormat::ALL.to_vec(),
            encrypted_at_rest: true,
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        Self {
            retention_days: 30,
            breadcrumbs: vec![
                Breadcrumb {
                    lat: 40.4406,
                    lon: -79.9959,
                    speed_mph: 20.0,
                    source: LocationSourceKind::Mg90Gnss,
                    event: Some("trip started by ignition".to_string()),
                },
                Breadcrumb {
                    lat: 40.4442,
                    lon: -80.0031,
                    speed_mph: 27.0,
                    source: LocationSourceKind::Mg90Gnss,
                    event: Some("cellular signal degraded".to_string()),
                },
            ],
            export_formats: TripExportFormat::ALL.to_vec(),
            encrypted_at_rest: true,
        }
    }
}

/// One breadcrumb point.
#[derive(Debug, Clone)]
pub struct Breadcrumb {
    /// Latitude.
    pub lat: f64,
    /// Longitude.
    pub lon: f64,
    /// Speed.
    pub speed_mph: f32,
    /// Source.
    pub source: LocationSourceKind,
    /// Optional event marker.
    pub event: Option<String>,
}

/// Trip export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripExportFormat {
    /// GPX.
    Gpx,
    /// GeoJSON.
    GeoJson,
    /// CSV.
    Csv,
    /// Full diagnostic bundle.
    DiagnosticBundle,
}

impl TripExportFormat {
    /// All formats.
    pub const ALL: [Self; 4] = [Self::Gpx, Self::GeoJson, Self::Csv, Self::DiagnosticBundle];

    /// Label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gpx => "GPX",
            Self::GeoJson => "GeoJSON",
            Self::Csv => "CSV",
            Self::DiagnosticBundle => "Diagnostic bundle",
        }
    }
}

/// Cellular dead-zone state.
#[derive(Debug, Clone)]
pub struct DeadZoneState {
    /// Recorded zones.
    pub zones: Vec<DeadZoneRecord>,
    /// Used for route risk awareness.
    pub route_risk: String,
}

impl DeadZoneState {
    /// The production dead-zone recorder: empty until
    /// [`MapsLocationSurface::record_dead_zone_from_current_status`] (the REAL
    /// seam — it requires a live fix + live link data) records one.
    fn live() -> Self {
        Self {
            zones: Vec::new(),
            route_risk: "No cellular dead zones recorded on this route".to_string(),
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        Self {
            zones: vec![DeadZoneRecord {
                position: "40.4442, -80.0031".to_string(),
                selected_wan: "Cellular A".to_string(),
                carrier: "FirstNet".to_string(),
                technology: "5G/LTE-A".to_string(),
                signal_dbm: -111,
                packet_loss_percent: 8.0,
                latency_ms: 220,
                outage_duration_s: 18,
                severity: DeadZoneSeverity::Degraded,
            }],
            route_risk: "One known weak segment in next 11 mi".to_string(),
        }
    }

    fn refresh_route_risk(&mut self) {
        let outage_count = self
            .zones
            .iter()
            .filter(|zone| zone.severity == DeadZoneSeverity::Outage)
            .count();
        let degraded_count = self
            .zones
            .iter()
            .filter(|zone| zone.severity == DeadZoneSeverity::Degraded)
            .count();
        self.route_risk = if outage_count > 0 {
            format!("{outage_count} cellular outage segment(s) recorded on this route")
        } else if degraded_count > 0 {
            format!("{degraded_count} degraded cellular segment(s) recorded on this route")
        } else if self.zones.is_empty() {
            "No cellular dead zones recorded on this route".to_string()
        } else {
            format!(
                "{} weak cellular segment(s) recorded on this route",
                self.zones.len()
            )
        };
    }
}

/// Dead-zone record.
#[derive(Debug, Clone)]
pub struct DeadZoneRecord {
    /// Position label.
    pub position: String,
    /// Selected WAN.
    pub selected_wan: String,
    /// Carrier.
    pub carrier: String,
    /// Technology.
    pub technology: String,
    /// Signal.
    pub signal_dbm: i32,
    /// Packet loss.
    pub packet_loss_percent: f32,
    /// Latency.
    pub latency_ms: u32,
    /// Outage duration.
    pub outage_duration_s: u32,
    /// Classified route risk severity.
    pub severity: DeadZoneSeverity,
}

/// Cellular route-risk severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadZoneSeverity {
    /// Current active cellular path is suitable.
    Good,
    /// Cellular path is usable but weak.
    Weak,
    /// Cellular path is degraded enough to warn during route planning.
    Degraded,
    /// Cellular path is effectively out or the active link reports unhealthy.
    Outage,
}

impl DeadZoneSeverity {
    /// Human label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Weak => "weak",
            Self::Degraded => "degraded",
            Self::Outage => "outage",
        }
    }
}

/// Vehicle telemetry state.
#[derive(Debug, Clone)]
pub struct VehicleState {
    /// Profile label.
    pub profile: String,
    /// Vehicle telemetry.
    pub telemetry: VehicleTelemetry,
    /// Profile notes.
    pub profile_notes: Vec<String>,
}

impl VehicleState {
    /// The production vehicle state: NO telemetry claim of any kind until a
    /// live `state/vehicle` mirror folds one in. The confidence label never
    /// starts with `"live vehicle-gateway mirror"`, so
    /// [`VehicleTelemetry::is_live`] (and every gauge/tile riding it) reads
    /// absent — Q33: absent reads absent, never fabricated.
    fn awaiting_gateway() -> Self {
        Self {
            profile: String::new(),
            telemetry: VehicleTelemetry {
                speed_mph: 0.0,
                rpm: 0,
                coolant_c: 0.0,
                battery_v: 0.0,
                fuel_percent: None,
                dtc_count: 0,
                ignition_on: false,
                moving: false,
                odometer_mi: None,
                runtime_min: 0,
                internal_temp_c: None,
                confidence: "no vehicle telemetry source — awaiting vehicle-gateway mirror"
                    .to_string(),
                last_update_age_s: 0.0,
            },
            profile_notes: Vec::new(),
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn ford_interceptor_2020() -> Self {
        Self {
            profile: "2020 Ford Police Interceptor Utility".to_string(),
            telemetry: VehicleTelemetry {
                speed_mph: 27.0,
                rpm: 1_840,
                coolant_c: 91.0,
                battery_v: 13.9,
                fuel_percent: Some(64.0),
                dtc_count: 0,
                ignition_on: true,
                moving: true,
                odometer_mi: Some(78_214),
                runtime_min: 42,
                internal_temp_c: None,
                confidence: "simulated CAN/OBD profile".to_string(),
                last_update_age_s: 0.8,
            },
            profile_notes: vec![
                "Generic OBD is not assumed to expose every Ford-specific field.".to_string(),
                "Profile layer is ready for Ford-specific PIDs as they are validated.".to_string(),
            ],
        }
    }
}

/// Vehicle telemetry.
#[derive(Debug, Clone)]
pub struct VehicleTelemetry {
    /// Vehicle speed.
    pub speed_mph: f32,
    /// Engine RPM.
    pub rpm: u32,
    /// Coolant temperature.
    pub coolant_c: f32,
    /// Battery/charging voltage.
    pub battery_v: f32,
    /// Fuel level.
    pub fuel_percent: Option<f32>,
    /// DTC count.
    pub dtc_count: u32,
    /// Ignition state.
    pub ignition_on: bool,
    /// Park/moving state.
    pub moving: bool,
    /// Odometer.
    pub odometer_mi: Option<u32>,
    /// Runtime.
    pub runtime_min: u32,
    /// Gateway MCU board temperature, `Celsius` (Rolling Node — from the
    /// `state/vehicle/<node>` mirror's `VehicleTelem::internal_temp_c`;
    /// `None` in simulator mode, which has no MCU to sample).
    pub internal_temp_c: Option<f32>,
    /// Confidence label.
    pub confidence: String,
    /// Last update age.
    pub last_update_age_s: f32,
}

impl VehicleTelemetry {
    /// Whether an online vehicle-gateway mirror supplied this telemetry.
    ///
    /// This is deliberately source-only. Diagnostic surfaces may still show a
    /// stale mirror's age and provenance after [`Self::is_live`] turns false.
    #[must_use]
    pub fn has_live_gateway_source(&self) -> bool {
        self.confidence.starts_with("live vehicle-gateway mirror")
    }

    /// Whether this telemetry is a fresh LIVE gateway reading.
    /// [`MapsLocationSurface::refresh_from_vehicle`] stamps the confidence
    /// label `"live vehicle-gateway mirror (…)"` only when a real
    /// `state/vehicle/<node>` mirror folded in with the adapter ONLINE; every
    /// other label (awaiting-mirror seed, offline adapter, test fixture) reads
    /// not-live. The retained mirror must also be at most five seconds old, so
    /// a quiet adapter cannot hold the speedometer or in-motion safety policy
    /// active indefinitely. Gauges and readouts ride this so they can never
    /// present a non-live number as a reading. PLATFORM-INTERFACES Q33.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.has_live_gateway_source()
            && self.last_update_age_s.is_finite()
            && (0.0..=VEHICLE_TELEMETRY_STALE_AFTER_S).contains(&self.last_update_age_s)
    }
}

/// Devices and I/O state.
#[derive(Debug, Clone)]
pub struct DeviceIoState {
    /// Serial recovery console.
    pub serial: SerialConsoleState,
    /// GPIO automation rules.
    pub gpio_rules: Vec<GpioAutomationRule>,
    /// USB device list.
    pub usb_devices: Vec<String>,
    /// Ethernet state.
    pub ethernet_state: String,
    /// CAN/OBD state.
    pub can_obd_state: String,
}

impl DeviceIoState {
    /// The production device state: nothing attached, nothing detected, no
    /// automation rules — the views already render designed empty states
    /// ("No USB devices attached.", "No GPIO automation rules defined.",
    /// "No console output.", em-dash readouts) for exactly this shape.
    fn live() -> Self {
        Self {
            serial: SerialConsoleState {
                connected: false,
                baud_profile: "115200 8N1".to_string(),
                transcript_lines: Vec::new(),
            },
            gpio_rules: Vec::new(),
            usb_devices: Vec::new(),
            ethernet_state: String::new(),
            can_obd_state: String::new(),
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        Self {
            serial: SerialConsoleState {
                connected: false,
                baud_profile: "115200 8N1".to_string(),
                transcript_lines: vec![
                    "Serial recovery console is reserved for MG90 recovery only.".to_string(),
                    "Normal configuration uses direct Ethernet local management.".to_string(),
                ],
            },
            gpio_rules: vec![
                GpioAutomationRule::new(
                    "ignition-start-trip",
                    "WHEN ignition input changes to ON",
                    "THEN start trip recording",
                ),
                GpioAutomationRule::new(
                    "input-marker",
                    "WHEN GPIO input 1 is triggered",
                    "THEN drop event marker on map",
                ),
                GpioAutomationRule::new(
                    "geofence-output",
                    "WHEN vehicle enters geofence",
                    "THEN set GPIO output 2 ON",
                ),
                GpioAutomationRule::new(
                    "weather-route-alert",
                    "WHEN weather alert intersects route",
                    "THEN create dashboard alert",
                ),
            ],
            usb_devices: vec!["USB GPS dongle simulator".to_string()],
            ethernet_state: "direct MG90 cable detected".to_string(),
            can_obd_state: "Ford 2020 Interceptor simulator online".to_string(),
        }
    }
}

/// Serial terminal state.
#[derive(Debug, Clone)]
pub struct SerialConsoleState {
    /// Connected.
    pub connected: bool,
    /// Baud/profile selector.
    pub baud_profile: String,
    /// Transcript.
    pub transcript_lines: Vec<String>,
}

/// GPIO automation rule.
#[derive(Debug, Clone)]
pub struct GpioAutomationRule {
    /// Stable id.
    pub id: String,
    /// Enabled flag.
    pub enabled: bool,
    /// Trigger text.
    pub trigger: String,
    /// Condition text.
    pub condition: String,
    /// Action text.
    pub action: String,
    /// Last run.
    pub last_run: String,
    /// Audit log.
    pub audit_log: Vec<String>,
}

impl GpioAutomationRule {
    /// TEST FIXTURE ONLY — fixture rule builder for the simulator seed.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn new(id: &str, trigger: &str, action: &str) -> Self {
        Self {
            id: id.to_string(),
            enabled: true,
            trigger: trigger.to_string(),
            condition: "simulator condition passes".to_string(),
            action: action.to_string(),
            last_run: "not run".to_string(),
            audit_log: vec!["created by simulator fixture".to_string()],
        }
    }
}

/// Firmware lifecycle state.
#[derive(Debug, Clone)]
pub struct FirmwareWorkflow {
    /// Current firmware.
    pub current: String,
    /// Target package.
    pub target_package: String,
    /// Validation checks.
    pub checks: Vec<FirmwareCheck>,
    /// Progress.
    pub progress_percent: u8,
    /// Restore-point integration.
    pub restore_point_ready: bool,
}

impl FirmwareWorkflow {
    /// The production firmware state: nothing read from a device, no package
    /// selected, ZERO pre-flight checks (checks run against a real selected
    /// package, they are not pre-passed), and no restore point claimed.
    fn live() -> Self {
        Self {
            current: String::new(),
            target_package: "no package selected".to_string(),
            checks: Vec::new(),
            progress_percent: 0,
            restore_point_ready: false,
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn simulated() -> Self {
        Self {
            current: "MGOS simulated current".to_string(),
            target_package: "no package selected".to_string(),
            checks: vec![
                FirmwareCheck::pass("correct MG90 model"),
                FirmwareCheck::pass("correct MGOS family"),
                FirmwareCheck::pass("package integrity placeholder"),
                FirmwareCheck::warn("verify vehicle/MG90 power before install"),
                FirmwareCheck::pass("pre-update backup completed"),
                FirmwareCheck::pass("direct Ethernet present"),
                FirmwareCheck::pass("credentials valid"),
                FirmwareCheck::pass("rollback/recovery plan available"),
            ],
            progress_percent: 0,
            restore_point_ready: true,
        }
    }
}

/// Firmware check.
#[derive(Debug, Clone)]
pub struct FirmwareCheck {
    /// Check label.
    pub label: String,
    /// Severity/pass state.
    pub state: CheckState,
}

impl FirmwareCheck {
    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn pass(label: &str) -> Self {
        Self {
            label: label.to_string(),
            state: CheckState::Pass,
        }
    }

    /// TEST FIXTURE ONLY.
    #[cfg(any(test, feature = "sim-fixture"))]
    fn warn(label: &str) -> Self {
        Self {
            label: label.to_string(),
            state: CheckState::Warn,
        }
    }
}

/// Check state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// Passing.
    Pass,
    /// Warning.
    Warn,
    /// Failed.
    Fail,
}

/// Local encrypted vault readiness model.
#[derive(Debug, Clone)]
pub struct EncryptedVaultState {
    /// Single local admin user.
    pub local_admin_user: String,
    /// Credential storage encrypted.
    pub credentials_encrypted: bool,
    /// Location/trip data encrypted.
    pub location_data_encrypted: bool,
    /// Vault backend label.
    pub backend: String,
}

impl EncryptedVaultState {
    fn ready_for_local_admin() -> Self {
        Self {
            local_admin_user: "local admin".to_string(),
            credentials_encrypted: true,
            location_data_encrypted: true,
            backend: "project-managed encrypted local vault skeleton".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_now_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or_default()
    }

    #[test]
    fn gps_health_rule_matches_product_lock() {
        let healthy = LocationSample {
            fix_type: "3D".to_string(),
            latitude: 0.0,
            longitude: 0.0,
            accuracy_m: 5.0,
            speed_mph: 0.0,
            heading_deg: 0.0,
            altitude_m: 0.0,
            satellites: Some(8),
            update_rate_hz: 1.0,
            update_age_s: 5.0,
        };
        assert!(healthy.healthy());
        let inaccurate = LocationSample {
            accuracy_m: 5.1,
            ..healthy.clone()
        };
        let stale = LocationSample {
            update_age_s: 5.1,
            ..healthy
        };
        assert!(!inaccurate.healthy());
        assert!(!stale.healthy());
    }

    #[test]
    fn has_fix_distinguishes_real_lock_from_acquiring() {
        let fixed = LocationSample {
            fix_type: "3D".to_string(),
            latitude: 40.4406,
            longitude: -79.9959,
            accuracy_m: 3.0,
            speed_mph: 27.0,
            heading_deg: 284.0,
            altitude_m: 311.0,
            satellites: Some(14),
            update_rate_hz: 1.0,
            update_age_s: 1.0,
        };
        assert!(fixed.has_fix());

        let acquiring = LocationSample {
            fix_type: "No fix".to_string(),
            latitude: 0.0,
            longitude: 0.0,
            satellites: None,
            ..fixed.clone()
        };
        assert!(!acquiring.has_fix());

        let hyphenated_acquiring = LocationSample {
            fix_type: "no-fix".to_string(),
            latitude: 32.1680,
            longitude: -95.8490,
            ..fixed.clone()
        };
        assert!(!hyphenated_acquiring.has_fix());

        let empty_fix = LocationSample {
            fix_type: String::new(),
            ..fixed.clone()
        };
        assert!(!empty_fix.has_fix());

        let null_island = LocationSample {
            latitude: 0.0,
            longitude: 0.0,
            ..fixed
        };
        assert!(!null_island.has_fix());
    }

    #[test]
    fn default_primary_source_is_acquiring_not_a_fake_pittsburgh_fix() {
        // Before any live `state/vehicle` mirror is folded, the primary MG90 GNSS
        // must present as "no lock", never a hard-coded moving-vehicle fix — the
        // #1 "looks like fake data" tell.
        let state = MapsLocationSurface::simulated();
        let primary = state.locations.primary_source().expect("mg90 primary");
        assert_eq!(primary.kind, LocationSourceKind::Mg90Gnss);
        assert!(
            !primary.sample.has_fix(),
            "the seeded primary must not read as a real position lock"
        );
        assert_eq!(primary.sample.fix_type, "No fix");
        assert!(primary.sample.latitude.abs() < f64::EPSILON);

        // The dedicated Simulator source keeps a fix so the Simulator section still
        // demonstrates a position.
        let sim = state
            .locations
            .sources
            .iter()
            .find(|s| s.kind == LocationSourceKind::Simulator)
            .expect("simulator source");
        assert!(
            sim.sample.has_fix(),
            "the Simulator source keeps a demo fix"
        );
    }

    #[test]
    fn dead_zone_recording_requires_a_position_fix() {
        // Without a lock there is no honest coordinate — recording must refuse
        // rather than pin a fabricated null-island point.
        let mut state = MapsLocationSurface::simulated();
        state.mg90.status.cellular_a.signal_dbm = -119;
        state.mg90.status.cellular_a.healthy = false;
        assert!(
            !state.record_dead_zone_from_current_status(),
            "no fix ⇒ no geolocated dead zone"
        );
    }

    #[test]
    fn start_and_end_navigation_toggle_the_guidance_flag() {
        let mut state = MapsLocationSurface::simulated();
        assert!(!state.local_navigation.navigating, "idle by default");
        state.choose_destination(1);
        assert!(state.route_preview);
        state.start_navigation();
        assert!(state.local_navigation.navigating, "Start begins guidance");
        assert!(!state.route_preview);
        state.simulate_arrival();
        assert!(!state.local_navigation.navigating, "arrival ends guidance");
        state.start_navigation();
        state.end_navigation();
        assert!(!state.local_navigation.navigating, "End returns to idle");
    }

    #[test]
    fn motion_rule_warns_above_one_mph() {
        let mut state = MapsLocationSurface::simulated();
        state.locations.sources[0].sample.speed_mph = 1.0;
        state.vehicle.telemetry.moving = false;
        state.mg90.ignition_on = false;
        assert!(!state.moving());
        state.locations.sources[0].sample.speed_mph = 1.01;
        assert!(state.moving());
    }

    #[test]
    fn primary_source_never_auto_failovers() {
        let mut manager = LocationManager::simulated();
        manager.sources[0].sample.accuracy_m = 99.0;
        assert_eq!(manager.primary, LocationSourceKind::Mg90Gnss);
        assert!(!manager.auto_failover);
        assert!(manager.primary_warning().is_some());
        assert!(manager
            .healthy_alternatives()
            .contains(&LocationSourceKind::UsbGpsd));
        assert_eq!(manager.primary, LocationSourceKind::Mg90Gnss);
    }

    #[test]
    fn manual_switch_readiness_requires_connected_fresh_accurate_peer() {
        let mut manager = LocationManager::simulated();
        manager.sources[1].status = SourceStatus::Disconnected;
        manager.sources[2].sample.update_age_s = 6.0;
        manager.sources[3].sample.accuracy_m = 6.0;

        assert!(manager.healthy_alternatives().is_empty());
        assert!(manager.primary_warning().is_none());
        assert!(!manager.sources[1].manual_switch_ready());
        assert!(!manager.sources[2].manual_switch_ready());
        assert!(!manager.sources[3].manual_switch_ready());

        manager.set_primary(LocationSourceKind::UsbGpsd);
        assert_eq!(manager.primary, LocationSourceKind::Mg90Gnss);

        manager.sources[1].status = SourceStatus::Connected;
        assert_eq!(
            manager.healthy_alternatives(),
            vec![LocationSourceKind::UsbGpsd]
        );
        manager.set_primary(LocationSourceKind::UsbGpsd);
        assert_eq!(manager.primary, LocationSourceKind::UsbGpsd);
    }

    #[test]
    fn primary_warning_reports_source_status_even_with_healthy_sample() {
        let mut manager = LocationManager::simulated();
        manager.sources[0].status = SourceStatus::Unhealthy;

        let warning = manager.primary_warning().expect("status warning");
        assert!(warning.contains("source is unhealthy"));
        assert!(manager
            .healthy_alternatives()
            .contains(&LocationSourceKind::UsbGpsd));
    }

    #[test]
    fn offline_navigation_status_is_ready_for_simulated_fixture() {
        let state = MapsLocationSurface::simulated();
        let status = state.offline_navigation_status();

        assert_eq!(status.readiness, OfflineNavigationReadiness::Ready);
        assert!(status.can_claim_turn_by_turn());
        assert!(status.blockers.is_empty());
        assert!(status.warnings.is_empty());
        assert_eq!(
            status.loaded_region.as_deref(),
            Some("Default state/province region")
        );
        assert_eq!(status.coverage_percent, Some(100));
        assert!(status
            .notes
            .iter()
            .any(|note| note.contains("Simulator fixture")));
    }

    #[test]
    fn stale_primary_blocks_until_operator_selects_healthy_peer() {
        let mut state = MapsLocationSurface::simulated();
        state.simulate_stale_primary_location();

        let blocked = state.offline_navigation_status();
        assert_eq!(blocked.readiness, OfflineNavigationReadiness::Blocked);
        assert!(!blocked.can_claim_turn_by_turn());
        assert!(blocked
            .blockers
            .iter()
            .any(|blocker| blocker.contains("stale")));
        assert!(blocked
            .warnings
            .iter()
            .any(|warning| warning.contains("manual switch required")));

        state.locations.set_primary(LocationSourceKind::UsbGpsd);
        let restored = state.offline_navigation_status();
        assert_eq!(restored.readiness, OfflineNavigationReadiness::Ready);
        assert!(restored.can_claim_turn_by_turn());
    }

    #[test]
    fn missing_offline_map_bundle_blocks_offline_navigation() {
        let mut state = MapsLocationSurface::simulated();
        state.simulate_no_offline_maps();

        let status = state.offline_navigation_status();
        assert_eq!(status.readiness, OfflineNavigationReadiness::Blocked);
        assert_eq!(status.loaded_region, None);
        assert!(status
            .blockers
            .iter()
            .any(|blocker| blocker == "No loaded offline map region is available."));

        state.simulate_ready_offline_navigation();
        assert_eq!(
            state.offline_navigation_status().readiness,
            OfflineNavigationReadiness::Ready
        );
    }

    #[test]
    fn setting_changes_always_start_with_backup_and_readback() {
        let state = MapsLocationSurface::simulated();
        let plan = state
            .setting_change_plan("wan.policy")
            .expect("sample setting exists");
        assert!(plan.backup_required);
        assert!(plan
            .steps
            .iter()
            .any(|step| step == "Create versioned backup"));
        assert!(plan
            .steps
            .iter()
            .any(|step| step == "Read back current value"));
        assert!(plan
            .steps
            .iter()
            .any(|step| step == "Verify direct-Ethernet management path"));
        assert!(plan.moving_warning);
    }

    #[test]
    fn trip_exports_cover_required_formats() {
        let trips = TripRecorderState::simulated();
        assert_eq!(trips.retention_days, 30);
        for format in TripExportFormat::ALL {
            assert!(trips.export_formats.contains(&format), "{format:?}");
        }
        assert!(trips.encrypted_at_rest);
    }

    #[test]
    fn setup_wizard_reaches_ready_offline_in_simulator() {
        let mut mg90 = Mg90State::simulated();
        mg90.setup_step = SetupStep::NotConnected;
        for _ in SetupStep::ALL {
            mg90.advance_setup_simulated();
        }
        assert_eq!(mg90.setup_step, SetupStep::Ready);
        assert!(mg90.authenticated);
    }

    #[test]
    fn active_mg90_link_classifies_dead_zone_severity() {
        let mut status = Mg90Status::simulated();
        assert_eq!(status.dead_zone_severity(), DeadZoneSeverity::Good);

        status.cellular_a.signal_dbm = -104;
        assert_eq!(status.dead_zone_severity(), DeadZoneSeverity::Weak);

        status.packet_loss_percent = 6.0;
        assert_eq!(status.dead_zone_severity(), DeadZoneSeverity::Degraded);

        status.cellular_a.healthy = false;
        assert_eq!(status.dead_zone_severity(), DeadZoneSeverity::Outage);
    }

    #[test]
    fn cellular_dead_zone_record_uses_current_location_and_updates_route_risk() {
        let mut state = MapsLocationSurface::simulated();
        let initial_zones = state.dead_zones.zones.len();

        assert!(!state.record_dead_zone_from_current_status());
        assert_eq!(state.dead_zones.zones.len(), initial_zones);

        // A dead zone can only be pinned to the map with a real position fix, so
        // establish one on the primary (the seed is honestly acquiring / no lock),
        // matching a live GNSS lock while driving.
        if let Some(src) = state
            .locations
            .sources
            .iter_mut()
            .find(|s| s.kind == LocationSourceKind::Mg90Gnss)
        {
            src.sample.fix_type = "3D".to_string();
            src.sample.latitude = 40.4406;
            src.sample.longitude = -79.9959;
            src.sample.accuracy_m = 3.0;
        }

        assert!(state.simulate_cellular_dead_zone());
        assert_eq!(state.dead_zones.zones.len(), initial_zones + 1);
        let recorded = state.dead_zones.zones.last().expect("record appended");
        assert_eq!(recorded.position, "40.4406, -79.9959");
        assert_eq!(recorded.selected_wan, "Cellular A");
        assert_eq!(recorded.severity, DeadZoneSeverity::Outage);
        assert!(state.dead_zones.route_risk.contains("outage"));
    }

    #[test]
    fn route_preview_offers_selectable_alternates() {
        let nav = LocalNavigationState::simulated();
        assert!(
            nav.route_options.len() >= 2,
            "preview needs at least a fastest + alternate"
        );
        // Option 0 mirrors the active route so entering preview is consistent.
        assert_eq!(nav.selected_route, 0);
        assert_eq!(nav.route_options[0].eta, nav.active_route.eta);
        assert_eq!(
            nav.route_options[0].remaining_time_min,
            nav.active_route.remaining_time_min
        );
    }

    #[test]
    fn applying_a_route_option_updates_the_active_route() {
        let mut nav = LocalNavigationState::simulated();
        let alt = nav.route_options[1].clone();
        nav.apply_route_option(1);
        assert_eq!(nav.selected_route, 1);
        assert_eq!(nav.active_route.eta, alt.eta);
        assert_eq!(nav.active_route.remaining_time_min, alt.remaining_time_min);
        assert!((nav.active_route.remaining_distance_mi - alt.remaining_distance_mi).abs() < 1e-6);
        assert_eq!(nav.active_route.current_road, alt.via);
        // A clear alternate clears the traffic alert strip.
        assert!(nav.active_route.traffic_alert.is_empty());
    }

    #[test]
    fn applying_out_of_range_route_option_is_a_no_op() {
        let mut nav = LocalNavigationState::simulated();
        let before = nav.active_route.eta.clone();
        nav.apply_route_option(99);
        assert_eq!(nav.selected_route, 0);
        assert_eq!(nav.active_route.eta, before);
    }

    #[test]
    fn destinations_carry_an_address_for_the_preview_summary() {
        let nav = LocalNavigationState::simulated();
        assert!(nav
            .destinations
            .iter()
            .all(|destination| !destination.address.trim().is_empty()));
    }

    #[test]
    fn each_quick_category_chip_has_a_matching_destination() {
        // The "Where to?" chips (Home / Work / Fuel / Food / Parking) must each
        // resolve to a recent/favorite so a chip tap always opens a preview.
        let nav = LocalNavigationState::simulated();
        for category in ["home", "work", "fuel", "food", "parking"] {
            assert!(
                nav.destination_in_category(category).is_some(),
                "no destination for category {category}"
            );
        }
    }

    #[test]
    fn choosing_a_destination_opens_preview_and_records_selection() {
        let mut state = MapsLocationSurface::simulated();
        state.open_destination_search();
        assert!(state.destination_search);

        state.choose_destination(3);
        assert!(!state.destination_search);
        assert!(state.route_preview);
        assert_eq!(state.local_navigation.selected_destination, 3);
        assert_eq!(
            state
                .local_navigation
                .active_destination()
                .map(|d| d.label.as_str()),
            state
                .local_navigation
                .destinations
                .get(3)
                .map(|d| d.label.as_str())
        );
    }

    #[test]
    fn out_of_range_destination_selection_is_a_no_op() {
        let mut nav = LocalNavigationState::simulated();
        nav.select_destination(999);
        assert_eq!(nav.selected_destination, 0);
        assert!(nav.active_destination().is_some());
    }

    #[test]
    fn arrival_and_end_navigation_toggle_the_flow_flags() {
        let mut state = MapsLocationSurface::simulated();
        state.route_preview = true;
        state.simulate_arrival();
        assert!(state.arrived);
        assert!(!state.route_preview);
        assert_eq!(state.active, WorkspaceTab::Drive);

        state.end_navigation();
        assert!(!state.arrived);
        assert!(!state.route_preview);
        assert!(!state.destination_search);
        assert!(!state.off_route);
    }

    #[test]
    fn off_route_toggles() {
        let mut state = MapsLocationSurface::simulated();
        assert!(!state.off_route);
        state.toggle_off_route();
        assert!(state.off_route);
        state.toggle_off_route();
        assert!(!state.off_route);
    }

    #[test]
    fn advanced_menu_partitions_the_full_section_list() {
        // Primary + Advanced must exactly partition ALL (no section lost, none
        // double-counted) so the "Advanced" progressive-disclosure nav still
        // reaches every workspace section.
        assert_eq!(
            WorkspaceTab::PRIMARY.len() + WorkspaceTab::ADVANCED.len(),
            WorkspaceTab::ALL.len()
        );
        for tab in WorkspaceTab::PRIMARY {
            assert!(!tab.is_advanced(), "{tab:?} is a primary surface");
            assert!(!WorkspaceTab::ADVANCED.contains(&tab));
        }
        for tab in WorkspaceTab::ADVANCED {
            assert!(tab.is_advanced(), "{tab:?} nests under Advanced");
            assert!(!WorkspaceTab::PRIMARY.contains(&tab));
        }
    }

    #[test]
    fn advanced_submenu_expands_by_toggle_and_auto_reveals_for_active_child() {
        let mut state = MapsLocationSurface::simulated();
        // Default: on a primary surface, Advanced is collapsed.
        assert_eq!(state.active, WorkspaceTab::Drive);
        assert!(!state.advanced_open());

        // Tapping "Advanced" expands the submenu without changing the content.
        state.toggle_advanced();
        assert!(state.advanced_open());
        assert_eq!(state.active, WorkspaceTab::Drive);
        state.toggle_advanced();
        assert!(!state.advanced_open());

        // Selecting an Advanced child auto-reveals the submenu even when the
        // driver never toggled it, so the highlighted item stays visible.
        state.active = WorkspaceTab::Mg90Settings;
        assert!(!state.advanced_expanded);
        assert!(state.advanced_open());
    }

    #[test]
    fn live_mirror_fold_selects_mg90_gnss_and_drops_simulator_label() {
        use mackes_mesh_types::vehicle::{
            CellLink, GpsFix, VehicleState as WireVehicleState, VehicleTelem, WanStatus,
        };

        // A live gateway with an active cellular uplink but NO GPS lock — the
        // honest "rolling out of the depot before the sky clears" case.
        let mirror = WireVehicleState {
            host: "eagle".to_string(),
            model: "MG90".to_string(),
            esn: "ESN-TEST".to_string(),
            mgos_version: "4.3.0.1".to_string(),
            online: true,
            gps: GpsFix {
                fix_type: "no-fix".to_string(),
                satellites: 0,
                hdop: 99.0,
                ..GpsFix::default()
            },
            imu: None,
            wan: WanStatus {
                active_wan: "Cellular A".to_string(),
                cellular_a: CellLink {
                    sim_state: "ready".to_string(),
                    carrier: "FirstNet".to_string(),
                    signal_dbm: -68,
                    technology: "5G/LTE-A".to_string(),
                    wan_ip: "100.64.0.9".to_string(),
                    healthy: true,
                },
                latency_ms: 31,
                link_quality: "excellent".to_string(),
                ..WanStatus::default()
            },
            telem: VehicleTelem::default(),
            gaps: Vec::new(),
            published_at_ms: test_now_ms(),
        };

        let mut state = MapsLocationSurface::simulated();
        state.refresh_from_vehicle(&mirror);

        // MG90 GNSS is now the primary source, and its label is NOT the
        // Simulator any longer — the whole point of wiring the live gateway.
        assert_eq!(state.locations.primary, LocationSourceKind::Mg90Gnss);
        assert_eq!(state.locations.primary.label(), "MG90 GNSS");
        assert_ne!(
            state.locations.primary.label(),
            LocationSourceKind::Simulator.label()
        );
        assert!(
            !state.simulator_enabled,
            "a live mirror retires the global Simulator indicator"
        );

        // No lock ⇒ the HUD still reports "Acquiring GPS" (`has_fix` false), but
        // the fold populated the MG90 source's live sample from the wire GpsFix.
        let primary = state.locations.primary_source().expect("mg90 source");
        assert!(!primary.sample.has_fix(), "no-fix mirror ⇒ no HUD lock");
        assert_eq!(primary.sample.fix_type, "no-fix");

        // Mg90Status reflects the live cellular uplink.
        assert_eq!(state.mg90.status.active_wan, "Cellular A");
        assert_eq!(state.mg90.status.cellular_a.carrier, "FirstNet");
        assert_eq!(state.mg90.status.cellular_a.signal_dbm, -68);
        assert_eq!(state.mg90.status.link_quality, "excellent");

        // The generic "simulator is active" gap is retracted for a live mirror.
        assert!(
            !state
                .real_hardware_gaps
                .iter()
                .any(|g| g == SIMULATED_MG90_GAP_NOTE),
            "live mirror retracts the simulator gap note"
        );
    }

    #[test]
    fn refresh_from_bus_is_fail_soft_when_no_mirror() {
        // No retained `state/vehicle/<node>` mirror for a bogus node (or no Bus
        // spool at all) ⇒ the simulated seed is left exactly as it was: the
        // honest offline fallback, not an error.
        let mut state = MapsLocationSurface::simulated();
        state.refresh_from_bus("no-such-node-4c1f9e2a");
        assert!(state.simulator_enabled);
        assert!(state
            .real_hardware_gaps
            .iter()
            .any(|g| g == SIMULATED_MG90_GAP_NOTE));
    }

    #[test]
    fn live_bus_vehicle_mirror_drives_car_readouts_and_glance() {
        use crate::car_status::{live_speed_mph, CarStatusItem};
        use mackes_mesh_types::vehicle::{
            CellLink, GpsFix, VehicleState as WireVehicleState, VehicleTelem, WanStatus,
        };

        // Exercise the same retained SQLite row that production reads, rather
        // than calling the typed fold directly. This is temp-spool backed so
        // it never mutates a developer Bus or relies on MDE_BUS_ROOT.
        let dir = tempfile::tempdir().expect("bus dir");
        let persist = mde_bus::persist::Persist::open(dir.path().to_path_buf()).expect("bus");
        let now = test_now_ms();
        let mut mirror = WireVehicleState {
            host: "mg90-live".to_string(),
            model: "MG90".to_string(),
            esn: "ESN-LIVE".to_string(),
            mgos_version: "4.3.0.1".to_string(),
            online: true,
            gps: GpsFix {
                fix_type: "3D".to_string(),
                latitude: 40.4406,
                longitude: -79.9959,
                satellites: 11,
                hdop: 0.8,
                ..GpsFix::default()
            },
            imu: None,
            wan: WanStatus {
                active_wan: "Cellular A".to_string(),
                cellular_a: CellLink {
                    sim_state: "ready".to_string(),
                    carrier: "FirstNet".to_string(),
                    signal_dbm: -68,
                    technology: "5G/LTE-A".to_string(),
                    wan_ip: "100.64.0.9".to_string(),
                    healthy: true,
                },
                latency_ms: 31,
                link_quality: "excellent".to_string(),
                ..WanStatus::default()
            },
            telem: VehicleTelem {
                speed_mph: 48.0,
                battery_v: 13.7,
                moving: true,
                obd_present: true,
                ..VehicleTelem::default()
            },
            gaps: Vec::new(),
            published_at_ms: now,
        };
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic("mg90-live");
        persist
            .write(
                &topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&serde_json::to_string(&mirror).expect("mirror json")),
            )
            .expect("retained vehicle mirror");

        let mut state = MapsLocationSurface::live();
        state.refresh_from_persist(&persist, "mg90-live");
        assert!(state.vehicle.telemetry.is_live());
        assert_eq!(live_speed_mph(&state), Some(48.0));
        assert_eq!(state.vehicle_glance().as_deref(), Some("48 mph"));
        assert_eq!(
            CarStatusItem::SpeedMph.value(&state),
            "48 mph",
            "the driver strip must consume the retained MG90 mirror"
        );
        assert_eq!(
            CarStatusItem::BatteryV.value(&state),
            "13.7 V",
            "telemetry tiles share the live mirror gate"
        );
        assert_eq!(
            state
                .locations
                .primary_source()
                .expect("MG90 source")
                .kind
                .label(),
            "MG90 GNSS"
        );
        assert_eq!(
            state.mg90.status.active_wan, "Cellular A",
            "dashboard connectivity data comes from the same retained row"
        );

        // A retained row that ages beyond the five-second safety window stays
        // provenance-visible but can no longer drive motion, instruments, or
        // the Car home glance.
        mirror.published_at_ms = now - 6_000;
        persist
            .write(
                &topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&serde_json::to_string(&mirror).expect("stale mirror json")),
            )
            .expect("stale retained vehicle mirror");
        state.refresh_from_persist(&persist, "mg90-live");
        assert!(!state.vehicle.telemetry.is_live());
        assert_eq!(live_speed_mph(&state), None);
        assert_eq!(state.vehicle_glance(), None);
    }

    #[test]
    fn latest_json_is_feed_local_and_fail_soft() {
        // The shared Persist handle must let a malformed feed fall out without
        // poisoning the other latest-wins reads in the same refresh.  This is
        // the exact failure mode seen when an adapter is interrupted midway
        // through a JSON publish.
        let dir = tempfile::tempdir().expect("temp bus dir");
        let persist = mde_bus::persist::Persist::open(dir.path().to_path_buf()).expect("bus");
        let good_topic = "state/overlay/test/good";
        let bad_topic = "state/overlay/test/bad";
        persist
            .write(
                good_topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(r#"{"fetched_at":42,"items":[1,2]}"#),
            )
            .expect("good payload");
        persist
            .write(
                bad_topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some("{not-json"),
            )
            .expect("bad payload is still a stored message");

        let good: Option<serde_json::Value> = read_latest_json(&persist, good_topic);
        assert_eq!(
            good.as_ref().and_then(|v| v["fetched_at"].as_i64()),
            Some(42)
        );
        let bad: Option<serde_json::Value> = read_latest_json(&persist, bad_topic);
        assert!(bad.is_none(), "malformed feed must fail soft locally");
    }

    #[test]
    fn earthquake_fold_replaces_snapshot_and_toggle_adds_attribution() {
        use mackes_mesh_types::earthquake::{EarthquakeEvent, EarthquakeSnapshot};

        let mut snapshot = EarthquakeSnapshot::empty("rig-1", test_now_ms());
        snapshot.events.push(EarthquakeEvent {
            id: "ci40659474".to_string(),
            occurred_at_ms: test_now_ms() - 60_000,
            updated_at_ms: test_now_ms(),
            latitude: 35.956,
            longitude: -117.95,
            depth_km: 2.98,
            magnitude: Some(0.53),
            place: "4 km WNW of Little Lake, CA".to_string(),
            pager_alert: None,
            detail_url: None,
        });

        let mut state = MapsLocationSurface::live();
        state.refresh_from_earthquakes(snapshot);
        assert_eq!(
            state
                .map
                .earthquakes
                .snapshot
                .as_ref()
                .expect("snapshot")
                .events
                .len(),
            1
        );
        assert!(!state.map.attribution_line().contains("USGS"));
        state.map.earthquake_overlay = true;
        assert!(state.map.attribution_line().contains("USGS"));
    }

    #[test]
    fn nws_fold_replaces_snapshot_and_toggle_controls_attribution() {
        use mackes_mesh_types::nws_alert::{NwsAlert, NwsAlertSnapshot, NwsSeverity};

        let mut snapshot = NwsAlertSnapshot::empty("rig-1", test_now_ms());
        snapshot.alerts.push(NwsAlert {
            id: "urn:oid:warning".to_string(),
            event: "Tornado Warning".to_string(),
            headline: "Tornado Warning issued".to_string(),
            area_desc: "Test County".to_string(),
            severity: NwsSeverity::Extreme,
            urgency: "Immediate".to_string(),
            certainty: "Observed".to_string(),
            sent_at_ms: Some(test_now_ms() - 60_000),
            expires_at_ms: Some(test_now_ms() + 60_000),
            polygons: Vec::new(),
            geometry_source: None,
        });

        let mut state = MapsLocationSurface::live();
        state.refresh_from_nws_alerts(snapshot);
        assert_eq!(
            state
                .map
                .nws_alerts
                .snapshot
                .as_ref()
                .expect("snapshot")
                .alerts
                .len(),
            1
        );
        // Isolate this layer's attribution from the safety-default NEXRAD layer,
        // whose courtesy line also names NWS.
        state.map.iem_radar_overlay = false;
        assert!(state.map.nws_alert_overlay);
        assert!(state.map.attribution_line().contains("NWS"));
        state.map.nws_alert_overlay = false;
        assert!(!state.map.attribution_line().contains("NWS"));
    }

    #[test]
    fn aircraft_fold_replaces_snapshot_and_toggle_controls_odbl_attribution() {
        use mackes_mesh_types::aircraft::{
            AircraftPositionSource, AircraftSnapshot, AircraftTrack,
        };

        let now = test_now_ms();
        let mut snapshot = AircraftSnapshot::empty("rig-1", now, 40.7128, -74.006, 0.0);
        snapshot.aircraft.push(AircraftTrack {
            id: "aaacc3".to_string(),
            callsign: Some("N123AB".to_string()),
            observed_at_ms: now,
            latitude: 40.70,
            longitude: -74.01,
            altitude_msl_ft: 425.0,
            estimated_agl_ft: 425.0,
            ground_speed_kt: Some(157.9),
            track_deg: Some(206.73),
            position_source: AircraftPositionSource::Adsb,
        });

        let mut state = MapsLocationSurface::live();
        state.refresh_from_aircraft(snapshot);
        assert_eq!(
            state
                .map
                .aircraft
                .snapshot
                .as_ref()
                .expect("snapshot")
                .aircraft
                .len(),
            1
        );
        assert!(!state.map.aircraft_overlay);
        assert!(!state.map.attribution_line().contains("adsb.lol"));
        state.map.aircraft_overlay = true;
        assert!(state.map.attribution_line().contains("adsb.lol"));
        assert!(state.map.attribution_line().contains("ODbL"));
    }

    #[test]
    fn transit_fold_and_toggle_control_massdot_attribution() {
        let now = test_now_ms();
        let snapshot = mackes_mesh_types::transit::TransitSnapshot::empty(
            "rig-1", now, now, "2.0", 42.36, -71.06,
        );
        let mut state = MapsLocationSurface::live();
        state.refresh_from_transit(snapshot);
        assert!(state.map.transit.snapshot.is_some());
        assert!(!state.map.transit_overlay);
        assert!(!state.map.attribution_line().contains("MassDOT"));
        state.map.transit_overlay = true;
        assert!(state.map.attribution_line().contains("MassDOT"));
    }

    #[test]
    fn nws_forecast_fold_and_toggle_control_noaa_attribution() {
        let snapshot = mackes_mesh_types::nws_forecast::NwsForecastSnapshot::unavailable(
            "rig-1",
            "no fresh fix",
        );
        let mut state = MapsLocationSurface::live();
        state.refresh_from_nws_forecast(snapshot);
        // Isolate this layer's attribution from the safety-default NEXRAD layer,
        // whose courtesy line also names NOAA.
        state.map.iem_radar_overlay = false;
        assert!(state.map.nws_forecast.snapshot.is_some());
        assert!(!state.map.nws_forecast_overlay);
        assert!(!state.map.attribution_line().contains("NOAA"));
        state.map.nws_forecast_overlay = true;
        assert!(state.map.attribution_line().contains("NOAA"));
    }

    #[test]
    fn caltrans_camera_fold_and_toggle_control_attribution() {
        let snapshot = mackes_mesh_types::caltrans_camera::CaltransCameraSnapshot::empty(
            "rig-1", 3, 1, 38.481, -121.511,
        );
        let mut state = MapsLocationSurface::live();
        state.refresh_from_caltrans_cameras(snapshot);
        assert!(state.map.caltrans_cameras.snapshot.is_some());
        assert!(!state.map.caltrans_camera_overlay);
        assert!(!state.map.attribution_line().contains("Caltrans"));
        state.map.caltrans_camera_overlay = true;
        assert!(state.map.attribution_line().contains("Caltrans CWWP2"));
    }

    #[test]
    fn iem_radar_fold_and_safety_default_control_attribution() {
        let snapshot =
            mackes_mesh_types::iem_radar::IemRadarSnapshot::empty("rig-1", 1, 42.36, -71.06);
        let mut state = MapsLocationSurface::live();
        state.refresh_from_iem_radar(snapshot);
        assert!(state.map.iem_radar.snapshot.is_some());
        assert!(state.map.iem_radar_overlay);
        assert!(state.map.attribution_line().contains("IEM"));
        state.map.iem_radar_overlay = false;
        assert!(!state.map.attribution_line().contains("NEXRAD"));
    }

    #[test]
    fn wildfire_fold_and_safety_default_control_attribution() {
        let snapshot =
            mackes_mesh_types::wildfire::WildfireSnapshot::empty("rig-1", 1, 44.0, -120.0, 200);
        let mut state = MapsLocationSurface::live();
        state.refresh_from_wildfire(snapshot);
        assert!(state.map.wildfire.snapshot.is_some());
        assert!(state.map.wildfire_overlay);
        assert!(state.map.attribution_line().contains("NIFC WFIGS"));
        state.map.wildfire_overlay = false;
        assert!(!state.map.attribution_line().contains("NIFC WFIGS"));
    }

    #[test]
    fn traffic_fold_and_regional_toggle_control_attribution() {
        let snapshot =
            mackes_mesh_types::traffic::TrafficSnapshot::empty("rig-1", 1, 35.7, -78.65, 100);
        let mut state = MapsLocationSurface::live();
        state.refresh_from_traffic(snapshot);
        assert!(state.map.traffic_events.snapshot.is_some());
        assert!(!state.map.traffic_event_overlay);
        assert!(!state.map.attribution_line().contains("NCDOT"));
        state.map.traffic_event_overlay = true;
        assert!(state
            .map
            .attribution_line()
            .contains("NCDOT DriveNC / TIMS"));
    }

    #[test]
    fn air_quality_fold_and_ambient_toggle_control_epa_attribution() {
        let snapshot = mackes_mesh_types::air_quality::AirQualitySnapshot::unconfigured("rig-1", 1);
        let mut state = MapsLocationSurface::live();
        state.refresh_from_air_quality(snapshot);
        assert!(state.map.air_quality.snapshot.is_some());
        assert!(!state.map.air_quality_overlay);
        assert!(!state.map.attribution_line().contains("US EPA AirNow"));
        state.map.air_quality_overlay = true;
        assert!(state
            .map
            .attribution_line()
            .contains("US EPA AirNow (preliminary)"));
    }

    // ── WL-UX-007/S1 — production simulator removal ─────────────────────────
    // PLATFORM-INTERFACES P8/Q33 + operator directive 2026-07-22: the
    // production constructor carries NO fabricated data of any kind.

    #[test]
    fn live_surface_is_empty_of_fabricated_data() {
        let s = MapsLocationSurface::live();

        assert!(!s.simulator_enabled, "no simulator in production");
        assert!(s.airspace.signals.is_empty(), "zero airspace contacts");
        assert!(!s.airspace.active, "airspace scanning idle until focused");
        assert!(s.trips.breadcrumbs.is_empty(), "no fabricated trip history");
        assert!(s.dead_zones.zones.is_empty(), "no fabricated dead zones");
        assert!(
            s.local_navigation.destinations.is_empty(),
            "no preset destinations — only real geocoding adds them"
        );
        assert!(s.local_navigation.route_options.is_empty());
        assert!(!s.local_navigation.navigating);
        assert!(!s.local_navigation.active_route.is_planned());
        assert!(s.mg90.settings.is_empty(), "no fabricated descriptors");
        assert!(s.mg90.backups.is_empty(), "no fabricated restore points");
        assert!(!s.mg90.authenticated);
        assert_eq!(s.mg90.setup_step, SetupStep::NotConnected);
        assert!(s.devices.gpio_rules.is_empty());
        assert!(s.devices.usb_devices.is_empty());
        assert!(s.firmware.checks.is_empty(), "no pre-passed checks");
        assert!(!s.firmware.restore_point_ready);
        assert!(
            !s.mg90.reset.armed(),
            "reset disarmed without a real backup"
        );

        // Vehicle telemetry is absent and never claims live.
        assert!(!s.vehicle.telemetry.is_live());
        assert!(s.vehicle.telemetry.fuel_percent.is_none());
        assert!(s.vehicle.telemetry.odometer_mi.is_none());

        // The MG90 GNSS primary is armed but source-less: no fix, no fake
        // coordinates, and no fabricated peer sources.
        let primary = s.locations.primary_source().expect("mg90 primary");
        assert_eq!(primary.kind, LocationSourceKind::Mg90Gnss);
        assert!(!primary.sample.has_fix());
        assert_eq!(primary.sample.fix_type, "No fix");
        assert!(primary.sample.latitude.abs() < f64::EPSILON);
        assert_eq!(primary.status, SourceStatus::Disconnected);
        assert_eq!(s.locations.sources.len(), 1, "MG90 GNSS only");

        // No live WAN is claimed.
        assert!(s.mg90.status.active_wan.is_empty());
        assert!(s.mg90.status.active_cellular_link().is_none());

        // The honest gap report leads with the awaiting-mirror note.
        assert!(s
            .real_hardware_gaps
            .iter()
            .any(|g| g == AWAITING_MIRROR_GAP_NOTE));
    }

    #[test]
    fn live_offline_maps_reflect_disk_not_fixtures() {
        // No region installed ⇒ the honest not-installed state, never the
        // fixture's "Default state/province region".
        let none = OfflineMapManagerState::from_installed(None);
        assert!(none.installed_regions.is_empty());
        assert!(none.available_regions.is_empty(), "no fabricated downloads");
        assert!(none.default_region.is_empty());
        assert!(none.used_gb.abs() < f32::EPSILON);
        assert!(none.loaded_region().is_none());

        // A really-installed region directory is reported from disk.
        let dir = tempfile::tempdir().expect("tempdir");
        let region = dir.path().join("east-texas");
        std::fs::create_dir(&region).expect("region dir");
        std::fs::write(region.join("east-texas.mbtiles"), vec![0u8; 4096]).expect("mbtiles");
        let installed = OfflineMapManagerState::from_installed(Some(region));
        assert_eq!(installed.default_region, "east-texas");
        assert_eq!(installed.installed_regions.len(), 1);
        assert_eq!(installed.installed_regions[0].status, RegionStatus::Loaded);
    }

    #[test]
    fn live_mirror_fold_goes_live_from_the_live_seed() {
        use mackes_mesh_types::vehicle::{
            CellLink, GpsFix, VehicleState as WireVehicleState, VehicleTelem, WanStatus,
        };

        let mut s = MapsLocationSurface::live();
        let mirror = WireVehicleState {
            host: "eagle".to_string(),
            model: "MG90".to_string(),
            esn: "ESN-TEST".to_string(),
            mgos_version: "4.3.0.1".to_string(),
            online: true,
            gps: GpsFix {
                fix_type: "3D".to_string(),
                latitude: 40.4406,
                longitude: -79.9959,
                satellites: 11,
                hdop: 0.8,
                ..GpsFix::default()
            },
            imu: None,
            wan: WanStatus {
                active_wan: "Cellular A".to_string(),
                cellular_a: CellLink {
                    sim_state: "ready".to_string(),
                    carrier: "FirstNet".to_string(),
                    signal_dbm: -68,
                    technology: "5G/LTE-A".to_string(),
                    wan_ip: "100.64.0.9".to_string(),
                    healthy: true,
                },
                latency_ms: 31,
                link_quality: "excellent".to_string(),
                ..WanStatus::default()
            },
            telem: VehicleTelem {
                speed_mph: 42.0,
                battery_v: 13.8,
                ..VehicleTelem::default()
            },
            gaps: Vec::new(),
            published_at_ms: test_now_ms(),
        };
        s.refresh_from_vehicle(&mirror);

        // The fold works from the live seed exactly as from the fixture: the
        // MG90 source connects, gains the wire fix, and telemetry goes live.
        let primary = s.locations.primary_source().expect("mg90 source");
        assert_eq!(primary.status, SourceStatus::Connected);
        assert!(primary.sample.has_fix());
        assert!(s.vehicle.telemetry.is_live());
        assert_eq!(s.mg90.status.active_wan, "Cellular A");
        // The awaiting-mirror gap retracts once the mirror is live.
        assert!(!s
            .real_hardware_gaps
            .iter()
            .any(|g| g == AWAITING_MIRROR_GAP_NOTE));
    }

    #[test]
    fn stale_vehicle_telemetry_cannot_drive_motion_or_glance_state() {
        use mackes_mesh_types::vehicle::{VehicleState as WireVehicleState, VehicleTelem};

        // A fresh online OBD sample remains usable even while GNSS has no fix:
        // telemetry freshness and position-lock readiness are independent.
        let mut mirror = WireVehicleState::offline("eagle");
        mirror.online = true;
        mirror.model = "MG90".to_string();
        mirror.mgos_version = "4.3.0.1".to_string();
        mirror.gaps.clear();
        mirror.telem = VehicleTelem {
            speed_mph: 42.0,
            battery_v: 13.8,
            moving: true,
            obd_present: true,
            ..VehicleTelem::default()
        };
        mirror.published_at_ms = test_now_ms();

        let mut state = MapsLocationSurface::live();
        state.refresh_from_vehicle(&mirror);
        assert!(!state
            .locations
            .primary_sample()
            .expect("MG90 source")
            .has_fix());
        assert!(state.vehicle.telemetry.is_live());
        assert!(
            state.moving(),
            "fresh live motion may drive the safety guard"
        );
        assert_eq!(state.vehicle_glance().as_deref(), Some("42 mph"));

        // Re-folding the same retained payload after its timestamp expires must
        // age both telemetry and GNSS. A last-known `moving=true` can no longer
        // hold motion/safety state active or keep the glance card populated.
        mirror.published_at_ms = test_now_ms() - 6_000;
        state.refresh_from_vehicle(&mirror);
        assert!(state.vehicle.telemetry.has_live_gateway_source());
        assert!(!state.vehicle.telemetry.is_live());
        assert!(state
            .locations
            .primary_sample()
            .expect("MG90 source")
            .stale());
        assert!(!state.moving(), "stale motion must fail safe to parked");
        assert_eq!(state.vehicle_glance(), None);
    }

    #[test]
    fn start_navigation_is_a_no_op_without_route_options() {
        // Without a routing engine there is no route: Start must never flip the
        // HUD into guidance over a fabricated empty maneuver banner.
        let mut s = MapsLocationSurface::live();
        s.route_preview = true;
        s.start_navigation();
        assert!(!s.local_navigation.navigating);
        assert!(s.route_preview, "stays on the preview, honestly routeless");
    }

    #[test]
    fn live_readiness_is_blocked_never_fabricated_ready() {
        // A fresh live seat (no mirror, no routing engine, usually no maps)
        // must not claim turn-by-turn readiness.
        let s = MapsLocationSurface::live();
        let status = s.offline_navigation_status();
        assert_eq!(status.readiness, OfflineNavigationReadiness::Blocked);
        assert!(!status.can_claim_turn_by_turn());
        assert!(!status.notes.iter().any(|n| n.contains("Simulator fixture")));
    }
}

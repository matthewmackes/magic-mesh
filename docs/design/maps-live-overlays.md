# Maps & Location — live-data overlay catalog (OVERLAY)

**Status:** locked 2026-07-22 via operator scoping survey + 3-agent codebase recon +
a 14-candidate live-verification workflow (`wf_6731d411-455`; every keyless API was
actually fetched from the control host and sample payloads inspected). Operator
rulings this session: **external public feeds emphasized**, **vehicle/MG90 rolling-node
lens first**, **zero-cost only** ("remove items that incur a cost" — no paid tiers, no
non-commercial license locks that become pay-or-rip-out items if Quazar ships).

This is the design catalog for `WL-FUNC-012` (Maps live-data overlays). No overlay
code lands with this doc.

---

## 0. Scope and operator rulings

- The Maps & Location cockpit (`crates/desktop/mde-maps-location-egui`) grows real
  live-data overlays fed by external public APIs, painted on the map scene and
  toggled from the existing Map-tab layer checkboxes.
- Primary lens: the vehicle in motion (MG90 rolling node) — driver situational
  awareness: what is on the road ahead, weather, hazards.
- **Zero-cost constraint (operator, 2026-07-22):** every feed in the catalog must be
  free of charge now AND at commercial shipping time. That means US-government
  public-domain data, free government developer keys, or free open-data standards.
  Feeds with paid tiers or non-commercial-only free tiers were removed (§4).
- The cockpit's existing fake overlays (cyan "weather" rect, single orange "traffic"
  line in `paint_map_scene`) are replaced by real data as part of this epic.

## 1. Current surface (grounded recon, 2026-07-22)

- **The map is synthetic, not geographic.** `paint_map_scene`
  (`crates/desktop/mde-maps-location-egui/src/view.rs:2069`) draws a hand-painted
  perspective scene in normalized `(u,v)` space via `scene_point` (`view.rs:2058`).
  There is no tile system and no lat/lon→screen projection anywhere;
  `LocationSample.latitude/longitude` (`model.rs:1975`) feeds only text readouts and
  the chevron heading.
- **Overlay toggles already exist as stubs.** `MapViewState` (`model.rs:759`) carries
  `route_visible / traffic_overlay / weather_overlay / dead_zone_overlay /
  gnss_overlay`, checkboxes in `show_map` (`view.rs:2936`), gated paint blocks at
  `view.rs:2115-2176`. `LocalNavigationState` already declares `ProviderContract`
  seams for `traffic` / `weather` / `satellite` as `graceful_unavailable`.
- **The live-adapter recipe is proven** by the MG90 vehicle worker
  (`crates/mesh/mackesd/src/workers/vehicle.rs`): injectable probe trait
  (`VehicleProbe`, `:74`) → tolerant fold with honest `gaps` (`build_state`, `:316`)
  → `bus_publish::publish_json` to a latest-wins topic (`state/vehicle/<node>`),
  ~5 s poll doubling as heartbeat, unset env ⇒ no-op idle. The cockpit polls
  `Persist::read_latest` throttled to 2 Hz (`refresh_from_bus`, `model.rs:466`),
  fail-soft to the simulated seed. Every overlay adapter clones this shape.
- **Style/test conventions:** colors from `mde-egui` `Style::*` tokens or a
  `// style-leak-ok: map-content-color` const (`view.rs:22-46`); icons via
  `paint_carbon(...)` with procedural fallback; crash-safety idioms (`finite_or`,
  `safe_rect`); headless tessellation smoke tests (`view.rs:4949`); `FakeProbe`
  fixture tests for adapters.
- Only the vehicle carries real lat/lon today. Mesh-node geo placement
  (peers' `external_addr` GeoIP, lighthouse `public_ip`, DO region slugs) is a
  separate future "mesh on map" overlay, out of scope here.

## 2. OVERLAY-0 — the shared prerequisite: vehicle-centered projection

All ten feeds return real lat/lon geometry; none can be painted until the scene
gains a projection. Design:

- A **local-tangent-plane transform** `geo_to_uv(fix: &GpsFix, lat, lon) -> (u, v)`:
  meters-from-vehicle via equirectangular approximation
  (`x = dlon * cos(lat) * 111_320`, `y = dlat * 110_540`), scaled by a per-zoom
  meters-per-UV factor, rotated by heading in Drive mode (track-up), then fed to the
  existing `scene_point`. Error is well under 1% at a <=50 km radius — exactly the
  vehicle-cockpit regime. No mercator/tile stack required for vector overlays.
- Raster-tile overlays (radar, §3.2) need slippy-tile math (Web Mercator z/x/y →
  scene quad) and an LRU tile cache; the nine vector feeds do NOT need it.
  **Coordination (2026-07-22):** the parallel Maps workstream
  (`docs/design/maps-worldclass-plan.md`) builds this exact lane as its P2 raster
  basemap unit, and it established the decisive render constraint: the DRM seat
  runs `egui_glow` (GLES), windowed runs wgpu, so tiles MUST land as
  raster→egui-texture quads (the `carbon_texture` cache pattern,
  `mde-egui/src/carbon.rs:316`), never a wgpu paint callback. OVERLAY-2 REUSES that
  tile lane rather than duplicating it — if radar lands first, it builds the lane
  under the same constraint and the basemap inherits it.
- Feeds are fetched by bounding box around the vehicle fix (or along the route
  polyline), so fetch and projection share the same locality assumption.
- Entities beyond the scene radius clamp to edge-of-scene direction chips (the
  Airspace radar's bearing math, `airspace.rs:641`, is the in-repo precedent).
- NaN/pole/date-line guards follow the existing `finite_or` idiom; a fix older than
  the staleness threshold freezes the projection origin rather than jumping.

## 3. The catalog — ten zero-cost live feeds (ranked, live-verified)

Rank order is the vehicle lens: driver value first, then feed quality, integration
cost, and license safety. Every entry was verified 2026-07-22; "verified" lines
describe what actually came back.

### 3.1 NWS severe-weather alerts — `api.weather.gov` (rank 1)

- **Paints:** severity-tinted translucent warning polygons (red Tornado, orange
  SvrTstm, green Flood…), tap-for-headline card, and an alert banner when the
  vehicle fix is inside a Warning polygon. Upgrades `weather_overlay`.
- **Verified:** point query returned an active alert with full CAP properties;
  nationwide `/alerts/active` returned 541 features, 64 with inline polygons,
  including a Severe Thunderstorm Warning minutes-fresh (sent 06:09, expires 06:19).
- **Endpoints:** `GET /alerts/active?point={lat},{lon}` (vehicle position);
  `?area={ST}` for state-wide look-ahead; `GET /zones/{type}/{id}` to resolve the
  ~88% of alerts whose inline geometry is null (zone shapes are static — cache
  forever). Filters: `?severity=`, `?urgency=`, `?event=`.
- **Format/cadence:** GeoJSON CAP FeatureCollection; warnings appear within
  seconds-to-1-minute of issuance. ETag/If-Modified-Since honored (most polls 304).
- **Poll:** 30-60 s on the point query; refresh on >5-10 km movement.
- **License:** US-gov public domain. Keyless. **A descriptive User-Agent is
  mandatory — NWS silently blocks without one.**
- **Risks:** must implement the affectedZones fallback or the layer looks empty;
  5xx spikes during major outbreaks (retry/backoff + stale-tolerance);
  severity can be "Unknown" — style for it.

### 3.2 Precipitation radar tiles — IEM NOAA NEXRAD (rank 2)

- **Paints:** alpha-blended radar raster (~40-60% opacity) under the vehicle marker,
  6-12 frame animation/scrub, on-map age badge. Replaces the fake cyan weather rect.
  Upgrades `weather_overlay`.
- **Verified:** IEM `nexrad-n0q-900913` z6 tile returned 200 `image/png` 7.4 KB with
  actual reflectivity paint; `-m05m..-m55m` history layers give 5-min-step animation.
- **Endpoints:**
  `https://mesonet.agron.iastate.edu/cache/tile.py/1.0.0/nexrad-n0q-900913/{z}/{x}/{y}.png`
  (+ `-m{05..55}m` frames). Slippy XYZ Web-Mercator PNG, transparent background.
- **Format/cadence:** raster tiles; mosaic ~5 min behind radar volume scans, 5-min
  cache headers. A moving map at one zoom pulls only ~4-12 tiles per refresh.
- **Poll:** check the frame clock every 60 s; fetch tiles only when a new frame
  lands; LRU cache keyed by frame+z/x/y. Radar latency (5-10 min) must be surfaced
  in the UI (age badge) or drivers over-trust it.
- **License:** NEXRAD data is US-gov public domain; IEM tiles are a free university
  courtesy service (stable 15+ years, no SLA) — cache hard, set a real User-Agent,
  courtesy attribution "IEM/NWS". CONUS+AK/HI/PR coverage.
- **Removed alternative:** RainViewer (global composite) is mid-sunset and its free
  tier is personal/educational-only — excluded by the zero-cost rule (§4).

### 3.3 State-511 DOT traffic events, closures, roadwork (rank 3)

- **Paints:** incident/closure/roadwork Carbon-icon markers with severity tint and
  tap-for-description card; closure segments where `MapEncodedPolyline` is present.
  Makes `traffic_overlay` real.
- **Verified:** `https://511ny.org/api/getevents?format=json` returned 200, 2.8 MB,
  2,290 statewide events (1,653 roadwork, 283 closures, 15 accidents/incidents…)
  with lat/lon, EventType/SubType, Severity, RoadwayName, LanesStatus; freshest
  incident stamps within ~5 min (TRANSCOM-sourced).
- **Endpoints:** `511ny.org/api/getevents|getalerts|getwinterroadconditions` (free
  developer key; the same TravelIQ/Castle-Rock API shape covers many states —
  511GA, 511WI, FL511…); state ArcGIS FeatureServer portals offer true bbox GeoJSON;
  FHWA WZDx work-zone feeds are a standards-based supplement.
- **Format/cadence:** plain JSON point arrays (occasional encoded polyline);
  incidents minutes-fresh, roadwork hourly-to-daily. Statewide blob → poll whole +
  client-side bbox filter.
- **Poll:** 120 s per state feed (511NY documents ~10 calls/min); adapter selects
  the state feed from the vehicle GPS.
- **License:** free public-sector feeds under a developer access agreement;
  attribution customary. **Honest trade-off of the zero-cost rule:** no
  probe-derived live *flow* coloring (that was TomTom, §4) — this layer shows
  events/closures/roadwork, not green/amber/red congestion lines.
- **Risks:** keyless 511NY access is undocumented behavior — register the free key;
  DD/MM/YYYY date strings and stringified-JSON `LanesStatus` are parse traps;
  per-state fragmentation = a config-table entry per state (start with 2-3 states).

### 3.4 NWS gridpoint drive-ahead route forecast (rank 4)

- **Paints:** severity glyphs (rain/fog/gust icons) at 8-12 points sampled along the
  route polyline (or projected heading, 50-100 km ahead), sized by ETA proximity;
  optional weather ribbon along the route. Upgrades `weather_overlay`.
- **Basis:** replaces Open-Meteo (removed — non-commercial free tier, §4) with the
  NWS forecast API the research named as its natural keyless companion:
  `GET /points/{lat},{lon}` → cached gridpoint URL → `forecastHourly` (2.5 km grid).
  Public domain, keyless, same User-Agent rule and 5xx caveats as §3.1.
- **Format/cadence:** GeoJSON properties with hourly periods; underlying grids
  refresh ~hourly. `/points` lookups are static per 2.5 km cell — cache them.
- **Poll:** one pass over the sampled route points every 5-10 min, spread out to
  stay polite; re-sample on route change or ~5 km of travel.
- **Trade-off vs the removed feed:** no 15-minutely nowcast granularity — hourly
  steps only, US-only coverage. Radar (§3.2) carries the "next 30 minutes" story.

### 3.5 DOT traffic cameras — Caltrans keyless pilot + 511 family (rank 5)

- **Paints:** clustered camera pins; auto-surfaced thumbnail cards for the next 2-3
  route-ahead cameras with age badges; tap for full view. New toggle.
- **Verified:** Caltrans D3 `cctvStatusD03.json` (838 KB, keyless): per-camera
  lat/lon, route/postmile, `inService`, snapshot JPEG (Last-Modified 38 s old at
  fetch) + HLS `playlist.m3u8`; 511NY `getcameras`: 2,926 point records.
- **Endpoints:** `cwwp2.dot.ca.gov/data/d{1..12}/cctv/cctvStatusD{01..12}.json`
  (keyless); `511ny.org/api/getcameras` (free key; Castle-Rock family); WSDOT
  REST (free AccessCode); state ArcGIS portals (bbox GeoJSON).
- **Cadence/poll:** camera list every 10-15 min; corridor snapshot JPEGs 30-60 s for
  <=10 cameras within ~5 mi of the route ahead; streams only on user tap.
- **License:** free government data; attribution per state ToS; some states restrict
  video rebroadcast more than stills (cockpit display is fine).
- **Risks:** fragmentation is the real cost — one adapter table entry per state,
  3-4 API dialects; endpoints churn when states re-let 511 vendor contracts; filter
  zombie/disabled cameras; trust the image Last-Modified, not record metadata.

### 3.6 Wildfire perimeters + hotspots — NIFC WFIGS + NASA FIRMS (rank 6)

- **Paints:** translucent red-orange perimeter polygons with containment-tinted
  outline; FRP-sized, age-faded heat dots; staleness badge. New toggle.
- **Verified:** NIFC WFIGS Current Perimeters keyless bbox query with `f=geojson`
  returned genuine MultiPolygon rings (e.g. "Morrill" 642,029 acres, 100%
  contained). FIRMS docs confirmed URL grammar + 5,000 transactions/10 min budget
  (key required for row samples — free, email-only signup).
- **Endpoints:** NIFC
  `services3.arcgis.com/T4QMspbfLg3qTGWY/.../WFIGS_Interagency_Perimeters_Current/FeatureServer/0/query`
  (bbox envelope, `f=geojson`; sibling incident-points layer); FIRMS
  `/api/area/csv/{MAP_KEY}/{SOURCE}/{bbox}/{days}` with SOURCE from
  `data_availability` (VIIRS/MODIS ~2-6 h effective per-location refresh;
  `GOES_NRT` ~10 min but coarse).
- **Poll:** FIRMS 5 min (optional 60 s GOES fast lane); NIFC perimeters 10-15 min;
  bbox ~100-200 km around the vehicle.
- **License:** NASA open data (any use incl. commercial; cite "NASA FIRMS"; show the
  NRT "not for safety-of-life decisions" disclaimer in UI copy); NIFC/WFIGS
  US-gov open data. Free key only.
- **Risks:** ArcGIS layer names have churned — keep the layer name configurable;
  FIRMS sources rotate as satellites retire — query `data_availability` at startup;
  hotspots are thermal anomalies (flares/industrial false positives — filter by
  confidence); request server-side geometry simplification for megafire polygons.

### 3.7 Air quality stations — AirNow (rank 7)

- **Paints:** EPA-palette circle markers (green→maroon), optional coarse IDW tint
  wash, and a banner chip promoted only when a nearby station is >=150 AQI (smoke
  season is when this layer earns its pixels). New toggle.
- **Verified:** live AirNow reporting-area probe returned same-morning PM2.5/Ozone
  observations for a metro area; official API confirmed key-gated (free EPA key,
  immediate email issue).
- **Endpoints:** `airnowapi.org/aq/data/?BBOX=...&parameters=PM25,OZONE&dataType=A`
  (key); point query `/aq/observation/latLong/current/`. OpenAQ v3 (CC-BY, free
  key) only if operation ever leaves the US.
- **Cadence/poll:** hourly observations posting 30-90 min after the hour → poll
  10-15 min, re-fetch on >50 km movement. Hard cap 500 req/hr per key — ample; issue
  per-deployment keys so vehicles never share one.
- **License:** US EPA / public domain; attribution "US EPA AirNow"; data flagged
  preliminary — occasional single-monitor spikes.
- **Risks:** AirNow servers are slow exactly during major wildfire events — build
  timeouts + last-good-value caching; grey markers past 2 h, drop past 6 h.

### 3.8 ADS-B live aircraft — adsb.lol (rank 8)

- **Paints:** heading-rotated, altitude-tinted aircraft icons with optional callsign
  labels, dead-reckoned between polls from `gs`+`track` so motion stays smooth at
  the 2 Hz repaint. Ship a "low-altitude only" filter (<3,000 ft AGL, 10 nm) as the
  one driver-relevant view (helicopter-overhead ≈ incident proxy). New toggle.
- **Verified:** `api.adsb.lol/v2/point/40.7128/-74.0060/50` returned 90 aircraft,
  `seen_pos` mostly 0.0-0.2 s — genuinely ~1 s fresh, keyless, no UA tricks.
- **Endpoints:** `/v2/point/{lat}/{lon}/{radius_nm}` (readsb/tar1090 `aircraft.json`
  schema; plain JSON points).
- **Poll:** every 3 s at 30-50 nm radius (community guidance ~1 req/s max — never
  the cockpit's 2 Hz); back off to 15-30 s when the layer is hidden.
- **License:** ODbL open data (attribution line; share-alike only for derived
  databases — a cockpit overlay is fine). Community-run, no SLA.
- **Fallbacks (zero-cost rule):** airplanes.live and OpenSky both carry
  non-commercial clauses — NOT ship-safe fallbacks. The honest mitigation is the
  shared readsb schema: any tar1090 host is schema-identical, and a $30 RTL-SDR
  dongle on the vehicle gives zero-internet local ADS-B with the same JSON — a
  natural future MG90 lane for this offline-first platform.
- **Risks:** `alt_baro` can be the string `"ground"`; MLAT/TISB positions are
  coarser; markers age out (fade >30 s, drop >60 s) — no stale-cache hazard.

### 3.9 GTFS-Realtime transit vehicle positions (rank 9)

- **Paints:** route-colored bearing-rotated chevrons with occupancy tint, greyed
  past 60 s, layer empties on TTL. Off by default; per-region config. New toggle.
- **Verified:** MBTA `VehiclePositions.pb` = 626 vehicles, header 25 s old,
  per-vehicle stamps 7-36 s old, keyless; MTA NYCT bus feed same-minute fresh.
- **Endpoints:** per-agency protobuf snapshots (no bbox anywhere — whole-fleet
  download, 70-230 KB, filter client-side), e.g.
  `cdn.mbta.com/realtime/VehiclePositions.pb`, `gtfsrt.prod.obanyc.com/vehiclePositions`;
  the Mobility Database catalogs ~2,500 agency URLs. Decode via `prost` /
  `gtfs-realtime` crate.
- **Poll:** 15-30 s per configured agency (data regenerates ~15 s).
- **License:** open-data terms per agency (MBTA/MTA free, attribution); the config
  table records each agency's terms.
- **Risks:** whole-fleet payloads on a cellular link — enable per-region agencies
  only, and run the fetch workstation-side (§6); URL churn per agency; proto v1.0
  and v2.0 both live; static-GTFS join needed for route names/colors.

### 3.10 USGS earthquakes (rank 10)

- **Paints:** magnitude-scaled circles colored by PAGER alert, fading over 24 h;
  optional toast for M5+ within N km of route. Ambient layer — sits empty for
  months outside CA/NV/AK/HI/OK. New toggle. (Promoted from runners-up when the
  lightning slot was removed for licensing, §4.)
- **Verified:** `all_hour.geojson` fetched live: 200, 3.8 KB, generated <60 s before
  fetch, 6 events; fdsnws bbox query verified keyless; `cache-control: max-age=60`.
- **Endpoints:** canned summaries
  `earthquake.usgs.gov/earthquakes/feed/v1.0/summary/{mag}_{window}.geojson`;
  `fdsnws/event/1/query?format=geojson&min/maxlat/lon=...&updatedafter=` for bbox.
- **Poll:** 60 s (matches server cache; faster returns the same cached body).
- **License:** US-gov public domain; the intended usage pattern IS a 1/min poller.
- **Risks:** lowest-risk API in the catalog (v1.0 unchanged a decade+). Handle
  event revisions/deletes by keying on `id` + `updated`; third GeoJSON coordinate
  is depth-km (trips naive parsers).

## 4. Removed for cost (operator rule, 2026-07-22)

| Feed | Why removed | What replaces it | What would re-enable it |
|---|---|---|---|
| TomTom / HERE traffic flow + incidents | Paid product; free tier capped 2,500 req/day and pricing revision effective July 2026 | State-511 events/closures (§3.3) — accepting the loss of probe-derived flow coloring | A paid TomTom/HERE contract at shipping time |
| Open-Meteo route forecast | Free tier is explicitly NON-COMMERCIAL (verified on terms page) → forced paid plan if Quazar ships | NWS gridpoint hourly forecast (§3.4), public domain | Paying Open-Meteo, or accepting non-commercial status |
| Blitzortung lightning | Free of charge but non-commercial community lock + reverse-engineered protocol → pay-Vaisala-or-rip-out on shipping | No direct replacement in the 10; radar (§3.2) + NWS alerts (§3.1) carry the storm story. Future free lane: NOAA GOES-GLM gridded lightning (public domain) if we render it ourselves | Vaisala/ENTLN contract, or a self-rendered GLM lane |
| RainViewer radar (as primary) | Free tier personal/educational-only + announced API sunset (nowcast already dead on live fetch) | IEM NEXRAD tiles (§3.2), public domain | n/a — IEM is strictly better for US operation |

Also excluded on the same rule when picking fallbacks: airplanes.live and OpenSky
(non-commercial clauses — see §3.8), Amtraker (informal licensing; and GTFS-RT
covers transit), aisstream.io (key-gated hobby beta, no ToS/SLA).

## 5. Runners-up (cataloged, not in the 10)

- **NOAA SWPC aurora/Kp** — flawless keyless public-domain API, but scenic garnish;
  the useful piece is a tiny Kp/G-scale chip as a GPS-degradation caveat for the
  MG90's own fix — that belongs on the Location Sources tab as a status badge, not
  an overlay slot.
- **Amtrak (Amtraker v3)** — keyless and fresh (~68 s median) but a community rehost
  with informal licensing; intercity-rail garnish behind GTFS-RT.
- **NOAA GOES-GLM lightning** — the future zero-cost lightning lane (public domain)
  if we ever invest in rendering gridded products ourselves.

## 6. Shared adapter architecture and cross-cutting rules

```
external API ──HTTP poll / stream──▶ mackesd overlay worker (one module per feed)
                                      │ bbox around state/vehicle/<node> fix
                                      │ honest gaps; backoff; no-op when unconfigured
                                      ▼
                      state/overlay/<feed>/<node>   (latest-wins JSON snapshot,
                                      │              normalized geometry + fetched_at)
                                      ▼
              cockpit fold: throttled read_latest → per-feed model (2 Hz)
                                      ▼
              paint_map_scene block gated on MapViewState.<feed>_overlay
                                      + checkbox in show_map
```

Rules every unit follows (from the cross-cutting synthesis):

1. **Poll at the feed's cadence, never the cockpit's.** 2 Hz is bus-repaint cadence.
   Feed cadences range 3 s (ADS-B) to 15 min (camera lists). Every snapshot carries
   `fetched_at`; the paint layer derives age from it.
2. **One staleness pattern, shared:** keep painting the last snapshot with a growing
   age badge; grey/fade past a per-feed threshold (radar ~20 min, traffic ~10 min,
   forecast ~60 min, cameras ~5 min); never render stale data as live. Snapshot
   (latest-wins) topics make reconnect resync free.
3. **License tier is a config field** on every feed entry (`public-domain`,
   `free-key-gov`, `open-data-attribution`) so a future `/release` audit is a grep.
   The §4 table is the precedent for what fails the audit.
4. **Shared plumbing:** descriptive User-Agent everywhere (NWS blocks silently
   without it); If-Modified-Since/ETag conditional GETs; exponential backoff;
   free keys (FIRMS, AirNow, 511 states) in mde-seal, scrubbed from logs; LRU
   tile/blob cache for the radar and camera-image lanes.
5. **Bandwidth lives workstation-side.** Heavyweight pulls (2.8 MB 511 blobs,
   200 KB GTFS snapshots, camera JPEG fans) run on the workstation adapter and
   publish trimmed bus snapshots — never on the MG90 cellular WAN.
6. **Fail-soft like the vehicle worker:** unset config ⇒ idle no-op; fetch failure ⇒
   keep last mirror + `gaps` note; the cockpit keeps its simulated seed when no
   mirror exists.

## 7. Layer-toggle UX

- The Map tab's `horizontal_wrapped` checkbox row (`show_map`, `view.rs:2936`) grows
  one checkbox per feed, same `MapViewState` bool pattern. With ~13 toggles the row
  wraps into a grouped "Layers" popover (Weather / Road / Hazards / Ambient groups)
  — a small UX unit, not a framework.
- Drive HUD shows at most the safety-relevant layers by default (alerts, radar,
  511 events, route weather); ambient layers (ADS-B, transit, quakes, AQI) default
  off in Drive, remembered per mode.
- Each active layer contributes an attribution line to the existing map attribution
  string (`MapViewState.attribution`) — NWS/IEM/NIFC/EPA/ODbL courtesy strings.

## 8. Implementation units (tracked as WL-FUNC-012)

- **OVERLAY-0** — `geo_to_uv` projection + edge-chip clamping + tests (prereq for all).
- **OVERLAY-1** — NWS alerts adapter + polygon paint + in-warning banner.
- **OVERLAY-2** — radar tile lane: slippy math + LRU cache + frame animation (the
  one raster unit; heaviest).
- **OVERLAY-3** — 511 events adapter (state table, NY first) + incident markers.
- **OVERLAY-4** — NWS gridpoint route-forecast sampler + route glyphs.
- **OVERLAY-5** — camera pins + corridor thumbnail lane (Caltrans first).
- **OVERLAY-6** — wildfire (NIFC + FIRMS) merged layer.
- **OVERLAY-7** — AirNow stations + >=150 banner.
- **OVERLAY-8** — ADS-B layer + dead-reckoning + low-altitude filter.
- **OVERLAY-9** — GTFS-RT layer + per-region agency config.
- **OVERLAY-10** — USGS quakes ambient layer.
- **OVERLAY-11** — layers popover UX + per-mode defaults + attribution lines.

Units 1-10 are independent after OVERLAY-0 — natural farm fan-out, one worker per
unit, disjoint files per the serialize-same-file rule (all touch
`MapViewState`/`show_map`/`paint_map_scene` at merge points — land those three-line
hooks serially on the integration branch, bodies in per-feed modules).
OVERLAY-2 additionally coordinates with the world-class plan's P2 raster-basemap
unit (shared tile lane, §2); and the whole hook-landing pipeline serializes behind
the in-flight `maps-worldclass-plan.md` P0/P1 units, which are already queued on
the same `view.rs`/`model.rs` files.

## 9. Verification

- Per-adapter: `FakeProbe`-style fixture tests with captured live payloads (the
  research's verified samples define the fixtures), including malformed/partial
  cases folding to `gaps`.
- Per-layer: tessellation smoke tests (every layer on, NaN fix, tiny viewport) per
  the existing `view.rs` pattern.
- Live: deploy to a seat (.15/.138) with real feeds and SSH-verify the
  `state/overlay/*` mirrors carry fresh `fetched_at` stamps + visually confirm
  paint — green unit tests are not "fixed" (live-verify-deploys rule).
- License audit: grep the feed config for license tiers; §4 documents the failures.

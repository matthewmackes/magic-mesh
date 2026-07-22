# Maps & Location → world-class, built-for-purpose — build plan

Grounded scope from a read-only investigation (2026-07-22) of
`crates/desktop/mde-maps-location-egui/` (`model.rs` 2966L, `view.rs` 5426L, `car_status.rs`,
`airspace.rs`) + the shell embed in `mde-shell-egui/src/main.rs`. Data flows:
`simulated()` fixture (`model.rs:140`) → `refresh_from_bus()` (`model.rs:467`) folds the retained
`state/vehicle/<node>` mirror via `refresh_from_vehicle()` (`model.rs:345`), called every frame by
the shell (`main.rs:1445-1449`). Wire GPS = `mackes_mesh_types::vehicle::GpsFix`; indoors it is the
honest sparse case (`fix_type="no-fix"`, `satellites=0`, `hdop=99`, `lat/lon=0`).

**DECISIVE RENDER CONSTRAINT:** the DRM seat runs **`egui_glow` (GLES)**, windowed runs `wgpu`
(`mde-egui/Cargo.toml`). A wgpu paint-callback vector map would only work windowed and silently
break the in-vehicle seat. → basemap MUST be **raster tiles → egui texture** (a plain textured
quad works on both backends). Reject wgpu-vector for v1.

## Root cause of "looks like fake data" (the dominant one is NOT sparse data)
- **1a (dominant):** `LocationManager::simulated()` seeds ALL sources — including the PRIMARY
  `Mg90Gnss` — at a hard-coded Pittsburgh fix (`40.4406,-79.9959`, 27mph, "US-30 W", "patrol
  staging", ETA "14:32", "Heavy rain…") (`model.rs:1764-1775`, `959-969`). `drive_hud()` paints
  that fake route UNCONDITIONALLY every frame (`view.rs:585-648`), and **Car Mode drops the header
  + tab-rail** (`view.rs:62-77`) — the only "simulated" markers. So Car Mode shows a full guided
  route to a fake destination with zero "this is fixture" signal.
- **1b:** the map chevron honestly gates on `has_fix()` ("Acquiring GPS" — already good,
  `view.rs:610-618`), but the `CarStatus` instrument strip does NOT (`car_status.rs:190-260`):
  unfixed GPS renders `0.0000` coords, `0°`, `0` sats, fabricated `495 m` accuracy; and the
  **`bars(0)==5` bug** (`0 >= -70`, `car_status.rs:295-305`) shows an absent cell signal as FULL
  bars. Empty carrier/latency/loss don't `empty_dash`.

## Prioritized build units
| # | Unit | Crate(s) | Effort | Notes |
|---|------|----------|--------|-------|
| **P0** | **Sparse-data honesty pass** — gate `CarStatus` GPS/cell tiles on `has_fix`/empty → `"—"`/"No fix"; fix `bars(0)=5`; add idle-Drive (no-route) state; un-hideable "SIMULATED" ribbon in Car Mode; retire fixture coords from the primary source when no live mirror | maps (`car_status.rs`,`view.rs`,`model.rs`) | S–M (2–3d) | Kills "looks fake", **no new deps** |
| **P0** | **Mode-button fix** — direct labeled Car↔Desktop toggle, distinct glyph, de-conflict the lower-right corner vs the maps FAB stack | shell `main.rs` (`mount_layout_profile_control`/`layout_mode_*`) + maps `view.rs` FAB placement | S (<1d) | Not dead code — a UX/corner-collision + "single tap only opens a menu" problem. 5s press-hold also opens it (undiscoverable). |
| **P1** | **Address entry UI** — real `egui::TextEdit` in `show_destination_search` (`view.rs:1223`, currently click does nothing), live results, add `lat/lon` to `Destination` | maps `view.rs`,`model.rs` | S–M (1–2d) | Removes "cannot enter addresses" |
| **P1** | **Offline geocoder** — bundled SQLite **FTS5** gazetteer via `rusqlite` (already in-tree) behind the `ProviderContract` seam (`model.rs:935`); house-number/street match. (Full Nominatim=PostGIS, reject.) | maps + `client_data_dir` | M (2–4d) | + region pipeline |
| **P2** | **Raster basemap** — MBTiles(`rusqlite`) or `pmtiles`; Web-Mercator `(z,x,y)` math; decode via in-tree `image`; cache via the `carbon_texture` pattern (`mde-egui/carbon.rs:316`); replace the procedural grid/road splines (`view.rs:2091-2113`) in `paint_map_scene`, keep chevron/route on top | maps + `mde-egui` | M (3–5d) | RASTER (glow+wgpu), NOT wgpu-vector |
| **P3** | **Offline region-build tooling** (shared by P1+P2): OSM extract → styled dark raster MBTiles + gazetteer DB per region → `client_data_dir/maps/<region>.*` | tooling | M | One-time pipeline |
| Later | Real offline routing (Valhalla) + optional mesh geocode service over the Bus (`action/geocode`→`state/geocode`) | — | L | True turn-by-turn / fleet gazetteer |

**Reusable seams:** texture cache `mde-egui/src/carbon.rs:316`; storage dir `mde-bus/src/lib.rs:103`;
SQLite `rusqlite` (`mde-bus/src/persist.rs`); `image` decode (in-tree); wire registry
`mackes-mesh-types` for any meshed geocode/route.

**Serialization note:** P0-sparse / P1-address / P2-basemap all touch maps `view.rs`+`model.rs` →
they SERIALIZE (a pipeline, not a fan-out). The shell half of P0-mode-button parallelizes; its maps
`view.rs` FAB de-conflict serializes with the others. The 2026-07-22 "Advanced menu + floating
buttons" operator directives land first (also `view.rs`/`model.rs`), then this pipeline.

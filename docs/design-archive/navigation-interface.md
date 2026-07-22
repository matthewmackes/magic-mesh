> **HISTORICAL / SUPERSEDED (2026-07-22):** interface-paradigm design retired by the PLATFORM-INTERFACES standard (Apple-HIG-principled Construct + Car); see [docs/design/platform-interfaces.md](../design/platform-interfaces.md). Archived; do not implement from this document.

# World-class navigation interface — design plan

**Goal:** a world-class in-vehicle navigation interface in `mde-maps-location-egui`,
matching the Android **Waze** + **Google Maps Navigation** design language, skinned in the
platform's Quasar-dark Carbon palette. Driver-first: glanceable at speed, large type, high
contrast, minimal chrome, full-bleed map with floating elevated cards.

## Design system (the visual grammar)

The universally-recognized nav grammar Waze and Google Maps have converged on:

- **Full-bleed map** — the map fills the surface edge-to-edge; every control floats over it
  as a rounded, drop-shadow-elevated card. Never a split panel.
- **Top maneuver banner** — a solid blue (GMaps) instruction bar: large directional turn
  arrow + big distance + street name + road-after. The primary focal point.
- **Bottom ETA sheet** — arrival time (traffic-colored green/amber/red) + remaining
  time · distance.
- **Bottom-left speedometer + speed-limit sign** — round speed chip (amber/red over limit)
  beside a round white / red-ring / black-number limit sign.
- **Floating action buttons** — right edge, circular Material FABs (recenter, mute,
  overview/report).
- **Route** — layered: soft under-glow + dark casing + bright core stroke, rounded joints.
- **Vehicle** — heading-aware chevron with a soft accuracy "flashlight" cone; grey +
  "Acquiring GPS" when no fix.
- **Palette** — Quasar-dark surfaces, GMaps-blue route/banner accent, Carbon icons
  (`assets/icons/Mackes-Carbon`). **Type** — large, Roboto-like, high contrast.
- **Motion** — smooth recenter/zoom; no jitter; the map pans under a fixed vehicle anchor
  in guidance mode.

## The full navigation experience (the flows)

A world-class nav interface is more than the turn-by-turn HUD — it's the whole journey:

1. **Destination search / "Where to?"** — a prominent search entry, recent + favorite
   destinations, categories (home/work/fuel/food). *(Model: `Destination` list exists.)*
2. **Route preview** — the proposed route on an overview map + route options (fastest /
   alternate), each with ETA · distance · traffic; a large "Start / GO" button. *(Model:
   `RoutePlan` + alternates.)*
3. **Turn-by-turn Drive HUD** — the guidance screen (maneuver banner + speedometer + ETA
   sheet + chevron + layered route). **✅ built** — Phase 1.
4. **Lane guidance** — lane-arrow strip under the maneuver banner for multi-lane maneuvers.
5. **Arrival** — arrival card + "You have arrived" + parking/destination detail.
6. **Off-route / re-route** — a recalculating state + updated route.
7. **Map (explore) tab** — pan/zoom/layers, the same beautiful scene, non-guidance.
8. **Routes & Trips** — saved trips, recents, favorites, trip recorder + dead-zone map.

## Roadmap

- **Phase 1 — Drive HUD ✅** (`61293f20`, deployed `.15`, crash-safe, 27 tests). The
  turn-by-turn guidance screen in the full Waze/GMaps grammar.
- **Phase 2 — refine + explore-map + route-preview**: self-critique the Drive HUD against
  Waze/GMaps (proportions, arrow/chevron shapes, banner/sheet sizing, color), extend the
  world-class scene to the **Map** tab (layer chips, recenter/compass FAB), and build the
  **route-preview** screen (overview + options + Start). Lane-guidance strip.
- **Phase 3 — destination search + arrival + re-route**: the "Where to?" search/recents
  surface, the arrival card, and the off-route/recalculating state — completing the flow
  from "where to?" → preview → guidance → arrival.

## Critical, always

- **Crash-safety** (fixed the prior `widget_rect.rs:163` panic): `has_fix` no-fix state,
  `finite_or`/`safe_rect`/`safe_width` guards on every allocated/painted rect, unique
  stable widget Ids. Any new surface follows the same rules.
- **Verification**: farm-build the maps + shell crates green + per-mode tests (incl.
  no-fix / small-viewport / NaN inputs); deploy to the physical `.15` seat; the beauty bar
  is judged on the dash against how Waze/GMaps actually look.

## Key files

- `crates/desktop/mde-maps-location-egui/src/view.rs` — the surface (Drive HUD +
  `paint_map_scene`/`paint_route`/`paint_vehicle_chevron` + the floating-card painters).
- `crates/desktop/mde-maps-location-egui/src/model.rs` — `RoutePlan`, `VehicleTelemetry`,
  `LocationSample` (+ `has_fix`), `MapViewState`, `Destination`, `TripRecorderState`,
  `ManeuverKind`.

# WL-FUNC-017 navigation Start authority — r533

Date: 2026-08-13

## Production slice

Maps previously treated every generation-valid daemon `Active` route projection
as if the operator had pressed **Start**. Route calculation therefore skipped the
preview and entered guidance automatically. The surface now records the exact
daemon route identity for which Start was admitted:

- a newly calculated route opens route preview and does not start guidance;
- Start authorizes only the exact projected route identity;
- later progress generations for that same route retain guidance;
- cancellation, a new route request, a non-active projection, or a replacement
  route revokes the prior Start authority; and
- a replacement route returns to preview and cannot inherit guidance authority.

Rendering remains I/O-free and consumes only the existing generation-validated
daemon navigation projection. The adjacent stale renderer comment was corrected
to describe the already-live geometry authority.

## Farm gates

- BigBoy `172.20.0.130`, slot `func017-nav-start-build-r2`:
  `cargo check -p mde-maps-location-egui --all-targets` — passed.
- `172.20.0.170`, slot `func017-nav-start-clippy-r2`:
  `cargo clippy -p mde-maps-location-egui --all-targets -- -D warnings` — passed.
- `172.20.0.90`, slot `func017-nav-replace-test-r2`:
  `model::tests::calculated_route_requires_explicit_start_and_same_route_progress_retains_guidance`
  — passed 1/1.
- `172.20.0.90`, same warmed slot:
  `model::tests::replacement_route_returns_to_preview_until_started` — passed 1/1.
- `172.20.0.196`, slot `func017-nav-start-fmt-r2`:
  `cargo fmt -p mde-maps-location-egui -- --check` — passed.
- Local non-build check: `git diff --check` — passed.

The initial `.50` test attempt was stopped without evidence when farm capacity
reported only 8.6 GiB free; no passing claim is based on that attempt. Earlier
zero-test exact-filter runs are likewise excluded from the evidence above.

## Remaining WL-FUNC-017 acceptance

Pre-release work still includes the governed production offline map region and
provider inputs, any remaining concrete MG90 manager/radio adapter gaps, and a
complete audit of map-first weather/navigation action wiring. Deferred
post-release acceptance includes live NWS/NOAA and MG90 hardware behavior,
offline/reconnect navigation traces, direct-DRM Maps/Car and Bottom/Left launcher
captures, physical radio/GNSS recovery, packaging/upgrade evidence, and the
reduced one-node restart/rejoin/recovery proof.

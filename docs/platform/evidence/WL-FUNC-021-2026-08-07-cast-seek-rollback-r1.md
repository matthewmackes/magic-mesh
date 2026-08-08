# WL-FUNC-021 cast seek rollback — 2026-08-07

## Material finding

`NetworkCaster::cast_dlna` started the renderer with `Play` before applying a
non-zero resume position. A rejected or unreachable `Seek` therefore returned a
typed failure while leaving the renderer playing from the wrong position. This
was a real S5 cast correctness gap, independent of Dell availability.

## Production change

`crates/desktop/mde-media-core/src/cast.rs` now preserves the original `Seek`
failure and sends a bounded best-effort DLNA `Stop` rollback after `Play` has
already succeeded. A rollback failure is not substituted for the useful seek
error. The new `soap_stop` builder is covered by the network fixture.

## Fixture proof

`cast::tests::failed_dlna_seek_stops_renderer_before_reporting_rejection` runs a
local TCP renderer fixture that accepts:

1. device description;
2. `SetAVTransportURI`;
3. `Play`;
4. a `500 Seek Not Supported` response; and
5. the compensating `Stop` request.

The test asserts the returned error names `Seek` and that the fifth request is
the `Stop` SOAP action.

## Farm gates

- `.90`, `MCNF_BUILD_SLOT=media-cast-suite-r1`:
  `cargo test -p mde-media-core cast::tests -- --nocapture` — **27 passed, 0 failed**.
- `.90`, `MCNF_BUILD_SLOT=media-cast-seek-final-r1`:
  `cargo test -p mde-media-core failed_dlna_seek_stops_renderer_before_reporting_rejection -- --nocapture` — **1 passed, 0 failed**.
- `.50`, `MCNF_BUILD_SLOT=media-cast-fmt-r3`: file-scoped
  `rustfmt --edition 2021 --check crates/desktop/mde-media-core/src/cast.rs` — **passed**.

The package-wide formatter remains unsuitable as a gate for this isolated
change because dirty, pre-existing `roaming.rs` edits are not rustfmt-clean;
the changed file itself is clean.

## Remaining live proof

Dell remains unreachable (`No route to host`), and no physical DLNA renderer is
available on the farm. The live renderer cast, Chromecast/mesh receiver path,
and installed two-seat owner-yield/handoff proof therefore remain open; this
fixture proves only the source-level rollback behavior.

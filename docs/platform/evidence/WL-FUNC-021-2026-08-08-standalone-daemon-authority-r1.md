# WL-FUNC-021 — standalone Music daemon authority (2026-08-08)

## Acceptance slice

The earliest incomplete code-level requirement after the existing S1 catalog
and S2 playback fixture evidence was S3's requirement that the GUI have no
competing worker, store, or playback authority. The prior UI-authority evidence
explicitly retained a standalone Airsonic/audio worker fallback.

Both public `MusicApp` construction paths now create the same daemon-projected
surface. The standalone review window does not load provider credentials or
start the legacy provider/playback worker. It reads retained `mde-bus` workspace
state, and without a host-installed authenticated action publisher it reports
transport mutations unavailable instead of falling back to GUI-owned playback.
The daemon and Bus remain the catalog, queue, and playback authorities.

## Hostile regression

`standalone_constructor_refuses_hostile_worker_playback_state` constructs the
public standalone surface, proves that no command sender or worker authority is
active, injects stale `Started` and `Playing(true)` compatibility updates into
the bounded queue, pumps the surface, and proves that neither now-playing nor
playing state changes.

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=func021-standalone-daemon-authority-r1 \
install-helpers/xcp-build.sh cargo test --locked -p mde-music-egui \
  standalone_constructor_refuses_hostile_worker_playback_state -- --nocapture
```

Result: 1 passed, 0 failed, 68 filtered out. The package-scoped farm format
check initially identified two assertion wraps; after that mechanical correction,
the same `.50` slot passed:

```text
cargo fmt -p mde-music-egui -- --check
```

## Boundary

This closes the production-constructor fallback seam only. The quarantined
legacy worker implementation remains as cleanup debt, but it is not started by
either public constructor and its queued updates are inert in daemon-projected
mode. Live physical renderer and cross-seat handoff proof remain separate
hardware acceptance work under FUNC-021.

## Source integrity

```text
49b7b9d031e76c8641555cbe59077bd18d4e2a1015e95203fb7ab94009ed8f91  crates/desktop/mde-music-egui/src/app.rs
9c7dd9c466db1e6753d385a800aecbfb0fb44577f62e6d50667c896ecede3171  crates/desktop/mde-music-egui/src/lib.rs
eb7bd79a3d962c8f0bf714c2238add28d106d14400538a972b1cb19bdc74c088  crates/desktop/mde-music-egui/Cargo.toml
```

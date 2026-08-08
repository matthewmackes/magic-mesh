# WL-FUNC-021 Music authority sidecar — 2026-08-06

## Scope

Audited the Music daemon/UI authority boundary across `mde-musicd`'s typed
workspace contract and `mde-music-egui`'s app/model/worker seams. The daemon
workspace snapshot is the retained catalog/queue read authority; the legacy
Airsonic worker remains a bounded standalone compatibility path only while no
daemon snapshot has been accepted.

## Correction

`crates/desktop/mde-music-egui/src/app.rs` now latches catalog presentation to
the daemon after a valid workspace snapshot is present:

- Home renders an honest unavailable state for an empty daemon snapshot instead
  of revealing legacy Airsonic album/starred rows.
- Search does not send a legacy worker request when daemon state is present but
  the authenticated daemon browse writer is unavailable.

The hostile regressions seed legacy album/starred/search state alongside an
empty daemon snapshot and verify that the legacy catalog does not reappear.

## Farm verification

- `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=authority-sidecar-test-r1 install-helpers/xcp-build.sh cargo test -p mde-music-egui --lib`
  - `50 passed; 0 failed; 0 ignored; 0 measured`
- `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=authority-sidecar-fmt-r2 install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check`
  - exit `0`

## Remaining blocker

`WL-FUNC-021` still requires live provider/network-loss, hardware
audio/video, DLNA/mesh-cast, seat, and RPM acceptance evidence. This sidecar
does not claim those runtime gates.

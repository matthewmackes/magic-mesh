# WL-FUNC-021 real mpv fixture evidence — 2026-08-06

## Farm gate

- Farm host: `172.20.0.130` (BigBoy), slot
  `music-real-mpv-r1`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-real-mpv-r1
  ./install-helpers/xcp-build.sh cargo test -p mde-media-core --features mpv
  --test mpv_fixture_decode -- --nocapture`.
- Result: `fixture_decodes_through_real_mpv_with_a_nonblank_frame` passed;
  `1 passed, 0 failed`.
- The test compiled and exercised `MpvEngine`, not `FakeMpv`, from the checked-in
  `tests/fixtures/tiny_clip.mkv` fixture. It reached a playable/ended state,
  observed a non-empty mpv `current-ao` property (the test permits the typed
  `null` fallback), and captured a nonblank frame with nonzero content checksum.

## Boundary

This closes the farm real-engine/nonblank-frame fixture gate for S2. It does not
claim a physical-seat audio path, live provider playback, seek/end/network-loss
recovery, DRM presentation, or operator-reviewed visual acceptance; those remain
separate live and hardware gates.

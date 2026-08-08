# WL-FUNC-021 — two-catalog outage playback evidence — 2026-08-06

## Scope

This slice covers the daemon-owned source-candidate and native decoder seam for
one logical Music queue track. It does not claim a physical provider outage or
mid-track resume.

## Delivered behavior

- `source_aware_upcoming_candidates` retains two admitted provider variants in
  policy order instead of collapsing a merged catalog row to one client.
- `EngineHandle::play_from_candidates` keeps those candidates under one
  `PlaybackTrack`, so a failed first provider does not create a duplicate queue
  boundary.
- A loopback HTTP fixture returns `503` for catalog A and valid finite PCM WAV
  bytes for catalog B. The engine refuses the first source, decodes the second,
  enqueues audio, and records exactly one logical track start.
- The existing network-loss policy remains bounded: a fallback is allowed only
  before audio has been emitted; post-audio provider failure is not replayed
  from byte zero.

## Farm verification

- `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=musicd-two-catalog-r1 install-helpers/xcp-build.sh cargo test -p mde-musicd two_catalog_outage_uses_next_admitted_source_once_without_duplicate_boundary -- --nocapture`
  passed **1/1**; the fixture observed `/catalog-a` followed by `/catalog-b`.
- `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=musicd-candidates-r1 install-helpers/xcp-build.sh cargo test -p mde-musicd source_aware_playback_keeps_two_catalog_candidates_under_one_queue_track -- --nocapture`
  passed **1/1**.
- `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=musicd-two-catalog-full-r1 install-helpers/xcp-build.sh cargo test -p mde-musicd --lib -- --nocapture`
  passed **173/173** unit tests.
- `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=musicd-two-catalog-fmt-r3 install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check`
  passed.
- `git diff --check` passed.

## Proof boundary

The fixture proves the local source-selection and decoder behavior only. Live
two-server outage playback, network reconnect, mid-track resume/range support,
PipeWire/DRM output, Dell/seat-15 acceptance, and RPM promotion remain open.

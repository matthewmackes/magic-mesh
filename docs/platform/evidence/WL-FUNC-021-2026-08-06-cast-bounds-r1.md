# WL-FUNC-021 / Media Player cast bounds and resume handoff — 2026-08-06

Status: implementation slice; the owning Music/Media epics remain `Remaining`.

## Delivered

`crates/desktop/mde-media-core/src/cast.rs` now keeps the typed renderer/caster
boundary bounded and preserves an admitted resume position for real DLNA/UPnP
casts:

- mesh, SSDP, mDNS, and merged renderer discovery retain at most 64 targets;
- one SSDP probe collects at most 64 KiB and ignores discovery fields over 1 KiB;
- a cast request rejects empty, non-finite, negative, or over-seven-day resume
  positions before opening a renderer connection;
- DLNA `SetAVTransportURI` and `Play` are followed by a typed `Seek` using the
  finite `REL_TIME` position when the request resumes past zero;
- a non-2xx `Seek` response is rejected, so the surface cannot claim a
  position-continuous cast when the renderer did not accept it.
- a loopback renderer fixture exercises the complete description lookup and
  ordered `SetAVTransportURI` → `Play` → `Seek` exchange, including escaped
  media/title fields and accepted HTTP status responses.

The existing mesh-node and Chromecast paths remain typed `Gated` outcomes. The
DLNA path remains the only network throw implemented here; discovery or fixture
evidence does not imply a live renderer or hardware success.

## Focused hostile coverage

- `hostile_ssdp_backlog_is_bounded_and_oversized_fields_are_ignored` proves a
  backlog larger than the retention limit produces exactly 64 targets and does
  not retain the oversized friendly-name field.
- `invalid_resume_positions_fail_before_network_access` proves malformed
  floating-point and out-of-range positions fail at the typed boundary.
- `seek_envelope_preserves_a_bounded_resume_position` proves the DLNA time
  conversion and `REL_TIME` envelope.
- `dlna_cast_fixture_requires_description_and_ordered_soap_acceptance` proves
  the live caster does not report success until a renderer accepts all three
  ordered SOAP actions.
- Existing HTTP/SOAP, discovery, unavailable-renderer, and typed-gate tests
  remain green.

## Farm verification

- `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-cast-r2 install-helpers/xcp-build.sh cargo test -p mde-media-core cast -- --nocapture`
  passed **17/17** focused cast tests (prior revision).
- `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-cast-fixture-r1 install-helpers/xcp-build.sh cargo test -p mde-media-core dlna_cast_fixture_requires_description_and_ordered_soap_acceptance -- --nocapture`
  passed **1/1** end-to-end loopback fixture test.
- `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-cast-full-r4 install-helpers/xcp-build.sh cargo test -p mde-media-core --lib -- --nocapture`
  passed **240/240** unit tests and no failures.
- `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-cast-fmt-r4 install-helpers/xcp-build.sh cargo fmt -p mde-media-core -- --check`
  passed.
- Local `git diff --check` passed. Source SHA-256:
  `88a0322908efa89b35e9a639d62d6023bc6e65e11e3bf3e5eead1b3cc6daf24a`.

## Remaining acceptance

Live DLNA discovery/control, position-continuous seat transfer, two-catalog
Music outage playback, live Jellyfin credentials/server playback, direct DRM,
GUI-worker removal, deterministic render captures, and Dell/seat-15 hardware
acceptance remain open. The implementation does not claim those external
proofs or production promotion.

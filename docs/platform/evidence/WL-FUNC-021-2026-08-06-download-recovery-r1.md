# WL-FUNC-021 evidence — interrupted download recovery (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

The Music daemon now reconciles durable download state once at startup. Any
record left in `downloading` after process loss becomes a redacted
`download_interrupted` failure with zero claimed bytes, no expected total, and
no pin claim. Completed records are preserved, the transition is written using
the existing same-directory atomic retained-state writer, and a second recovery
pass is a no-op. The Downloaded workspace therefore cannot show phantom
progress or imply that a partial file is resumable when the cache writer only
installs complete content.

The daemon remains the sole download/cache authority; no GUI worker, new Bus
topic, or alternate persistence store was introduced.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-recovery-r1 \
  bash install-helpers/xcp-build.sh \
  cargo test -p mde-musicd --lib \
  interrupted_download_recovery_clears_phantom_progress
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-recovery-r2 \
  bash install-helpers/xcp-build.sh cargo test -p mde-musicd --lib
result: 150 passed, 0 failed

ssh mm@172.20.0.50 \
  'cd /home/mm/magic-mesh-farm-music-recovery-r2 && \
   rustfmt --check --edition 2021 \
   crates/services/mde-musicd/src/bus_responder.rs'
result: pass
```

The focused regression is
`bus_responder::tests::interrupted_download_recovery_clears_phantom_progress`.
The full suite also preserves the existing source fan-out, cache fallback,
typed download lifecycle, workspace revision, MPRIS, and catalog tests.

## Remaining proof

This closes only restart-state honesty. Live two-catalog playback, network-loss
audio on approved hardware, target/DLNA handoff, GUI-worker removal, direct DRM,
and Dell/seat-15 acceptance remain open under WL-FUNC-021 and WL-CRIT-006.

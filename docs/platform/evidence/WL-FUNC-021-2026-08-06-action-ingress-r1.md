# WL-FUNC-021 evidence — bounded Music action ingress (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

Every Music action poller—queue control, typed workspace mutations, transport,
browse/search, and peer/handoff verbs—now reads at most 64 retained messages
per topic per sweep through `Persist::list_since_limit`. The exclusive ULID
cursor advances only through that page, so restart/recovery and delayed
provider calls cannot materialize a retained topic's entire history into the
single-threaded daemon process. The existing one Music daemon, Bus topics,
authorization boundary, and reply correlation remain authoritative.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-ingress-r1 \
  bash install-helpers/xcp-build.sh \
  cargo test -p mde-musicd --lib \
  queue_action_recovery_reads_a_bounded_page_and_advances_the_cursor
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-ingress-r1 \
  bash install-helpers/xcp-build.sh cargo test -p mde-musicd --lib
result: 151 passed, 0 failed

ssh mm@172.20.0.90 \
  'cd /home/mm/magic-mesh-farm-music-ingress-r1 && \
   rustfmt --check --edition 2021 \
   crates/services/mde-musicd/src/bus_responder.rs'
result: pass
```

The focused regression is
`bus_responder::tests::queue_action_recovery_reads_a_bounded_page_and_advances_the_cursor`.

## Remaining proof

Bounded admission does not close live two-catalog playback, provider retry
acceptance, target/DLNA handoff, GUI-worker removal, direct DRM, or
Dell/seat-15 evidence. Those remain `Remaining` under WL-FUNC-021 and
WL-CRIT-006.

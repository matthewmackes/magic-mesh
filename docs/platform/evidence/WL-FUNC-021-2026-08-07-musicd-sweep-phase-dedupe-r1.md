# WL-FUNC-021 — Music daemon sweep phase and idle projection dedupe (2026-08-07)

## Finding

Each `mde-musicd` instance entered the same fixed 500 ms Bus/provider sweep at
service start. In addition, the five-second workspace timer serialized and
published an unchanged idle projection on every seat. Those synchronized wakes
and retained-index writes were a common-mode CPU/I/O amplifier even when no
Music action or playback state had changed.

## Change

- Added a deterministic FNV-1a host phase bounded below 251 ms before the first
  full sweep. The regular 500 ms cadence and stop predicate are unchanged.
- Workspace revisions now compare projections without their monotonic revision
  field and skip the retained JSON/index write when the idle projection is
  unchanged. Real queue, playback, source, cache, target, or catalog changes
  still publish a new revision.

## Verification

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=musicd-phase-r2 \
  install-helpers/xcp-build.sh cargo test --locked -p mde-musicd \
  bus_responder -- --nocapture
```

Result: 56 passed, 0 failed, 131 filtered out. Coverage includes stable/bounded
host phase and revision-insensitive projection deduplication.

This is source/farm evidence only. Live five-seat CPU sampling and post-restart
NWS/provider-loss acceptance remain open while the authorized Dell endpoints
are unreachable.

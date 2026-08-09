# WL-FUNC-021 / WL-ARCH-009 media-source Bus replacement recovery r81

Date: 2026-08-09

## Result

`MediaSourcesWorker` now opens a fresh Bus transaction on every bounded poll and
accepts it only when `index.sqlite` has the same device/inode identity before
and after SQLite opens. The worker verifies that identity again after each
publication. A same-path replacement therefore forces the complete current
mesh/gateway/mDNS fold into the new index without restarting discovery; a race
during publication clears the fingerprint gate and retries on the next tick.
Late/unopenable storage remains retryable and does not discard in-memory mDNS
state. The retired initial `Persist` handle is dropped after activation.

## Focused farm verification

Farm: machine196 (`172.20.0.196`)

Slot: `media-sources-bus-r81`

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=media-sources-bus-r81 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  same_path_bus_replacement_receives_current_fold_without_worker_restart \
  --locked -- --nocapture

# 1 passed; 0 failed; 4584 filtered out

MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=media-sources-bus-r81 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  late_bus_recovers_and_publishes_sources_without_worker_restart \
  --locked -- --nocapture

# 1 passed; 0 failed; 4585 filtered out
```

Farm file-scoped `rustfmt --check` identified one wrapping-only difference; it
was applied before the final source sync and both exact tests above passed the
final source. Scoped `git diff --check` also passed.

## Boundary

This proves publication recovery with real Bus indexes and an isolated peers
plane. It does not claim a live multicast service, Jellyfin server, media
render, or seat deployment.

## Source hash

```text
378e2959c08b75383e51d831187b96f3b7564c4be1b29f23279a024275a42e75  crates/mesh/mackesd/src/workers/media_sources.rs
```

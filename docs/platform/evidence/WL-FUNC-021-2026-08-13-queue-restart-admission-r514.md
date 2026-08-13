# WL-FUNC-021 — bounded queue restart admission (r514)

Date: 2026-08-13

## Production result

`mde-musicd` now applies the same queue authority bounds at restart that it
applies to live mutations. Durable queue input is read through a 256 KiB
ceiling and fails closed when it contains more than 512 entries, blank or
oversized song identities, an out-of-range cursor, or a preferred provider
identity that does not name the exact current track. Legacy version-zero queues
remain supported only when they satisfy those invariants.

This closes a restart-integrity gap where an externally corrupted or stale
queue file could bypass live mutation admission and restore incoherent queue or
provider authority.

## Farm evidence

- `.90`, slot `func021-queue-r514`: `cargo test -p mde-musicd
  restart_refuses_unbounded_or_incoherent_durable_queue_authority -- --nocapture`
  passed 1/1 (270 filtered).
- `.170`, slot `func021-queue-r514-clippy`: `cargo clippy -p mde-musicd
  --lib -- -D warnings` passed.
- `.50`, slot `func021-queue-r514-fmt`: file-scoped `rustfmt --edition 2021
  --check crates/services/mde-musicd/src/queue.rs` passed. Package-wide format
  exposed unrelated existing drift and is not claimed.
- `git diff --check` passed before commit.

BigBoy was intentionally unused. No live/post-release acceptance is claimed.

## Remaining acceptance

First-release package integration remains, followed by the operator-deferred,
non-blocking installed-seat provider loss/switching, queue continuity, restart,
and audible playback proof.

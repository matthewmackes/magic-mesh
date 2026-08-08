# WL-FUNC-021 — mde-musicd reconnect-loop audit (2026-08-06)

## Scope

This audit covers `crates/services/mde-musicd/src/reconnect.rs` and the
read-only engine call sites that consume its schedule and timeout constants.
No engine or worklist files were edited. The existing dirty changes in
`reconnect.rs` were preserved.

## Findings

- `reconnect_after_loss` in `engine.rs` caps mid-track recovery at
  `MAX_MIDTRACK_RECONNECTS = 3`.
- Each reconnect attempt waits on the bounded `1s, 2s, 4s` schedule from
  `backoff_delay_secs`, with an interruptible 100 ms sleep slice that checks
  the stop flag. There is no zero-delay retry path for the production
  constants (`DEFAULT_BASE_SECS = 1`, `DEFAULT_CAP_SECS = 60`).
- Resumed stream requests use the existing finite budgets from `reconnect.rs`:
  a 3-second connect timeout and a 30-second total request timeout. The
  engine regression fixture verifies that a provider which stalls after
  headers is rejected rather than pinning the decode thread.
- The initial live/radio stream intentionally has no body deadline because
  its body is expected to remain open. It is not a reconnect retry loop and
  is outside this scoped audit; changing it would require an engine change
  and would break live-stream playback semantics.
- The bounded decoder back-pressure wait is an 8 ms sleep while the ring is
  above its target, and it exits when stop is signalled. No unbounded busy
  polling or retry-driven CPU spike was found in the reconnect path.

## Verification

Farm lane `.50`, slot `music-reconnect-audit-r1`:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-reconnect-audit-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd reconnect --locked -- --nocapture
```

Result: **8 passed, 0 failed** (including the reconnect schedule, bounded
interruptible retry, stalled-after-headers timeout, and midstream resume
fixtures).

## Disposition

No additional source patch was warranted in the authorized write scope. The
existing reconnect implementation is bounded and covered by executable tests;
the remaining live-stream body lifetime is intentional rather than an
unbounded retry or busy-poll loop.

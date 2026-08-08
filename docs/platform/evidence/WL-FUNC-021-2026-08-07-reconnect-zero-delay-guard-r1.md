# WL-FUNC-021 — reconnect zero-delay guard (2026-08-07)

## Finding

`backoff_delay_secs` accepted a zero retry base or cap and returned a
zero-second delay. A caller with that malformed budget could issue duplicate
provider retries in a tight loop, defeating the reconnect CPU bound. The
primitive now normalizes either zero input to a one-second floor while keeping
the normal production schedule unchanged.

## Change

- `crates/services/mde-musicd/src/reconnect.rs`
  - added `MIN_RETRY_DELAY_SECS = 1`;
  - normalized zero base/cap values before exponential calculation;
  - added regression coverage that every zero-budget attempt remains delayed.

The existing finite reconnect connect/request timeout constants and recovery
behavior were preserved. No worklist or unrelated source file was edited.

## Verification

Farm `.50`, slot `music-reconnect-zero-delay-r1`:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-reconnect-zero-delay-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd reconnect --locked -- --nocapture
```

Result: **9 passed, 0 failed**, including the zero-budget guard, bounded
stalled-provider timeout, interruptible retry budget, and mid-stream audible
offset recovery tests.

## Blockers

No farm blocker. Live Dell provider-loss interruption and hardware/audio
continuity remain outside this source-only, non-destructive gate because the
authorized Dell endpoints are unavailable.

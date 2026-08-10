# WL-FUNC-022 — stopwatch origin authority (r143)

Date: 2026-08-10

Source revision: `3e9ecc58`

## Result

The Clock UI now refuses local control commands for a mirrored stopwatch whose
origin is another node. The daemon independently binds an upserted stopwatch's
origin to the signed command origin and refuses any attempt to transfer an
existing stopwatch identity between origins.

The daemon checks remain authoritative even if a caller bypasses the UI. A
rejected command leaves the snapshot and durable revision unchanged.

## Focused farm proof

BigBoy build VM `.130`:

```text
timers::tests::mirrored_stopwatch_refuses_local_control_commands
```

Result: 1 passed, 0 failed.

Machine 194 build VM `.50`, isolated slot `clock-origin-worker-r1`:

```text
cargo test -p mackesd --features async-services \
  workers::clock::tests::stopwatch_ -- --nocapture
```

Result: 2 passed, 0 failed:

- `stopwatch_commands_cannot_claim_a_foreign_origin`
- `stopwatch_identity_conflict_cannot_transfer_an_existing_origin`

Focused rustfmt and `git diff --check` passed. No physical seat was used.

## Remaining boundary

Peer stopwatch transport, stale/frozen mirror presentation, multi-process
package integration, and live installed-seat proof remain before WL-FUNC-022
can close.

# WL-FUNC-021 — MPRIS idle wakeup removal (2026-08-07)

## Change

The `mde-musicd` MPRIS lifetime thread had no periodic work but still woke every
200 ms. On a fleet of idle seats that created synchronized timer wakeups. The
thread now waits on a Tokio `Notify` and is woken only by handle shutdown; the
stop path retains its permit, so shutdown cannot race the wait.

## Verification

Farm `.50`, slot `mpris-event-r1`:

```text
cargo test --locked -p mde-musicd --lib mpris -- --nocapture
10 passed, 0 failed, 178 filtered out
```

The subsequent full farm library gate on `.50`, slot
`musicd-full-mpris-r1`, passed `188 passed, 0 failed`.

The focused regression proves prompt notification-driven teardown. This is
source/farm evidence only; live multi-seat CPU sampling remains open while Dell
and the installed seat are unreachable.

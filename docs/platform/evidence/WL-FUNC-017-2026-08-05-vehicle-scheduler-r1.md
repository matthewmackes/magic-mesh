# WL-FUNC-017 — MG90 heartbeat and enrichment scheduler (2026-08-05)

The multi-gateway roster now schedules current status, slow enrichment, and
heartbeat work independently for every source/manager assignment. Heartbeat
plans slower than two seconds reject, missed intervals coalesce instead of
bursting, delayed/failed enrichment cannot erase accepted fields, and selected
snapshots publish immediately on semantic change or on an independent
per-gateway heartbeat clock. Selection and output remain deterministic across
multiple gateways and managers.

The production single-gateway worker now executes its blocking probe fold off
the async scheduling lane, so a cached accepted snapshot continues heartbeating
while a probe is slow. Publication sequence allocation remains exclusively on
the parent worker and is monotonic across completed background probes.

## Verification

- Farm `.90`, slot `wl-func017-vehicle-scheduler-r1`:
  `cargo test -p mackesd --lib workers::vehicle::tests -- --nocapture`.
- Result: `53 passed; 0 failed; 4408 filtered out`.
- Farm file-scoped `rustfmt --check` and scoped `git diff --check` passed; the
  disposable 6.1-GiB slot was removed.

## Remaining acceptance edge

The concrete single-gateway adapter still performs LCI/current status and slow
radio/GNSS/application enrichment in one background `build_state` fold. Its
heartbeats are isolated, but the actual adapters still need to split fast and
slow probes before the full cadence requirement and 30-minute hardware bench
acceptance can close.

# WL-FUNC-017 — production MG90 fast/slow split (2026-08-05)

The production single-gateway worker now publishes an honest pending/offline
snapshot before any blocking operation and runs cached heartbeats independently
at no more than two-second intervals. Current status and slow GNSS/WAN/OBD
enrichment have separate single-in-flight tasks and deadlines; timed-out late
results are discarded, while failed enrichment retains the last sourced values
and records explicit freshness gaps.

Every curl operation has a two-second connect timeout and six-second maximum.
Session cookie jars are random, exclusively created with `O_NOFOLLOW` and mode
0600 under a verified private `/run` directory, and removed on every path.
Publication sequence numbers remain owned by the parent worker.

## Verification

- Farm `.90`, slot `wl-func017-production-split-r1`:
  `cargo test -p mackesd --lib workers::vehicle::tests -- --nocapture`.
- Result: `58 passed; 0 failed; 4388 filtered out`.
- Farm file-scoped `rustfmt --check`: passed.
- The disposable 6.1 GB farm slot was removed after the result was captured.

## Remaining acceptance edge

A live MG90 run must measure actual Bus heartbeat gaps and show timeout and
freshness transitions with unreachable or degraded hardware. No live hardware
claim is made by this farm evidence.

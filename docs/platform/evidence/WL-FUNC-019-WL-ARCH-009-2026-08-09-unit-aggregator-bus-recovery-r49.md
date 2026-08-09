# WL-FUNC-019 / WL-ARCH-009 — Unit Aggregator Bus recovery (r49)

Date: 2026-08-09

Farm: BigBoy `172.20.0.130`, slot `unit-aggregator-bus-r49`

## Production semantics

- The worker now resolves an absent user Bus root to `mde_bus::SYSTEM_BUS_ROOT`; an unresolved or unopenable root is retried by the same worker with shutdown-aware exponential backoff bounded from 10 ms to 2 s.
- Bus activation is atomic across `Persist::open`, live-index identity validation, and the `action/units/get-stream` tail read. Existing requests are skipped, while the first post-activation request is processed.
- The production cloud source is no longer frozen at construction. Every cycle reads all current `state/cloud/*` topics through the activated/reopened Bus handle. Open, topic-list, topic-read, missing-body, and decode failures are explicit errors and defer the cycle rather than becoming an empty cloud view.
- Mesh, cloud, and LAN inputs, derived edges, serialized output, and a cloned first-seen map are staged before publication. Source or mirror-write failure commits neither `SeenTracker`-equivalent memory nor `last`/`last_pub_at`; the next poll retries immediately.
- A changed Bus index is reopened and the transient request lane is tail-primed again. External cloud writes, forward requests, and replacement-index state remain visible.
- Request rows are fully read before effects. A reply is materialized once, retained in same-worker memory across write failures, written before cursor advancement, and not recomputed on retry. Process-crash caveat: this in-memory pending reply cannot provide exactly-once recovery across a daemon crash; restart tail-priming intentionally skips retained requests, and a durable request/reply ledger would be required to close that protocol gap.

## Hostile coverage

- Late/unopenable Bus recovery, startup tail-prime with no retained reply, first forward request, external cloud mutation, replacement-index cloud state, replacement forward request, and shutdown.
- Final cloud-lane decode failure is returned explicitly.
- Cloud source failure produces zero mirror output and zero first-seen mutation.
- Mirror write failure preserves `last`, heartbeat, and first-seen state for immediate retry.
- Reply write failure preserves the exact pending body and cursor, then emits one reply on retry without recomputation or duplication.
- Canonical system-spool fallback and shutdown during startup backoff.

## Verification

The first synced module run passed 66/66. After the final strict-decode regression was added, a normal resync encountered an unrelated concurrent compile defect in `workers/weather_forecast.rs` (`E0618`/`E0282`); that file is outside this slice and was not modified here. The isolated farm slot was overlaid with the `HEAD` version of that unrelated file, then the exact affected module suite was rerun:

```text
cargo test -p mackesd --lib --features async-services workers::unit_aggregator -- --nocapture
test result: ok. 67 passed; 0 failed; 0 ignored; 0 measured; 4439 filtered out
```

The seven newly added hostile tests were then invoked by fully qualified name
against the same farm test binary: **7/7 passed** (one test per invocation).

Farm formatting and local scoped whitespace verification:

```text
rustfmt --edition 2021 --config skip_children=true --check \
  crates/mesh/mackesd/src/workers/unit_aggregator/mod.rs \
  crates/mesh/mackesd/src/workers/unit_aggregator/sources.rs
# exit 0

git diff --check -- \
  crates/mesh/mackesd/src/workers/unit_aggregator/mod.rs \
  crates/mesh/mackesd/src/workers/unit_aggregator/sources.rs
# exit 0
```

## Source hashes

```text
fc4f5f999e43dfc03bee5b2b3b4b3c4d95017e729698f40cec3dcf34a3f26152  crates/mesh/mackesd/src/workers/unit_aggregator/mod.rs
14a67bacfe81afad08851d8ae264903e5aa5fb5767e78b2053e88d5f059c8b31  crates/mesh/mackesd/src/workers/unit_aggregator/sources.rs
```

No commit or push was performed. `docs/platform/WORKLIST.md` was not edited.

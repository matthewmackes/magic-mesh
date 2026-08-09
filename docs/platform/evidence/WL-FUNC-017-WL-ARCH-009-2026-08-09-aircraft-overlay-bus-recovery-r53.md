# WL-FUNC-017 / WL-ARCH-009 — aircraft overlay Bus recovery r53

Date: 2026-08-09

## Scope and behavior

- Production Bus roots are no longer frozen as an optional construction-time value. Every context read and overlay publication resolves the current user Bus root, with canonical `/run/mde-bus` fallback, and opens a fresh `Persist` handle. An explicit test override remains fixed by design.
- Vehicle context now distinguishes `Ok(None)` (successfully read but genuinely absent, invalid, or stale) from Bus open/read/body/decode errors. The former publishes an empty retraction and clears private aircraft; the latter defers the pass without provider or publication effects.
- Snapshot publication returns `Result`. A modified or 304 refresh updates `last_good` and resets successful scheduling only after the Bus write succeeds. Failed publication leaves the refresh uncommitted and eligible for retry.
- Existing point-bound HTTP validators, 304-to-query matching, and shutdown/provider isolation remain intact. A genuinely absent/stale context clears private vehicle-scoped aircraft even when its retraction publication is temporarily unavailable.

## Farm proof

Farm host: machine193, `172.20.0.90`

Slot: `aircraft-overlay-bus-r53`

Source SHA-256: `4a5b0c16e33fca130ea1510823770591bf32437de1557dc9015bc5900c667988`

The detached farm worktree contained the repository baseline plus only `crates/mesh/mackesd/src/workers/aircraft_overlay.rs` from this slice.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=aircraft-overlay-bus-r53 \
  /tmp/magic-mesh-aircraft-r53/install-helpers/xcp-build.sh cargo test \
  -p mackesd --features async-services \
  workers::aircraft_overlay::tests::late_and_replaced_bus_are_reopened_per_transaction -- --exact

Result: PASS — 1 passed, 0 failed, 4510 filtered out. The same worker failed closed while its Bus path was unopenable, recovered after the Bus appeared, and read a different fix after the Bus directory was replaced. The helper assertion also proves canonical system-root fallback.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=aircraft-overlay-bus-r53 \
  /tmp/magic-mesh-aircraft-r53/install-helpers/xcp-build.sh shell

Remote exact commands:
cargo test -p mackesd --features async-services --lib \
  workers::aircraft_overlay::tests::failed_vehicle_context_read_is_effect_free -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::aircraft_overlay::tests::publish_failure_retries_without_committing_last_good_or_success -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::aircraft_overlay::tests::no_vehicle_fix_degraded_snapshot_retracts_prior_aircraft_and_query_origin -- --exact

Results: PASS — each command ran 1 exact test with 0 failures and 4510 filtered out.
- Malformed retained vehicle JSON caused zero probe calls and no aircraft topic write.
- An unopenable publication root returned scheduling failure and retained no `last_good`; after recovery, the retry published exactly one row and committed private state.
- Genuine fix loss published an empty, zero-origin overlay and removed prior private aircraft.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=aircraft-overlay-bus-r53 \
  /tmp/magic-mesh-aircraft-r53/install-helpers/xcp-build.sh shell

Remote commands:
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/aircraft_overlay.rs
sha256sum crates/mesh/mackesd/src/workers/aircraft_overlay.rs

Result: PASS; rustfmt produced no diff and sha256sum returned the source hash above.
```

## Residual caveat

The aircraft projection is one Bus-topic write, not a durable fetch outbox. A process crash after the provider returns but before publication loses that fetched response and the next worker instance must fetch again. Also, when genuine context loss is detected while the Bus is unavailable, private aircraft are cleared immediately but an older retained Bus projection can remain externally visible until a later retraction retry succeeds.

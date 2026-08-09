# WL-FUNC-017 / WL-ARCH-009 — air-quality Bus recovery r58

Date: 2026-08-09

## Scope and semantics

- `AirQualityOverlayWorker` no longer freezes an optional Bus root at construction. Each context read and publication resolves an explicit override first, otherwise the current user root, with canonical `/run/mde-bus` fallback, and opens a fresh `Persist` transaction.
- Vehicle context is `Result<Option<AirQualityContext>>`: Bus open/read/body/JSON failures defer without provider or publication effects; only a successfully read genuinely absent, stale, or unsupported fix may commit the no-fix empty status.
- Fresh, degraded, unconfigured, secret-store, and no-fix publications return `Result`. `last_good`, success, suppression flags, provider retry metadata, authentication-triggered key reload, and retry progression advance only after the required write commits.
- An off-thread AirNow result remains staged until the worker freshly reopens the Bus, decodes vehicle state again, and verifies exact context equality. Movement publishes an empty degraded status for the new context instead of admitting the old query's stations; context loss publishes the no-fix retraction; post-fetch context read faults discard the result effect-free.
- Sealed-only key loading and key-safe authentication diagnostics remain unchanged. Every provider failure status contains no station records, including when a private last-good snapshot exists.

## Farm proof

Farm host: machine193, `172.20.0.90`

Slot: `air-quality-bus-r58`

Source SHA-256: `05a6e75205f8e671d483639625c9f3dd9f090d09185e41bfc45e9dc17466aeaa`

The detached farm worktree contained the repository baseline plus only `crates/mesh/mackesd/src/workers/air_quality_overlay.rs` from this slice.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=air-quality-bus-r58 \
  /tmp/magic-mesh-air-quality-r58/install-helpers/xcp-build.sh cargo test \
  -p mackesd --features async-services --lib \
  workers::air_quality_overlay::tests::late_and_replaced_bus_are_reopened_per_transaction -- --exact

Result: PASS — 1 passed, 0 failed, 4523 filtered out. The same worker failed closed on an unopenable path, recovered when the Bus appeared, then observed a different vehicle context after same-path Bus replacement. The test also proves canonical system fallback.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=air-quality-bus-r58 \
  /tmp/magic-mesh-air-quality-r58/install-helpers/xcp-build.sh shell

Remote exact commands:
cargo test -p mackesd --features async-services --lib \
  workers::air_quality_overlay::tests::failed_context_read_defers_without_fetch_or_publication -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::air_quality_overlay::tests::post_fetch_context_change_withholds_stale_station_result -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::air_quality_overlay::tests::write_failure_remains_uncommitted_and_corrects_forward_once -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::air_quality_overlay::tests::missing_sealed_key_publishes_unconfigured_without_fetch_time -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::air_quality_overlay::tests::failed_refresh_withholds_stations_but_keeps_private_last_good -- --exact

Results: PASS — each command ran 1 exact test with 0 failures and 4523 filtered out.
- Malformed vehicle JSON produced zero probe calls and no AQI topic write.
- A valid AirNow response completed off-thread, but a vehicle move before commit produced an empty new-context status with no fetched timestamp or station records.
- An unopenable publication root committed neither success nor `last_good`; after recovery, the same worker corrected forward with exactly one fresh row.
- Missing sealed credentials still publish an explicit empty unconfigured state without a fetch timestamp.
- Provider failure still publishes an empty degraded state while retaining private retry bookkeeping only.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=air-quality-bus-r58 \
  /tmp/magic-mesh-air-quality-r58/install-helpers/xcp-build.sh shell

Remote commands:
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/air_quality_overlay.rs
sha256sum crates/mesh/mackesd/src/workers/air_quality_overlay.rs

Result: PASS; rustfmt produced no diff and sha256sum returned the source hash above.
```

## Residual caveats

The AQI projection is a single Bus-topic write, not a durable fetched-result outbox. A process crash after AirNow returns but before publication loses that response and requires another provider fetch. By required fail-closed semantics, a Bus/context read error produces no replacement write; therefore an older retained projection can remain visible until context reads recover. Likewise, a publication failure cannot immediately retract a retained projection, but it commits no success, flags, key reload, backoff advance, or `last_good`, and a later successful transaction corrects forward.

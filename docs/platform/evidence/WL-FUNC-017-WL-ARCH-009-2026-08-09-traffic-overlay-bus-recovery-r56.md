# WL-FUNC-017 / WL-ARCH-009 — Traffic Overlay Bus recovery r56

Date: 2026-08-09

## Scope

- Production source: `crates/mesh/mackesd/src/workers/traffic_overlay.rs`
- Verification host: machine9 (`172.20.0.50`)
- Isolated farm slot: `traffic-overlay-bus-r56`
- Clean base revision: `57e6cab37b16609c962f48cc8239675c6c2ac5e3`

## Corrected behavior

- Construction retains only an explicit Bus-root override. Every authority transaction resolves the current configured/user Bus root and falls back to `mde_bus::SYSTEM_BUS_ROOT`, opens it, and refreshes a replaced index before reads or writes.
- Vehicle context is `Result<Option<TrafficContext>>`: no row or a semantically invalid/stale fix is absence, while Bus open/read, missing-body, and JSON decode failures defer the pass with no empty projection or `last_good` mutation.
- Publication is an explicit `io::Result`. Fresh `last_good`, no-context clearing, retry/backoff success, and rate-limit scheduling advance only after their required publication succeeds.
- A provider result remains pending in the live worker across Bus read/write failure. Each retry reopens the Bus and rechecks the complete current vehicle context before committing, so publication can correct forward without repeating provider I/O or losing a validator-backed fresh result.
- When an in-memory last-good result is unavailable, a degraded projection's persisted source is read as `Result<Option<_>>`; storage/body/decode failure defers instead of manufacturing a false empty projection.
- The existing `Retry-After` delay and paused-last-good behavior remain unchanged after a successful degraded publication.

## Focused verification

The dirty tree was synced with:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=traffic-overlay-bus-r56 install-helpers/xcp-build.sh sync
```

The isolated farm workspace was restored from the clean base archive and only the owned source was overlaid, excluding unrelated agents' dirty files.

Farm rustfmt:

```text
rustfmt --edition 2021 crates/mesh/mackesd/src/workers/traffic_overlay.rs
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/traffic_overlay.rs
```

Result: PASS, with no check output.

All test commands used `CARGO_TARGET_DIR=/home/mm/target-traffic-overlay-bus-r56`:

```text
cargo test -p mackesd workers::traffic_overlay::tests::late_and_replaced_bus_recovers_external_context_and_shutdown -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4511 filtered out`. The same worker survived an unopenable root, published from a late external vehicle context, followed a replaced index and new external context, and stopped promptly on shutdown. The test also checks explicit/current/system root precedence.

```text
cargo test -p mackesd workers::traffic_overlay::tests::failed_context_and_persisted_source_reads_are_effect_free -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4511 filtered out`. Injected read and decode failures produced no publication or private-state mutation, including the persisted degraded-source path.

```text
cargo test -p mackesd workers::traffic_overlay::tests::failed_write_retains_prepared_result_and_corrects_forward_without_refetch -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4511 filtered out`. A hostile traffic topic caused publication failure; after repair the same prepared result published, with the provider call count remaining exactly one.

```text
cargo test -p mackesd workers::traffic_overlay::tests::rate_limit_retains_fetch_time_and_publishes_paused_last_good -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4511 filtered out`, preserving the existing Retry-After and paused-last-good contract.

Scoped checks:

```text
git diff --check -- crates/mesh/mackesd/src/workers/traffic_overlay.rs
git diff --numstat -- crates/mesh/mackesd/src/workers/traffic_overlay.rs
```

Result: PASS; source delta `486 insertions, 85 deletions`. No broad or filler test suite was run.

## Hashes

- Traffic Overlay source SHA-256: `7a4f7f3507eae69a4f4ae0ad42791c6da720a404d1eb14265bb25510fdc89502`
- Traffic Overlay working Git blob: `70bdb4f9e3cac59520e5806971c8730c54092339`
- Farm source SHA-256: `7a4f7f3507eae69a4f4ae0ad42791c6da720a404d1eb14265bb25510fdc89502`

## Residual non-atomic caveats

- Provider completion, Bus context recheck, and Bus publication cannot be one atomic transaction. Context can change after the final recheck but before the write; the next observed context pass corrects the retained projection.
- The unpublished prepared response is process-memory state. A process crash discards it, but also discards the HTTP validator, so restart performs a full provider request rather than accepting an unrecoverable 304.
- A crash after a successful Bus write but before in-memory `last_good`/schedule mutation can append a semantically duplicate state row after restart. The latest retained state remains correct; there is no durable publication transaction marker.
- Persisted degraded-source read and degraded publication share one refreshed handle but are not an isolated compare-and-swap transaction against concurrent external writers.

No blockers. No commit or push was performed, and `docs/platform/WORKLIST.md` was not edited.

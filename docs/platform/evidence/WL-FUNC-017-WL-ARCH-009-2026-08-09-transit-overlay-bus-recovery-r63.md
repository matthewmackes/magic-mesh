# WL-FUNC-017 / WL-ARCH-009 — Transit overlay Bus recovery (r63)

Date: 2026-08-09

Scope was limited to `crates/mesh/mackesd/src/workers/transit_overlay.rs` and this evidence file. `docs/platform/WORKLIST.md` was not edited. No commit or push was made.

## Production semantics

- Construction no longer freezes an optional Bus root. Every read/publication transaction resolves an explicit override first, then the current user/service root, then canonical `mde_bus::SYSTEM_BUS_ROOT`.
- Every transaction fresh-opens Persist and follows a replaced index before reads and writes. Late, temporarily unopenable, and atomically replaced Bus storage recovers in the same worker with bounded shutdown-aware retry.
- Vehicle context is `Result<Option<TransitPoint>>`. Bus open/read failures, missing bodies, malformed JSON, wrong-host rows, and structurally invalid fixes defer effect-free. Only a successfully decoded offline/no-fix or stale fix is genuine no-context state.
- MBTA HTTP/protobuf work remains off the Tokio thread. The worker then fresh-opens the Bus and re-reads the exact vehicle point before publishing. A changed point discards the prepared old-point result and retracts an in-memory prior-point snapshot before fetching for the new point; context loss follows the no-context path.
- A prepared MBTA result is retained across Bus open/read/write failures. Retry republishes that same bounded result without another provider request, and clears it only after a durable effect or a proven context change/loss.
- Fresh, 304-refresh, degraded, moved-point retraction, and no-context publication all return `io::Result`. `last_good`, retry/cadence success, and no-context suppression advance only after the corresponding Bus write succeeds.
- HTTP ETag/Last-Modified values are staged after a bounded HTTP 200 body read. They become request validators only after the normalized `Modified` snapshot is published successfully. Invalid payloads, context races, and failed writes cannot advance conditional state.
- No-context publication is transition-based. Repeated no-context polls verify that the current Bus still contains the canonical empty transit projection and append no duplicate rows. A successful valid-context read resets suppression; a failed empty write remains retryable; a replacement index receives exactly one corrected-forward empty row.
- Existing GTFS-Realtime limits remain unchanged: 512 KiB body, 4,096 processed entities, 256 retained vehicles, 128 gaps, strict full-dataset/version/clock validation, and bounded strings/positions.

## Focused verification

Farm host: machine194, `172.20.0.170`

Explicit slot: `MCNF_BUILD_HOST=172.20.0.170`, `MCNF_BUILD_SLOT=transit-overlay-bus-r63`

The required farm helper created the slot:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=transit-overlay-bus-r63 \
  install-helpers/xcp-build.sh sync
```

The helper initially refused at its 8 GiB safety floor. Read-only inventory identified two completed slots owned by this same drain (`iem-radar-bus-r59` and `clock-bus-r43`); only those disposable slots were removed. Machine194 then had 12 GiB free and helper sync succeeded. The remote verification tree was normalized to `git archive HEAD`, then only the owned transit source was overlaid so concurrent dirty files could not affect the result.

The first cold build and exact test used:

```text
ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.170 \
  'cd magic-mesh-farm-transit-overlay-bus-r63 && \
  cargo test -p mackesd --lib \
  workers::transit_overlay::tests::failed_write_retries_prepared_result_without_refetch_or_early_validator_commit \
  -- --exact --nocapture'
```

Result: pass. `1 passed; 0 failed; 4539 filtered out`. Cold compilation completed in 4m42s and emitted the repository's 256 existing `mackesd` warnings with no errors.

After adding the direct HTTP-validator fixture, its incremental exact build/test passed with `1 passed; 0 failed; 4540 filtered out`. The final built test binary then ran all other cases individually with `--exact --nocapture`:

```text
workers::transit_overlay::tests::context_read_or_decode_fault_is_effect_free
workers::transit_overlay::tests::late_and_replaced_bus_recovers_in_the_same_worker
workers::transit_overlay::tests::post_fetch_point_race_discards_old_feed_before_publication
workers::transit_overlay::tests::repeated_no_context_polls_publish_once_and_replacement_retries
workers::transit_overlay::tests::entity_retention_and_gap_cardinality_are_bounded
workers::transit_overlay::tests::http_validators_remain_staged_until_publication_commit
workers::transit_overlay::tests::vehicle_point_requires_same_host_online_finite_fresh_fix
workers::transit_overlay::tests::failed_refresh_keeps_timestamp_and_publishes_degraded_latest_snapshot
workers::transit_overlay::tests::not_modified_cannot_relabel_a_moved_points_snapshot
workers::transit_overlay::tests::failed_refresh_retracts_retained_vehicles_after_query_point_moves
workers::transit_overlay::tests::no_fresh_vehicle_fix_publishes_empty_state_before_first_fetch
workers::transit_overlay::tests::no_vehicle_fix_degraded_snapshot_clears_stale_bus_row_and_private_cache
workers::transit_overlay::tests::shutdown_wins_while_blocking_http_is_in_flight
```

Results: all thirteen passed individually. Together with the initial failed-write test, all fourteen exact affected tests passed on machine194.

Operational coverage:

- The late/replaced test starts with an unopenable root, activates it without restarting the worker, publishes through an external vehicle handle, replaces `index.sqlite`, and observes the changed-point projection from the same worker. It also asserts canonical system fallback, bounded retained vehicles, committed provider state, and prompt shutdown.
- The post-fetch race blocks the first provider request, externally moves the vehicle, then proves no old-point row is published, the new point is fetched, and the discarded request never commits provider state.
- The failed-write fixture proves one prepared result survives an injected write error, publishes on retry with exactly one provider fetch, and commits its validator exactly once. The same fixture proves failed no-context publication retains state/suppression, successful retry publishes once, repeated polls suppress duplicates, and missing replacement output triggers one corrected-forward row.
- The no-context runtime fixture proves repeated polls append exactly one row to the original index and exactly one row after index replacement.
- The context fault fixture proves read and decode errors publish nothing and mutate neither `last_good` nor suppression state. The bounded fixture retains the 4,096/256/128 GTFS limits.

Final farm formatting and scoped diff commands:

```text
ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.170 \
  'cd magic-mesh-farm-transit-overlay-bus-r63 && \
  rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/transit_overlay.rs'

ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.170 \
  'cd magic-mesh-farm-transit-overlay-bus-r63 && \
  git diff --no-index --check /dev/null \
  crates/mesh/mackesd/src/workers/transit_overlay.rs'
```

Results: pass. The authoritative local HEAD-relative `git diff --check -- crates/mesh/mackesd/src/workers/transit_overlay.rs` also passed.

## Hashes

Base HEAD: `a14a6b0dd00b306af690edc7f06661da170d2a61`

```text
d13891fc05a03a2c075c33b95912017da7653e8cf593c8c87fc701aa47616d1c  crates/mesh/mackesd/src/workers/transit_overlay.rs
bde9bf9fb7494245b37b9782f100e7bca92a54d7c9b67f266dbecb848a156e9c  scoped source patch against HEAD
```

The local and machine194 source hashes matched exactly.

## Blockers

None.

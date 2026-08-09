# WL-FUNC-017 / WL-ARCH-009: wildfire overlay Bus recovery (r64)

Date: 2026-08-09

Base revision at final verification: `2e28e06d50c199764a36d80292ef859f9026bb8c`

## Scope and semantics

The implementation is confined to `crates/mesh/mackesd/src/workers/wildfire_overlay.rs` and this evidence record.

- Every transaction resolves an explicit override first, then the current user Bus root, then canonical `SYSTEM_BUS_ROOT`. The same worker retries late or unopenable storage and reopens a replaced index.
- Vehicle context acquisition is `Result<Option<WildfireContext>>`. Bus open/read/body/decode failure defers the cycle without output, cache, suppression, pending-response, or retry-state effects. Only a successfully read absent or semantically invalid/stale fix enters the no-context transition.
- WFIGS work remains off-thread. Before any publication or in-memory commit, the worker reopens the Bus and re-reads the exact vehicle context. A changed point publishes an empty snapshot for the new point and cannot admit the prepared old-point perimeter.
- Fresh, degraded, moved-context, and no-context publications return `Result`. `last_good`, no-context suppression, pending response, and provider/rate-limit cadence advance only after the required append succeeds.
- A prepared WFIGS result survives publication failure in the same process and corrects forward without another provider call while its exact context remains current.
- Repeated no-context polls read the durable output identity and append only once per transition. If the Bus index is replaced and the output row disappears, the same worker appends exactly one empty row to the replacement index.
- Existing WFIGS endpoint/body/geometry bounds and point-bound ETag/NotModified identity remain enforced.

## Farm verification

Host: machine9, `172.20.0.50`

Explicit slot: `MCNF_BUILD_SLOT=wildfire-overlay-bus-r64`

A clean `git archive HEAD` staging tree received only the owned worker source. The farm helper synced that scoped tree to `/home/mm/magic-mesh-farm-wildfire-overlay-bus-r64`:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=wildfire-overlay-bus-r64 \
install-helpers/xcp-build.sh sync
PASS
```

Scoped formatting on the farm:

```text
rustfmt --edition 2021 crates/mesh/mackesd/src/workers/wildfire_overlay.rs
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/wildfire_overlay.rs
PASS
```

The first cold exact invocation used the farm helper command shape below and completed in 5m53s after dependencies were built:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=wildfire-overlay-bus-r64 \
install-helpers/xcp-build.sh cargo test -p mackesd \
  workers::wildfire_overlay::tests::<test-name> -- --exact --nocapture
```

Final-hash focused invocations reused the same isolated slot and used `cargo test -p mackesd --lib workers::wildfire_overlay::tests::<test-name> -- --exact`.

Results:

```text
failed_context_and_publication_are_effect_free_and_no_context_is_transition_bounded: PASS
late_and_replaced_bus_recovers_external_context_and_shutdown: PASS
post_fetch_movement_withholds_stale_perimeters: PASS
failed_write_corrects_forward_without_refetch: PASS
not_modified_remains_bound_to_matching_context: PASS
endpoint_query_geometry_and_payload_bounds_are_strict: PASS
captured_live_schema_normalizes_multipolygon_and_omits_non_wildfire: PASS
```

An initial compile identified an incorrect private `Priority` import; the owned source was corrected to the public `mde_bus::hooks::config::Priority` path before successful verification. The first 304 test attempt discarded its watch sender and therefore exercised shutdown cancellation; retaining the sender corrected the fixture, after which the exact test passed. No production behavior was weakened to satisfy either correction.

Scoped integrity:

```text
git diff --check -- crates/mesh/mackesd/src/workers/wildfire_overlay.rs
PASS

git diff --numstat -- crates/mesh/mackesd/src/workers/wildfire_overlay.rs
750  93  crates/mesh/mackesd/src/workers/wildfire_overlay.rs
```

Farm and workspace source hashes matched:

```text
SHA-256  ab5583843d1cfbd3ff30b0add964c7155d527ba27e0c1e1e4d50ac473cecfd20
Git blob baaf16c416aa5fe041f057cb0bd0cb959b15b23a
```

## Residual non-atomic boundaries

- Vehicle context can change after the final recheck and before the output append; the Bus does not provide a cross-topic conditional transaction.
- `last_good`, no-context suppression, and prepared provider results are process-memory state. A crash loses them and can require a provider refetch or a durable-output reconciliation on restart.
- Output append is ordered before in-memory commit but is not atomic with it. A crash after append can produce a later semantically duplicate projection.
- The HTTP validator may advance before Bus publication. Same-process pending state avoids a refetch after write failure, while a crash can require revalidation and may produce a safe degraded row instead.

No blocker remains. No commit, push, or `WORKLIST.md` edit was performed. Unrelated dirty files were not included in the farm staging tree and were left untouched.

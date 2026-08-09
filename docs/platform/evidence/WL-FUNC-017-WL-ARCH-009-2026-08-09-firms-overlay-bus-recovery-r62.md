# WL-FUNC-017 / WL-ARCH-009 — FIRMS overlay Bus recovery r62

Date: 2026-08-09

## Scope and semantics

- `FirmsOverlayWorker` no longer freezes an optional Bus root at construction. Every context read and publication resolves an explicit override first, otherwise the current user root, with canonical `/run/mde-bus` fallback, then opens a fresh `Persist` transaction.
- Vehicle context is `Result<Option<FirmsContext>>`: Bus open/read/body/JSON failures defer without FIRMS I/O or publication; only a successfully read genuinely absent or stale fix can commit the empty no-context status.
- Fresh, degraded, unconfigured, secret-store, and no-context publications return `Result`. Success, private hotspot state, suppression flags, retry metadata/progression, and authentication-triggered sealed-key reload advance only after the required write commits.
- No-fix and unconfigured suppression are Bus-backed rather than process-local: each in-memory flag only prompts a read of the current retained FIRMS row. A repeat is suppressed only when the current row exactly matches that worker status apart from publication time; a cleared or same-path replacement index receives exactly one new row. A failed read or write neither enables suppression nor clears `last_good`. Secret-store-error status is not suppression-gated and continues to attempt publication each cadence.
- The blocking FIRMS result remains staged until the worker freshly reopens the Bus, decodes vehicle state again, and proves exact context equality. Movement publishes an empty degraded state for the new context instead of admitting old-location hotspots; context loss publishes the empty no-fix state; a post-fetch context read error discards the result effect-free.
- Strict mde-seal key loading and key-safe diagnostics remain intact. Every committed failure state contains no hotspot records, and a committed failure clears private vehicle-scoped hotspot state.

## Farm proof

Farm host: machine193, `172.20.0.90`

Slot: `firms-overlay-bus-r62`

Source SHA-256: `bea539e381d676cd18d8a00d2497d7bcf3c9e207768593ffba8ab9f1a1cf149f`

The detached farm worktree contained the repository baseline plus only `crates/mesh/mackesd/src/workers/firms_overlay.rs` from this slice.

### Initial r62 proof before the landing correction

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62/install-helpers/xcp-build.sh cargo test \
  -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::late_and_replaced_bus_are_reopened_per_transaction -- --exact

Result: PASS — 1 passed, 0 failed, 4529 filtered out. The same worker failed closed on an unopenable path, recovered when the Bus appeared, then observed a different vehicle context after same-path Bus replacement. The test also proves canonical system fallback.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62/install-helpers/xcp-build.sh shell

Remote exact commands:
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::failed_context_read_defers_without_fetch_or_publication -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::post_fetch_context_change_withholds_stale_hotspot_result -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::write_failure_defers_hotspot_clear_and_key_reload_then_corrects_forward -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::missing_sealed_key_publishes_unconfigured_without_fetch_time -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::failed_refresh_publishes_empty_degraded_snapshot_without_replaying_hotspots -- --exact

Results: PASS — each command ran 1 exact test with 0 failures and 4529 filtered out.
- Malformed vehicle JSON caused zero FIRMS probe calls and no FIRMS topic write.
- A completed blocking FIRMS response was rejected after vehicle movement; the committed replacement was empty, unfetched, and bound to the new context.
- An unopenable degraded write committed no hotspot clear, retry metadata, or sealed-key reload. After recovery, the same worker committed exactly one empty corrected-forward row, cleared private hotspots, and exposed the authentication reload/retry metadata.
- Missing sealed credentials still publish an explicit empty unconfigured state without a fetch timestamp.
- Provider failure still commits an empty degraded state and never replays prior-location hotspots.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62/install-helpers/xcp-build.sh shell

Remote commands:
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/firms_overlay.rs
sha256sum crates/mesh/mackesd/src/workers/firms_overlay.rs

Result: PASS; rustfmt produced no diff and sha256sum returned
`a8cbd23b16886a0b9c4f00d43f7c2553fd07979565aca8fbe7962d553893e4a0`.
```

That initial source hash was superseded by the landing correction below.

### Landing correction proof

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62-correction/install-helpers/xcp-build.sh cargo test \
  -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::repeated_no_fix_publishes_once_and_replacement_index_gets_one_row -- --exact

Result: PASS — 1 passed, 0 failed, 4540 filtered out. An initially
unopenable Bus left suppression false and retained private hotspots intact;
after recovery, repeated no-fix passes produced one row. Removing the SQLite
index at the same path caused the same worker to publish exactly one empty row
to the replacement index, and another repeated pass produced no duplicate.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62-correction/install-helpers/xcp-build.sh shell

Remote exact commands:
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::write_failure_defers_hotspot_clear_and_key_reload_then_corrects_forward -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::late_and_replaced_bus_are_reopened_per_transaction -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::no_vehicle_fix_degraded_snapshot_retracts_prior_hotspots_and_query_origin -- --exact

Results: PASS — each command ran 1 exact test with 0 failures and 4540 filtered out.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62-correction/install-helpers/xcp-build.sh shell

Remote commands:
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/firms_overlay.rs
sha256sum crates/mesh/mackesd/src/workers/firms_overlay.rs

Result: PASS; rustfmt produced no diff and sha256sum returned
81771f525242f06841f307c26a9c2715c4e35db6a635fe3ff1ee8d12588cf306.
```

That no-fix-only landing source was superseded by the unconfigured correction below.

### Unconfigured replacement-index landing correction

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62-correction/install-helpers/xcp-build.sh cargo test \
  -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::repeated_unconfigured_publishes_once_and_replacement_index_gets_one_row -- --exact

Final result: PASS — 1 passed, 0 failed, 4541 filtered out. Repeated
unconfigured passes wrote one row to the original index; after same-path index
replacement, the same worker wrote exactly one unconfigured row to the new
index and suppressed only the subsequent exact repeat. The successful status
write cleared retained private hotspots.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62-correction/install-helpers/xcp-build.sh shell

Remote exact command:
cargo test -p mackesd --features async-services --lib \
  workers::firms_overlay::tests::repeated_no_fix_publishes_once_and_replacement_index_gets_one_row -- --exact

Result: PASS — 1 passed, 0 failed, 4541 filtered out. This confirms the
shared exact-status predicate preserves the no-fix replacement behavior and
does not confuse an empty status of another availability class with no-fix.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firms-overlay-bus-r62 \
  /tmp/magic-mesh-firms-r62-correction/install-helpers/xcp-build.sh shell

Remote commands:
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/firms_overlay.rs
sha256sum crates/mesh/mackesd/src/workers/firms_overlay.rs

Result: PASS; rustfmt produced no diff and sha256sum returned
bea539e381d676cd18d8a00d2497d7bcf3c9e207768593ffba8ab9f1a1cf149f.
```

## Residual caveats

The FIRMS projection is a single Bus-topic write, not a durable fetched-result outbox. A process crash after FIRMS returns but before publication loses that response and requires another provider fetch. Required effect-free context read failures cannot retract an older retained projection until reads recover. Likewise, an unavailable publication path can leave an older retained projection externally visible, but it commits no success, hotspot clear, flags, retry progression, or key reload; a later successful transaction corrects forward with an empty or fresh snapshot as appropriate.

# WL-UX-013 / WL-ARCH-009 — Health Reconciler Bus recovery and atomic ingress (r48)

Date: 2026-08-09

Scope was limited to `crates/mesh/mackesd/src/workers/health_reconciler.rs` and this evidence file. `docs/platform/WORKLIST.md` was not edited. No commit or push was made.

## Production semantics

- The worker retains only an explicit Bus-root override. Without one, each pass resolves the current user Bus root and falls back to `mde_bus::SYSTEM_BUS_ROOT`; construction no longer freezes an absent `default_data_dir()` forever.
- An unavailable or unreadable Bus is retried by the same worker with shutdown-aware exponential backoff bounded from 10 ms to 2 s. The blocking reconcile is raced against shutdown, and a `spawn_blocking` join failure is logged rather than discarded. The bounded legacy heartbeat/file reconciliation remains available while combined publication ingress waits for a complete Bus snapshot.
- Each pass fresh-opens the selected Persist root and calls `reopen_if_index_changed()` before reads, so late creation, external writers, and atomically replaced indexes are visible.
- Combined health-publication ingress is prepare/read-all/apply: it stages the approved publisher set, every bounded canonical file candidate, Bus open/discovery, and every exact publisher-lane read against cloned state before changing retained ledger/cursors, writing projections, or replacing the checkpoint.
- Failure to open the Bus or read any required publisher lane rejects the whole combined candidate. Last-good in-memory state, cursors, projection, and checkpoint remain unchanged; unavailable Bus is not interpreted as an empty current Bus.
- Existing fail-closed semantics remain: terminal malformed/invalid rows may advance their cursor deliberately, while projection failure retains the row for retry. Publisher/message/file/checkpoint bounds and per-topic fairness remain enforced.

## Focused verification

Farm: machine194, `172.20.0.170`

Explicit environment: `MCNF_BUILD_HOST=172.20.0.170`, `MCNF_BUILD_SLOT=health-reconciler-bus-r48`

The slot was seeded from `git archive HEAD`, then only the owned source file was copied into it. The first exact `cargo test` invocation compiled `mackesd` successfully. The repository emitted existing warnings; no test failed.

Exact test command shape (run once for each test below):

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=health-reconciler-bus-r48 \
ssh mm@172.20.0.170 'cd magic-mesh-farm-health-reconciler-bus-r48 && \
  cargo test -p mackesd --lib <exact-test-name> -- --exact --nocapture'
```

Final exact execution also ran the newly built `mackesd_core-*` test binary with `--exact` for each fully qualified name:

```text
workers::health_reconciler::tests::health_reconciler_bus_root_honors_override_and_system_fallback
workers::health_reconciler::tests::final_publisher_read_failure_preserves_complete_ingress_checkpoint
workers::health_reconciler::tests::late_and_replaced_bus_recovers_external_forward_state_and_shutdown
workers::health_reconciler::tests::health_ingress_projects_approved_bus_state_and_restores_after_malformed_inputs
workers::health_reconciler::tests::health_ingress_enforces_publisher_and_per_topic_message_bounds
```

Result for every exact test: `1 passed; 0 failed; 4491 filtered out`. Durations were 0.00 s, 0.02 s, 0.08 s, 0.02 s, and 0.05 s respectively.

The hostile final-lane test stages both a valid bounded file candidate and a forward Bus row, then fails the final sorted publisher read. It proves no in-memory ledger/cursor, checkpoint byte, or prior projection byte advances. The async recovery test starts with an unopenable root, activates it late, observes a publication from a separate Persist handle after activation, replaces `index.sqlite`, observes the replacement generation in the same worker, and completes shutdown within one second.

Scoped farm formatting:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=health-reconciler-bus-r48 \
ssh mm@172.20.0.170 'cd magic-mesh-farm-health-reconciler-bus-r48 && \
  rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/health_reconciler.rs'
```

Result: pass.

Scoped farm diff check compared `HEAD:crates/mesh/mackesd/src/workers/health_reconciler.rs` with the slot copy using `git diff --no-index --check`.

Result: pass.

## Hashes

```text
163ceb3960df4fbd0dbe49b7447265474bcc030803ab69ea4efaf3a6f6dd559c  crates/mesh/mackesd/src/workers/health_reconciler.rs
c10d1674cf7fe26c172d7c1b0dd0d40e5e5708b4e7fa240a761cf5d84db323dc  scoped source patch against HEAD
```

The local source hash and machine194 slot source hash matched exactly.

## Blockers

None.

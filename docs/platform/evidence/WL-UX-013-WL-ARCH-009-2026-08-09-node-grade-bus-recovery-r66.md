# Node-grade Bus recovery and atomic action ingress (R66)

Date: 2026-08-09
Worklist: `WL-UX-013`, `WL-ARCH-009`
Base commit: `2e28e06d50c199764a36d80292ef859f9026bb8c`

## Runtime semantics

`NodeGradeWorker` no longer freezes one optional `Persist` at construction.
Each action and health-projection transaction fresh-resolves and opens the Bus.
An explicit root remains authoritative; production otherwise tries the current
user/service root and then the canonical `mde_bus::SYSTEM_BUS_ROOT`. The worker
records the opened SQLite index device/inode and detects an atomically replaced
index without requiring a process restart. Open, activation, ingress, or
publication failure returns to the existing bounded periodic run loop, whose
phase and cadence waits remain shutdown-interruptible.

Every newly observed index is activated transactionally. The worker first reads
the current tail of the transient `action/system-mesh-health` mutation lane,
then stages every durable local action journal and reads each exact result lane,
and finally flushes terminal results. Only after those operations succeed does
it install the action tail and index identity. Retained mutation commands are
therefore skipped on initial and replacement indexes, while a command written
after activation is read as forward work and executes exactly once. Durable
claimed/complete action journals are an outbox, not transient ingress: they are
flushed rather than tail-skipped, preserving no-repeat recovery across restart
and Bus replacement.

Steady-state ingress stages the complete action lane, every result lane needed
for those actions, and all pending durable-result lanes before changing a
cursor, claiming or executing remediation, or publishing a terminal result. An
unreadable action/result lane defers the complete mutation sweep. Malformed
terminal action rows may advance deliberately only after the complete read set
has succeeded.

Action cursor advancement is now the durable result-publication boundary. A
failed result write leaves the completed journal and prior cursor intact; the
same worker republishes it on retry without repeating remediation. A durable
execution claim whose terminal journal write fails is recovered as an
interrupted result and is likewise never re-executed.

Health generation, pressure history, and `last_snapshot` remain candidate
in-memory state until required canonical and Bus publications succeed. The
canonical node authority is written first, then its canonical snapshot, and
only then are either projection published to the Bus. Snapshot folding includes
the candidate node row without requiring an early authority replacement.

The two canonical files cannot be atomically committed as one filesystem
object. If the node authority succeeds and the snapshot fails, the node row is
therefore an intentional corrected-forward generation floor while the prior
snapshot remains visible; neither candidate reaches the Bus. Every nonzero
cycle now selects above both the in-memory generation and the durable node
generation. Retry consequently advances to a new generation rather than
equivocating at the partially durable generation. The published-at repair floor
remains limited to restart recovery, preserving normal one-step generations.

## Focused machine194 verification

Host: machine194 (`172.20.0.170`)
Slot: `node-grade-bus-r66`

The helper performed the initial explicit-host sync/build:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=node-grade-bus-r66 \
install-helpers/xcp-build.sh cargo test -p mackesd \
  --features async-services \
  workers::node_grade::tests::late_and_replaced_bus_activation_skips_retained_and_executes_forward_once \
  -- --exact --nocapture
```

Two unrelated concurrent slices prevented the first compile from reaching the
test: `cloud/verbs/app.rs` still referenced a renamed Cloud field, and
`vehicle.rs` moved a topic while borrowing it. Only the isolated farm copy
received one-line compile shims for those two defects; neither unrelated local
file was edited by this slice. The warmed target then passed the exact test.

After the warm target consumed enough space for the helper's 8-GiB sync safety
floor to refuse another full-tree sync, only the owned source was checksum-
synced to the same slot. The final source ran these exact library tests:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::node_grade::tests::late_and_replaced_bus_activation_skips_retained_and_executes_forward_once \
  -- --exact

cargo test -q -p mackesd --features async-services --lib \
  workers::node_grade::tests::result_publication_failure_replays_durable_result_without_repeating_mutation \
  -- --exact

cargo test -q -p mackesd --features async-services --lib \
  workers::node_grade::tests::bus_roots_use_current_then_canonical_system_fallback \
  -- --exact

cargo test -q -p mackesd --features async-services --lib \
  workers::node_grade::tests::terminal_result_storage_failure_recovers_without_repeating_mutation \
  -- --exact

cargo test -q -p mackesd --features async-services --lib \
  workers::node_grade::tests::partial_canonical_publication_retries_forward_before_bus_projection \
  -- --exact

cargo test -q -p mackesd --features async-services --lib \
  workers::node_grade::tests::restart_generation_uses_durable_publication_floor_after_counter_rollback \
  -- --exact

cargo test -q -p mackesd --features async-services --lib \
  workers::node_grade::tests::applied_actions_emit_audited_results_with_refreshed_evidence \
  -- --exact
```

The original four results were `1 passed; 0 failed; 4,559 filtered out`; after
adding the review regression, the final four cycle-focused reruns were each
`1 passed; 0 failed; 4,560 filtered out`. The activation test proves
late open recovery, initial/replacement retained-command suppression, external
forward writes, and exact-once remediation on both indexes. The publication
test proves a failed terminal Bus write retains the cursor and completed journal
and repairs forward without another remediation. The terminal-storage test
preserves the durable claim/no-repeat contract. The new fault injection fails
between canonical node and snapshot writes. It observes the node at generation
N, the canonical snapshot and both Bus lanes still at N-1, unchanged in-memory
generation, and a successful retry at N+1; Bus histories contain N-1 and N+1
exactly, never the partial N generation. Restart-floor and action-refresh cycle
tests also pass with the revised generation selection.

Farm formatting passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/node_grade.rs
```

Scoped integrity check:

```text
git diff --check -- crates/mesh/mackesd/src/workers/node_grade.rs \
  docs/platform/evidence/WL-UX-013-WL-ARCH-009-2026-08-09-node-grade-bus-recovery-r66.md
```

Source SHA-256 (local and machine194 matched):

```text
6f01e662a4a95119cc0b1a8547507f673b2a2a1699a6e9727b6e0805a572088b  crates/mesh/mackesd/src/workers/node_grade.rs
```

## Scope

No broad suite, live-seat mutation, WORKLIST edit, commit, or push was made.
This checkpoint owns only `node_grade.rs` and this evidence file.

# WL-ARCH-009 — spawn-lighthouse-onboard Bus recovery (r40)

Date: 2026-08-09

## Scope

`spawn_lighthouse_onboard` now keeps the same worker alive while its Bus root is
unresolved, unopenable, or not yet safe to activate. Explicit roots still win;
otherwise the worker resolves `mde_bus::default_data_dir()` and falls back to
`mde_bus::SYSTEM_BUS_ROOT`. Startup retries use shutdown-interruptible
exponential backoff bounded from 10 ms through 2 s.

The worker has one transient mutation input,
`action/onboard/spawn-lighthouse`, and one append-only durable output-history
lane, `event/onboard/spawn-lighthouse`. Startup atomically reads the command
tail before installing the cursor, so retained provisioning commands never
replay. Commands written after activation remain forward work.

Each runtime tick calls `reopen_if_index_changed()` on its long-held mutable
`Persist` before reading the complete forward command batch. This preserves the
old fresh-open-per-tick visibility contract when an external writer atomically
replaces `index.sqlite`. The tick changes no cursor and reaches no authorization,
founding facts, provider, enrollment, or CA mutation seam until that read
succeeds. A read failure therefore preserves the cursor and defers the complete
mutation sweep.

Result events are written through the recovered Bus handle. If durable
publication fails after a mutation, the resolved event is retained in memory as
a same-worker publication barrier and retried before reading later commands; the
command cursor advances only after publication succeeds, and that worker does
not replay the completed provider sequence. This barrier is intentionally not a
durable process-crash outbox: a daemon/process crash after the external mutation
but before event persistence remains outside this correction's guarantee.

There is no durable state input lane to fold. Existing durable event history is
append-only output and remains intact through the outage.

## Focused farm proof

Host: machine194, `172.20.0.170`

Slot: `lighthouse-onboard-bus-r40`

The requested slot was established with explicit routing:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=lighthouse-onboard-bus-r40 \
  ./install-helpers/xcp-build.sh sync
```

Machine194 initially had less than the farm's 8 GiB sync floor. The completed,
inactive `mesh-mount-bus-r29` and `onboard-apply-bus-r35` disposable slots were
removed after confirming no process referenced them, recovering 12.9 GiB. These
were reproducible build artifacts and are not recoverable. The successful r40
slot was then rebuilt from clean `HEAD` with only
`spawn_lighthouse_onboard.rs` overlaid, excluding concurrent worker edits.

Primary exact test:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::spawn_lighthouse_onboard::tests::late_bus_recovers_without_replay_and_defers_reads_and_publication \
  -- --exact --nocapture
```

Final result: `1 passed; 0 failed; 4,467 filtered out`. The same worker survived
an unresolved root, an open failure, and an activation-tail failure. A retained
spawn caused no effects. A runtime read failure deferred a forward command. The
recovered read executed `provision`, `push_enroll`, and `migrate_ca` once; an
injected durable-publication failure emitted no history or cursor success; after
publication recovery exactly one result event appeared and the provider sequence
was not repeated. The forward command was written after activation through a
separate `Persist` handle and was observed by the worker's refreshed long-held
handle. Shutdown completed cleanly.

Warmed-slot exact checks:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::spawn_lighthouse_onboard::tests::service_bus_root_honors_override_and_falls_back_to_system_spool \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::spawn_lighthouse_onboard::tests::worker_drains_the_request_and_publishes_the_matching_event \
  -- --exact --nocapture
```

Results: each `1 passed; 0 failed; 4,467 filtered out`.

Final farm gates:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/spawn_lighthouse_onboard.rs
git diff --no-index --check <clean-HEAD-source> \
  crates/mesh/mackesd/src/workers/spawn_lighthouse_onboard.rs
```

Results: passed. The temporary clean baseline was deleted after the check.

Scoped local integrity gate:

```text
git diff --check -- \
  crates/mesh/mackesd/src/workers/spawn_lighthouse_onboard.rs \
  docs/platform/evidence/WL-ARCH-009-2026-08-09-spawn-lighthouse-onboard-bus-recovery-r40.md
```

Result: passed.

## Artifact identity

```text
6844893bffa13434054542827d1e2140375bd6c18c57aaa74f88cf1411a2b579  crates/mesh/mackesd/src/workers/spawn_lighthouse_onboard.rs
```

No WORKLIST edit or commit was made.

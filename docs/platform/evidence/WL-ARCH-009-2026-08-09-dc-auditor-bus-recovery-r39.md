# Datacenter auditor late-Bus recovery checkpoint (R39)

Date: 2026-08-09
Worklist: `WL-ARCH-009`
Base commit: `55df5cce39bebce729a390c5fb3a963e0e47d76f`

## Runtime semantics

`dc_auditor` is a durable passive projection, not a transient command
consumer. It now resolves an explicit or user Bus first, falls back to
`mde_bus::SYSTEM_BUS_ROOT`, and keeps the same worker alive through unresolved
and unopenable startup with shutdown-aware exponential backoff bounded from
10 ms to 2 s.

No request cursor is tail-primed. After Bus activation the worker immediately
enumerates every currently registered `action/dc/<verb>` lane and folds its full
retained history. Requests written during a Bus outage are therefore projected
to their stable `event/dc/audit/<request-ulid>` records rather than skipped as
startup backlog. Later registered-lane requests fold on subsequent passes.

Each leader pass first calls `Persist::reopen_if_index_changed()` on the
worker's retained handle, then uses that same refreshed handle for complete
lane discovery, reads, and writes. This preserves visibility of a Bus index
recreated by another process. The late-Bus regression publishes its forward
request through a separately opened handle after worker activation and proves
the retained worker observes and projects it without restarting.

The complete snapshot stages both every exact durable audit-output lane and
every registered request lane before effects. Audit identities are recovered
only from the exact `event/dc/audit/<ulid>` shape: a 26-character uppercase
Crockford-base32 ULID with a valid leading digit. Every admitted output lane is
read before its topic identity can seed `seen`. Output discovery/read failure,
request discovery/read failure, or request-lane read failure rejects the whole
snapshot before any write or in-memory state advance. An unavailable lane can
therefore never look empty and another lane cannot partially advance the
projection.

After a complete snapshot, the core first reconciles `seen` from durable audit
topic identities and only then derives request candidates. A fresh process thus
does not append another row when `event/dc/audit/<request-ulid>` already exists;
the same recovery closes the crash window after a successful Bus write but
before in-memory remember. For new work, projection and publication remain
split: the worker writes through the same refreshed `Persist`, and only a
successful write commits the ULID to memory. A failed write leaves that request
retryable.

## Focused farm verification

Host: machine9 (`172.20.0.50`)
Slot: `dc-auditor-bus-r39`

The following exact affected tests ran in that explicit slot:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::dc_auditor::tests::incomplete_reads_and_failed_writes_do_not_advance_projection_state \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::dc_auditor::tests::durable_output_snapshot_makes_restart_idempotent_and_read_failure_atomic \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::dc_auditor::tests::late_bus_folds_retained_history_and_forward_requests_without_restart \
  -- --exact --nocapture
```

Each final-source command passed: `1 passed; 0 failed; 4,471 filtered out`. The
atomicity test reads one request lane and injects failure on the complete
snapshot, observing zero writes and an empty dedup set. It then injects an audit
write failure, again observes an empty dedup set, and retries successfully. The
restart regression creates a durable request and audit record, starts a fresh
core/pass, recovers the projected ULID from the exact output topic, and observes
zero duplicate publication. Its incomplete-output seam reads one output lane,
injects failure on the next, and observes zero writes and no `seen` mutation.
The late-Bus test keeps one worker alive through unresolved and open-error
attempts, projects retained startup history, then observes a forward request
written through a separate post-activation handle exactly once before prompt
shutdown.

Exact formatting and scoped diff checks passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/dc_auditor.rs
git diff --check -- \
  crates/mesh/mackesd/src/workers/dc_auditor.rs
```

The first helper-driven farm compile was blocked before this module by an
unrelated concurrent `spawn_lighthouse_onboard.rs` refactor. The local file was
not touched. Only that file's ephemeral machine9 slot copy was restored to
`HEAD`, after which all three dc-auditor tests compiled and passed. This is not
a dc-auditor blocker.

Source SHA-256:
`94e587d22956f8cda8788f9574d817ee888126d476494d1e9371787388fcba65`.

## Scope

No broad suite, package build, live seat proof, WORKLIST edit, or unrelated test
was run. This checkpoint is limited to dc-auditor Bus-root recovery, retained
handle refresh, durable request/output-history folding, restart idempotence,
complete-lane read atomicity, and publish-before-dedup state ordering.

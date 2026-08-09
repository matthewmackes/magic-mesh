# WL-ARCH-009 — datacenter job-ledger Bus recovery (r37)

Date: 2026-08-09

## Scope

The passive `dc_jobs` projection no longer exits permanently when shared Bus
storage starts late. It uses the canonical system spool fallback and retries in
the same worker with shutdown-aware exponential backoff bounded from 10 ms to
2 s. Retained registered action and reply lanes are durable job history, so
they deliberately fold from the beginning after activation rather than being
tail-skipped as transient commands.

Each sweep now reads every registered request and exact reply lane before
publishing or remembering any transition. A failed reply read is unavailable
state, not `None`: it cannot regress an `ok`/`error` job to a fabricated
`pending` status. Status publication uses the same activated `Persist` handle,
and the in-memory transition is committed only after its Bus write succeeds.
The dead default-root re-open helper and its helper-only test were removed.

## Focused farm proof

Host: machine 196 (`172.20.0.196`)

Slot: `dc-jobs-bus-r37`

Three exact tests each passed with `1 passed; 0 failed; 4,465 filtered out`:

```text
workers::dc_jobs::tests::retained_job_history_folds_pending_then_terminal_reply
workers::dc_jobs::tests::late_bus_is_opened_by_the_same_worker
workers::dc_jobs::tests::service_bus_root_falls_back_to_the_shared_system_spool
```

They prove retained request/reply history produces an honest pending-to-ok
sequence, one worker opens a path that becomes usable after startup, and root
selection preserves explicit paths while falling back to the system spool.

The final source passed farm single-file `rustfmt --edition 2021 --check` and
local scoped `git diff --check`.

## Artifact identity

```text
ac1d91c848e5a6f7dc97e957c83b7605b49fda868e4f7b8a74ff6a722a229db9  crates/mesh/mackesd/src/workers/dc_jobs.rs
```

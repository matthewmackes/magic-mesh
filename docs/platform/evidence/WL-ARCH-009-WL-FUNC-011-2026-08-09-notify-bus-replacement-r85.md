# WL-ARCH-009 / WL-FUNC-011 — notification Bus replacement recovery (r85)

Date: 2026-08-09

Farm: machine196 (`172.20.0.196`), `MCNF_BUILD_SLOT=notify-bus-r85`

## Production correction

- The notification worker now fresh-opens the configured/current Bus on every
  poll. Opening is accepted only when the path identity is stable before/after
  open and the SQLite connection inode matches the live `index.sqlite`.
- Initial and replacement activation stages the retained Cloud tail, creates
  missing Peer/Updates lane primes idempotently, republishes retained segment
  rollups, verifies the Bus identity, and only then installs the Cloud cursor.
  Retained replacement Cloud events are not replayed; the first forward event
  is folded without restarting the worker.
- Peer/update probes and Cloud input operate on cloned state. Event, rollup,
  and Cloud reads are strict `Result` operations; failed reads or writes retain
  the prior source baselines, deduplication log, rollups, cursor, and tick phase.
  The final path check prevents a retired-index append from committing state.
- Lane primes are created only when absent. An activation retry therefore does
  not append repeated “monitor online” events that Chat could fold as alerts.

## Focused verification

The shared worktree contained unrelated in-progress worker edits. The normal
farm synchronization first exposed their compile errors. To avoid modifying or
waiting on those files, the disposable r85 slot retained the synchronized
`notify.rs` and overlaid the five unrelated worker paths from pushed `HEAD` via
`git archive`. No local file was reverted.

The final source compiled on machine196 and these exact tests passed:

```text
workers::notify::tests::same_path_bus_replacement_skips_retained_cloud_and_folds_forward_once
workers::notify::tests::repeated_bus_activation_does_not_duplicate_lane_primes
workers::notify::tests::late_bus_recovers_in_the_same_worker_and_primes_forward_lanes
workers::notify::tests::cloud_notify_lane_folds_into_alerts_segment_without_reemitting

each: 1 passed; 0 failed; 4,599 filtered out
```

Scoped farm `rustfmt --edition 2021 --check` and local `git diff --check`
passed. Farm and local source hashes matched.

## Residual boundary

An event append and its segment-rollup append are two SQLite writes rather than
one multi-topic transaction. A failure between them retains pre-tick memory and
retries, so notification truth is not lost, but an identical event append can
appear again. Chat's stable folded-alert identity deduplicates a refold; this
slice does not claim a new cross-topic atomic primitive or live toast/audio
hardware proof.

## Hash

```text
79126a694655fbe185320bb9941e67947137dbc1938b83fa07e2ddd6b47687c8  crates/mesh/mackesd/src/workers/notify.rs
```


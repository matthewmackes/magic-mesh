# WL-ARCH-009 — runtime-status aggregate ownership (2026-08-09)

## Root cause

Each of the six isolated `mackesd serve --group` processes constructs its own
`WorkerStatusMap`, but every process unconditionally started the same raw
`worker-runtime-status` thread. Those six threads wrote their group-local,
partial maps to the same `/run/mde/mackesd-status.json` file and
`state/mackesd/<node>` Bus topic. Last-writer wins therefore mislabeled one
group's partial view as the node aggregate.

## Correction

- The canonical worker registry now owns six distinct group publication
  identities and one aggregate identity.
- Each process can start only its registered group publisher. It writes an
  atomic, bounded file under `/run/mde/mackesd-status-groups/<group>.json` and
  only its already group-scoped per-worker Bus rows.
- Observation is the sole registered owner of the node-global aggregate. It
  reads exactly one regular bounded file for all six closed groups and replaces
  the global file/Bus topic only after the complete fold is admitted.
- Missing, duplicated, cross-node, or foreign-group inputs reject the whole
  fold and preserve the prior global projection. Source observation clocks are
  retained, so the aggregate owner cannot make a stale group look fresh.
- The registry inventory is pinned at 152 rows with digest
  `160560f2ca1712cdc685ab2a646892c38267b0ba83e17cd5fe47b36dc85b77a6`.

## Focused farm verification

Verification used machine `.90`, slot `arch009-runtime-status-owner-r98`, from
a detached `8a24dd07` proof worktree containing only the four owned source-file
changes. This avoided unrelated concurrent Cloud edits in the main worktree.

```text
cargo test -p mackesd --lib workers::worker_runtime_status::tests -- --nocapture
17 passed; 0 failed; 4613 filtered out

cargo test -p mackesd --bin mackesd process_group_thread_admission_tests -- --nocapture
5 passed; 0 failed; 56 filtered out

cargo test -p mackesd --lib worker_role::tests -- --nocapture
28 passed; 0 failed; 4602 filtered out
```

The hostile regressions prove disjoint group paths, bounded/symlink-refusing
reads, exact six-group admission, preservation of source clocks, rejection of
foreign ownership, and refusal of aggregate ownership by the other five
process groups. Registry digest/drift tests were also run after pinning the
final digest.

## Remaining gaps

- Package/live proof must show all six group files and one complete global
  projection across six cgroups, including group-process loss and recovery.
- `WorkerSpec` publication/subscription/dependency/action descriptors remain
  incomplete.
- Other process-local infrastructure (service-key retry, etcd startup probe,
  watchdog, and signal handling) still needs an explicit census policy and
  ownership drift coverage.

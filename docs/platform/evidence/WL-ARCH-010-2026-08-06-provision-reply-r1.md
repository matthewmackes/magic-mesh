# WL-ARCH-010 compute-provision reply boundedness — 2026-08-06

## Goal

Keep the synchronous certificate/provision reply wait bounded while preserving
the explicit RPC rule that the oldest reply for a request ULID wins.

## Implementation

- `crates/mesh/mackesd/src/workers/compute_provision.rs`
  - `await_reply_sync` now uses the SQL-enforced `Persist::list_since_limit`
    page of one row instead of materializing the entire retained reply topic.
  - Added a hostile regression with one oldest reply plus 64 newer duplicates;
    the oldest body remains the result.
  - `read_new_creates` now admits at most 64 create requests per worker poll,
    with a hostile 65-request regression proving cursor advancement.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=compute-provision-r2 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  compute_provision::tests -- --nocapture
```

- BigBoy focused module: 41 passed, 0 failed.
- The first attempt reached final linking but failed only because BigBoy's
  `/home` scratch filesystem was full (`mold: No space left on device`); the
  exact generated farm slots were removed and the retry passed with 21 GiB
  available.
- Touched-file rustfmt check reports one unrelated pre-existing import-order
  diff elsewhere in this large dirty module; the changed hunk is formatted and
  no whole-file rewrite was applied.
- `git diff --check`: passed locally.

## Source hash

```text
5c4a8cfff24f1ace4c8c300b92f7c8cd347149069c729a8e71770355afa90862  crates/mesh/mackesd/src/workers/compute_provision.rs
```

## Remaining authority proof

This bounds one Execute reply wait; it does not close the broader Workload
authority epic. Live libvirt/Quadlet recovery, caller migration, Display1/KMS,
Dell/seat-15 acceptance, and remaining adapter/recovery queues remain open.

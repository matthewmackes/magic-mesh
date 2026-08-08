# WL-ARCH-010 evidence — bounded Fleet projection reader (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

The shell Fleet/Datacenter reader now treats health, Workload roster, adfilter,
and browser-policy topics as latest-value projections. Each retained topic is
queried with one indexed `read_latest` probe; per-node fan-out still reads one
latest row per enumerated topic. The render fold therefore receives current
typed projections without materializing stale topic history, while the shell
continues to publish power intent only through the typed Workload operation
lane.

Changed file:

- `crates/desktop/mde-shell-egui/src/datacenter.rs`

## Farm verification

The focused suite ran on BigBoy `.130` in isolated slot `datacenter-r1`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=datacenter-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui \
  datacenter::tests -- --nocapture
result: 28 passed, 0 failed

ssh mm@172.20.0.130 \
  'rustfmt --edition 2021 --check \
   crates/desktop/mde-shell-egui/src/datacenter.rs'
result: reports pre-existing unrelated formatting drift in this large dirty
module; the new latest-value reader/test hunk is formatted and no unrelated
module rewrite was applied
```

The hostile regression,
`datacenter::tests::latest_value_reader_does_not_materialize_retained_projection_history`,
seeds 65 retained health snapshots and proves that the reader returns only the
newest complete projection. Local `git diff --check` passed.

## Runtime and remaining proof

This is a bounded presentation/read migration, not live Workload completion.
Libvirt/Quadlet adapter recovery, caller migration, Display1/KMS, direct DRM,
Dell/seat-15 acceptance, and full release evidence remain open.

## Source hash at capture

```text
a0729094c89ea5e1fad821111eaefefdb69815e0e6a0d107c732eb8859d5cecc  crates/desktop/mde-shell-egui/src/datacenter.rs
```

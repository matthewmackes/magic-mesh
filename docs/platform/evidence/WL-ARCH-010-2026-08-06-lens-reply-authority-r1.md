# WL-ARCH-010 IAC lens reply authority — 2026-08-06

## Goal

Remove duplicate whole-history reply readers from the Images and Configure
Execute lenses while keeping their rich payloads on the single Workloads wire
reply reader.

## Implementation

- `crates/desktop/mde-shell-egui/src/iac/images.rs`
  - Image-roster resolution now calls `WorkloadsState::read_wire_reply`.
  - Removed the lens-local unbounded `list_since` reader.
- `crates/desktop/mde-shell-egui/src/iac/configure.rs`
  - Inventory and output resolution now call the same bounded parent reader.
  - Removed the second lens-local unbounded `list_since` reader.

The parent reader uses the SQL-enforced `Persist::list_since_limit` page of one
message and preserves oldest-reply ordering. This keeps the shell's read path
under one authority and prevents a retained reply topic from becoming an
unbounded GUI allocation.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=iac-reply-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui \
  iac::tests::workload_reply_reader_keeps_oldest_reply_without_scanning_history \
  -- --nocapture
```

- Focused hostile regression: 1 passed, 0 failed.
- Updated IAC module: 48 passed, 4 failed out of 52. The four failures are the
  existing deterministic UI/fixture tests outside the reader paths and were
  individually reproduced on separate farm worktrees; no clean full-IAC gate
  is claimed.
- Full-file rustfmt still reports unrelated pre-existing drift in the large
  dirty IAC modules; the changed lens hunks have no formatter diff and no
  whole-file rewrite was applied.
- `git diff --check`: passed locally.

## Source hashes

```text
cd7f936349fb0d85b1e606e3b7cbe79a693456cda3e4c737e102c510ac77e4de  crates/desktop/mde-shell-egui/src/iac/mod.rs
b6a267d7b61c1b6d0a93c0fff2e82090f27c09c4cea2e7b3029dab860e49b946  crates/desktop/mde-shell-egui/src/iac/tests.rs
ffb47679c58daedcb573f93732f2cc131e484bf4f144d65a6cc3a69bbea1b367  crates/desktop/mde-shell-egui/src/iac/images.rs
6acbcf69ca40d7d82314eaef8fbb18cc6a8e320311be965895a9b63b508d3fee  crates/desktop/mde-shell-egui/src/iac/configure.rs
```

## Remaining authority proof

This is a read-side Execute migration only. Workload caller migration, live
libvirt/Quadlet recovery, Display1/KMS, Dell/seat-15 acceptance, and remaining
adapter/recovery queues remain open in the canonical Worklist.

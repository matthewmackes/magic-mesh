# WL-ARCH-010 reply-reader boundedness — 2026-08-06

## Goal

Keep the Workloads/IAC shell reply readers bounded while preserving the RPC
ordering contract: the oldest reply for a correlation ULID wins.

## Implementation

- `crates/desktop/mde-shell-egui/src/iac/mod.rs`
  - `read_reply` now uses the SQL-enforced `Persist::list_since_limit` page of
    one message.
  - `read_wire_reply` uses the same bounded page, so console endpoint decoding
    cannot materialize retained reply history.
- `crates/desktop/mde-shell-egui/src/iac/tests.rs`
  - Added a hostile regression with one oldest reply plus 64 newer duplicates.
  - The reader returns the oldest reply while never requesting more than one
    decoded row.

## Farm verification

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=iac-reply-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui iac::tests -- --nocapture
```

- The new regression `workload_reply_reader_keeps_oldest_reply_without_scanning_history` passed.
- The focused IAC module run was mixed: 48 passed, 4 failed. The failures are
  existing app/UI fixture tests outside the changed reader paths:
  `android_apps_catalog_renders_in_active_plan_and_run_routes`,
  `console_attach_decodes_the_endpoint_and_renders_it_honestly`,
  `lifecycle_reboot_and_delete_are_typed_confirm_gated`, and
  `ui_mutation_requests_carry_their_explicit_placement_node`.
- Each of those four failures reproduced individually on separate farm
  worktrees; this checkpoint therefore does not claim a clean full IAC module
  gate.
- The changed files have no new rustfmt diff. A full-file farm formatter check
  still reports unrelated pre-existing drift in the large dirty IAC modules;
  no whole-file rewrite was applied.
- `git diff --check`: passed locally.

## Source hashes

```text
cd7f936349fb0d85b1e606e3b7cbe79a693456cda3e4c737e102c510ac77e4de  crates/desktop/mde-shell-egui/src/iac/mod.rs
b6a267d7b61c1b6d0a93c0fff2e82090f27c09c4cea2e7b3029dab860e49b946  crates/desktop/mde-shell-egui/src/iac/tests.rs
```

## Remaining authority proof

This is a bounded read-side migration only. It does not close the broader
Workload authority epic: caller migration, live libvirt/Quadlet recovery,
Display1/KMS, Dell/seat-15 acceptance, and the remaining adapter/recovery
queues remain open in the canonical Worklist.

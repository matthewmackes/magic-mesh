# WL-ARCH-010 evidence — bounded VDI console reader (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

The shell's brokered-console resolver no longer scans the entire retained
`state/vdi/console` topic on every half-second poll. The topic is shared by
multiple sessions, so the reader uses the newest 64 messages rather than a
single global latest row; the existing typed resolver still filters by
`session_id` and keeps the newest matching record. This bounds retained-state
materialization while preserving the multi-session broker contract.

Changed files:

- `crates/desktop/mde-shell-egui/src/vdi/mod.rs`
- `crates/desktop/mde-shell-egui/src/vdi/tests.rs`

## Farm verification

All heavy verification ran on the explicit `.90` farm host in isolated slot
`vdiconsole-r1`:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=vdiconsole-r1 \
  install-helpers/xcp-build.sh \
  cargo test -p mde-shell-egui --features live-vdi \
  vdi::tests::console_reader_uses_a_bounded_tail_of_retained_state -- --exact
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=vdiconsole-r1 \
  install-helpers/xcp-build.sh \
  cargo test -p mde-shell-egui --features live-vdi vdi::tests -- --nocapture
result: 72 passed, 0 failed, 2 ignored live-console tests

ssh mm@172.20.0.90 \
  'rustfmt --edition 2021 --check \
   crates/desktop/mde-shell-egui/src/vdi/mod.rs \
   crates/desktop/mde-shell-egui/src/vdi/tests.rs'
result: pass
```

The hostile regression seeds 65 retained console records, proves the reader
returns exactly the newest 64, excludes the oldest record, and retains the
newest record. Required stewardship gates also passed on BigBoy `.130` slot
`drain-gates-r6`: worklist self-test, worklist lint (17 active / 17 Remaining),
document supersession, and Workload authority. Local `git diff --check` passed.

## Runtime and remaining proof

The implementation does not change the installed Dell runtime, reboot a seat,
or claim an endpoint-dependent console result. Live RDP/VNC/SPICE targets,
Display1/KMS, Workload caller migration, restart/crash recovery, and Dell/
seat-15 acceptance remain open under WL-ARCH-010.

## Source hashes at capture

```text
3c0e58ffe040272aaa70e4eecf32b069f0f8ea3515af603ee36ed3309fb01014  crates/desktop/mde-shell-egui/src/vdi/mod.rs
7ab474a25e1c0acc7eab646b842454ffbc7a141d80acabc0e05bb5a07fef83dc  crates/desktop/mde-shell-egui/src/vdi/tests.rs
```

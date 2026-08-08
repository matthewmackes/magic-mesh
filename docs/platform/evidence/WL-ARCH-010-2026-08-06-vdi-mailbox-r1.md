# WL-ARCH-010 evidence — bounded live VDI handoff (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

The live RDP, VNC, and SPICE workers no longer accumulate decoded frames or
operator input in unbounded standard-library channels. Each transport publishes
decoded frames into one shared latest-value mailbox, so a stalled egui consumer
retains at most one frame. Input admission is bounded to 256 events; pointer
motion and wheel events coalesce or drop under pressure, while key and button
releases can evict lower-value queued input so a held guest key cannot be left
without its release solely because of pointer flood. Oversized text events are
rejected before admission.

The transport authority is unchanged: the shell owns the live handle, the
worker owns the protocol session, and the existing `event_rx` remains reserved
for bounded-rate control, error, and clipboard events. No new Bus topic or
second Workload/VDI authority was introduced.

## Farm verification

All heavy verification ran on an explicit farm host and isolated slot:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=vdi-mailbox-r1 \
  install-helpers/xcp-build.sh \
  cargo test -p mde-shell-egui --features live-vdi vdi::tests::
result: 71 passed, 0 failed; 2 ignored live-console tests

ssh mm@172.20.0.90 \
  'cd /home/mm/magic-mesh-farm-vdi-mailbox-r1 && \
   rustfmt --check --edition 2021 \
   crates/desktop/mde-shell-egui/src/vdi/mod.rs \
   crates/desktop/mde-shell-egui/src/vdi/tests.rs'
result: pass
```

The current tree also passed the required stewardship/authority checks on
BigBoy (`172.20.0.130`, slot `drain-gates-r4`):

```text
bash install-helpers/lint-worklist.sh --self-test
bash install-helpers/lint-worklist.sh
bash install-helpers/lint-doc-supersession.sh
bash install-helpers/lint-workload-authority.sh
result: all pass; 17 active items, 17 Remaining, 0 Blocked, 0 Needs clarification
```

The focused regressions are:

- `live_frame_mailbox_keeps_only_the_newest_decoded_frame`
- `live_input_mailbox_bounds_flood_and_prioritizes_key_release`

The broad live-feature shell run on the pre-patch farm snapshot reported
1,469 passed and 12 unrelated existing shell failures; it is retained as
baseline context, not as evidence against this slice. The post-patch focused
VDI suite is the authoritative result above.

## Remaining proof

The endpoint-dependent RDP/VNC/SPICE console tests remain intentionally ignored
until approved live targets are available. Dell runtime services were not
mutated or rebooted. Display1/KMS, Workload caller migration, live Dell/seat-15
acceptance, and the wider WL-ARCH-010 recovery obligations remain `Remaining`.

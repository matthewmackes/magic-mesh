# WL-ARCH-010 console authority removal — 2026-08-08 r1

## Scope

This checkpoint advances S1 and S4 by deleting the retired raw console relay
and its reachable readers. Managed VM presentation now has one path: typed
Workload `Open` / `StartAndAttach`, a bounded Workload projection, and an
authenticated node-local Display1 lease.

The checked inventory is
`docs/platform/workload-authority-inventory.md`. It records publishers,
consumer, projection writer, adapters, session semantics, placement, shell
callers, retired symbols, and the follow-up migration authority boundary.

## Removed reachability

- deleted the 2,463-line `workers/console_broker.rs` worker and its module
  registration;
- deleted cloud `verbs/console.rs` and unclassified `console-attach`, with a
  hostile unknown-verb refusal before auth/backend dispatch;
- removed the shell `state/vdi/console` reader, raw endpoint decoder, polling
  state, and dead Workloads console-handle UI;
- retained the successful Console/Open operation as typed Workload `Open` with
  `QemuDisplay1Dmabuf`;
- made endpoint-less legacy broker sessions fail closed with an actionable
  typed-lease message;
- deleted the obsolete Browser raw transport schema, example, and verifier;
- migrated `verify-live-mirrors.py` from raw console endpoint validation to
  session-rail validation only.

## Negative guards

`install-helpers/lint-workload-authority.sh` now rejects both retired lifecycle
topics, `VmPowerRequest`, `LIFECYCLE_TOPIC`, the console worker/module/dispatch,
the raw console topic, retired Browser attach artifacts, and a retired spawn.
Its self-test injects each class of forbidden source and proves detection.

## Verification

- Local tiny checks:
  - `bash -n install-helpers/lint-workload-authority.sh packaging/browser-vm/verify-contract.sh`
  - `install-helpers/lint-workload-authority.sh --self-test` — passed
  - `install-helpers/lint-workload-authority.sh` — passed
  - `install-helpers/verify-live-mirrors.py --self-test` — passed
  - `install-helpers/lint-worklist.sh --self-test` — passed
  - `git diff --check` — passed
- BigBoy `.130`, slot `arch010-console-retire-r1`:
  `cargo test -p mackesd retired_lifecycle_and_console_verbs_are_refused_before_auth_or_backend -- --nocapture`
  — 1 passed, 0 failed; daemon and all test targets compiled.
- BigBoy `.130`, slot `arch010-shell-open-r1`:
  `cargo test -p mde-shell-egui --features live-vdi lifecycle_and_console_actions_reject_incomplete_workload_identity -- --nocapture`
  — 1 passed, 0 failed.
  The warmed follow-up
  `endpointless_legacy_session_never_revives_raw_console_resolution` also passed
  1/1 and proves no transport is started for the retired endpoint-less shape.
  `ui_mutation_requests_carry_their_explicit_placement_node` passed 1/1 and
  confirms Console/Open emits the typed Workload operation with the selected
  node, workload identity, and Display1 attachment preference.
- `.90`, slot `arch010-browser-contract-r1`:
  `packaging/browser-vm/verify-contract.sh` — passed.

## Source hashes

```text
53ef2b3b4df631efa73ad9361c2b03245cbce1638c3238bb04548e954446363c  install-helpers/lint-workload-authority.sh
d715ad9ae79f3183071fb1bb3ef2b9cacacaf84fbe13c8aa6ab9a75d996f533f  docs/platform/workload-authority-inventory.md
84cb95e588ec71ac25247edaa416ed6691462103fdd281d230b2106d1082e919  install-helpers/verify-live-mirrors.py
c1240861dc702ccc117df291fdeed47d20a8901f30c334e3f02112739525e137  crates/mesh/mackesd/src/workers/cloud/verbs.rs
6bbc0c926940a739fe455c3404f06095ff9c1bf239ac8b240b3dd265574f7f72  crates/desktop/mde-shell-egui/src/iac/mod.rs
36da371e0f6c330fc4a54090f3f1564ddd20975323f17e59f17e0788e18f110a  crates/desktop/mde-shell-egui/src/vdi/mod.rs
daedfc399b86ee01734783b31f6a6904345c3f8f2303defd3eba8f4c8ea0a0b3  packaging/browser-vm/verify-contract.sh
```

## Limitations

This does not close ARCH-010. The follow-up migration checkpoint removed direct
libvirt effects from `compute_migrate`, but its in-process command queue is not
journaled. Native KMS/EGL and full attachment recovery remain incomplete, and
live Dell/seat-15 plus five-seat/three-lighthouse proof is still required.

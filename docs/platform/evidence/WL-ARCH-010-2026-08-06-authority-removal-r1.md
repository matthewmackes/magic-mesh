# WL-ARCH-010 — retired shell lifecycle authority removal (2026-08-06)

Status: implementation and focused farm verification complete; WL-ARCH-010
remains `Remaining` because live Workload adapters, Display1/KMS, restart and
crash recovery, Dell/seat acceptance, and the remaining caller inventory are
still open.

## Implementation

- Removed the unreachable `WorkloadsState::issue_lifecycle_direct` forwarding
  helper from `crates/desktop/mde-shell-egui/src/iac/mod.rs`. The helper was the
  last shell-side compatibility-shaped entry point that could make a legacy
  lifecycle arm look like a direct Workload caller.
- Preserved the typed `issue_workload_direct` path as the sole VM/container
  review/publish seam. The retired `ArmAction::Lifecycle` remains only as a
  bounded render/test shape and `perform_cloud` rejects it before any Bus
  write, with an explicit refusal.
- Updated lifecycle regression coverage in `iac/tests.rs` to prove the retired
  shape leaves `mutation_pending` empty and reports `Nothing was sent`.
- Updated the Front Door service accessibility regression to assert the
  canonical `action/workload/operation` fields (`workload_id`, `target_node`,
  `backend`, and closed `action`) and to reject the retired untyped service
  metadata shape.

## Farm verification

The heavy shell test ran with an explicit farm host and isolated slot:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=execute-authority-removal-ui-r2 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui \
  --no-default-features lifecycle
result: 25 passed, 0 failed; 1,426 filtered out
```

The package formatter was also attempted on `.90` with an explicit slot, but
the package has unrelated pre-existing rustfmt drift in dirty files including
`chooser`, `datacenter`, `display1_client`, `health_modal`, and other shell
modules. No broad formatter rewrite was applied. `git diff --check` passed.

The workload-authority guard passed both its self-test and repository scan:

```text
install-helpers/lint-workload-authority.sh --self-test
install-helpers/lint-workload-authority.sh
result: clean — one typed Workload shell authority and one spawned
workload_compute actuator
```

The negative search found no `issue_lifecycle_direct`,
`action/vm/lifecycle`, `action/container/lifecycle`, `VmPowerRequest`, or
`LIFECYCLE_TOPIC` production-shell symbol. The separate cloud-object
`action/cloud/instance-*` lane is intentionally outside this VM/container
authority migration; its live cloud-provider proof remains open.

## Remaining risk

The retired daemon `lifecycle_exec` implementation is not spawned; its
directory responder remains as an explicit refusal/result-compatibility seam
during migration. A complete daemon inventory and removal proof, live
libvirt/Quadlet execution, crash/restart recovery, Display1/KMS proof, and
Dell/seat-15 acceptance are still required before WL-ARCH-010 can close.

## Source hashes

```text
439145e46445984c08d8ced1adfee7769173792b27f61ef2d52be1d0e1937e72  crates/desktop/mde-shell-egui/src/iac/mod.rs
1679cee699161d555549cf84e001ad7bb06d874bc18636b8718b47accfe87c71  crates/desktop/mde-shell-egui/src/iac/tests.rs
1a98a22fcea870a0ce777e8fd856db26ad992dd29d07da66f0023729c8103dfc  crates/desktop/mde-shell-egui/src/front_door.rs
```

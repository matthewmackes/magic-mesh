# WL-ARCH-010 phone/cloud lifecycle hard cut — 2026-08-09

## Outcome

The KDE Connect host and Phones hub no longer expose placement-wide cloud
start, stop, or reboot commands. Those commands carried only a curated phone
key, with no bounded Workload identity, backend, expected generation, deadline,
or attachment contract, so translating them into wildcard lifecycle effects
would violate the sole Workload authority. `cloud-list` and `cloud-status`
remain read-only, paired-device-gated inventory commands.

The shell's dead `ArmAction::Lifecycle`, cloud lifecycle request builder,
arming path, refusal branch, and historical Explorer cloud-topic fixtures were
deleted. Every live Workloads row now passes a typed
`WorkloadOperationAction` and attachment protocol directly to
`action/workload/operation`; no legacy instance/container verb string selects
the operation. Container restart no longer incorrectly requests a QEMU Display1
attachment.

The producerless shared `LifecycleAction` compatibility type was deleted.
Retired cloud instance lifecycle wire tokens remain recognizable only inside
the cloud consumer's closed refusal list. They are unclassified and return an
actionable no-effect reply before request parsing, placement, authorization,
replay consumption, or backend contact. The stale workspace-destroy advice to
use `instance-delete`, executable request fields, and destructive cloud audit
path were removed.

The authority inventory and lint now reject restoration of the dead shell
symbols, any shell `action/cloud/instance-*` publisher, phone bulk lifecycle
keys, and the shared lifecycle action type. Tests that only exercised deleted
cloud-topic construction or bulk authorization were removed; typed Workload
publication, missing-identity refusal, read-only phone catalog, and cloud
fail-closed compatibility behavior remain covered.

The production-only literal scanner now performs its match in one `awk`
process. This removes a `pipefail`/SIGPIPE false negative that appeared when
`grep -q` found an early match in a large source file.

## Farm verification

- BigBoy (`172.20.0.130`), slot `arch010-phone-lifecycle-shell-r1`: final shell
  `cargo test -p mde-shell-egui --locked -- --nocapture` passed 1485/1485. The
  first run exposed one case-sensitive help-copy assertion; the retained seam
  test was corrected to assert the actual capability and typed-operation
  posture, passed alone, and then passed in the full rerun.
- BigBoy (`172.20.0.130`), slot `arch010-phone-lifecycle-cloud-r1`: focused
  `cargo test -p mackesd --lib --features async-services workers::cloud
  --locked -- --nocapture` passed 199/199 after final synchronization.
- Machine 9 (`172.20.0.50`), slot `arch010-phone-lifecycle-kdc-r1`: focused KDC
  `cargo test -p mackesd --lib --features async-services
  workers::kdc_host::tests --locked -- --nocapture` passed 59/59 after final
  synchronization.
- Machine 193 (`172.20.0.90`), slot `arch010-phone-lifecycle-types-r1`:
  `cargo test -p mackes-mesh-types cloud:: --locked -- --nocapture` passed
  37/37.
- Machine 193 (`172.20.0.90`), slot `arch010-phone-lifecycle-lints-r2`: scoped
  `rustfmt --check`, workload-authority lint and self-test, worklist lint and
  self-test, and documentation-supersession lint all passed. The live worklist
  result was `items=18 remaining=18 blocked=0 needs_clarification=0`.
- Machine 196 (`172.20.0.196`) could not retain enough free space for rsync's
  replacement files even after this slice's exact disposable workspace was
  recreated. Only that workspace was removed; final lint work moved to machine
  193.
- Machine 194 (`172.20.0.170`) had less than 1 GiB free and was not assigned a
  Rust job that was certain to fail with ENOSPC.

## Source hashes

- `776f845309bc52b58fd2ca983293eddf2575ae48507e38451d169eaffedab10a`
  — `crates/desktop/mde-shell-egui/src/explorer/mod.rs`
- `27ebbe12369fcd095cc721c84576a27eeaee3739e859f90bb70c445aebe89365`
  — `crates/desktop/mde-shell-egui/src/iac/mod.rs`
- `596d79dc1a34ccc1dc1afa330e49393ab3819c3164404c9bb162be7b54bfa1ae`
  — `crates/desktop/mde-shell-egui/src/phones_hub.rs`
- `10d7c93407c55098fca4d753ea32e71a996c1b46c1dc3ba2c3f8c6311725d9d8`
  — `crates/mesh/mackes-mesh-types/src/cloud.rs`
- `4d90b590588c56f24876a4a54bc8176f0ff32057451e92f72942a4d6a2e53723`
  — `crates/mesh/mackesd/src/workers/cloud/verbs.rs`
- `fc31b76709d3e8ae28e7027d5a9850f2ab4d0f56af6e33cce3fb0fa7ae6863be`
  — `crates/mesh/mackesd/src/workers/kdc_host/cloud.rs`
- `c719e75f4bdc66a49b0bb1f9852a04feb7914c66e68fd3ea05bf7e16fc00ab82`
  — `crates/mesh/mackesd/src/workers/kdc_host/mod.rs`
- `2ddb9142ed2f35282158811a8e7f3e946aeea061bd8b9798242c53569d1c4ddd`
  — `docs/platform/workload-authority-inventory.md`
- `7cea1e3fd5a6d798fcea4a78b1a205858da549b405568a99bf3dcb006708ffb6`
  — `install-helpers/lint-workload-authority.sh`

## Remaining boundary

This removes another live competing publisher and its stale compatibility
surface, but does not close WL-ARCH-010. Restart/crash injection, native
attachment, package/live-seat proof, and the remaining caller/adapter audit keep
the epic `Remaining`.

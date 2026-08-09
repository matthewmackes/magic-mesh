# WL-ARCH-010 native attachment route authority — 2026-08-09

## Outcome

The sole production Workload actuator now refuses `StartAndAttach` before any
runtime effect unless the request names both the libvirt/virtqemud backend and
the node-local QEMU Display1 DMA-BUF protocol. A retained or replayed Quadlet
request can therefore no longer materialize a container unit or create a
Display1 broker, and an unsupported VNC/RDP/SPICE preference cannot silently
receive a different presentation transport.

The live resource-table path in `iac/mod.rs` now derives VM lifecycle intent
from whether the row owns a console. `DeliveryType::ServiceVm` reaches that
path with `console=false`, so Start emits typed `Start` with no attachment and
Reboot emits typed `Restart` with no attachment. Interactive VM rows retain
their QEMU Display1 Start-and-attach route. The undeclared legacy
`iac/views/service_vm.rs` file is unchanged and provides no evidence here.

## BigBoy verification

Host `172.20.0.130`, slot `arch010-r4-20260809`:

- exact-file `rustfmt --edition 2021 --check`: passed;
- hostile actuator-route regression: 1 passed, 0 failed;
- complete `workers::workload_compute::tests` suite: 38 passed, 0 failed;
- reachable shell regression
  `iac::tests::live_headless_service_vm_lifecycle_intent_has_no_attachment`:
  1 passed, 0 failed, 1,493 filtered out;
- workload-authority self-test and repository scan: passed; the scan reports
  one typed Workload actuator/projection and no retired lifecycle/console path;
- scoped authoritative-worktree `git diff --check`: passed.

The first farm command used `--locked` and refused before compilation because
the concurrently changed workspace requires a lockfile refresh. The successful
commands used farm-local `--offline` resolution and did not alter the
authoritative `Cargo.lock`.

## Source hashes

- `3ef39b825565f86bc4a27ac93ea04d13ac3725165891626da40642f37035b7fe`
  — `crates/mesh/mackesd/src/workers/workload_compute.rs`
- `6937e26b3e71d3c30d17e878f0c8cae866de923a74853ef64de42ca067859fdd`
  — `crates/desktop/mde-shell-egui/src/iac/mod.rs`
- `480cc076d43ae702f83b4f47620394afcafdc9186cee21f51a920ab107df9702`
  — `crates/desktop/mde-shell-egui/src/iac/tests.rs`

## Remaining boundary

This correction proves the typed local route decision and replay defense. It
does not prove a live libvirt VM start, native first frame/input/audio/clipboard,
remote RDP/SPICE/VNC recovery, package install/upgrade, or the required Dell and
seat-15 lifecycle matrix. WL-ARCH-010 therefore remains `Remaining`.

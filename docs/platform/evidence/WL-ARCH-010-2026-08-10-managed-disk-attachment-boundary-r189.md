# WL-ARCH-010 — managed VM disk attachment boundary

- Date: 2026-08-10
- Scope: the Workload VM domain builder rejects disk paths that are not a
  direct `.qcow2` child of `/var/lib/mde-vms` before producing libvirt XML.
- Implementation: `crates/mesh/mackesd/src/workers/workload_vm.rs`
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `arch010-disk-attachment-safety-r189`
- Gate:
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-disk-attachment-safety-r189 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_vm::tests::definition_refuses_disk_attachment_outside_managed_pool -- --exact --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4724 filtered out`.
- Hostile cases cover parent traversal, nested pool paths, external paths,
  relative paths, and non-qcow2 extensions. No libvirt command is reached by
  this pure domain-construction boundary.
- Live limit: this proves deterministic pre-libvirt admission only; it does
  not prove a physical guest boot, storage mount policy, or Dell/seat-15
  attachment lifecycle.

# WL-ARCH-010 evidence — VM identity bound (r221)

- Scope: Workload-owned libvirt domain construction.
- Change: domain and network identities are bounded to 128 bytes before XML
  construction, while existing XML escaping remains intact.
- Farm host: `172.20.0.130` (BigBoy).
- Farm slot: `arch010-vm-identity-bound-r221`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-vm-identity-bound-r221 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_vm::tests::definition_refuses_unbounded_domain_and_network_identities -- --exact --nocapture`
- Result: `1 passed; 0 failed; 4750 filtered out`.

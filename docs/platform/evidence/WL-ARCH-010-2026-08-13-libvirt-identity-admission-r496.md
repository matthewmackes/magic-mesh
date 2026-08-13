# WL-ARCH-010 — fail-closed libvirt identity admission (r496)

- Recorded: 2026-08-13T12:18:17Z
- Scope: `crates/mesh/mackesd/src/workers/workload_vm.rs`
- Source SHA-256: `36e7b0ff5d053054b7b8e370a0b0ab0d70a4cb648ee7dace38cf9c2439d53091`

## Result

The sole Workloads libvirt-domain builder now refuses empty, boundary-whitespace,
and control-bearing domain or network identities before producing XML for the
libvirt actuator. The existing byte bound remains enforced.

This closes a reachable lifecycle failure loop: a retained request with a
bounded but invalid identity can no longer repeatedly reach `virsh define` and
fail there without an actionable typed admission result. Valid escaped
identities and the managed disk/domain binding remain unchanged.

## Farm gates

- `.90` / `arch010-vm-identity-clippy-r496b`: strict
  `cargo clippy -p mackesd --lib -- -D warnings` passed on the unmodified shared
  tree in 5m47s.
- `.90` / `arch010-vm-identity-clippy-r496b`: the exact standalone Rust harness
  for the dependency-free production module passed 1/1
  (`tests::invalid_libvirt_identities_fail_before_domain_definition`; 8
  filtered). Package-level strict Clippy above proves the same source integrates
  into `mackesd`.
- `.50` / `arch010-vm-identity-fmt-r496`: file-scoped
  `rustfmt --edition 2021 --check` passed.
- Local orchestration-only `git diff --check`: passed.

The initial `.170` package-test link was abandoned after the farm host stopped
accepting prompt SSH command execution; it is not counted as evidence. `.130`
was excluded after its toolchain state changed to `bare`. No source workaround
was applied in any final gate.

## Remaining epic acceptance

Package/repository gates, one real libvirt/Quadlet `StartAndAttach` readiness
path, native KMS/Display1 recovery, and the deferred post-release installed-seat
and fleet lifecycle matrix remain open.

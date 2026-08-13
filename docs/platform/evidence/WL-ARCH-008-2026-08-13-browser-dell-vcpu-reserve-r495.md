# WL-ARCH-008 Browser Dell vCPU reserve — r495

Date: 2026-08-13

## Result

The reachable `browser-provision` handler now emits the S4 Dell-safe Browser VM
baseline with three guest vCPUs. The prior generic four-vCPU default consumed
every hardware thread on a four-thread seat, leaving no CPU reserve for Dom0,
the shell, libvirt, or the VDI transport. The Browser-specific desired-state
builder now overrides that generic default before persistence, while retaining
the existing 8-GiB memory, 64-GiB disk, immutable image identity, and canonical
`browser-vm` route.

The existing Browser baseline regression was strengthened in place to assert
both the exact three-vCPU contract and the invariant that the guest consumes
fewer than four host threads. No duplicate test was added.

## Farm verification

- `.90`, slot `arch008-browser-vcpu-test-r495c`: the fully qualified focused
  regression passed 1/1:
  `workers::cloud::verbs::browser::tests::browser_profile_is_the_baseline_desktop_vm_shape`.
- `.50`, slot `arch008-browser-vcpu-clippy-r495`: strict
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.170`, slot `arch008-browser-vcpu-filefmt-r495`: touched-file
  `rustfmt --edition 2021 --check` passed.

The first focused invocation selected zero tests because `--exact` was paired
with an unqualified name and was rejected as evidence. A corrected `.196`
rerun was refused by the farm's 8-GiB sync floor, and its disposable workspace
was removed. A corrected `.50` attempt was stopped after the host became
unresponsive during final test compilation; the final 1/1 result above came
from `.90` without source modification or a farm-only workaround. Package-wide
fmt exposed unrelated pre-existing mackesd formatting drift; the authorized
file passed its direct farm check.

## Remaining epic acceptance

This slice closes the Browser-specific Dell CPU-reserve gap. WL-ARCH-008 still
requires its remaining portable-import, guest image/audio, package, and
post-release multi-seat Browser VM performance/readiness proof.

# WL-ARCH-010 Dell typed capacity-refusal evidence — 2026-08-08

## Scope

This slice proves that Dell's four-thread workstation cannot start the Browser
Standard profile through a side channel. It exercises the installed Release 21
Workload request helper, live compute reconciler, node-owned capacity probe,
typed operation/state projections, and libvirt boundary. It does not claim a
successful Browser first frame or complete ARCH-010 lifecycle closure.

## Live prerequisites

The read-only Workloads verifier initially found the following live Dell seams
healthy: all six grouped workers with zero restarts, one shell, a root-only
encrypted cloud-arm credential and matching service drop-ins, Workstation role
pin, Podman 5.8.4 with its socket active, `/dev/kvm`, libvirt 12.0.0, the
`default` network, and the `mde-vms` pool with 159.74 GiB available. The
existing `browser-vm` domain was shut off and retained its Display1 plus SPICE
recovery definition.

The direct RPM proof transaction does not resolve weak dependencies, so Dell
lacked the package's `ansible-core` recommendation. `ansible-core-2.20.7-1.fc44`
and its Fedora dependency were installed without replacing any package. After
one bounded `mackesd-compute.service` restart, the fresh
`state/cloud/DELL-LAPTOP` projection reported OpenTofu, Ansible, and libvirt
`up`, `apply_armed=true`, and no required cloud-mirror blocker. The same restart
created the previously missing empty schema-1
`state/workloads/peer:DELL-LAPTOP` projection.

## Typed refusal

Dell exposes four logical CPUs. The Browser helper's Standard profile requests
four guest vCPUs, 8192 MiB RAM, and a 64 GiB disk; the Workload contract must
reserve at least one CPU for the host. The installed
`request-browser-vm-workload` helper submitted one capability-bound
`start_and_attach` request for `vm:peer:DELL-LAPTOP:browser-vm`. The credential
was decrypted only into a root-owned mode-0600 `/run` file because Dell's
transient-unit API reset the credential-scoped launch; an unconditional trap
removed that temporary file, and no credential bytes were printed or retained.

The helper returned `operation-failed`. The follow-up verifier required and
accepted all of these exact live facts:

- fresh authorized operation ULID `01KZJ3G3E75HSDXKBWPGFN9W9Q`, SHA-256
  `c64230bdf0c9b207b918a30b97fca52a7e8b04e2bb19e201ffc2656fdd60c44f`;
- target node `peer:DELL-LAPTOP`, Workload
  `vm:peer:DELL-LAPTOP:browser-vm`, action `start_and_attach`, and a redacted
  present armed token;
- fresh schema-1 state ULID `01KZJ3G3JES45N8EVCX5HWCSSM`, SHA-256
  `fb685007b9c865aefb300ecac1f66704e59c11b1c495db132051f9cdcdc0db9a`;
- expected terminal phase `failed`, no attachment, no actuator attempt, and no
  retryable state; and
- healthy Workstation placement, libvirt/KVM, cloud mirror, shell, and all six
  worker services.

The authoritative projection reported:

```text
phase=failed
power=failed
readiness=failed
attempt=0
retryable=false
reason=workload would consume the reserved host CPU
resources={vcpu:4,memory_mb:8192,disk_gb:64}
attachment=null
```

`virsh domstate browser-vm` remained `shut off`, proving refusal occurred before
the libvirt actuator or Display1 lease path.

## Accuracy correction and validation

The live Release 21 projection correctly refused capacity but inaccurately
recommended the Standard profile, which was already selected. The source now
recommends the defined Small profile for both CPU-reserve and memory-reserve
denials. A focused unit test locks that operator-visible remediation for both
denial paths. Farm 9 passed a targeted `rustfmt --check` for
`workload_compute.rs`. Farm 196's isolated `arch010-small-profile-r1` slot ran
out of space while writing Cargo's incremental cache before the test could run;
that exact abandoned scratch slot was removed. The same focused test then
passed on farm 193 in isolated slot `arch010-small-profile-r2`: one passed, zero
failed, and 4,491 unrelated library tests filtered out.

## Remaining boundary

ARCH-010 remains `Remaining`. A larger Workstation must still take one typed
StartAndAttach operation through admission to a native first frame, and live
restart, cancellation, remote recovery, container, audio, clipboard, and
multi-seat evidence remain open.

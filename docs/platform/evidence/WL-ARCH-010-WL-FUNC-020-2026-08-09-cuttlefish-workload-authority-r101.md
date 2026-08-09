# WL-ARCH-010 / WL-FUNC-020 — Cuttlefish Workloads authority r101

Date: 2026-08-09

## Correction

Cuttlefish's production provider no longer rediscovers its outer Android VM
through the Cloud runner's libvirt roster. Cloud reads and validates one typed
`WorkloadStateSnapshot`, copies only the matching VM-backed status and exact
generation into the provider, and distinguishes two materially different
states:

- an available authoritative snapshot without the target is honest `Absent`;
- an unavailable, stale, malformed, or unreadable authority is
  `ProviderUnavailable`, never fabricated absence.

A same-ID Quadlet/container row is rejected before it can become Android VM
readiness. Guest-owned package/session evidence remains required before a
running outer VM can publish a ready VDI source, and loss of outer readiness
still revokes retained guest state.

The dead `CloudRunner::list_instances` seam, its production `virsh list` plus
`domstate` implementation, normalization helper, and obsolete normalization
test were deleted. The runtime-authority scanner no longer allowlists those
commands; its hostile Cloud and Cuttlefish fixtures remain fail-closed.

## Focused farm proof

Host: `172.20.0.90` (farm machine 9)

Slot: `arch009-nebula-dispatch-r98`

Command:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=arch009-nebula-dispatch-r98 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services cuttlefish --locked -- --nocapture
```

Result:

```text
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 4617 filtered out
```

The set includes hostile unavailable-authority, same-ID non-VM, package drift,
identity drift, reconnect cleanup, and readiness-revocation coverage.

Repository gates:

```text
install-helpers/lint-workload-authority.sh --self-test
PASS — lifecycle and presentation guards are fail-closed

install-helpers/lint-workload-authority.sh
PASS — one typed Workload actuator/projection; retired lifecycle and console paths absent

git diff --check
PASS
```

Implementation artifacts at proof:

```text
e57382b701be3188f72f3c6532bd937aa686c9fc4365148768603b23a21dfde2  crates/mesh/mackesd/src/workers/cloud/mod.rs
6f78b88ba432920064134901486e25973819e5f568a7f58d0e5ceb4f9e7f915f  crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish.rs
a259e8b45d7cd9c62b414a101909e1db1bfeca8c2d0ed895f00cbfbe9c87ae3b  install-helpers/lint-workload-authority.sh
```

## Remaining acceptance gap

This removes the duplicate Cloud/Cuttlefish runtime reader; it is not a live
nested-KVM Android launch. WL-FUNC-020 still needs signed release artifacts,
installed guest packaging, remote attachment, isolation, and five-seat live
proof. WL-ARCH-010 still has direct storage inventory debt outside the typed
Workloads adapter plus broader migration, native attachment, and live
acceptance work.

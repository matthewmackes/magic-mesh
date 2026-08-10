# WL-ARCH-010 — bounded virtio-net queue fan-out (r144)

Date: 2026-08-10

Source revision: `624b03b9`

## Result

Managed VM XML now requests one virtio-net queue per admitted guest vCPU when
the host retains a VM-free Dom0 thread, with an eight-queue ceiling. A host
without that capacity falls back to one queue. The XML leaves the backend name
unset, preserving libvirt/QEMU backend selection and existing service
compatibility while allowing multiqueue acceleration where available.

## Focused farm proof

Machine 193 build VM `.90`, slot `arch010-virtio-net-r144`:

```text
cargo test -p mackesd --lib workers::workload_vm::tests:: -- --nocapture
```

Result: 3 passed, 0 failed, 4,682 filtered. Focused rustfmt and
`git diff --check` passed. No physical seat was used.

## Remaining boundary

This proves bounded XML generation only. Runtime libvirt admission, guest
driver negotiation, IRQ placement, and Dell/seat-15 first-frame/network proof
remain part of WL-ARCH-010 live acceptance.

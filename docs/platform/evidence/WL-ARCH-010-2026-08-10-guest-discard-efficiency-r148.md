# WL-ARCH-010 — guest discard efficiency (r148)

Date: 2026-08-10

The sole libvirt Workload VM definition now requests `discard='unmap'` on its
qcow2 virtio disk while retaining `cache='none'`, native I/O, the single
managed I/O thread, Dom0-reserved CPU pinning, and bounded virtio-net queues.
Guest TRIM/discard can therefore release unused overlay blocks without adding
a second storage authority or changing the disk path boundary.

Focused farm gate on `.90`, slot `arch010-disk-discard-r148`:

~~~text
cargo test -p mackesd --lib workers::workload_vm::tests:: -- --nocapture
3 passed, 0 failed, 4,682 filtered
~~~

This is XML-generation proof only; live libvirt discard negotiation and Dell
guest trim remain part of the outstanding native VM acceptance boundary.


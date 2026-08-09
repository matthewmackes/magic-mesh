# WL-ARCH-010 physical-storage Workloads authority — r102

- Date: 2026-08-09
- Base revision: `3f260a944468145626360850c0eaa5fa82fce687`
- Owned implementation: `crates/mesh/mackesd/src/workers/storage.rs`
- Post-format source SHA-256: `1660d5f8cef92c45fa75f02147b865623449e21b2f3681bd8319cdf66e25092a`

## Correction

Physical storage no longer shells `virsh list/domblklist/domname` or `podman
ps/inspect` to build an independent runtime in-use roster. The production hard
wall consumes the bounded typed `state/workloads/<node>` projection and admits
it only when it is duplicate-key-free, within the Workload wire bound, schema-
valid, same-node, not future-dated, and no older than 120 seconds.

The Workloads contract intentionally does not expose host block-device backing
paths. Therefore any active or failed VM/container makes every physical disk
unverifiable and destructive storage operations fail closed. A disk is proven
free only when current typed authority contains no Workload outside `Defined`
or `Stopped`. Missing, stale, malformed, future, or wrong-node authority also
returns the existing typed `InUseUnknown` refusal.

The retired direct-roster helpers and their raw backing-path test were deleted.
Physical safety and legitimate storage behavior remain unchanged:

- `/proc/self/mountinfo` still protects the root, boot, EFI, and mesh-storage
  backing disks.
- UDisks2 remains the physical topology authority.
- Typed arming and stage/apply topology revalidation remain mandatory.
- parted, filesystem tools, mount/umount, and SELinux commands remain the
  privileged storage mutation path.

## Focused farm verification

Host/slot:

```text
MCNF_BUILD_HOST=172.20.0.50
MCNF_BUILD_SLOT=arch010-storage-isolated-r98
```

Command:

```text
install-helpers/xcp-build.sh cargo test -p mackesd --lib --features async-services physical_storage_uses_typed_workloads_authority_and_fails_closed --locked -- --nocapture
```

Result:

```text
running 1 test
test workers::storage::tests::physical_storage_uses_typed_workloads_authority_and_fails_closed ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4631 filtered out
```

The hostile regression covers absent authority, active VM/container authority,
the stopped/defined free proof, wrong-node data, stale data, and duplicate-key
malformation. `git diff --check` passed for both owned files. The crate emitted
258 pre-existing warnings; this slice added no warning diagnosed in
`storage.rs`.

## Residual direct reads

- `storage.rs` has no remaining `virsh` or Podman command/read path.
- `virtual_storage.rs` retains legitimate Podman storage inventory and mutation:
  `volume ls`, `system df`, `volume create`, `volume rm`, and `volume prune`.
  It no longer uses Podman process/inspect runtime inventory for in-use walls.
- Cloud retains `virsh ... version` only as a libvirt capability probe, not a
  lifecycle or runtime roster.
- The sole direct libvirt/Podman lifecycle observation and actuation authority
  remains `workload_compute`; no live-seat deployment or destructive disk action
  was needed for this code-only authority migration.

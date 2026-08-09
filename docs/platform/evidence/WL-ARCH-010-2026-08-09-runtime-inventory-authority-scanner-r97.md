# WL-ARCH-010 runtime-inventory authority scanner — 2026-08-09 r97

## Scope

Base revision: `0ce7b4072320ffea89593de384d5f9ca21e9fb39` with concurrent,
uncommitted worktree changes preserved. This checkpoint advances S1/S4; it does
not close `WL-ARCH-010`.

`install-helpers/lint-workload-authority.sh` now scans production Rust, shell,
and Python sources under all of `mackesd/src` and the egui shell for direct
`virsh`/`podman` command literals, the Cloud `list_instances` seam, and the
existing storage runtime wrappers. The exact
`workers/workload_compute.rs` path is excluded because it is the sole Workload
runtime adapter. Existing reviewed reads are line-exact and count-limited, so
removal passes while duplication or argument-shape changes fail.

The self-test independently rejects hostile Cloud, Cuttlefish, storage, and
shell fixtures and accepts a `virsh domstate` fixture only at the
`workload_compute` adapter path. Only the item or block carrying `#[cfg(test)]`
is excluded, and a fixture proves production declarations after it remain in
scan scope.

## Focused proof

Farm host: `.90` (`172.20.0.90`)  
Farm slot: `arch010-authority-scanner-r97`

The supported farm sync populated
`magic-mesh-farm-arch010-authority-scanner-r97`; the isolated directory has no
Git metadata, so whitespace proof ran in the source worktree.

```text
bash -n install-helpers/lint-workload-authority.sh
install-helpers/lint-workload-authority.sh --self-test
install-helpers/lint-workload-authority.sh
git diff --check
```

Result: exit 0. The self-test reported its lifecycle/presentation guards
fail-closed, and the repository scan reported one typed Workload
actuator/projection with retired lifecycle and console paths absent.

Scanner SHA-256:
`30e5231400dd8c08529fbf30bb0b5ad4a86a43909748e50a79233a8c79d78475`.

## Residual direct reads and scanner limits

- Cloud's retained runner still reads libvirt `list`/`domstate`, and the
  Cuttlefish provider still consumes `list_instances`; these are the known r94
  caller-migration debt.
- `storage.rs` and `virtual_storage.rs` still read running libvirt domains and
  Podman mounts for conservative in-use decisions. Their exact current lines
  are pinned, not declared migrated.
- The scan also pins legitimate non-inventory Podman image/volume operations,
  service-name literals, and the libvirt version health probe so changing their
  command shape requires review.
- A runtime binary assembled entirely through variables/constants, an indirect
  wrapper with none of the scanned symbols, generated code, or operational
  scripts outside `mackesd/src` and the egui shell are not detected by this
  bounded scanner.

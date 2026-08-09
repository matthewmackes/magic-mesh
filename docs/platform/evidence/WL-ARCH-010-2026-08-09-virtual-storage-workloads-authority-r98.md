# WL-ARCH-010 virtual-storage Workloads authority — 2026-08-09 r98

## Corrected authority boundary

Base revision: `8a24dd07b9201ef5a43c11f13fcd48d98a7159aa`, with concurrent
worktree changes preserved. This checkpoint advances S1/S4 and does not close
`WL-ARCH-010`.

The virtual-storage destructive in-use wall previously enumerated running
libvirt domains with `virsh list`/`domblklist` and running containers with
Podman `ps`/`inspect`. That created a second VM/container runtime roster beside
the typed `state/workloads/<node>` projection.

`ComputeVirtualInUse` now reads only the bounded, duplicate-key-rejecting,
node-matched, current typed Workloads projection. Because the Workloads contract
intentionally carries catalog identity rather than host image paths or volume
mount details, any active or failed VM makes a specific image unverifiable and
any active or failed container makes a specific volume unverifiable. Existing
destructive operations therefore refuse with `Unknown`. A current projection
with no active workload for the relevant backend proves that backend's resource
free. Missing, unreadable, oversized, malformed, wrong-node, stale,
future-dated, or recursively duplicate-key projections also fail closed.

The retired runtime probe and its now-dead `bounded_stdout` helper were removed.
The remaining `output_with_timeout` and `DEFAULT_CMD_TIMEOUT` imports are live:
the Podman storage runner still uses them for bounded volume inventory and
mutation commands.

## Focused farm proof

Farm host: `.50` (`172.20.0.50`)  
Farm slot: `arch010-storage-isolated-r98`

To avoid consuming unrelated, concurrently incomplete edits, the supported
farm helper synced detached base revision `8a24dd07` plus only the assigned
`virtual_storage.rs` change.

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=arch010-storage-isolated-r98 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services \
  typed_workloads_authority_is_conservative_and_hostile_rows_fail_closed \
  --locked -- --nocapture
```

Exact terminal result:

```text
warning: `mackesd` (lib test) generated 258 warnings (run `cargo fix --lib -p mackesd --tests` to apply 2 suggestions)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5m 28s
     Running unittests src/lib.rs (target/debug/deps/mackesd_core-7b2dac935c32c5ff)

running 1 test
test workers::virtual_storage::tests::typed_workloads_authority_is_conservative_and_hostile_rows_fail_closed ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4628 filtered out; finished in 0.01s
```

The hostile regression covers absent authority, active VM and failed-container
authority, stopped/defined workloads, a stale projection, and recursive
duplicate keys. The final compile emitted no dead-code warning for
`virtual_storage::bounded_stdout`.

Final `virtual_storage.rs` SHA-256:
`c30045751570661d4187aac9f386906f61d6abad12a7aa5fe1181b32fbcf5d54`.

## Residual direct reads

- `storage.rs` still directly enumerates running libvirt domains and Podman
  mounts to protect physical block devices; that coherent physical-storage path
  remains for migration.
- `virtual_storage.rs` still invokes Podman for its legitimate storage adapter:
  volume list, system-df, create, remove, and prune. Those are storage
  inventory/mutations, not VM/container lifecycle or in-use roster authority.
- Workloads does not expose adapter host paths. The conservative wall therefore
  cannot identify the exact active workload using a requested image or volume;
  it intentionally refuses all existing resources of an active backend.

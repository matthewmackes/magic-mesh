# WL-ARCH-010 Cloud Workload-roster authority — 2026-08-09 r94

## Scope

Base revision: `2900834445abf0c21f701ad6e417c0e8de4bf7c9` with an uncommitted
worktree patch. This checkpoint advances `WL-ARCH-010` without Dell, seat 15,
battery/status-bar layout, or RDP/LAN discovery work.

Generic Cloud `list`, `list-instances`, `list-instances-local`, and `status`
replies and the `state/cloud/<node>` compute resource table no longer call the
Cloud runner's direct `virsh` inventory. They read `state/workloads/<node>`,
require the exact local node, reject duplicate JSON keys, enforce the Workload
wire bound and schema, and fail closed on absent, stale, future, or malformed
projections. Only VM-backed Workloads become compute instances; a Quadlet
container cannot leak into the VM roster.

The hostile fixtures give the fake backend a contradictory `backend-bypass`
domain and prove it is ignored. They also prove that an absent or 120-second-old
Workload projection produces a gated reply rather than falling back to backend
inventory. Existing placement-local reads now publish and consume the typed
projection.

This is not epic closure. The drift reconciler and Cuttlefish provider still
contain direct `list_instances` reads and require a later bounded migration to
the typed Workload projection.

## Farm verification

Host: machine 196, `172.20.0.196`  
Slot: `arch010-cloud-workload-authority-r93`

Final command:

```text
MCNF_BUILD_HOST=172.20.0.196 \
MCNF_BUILD_SLOT=arch010-cloud-workload-authority-r93 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services workers::cloud --locked -- --nocapture
```

Result: `208 passed; 0 failed; 4419 filtered out`. The first full run passed
206 tests and exposed one stale direct-roster test; after converting that test
to the typed projection, the second run passed 207/207. A final hostile stale-
projection fixture raised the final green count to 208/208.

The crate-wide formatting check is currently blocked by unrelated pre-existing
format drift in other `mackesd` files. The two changed Cloud files were formatted
directly with the farm Rust toolchain, copied back, and `git diff --check` passed.

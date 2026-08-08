# WL-ARCH-010 evidence — Quadlet materialization and backend admission (2026-08-06)

Working-tree changes are intentionally uncommitted. Dell runtime was not
mutated; review synchronization is limited to source, evidence, and handoff
files.

## Implementation slice

The sole `SystemWorkloadActuator` container path now:

- validates the promoted, non-empty regular OCI archive already required by
  the catalog boundary;
- checks local Podman storage and loads that exact archive when the approved
  `name:version` image is absent;
- renders a bounded Quadlet unit from the typed Workload identity and catalog
  image, using a deterministic SHA-256 suffix so replacing `:` separators
  cannot collide;
- atomically installs the unit under `/run/containers/systemd/`, reloads the
  system systemd generator, and removes/reloads it at typed Destroy.

The legacy cloud `container-deploy` handler now fails closed before parsing,
staging, or invoking Ansible. The shell Containers lens retains its local
Quadlet preview but no longer publishes a legacy lifecycle request while the
typed create declaration is not available.

The mesh type contract adds `WorkloadStorageCapacity` and
`admit_workload_for_backend`, selecting independent VM or container storage
capacity and reservations. The existing mackesd live capacity probe is not
yet migrated to provide this new record; that follow-up remains open under
ARCH-010 S5.

## Farm verification

All commands were run through `install-helpers/xcp-build.sh` with isolated
slots and explicit hosts:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-quadlet-materialize-20260806-r2 \
  ./install-helpers/xcp-build.sh cargo test --locked -p mackesd \
  workload_compute::tests::quadlet_materialization_is_tied_to_typed_workload_identity -- --nocapture
```

Result: **1/1 passed**. The first full workload run reached 25/26 and exposed
only a fixture assertion error; it was corrected and rerun as the focused
1/1 gate. No source failure was claimed from that first run.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-admission-pools-20260806-r1 \
  ./install-helpers/xcp-build.sh cargo test --locked -p mackes-mesh-types \
  workloads::tests::backend_admission -- --nocapture
```

Result: **2/2 passed**. The exact slot was removed after the gate.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-container-retire-shell-20260806-r2 \
  ./install-helpers/xcp-build.sh cargo test --locked -p mde-shell-egui \
  iac::containers -- --nocapture
```

Result: **4/4 passed**. An earlier `.90` attempt stopped at dependency
compilation with `ENOSPC`; the exact slot was removed and the gate was rerouted
to `.50`, where it completed successfully.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-container-retire-20260806-r4 \
  ./install-helpers/xcp-build.sh cargo test --locked -p mackesd \
  workers::cloud::verbs::container::tests::retired_deploy_refuses_before_staging_or_runner_activity -- --nocapture
```

Result: **1/1 passed**. The retired cloud handler refuses before any Quadlet
staging or external runner activity.

The focused cloud refusal gate above is the signed negative-path check for the
retired adapter.

## Local gates and capacity

```text
./install-helpers/lint-worklist.sh --self-test       passed
./install-helpers/lint-workload-authority.sh        clean
git diff --check                                     passed
```

At capture, `.50` reported 3.3 GiB free and BigBoy `.130` 6.3 GiB free. The
`.90` full-home condition was handled by removing only the exact failed slot;
`.170` was not used. These are capacity-relative facts, not a claim that the
farm is idle or release-ready.

## Source hashes at handoff

```text
WORKLIST.md                                      2079a177d32c01b1284d2b21f201c216d5347dedbe6ffb71583a659f6fdd28a7
workload_compute.rs                              f6ddc449a89bb390a3d8bb2abd9cdd4003a42e95f5196f0ae6f42d6240154319
cloud/verbs/container.rs                         77d16480f4b81459af6ee300e2aadab48a69c79c09a34e54d9260a0dac4ba4d5
mde-shell-egui/iac/containers.rs                ae186d83be4d3df86f9c961a057ae915d75ab8fec1b9730f07c26c2ef60feb96
mackes-mesh-types/workloads.rs                   ad488f7e755ed24b4b641e7e837d49608d10e9d75a90a61c2c0306d96281e6f6
```

## Remaining acceptance

This checkpoint does not claim live Podman/Quadlet execution, backend-specific
capacity probe wiring, restart recovery, native KMS/EGL attachment, packaging,
or Dell/seat-15 proof. Those remain Remaining under ARCH-010.

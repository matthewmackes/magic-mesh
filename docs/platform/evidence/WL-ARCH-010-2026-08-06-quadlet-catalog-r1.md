# WL-ARCH-010 — Quadlet catalog admission (2026-08-06)

## Deliverable

`SystemWorkloadActuator` now resolves Quadlet `image_ref` values only through
the promoted Workload image catalog. It requires the container manifest kind,
the exact `PROMOTED` version, and a non-empty regular `.oci.tar` artifact; it
rejects symlinked artifacts. `StartAndAttach` and `Start` validate this before
creating a Display1 attachment or invoking systemd.

Source SHA-256:

```text
a918c804465bc6a58fefaff125f64844a1aee5ee66ea26e8d061994879c46683  crates/mesh/mackesd/src/workers/workload_compute.rs
```

## Hostile validation

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-quadlet-catalog-20260806-r2 install-helpers/xcp-build.sh cargo test -p mackesd workload_compute::tests::approved_container -- --nocapture
```

Result: **2/2 passed** on `.90`, covering promotion/catalog kind, empty
artifact, version mismatch, and symlink rejection. An earlier BigBoy attempt
was rerouted after the VM reported `ENOSPC` during metadata emission; no test
failure was observed. Temporary farm slots were removed after verification.

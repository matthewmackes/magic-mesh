# WL-ARCH-010 shell Workload projection duplicate-key gate — 2026-08-08

Status: focused projection-boundary verification passed; live seat rendering,
mutation acceptance, restart/recovery, and Dell/seat-15 acceptance remain open.

## Verification

The shell Workload projection reader rejects duplicate JSON object keys before
decoding `state/workloads/<node>`, preventing a hostile duplicate `node` field
from becoming authoritative UI state.

Farm command on `.50`, slot `arch010-shell-authority-duplicate-20260808-r2`:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=arch010-shell-authority-duplicate-20260808-r2 \
install-helpers/xcp-build.sh cargo test --locked -p mde-shell-egui \
  workload_api::tests:: -- --nocapture
```

Result: PASS — 5 passed, 0 failed, 1454 filtered out; build and test completed
in 5m32s. The five tests covered typed action publication, capability-bound
image/request/cancel validation, and duplicate-key projection rejection.

Source SHA-256:

```text
dcaf1486ed8449f0e76ecad17f703d478d0298060dead9ba6eeec388d4112456  crates/desktop/mde-shell-egui/src/workload_api.rs
```

This evidence does not claim the unavailable `.90` storage gate: that separate
job stopped at the linker with `mold: failed to write to an output file. Disk
full?` and produced no test result.

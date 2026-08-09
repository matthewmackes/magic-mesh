# WL-ARCH-010 Cloud drift Workload-roster authority — 2026-08-09 r96

## Corrected authority boundary

Cloud list/status and resource tables already consumed the typed
`state/workloads/<node>` projection, but the periodic drift fold still called
the Cloud runner's direct `virsh list` inventory. That left two contradictory
runtime truths inside one worker and converted backend read failure into an
empty roster, which falsely marked every desired workload absent.

The drift tick now reads the same bounded, local-node, fresh Workload projection
as all other Cloud roster consumers and passes it explicitly into the pure drift
fold. The reconciler no longer calls direct runtime inventory. If Workload
authority is unavailable, desired rows are published as `unknown`, unreachable,
and `DriftFlag::Unknown`; they are never fabricated as absent or in sync. A
present typed roster still distinguishes running and genuinely absent Workloads.

The hostile regression gives the fake backend a contradictory running domain
while withholding the typed roster. The backend row cannot leak into the Cloud
projection and cannot become evidence of reachability.

## Farm verification

Machine 9 (`172.20.0.90`), slot
`cloud-drift-workload-authority-r95`:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=cloud-drift-workload-authority-r95 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services workers::cloud::reconcile --locked -- --nocapture
```

Result: `19 passed; 0 failed; 4609 filtered out`. Existing warning debt was
outside this bounded change. The remaining direct Cloud inventory consumer is
the Cuttlefish provider observation path; this checkpoint does not claim its
migration or ARCH-010 closure.

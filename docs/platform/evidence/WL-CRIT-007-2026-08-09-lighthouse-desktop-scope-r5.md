# WL-CRIT-007 Lighthouse desktop-recovery scope — 2026-08-09

## Correction

Peer-return recovery now admits communal XDG bind restoration only for the
Workstation role. A healthy Lighthouse publishes
`skipped-workstation-xdg-lighthouse` and converges without starting the
Workstation-only `mcnf-xdg-bind-recovery.service`. Previously every healthy
Lighthouse return event started that role-inapplicable unit, making coordination
recovery depend on desktop state that a Lighthouse does not own.

The hostile configured-Lighthouse regression presents healthy Nebula, etcd,
Syncthing, and all six grouped mackesd services. Recovery reports
`already-recovered` with an empty mutation ledger and the explicit XDG skip.
Existing Workstation recovery continues to require successful XDG restoration.

## Farm verification

- BigBoy `172.20.0.130`, slot `crit007-r4-20260809`.
- Exact helper and fixture shell syntax passed.
- The complete peer-recovery fixture passed: offline refusal, malformed and
  unsupported roles, missing Lighthouse membership, configured-Lighthouse XDG
  isolation, ordered substrate restoration, downstream cutoff, boot races,
  healthy replay, single-flight coalescing, and resume/network trigger filtering.
- Local scoped `git diff --check` passed.

Source SHA-256:

```text
dc4a9b84a333f20723b61b6d1a1f98ce94efc19b9f38bfbba2e5c0fce3feca88  install-helpers/mesh-peer-recovery.sh
05339c534b507ca5995950f7fa1e8f52185fc9d0c91eff4fa91153d3e6105e58  install-helpers/test-mesh-peer-recovery.sh
```

## Remaining live limitation

This bounded fixture proves role-correct recovery mutation only. Physical
suspend/resume and live network-return convergence across the remaining Eagle,
T480, Surface, and Lighthouse fleet matrix remain required; WL-CRIT-007 stays
`Remaining`.

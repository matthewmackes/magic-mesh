# WL-UX-013 / WL-UX-014 Grade E authority — 2026-08-09 r5

## Supersession boundary

This record supersedes only the Grade-E-fail-closed statement in
`WL-UX-014-2026-08-09-shared-health-kiron-contract-r4.md`. The earlier file is
left unchanged as truthful evidence for its candidate. Its other contract,
validation, shell-mapping, and live-limit statements remain historical evidence.

UX-013 now defines all six production grades in the canonical shared health
type. UX-014 carries that exact authority grade and does not regrade in the
shell:

- any distinct active required critical condition produces F;
- otherwise two or more distinct active required warning identities produce E;
- otherwise one distinct active required warning produces D;
- with no actionable condition, capability/headroom produces A, B, or C.

Optional, informational, resolved, and wrong-node conditions do not contribute.
Duplicate delivery of the same `(scope, condition id)` does not escalate the
grade, and the strongest severity wins for that identity. Scope is part of the
identity: equal `disk-pressure` IDs from two different node scopes remain two
distinct mesh conditions and correctly produce mesh grade E.

The one `HealthKironAlert` contract maps E to critical attention and an exact
15,000 ms timed dwell. F remains critical and held until acknowledgement.
Unknown future grade letters still fail closed at typed deserialization.

## Changed authority and contract files

- `crates/mesh/mackes-mesh-types/src/health.rs`
  (`492a3c445d3b17bf02eeaa185c6b6f2c793c8d7fa0b04b4f7c610aa57eb5cbc0`)
- `crates/mesh/mackesd/src/workers/node_grade.rs`
  (`8992f745a3ad1ea24b5592c61fd220795625eb891c9ddad0c26eef8f3849194d`)
- `crates/desktop/mde-shell-egui/src/toast_bridge.rs`
  (`58ea5fc7b1351b8e71b33f5e1660064f4db0e1a95d28eebcb69f77dfd50edffb`)
- `docs/design/node-grade.md`

No Clock, ARCH-010, or `docs/platform/WORKLIST.md` file was edited by this
slice.

## Farm proof

Primary host: machine 9 `172.20.0.50`  
Slot: `ux013-grade-e-r5-20260809`

- complete shared `health::tests` module: 18 passed, 0 failed;
- hostile same-node duplicate-identity test retained D;
- hostile equal-ID/two-node-scope mesh fold produced E;
- node and mesh compounded-warning policy tests produced E;
- canonical A-F KIRON round-trip, attention, and dwell test passed;
- worker-generated two-warning production path: 1 passed, 0 failed;
- `health.rs` and `toast_bridge.rs` exact-file Rustfmt passed; the new
  `node_grade.rs` test was farm-formatted while unrelated pre-existing file
  drift was retained outside this patch;
- scoped `git diff --check`: passed.

Independent corroboration supplied during this run: machine 194
`172.20.0.170` passed its node-grade tests, and machine 196 `172.20.0.196`
passed the shell shared-health mapping test 1/1. The redundant cold shell build
on machine 9 was stopped after that independent shell result was available; no
machine-9 shell pass is claimed.

## Remaining production limits

This closes only Grade E authority and the shared KIRON contract mapping. It
does not close either epic.

WL-UX-013 still lacks production expected-state publishers and complete
planned/unplanned boot, sleep, shutdown, maintenance, network-loss, and rejoin
traces; bounded history/detail/filter/recurrence storage and modal behavior;
governed recovery plus redacted export; and wide/narrow/largest-text plus
five-seat/lighthouse live proof.

WL-UX-014 still lacks the six governed scene and audio asset packages with
license/hash/size manifests; grouping/ticker and full interruption controls;
deterministic live-3D, pre-rendered, and static render tiers with device-loss
recovery; package/upgrade proof; and direct-DRM five-seat evidence for visuals,
audio, suspend/resume, lock, immersive, multi-display, and performance. No
scene, audio, fallback-tier, packaging, or live-seat readiness claim is made.

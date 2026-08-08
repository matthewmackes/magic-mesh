# WL-ARCH-009 — worker-runtime schema admission

Status: hostile contract checkpoint complete; process split, package, and
fleet convergence remain `Remaining`.

## Change

The shared worker-runtime contract test suite now exercises unknown schema
versions at every versioned boundary: worker contract, relation, timeline
event, runtime snapshot, change-set request, and change-set result. Each must
refuse the row before typed admission.

## Verification

Farm `.90`, slot `worker-runtime-schema-admission-20260806-r2`:

```text
8 passed, 0 failed, 426 filtered out
```

No process unit, worker ownership table, or live runtime was changed by this
slice.

## Source hash at capture

```text
a01c0f6bcdd71ae85ff15510e402cc0298567d99f86a91513c30e042ee170fe4  crates/mesh/mackes-mesh-types/src/worker_runtime.rs
```

# WL-ARCH-009 — Network Operations typed route cutover (r547)

Date: 2026-08-13

## Implemented result

The canonical `shell/goto/mesh-map` action now resolves directly to
`WorkersDestination::MeshMap`. Applying that action opens `Surface::Workers`,
selects its Network mode, and selects the Mesh Map leaf without activating the
retired sibling `Surface::MeshView` route.

Explorer's live "Open in Fleet" action now emits the canonical route. The
retired `shell/goto/meshview` and `shell/goto/mesh` spellings are no longer
accepted, so stale publishers cannot preserve the old route as a compatibility
bypass.

## Hostile regression

`mesh_map_action_enters_typed_workers_leaf_without_legacy_surface` starts from
the collapsed Remote Sessions surface, applies the external Mesh Map action,
and proves that the shell expands into the exact typed Workers leaf. It also
proves that both retired spellings fail closed. The resolver and onboard
self-test regressions independently pin the same typed destination.

## Farm gates

- BigBoy slot 2: the one requested focused regression run compiled the current
  production changes but stopped before execution on two test-module imports
  introduced by this slice. Both imports were corrected exactly; cadence
  prohibited a rerun, so this invocation is recorded as failed compilation and
  not as passing test evidence.
- `.170` slot 2: the one requested all-target/all-feature build found the same
  two test-module imports and, independently, the farm image lacked `libmpv` at
  final binary link (`mold: fatal: library not found: mpv`). The imports were
  corrected; the external fixture failure remains. This run is not represented
  as green.
- BigBoy slot 3: strict all-target/all-feature Clippy was queued behind the
  focused gate's target lock. It was cancelled without compiling when the stop
  cadence arrived; no Clippy claim is made.
- No formatting rerun or broader gate was added after the stop cadence.
- Scoped `git diff --check` passed on the final source.

## Remaining ARCH-009 acceptance

- Audit and cut over the remaining Fleet, Workbench, Explorer, This Node,
  System, Storage, About, and Phones aliases that still normalize through
  legacy `Surface` values.
- Complete Network Operations geo/fabric/flow/history projections and remove
  superseded help/package references.
- Finish S5 responsive/largest-text evidence and provider/action ownership
  inventory.
- Build the first full release, then run deferred non-blocking one-node process
  isolation, staged-change/partial-failure, recovery, and live capture
  acceptance. Additional nodes remain optional.

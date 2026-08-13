# WL-UX-009 S4 — finite spring timeline recovery (r505)

Date: 2026-08-13

## Production gap closed

The shared `Motion::spring_to` path accepted non-finite geometry targets,
persisted position/velocity, frame deltas, and caller-provided spring
coefficients. A poisoned value propagated through `Spring::step`; because NaN
can never satisfy the settle threshold, the direct-DRM runner then requested a
repaint on every event-loop iteration indefinitely.

The centralized carrier now repairs invalid timelines to a finite, zero-velocity
settled pair. An invalid incoming target preserves the last finite visual
position; invalid stored state, frame time, or spring coefficients settle at the
finite requested endpoint. Reduced-motion mode also stores its settled endpoint,
so poisoned temporary memory cannot survive a compatibility-mode transition.
Normal finite spring behavior and first-sight endpoint behavior are unchanged.

## Farm evidence

- `.50`, slot `ux009-motion-spring-test`:
  `cargo test -p mde-egui poisoned_spring -- --nocapture` — passed 1/1, 309
  filtered.
- `.50`, same warm slot:
  `cargo test -p mde-egui spring_context_driver_repairs_poisoned_memory_without_repaint_churn -- --nocapture`
  — passed 1/1, 309 filtered.
- `.90`, slot `ux009-motion-spring-clippy`:
  `cargo clippy -p mde-egui --lib -- -D warnings` — passed.
- `.170`, slot `ux009-motion-spring-fmt`:
  `cargo fmt -p mde-egui -- --check` — passed.
- `git diff --check` — passed for the shared tree.

## Remaining WL-UX-009 acceptance

This closes the identified S4 non-finite spring/idle-loop gap only. The epic
still requires complete Construct-surface Style/Visuals adoption, supported
appearance/responsive capture coverage, release payload verification, and the
deferred post-release direct-DRM motion/focus/human-review evidence. No shell
surface, asset, Cargo metadata, or canonical worklist entry was changed here.

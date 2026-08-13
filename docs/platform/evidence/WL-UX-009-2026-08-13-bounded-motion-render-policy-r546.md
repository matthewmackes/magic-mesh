# WL-UX-009 — bounded motion render policy (r546)

Date: 2026-08-13

## Result

The shared Quazar motion resolver now rejects non-finite and non-positive
durations by settling immediately, and clamps every finite caller-supplied
timeline to a centralized one-second maximum. A malformed or bespoke motion
spec therefore cannot retain repaint authority indefinitely in the
event-driven direct-DRM runner. Normal and reduced-motion modes share the same
fail-closed boundary; disabled motion remains endpoint-only.

The hostile regression covers `NaN`, positive and negative infinity, negative
timing, and `f32::MAX`. It proves invalid timelines settle on their first frame
and an oversized finite timeline reaches its endpoint and releases repaint
authority within the shared bound.

## Farm gates

All valid gates ran in `magic-mesh-farm-2` on BigBoy (`172.20.0.130`, slot 2):

- Focused hostile regression:
  `cargo test -p mde-egui motion::tests::hostile_motion_durations_cannot_hold_the_renderer_awake -- --exact --nocapture`
  — passed 1/1 (311 filtered out).
- Strict relevant Clippy:
  `cargo clippy -p mde-egui --all-targets -- -D warnings` — passed.
- Relevant build: `cargo build -p mde-egui --all-targets` — passed.
- Format check: `cargo fmt -p mde-egui -- --check` identified one line-wrap
  in the new assertion; that exact Rustfmt delta was applied without a filler
  rerun.
- Scoped `git diff --check` — passed after the formatting delta.

An earlier focused invocation is not counted as evidence: another farm cleanup
deleted its active target directory during compilation, producing only
missing-file infrastructure errors. The fully qualified regression above was
then run in an ownership-safe workspace and is the authoritative result.

## Remaining WL-UX-009 acceptance

- Finish inventory and migration of Construct-owned surfaces that still bypass
  shared Style/Visuals or motion primitives.
- Complete deterministic wide/narrow/tablet/largest-text and Dark/Light render
  fixtures and review.
- Package the frozen font/icon/style registry in the first full release.
- After that release, perform the deferred non-blocking direct-DRM motion trace,
  reduced-motion capture, and full visual-consistency review.

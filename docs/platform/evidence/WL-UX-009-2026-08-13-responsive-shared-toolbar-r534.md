# WL-UX-009 — responsive shared toolbar (r534)

Date: 2026-08-13

## Production gap

The shared Quazar `Toolbar` positioned its leading and trailing action groups
independently on one fixed-height row. At narrow widths or the largest
text/touch density, the measured groups could overlap. That made visible actions
ambiguous and could leave their hit targets underneath another control in every
workspace that composes this shared primitive.

## Implementation

`crates/shared/mde-egui/src/nav_chrome.rs` now measures both action groups before
allocating the toolbar. It keeps the compact inline arrangement when the groups
fit and allocates two disjoint density-aware rows when they do not. The stacked
arrangement retains every action, stable combined action indices, keyboard
activation, AccessKit labels, shared Quazar colors/motion, and the configured
minimum hit-target floor. `ToolbarResponse::layout` exposes the selected runtime
arrangement to consuming workspaces without creating a second layout authority.

## Farm evidence

- `172.20.0.196`, slot `ux009-toolbar-focused-r534b`: exact responsive toolbar
  regression passed 1/1 (`310` filtered out).
- BigBoy `172.20.0.130`, slot `ux009-toolbar-suite-r534`: full `mde-egui` library
  suite and production compile passed 311/311.
- `172.20.0.170`, slot `ux009-toolbar-clippy-r534`: strict
  `cargo clippy -p mde-egui --all-targets -- -D warnings` passed.
- `172.20.0.50`, slot `ux009-toolbar-fmt-r534b`: package
  `cargo fmt -p mde-egui -- --check` passed. No later work was scheduled on
  `.50` after its low-free-space advisory.
- Local scoped `git diff --check` passed; no heavy local gate ran.

The superseded `.90` focused invocation selected zero tests because `--exact`
was paired with an unqualified name. It is intentionally excluded from proof;
the qualified `.196` invocation above is the authoritative focused result.

## Remaining WL-UX-009 criteria

This closes one shared responsive-runtime gap, not the epic. Remaining work is
the complete active-surface Style/Visuals migration and inventory, supported
Dark/Light/responsive/largest-text/stale/unavailable coverage, centralized
motion/focus/repaint proof, font/icon/package verification, and post-release
direct-DRM/human consistency review. Proof and live acceptance remain deferred
and non-blocking until after the first full release under the current operator
direction.

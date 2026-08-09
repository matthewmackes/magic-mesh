# WL-UX-009 shared workspace-state responsive Light correction — 2026-08-09

## Production correction

`crates/shared/mde-egui/src/widgets.rs` now budgets `WorkspaceStatePanel` width for
both card padding and its hairline stroke, wraps title/detail copy, and resolves
the shared surface, border, semantic state, and text tokens against the installed
Quazar appearance. This prevents the shared empty/offline/error card used by
Terminal and other adopters from escaping a narrow viewport and from painting
Dark tokens in the windowed Light path.

The deterministic regression renders an Offline panel at 112 x 480 logical
points with Touch density and Quazar Light. It requires the complete response
geometry to remain inside the available viewport and requires Light surface,
strong-title, and dim-detail pixels before the DRM-only post-frame remap.

## Farm proof

- Host: `172.20.0.50`
- Slot: `ux009-workspace-state-responsive-r1-20260809`
- Hostile first run: failed because the response exceeded the viewport by the
  one-point border on each side; the final width budget includes that border.
- Focused final regression: 1 passed, 0 failed; 288 filtered out.
- Complete `widgets::tests` module: 12 passed, 0 failed; 277 filtered out.
- Exact-file Rustfmt using Rust 1.94.0: passed.
- Scoped `git diff --check`: passed locally after the formatted file was returned.
- Final source SHA-256:
  `4042610de322999068cde6492d15b803d09b5b46c57ec1fa3ec809ed44e233ce`.

This is bounded implementation evidence only. It does not claim the remaining
WL-UX-009 adoption inventory, render matrix, direct-DRM review, or epic closure.

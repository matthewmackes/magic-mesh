# WL-FUNC-016 workgroup-root seam gate — 2026-08-11

## Scope

The collab file-link command now keeps production root resolution unchanged
(`MDE_WORKGROUP_ROOT`, falling back to `/mnt/mesh-storage`) while exposing a
scoped root seam for non-live tests. The regression uses a temporary root, so
farm seat mount permissions cannot mask content-address behavior.

## Farm evidence

- `.50`, slot `func016-fix-test`: focused link regression — 1 passed, 0 failed.
- `.170`, slot `func016-fix-clippy`: `cargo clippy -p mde-collab-egui --lib` —
  passed with warnings only.
- `.170`, slot `func016-suite`: full `mde-collab-egui` library suite — 132
  passed, 0 failed.
- `.170`, slot `func016-build`: `cargo build -p mde-collab-egui` — passed in
  7m15s.
- Earlier parallel baselines: SPICE 48/48, VNC 115/115, and Files 156/156.

The original baseline failure was deterministic `PermissionDenied` while
creating `/mnt/mesh-storage` on an unmounted farm seat. The fix preserves the
production path and makes the test root explicit.

## Remaining acceptance

Live guest/seat clipboard proof remains deferred under the instruction to
postpone acceptance testing until after the first full workspace build.

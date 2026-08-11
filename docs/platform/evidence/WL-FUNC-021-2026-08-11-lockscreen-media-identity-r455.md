# WL-FUNC-021 lockscreen media identity — 2026-08-11

- Scope: locked-screen media controls bind to the exact media identity rendered in the retained snapshot.
- Hostile boundary: replacement media cannot inherit a retained transport action.
- Focused gate: `cargo test -p mde-shell-egui --bin mde-shell-egui curtain::tests::retained_locked_strip_cannot_control_replacement_media -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, exact hostile regression passed.
- Remaining boundary: exercise replacement playback through a live lock-screen transport and daemon receipt.

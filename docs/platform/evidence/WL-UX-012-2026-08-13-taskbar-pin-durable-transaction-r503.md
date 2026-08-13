# WL-UX-012 taskbar pin durable transaction — 2026-08-13

## Result

Taskbar pin and unpin operations now publish their in-memory projection only
after the exact bounded pin identities are durably written. A failed preference
transaction leaves the visible projection unchanged, so a shell restart cannot
silently undo a pin state that the running taskbar claimed was committed.

The transaction also rejects duplicate, out-of-catalog, and over-capacity pin
sets before persistence. Restart decoding was exercised against the exact bytes
written by the transaction.

## Farm evidence

- Host `.50`, slot `ux012pins-final-r503`:
  `cargo test -p mde-shell-egui
  taskbar_pin_projection_commits_only_after_exact_preferences_are_durable --
  --nocapture`
- Result: 1 passed, 0 failed, 1,579 filtered out.
- The cold shell build completed in 10 minutes 8 seconds.
- File-scoped `rustfmt --edition 2021 --check` was previously reported passing
  on `.50` for `crates/desktop/mde-shell-egui/src/nav_bar.rs`.
- A broad all-target Clippy attempt reached unrelated pre-existing test lints in
  `health_modal.rs`, `car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`; it
  is not claimed as a passing gate for this slice.

## Remaining acceptance

The first full release must include the current shell bytes. Responsive
Bottom/Left, restart/upgrade, and direct-seat review remain deferred and
non-blocking until after that release under the active acceptance policy.

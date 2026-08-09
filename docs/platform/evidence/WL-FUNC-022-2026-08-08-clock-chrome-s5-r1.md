# WL-FUNC-022 Clock and bell chrome split S5 — 2026-08-08

Bottom and Left clock targets now open `Surface::Clock` directly. A separate
bell opens Notification Center and caps its unread badge at `99+`. Exposing
visible retained rows marks those rows read without clearing them. Geometry and
target IDs keep weather, live battery, time, bell, health, and placement actions
disjoint while preserving curtain and focused-VDI gesture guards.

## Verification

Machine 9 (`172.20.0.50`), slot `func022-clock-bell-chrome-s5-r1`:

```text
cargo test --locked -p mde-shell-egui --bin mde-shell-egui \
  --no-default-features status_bar::tests:: -- --nocapture
27 passed; 0 failed; 1455 filtered out
```

Four exact regressions also passed independently: bounded unread/read exposure,
bottom taskbar clock composition, first-swipe VDI guarding, and curtain click
capture. Scoped formatting, diff checking, and the locked no-default shell
check passed. Because concurrent Music edits were incomplete at gate time, the
disposable farm copy used the committed Music baseline; a current combined-tree
default-feature gate remains required.

## Remaining acceptance gap

Typed actionable Clock banners, retained action metadata, explicit keyboard
focus traversal, idle curtain Clock content, and Bottom/Left direct-DRM captures
remain. FUNC-022 stays `Remaining`.

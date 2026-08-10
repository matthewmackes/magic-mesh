# WL-UX-012 — taskbar pin deduplication (r156)

Date: 2026-08-10

Taskbar pin projection now removes duplicate surfaces while preserving the
user's first-seen order, ensuring each centered slot has one typed identity.

## Farm proof

BigBoy (`172.20.0.130`), slot `ux012-pin-dedupe-r156b`:

```text
cargo test -p mde-shell-egui --bin mde-shell-egui taskbar_pin_projection_preserves_order_without_duplicate_center_slots -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 1541 filtered out
```

Responsive captures and live three-seat proof remain open.

# WL-UX-012 narrow center geometry — 2026-08-09 r3

## Production correction

Bottom taskbar center-lane admission now follows physical 40px hit geometry.
At 480px, the one usable centered slot is reserved for More so hidden sessions
and apps remain reachable. At 320px, no center control is emitted because it
would overlap Home. Zero-capacity catalog selection no longer fabricates an
overflow target outside the admitted lane.

## Farm proof

- Host: `172.20.0.50`
- Slot: `ux012-narrow-center-geometry-r3-20260809`
- Command: `cargo test -p mde-shell-egui --bin mde-shell-egui nav_bar::tests -- --nocapture`
- Result: 50 passed, 0 failed.
- Exact-file `rustfmt --check`: passed.
- Local scoped `git diff --check`: passed.
- `nav_bar.rs` SHA-256: `f8995d2b27f508efe9aa314c7e34557faaa153225bf9b8a271068ec7113777de`.

No live-seat or visual-capture claim is made by this deterministic geometry
slice.

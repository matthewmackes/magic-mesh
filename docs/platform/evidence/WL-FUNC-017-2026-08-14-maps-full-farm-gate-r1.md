# WL-FUNC-017 Maps full farm gate

- Date: 2026-08-14
- Revision: `109a174e091983877187dd636ebfb15b6e0f504a`
- Farm: `.90` `172.20.0.90`, slot `maps-full-audit`
- Command: `cargo test -p mde-maps-location-egui --lib`
- Result: 324 passed, 0 failed, 0 ignored.
- Defect repaired: malformed schema-v1 cache indexes now invalidate their index without deleting unbound payloads; valid legacy ownership is still quarantined.
- Boundary: live Maps/provider and installed-seat acceptance remain owned by `WL-TEST-001`.

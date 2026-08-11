# WL-UX-012 Home wallpaper inode — 2026-08-11

- Scope: Bing Home wallpaper decode consumes one bounded regular-file descriptor.
- Hostile boundary: symlink, device/FIFO, oversized, or replaced cache paths cannot redirect image decoding.
- Focused gate: `cargo test -p mde-shell-egui --bin mde-shell-egui backdrop::wallpaper_decode_tests::replaced_home_wallpaper_path_cannot_redirect_decode -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, exact hostile regression passed.
- Remaining boundary: capture Home rendering and cache replacement recovery on direct-DRM hardware.

# WL-FUNC-021 media full farm gate

- Date: 2026-08-14
- Revision: `109a174e091983877187dd636ebfb15b6e0f504a`
- Farm: `.90` `172.20.0.90`, slot `media-full-fix-audit-2`
- Command: `cargo test -p mde-media-egui --lib`
- Result: 114 passed, 0 failed, 0 ignored.
- Defect repaired: Jellyfin fixtures now use the supported VP9 baseline; the capability bridge explicitly refuses optional H.264 and negotiates transcode rather than claiming direct play.
- Boundary: physical renderer/provider/cast and installed-seat acceptance remain owned by `WL-TEST-001`; no second-seat proof is required.

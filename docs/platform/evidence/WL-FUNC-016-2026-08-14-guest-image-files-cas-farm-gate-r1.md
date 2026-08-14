# WL-FUNC-016 guest image Files/CAS farm gate

- Host/slot: `172.20.0.90` / `clipboard-files-image`
- Command: `cargo test -p mackesd --lib guest_clipboard_image_commit_publishes_exact_cas_and_files_identity`
- Result: 1 passed, 0 failed.
- Host/slot: `172.20.0.90` / `clipboard-shell-image-live`
- Command: `cargo test -p mde-shell-egui --features live-vdi --bin mde-shell-egui rdp_guest_image`
- Result: 2 passed, 0 failed.

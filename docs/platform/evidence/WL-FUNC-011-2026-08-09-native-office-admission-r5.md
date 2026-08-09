# WL-FUNC-011 S6 native-office admission r5

- Scope: production `mde-editor-egui` office open boundary; no `WORKLIST.md`, device-control, workload-compute, deployment, or package mutation.
- Correction: Document, Spreadsheet, and Presentation extensions are intercepted before the UTF-8 rope loader. Admission rejects symlinks, non-regular files, and files over 256 MiB, then reports the absent sandboxed adapter without opening a tab, changing bytes, or claiming a session.
- BigBoy `172.20.0.130`, slot `func011-office-r5`: `cargo test -p mde-editor-egui office_ -- --nocapture` passed 5/5; exact-crate `cargo fmt --check` passed.
- Repository audit: no production LibreOfficeKit adapter or package contract exists. Fedora 42 offers `libreoffice-core`, `libreofficekit`, and `libreofficekit-devel`, but `libreofficekit` exposes `/usr/lib64/liblibreofficekitgtk.so`; that GTK/VCL bridge is forbidden by S6 and was not used or packaged.
- Remaining blocker: implement and package a safe out-of-process LibreOfficeKit adapter, then prove sandboxed open/edit/save/recovery and native rendering for all three office families. Current source SHA-256: `office_session.rs` `b6634c1846e8fe1857a96629b73bfdd23ee0965df996cd8dafd0f5f9c7713f6f`.

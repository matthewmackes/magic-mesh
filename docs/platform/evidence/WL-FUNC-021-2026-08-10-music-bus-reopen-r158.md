# WL-FUNC-021 Music Bus replacement recovery — r158

- Revision: `89e4cc64`
- Scope: Music reuses the Bus SQLite handle, reopens after inode replacement, and retries after read errors.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-music-bus-r158 install-helpers/xcp-build.sh cargo test -p mde-music-egui --lib workspace_reader::tests::reader_reopens_replaced_bus_index_and_converges -- --nocapture`
- Result: `1 passed; 0 failed; 69 filtered out` on seat 90.


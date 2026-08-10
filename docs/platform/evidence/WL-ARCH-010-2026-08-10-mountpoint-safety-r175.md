# WL-ARCH-010 mountpoint safety — r175

- Scope: physical-storage `Mount` admission rejects relative, root, and lexical-traversal destinations before privileged geometry execution; normal absolute destinations remain admissible.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-mountpoint-safety-r175 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::storage::tests::validate_mount_unmount_and_resize_directions -- --nocapture`
- Result: `1 passed; 0 failed; 4709 filtered out` on seat `.90`; the gate compiled the changed storage worker.

# WL-ARCH-010 virtual image failure cleanup — r168

- Scope: `qemu-img` create/convert/clone destinations are refused when already present and partial destinations are removed when the backend fails; existing bytes are retained.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-virtual-cleanup-r168 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::virtual_storage::tests::failed_new_image_operation_cleans_partial_destination_and_never_overwrites -- --nocapture`
- Result: `1 passed; 0 failed; 4708 filtered out` on BigBoy.

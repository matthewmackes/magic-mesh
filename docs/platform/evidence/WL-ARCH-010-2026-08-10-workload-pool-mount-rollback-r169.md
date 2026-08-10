# WL-ARCH-010 workload-pool mount rollback — r169

- Scope: post-mount subtree/SELinux/restorecon failures now attempt a bounded `umount`; a failed cleanup is surfaced as a typed `workload_pool_cleanup` error.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-storage-rollback-r169 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::storage::tests::workload_storage_preview_is_workstation_only_and_rechecks_exact_extent -- --nocapture`
- Result: `1 passed; 0 failed; 4708 filtered out` on seat `.90`; the gate also compiled the changed storage worker.

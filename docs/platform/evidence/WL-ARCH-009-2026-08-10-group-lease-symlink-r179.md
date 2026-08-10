# WL-ARCH-009 worker-group lease symlink safety — r179

- Scope: process-group ownership leases open with kernel-enforced `O_NOFOLLOW|O_CLOEXEC`, refusing a lock-leaf symlink even if it changes after metadata inspection.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-group-lease-r179 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::tests::group_lease_refuses_symlinked_lock_leaf -- --nocapture`
- Result: `1 passed; 0 failed; 4711 filtered out` on seat `.90`.

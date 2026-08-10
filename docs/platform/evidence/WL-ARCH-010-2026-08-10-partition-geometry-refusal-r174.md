# WL-ARCH-010 partition geometry refusal — r174

- Scope: storage admission rejects zero-sized partitions and `start + size` overflow before topology fit or destructive geometry execution.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-partition-geometry-r174 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::storage::tests::validate_create_partition_rejects_zero_size_and_geometry_overflow -- --nocapture`
- Result: `1 passed; 0 failed; 4709 filtered out` on seat `.90`; the gate compiled the changed storage worker.

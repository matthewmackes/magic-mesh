# WL-ARCH-010 storage name safety — r177

- Scope: physical-storage admission rejects path-like LUKS mapper names and absolute/traversal btrfs subvolume names before privileged fs-tool execution; nested normal relative subvolume names remain supported.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-storage-name-safety-r176 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::storage::tests:: -- --nocapture`
- Result: `56 passed; 0 failed; 4654 filtered out` on seat `.90`; all storage worker tests compiled and passed.

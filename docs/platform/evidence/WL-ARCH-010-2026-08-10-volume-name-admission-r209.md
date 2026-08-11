# WL-ARCH-010 — backend volume-name admission (r209)

- Scope: Podman create/remove operations reject empty, option-like, traversal,
  slash, whitespace, Unicode, and oversized volume identities before topology.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-volume-name-admission-r209 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::virtual_storage::tests::volume_name_admission_rejects_backend_unsafe_identities_before_topology -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4738 filtered out`.

# Device authority backstop — farm gates (2026-08-12)

## Scope

This slice verifies the pending device-manager authority change: a device
control request may target only a freshly published selected mesh node owned by
the appropriate worker. Remote, stale, or forged selections remain read-only
at every menu, row, and dispatch seam.

## Farm evidence

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=device-manager-authority-test-20260812a MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo test -p mde-shell-egui --locked remote_mesh_node_inventory_is_read_only_at_every_device_control_seam
Finished `test` profile in 10m 16s
1 passed, 0 failed; 1 filtered test target executed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=device-manager-authority-clippy-20260812b MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo clippy -p mde-shell-egui --locked --features live-vdi --bin mde-shell-egui
Finished `dev` profile in 1m 41s
exit 0; warnings only
```

The clippy warnings are existing workspace inventory; no error or denied lint
occurred. The source edit in `crates/desktop/mde-shell-egui/src/device_manager/mod.rs`
remains user-owned, unstaged, and untouched by this evidence commit.

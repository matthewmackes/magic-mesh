# WL-FUNC-019 Remote Sessions catalog rollback refusal (r5)

Date: 2026-08-09

Remote Sessions previously admitted a structurally valid older catalog from the
same publisher after a newer snapshot. That could roll the browser back to old
resource state. The shell now rejects same-publisher timestamp rollback and
same-timestamp equivocation, retains the last-good visible snapshot, marks the
feed conflicted, and revokes cached Android Start and Workload Cancel handles.

Farm proof used `172.20.0.50`, slot
`func019-remote-rollback-r1-20260809`:

- `cargo test -p mde-shell-egui remote_sessions_rejects_same_publisher_rollback_and_revokes_action_handles -- --nocapture`: 1 passed, 0 failed.
- `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/vdi/resources.rs`: passed.
- Source SHA-256: `3109dc388a1abdf1a2707247ee63af8f106896427ad58120a6ffb592f4c81163`.

This is a focused fail-closed regression. It does not claim the epic's remaining
responsive captures or five-seat loss/rejoin recovery proof.

# WL-FUNC-019 — peer App target binding (r22)

Date: 2026-08-10

Base revision: `b2895c9f`

## Defect and correction

A legacy peer-App reply could carry a row naming a different node from the
authenticated reply peer. The fold previously accepted that row and could turn
the foreign node into a launch target.

Rows with an empty legacy node still inherit the authenticated reply node for
rolling compatibility. An explicit conflicting node is now discarded before it
can become a resource or action target.

## Focused farm proof

Machine 193 (`172.20.0.90`) passed the exact
`legacy_cross_peer_row_is_rejected_instead_of_becoming_a_launch_target`
regression: 1 passed, 0 failed, 1,536 filtered out. `git diff --check` passed.

Source SHA-256:

- `3c6feba2680cc0d1f1959e1b26e9fff95bcb7f7e7397239f7c6b89eaf0ed0679`
  — `crates/desktop/mde-shell-egui/src/front_door_peer_apps.rs`

This closes one silent target-substitution path. Universal live discovery and
three-seat-maximum action/recovery proof remain open.

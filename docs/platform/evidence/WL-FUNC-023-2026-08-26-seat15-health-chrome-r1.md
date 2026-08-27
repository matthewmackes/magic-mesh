# WL-FUNC-023 — Seat 15 Health chrome after wipe — r1

Date: 2026-08-26  
Classification: live-seat leftover; **not** dest-cut, freeze, or
`production_admitted`  
`production_admitted: false`

## Authority

Operator 2026-08-26: wipe/reset Seat 15, then continue the dest-cut /
Health Fix goal.

## Live Health (daemon)

Snapshot
`/mnt/mesh-storage/system-mesh-health/snapshots/Basement-Test-Workstation.json`
after re-enroll: mesh grade **F**, 18 warnings / 3 critical /
21 unacknowledged, **0 reachable lighthouses** in the summary (overlay
ping to `10.42.0.1` was ok). Seat 15 conditions include
`lighthouse-unreachable`, `xdg-binds-down`, `collab-identity-missing`,
`cloud-arming-missing`, `mesh-storage-missing` (critical),
`workstation-audio`. Remediation is still dest-gated
(`open_onboarding` / `recover_xdg_binds` / `restore_workstation_audio`).
No dest was invented.

## Chrome

Dest-cut Construct pid **2790** had been running since dest-cut boot
07:12 and survived the wipe. Packed action was **Open Control Panel** /
`shell/goto/workers`. No `power-honor.json`.

Farm (BigBoy `.130` slot 3) ran the dirty-tree Health units:

- `mesh_wide_reports_node_scoped_required_conditions` ok
- `missing_snapshot_does_not_report_a_healthy_zero` ok
- `shared_health_contract_maps_into_toast_host_without_regrading` ok
- `resolve_action_maps_goto_and_plane_verbs` ok

Then `cargo build -p mde-shell-egui --release --features drm,live-vdi,media-mpv`.
Binary sha256 `d8baeefe7b6bbdb0f199d9202bb6dbf0f560cb11f1eba4c0944c6b13c8ecbadd`.
Contains `shell/goto/health` and `Health evidence is not current`.

Packaged `seat-update-warning` + 5s, dest-cut binary backed up as
`/usr/bin/mde-shell-egui.destcut-bc14a22d7`, new binary installed, Construct
restarted. New pid **2287059** holds `/dev/dri/card1` at 17:08 EDT.

## Non-claims

- Construct Health **Fix was not clicked**. This is not live Fix proof.
- Restarting Construct with no `power-honor.json` can drop the login curtain.
- This is not a signed dest-cut or RPM replace. NEVRA stays
  `magic-mesh-13.0.0-35`.
- `production_admitted` was not flipped. Sunshine was not started.
- `Restart mackesd` was not confirmed.
- Foreign dirty `mackesd` files were not reverted.

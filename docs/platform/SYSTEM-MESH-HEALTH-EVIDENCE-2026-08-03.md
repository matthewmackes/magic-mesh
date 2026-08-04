# System and Mesh Health five-seat evidence — 2026-08-03

This is the acceptance ledger for the System and Mesh Health authority described
in `docs/design/node-grade.md`. Evidence was collected directly from Seat 15,
Dell, Eagle, T480, and Surface; replicated copies were not counted as a seat
poll. All times and generations below came from the installed preview's typed
health snapshot.

## Exact preview identity

- RPM: `magic-mesh-12.1.6-1.x86_64`
- RPM SHA-256:
  `9e37adae2f14617b624ad5c06d271a342e03f83d88d138bd11a050f650d7cec2`
- RPM build time: `1785793282`
- `/usr/bin/mackesd`:
  `639c1383ba33486acbebea823026b5055004a10ca1f913a647b981e9026914be`
- `/usr/bin/mde-shell-egui`:
  `5f2f1a9822d5c9e8ce5b5d220da6e71c1fc82eae65e122be35563d975a2f03ce`
- `/usr/bin/mde-bus`:
  `21319226cbbbc8acc79efd1091d676ef2595f0ffa6e87d8315c1379d5971971c`

Every seat passed `rpm -Uvh --test`, then reported this exact installed package,
build time, and all three binary digests. `rpm -V magic-mesh` was empty on all
five seats. The Fedora 44 native release cut passed its real-payload and package
requirements gates before deployment.

## Reboot, resume, and two-cycle direct poll

The five seats were rebooted after installing the exact RPM, then independently
suspended to RAM and resumed with RTC wake. Boot identity remained stable across
resume, and each kernel journal contained an entry/exit pair (`2` suspend
markers). Two direct zero-condition polls were separated by more than two normal
publisher periods:

| Seat | Boot ID | Post-resume generation | Final generation | Root used | Audio capture | Inventory (disabled/degraded) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Seat 15 | `ea898f26-9011-4c05-9abc-c65049ad597b` | 33 | 59 | 46% | 384044 bytes | 0 / 0 |
| Dell | `991180f5-815c-40a9-a111-feb8c8f0a816` | 17 | 42 | 84% | 384044 bytes | 0 / 0 |
| Eagle | `0eb63ff9-eed0-420a-93cf-ac37c7e0e2ec` | 9 | 34 | 27% | 384044 bytes | 0 / 0 |
| Surface | `7e616b35-c725-4c73-93f0-921f876a722b` | 27 | 52 | 20% | 384044 bytes | 0 / 0 |
| T480 | `4c91b01d-0d90-4da2-a893-0b9822b0890b` | 34 | 59 | 31% | 384044 bytes | 0 / 0 |

At the final generation every observer rendered all five canonical publishers as
fresh and grade A, plus a mesh-wide grade A. Each summary reported zero active
warnings, zero active critical conditions, and zero unacknowledged actionable
conditions. Every observer saw at least three reachable lighthouses. Mackesd,
the shell, Nebula, and Syncthing were active; the typed mesh status also reported
Bus, DNS, KDC, and sync available. Voice was unassigned and did not affect health.

The audio proof on every seat matched the current boot, completed direct
PipeWire playback, and captured 384044 bytes through the `mm` user session. The
guided action audit IDs were:

- Seat 15: `health:Basement-Test-Workstation:01KZ4SKJ4VXMXR46VK2ZXKVFRE`
- Dell: `health:DELL-LAPTOP:01KZ4SJR0WVKN81G23HF0ZZFJ0`
- Eagle: `health:T470S-EAGLE:01KZ4SK6QNFV5KJJVEMH2HXCBD`
- Surface: `health:SURFACE:01KZ4SNSWEE4A14TGK3N89N5XW`
- T480: `health:T480:01KZ4SMXCJEH6FTNWK8SZEZYG5`

Seat 15's XFS root LV was grown online from 15 GiB to 30 GiB and settled at 46%
used while retaining VG reserve. Seat 15's firmware metadata refresh completed
successfully after the earlier failed refresh. The final inventories contained
40, 71, 62, 72, and 73 devices
respectively, with no disabled or degraded device on any seat; the former 17
phantom disabled PCI rows were absent. The reboot-created Bus contained zero
files in each retired raw `service`, `disk`, `smart`, `journal`, and `node-grade`
notification lane on all five seats.

## Rendered UI proof

The following screenshots are direct DRM readbacks from the exact installed
shell. Each live zero-state image shows the centered five-seat matrix, Overall A,
current freshness, zero active issues, and the shared zero-badge taskbar health
icon with no duplicate health page or legacy journal-warning toast:

- [Seat 15 zero state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-seat15-zero-9e37adae.png)
- [Dell zero state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-dell-zero-9e37adae.png)
- [Eagle zero state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-eagle-zero-9e37adae.png)
- [T480 zero state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-t480-zero-9e37adae.png)
- [Surface zero state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-surface-zero-9e37adae.png)

Critical auto-open was verified separately on every seat with a unique critical
condition in an isolated `/run` workgroup. No force-open environment flag was
present. The live daemon and replicated ledger were not stopped or mutated. The
modal opened, the affected row changed to F, the mesh grade changed to F, and the
badge count became one. The proof workgroup and shell drop-in were removed and
the original runtime configuration restored before the final direct poll.

- [Seat 15 critical auto-open](evidence/SYSTEM-MESH-HEALTH-2026-08-03-seat15-critical-auto-open-9e37adae.png)
- [Dell critical auto-open](evidence/SYSTEM-MESH-HEALTH-2026-08-03-dell-critical-auto-open-9e37adae.png)
- [Eagle critical auto-open](evidence/SYSTEM-MESH-HEALTH-2026-08-03-eagle-critical-auto-open-9e37adae.png)
- [T480 critical auto-open](evidence/SYSTEM-MESH-HEALTH-2026-08-03-t480-critical-auto-open-9e37adae.png)
- [Surface critical auto-open](evidence/SYSTEM-MESH-HEALTH-2026-08-03-surface-critical-auto-open-9e37adae.png)

The shell render test additionally writes dark and light desktop zero states,
many-condition guided confirmation, a narrow many-condition layout, and a stale
large-text layout. Its behavioral assertions cover fixed matrix ordering, zero
state, exact badge semantics, acknowledgement/snooze, Escape, focus containment,
queued critical opening and recurrence, and allowlisted device deep links.

- [Dark desktop zero state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-synthetic-health-dark-zero-desktop-9e37adae.png)
- [Light desktop zero state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-synthetic-health-light-zero-desktop-9e37adae.png)
- [Dark many-condition guided confirmation](evidence/SYSTEM-MESH-HEALTH-2026-08-03-synthetic-health-dark-many-guided-desktop-9e37adae.png)
- [Dark narrow many-condition state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-synthetic-health-dark-many-narrow-9e37adae.png)
- [Light stale large-text state](evidence/SYSTEM-MESH-HEALTH-2026-08-03-synthetic-health-light-stale-large-text-9e37adae.png)

## Farm verification

- `mackes-mesh-types`: 345 tests passed on `172.20.0.90`, slot
  `health-types-final-20260803`.
- `mackesd`: 10 node-grade tests and 12 notification tests passed on
  `172.20.0.50`, slot `health-daemon-final-20260803`.
- `mde-shell-egui`: 8 health-modal render/behavior tests, 20 status-bar tests,
  three updated Front Door tests, and the updated bottom-taskbar navigation test
  passed with the shipped `drm,live-vdi,media-mpv` features on XEN-BIGBOY.
- The exact Fedora 44 RPM release build, shipped-feature shell relink, package
  generation, payload gate, and requirements gate passed on XEN-BIGBOY.
- Worklist self-test/lint and documentation supersession lint passed on
  `172.20.0.90`, slot `health-docs-final-20260803`.

The aggregate workspace wrappers are not green in the shared dirty tree. The
umbrella gate stops at formatting in the concurrently edited
`mde-vdi-rdp/tests/live_rdp.rs`; workspace Clippy stops in concurrent
`mde-bookmarks` and `mde-maps-location-egui` changes. The full shipped-feature
shell run reached 1440 passing tests before 11 unrelated or stale assertions
failed (two tests were ignored). The health-modal tests were green in that run
and in the focused rerun above. These aggregate failures were not changed or
masked as part of this health cutover.

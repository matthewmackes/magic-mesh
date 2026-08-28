# WL-FUNC-023 — live Fix of Seat 15 and Dell (2026-08-28)

Operator: Fix 15 and Dell. `production_admitted` unchanged. No REL freeze.
No invented dest, token, mesh-id, or WAN IP. `Restart mackesd` was not
confirmed. Sunshine was not started. Surface was not mutated. Foreign dirty
`mackesd` files were not folded.

Control host `rocky9-kvm2` has no Nebula, so overlay SSH to `10.42.0.5` /
`10.42.0.4` times out. Underlay SSH with `/root/.ssh/mackes_mesh_ed25519`
as `mm` is the live path.

## Reachability (unchanged overlay identity)

| Seat | Underlay | Overlay-ip | Host cert | nebula | ping LH1–LH3 |
|---|---|---|---|---|---|
| Seat 15 `Basement-Test-Workstation` | `172.20.0.15` | `10.42.0.5` | present | active | ok |
| Dell `DELL-LAPTOP` | `172.20.146.225` | `10.42.0.4` | present | active | ok |

Installed NEVRA remains `magic-mesh-13.0.0-35`. Construct is active on both
seats. `power-honor.json` is absent on both.

## Mutation

Packaged `/usr/libexec/mackesd/seat-update-warning` ran as root on each seat
(`WARN_RC=0`; red `AI-GENERATED-ALERT`; five-second wait) at `2026-08-28T06:23:04-04:00` before mutation.

Seat 15 typed heals:

- `mcnf-xdg-bind-recovery.service` → `Result=success` `ExecMainStatus=0`;
  `/home/mm/Downloads` is a mount. Live Bus Health dropped
  `xdg-binds-down`.
- `mcnf-lifecycle-firstboot.service` → `Result=exit-code` `ExecMainStatus=1`.
  `mackesd onboard lifecycle-firstboot` reported
  `ready:false` `missing_requirements:["units","verification"]`,
  `first-boot marker: Pending; pending enrollment tokens: 66`.
  Marker `/var/lib/mackesd/lifecycle/pending-convergence` remains.
  Live Bus Health now lists `firstboot-pending`.
- `mesh-status.service` → `Result=success` `ExecMainStatus=0`.
- mm PipeWire / Pulse / WirePlumber enable-now → active.

Dell typed heals:

- XDG Downloads were already mounted; recovery was not re-run.
- `mcnf-lifecycle-firstboot.service` → `Result=success` `ExecMainStatus=0`;
  journal `first-boot marker: Converged`;
  `/var/lib/mackesd/lifecycle/firstboot-converged` present. Live Bus Health
  has no `firstboot-pending`.
- `mesh-status.service` → `Result=success` `ExecMainStatus=0`.
- mm PipeWire / Pulse / WirePlumber enable-now → active.

## Health after (live Bus node envelopes)

Both seats remain grade **F**. Mesh-status `/run/mde/mesh-status.json` lists
the three workstations and no lighthouse rows, so Health still reports
`lighthouse-unreachable` / `reachable_lighthouses: 0` while overlay ping to
`10.42.0.1`–`.3` succeeds. No lighthouse dest was invented.

Seat 15 remaining: `lighthouse-unreachable`, `firstboot-pending`,
`collab-identity-missing`, `cloud-arming-missing`, `mesh-storage-missing`
(critical), `workstation-audio`, `firmware-refresh`.
`mcnf-collaboration-identity.service` is **failed**
(`SecretStore identity does not match receipt`). `/mnt/mesh-storage` is not
a mountpoint.

Dell remaining: `lighthouse-unreachable`, `collab-identity-missing`,
`cloud-arming-missing`, `browser-vm-image-missing`, `mesh-storage-missing`
(critical), `workstation-audio`, `firmware-refresh`.
`mcnf-collaboration-identity.service` is **active**. `/mnt/mesh-storage` is
not a mountpoint.

Those leftovers stay dest-gated `open_onboarding` (or firmware/audio
provider nags). Dest was not invented.

## Non-claims

- Construct Health Fix was not clicked on the DRM seat.
- Cloud arming, Browser VM image, collab SHA, join token, mesh-id, and
  `/mnt/mesh-storage` dests were not invented.
- `production_admitted` was not flipped.
- `Restart mackesd` was not confirmed.
- Sunshine was not started.
- Lighthouses and Surface were not mutated.
- This does not close `WL-FUNC-023` or lift `WL-TEST-003`.

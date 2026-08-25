# WL-FUNC-023 — live Fix of Seat 15 and Dell overlay leftovers (2026-08-25)

Operator: fix Dell and Seat 15 now. `production_admitted` unchanged. No REL
freeze. No invented dest, token, or mesh-id. `Restart mackesd` was not
confirmed. Foreign dirty `mackesd` files were not folded.

## Overlay leftover (closed on these seats)

Earlier mm-user probes treated root-only `identity/current/host.crt` as
absent. Root inspection after the dest-cut enroll shows both seats already
hold live overlay identity:

| Seat | Underlay | Overlay-ip | Host cert | nebula | ping LH1 `10.42.0.1` |
|---|---|---|---|---|---|
| Seat 15 `Basement-Test-Workstation` | `172.20.0.15` | `10.42.0.5` | present | active | ok |
| Dell `DELL-LAPTOP` | `172.20.146.225` | `10.42.0.4` | present | active | ok |

## Mutation

Packaged `/usr/libexec/mackesd/seat-update-warning` (`AI-GENERATED-ALERT` +
5s) ran as root on each seat (`WARN_RC=0`) before mutation.

Seat 15 typed heals:

- `mcnf-xdg-bind-recovery.service` → `Result=success` `ExecMainStatus=0`;
  `/home/mm/Downloads` is a mount.
- `mcnf-lifecycle-firstboot.service` → `Result=success` `ExecMainStatus=0`;
  `pending-convergence` cleared; `firstboot-converged` present.
- `mesh-status.service` start; `runuser` enable-now of mm PipeWire units.

Dell typed heals: `mesh-status.service` start; mm PipeWire enable-now. XDG
Downloads were already mounted. No firstboot pending marker.

## Health after

Seat 15 dropped `firstboot-pending` and `xdg-binds-down`. Both seats still
grade F on:

- grouped-plane false `required-service-mackesd` / `dns` / `kdc` (do not
  confirm Restart mackesd; source skip is `bc14a22d7`, not dest-cut yet)
- `lighthouse-unreachable` while overlay ping to LH1 succeeds
  (`mesh-status.json` lists three workstations and no lighthouse rows)
- dest-gated `cloud-arming-missing` (and Dell `browser-vm-image-missing`)
- `workstation-audio` (mm PipeWire/Pulse/WirePlumber active with 1 sink)
- `firmware-refresh`

Collaboration admission `source_revision` remains `7e3474eeb…` vs installed
`4071ed295e18a8bd117cea5ee639eb5cafab3485`. Release-signer GNUPGHOME was
not present on this control host, so receipts were not rematerialized.

## Non-claims

- Construct Health Fix was not clicked on the DRM seat.
- Cloud arming, Browser VM image, and collab SHA dests were not invented.
- Lighthouses were not mutated.

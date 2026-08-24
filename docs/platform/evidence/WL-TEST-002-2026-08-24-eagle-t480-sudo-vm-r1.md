# WL-TEST-002 — Eagle + T480 sudo and VM path (2026-08-24)

Observed, then corrected-forward. Used the existing promotion sidecar
`/root/.mcnf-xapi-cred` with `sudo -S` exactly as
`WL-TEST-002-2026-08-23-unpublished-seat-install-eagle-t480-r2.md`. The
secret was not logged.

## Alert

`seat-update-warning.sh` ran as root on each seat (`AI-GENERATED-ALERT`,
`--no-broker`) and waited 5s before mutation.

## Sudo

Dell and Seat 15 already ship `/etc/sudoers.d/90-mm-nopasswd`
(`mm ALL=(ALL) NOPASSWD:ALL`). Eagle and T480 were wheel-only (password
required). Installed the same drop-in after `visudo -cf`. Fresh SSH
sessions now pass `sudo -n true`.

T480 LAN `BatchMode` pubkey was refused because
`mackes_mesh_ed25519.pub` (`SHA256:mDWs121t…`) was absent from
`authorized_keys`. Appended that farm key (Dell already had it). T480
now accepts `mm@172.20.146.68` with the mesh key.

RPM post-install now writes the same sudoers drop-in when `mm` exists,
so later seats do not repeat this gap.

## VM path (same as Dell / Seat 15 / Surface)

| Seat | Address | `sudo -n` | compute | observation | pool | `event/kvm/services` |
|---|---|---|---|---|---|---|
| Eagle | `172.20.146.88` | yes | active | active | `mde-vms` running | virtqemud; podman.socket enabled |
| T480 | `172.20.146.68` | yes | active | active | `mde-vms` running | virtqemud; podman.socket enabled |

`mm` is in `libvirt`. Identity `Requires=` on compute/observation was
replaced with the optional `Wants=` drop-in. Provider topic remains
`unknown` on installed `13.0.0-35` until the virtqemud/`mde-vms` fold is
packaged. Collaboration identity dest was not invented.

No reboot. `production_admitted` stays false.

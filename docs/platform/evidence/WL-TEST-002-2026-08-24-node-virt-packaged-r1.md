# WL-TEST-002 — package the Eagle/T480 sudo + virt path (2026-08-24)

The live seat mutations in
`WL-TEST-002-2026-08-24-eagle-t480-sudo-vm-r1.md` are now the RPM path.
No dest invented. No farm SSH pubkey baked into the package. No
`production_admitted` flip.

## Packaged path

| Piece | Dest |
|---|---|
| `install-helpers/install-mm-nopasswd.sh` | `/usr/libexec/mackesd/install-mm-nopasswd` |
| `install-helpers/prepare-node-virt.sh` | `/usr/libexec/mackesd/prepare-node-virt` |
| `packaging/systemd/mcnf-node-virt.service` | `/usr/lib/systemd/system/mcnf-node-virt.service` |

Workstation and server RPMs enable `mcnf-node-virt.service` (`WantedBy=mackesd.target`)
and run both helpers in post-install. The oneshot re-runs them before
`mackesd-compute` / `mackesd-observation`. Thin lighthouse ships only
`install-mm-nopasswd` and does not enable virt sockets.

`install-mm-nopasswd` writes `/etc/sudoers.d/90-mm-nopasswd` after `visudo -cf`
and does not overwrite an existing drop-in.
`prepare-node-virt` enables Fedora modular `virtqemud` / `virtnetworkd` /
`virtstoraged` / `podman` sockets when the unit files exist, grants `mm`/`mde`
the `libvirt` group, and starts a dir pool (`mde-vms`, `default`, or `images`).

Role provision enables those units on Workstation and masks them on
Lighthouse. Ansible `infra/ansible/node-virt.yml` stays the playbook twin.

Farm: `cargo test -p mackesd --lib role_provision` on `172.20.0.90` slot1
(`node-virt-packaged-role-provision-20260824-r2`) passed.

## Leftover

Seats still run unpublished `13.0.0-35`. Provider topic `unknown` and
Compute advertise stay stale until this package is installed. T480 farm-key
`authorized_keys` is host-specific and stays out of the RPM.

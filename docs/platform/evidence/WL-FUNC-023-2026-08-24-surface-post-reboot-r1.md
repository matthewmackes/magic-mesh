# WL-FUNC-023 leftover — Surface post-reboot recovery (2026-08-24)

Operator reported Surface rebooted. Seat was ~2 minutes up at probe. Red
`AI-GENERATED-ALERT` and five-second hold before mutation.
`production_admitted` unchanged. Collaboration identity dest from earlier
today survived the reboot.

## Survived reboot

`SURFACE` (`172.20.146.79`, overlay `10.42.0.7`) still has
`magic-mesh-13.0.0-35`, `/etc/mackesd/etcd-endpoints` (live quorum),
identity receipt + materializer outputs, Nebula, all six grouped `mackesd`
units, and `mde-shell-egui`. Overlay ping to LH1, Dell (`10.42.0.4`), and
Seat 15 (`10.42.0.5`) succeeds.

## Corrected-forward

`/var/lib/mackesd/nebula/overlay-ip` was 0 bytes after boot; rewritten to
`10.42.0.7`.

`mcnf-xdg-bind-recovery` refused `local data would be obscured:
/home/mm/Downloads` because of an empty root-owned
`.mde-vdi-clipboard-staging` directory (no user files). That directory was
removed; recovery then PASSed. Documents/Downloads/Music/Pictures/Videos
are communal bind mounts. `mcnf-peer-recovery` reached
`workstation-session-already-ready`.

## Honest leftover

`mcnf-lifecycle-firstboot.service` still fails: `ready:false`,
`missing_requirements: ["units","verification"]`,
`pending enrollment tokens: 66`, marker `pending-convergence`. The
installed first-boot catalog still requires `mackesd.service` (monolithic)
while this seat runs grouped `mackesd-*.service`. Tokens were not invented
or drained. Do not start a second monolithic `mackesd` beside the groups.

# WL-FUNC-023 / WL-TEST-002 dest-cut `4071ed295` upgrade + Dell wipe — r1

Date: 2026-08-24  
Classification: unpublished dest-cut install; **not** freeze, publication,
or `production_admitted`  
`published: false`  
`production_admitted: false`

Operator 2026-08-24: upgrade dest-cut of HEAD `4071ed295` and
fresh-install wipe + token.

## Cut and dest

Fedora 44 container lanes (`--full` on `.90`, `--server` on `.130`) from a
clean worktree of `4071ed295e18a8bd117cea5ee639eb5cafab3485` /
epoch `1787600663`. Workstation `mackesd` identity string is that SHA.
Signed with governed fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C`
(key id `497ddc7c`). Ephemeral keyring destroyed. The 2026-08-22 sidecar
was not replaced. New dest:
`/root/mcnf-private/unpublished-signed-candidate-4071ed295.json` (0400).

NEVRA stayed `magic-mesh-13.0.0-35` (same VR as the installed
`7e3474eeb` cut). Seat upgrade is `rpm -Uvh --replacepkgs --force`.

## Alert

`seat-update-warning.sh` ran on each mutated seat (`AI-GENERATED-ALERT` +
5s). Broker persisted `--no-broker`. Control-host `mde-bus` is absent.

## Upgrade (identity kept)

| Seat | After | Notes |
|---|---|---|
| Seat 15 | `4071ed295` · nebula + `mackesd-control` active · `browser-vm` running | `overlay-ip` empty — heal nag path |
| Surface | `4071ed295` · nebula + `mackesd-control` active | `overlay-ip` and `etcd-endpoints` present |

`%post` logged transient systemd socket resets; package identity still
replaced. Monolithic `mackesd.service` stays inactive (grouped plane).

## Dell wipe + token

Dest-signed `leave --yes` (`FORCE OFFBOARD 1 SYSTEMS`) on `DELL-LAPTOP`.
`remove-peer` was not used. Then the same dest-cut RPM replace. A new
bearer dest was minted on a lighthouse (`enroll-token`); token never
printed.

`enroll --token-stdin` with `?fp=` routed to `join` (TLS pin). Overlay
`10.42.0.1:4243` timed out after leave (no overlay). Underlay enroll
wrote `/etc/nebula/ca.crt`. Host cert was not materialized. `nebula` and
`mackesd-integrations` failed (integrations start-limit; collaboration
identity drop-in still present). `etcd-endpoints` file remains;
`overlay-ip` absent.

## Non-claims

- Construct Health Fix was not clicked on the live seat in this record.
- Lighthouses and MG90 were not mutated.
- Official REL freeze / publish was not run.

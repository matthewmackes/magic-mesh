# WL-FUNC-017 — restore Seat 15 MG90 credentials (2026-08-24)

Operator authorized destructive seat mutation. Red `AI-GENERATED-ALERT` via
`/usr/local/bin/seat-update-warning.sh` on Seat 15 (`--no-broker` persist),
then five-second hold. No dest invented. No WAN IP, password, or HTTP secret
recorded. `production_admitted` unchanged. MG90 was **not** rebooted.

## Target

| Field | Value |
|---|---|
| Seat | `172.20.0.15` `Basement-Test-Workstation` |
| Installed RPM | `magic-mesh-13.0.0-35` (`7e3474eeb`) |
| Gateway | `172.20.0.25:2222` ESN `ND84720078011035` MGOS `4.3.0.1` |

## Mutation

Copied root-only files from the control host (`0600`, owner root) plus the
pinned `mg90_known_hosts` (`0644`):

- `/etc/mackesd/mg90-root-password` (17 bytes)
- `/etc/mackesd/mg90-http-password` (6 bytes)
- `/etc/mackesd/mg90_known_hosts` (100 bytes)

## Result

Seat 15 `mg90-access ssh-probe` returns ESN `ND84720078011035`.

Live inspect from the control host: hostname matches ESN, GNSS `enable: yes`,
MCU `IGNTHRESH 1.5` / `LOWVOLT 11` / `HIGHVOLT 36` / `OFFDELAY 5`, `wlan1`
SSID `SERRRA-TEST` type `AP`. `omgconf list` returns the committed YAML set
including `mcu.yaml`, `gps.yaml`, `wan.yaml`, `wifi-networks.yaml`.

A same-value `IGNTHRESH: 1.5` `sed` + `omgconf commit` ran on the live
gateway. oMG reported `nothing to commit, working directory clean` (no
policy change).

`mackesd-integrations.service` did **not** start. It `Requires=`
`mcnf-collaboration-identity.service`, which fails closed:
`/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json`
is absent. That receipt is not invented here. Vehicle Bus verbs on this
installed `13.0.0-35` binary therefore stay down until collaboration identity
is dest-backed or the unit dependency is corrected-forward.

## Leftover

Seat 15 still cannot publish `state/vehicle/*` or drain `action/vehicle/*`
until `mackesd-integrations` can start. The new inspect/set-mcu/set-gps
verbs also need a package newer than `13.0.0-35`.

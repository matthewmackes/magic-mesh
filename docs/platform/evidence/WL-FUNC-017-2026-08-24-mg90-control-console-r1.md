# WL-FUNC-017 — MG90 Admin is a live control console (2026-08-24)

The Construct MG90 Admin surface is no longer a seven-chip mock. It is a
sidebar console over the live AirLink MG90 (`172.20.0.25`) and typed
`mackesd` `action/vehicle/*` verbs. No dest invented. No WAN IP, ESN-as-
password, or credential contents recorded here.

## Control plane

| Verb | Kind | Live behavior |
|---|---|---|
| `inspect` | READ | Bounded JSON from the gateway (hostname, uptime, country, GNSS enable, MCU volts, `wlan1` SSID/type) |
| `list-config` | READ | `omgconf list` |
| `get-config` | READ | `cat /opt/inmotiontechnology/config/<bare.yaml>` (`omgconf latest` prints nothing on MGOS 4.3.0.1) |
| `set-mcu` | MUTATION | Allowlisted MCU key; `sed` + `omgconf commit mcu.yaml`; typed-arm ESN; HMAC `vehicle-set-mcu` |
| `set-gps` | MUTATION | `yes`/`no`; `sed` + `omgconf commit gps.yaml`; HMAC `vehicle-set-gps` |
| `reboot` | MUTATION | Existing armed reboot; HMAC `vehicle-reboot` |

Reads publish from `mde-maps-location-egui`. Privileged mutations are queued
for the Construct shell to mint (`authorize_root_mutation_body`, target
`gateway`). The UI does not SSH.

## Console panes

Overview, WAN, Wi-Fi, LAN, GNSS, Vehicle I/O, VPN, Services, Access, Config,
Power. Keys 1–9 select the first nine. Config and Power are mouse-only.

## Leftover

Seat files `/etc/mackesd/mg90-root-password` and `mg90-http-password` are
still missing on some seats after RPM reinstall. Restoring them is a seat
mutation (red alert + 5s) and is not done by this change. This change does
not reboot the live MG90 and does not install a new RPM.

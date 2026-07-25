# MG90 host-to-host access contract

`install-helpers/mg90-access.sh` is the canonical adapter for the bench MG90 at
`172.20.0.25`. It exposes the communication planes that are currently available
without hiding transport failures:

Protocol reference: [AirLink MG90 Software Configuration Guide, Rev 6](https://www.communica.se/sierrawireless/4118700%20AirLink%20MG90%20Software%20Configuration%20Guide_r6.pdf).

| Plane | Endpoint | Adapter command | Use |
| --- | --- | --- | --- |
| Root SSH | `172.20.0.25:2222` | `ssh-probe`, `ssh-exec` | OS identity, raw NMEA, committed config, explicitly armed control |
| MG-LCI | `http://172.20.0.25/` | `lci-get` | General board/ignition and WAN status |
| MG90 apps | `http://172.20.0.25:11532/` | `app-get` | GPS, OBD-II, heavy telemetry, GPIO, and Acetech status pages |
| Status broadcast | MG90-configured UDP port | `status-listen PORT` | Documented JSON beacon: GNSS, WAN, VPN, GPIO, ignition, battery, temperature |
| GPS forwarding | TCP `9345` / UDP `5067` by default | `gps-tcp-connect`, `gps-udp-listen` | NMEA/TAIP streams; MG90 can also forward to configured remote servers |
| Reachability | ports 80/11532/2222 | `inventory` | Read-only transport inventory |

The adapter is deliberately pinned to the MG90 SSH host key and uses the MG90's
legacy SSH algorithms only where required. It does not enable `ssh-dss`, disable
host-key verification, or put a password in a command argument.

## Provisioning

Run these as root on the node that will call the adapter. Supply the current
credentials out of band; do not commit them, paste them into a worklist, or put
them in a process argument.

```sh
sudo install -o root -g root -m 0600 /dev/stdin /etc/mackesd/mg90-root-password
sudo install -o root -g root -m 0600 /dev/stdin /etc/mackesd/mg90-http-password
```

The first command must receive the MG90 root SSH password and the second the
authenticated HTTP password. The historical ESN-as-password convention is not a
credential contract and must not be guessed. The SSH pin can be installed for a
system-wide invocation with:

```sh
sudo install -o root -g root -m 0644 install-helpers/mg90-known-hosts \
  /etc/mackesd/mg90_known_hosts
```

Rotate that pin deliberately if the gateway is replaced; a host-key mismatch is
an identity failure, not a reason to use `StrictHostKeyChecking=no`.

## Commands

```sh
sudo ./install-helpers/mg90-access.sh --self-test
sudo ./install-helpers/mg90-access.sh inventory
sudo ./install-helpers/mg90-access.sh ssh-probe
sudo ./install-helpers/mg90-access.sh ssh-exec -- cat /var/run/omgtime.g.info
sudo ./install-helpers/mg90-access.sh lci-get /MG-LCI/status/general.html
sudo ./install-helpers/mg90-access.sh lci-get '/MG-LCI/wan/status/status.html?displayExtended=true'
sudo ./install-helpers/mg90-access.sh app-get /omggpsd/
sudo ./install-helpers/mg90-access.sh app-get /obdii_status/
sudo ./install-helpers/mg90-access.sh app-get /hdobd_status/
sudo ./install-helpers/mg90-access.sh app-get /indiod_status/
sudo ./install-helpers/mg90-access.sh app-get /acetech_status/
sudo ./install-helpers/mg90-access.sh status-listen <configured-status-port>
sudo ./install-helpers/mg90-access.sh gps-udp-listen
sudo ./install-helpers/mg90-access.sh gps-tcp-connect
```

`app-get` is a read surface. The returned pages advertise the device-specific
JSON calls (`odoStatus`, `adapterStatus`, `currentStatus`, `historyStatus`, and
`updateStatus`); callers must parse and validate those responses before adding
them to the mesh mirror. Configuration or calibration calls remain separate,
explicit operations and are not invoked by `inventory` or `ssh-probe`.

The guide documents Status Broadcast as the preferred streaming telemetry plane:
the JSON beacon can include location, GPIO input/output states, each WAN link,
GNSS fix/satellite/antenna state, VPN state, ignition, battery, and temperature.
The guide also documents GPS forwarding with NMEA and TAIP sentence selection,
fixed or threshold-driven intervals, local TCP/UDP, serial forwarding, and remote
server lists. The adapter's listeners only receive data; enabling or changing
these streams is an explicit LCI configuration action and is not performed here.

For the daemon-side vehicle mirror, set `MDE_VEHICLE_STATUS_PORT` to the same
local UDP port selected on the MG90's `Status > Broadcast` page. The worker binds
that local receiver at startup and uses the beacon as a typed GNSS/power fallback
plane. An unset variable means the beacon plane is disabled. A non-numeric,
zero, out-of-range, or already-bound local port is retained as a configuration
error and is carried in the `gaps` field of the `state/vehicle/<node>` snapshot;
it is no longer silently treated as an absent beacon. This is receiver diagnostics
only—the worker does not alter the MG90 configuration. `status-listen PORT` performs
the equivalent bounded
receive-only check for an operator shell session and prints raw datagrams.

The Rev. 6 guide does not define a Wi-Fi, cellular, or Bluetooth scanner-contact
protocol or endpoint. The Airspace worker therefore remains `NO SCANNER FEED`
until a real scanner adapter is supplied; this access helper and the vehicle
Status Broadcast path must not be used to manufacture contacts.

## Failure interpretation

- Port `2222` reachable plus `Permission denied` means the current root credential
  is wrong or root SSH is classifier-gated; it is not a network failure.
- A host-key verification failure means the pin is absent or the gateway identity
  changed. Stop and rotate the pin intentionally.
- LCI success does not imply root access. LCI and the application server have
  independent sessions and permissions.
- The current `mackesd` vehicle worker consumes LCI general/WAN and raw SSH NMEA,
  and uses the documented UDP JSON beacon when `MDE_VEHICLE_STATUS_PORT` is
  configured and the local receiver is ready. The beacon is primary for the
  documented GNSS/power fields, with LCI/NMEA as authenticated or local
  fallbacks. Until OBD and application parsers are wired into `VehicleState`,
  those fields remain honest gaps rather than fabricated telemetry. Receiver
  configuration failures are also honest gaps, not a reason to claim beacon
  telemetry.

The guide also describes AMM management/deployment and a dedicated management
tunnel. Those are separate Sierra Wireless control-plane products/configurations;
they are not inferred from local LCI reachability and remain an external
integration surface for this mesh.

The host-to-host mesh/BUS path remains separate: `state/vehicle/<node>` is the
latest-wins mirror and `action/vehicle/*` is the typed command plane. Use the MG90
adapter for device transport, then publish validated results through those Bus
contracts.

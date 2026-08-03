# MG90 live control handoff — 2026-08-02

This is the durable handoff for the real MG90 currently attached to the
platform. It is an attached vehicle gateway, not a mesh node or lighthouse.

## Verified live state

The live proof seat is `.15` (`172.20.0.15`, Basement-Test-Workstation). Its
running `mackesd` publishes a v2 mirror for:

| Field | Sanitized observation |
| --- | --- |
| MG90 identity | ESN `ND84720078011035` |
| Firmware | `4.3.0.1` |
| Management node | `Basement-Test-Workstation` |
| Source | direct gateway |
| Online | true |
| Cadence | observed 2-second and 5-second publication paths; latest records are fresh |
| Cellular | A installed/active; B installed/standby |
| Wi-Fi | A installed/standby; B not reported |
| Bluetooth | not reported |
| GNSS | installed, acquiring, no fix; no antenna fault inferred |
| Power/vehicle | battery and ignition reported; movement is stopped |
| OBD | explicitly `not_installed`; no synthetic RPM/speed/fuel values |

The live state is visible below the workstation Bus path:

```text
/run/mde-bus/state/vehicle/Basement-Test-Workstation/ND84720078011035/
```

Do not copy raw snapshots into Git. They contain location, carrier, WAN, and
device telemetry. Use sanitized fixtures or a redacted evidence bundle.

## Control and data planes

1. **Direct SSH management plane** — MG90 `:2222`, pinned by
   `install-helpers/mg90-known-hosts`. Used for identity, NMEA/IMU, bounded
   read-only inspection, committed configuration reads, and explicitly armed
   typed actions such as reboot. The password is supplied through the root-only
   workstation file `/etc/mackesd/mg90-root-password`; never put it in Git,
   argv, logs, or a worklist.
2. **MG-LCI HTTP plane** — MG90 `:80`, authenticated form session under
   `/MG-LCI/`. Used for general board/ignition state and extended WAN status.
   The session is separate from SSH and must use the operator-provisioned HTTP
   secret; do not infer it from the ESN or SSH credential.
3. **MG90 application API plane** — MG90 `:11532`, CherryPy session login at
   `do_login`. The allowlisted application surfaces are `/omggpsd/`,
   `/obdii_status/`, `/hdobd_status/`, `/indiod_status/`, and
   `/acetech_status/`. Their pages advertise device JSON methods including
   `odoStatus`, `adapterStatus`, `currentStatus`, `historyStatus`, and
   `updateStatus`. Reads are bounded and parsed only after the payload schema
   is proven. Configuration/calibration calls are separate explicit actions.
4. **MG90 Status Broadcast plane** — configured UDP beacon received by the
   workstation on `MDE_VEHICLE_STATUS_PORT`. This is receive-only from the
   platform and may carry GNSS, WAN, GPIO, ignition, battery, temperature, and
   VPN fields. The platform does not silently enable or reconfigure it.
5. **GPS forwarding plane** — NMEA/TAIP over TCP `9345` or UDP `5067`,
   receive-only in the adapter. It is a fallback/data plane, not proof of a
   valid GNSS fix.
6. **Workstation Bus mirror plane** — v2 state is published at
   `state/vehicle/<management-node>/<mg90-id>`. It carries identity,
   provenance, freshness, radios, GNSS, WAN, power, vehicle gaps, and explicit
   unavailable states. Credentials and raw API responses never enter it.
7. **Mesh sharing plane** — approved workstation managers publish the freshest
   complete snapshot; remote nodes render it read-only with source, manager,
   age, stale, cache, and resync state. Lighthouses relay only and do not own
   MG90 state.
8. **Typed action/reply plane** — allowlisted `action/vehicle/*` requests are
   authorized, audited, idempotent, and answered on the typed reply topic.
   Reboot is destructive and requires the existing exact typed arm and action
   authorization. Queued/resync/revocation behavior follows the MG90 contract
   in `docs/platform/WORKLIST.md`.
9. **Construct/Car operator plane** — Maps, Car, Airspace, This Node, and the
   gateway console consume the same v2 snapshot. They must show stale,
   unavailable, no-fix, not-reported, unsupported, and not-installed states;
   they must not invent local hardware or expose credentials.

## Reconciled access state — 2026-08-02

ClaudeCode's project memory supplied the historical operator-approved MG90
credential convention. The `.15` host was reconciled as follows:

- `/etc/mackesd/mg90-root-password` was restored as a root-owned `0600` file;
  the pinned SSH probe now succeeds and returns the MG90 identity plane.
- `/etc/mackesd/mg90-http-password` was installed as a root-owned `0600` file.
- MG-LCI general and extended-WAN reads return HTTP 200.
- The CherryPy application plane requires `do_login`, application form fields,
  and a same-origin `Referer`; with that contract, `/omggpsd/`,
  `/obdii_status/`, `/hdobd_status/`, `/indiod_status/`, and
  `/acetech_status/` all return authenticated content.
- `.15` now runs `mackesd` with `MDE_VEHICLE_OBD_STATUS_PATH=/obdii_status/`.
  The live mirror receives the OBD response and reports
  `unsupported: OBD/HDOBD response schema is not verified`, which is the
  correct fail-closed result until a sanitized schema fixture is approved.
- The reconciled `mg90-access.sh` passed its self-test and live `ssh-probe`; its
  authenticated `/obdii_status/` read returned the expected `odoStatus` and
  `adapterStatus` markers.
- Farm `.90` ran the focused `mackesd` vehicle suite: **80 passed, 0 failed**.

No credential value is recorded in this document. If the MG90 is replaced,
rotate both root/API files and the pinned host key deliberately; do not guess
passwords or disable host-key verification.

## Next AI actions

1. Add a sanitized, versioned OBD/HDOBD fixture and schema verifier before
   promoting any diagnostic value into typed telemetry.
2. Determine whether the MG90 Status Broadcast is configured and bind the
   workstation receiver without changing MG90 configuration implicitly.
3. Capture a redacted 30-minute cadence run and sanitized API fixtures. Prove
   OBD/HDOBD payload schema before promoting any vehicle values.
4. Keep `WL-FUNC-017` at `Remaining` until the live cadence, radio inventory,
   API evidence, navigation, and direct-DRM acceptance criteria pass.

Credentials are intentionally not included in this handoff.

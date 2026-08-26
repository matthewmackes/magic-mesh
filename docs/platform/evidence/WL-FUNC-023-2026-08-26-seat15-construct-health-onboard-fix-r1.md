# WL-FUNC-023 leftover — Seat 15 Construct Health / ONBOARD Fix (2026-08-26)

Date: 2026-08-26
Observed: `2026-08-26T12:05:11Z` (`08:05:11-0400`) on Seat 15 after dest-cut
`bc14a22d7`. Click attempts `11:59:03Z`–`12:04:48Z`.
Classification: leftover-honesty / live-seat Health snapshot + Fix click
attempt; **not** ONBOARD dest mint, **not** freeze, **not**
`production_admitted`
Dest-cut identity: `bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c` / epoch
`1787672034` / `magic-mesh-13.0.0-35`
Seat: Seat 15 `172.20.0.15` (`Basement-Test-Workstation`)
`published: false`
`production_admitted: false`

Control host `mm@172.20.0.15` with `/root/.ssh/mackes_mesh_ed25519` and
`sudo -n`. Packaged `/usr/libexec/mackesd/seat-update-warning` ran before
each mutation (`WARN_RC=0`; red `AI-GENERATED-ALERT`; five-second wait).
Toast broker `http://10.42.0.5:8443/event_toast_show` was unreachable;
the helper persisted the toast ULID for retry. Sunshine was left stopped.
`Restart mackesd` was not confirmed. Construct was not restarted (no
`power-honor.json`; a restart would drop the login curtain). Foreign dirty
`mackesd` / shell files were not folded. Unpublished signed candidate JSON
was not overwritten.

## Installed identity (unchanged by this probe)

| Field | Seat 15 |
|---|---|
| RPM | `magic-mesh-13.0.0-35` `buildtime=1787672034` |
| `mackesd --version` | `13.0.0 "Construct" · bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c · 2026-08-25 · dev` |
| Overlay-ip | `10.42.0.5` (root-only `/var/lib/mackesd/nebula/overlay-ip`) |
| Host cert | present `/etc/nebula/identity/current/host.crt`; `/etc/nebula/host.crt` → that path |
| `nebula1` | `10.42.0.5/17` · `nebula.service` active |
| Overlay ping LH1 `10.42.0.1` | ok |
| Construct | pid `2790` root, active since 2026-08-26 07:12:22, holds `/dev/dri/card1` @ 1920×1080 HDMI-A-1 |
| Grouped plane | `mackesd.target` active; `mackesd-compute` / `mackesd-observation` active; monolithic `mackesd.service` inactive |
| `mackesd-control` | inactive (dead); drop-in `Requires=mcnf-collaboration-identity.service` |

## Health grade / facts

Latest node envelope
`/run/mde-bus/state/health/node/Basement-Test-Workstation/01M0YZB8CYDMCCB2N9XPM207E2.json`
(body is a JSON string). Roster snapshot
`state/health/system-mesh/01M0YZB8CYDMCCB2N9XPM207E3.json`.

| | At `11:54Z` (first read) | At `12:05Z` (final read) |
|---|---|---|
| Node grade | **F** · capability 92 | **F** · capability 95 |
| Mesh factor | 80 | 100 |
| Mesh summary | grade F · 3/3 fresh · **0** reachable lighthouses · 19 warn / 3 crit | grade F · 2/3 fresh · **3** reachable lighthouses · 12 warn / 2 crit |
| `restart_mackesd` offered | 0 | 0 |
| `overlay-identity-missing` | 0 | 0 |
| `open_onboarding` offered (mesh-wide) | yes (Seat 15 + Dell + Surface dest-gates) | 7 rows |

Seat 15 active conditions at `12:05Z` (six; `lighthouse-unreachable` had
moved to resolved):

| id | severity | Fix | confirm |
|---|---|---|---|
| `firstboot-pending` | warning | `run_lifecycle_firstboot` | yes |
| `xdg-binds-down` | warning | `recover_xdg_binds` | yes |
| `collab-identity-missing` | warning | **`open_onboarding`** | yes |
| `cloud-arming-missing` | warning | **`open_onboarding`** | yes |
| `mesh-storage-missing` | **critical** | **`open_onboarding`** | yes |
| `workstation-audio` | warning | `restore_workstation_audio` | yes |

Dest-cut `bc14a22d7` nags are live on this installed RPM: grouped plane does
**not** offer `Restart mackesd` / `restart_dns` / `restart_kdc`; overlay-ip
and host cert are present so Health does **not** offer `Publish overlay IP`
or `overlay-identity-missing`. The typed ONBOARD Fix is what dest-gated
leftovers get.

## Why ONBOARD is still the Fix (dest-gated; not invented)

- Collaboration receipt **and** admission JSON exist. `source_revision` on
  both is `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac` (prior cut), not
  installed `bc14a22d79…`. `mcnf-collaboration-identity.service` is
  **failed** (`Result=exit-code`, `ExecMainStatus=2`) with
  `REFUSED[WL-FUNC-011/collaboration-identity-materializer]: SecretStore
  identity does not match receipt`. That blocks `mackesd-control` via the
  `Requires=` drop-in. A new receipt was not invented.
- `/mnt/mesh-storage` is a directory and **not** a mountpoint.
  `mountpoint -q` fails; Health nags `mesh-storage-missing` → Open
  Onboarding.
- Cloud arming credential was not present on the probed paths; Health nags
  `cloud-arming-missing` → Open Onboarding. No systemd credential was
  invented.
- `pending-convergence` (7 bytes, stamped 07:12) is back after the dest-cut
  reboot; `/home/mm/Downloads` is not a mount.

First-read `lighthouse-unreachable` (`refresh_provider`, no confirm) is
resolved at the final snapshot while overlay ping to LH1 was already ok at
first read. Mesh-wide `reachable_lighthouses` went 0 → 3. No durable
`action/health/remediate` or
`/var/lib/mackesd/node-grade-action-results` row was found, so this is
**not** claimed as a Construct Refresh-provider click.

Dell’s node row was not fresh at `12:05Z` (`fresh_nodes` 2/3). This unit
did not SSH Dell.

## Construct Health / ONBOARD Fix click

`/usr/bin/magic-setup` is present (`944640` bytes, dest-cut mtime 2026-08-25
11:33). Construct Open Onboarding launches that binary locally after a
CONFIRM-gated Fix (`health_modal.rs`); it does not publish
`action/health/remediate`.

Input Construct already held: Keychron `/dev/input/event4`, PixArt MS116
`/dev/input/event3`.

Attempts after the packaged five-second warning (Sunshine not started):

1. Relative writes to existing MS116 `event3` (park top-left, health target
   `(1015,12)`, then estimated modal Fix / Confirm). Construct logged
   `libinput error: event3 … high-resolution scroll` at 07:59:08 — the
   pointer device was live. No `magic-setup` process.
2. Persistent uinput touchscreen `mackesd-seat-remote-input` kept open long
   enough for udev. Construct **did** add `/dev/input/event13` to pid 2790
   (`PROP=2` DIRECT). Absolute taps at the computed Health icon and Open
   Onboarding / Confirm coordinates. Device destroyed after taps. No
   `magic-setup`.
3. ABS + `BTN_LEFT` pointer variant (Construct again opened `event13`;
   kernel also attached `js0` on that variant). No `magic-setup`.
4. Repeat touchscreen with `UI_ABS_SETUP` ranges. Construct opened
   `event13` again. No `magic-setup`.

No `state/health/remediation/` files. Node-grade action-results dir stayed
empty. Overlay-ip remained `10.42.0.5`. Construct pid stayed `2790`.

There is no DRM frame (Construct owns `card1`; `kmsgrab` cannot steal it;
grim/Moonlight were not started). Button hit targets inside the centered
Health modal were therefore not visually confirmed. Open Onboarding was
**not** observed as a launched `magic-setup`.

## Non-claims

- Construct Health Fix / Open Onboarding was not proven clicked.
- Cloud arming, Browser VM image, collab SHA, join token, mesh-id, and WAN
  IP dests were not invented.
- `production_admitted` was not flipped.
- `Restart mackesd` was not confirmed; `mackesd-control` was not started.
- Sunshine was not started.
- Lighthouses were not mutated. Dell and Surface were not SSH’d.
- This does not close `WL-FUNC-023`. Leftover remains `@leftover:{live-seat}`
  (Construct ONBOARD Fix on a dest-cut DRM seat) plus dest-operator collab /
  arming / mesh-storage mounts.

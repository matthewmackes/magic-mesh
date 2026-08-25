# WL-FUNC-023 / WL-TEST-002 dest-cut `bc14a22d7` onto seats and LH1–LH3 — r1

Date: 2026-08-25
Classification: unpublished dest-cut install; **not** freeze, publication,
or `production_admitted`
`published: false`
`production_admitted: false`

Operator dest-cut of heal SHA `bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c`
/ epoch `1787672034` onto Seat 15, Dell, Surface, and the three
lighthouses. Mesh-id unchanged (`mcnf-clean-20260728`). The 2026-08-22
sidecar was not replaced.

## Cut and dest

Fedora 44 container lanes from a clean worktree of `bc14a22d7` (dirty
main checkout was not used): `--full 44` on `.130` slot 2 (workstation +
lighthouse) and `--server 44` on `.90` slot 1 (server only; server-lane
lighthouse was not bound).

Signed with governed fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C`
(key id `497ddc7c`). Ephemeral keyring destroyed. New dest:
`/root/mcnf-private/unpublished-signed-candidate-bc14a22d7.json` (0400).

| Role | NEVRA | notes |
|---|---|---|
| Workstation | `magic-mesh-13.0.0-35.x86_64` | same VR as `4071ed295`; install was `rpm -Uvh --replacepkgs --force` |
| Lighthouse | `magic-mesh-lighthouse-13.0.0-11.x86_64` | 12.1.6 → 13.0.0 jump |
| Server | `magic-mesh-server-13.0.0-35.x86_64` | cut and signed; not installed in this dest-cut |

## Alert

Workstations used packaged `/usr/libexec/mackesd/seat-update-warning`.
Lighthouses have no `mde-bus`; each printed `AI-GENERATED-ALERT` and
waited five seconds before mutation. `Restart mackesd` was not
confirmed.

## Workstations (identity kept)

`%post` logged transient systemd socket resets; package identity still
replaced. Monolithic `mackesd.service` stays inactive (grouped plane).

| Seat | Overlay | Host cert | nebula | After `mackesd --version` |
|---|---|---|---|---|
| Seat 15 `172.20.0.15` | `10.42.0.5` | present | active | `13.0.0 "Construct" · bc14a22d79…` · `mackesd-control` active |
| Dell `172.20.146.225` | `10.42.0.4` | present | active | same SHA · `mackesd-control` active |
| Surface `172.20.146.79` | `10.42.0.7` | present | active | same SHA · `mackesd-control` active |

Installed NEVRA on all three: `magic-mesh-13.0.0-35` `buildtime=1787672034`.

## Lighthouses (one at a time; quorum preserved)

Order: LH3 (`10.42.0.3` / `64.23.131.57`), LH2 (`10.42.0.2` /
`46.101.219.245`), then founding LH1 (`10.42.0.1` / `104.236.118.177`).
Governed pubkey `packaging/repo/RPM-GPG-KEY-magic-mesh` was imported
before `rpm --checksig` / `-Uvh` (LHs had no prior key). `alsa-lib`
(`1.2.16.1-1.fc43`) was installed to satisfy a lighthouse RPM Requires
on `libasound.so.2`; `--nodeps` was not used.

Seat 15 overlay ping to `.1` `.2` `.3` stayed ok after each LH.

| LH | Before | After | Overlay | nebula | Host cert |
|---|---|---|---|---|---|
| LH3 | `magic-mesh-lighthouse-12.1.6-5` | `13.0.0-11` · `bc14a22d79…` | `10.42.0.3` | active | present |
| LH2 | `12.1.6-5` | same | `10.42.0.2` | active | present |
| LH1 | `12.1.6-11` | same | `10.42.0.1` | active | present |

Grouped plane after the jump: `mackesd.target` active;
`mackesd-compute` and `mackesd-observation` running; `mackesd.service`
absent/inactive. `mackesd-control` is inactive: drop-in Requires
`mcnf-collaboration-identity.service`, which fails closed with no
receipt at
`/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json`.
LH1 `etcd.service` stayed active and listening on `10.42.0.1:2379`.
Cert-less `etcdctl` handshake EOF is not a cluster-health proof.

## Non-claims

- Official REL freeze / publish was not run.
- `production_admitted` was not flipped.
- Cloud arming, Browser VM image, and collab SHA dests were not invented.
- Eagle and T480 were not mutated (non-gating).
- Server RPM was not installed.
- Construct Health Fix was not clicked on a DRM seat.
- `Restart mackesd` was not confirmed.

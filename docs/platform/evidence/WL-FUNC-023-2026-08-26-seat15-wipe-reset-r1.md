# WL-FUNC-023 — Seat 15 full wipe + reset — r1

Date: 2026-08-26  
Classification: live leftover (3) offboard + recommission; **not** freeze,
publication, or `production_admitted`  
Control host: `rocky9-kvm2`  
Seat: `Basement-Test-Workstation` `172.20.0.15`  
Mesh-id (existing, not invented): `mcnf-clean-20260728`  
`production_admitted: false`  
`published: false`

## Authority

Operator 2026-08-26: fully wipe and reset 15. Red `AI-GENERATED-ALERT` + 5s
via packaged `/usr/libexec/mackesd/seat-update-warning` before leave.

## Dests used (hashes / modes only)

| Dest | Mode | Note |
|---|---|---|
| `/root/mcnf-private/lifecycle-confirmation-ed25519` | `0600` | existing seed; never printed |
| `/root/mcnf-private/enroll-endpoint-fp-der` | `0400` | join TLS pin |
| `/root/mcnf-private/enroll-bearer-seat15-wipe-20260826` | `0600` | minted on LH1; never printed |
| sidecar `enroll-bearer-seat15-wipe-20260826.json` | `0400` | `bearer_sha256=b5aacfe53a6e59d05c8cf7ed2883f50d9440116e1f04406508d533aae36b34d5` |

Confirmation verifying-key sha256 stayed
`bc37d10b7d79a825ce4859384692f31bf573c7811d19a599b1b07606b0aeb4a2`.
`remove-peer` was not used. Unpublished signed-candidate dests were not
replaced.

## Leave

Dest-signed `mackesd leave --yes` (`FORCE OFFBOARD 1 SYSTEMS`) on Seat 15
after the packaged five-second warning. After leave: no host cert, no CA,
`/etc/nebula` empty. Extra local wipe then removed
`/home/mm/browser-vm-review`, `/root/ux006-seat15-nav-fix-f44v3`, firstboot
markers, collaboration-admission JSON, and `node-signing.key`. `browser-vm`
was destroyed (now shut off).

## Enroll

First enroll hit `104.236.118.177:4243` connection refused: dest-cut
`mackesd-control` on LH1 (and LH2/LH3) `Requires=`
`mcnf-collaboration-identity.service`, and the collaboration receipt path is
absent. LH1 enroll-endpoint cert was already present.

Corrected-forward on LH1 only: mask
`/usr/lib/systemd/system/mackesd-control.service.d/40-collaboration-identity.conf`
with `/etc/systemd/system/mackesd-control.service.d/40-collaboration-identity.conf`
→ `/dev/null`, start `mackesd-control`. `:4243` bound.

Installed `enroll --token-stdin` with `?fp=` routed to `join`. Join printed
`joined mcnf-clean-20260728 as peer:Basement-Test-Workstation (overlay
10.42.0.5)` and consumed the bearer. The CLI still exited non-zero:
nebula start-limit, grouped units blocked on the same collab Requires,
and `setup-syncthing` refused because `/mnt/mesh-storage` is a directory
not a mountpoint. Identity files were already written.

## Recovered-forward on the seat

- `verify-boot-recovery --identity-guard` PASS; `nebula.service` active;
  overlay ping to `10.42.0.1` ok.
- Wrote `/var/lib/mackesd/nebula/overlay-ip` `10.42.0.5` (join did not).
- Same collab drop-in mask as LH1 so `mackesd-control` can start.
- Construct (`mde-shell-egui`) remained active. Sunshine was not started.

## After

| Check | Result |
|---|---|
| host cert / CA | present |
| role pin | workstation |
| overlay | `10.42.0.5` on `nebula1`; LH1 ping ok |
| leftover review trees | gone |
| `browser-vm` | shut off |
| `/mnt/mesh-storage` | still not a mountpoint |

## Non-claims

- `production_admitted` was not flipped.
- Dell, Surface, Eagle, T480, LH2, and LH3 were not offboarded.
- Collaboration identity receipt was not invented.
- This is not a 13.0.0 freeze or publish.
- Construct Health Fix was not clicked; live dest-cut chrome is unchanged.

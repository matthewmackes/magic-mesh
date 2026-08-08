# WL-FUNC-021 newest release promotion across enrolled seats (2026-08-07)

## Result

The newest Fedora 44 workstation RPM was promoted to every enrolled seat that
accepted the configured deployment key and non-interactive root path: Dell and
seat 15. No package or service mutation was attempted on the three seats that
failed the read-only access preflight.

## Farm artifact

- Build host: `172.20.0.130` (BigBoy)
- Build slot: `promote-all-current-source-r8`
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=promote-all-current-source-r8 ./install-helpers/xcp-build.sh container-rpm 44`
- Package: `magic-mesh-12.1.6-5.x86_64.rpm`
- Size: `87,549,859` bytes
- SHA-256: `63e2c4e34aa429da6fb598885fa0bc2f8f84c153d133deffe0c4a4ef67ee8067`
- Shell feature cut: `drm,live-vdi,media-mpv`
- Base and lighthouse RPM payload-size checks: pass
- Refreshed farm source snapshot contained the provider-loss regression test;
  the local and farm `mde-musicd` engine source hash was
  `ccfa462e02235e816fac105c854a891c7bbc542b775619b1f450f4eb0d447d6e`.

## Deployment and live verification

The exact artifact hash was checked on each destination before an RPM test
transaction and install. `mackesd`, the user Music daemon, and the shell were
restarted using the existing service boundaries.

| Seat | Host | Installed package | Music live verifier | CPU proof |
| --- | --- | --- | --- | --- |
| Dell | `172.20.146.225` | `magic-mesh-12.1.6-5.x86_64` | PASS | max `222‰`, mean `193‰`, restarts `0 → 0`, PASS |
| seat 15 | `172.20.0.15` | `magic-mesh-12.1.6-5.x86_64` | PASS | max `400‰`, mean `115‰`, restarts `0 → 0`, PASS |

`verify-music-live-seat.sh` confirmed active `mde-musicd`, RPM-owned
`/usr/bin/mde-musicd`, Music Bus ping/get-state/list-albums responses, the
Music and shell payload paths, and clean `rpm -V` on both seats.

## Remaining enrolled-seat boundaries

The all-seat rollout is not closed because the remaining three endpoints did
not pass the access preflight:

| Seat | Host | Read-only preflight result | Promotion result |
| --- | --- | --- | --- |
| T480 | `172.20.146.68` | configured SSH key refused; overlay timed out | not attempted |
| Eagle | `172.20.146.88` | reachable, but still on `magic-mesh-12.1.6-2.x86_64` and non-interactive sudo unavailable | not attempted |
| Microsoft Surface | `172.20.146.79` | configured SSH key refused; overlay timed out | not attempted |

No password, alternate secret, or sudo bypass was used. The five-seat CPU/NWS
acceptance, live provider-loss recovery, renderer proof, and live cross-seat
handoff remain open in `WL-FUNC-021`.

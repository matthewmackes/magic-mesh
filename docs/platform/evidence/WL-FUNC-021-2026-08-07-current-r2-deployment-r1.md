# WL-FUNC-021 current release-5 deployment (2026-08-07)

## Scope

This records the fresh Fedora 44 package cut after the provider-terminal
authority and stale-video-frame fixes, and its deployment to the two live
Music seats.

## Farm artifact

- Build host: `172.20.0.130` (BigBoy)
- Build slot: `music-current-release5-media-authority-r2`
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-current-release5-media-authority-r2 ./install-helpers/xcp-build.sh container-rpm 44`
- Package: `magic-mesh-12.1.6-5.x86_64.rpm`
- Size: `87,537,581` bytes
- SHA-256: `8219d399ae7abf498f4916c9c43240628bbef02e9ef71971d235db3ada450be3`
- Shell features: `drm,live-vdi,media-mpv`
- `verify-rpm-payload.sh payload`: pass
- `verify-rpm-payload.sh size`: pass
- Required `qemu-kvm` and `libvirt-daemon-kvm` dependencies: present

The exact package was copied to `/tmp/magic-mesh-12.1.6-5-current-r2.x86_64.rpm`
on both seats; each remote copy reported the SHA-256 above. A non-mutating
`rpm -Uvh --test --replacepkgs --force --nosignature` transaction passed on
both hosts, followed by a successful real install.

## Live seat verification

| Seat | Host | Installed package | `rpm -V` | Services | Music live verifier |
|---|---|---|---|---|---|
| Seat 15 | `172.20.0.15` | `magic-mesh-12.1.6-5.x86_64` | clean | `mde-musicd`, `mackesd`, `mde-shell-egui` active | PASS |
| Dell | `172.20.146.225` | `magic-mesh-12.1.6-5.x86_64` | clean | `mde-musicd`, `mackesd`, `mde-shell-egui` active | PASS |

`verify-music-live-seat.sh` confirmed RPM ownership of `/usr/bin/mde-musicd`,
Bus ping/get-state/list-albums responses, payload presence, zero daemon
restarts, and clean installed-file verification on both seats. This proves the
fresh package and service boundary; physical renderer, provider-loss recovery,
cross-seat handoff, and five-seat CPU/NWS proof remain open.

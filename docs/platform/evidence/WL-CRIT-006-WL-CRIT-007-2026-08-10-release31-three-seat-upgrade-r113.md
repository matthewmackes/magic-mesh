# WL-CRIT-006 / WL-CRIT-007 — release 31 three-seat upgrade (r113)

Date: 2026-08-10

Source revision: `b2895c9f96e01e5ba1e51714d93d489ed4f46156`

## Artifact

The native Fedora 44 BigBoy lane built `magic-mesh-12.1.6-31.x86_64.rpm` with
the governed DRM, live-VDI, and media-mpv feature set. The final signed RPM is
90,846,001 bytes with SHA-256
`8b58c1c36e0a3d530d78885632037d51b2856d16810daf52ccd17efe20f48269`.
Its RSA/SHA-256 header signature uses release signing subkey
`E8EAC651D0921C73`; the signed `SHA256SUMS` bundle verified before rollout.

This artifact predates commits `73dbdc0a`, `7f757e23`, and `094082a8` and is
therefore an engineering-preview/live-upgrade checkpoint, not production
promotion of the current branch.

## Required three-seat rollout

The operator alert was published and its mandatory five-second delay completed
before every seat mutation. Dell, seat 15, and Surface each:

- received byte-identical RPM bytes matching the signed artifact hash;
- passed `rpm -Uvh --test --replacepkgs --force` before installation;
- upgraded from exact `magic-mesh-12.1.6-30.x86_64` to exact
  `magic-mesh-12.1.6-31.x86_64`;
- passed `rpm -V magic-mesh` with no verification findings;
- returned with `nebula.service`, `mackesd.target`, all six grouped daemon
  services, and `mde-shell-egui.service` active;
- reported shell `NRestarts=0` and identical installed binary hashes:
  `mackesd` `54a2848163d4882c17d0221b0a418003d301c7c9efbd6dccb2184828f8b867cd`,
  shell `ad1067c513dd668adcd2e68b10260c6d9b50e990ce0b86d0285d513969776761`.

Dell's persistent `browser-vm` remained running, autostart-enabled, four-vCPU,
8-GiB, with its exact overlay disk and control seed unchanged:
`browser-vm-r13-af3348bc-overlay.qcow2` and
`browser-vm-r13-control-seed/seed.iso`.

Dell and seat 15 retain their pre-existing non-core `fwupd-refresh.service`
failure. Surface has no failed system unit. The RPM scriptlet printed transient
systemd transport-reset diagnostics while replacing the running daemon stack;
the transaction itself completed and the final package, payload, grouped
services, shell, and Browser VM state all verified directly.

## Remaining release proof

This closes the release-31 package upgrade on the required three-seat physical
baseline. It does not prove current-HEAD GitHub checks, three-lighthouse
convergence, failure injection, reboot pixels, signed schema-5 promotion, or
production readiness; those WL-CRIT-006/WL-CRIT-007 gates remain open.

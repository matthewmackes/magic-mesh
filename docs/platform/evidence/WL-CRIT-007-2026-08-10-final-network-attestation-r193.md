# WL-CRIT-007 final network attestation before desktop recovery — r193

- Scope: `mesh-peer-recovery.sh` now re-attests physical network readiness after
  grouped `mackesd` services become ready and before Workstation-only XDG bind
  recovery. A link loss during grouped-service settling reports
  `offline-before-desktop`, leaves the already-observed service starts intact,
  and refuses the final desktop mutation.
- Focused farm gate: `.90` (`172.20.0.90`), slot
  `crit007-final-network-attestation-r193`.
- Command: `sudo -n bash install-helpers/test-mesh-peer-recovery.sh` after
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=crit007-final-network-attestation-r193
  install-helpers/xcp-build.sh sync`.
- Result: all recovery fixtures passed, including the injected late link loss;
  the fixture observed `offline-before-desktop`, allowed only the additive
  grouped-start prefix, and recorded no `xdg-binds` mutation.
- Additional checks: `bash -n install-helpers/mesh-peer-recovery.sh
  install-helpers/test-mesh-peer-recovery.sh` passed.
- Live limits: physical suspend/resume, real NetworkManager transitions,
  installed-seat desktop recovery, and fleet/lighthouse convergence remain
  unverified.

# WL-CRIT-007 overlay-to-substrate network attestation — r184

- Scope: `mesh-peer-recovery.sh` now re-attests physical network readiness after
  Nebula's bounded TUN-readiness wait and before configured etcd/Syncthing or
  grouped recovery can mutate state.
- Focused farm gate: `.90` (`172.20.0.90`), slot
  `crit007-overlay-substrate-attestation-r184`.
- Command: `sudo -n bash install-helpers/test-mesh-peer-recovery.sh` after
  `xcp-build.sh sync`.
- Result: all recovery fixtures passed, including the injected link loss after
  Nebula readiness; the only recorded mutation was `nebula.service`, and no
  configured substrate or downstream service mutation occurred.
- Limits: this is a deterministic fault fixture. Physical suspend/resume,
  real NetworkManager transitions, and fleet convergence remain live-proof
  requirements.

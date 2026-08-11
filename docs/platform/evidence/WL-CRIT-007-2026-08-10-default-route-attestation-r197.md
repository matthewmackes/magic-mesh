# WL-CRIT-007 default-route network attestation — r197

- Scope: `mesh-peer-recovery.sh` now requires an actual IPv4 default route in
  addition to NetworkManager or systemd-networkd online state. A cached global
  manager-online result without a usable substrate route is refused before
  lock acquisition or service mutation.
- Focused farm gate: `.90` (`172.20.0.90`), slot
  `crit007-default-route-attestation-r197`.
- Command: `MCNF_BUILD_HOST=172.20.0.90
  MCNF_BUILD_SLOT=crit007-default-route-attestation-r197
  install-helpers/xcp-build.sh sync`, followed by
  `sudo -n bash install-helpers/test-mesh-peer-recovery.sh` in the synced farm
  workspace.
- Result: all recovery fixtures passed. The new default-route fixture observed
  `offline-no-mutation` with NetworkManager still reporting online, and the
  mutation log remained empty. Existing stale-network, overlay-loss,
  substrate-order, role, lighthouse, grouped-runtime, late-network,
  single-flight, and trigger fixtures also passed.
- Live limits: physical link transitions, route loss under a real manager,
  installed-seat recovery, and fleet/lighthouse convergence remain
  unverified.

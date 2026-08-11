# WL-CRIT-007 evidence — recovery substrate boundary (r218)

- Revision: working tree before commit `r218`
- Scope: post-etcd recovery ordering
- Change: recovery re-attests physical network readiness after etcd starts and
  before Syncthing or downstream grouped-service mutation. Link loss emits
  `offline-after-etcd` and exits without stale downstream mutation.
- Farm host: `172.20.0.90`
- Farm slot: `crit007-recovery-substrate-boundary-r218`
- Gates:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=crit007-recovery-substrate-boundary-r218 install-helpers/xcp-build.sh sync`
  followed by the farm shell invocation of
  `sudo -n bash install-helpers/test-mesh-peer-recovery.sh`.
- Result: all recovery fixtures passed, including the new substrate-boundary
  fixture; `git diff --check` passed.
- This is focused behavioral coverage; no broad test expansion was added.

# MCNF Promotion Pipeline

Order: Build RPM on the farm → L1 clean install → L2 mini-mesh feature test →
L3 stability → L4 staged lighthouse replacement → Eagle → live DO lighthouses.
The loop repeats until the worklist is clear, all gates are green, and the
operator declares the release complete.

Entrypoint:

```bash
automation/promotion/mcnf-promotion-cycle.sh cycle
```

Safety:

- Every run starts with DO account-limit inventory.
- Defaults cap active DO droplets at `MCNF_DO_MAX_ACTIVE=8` and require
  `MCNF_DO_MIN_FREE=2` free droplet slots.
- Live DO promotion requires `MCNF_ARM_LIVE=1`.
- A red tier stops the cycle; do not run Eagle or DO promotion until L1-L4 are
  green for the candidate RPM.
- Verified promotion stages publish `event/dc/promote/{build,eagle,do}` so the
  Workbench Datacenter strip has the same evidence as the CLI. When the
  orchestration host lacks `mde-bus`, the script publishes to Eagle's Bus over
  SSH.
- `live-audit` is the post-promotion substrate guard: it verifies the promoted
  DO lighthouses and Eagle are on the candidate package, core services are
  active, `qnm-shared`/LizardFS are not enabled, and no FUSE/LizardFS mounts are
  present. It also publishes actual installed versions for Eagle and each live
  lighthouse to `event/dc/promote/*`, so the version matrix shows host-level
  drift instead of only the target DO version.
- `media-verify` is the MEDIA-LIGHTHOUSE live gate. It rechecks DO limits, then
  verifies `music.mesh`, `music-writer.mesh`, and the shared Navidrome account.
  Add `--mutate-playlist` only when a temporary playlist write/read/delete proof
  is intended.
- Artifacts are taken from `MCNF_BUILD_ARTIFACTS` or built with
  `install-helpers/xcp-build.sh rpm`.

Useful stages:

```bash
automation/promotion/mcnf-promotion-cycle.sh inventory
automation/promotion/mcnf-promotion-cycle.sh build
automation/promotion/mcnf-promotion-cycle.sh l1
automation/promotion/mcnf-promotion-cycle.sh l2
automation/promotion/mcnf-promotion-cycle.sh l3
automation/promotion/mcnf-promotion-cycle.sh l4
automation/promotion/mcnf-promotion-cycle.sh eagle
MCNF_ARM_LIVE=1 automation/promotion/mcnf-promotion-cycle.sh do
automation/promotion/mcnf-promotion-cycle.sh live-smoke
automation/promotion/mcnf-promotion-cycle.sh live-audit
automation/promotion/mcnf-promotion-cycle.sh media-verify
automation/promotion/mcnf-promotion-cycle.sh media-verify --mutate-playlist
```

2026-07-06 production-candidate evidence: `magic-mesh-11.4.8-1.x86_64`
passed L1/L2/L3/L4, promoted to Eagle and both DO lighthouses, passed live
smoke, and passed `live-audit`. The production release still requires operator
bug hunting and an explicit release declaration.

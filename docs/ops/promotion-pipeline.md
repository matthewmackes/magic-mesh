# MCNF Promotion Pipeline

Order: Build RPM on the farm → L1 clean install → L2 mini-mesh feature test →
L3 stability → L4 staged lighthouse replacement → Eagle → live DO lighthouses →
live audit → fd/EMFILE soak. Media/file-sharing lighthouse promotion is retired.
The loop repeats until the worklist is clear, all gates are green, and the
operator declares the release complete.

Rollback (the inverse: re-point the channel to the previous NEVRA and downgrade
the fleet) is a separate, typed-confirm path documented in
[`docs/RELEASE-ROLLBACK.md`](../RELEASE-ROLLBACK.md)
(`automation/promotion/mcnf-channel-rollback.sh`).

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
  present. It also proves the effective `mackesd` stop policy resolved by
  systemd is `TimeoutStopUSec=1min 30s` and
  `TimeoutStopFailureMode=terminate`, with the packaged
  `mackesd.service.d/90-stop-policy.conf` present and no stale local
  `20s`/`abort` watchdog override. It publishes actual installed versions for
  Eagle and each live lighthouse to `event/dc/promote/*`, so the version matrix
  shows host-level drift instead of only the target DO version.
- The retired `media-verify` stage is not a lighthouse gate. Media/file-sharing
  workloads belong on non-lighthouse hosts and are outside this promotion path.
- Artifacts are taken from `MCNF_BUILD_ARTIFACTS` or built with
  `install-helpers/xcp-build.sh rpm`.

Useful stages:

```bash
automation/promotion/mcnf-promotion-cycle.sh status
automation/promotion/mcnf-promotion-cycle.sh statrep
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
automation/promotion/mcnf-promotion-cycle.sh fd-soak
```

`status`/`statrep` is read-only and is the first command to run before deciding
whether another promotion cycle is needed. It reports the latest candidate RPM
and sha256, current worklist open/in-progress count, DO account headroom, live
lighthouse/Eagle installed versions, farm utilization, and the release
declaration marker. Completion is deliberately strict: the status report must
show zero open worklist items, current gates green on the final candidate, live
audit/soak green after that candidate, and an operator-authored
`docs/ops/production-release-declaration.md`.

`cycle` runs the fd soak after `live-audit`; the soak defaults to one hour
through `automation/promotion/live-fd-soak.sh`. The former media-lighthouse
verification and playlist mutation stages are retired.

2026-07-07 production-candidate evidence: latest rebuilt
`magic-mesh-12.0.0-1.x86_64` (`/root/mcnf-release-artifacts`, 112291230
bytes, built 13:19 EDT, sha256
`7e780ab7aee218116865a08b667cf04e7042a6b34d68759f80c3a3439489e251`)
carries the Inter platform font, the bottom Windows-style notification rail, the
session rail, the bounded Caddy/SELinux install behavior, the fd-budget guards,
the packaged `mackesd` stop-policy drop-in, and the `%post` cleanup for stale
local `mde-shell.service` Construct units. It passed L1 clean install (6 passed),
L2 mini-mesh (15 passed), L3 stability/fd budget (14 passed), and L4 staged
lighthouse replacement (33 passed). It promoted to Eagle and both DO lighthouses
by force-replacing the same NEVRA package, then passed post-roll `live-smoke`,
`live-audit`, and the one-hour fd/EMFILE soak. Eagle needed the expected
seat-owner correction after the RPM
replacement stopped Construct: Cosmic was terminated, `mde-shell-egui.service` was
started, `/dev/dri/card1` and `/dev/tty1` were owned by `/usr/bin/mde-shell-egui`,
and the stale local `/etc/systemd/system/mde-shell.service` stayed absent. The
soak (`automation/promotion/live-fd-soak.sh`, start
`2026-07-07 17:56:29 UTC`, duration `3600s`) finished at elapsed `3603s` with
all promoted services active, `LimitNOFILE=65536`, EMFILE `0`, and final fd
counts `142` (`104.131.64.207`), `140` (`165.227.188.238`), and `171` (Eagle).
(`music.mesh=2`, `music-writer.mesh=1`, Subsonic ping ok, temporary playlist
create/read/delete ok). The production release still requires operator bug
hunting and an explicit release declaration.

2026-07-07 browser stop/compact-chrome bench evidence: a later
`magic-mesh-12.0.0-1.x86_64` candidate was rebuilt on BigBoy for the Browser
Stop control and compact Chromium-style chrome (`/root/mcnf-release-artifacts`,
112643918 bytes, built 21:41:57 EDT, sha256
`defb01d677ac56af2fff312e43d17984a2fb19cd9d13e3b66cd3a46ca641a734`). It passed
L1 clean install (6 passed), L2 mini-mesh (15 passed), L3 stability/fd budget
(14 passed), and L4 staged lighthouse replacement (33 passed) on the farm
testbed. Eagle was excluded from this bench pass by the operator's 2026-07-07
directive, and the encrypted bench seats were not rebooted.

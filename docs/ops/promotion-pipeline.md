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

2026-07-30 Dell GUI Alpha proof evidence: BigBoy slot
`alpha-dell-12-1-6` built Fedora 44 packages from release commit
`5ee166c3` with base `magic-mesh-12.1.6-1.x86_64.rpm` (82.3 MiB,
sha256 `c2af231716d3973dcd5dfb81eadd8647b10c431d0f046cdc0f0742a27b646ba4`),
browser `magic-mesh-browser-12.1.6-1.x86_64.rpm` (39.0 MiB, sha256
`a29f21ef77e9ec222dd727a98e8677560f8a2206d9e1ae40007b0950ecc20475`), and
lighthouse `magic-mesh-lighthouse-12.1.6-1.x86_64.rpm` (11.3 MiB, sha256
`4e1e3b6be87768f409df0b69088b8d3b8005985e90e484f584dfbc1811cc8124`).
All three passed the Fedora 44 payload-size guard. On Dell-LAPTOP
(`172.20.146.225`), the base/browser pair passed `rpm -Uvh --test`, installed
over 12.1.5, passed `rpm -V`, reported 12.1.6 through `mackesd`,
`mde-shell-egui`, and `mesh-help`, and left `mde-shell-egui.service` active
after restart. This is a Dell proofing Alpha, not a production promotion:
the candidate was unsigned and no live fleet promotion was attempted.

2026-07-29 Dell GUI Alpha proof evidence: source tip `247db21d` (wallpaper
enable/selection, animated responsive tray, native DRM clipboard clear,
route-readiness gating, and This Node narrow/large-text wrapping) was cut in
BigBoy slot `alpha-dell-gui-wave-f44-20260730` using the Fedora 44 container
lane. Base `magic-mesh-12.1.6-1.x86_64.rpm` (82.3 MiB, sha256
`5492561ab3fcaa97a1f72922141f4b9e38842b156bb43168ad711c3863d6dbe6`), browser
`magic-mesh-browser-12.1.6-1.x86_64.rpm` (39.1 MiB, sha256
`7819c9423907ea2b08f0a85671c397720c5336f1188bdb18824465e939d29c56`), and
lighthouse `magic-mesh-lighthouse-12.1.6-1.x86_64.rpm` (11.3 MiB, sha256
`b336256288536ac99aa87d450f872a3262e135c03b60881392b7f47afa8254dc`) all
passed the 90 MiB payload guard. On Fedora 44 Dell-LAPTOP
(`172.20.146.225`), the base/browser transaction test passed, installation
with `--replacepkgs` completed, `rpm -V` was clean, `mackesd --help` and
`mesh-help --help` passed, and `mde-shell-egui.service` was active with
`NRestarts=0` and a live `mde-shell-egui` process. The first restart was
canceled by the known tty/getty handoff; a separate `systemctl start` restored
the service. The GUI binary's headless `--help` probe is not counted because
Winit requires a display. This remains a Dell proofing Alpha, not a production
promotion; artifacts are unsigned and no fleet promotion was attempted.

2026-07-30 integrated engineering Alpha cut for Dell proofing: source tip
`49839583` (live Communications projections, native DRM/VNC text clipboard,
MG90 health rail, provider-aware This Node, overlay-safe Home gestures, and
bounded animated taskbar/tray geometry) was built on BigBoy through
`install-helpers/xcp-build.sh rpm`. Base
`magic-mesh-12.1.6-1.x86_64.rpm` (82.2 MiB, sha256
`d87c787cb4e97bcb5987996f03a7e03207c4d08a5c61a5b1f4028740ae979537`), browser
`magic-mesh-browser-12.1.6-1.x86_64.rpm` (38.9 MiB, sha256
`65e5b6dd9d48820d0f67f60db4a1fb8ddae5f26491062987e07cbee05fbfff6e`), and
lighthouse `magic-mesh-lighthouse-12.1.6-1.x86_64.rpm` (11.2 MiB, sha256
`c2f616fc31b3b323bb2f47a558c85412908bb990206c2b1144ea298375ebd5bb`) all
passed the 90 MiB payload guard. The cut is staged in
`/root/mcnf-release-artifacts` for Dell proofing. It is unsigned and has not
been promoted to the fleet; live Dell installation and GUI visual signoff
 remain the next proofing action.

2026-07-30 final integrated engineering Alpha cut for Dell proofing: source tip
`5c894463` (retired Home App Grid, Front Door accessibility hint, honest
RDP/SPICE clipboard capability seams, and deterministic all-source MG90
selection) was built on BigBoy through `install-helpers/xcp-build.sh rpm`.
Base `magic-mesh-12.1.6-1.x86_64.rpm` (82.2 MiB, sha256
`429e374acf048ee65f328f835788deb54072629ddd7481c5380e49eba8f2b9db`), browser
`magic-mesh-browser-12.1.6-1.x86_64.rpm` (38.9 MiB, sha256
`67cd2493c8461238eb3498f93e80537154a308c93919943b3f2ff45942199b16`), and
lighthouse `magic-mesh-lighthouse-12.1.6-1.x86_64.rpm` (11.2 MiB, sha256
`940b03bf0f1eb20f61feaf6e70708c3b085b3268932c25612801962262bd1d6a`) all
passed the 90 MiB payload guard. Artifacts are staged in
`/root/mcnf-release-artifacts` for Dell proofing. This Alpha is unsigned and
not fleet-promoted; live Dell installation and GUI visual signoff remain.

2026-07-30 refreshed integrated engineering Alpha cut for Dell proofing: source
tip `5086861f` (searchable taskbar catalog without retired launcher groups,
DRM clipboard no-op rejection, and This Node disabled-action accessibility
reasons) was rebuilt on BigBoy through `install-helpers/xcp-build.sh rpm`.
Base `magic-mesh-12.1.6-1.x86_64.rpm` (82.2 MiB, sha256
`2b083cd680990eddafe75d9313901338e1f674cf4061aaef7b3759d354c1a95d`), browser
`magic-mesh-browser-12.1.6-1.x86_64.rpm` (38.9 MiB, sha256
`f5f2444a962da692aaf508933e6cc8732e310d682d5a4cc7d66ef58d2fa54c00`), and
lighthouse `magic-mesh-lighthouse-12.1.6-1.x86_64.rpm` (11.2 MiB, sha256
`1c8afe79ab12a766b36db8ef16b3330e9ee27cc6b87b6204c1718d154ac339a5`) all
passed the 90 MiB payload guard. Artifacts are staged in
`/root/mcnf-release-artifacts` for Dell proofing. This Alpha is unsigned and
not fleet-promoted; live Dell installation and GUI visual signoff remain.

2026-07-30 refreshed integrated engineering Alpha cut for Dell proofing: source
tip `af8c17b2` (truthful Maps Airspace stale-health presentation and Quazar
per-theme disabled-state contrast) was rebuilt on BigBoy through
`install-helpers/xcp-build.sh rpm`. Base
`magic-mesh-12.1.6-1.x86_64.rpm` (82.2 MiB, sha256
`03e02dceb97b908bd96e85d0ab367eb63c74d70886bff76fbb07a561ff6122d5`), browser
`magic-mesh-browser-12.1.6-1.x86_64.rpm` (38.9 MiB, sha256
`7a5263c4c5b697cb36efe95a25b98939bd666386052aec496d4ad718fcc4aca1`), and
lighthouse `magic-mesh-lighthouse-12.1.6-1.x86_64.rpm` (11.2 MiB, sha256
`e221fefe728c4444d4cf6d95e543f43ab4d5c76b48b4bcb5a67a7f14ae2e30e6`) all
passed the 90 MiB payload guard. Artifacts are staged in
`/root/mcnf-release-artifacts` for Dell proofing. This Alpha is unsigned and
not fleet-promoted; live Dell installation and GUI visual signoff remain.

2026-07-07 production-candidate evidence: latest rebuilt
`magic-mesh-12.0.0-1.x86_64` (`/root/mcnf-release-artifacts`, 112291230
bytes, built 13:19 EDT, sha256
`7e780ab7aee218116865a08b667cf04e7042a6b34d68759f80c3a3439489e251`)
carries the historical Inter platform font, the bottom Windows-style notification rail, the
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

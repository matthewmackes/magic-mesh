# Needs-Operator — blocked worklist items (2026-06-24)

These worklist tasks are code-complete (or have no buildable artifact) and cannot be advanced from a build — each needs a live-infra or operator action. Marked `[!]` in `docs/WORKLIST.md`. Classified by the 2026-06-24 assessment.

## DO Spaces media bucket

These all share one blocker: the DigitalOcean Spaces bucket + S3 access keys are console-minted (doctl has no key-gen verb) and don't exist on the dev host, so nothing downstream can be built or live-verified. The Navidrome substrate is also an unpackaged standalone helper (`install-helpers/setup-media-navidrome.sh`) with no Cargo.toml/RPM-spec asset and no mackesd worker yet.

- **MEDIA-2** — DO Spaces keys are console-minted and no S3 keys / rclone config exist on the dev host; the bucket + sealed key-pair require an operator action in the DO console that cannot be produced in a build.
- **MEDIA-3** — Navidrome substrate is an unpackaged helper with no mackesd worker; the live Subsonic-API acceptance (ping.view from another node) needs a live Lighthouse_Media node + the MEDIA-2 bucket/keys.
- **MEDIA-4** — the rclone bucket-mount substrate lives only in the unpackaged helper; the scan/cover-art/stream acceptance needs the live MEDIA-2 bucket.
- **MEDIA-6** — only the env-var half exists; idempotent account creation, the durable shared-playlist write path, and end-to-end stream/browse all need the live MEDIA-2 bucket + a running Lighthouse_Media instance.
- **MEDIA-9** — no upload path or rescan trigger is wired, and every acceptance (upload, rescan refresh, tracks appear in mde-music) needs the live MEDIA-2 bucket + running Lighthouse_Media instances.
- **MEDIA-10** — pure live verification (>=2 Lighthouse_Media nodes serving the same bucket, kill-one resilience, fresh-node auto-config) requiring real DO infrastructure + the MEDIA-2 bucket/keys.

## SUBSTRATE-V2 cutover (etcd + Syncthing, operator-gated big-bang)

- **OPROG-2** — KEYSTONE: code-complete + dormant; the acceptance is the operator-gated, rehearsed big-bang cutover on the live fleet (gated by writing `/etc/mackesd/etcd-endpoints`), an operator go-ahead — not a build-verifiable change.
- **INCIDENT-WEDGE-2** — cutover tooling exists, but the acceptance requires running the operator-gated SUBSTRATE-V2 cutover on the live founding lighthouse so it coordinates via etcd and new joins use Syncthing (no FUSE).
- **OPROG-4** — supporting code exists, but the deliverable is live provisioning of 3 lighthouse nodes (2 Lighthouse_Media), gated on the OPROG-2 cutover.

## Live multi-node / DO infra verification

- **LH-JOIN-QNM-1** — both code fixes landed (wedge-proof mount loop + source-side share guard); the two remaining acceptance bullets both require the fix shipped in an RPM and re-verified on a real DO lighthouse / VM bed.
- **CONNECT-3** — firewalld enforcement worker is code-complete + unit-tested; only live-lighthouse end-to-end verification remains, gated on a live node / the down build-farm VM.
- **DATACENTER-2** — http-over-etcd Tofu backend built + both states migrated + lock-block proven; the sole open bullet (a plan from two eligible nodes sees identical state) needs etcd clustered across live nodes + a literal 2nd-node run.
- **DATACENTER-3** — age-into-etcd secret store built + XAPI/DO creds resolve from it; the open acceptance needs the store replicated to other live nodes + the live UniFi cred (coupled to DC-14) — live multi-node distribution.
- **DATACENTER-23** — DR backup/restore + leader-gated scheduler + RPC/button built and round-trip-verified; the open acceptance (off-fleet push target + a guided restore that re-elects a leader on live infra) are operator/live-infra actions.

## Live nightly test tiers (need a cut RPM + live mesh running over time)

- **BUILD-PLATFORM-5** — machinery built + reachable; a green per-feature nightly run needs the live snapshot-reset VM pool + a cut RPM + the etcd/Syncthing substrate on the 2-node bed.
- **BUILD-PLATFORM-6** — soak/chaos/reboot runner built + wired; the acceptance needs a live multi-node mesh + cut RPM running over the soak/chaos window.
- **BUILD-PLATFORM-7** — aggregator + Workbench Build panel code-complete; the live nightly summary depends on BUILD-PLATFORM-5/6 actually running on live infra.
- **BUS-RUN-FULL-1-acc5** — the parent task is done; this acceptance sub-bullet needs a multi-hour soak observed on a real 391 MB-/run VM. The code path is unit-soak-tested + the GC runs live, but the live observation is an operator/infra action.

## Real Cosmic-session / hardware compositor verification

- **MOTION-TRANS-4** — no resize-motion code yet, and the acceptance (resize smooth under compositor load, no full-window flash) is real-Cosmic-session compositor behaviour that can't be verified in a headless build.
- **MOTION-PERF-4** — no code artifact, and the acceptance (no clipping/blur/jitter at 1.0/1.25/1.5/2.0; smooth under GPU/CPU/network stress) is an explicit hardware / live-Cosmic-session compositor verification.

## Physical-host operator action

- **OPROG-5** — pure live-infra operator action: tear down the nodes on XCP-1, migrate any required instances to XCP-2, decommission XCP-1. No code/artifact deliverable.

## 2026-06-25 reconcile-rescue: additional confirmed blockers (adversarially verified)
- **BIRTHRIGHT-1** (reverted [✓]→[!]) — provisions LizardFS at enrollment but /mnt/mesh-storage never mounts (LH-JOIN-QNM-1/OPROG-1); resolves with the LizardFS rip-out (OPROG-2) + a fresh-from-RPM live join verify.
- **XCP-6** ([!]) — directory-advert writer uses HostTarget::Local (dead on glibc-2.17 dom0) + nothing reads compute/xcp-host/*; reopen needs a dom0-viable target + a real live directory consumer.
- **BRAND-2** ([!]) — Carbon icon theme code + tarball are in-repo; only a live `gsettings get …icon-theme`=Carbon on a real Cosmic Workstation remains.
- **APPS-9b** ([!]) — Super→launcher applet toggle built+tested; only the operator-gated Cosmic shortcut RON remains (cosmic-comp is not a workspace dep, can't be authored headless).
- **BOOT-REC-4** ([!]) — live reboot/power-cycle auto-recovery drill of every role before /release; mount half superseded by the LizardFS→Syncthing cutover.
- **FARM-AUTO-PROD** ([!]) — all farm automation built; only the standing build-job tags executing on the live farm VMs remain (operator-gated live exec).
- **MUSIC-BROWSE/ART** ([!]) — full art chain wired daemon→GUI; only a live re-verify on a Workstation with mde-musicd running remains.
- **DRAIN-5** (stays [ ]) — cited spec §C.4 is absent + the unit count (21 vs 25) is stale; needs operator spec reconciliation + the /ship-skill encoding (skill self-edit is permission-gated).

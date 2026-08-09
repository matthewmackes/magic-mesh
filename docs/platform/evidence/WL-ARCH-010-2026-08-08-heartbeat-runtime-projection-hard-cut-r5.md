# WL-ARCH-010 heartbeat runtime-projection hard cut — 2026-08-08

Status: this authority-removal slice is implemented and focused farm proof
passes. `WL-ARCH-010` remains `Remaining`; native attachment and the full live
lifecycle/recovery matrix are not proved by this slice.

## Change

- Peer heartbeat descriptors no longer execute `podman ps --all` or
  `virsh list --all`, and the replicated `ServiceDescriptors` contract no
  longer publishes VM or container inventories.
- Remote desktop VM cards now fold each peer's validated, node-matched
  `state/workloads/<node>` snapshot. Peer descriptors retain only the seat's
  advertised RDP/VNC listeners, so there is one runtime/readiness projection.
- Old heartbeat JSON carrying `containers` or `vms` remains readable during a
  rolling upgrade, but a current writer discards and never republishes those
  retired fields.
- `lint-workload-authority.sh` now rejects raw heartbeat runtime commands,
  retired peer contract fields, and desktop-source readers of descriptor VMs.

## Verification

- BigBoy `.130`, slot `arch009-release22-f44-r1`: the focused release-mode
  peer-seat/typed-Workload fold passed 1/1 with 4,490 unrelated tests filtered.
- Machine `.90`, slot `arch010-small-profile-r2`: the final-tree integrated
  remote/local Workload roster and foreign-node projection refusal each passed
  1/1 with 4,490 unrelated tests filtered.
- Machine `.196`, slot `arch010-peer-contract-r5`: the rolling-upgrade contract
  test passed 1/1 with 484 unrelated tests filtered.
- Machine `.90`, slot `arch010-authority-lint-r5`: authority lint self-test and
  live repository scan passed.
- Machine `.50`, slot `arch010-descriptor-fmt-r5`: scoped `rustfmt --check`
  passed for the three changed Rust sources.
- Scoped `git diff --check` passed.

## Source identity

- `peers.rs`: `ba7278789591480168cbc69b1c2c10fb27f244b7286e1852d51118138c5f0f42`
- `descriptors.rs`: `a42dde5681717fbc08697126bd95f3583c5d8ca2134fc04bc972f3942ce18fe9`
- `desktop_sources.rs`: `4b93d6d7b0a3e2f4eb914d4bacecabce0c53b729b844dff9f77d4d0cab1f711a`
- `lint-workload-authority.sh`: `3bd557239f1f88efa4f5f29c7ed619baddc93d48cb71bc660695fe327e365edf`

This proves removal of the heartbeat's competing VM/container runtime view and
one typed projection for desktop cards. It does not claim Display1 first-frame,
input/audio/clipboard, live VM/container restart recovery, or release-seat
acceptance.

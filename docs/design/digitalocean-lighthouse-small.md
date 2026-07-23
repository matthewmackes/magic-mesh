# DigitalOcean Small Lighthouse

**Status:** design locked 2026-07-22. This is the stock control-plane
lighthouse profile, not the media-lighthouse class.

## Decision

The default DigitalOcean lighthouse target is the smallest Basic Droplet that
DigitalOcean currently publishes: `s-1vcpu-512mb-10gb` (one shared vCPU, 512 MiB
RAM and 10 GiB SSD). The provisioning scripts and the `mackesd
onboard spawn-lighthouse` planner use this slug by default. A caller can still
select a larger size explicitly when a mesh has unusually large state or peer
count.

The 512 MiB node is a relay/control-plane appliance. It runs Nebula, the local
etcd voter when it is a full lighthouse, `mackesd`, bounded Caddy ingress, and
`mesh-health` recovery.
It does not run a desktop, Navidrome, Netdata, the notification broker,
starship/shell bootstrap, or other optional first-boot fetches. Media remains
the separately sized `Lighthouse_Media` class; this avoids repeating the
historical low-memory Netdata/media OOM failure.

## Runtime guardrails

`install-helpers/configure-small-lighthouse.sh` is applied by both DO cloud-init
paths after `found` or `join` pins the role. It is idempotent and writes:

- systemd cgroup ceilings for `mackesd`, etcd, Nebula and Caddy;
- a 512 MiB emergency swapfile only when no swap already exists;
- bounded journald retention (64 MiB persistent / 16 MiB runtime, seven days);
- low swap aggressiveness; and
- a reversible disable list for optional workstation/bootstrap units.

The helper leaves all units and binaries installed, so a deliberate resize and
role promotion can re-enable them. `/etc/mackesd/lighthouse-profile` records the
effective profile as `small`, and `MDE_LIGHTHOUSE_PROFILE=small` is visible to
the daemon without putting a secret in the environment.

## Acceptance contract

A small lighthouse is effective when a fresh or joined node can, without manual
service surgery:

1. boot and keep Nebula reachable on UDP 4242;
2. serve enrollment on TCP 4243 and HTTPS fallback on TCP 443;
3. participate as an etcd voter and survive a mackesd restart;
4. maintain a two-lighthouse roster and reconnect after the peer is restarted;
5. remain below the 512 MiB host ceiling during a 30-minute join/reconcile soak
   with no host OOM kill (service-level cgroup pressure is allowed); and
6. retain at least 1 GiB free disk after bootstrap and bounded logs.

The live add/retire/add drill remains part of `WL-RUN-003`; the design and
local packaging proof do not pretend to replace that live DigitalOcean gate.

## Explicit non-goals

This profile does not promise media serving, general-purpose workload
execution, desktop rendering, or arbitrary peer counts on 512 MiB. Those duties
select a larger droplet or the `Lighthouse_Media` profile instead of silently
overcommitting the smallest instance.

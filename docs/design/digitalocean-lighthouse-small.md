# DigitalOcean Small Lighthouse

**Status:** design locked 2026-07-23. This is the only supported DigitalOcean
lighthouse profile: a thin control-plane appliance. Media and file-sharing
lighthouse classes are retired and must not be provisioned.

## Decision

Every DigitalOcean lighthouse uses the smallest Basic Droplet that DigitalOcean
currently publishes: `s-1vcpu-512mb-10gb` (one shared vCPU, 512 MiB RAM and 10
GiB SSD). The provisioning scripts and the `mackesd onboard spawn-lighthouse`
planner use this slug and reject role promotion or sizing that would create a
media, file-sharing, or general-purpose lighthouse.

DO cloud-init installs the dedicated `magic-mesh-lighthouse` RPM variant (or a
direct URL to that artifact). It is a control-plane-only package: `mackesd`,
Nebula, etcd bootstrap, bounded Caddy ingress, health recovery, and the base
SELinux policy. The variant deliberately omits Navidrome/music ingestion,
Syncthing/file-sharing helpers and unit files, browser/desktop, virtualization,
and birthright/optional first-boot payloads. The full `magic-mesh` and
`magic-mesh-server` packages are not valid DO lighthouse inputs.

The 512 MiB node is a relay/control-plane appliance. It runs Nebula, the local
etcd voter when it is a full lighthouse, `mackesd`, bounded Caddy ingress, and
`mesh-health` recovery.
It does not run a desktop, Navidrome, Netdata, the notification broker,
starship/shell bootstrap, file-sharing services, or other optional first-boot
fetches. There is no supported `Lighthouse_Media` or file-sharing subclass.

## Runtime guardrails

`install-helpers/configure-small-lighthouse.sh` is applied by both DO cloud-init
paths after `found` or `join` pins the role. It is idempotent and writes:

- systemd cgroup ceilings for `mackesd`, etcd, Nebula and Caddy;
- a 512 MiB emergency swapfile only when no swap already exists;
- bounded journald retention (64 MiB persistent / 16 MiB runtime, seven days);
- low swap aggressiveness; and
- a reversible disable list for optional workstation/bootstrap units.

The helper leaves package files installed for upgrade compatibility, but no
optional unit is enabled and the daemon refuses a lighthouse role with media
capability. `/etc/mackesd/lighthouse-profile` records the effective profile as
`small`, and `MDE_LIGHTHOUSE_PROFILE=small` is visible to the daemon without
putting a secret in the environment.

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

This profile does not promise media serving, file sharing, general-purpose
workload execution, desktop rendering, or arbitrary peer counts on 512 MiB.
Those duties belong on non-lighthouse nodes; no larger or media lighthouse
variant is supported.

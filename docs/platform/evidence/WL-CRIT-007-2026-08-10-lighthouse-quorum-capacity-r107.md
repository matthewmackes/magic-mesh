# WL-CRIT-007 — lighthouse quorum capacity recovery (r107)

Date: 2026-08-10

Revision deployed to the five physical seats: `d43ceef1e5fcbb5712eb6bcc2fc087a29b43d1ff`

## Incident and root cause

All five Fedora 44 seats had converged on signed
`magic-mesh-12.1.6-30.x86_64`, but coordinated publication and privileged
actions remained unavailable. Each of the three etcd endpoints reported
`RAFT NO LEADER`, and linearizable reads timed out. Nebula transport itself was
intact: every voter's `2379` and `2380` ports were reachable and raft streams
were established.

Read-only inspection of lighthouse `.1` proved resource starvation rather than
membership or firewall drift:

- the DigitalOcean inventory placed all three voters on 512 MiB / 1-vCPU / 10
  GB droplets;
- `.1` had 444 MiB usable RAM, 262 MiB swap in use, load above 25, and only
  about 6 MiB free;
- `vmstat` observed 5–11 MiB/s swap-in and swap-out with up to 98% system CPU;
- the kernel OOM-killed `mackesd` repeatedly, including at 07:53 and 08:11 UTC;
- etcd was itself carrying about 102 MiB of swapped pages; and
- raft terms advanced from 5504 through 5520 while leadership lasted only
  seconds. The three persisted voter IDs and cluster ID remained consistent.

The supported 512 MiB profile bounded individual cgroups, but aggregate Fedora,
Nebula, etcd, and monolithic release-5 mackesd demand still exceeded the host.
No etcd data directory, snapshot, member, certificate, or overlay identity was
deleted or rewritten.

## Corrected-forward capacity recovery

Every mutation emitted the red `AI-GENERATED-ALERT` and waited five seconds.
The first member's wedged monolithic mackesd was stopped to protect etcd. Its
graceful stop remained trapped in uninterruptible swap I/O, so only that service
cgroup was killed. The already-stopping `.1` etcd process was then force-killed
and restarted from the intact `/var/lib/etcd` directory.

The three DigitalOcean droplets were resized one at a time from
`s-1vcpu-512mb-10gb` to `s-1vcpu-1gb` without `--resize-disk`:

| overlay | droplet ID | region | result |
|---|---:|---|---|
| `10.42.0.2` | `588147933` | `fra1` | resized first, powered on, and rejoined before the next member |
| `10.42.0.1` | `588147605` | `nyc3` | resized second; formed a stable majority with `.2` |
| `10.42.0.3` | `588149952` | `sfo3` | resized last while two healthy 1-GiB voters remained online |

This is a reversible CPU/RAM-only resize: each disk remains 10 GB. DigitalOcean
reported all three droplets active with 1,024 MiB RAM. The monthly plan cost
changed from $4 to $6 per lighthouse, or $12 to $18 for the three-member fleet.

After `.1` and `.2` returned, three consecutive status samples held term 5534
with one leader and no errors. After `.3` returned, all members converged:

```text
10.42.0.1  leader=true   term=5534  applied=455216
10.42.0.2  leader=false  term=5534  applied=455217
10.42.0.3  leader=false  term=5534  applied=455218
```

All three `/health` endpoints then returned `{"health":"true","reason":""}`.

## Lighthouse release correction

The accessible `.1` member was upgraded corrected-forward from
`magic-mesh-lighthouse-12.1.6-5` to the signed thin
`magic-mesh-lighthouse-12.1.6-9` artifact built from `d43ceef1`.

- artifact SHA-256:
  `25cbf3f7fe44c33443d4dd5d0f95c7cc9e08ebd4ffca70b4d69c8e34a283f423`;
- `rpm -K`: `digests signatures OK`;
- `rpm -Uvh --test`: passed;
- installed `rpm -V magic-mesh-lighthouse`: passed;
- `/etc/etcd/etcd.env` retained SHA-256
  `e6130b27adef7b8ad6f32d5e874032d1964cc61e7b1a66aac60724779ef3428a`;
- all three original member IDs remained `started`; and
- the monolithic service is absent/inactive while all six bounded grouped
  services, etcd, and Nebula are active.

Release 9 on `.1` settled with about 419 MiB available. The stale local
file-backed DNF mirror still emits a harmless repository warning and should be
removed or repopulated in a separate packaging/operations correction.

## Five-seat acceptance hold

Two live passes separated by mesh-health watchdog executions proved:

- all 25 directed seat-to-seat overlay paths (`10.42.0.4` through `.8`) pass;
- Dell, seat 15, Eagle, Surface, and T480 all remain on exact
  `magic-mesh-12.1.6-30.x86_64` with clean installed-file verification;
- all six grouped mackesd services, `mde-shell-egui`, Nebula, and Syncthing are
  active on every seat with no failed MCNF service;
- each mesh-health invocation reports `Result=success` and
  `ExecMainStatus=0`;
- peer-publication stamps are fresh at 45–57 seconds;
- Nebula restart counters remain zero; and
- the cloud-arm and mesh-secret one-shots have systemd `Result=success`.
  Exit 124 on the bounded cloud lookup is an accepted success status, not a
  failed unit.

Dell's persistent Browser VM remains running, autostart-enabled, 4-vCPU / 8-GiB,
with UUID `a1100a2f-5b65-4064-ac9f-925e1affa1fb`. Its inactive XML SHA-256 is
unchanged at `a3157ab7c197ff8500cbef187742f002b2f7dc42bc64c36f9a45044b325dbe33`.

## Recurrence prevention and focused gates

The supported lighthouse plan is now `s-1vcpu-1gb` everywhere a node can be
planned or provisioned: founding and join helpers, cloud-init guidance,
promotion automation, the onboarding planner, Datacenter HCL, node-admin CLI,
OpenTofu defaults and validation, and operator/design documentation. The
former 512-MiB plan is rejected by strict callers and normalized to the sole
supported profile at the rolling wire boundary.

Only checks that cover the correction were run:

- machine 196, slot `lh-capacity-r107-ops`: shell syntax for the changed
  provision/promotion helpers plus worklist linter self-test and live lint;
- machine 193, slot `lh-capacity-r107-dc`: the three exact Datacenter
  lighthouse-resource library tests passed 3/3;
- machine 9, slot `lh-capacity-r107-spawn`: the unsupported-size normalization
  library test passed 1/1; and
- local OpenTofu `fmt -check` passed for `infra/tofu/zone1-do/variables.tf`.

The first Rust invocations accidentally omitted `--lib` and began linking
unrelated integration binaries. They were stopped rather than counted as
evidence; only the focused library results above support this checkpoint.

## Remaining limit

Lighthouses `.2` and `.3` still run release 5. The account's current local SSH
key is registered with DigitalOcean but was not injected into those two
existing droplets, so this checkpoint does not claim a three-lighthouse
release-9 rollout. Their capacity and quorum are healthy; completing the signed
package rollout requires a supported access correction or a quorum-preserving
add/retire replacement, not a password or membership shortcut.

# WL-CRIT-007 corrected-forward recovery evidence — 2026-08-08

## Scope

This slice exercises the package-owned boot/network-return recovery path on
`Basement-Test-Workstation` (`172.20.0.15`, workstation, Nebula
`10.42.0.5/17`) and `DELL-LAPTOP` (`172.20.146.225`, workstation, Nebula
`10.42.0.4/17`). It is evidence for two ordinary-seat reboot paths, Dell's
network-return path, and the required one-lighthouse-loss behavior. It does not
claim the still-open physical suspend/resume or complete fleet proof.

## Corrected-forward package progression

The recovery contract was corrected forward rather than bypassed:

- releases 14 through 16 established the root-owned XDG bind helper, bounded
  network-return orchestration, and host mount-namespace execution;
- release 17 admitted the canonical hyphenated grouped-worker names;
- release 18 preserved an already-active boot-time Syncthing process and made
  the outer controller wait for terminal `Type=notify` success;
- release 19 waited for all six grouped children instead of treating normal
  target activation as failure;
- release 20 preserved an already-activating grouped target and used one
  bounded non-blocking restart only when readiness failed to settle;
- release 21 requires a strict majority of configured etcd endpoints to commit
  proposals. This preserves fail-closed coordination while allowing the epic's
  required one-lighthouse-loss case.

Release 20 was built canonically in the farm Fedora container lane on BigBoy. The
installed artifact is `magic-mesh-12.1.6-20.x86_64`, 89,399,678 bytes, SHA-256
`3442b35b6ee336d70c0f4ebab221827190811b4f1d81ddc323ebb8a6f9849df7`.
`rpm -K`, NVR inspection, real-payload verification, controller/farm digest
comparison, and the package/helper source digest comparison passed. A prior
host-side package-only recut was rejected by `rpm -Uvh --test` before install
because it inherited stale host target binaries and old ABI requirements; it
was replaced by the canonical container artifact and never activated.

The Release 20 package-owned reboot changed boot ID from
`0727053b-42a9-4e1b-8710-35a9b2e55df7` to
`0bd486e3-804b-42ff-800f-9c816f1b2bdd`. The installed recovery unit settled
with `Result=success`, all six grouped mackesd services were active, Syncthing
was active, and the exact XDG binds were restored. The post gate then correctly
refused stale retired lighthouse addresses rather than reporting success.

## Lighthouse roster repair and one-loss proof

The seat bundle and filesystem fallback still named retired public addresses.
Root-only recovery backups were created under
`/root/mcnf-roster-repair-20260808-r1`; the four retired lighthouse fallback
rows were moved out of the active peer-directory path, and the seat bundle was
atomically updated to the live roster:

| Overlay | Public underlay | Directory identity |
|---|---|---|
| `10.42.0.1` | `104.236.118.177:4242` | `lh-mcnf-clean-20260728-1785239652` |
| `10.42.0.2` | `46.101.219.245:4242` | `lh-join-1785239775` |
| `10.42.0.3` | `64.23.131.57:4242` | `lh-join-1785240310` |

The current DigitalOcean inventory independently reported all three droplets
active. After transport restart, `.2` and `.3` were reachable over Nebula and
accepted TCP 2379; `.1` remained unavailable. Individual `etcdctl endpoint
health` calls to `.2` and `.3` each committed proposals. `endpoint status`
reported matching 57 MB databases, raft term 4980, advancing applied indexes,
and `.2` as leader. `member list` from each reachable endpoint agreed on the
same three voting members. This is a healthy strict 2-of-3 quorum with one
lighthouse unavailable.

## Three-lighthouse convergence and Dell roster correction

Dell still carried the same retired public lighthouse addresses in its
Nebula bundle and four retired peer-directory fallback rows. Before mutation,
the exact files were preserved under
`/root/mcnf-roster-repair-20260808-dell-r1`. The bundle was updated to the
three live rows above, the retired rows were moved outside the active peer
path, and `/etc/mackesd/etcd-endpoints` was aligned to `.1`, `.2`, and `.3`.
Dell then reached `.2`, `.3`, and the release seat over the overlay.

The live coordination plane subsequently exposed a transport/resource failure
rather than a data failure. All three persisted members retained cluster ID
`2bfed5089c4a5a0d` and the expected voter IDs, but legacy supervisor restart
storms saturated the Nebula UDP receive queues on the 444 MiB lighthouses and
prevented raft heartbeats. The stale `.2` environment roster was backed up as
`/etc/etcd/etcd.env.pre-quorum-repair-20260809-r1` and aligned with all three
members. Coordinated etcd restarts preserved both data directories. The
supervisor storms were stopped, Nebula was restarted on the exact three
lighthouses, and the original services were then restored.

After convergence, all three overlay paths passed bidirectional ping, every
Nebula UDP receive queue was empty, and each local `etcdctl endpoint health`
committed a proposal. The three members agreed on one 58 MB database and one
membership roster; `.3` was leader at term 5021 with applied index 417496.
All three `mackesd.service` instances returned active with zero restarts, and
Dell's installed boot gate reported strict `3/3` coordination health.

The corrected Release 21 source gate passed live on the seat with:

```text
ok   Nebula identity active
ok   one current, trusted Nebula identity
ok   mackesd target active
ok   /mnt/mesh-storage present (shared dir)
ok   syncthing active (file plane)
ok   etcd strict quorum healthy (2/3; require 2)
ok   bus healthz answers
ok   ~/Documents bind-mounted (FPG-7 sync)
BOOT-REC-4: PASS — node fully recovered.
```

The verifier self-test passes 13/13 locally and on farm 9 slot
`crit007-quorum-verifier-s4-r11`; shell syntax checks pass there as well.

## Release 21 target-ABI correction

The first Release 21 candidate was cut in an isolated Fedora 43 container on
BigBoy (`crit007-corrected-package-s4-r3`). Farm and controller copies matched
at SHA-256
`7546b42664c1592bae08847f999a12c6ee0109d43dbb105327ab607a432fc74c`;
the 89,530,908-byte `magic-mesh-12.1.6-21.x86_64` artifact passed `rpm -K`,
NVR, size, real-payload, and hard-Require checks. The Fedora 44 proof seat then
correctly refused `rpm -Uvh --test` because the candidate required Fedora 43
FFmpeg/mpv sonames (`libavcodec.so.61`, among others), while the installed F44
runtime provides the newer `.62` family. No install or seat mutation occurred.

Per `docs/BUILD-ENVIRONMENT.md`, Workstation release artifacts must be cut for
Fedora 44. A clean F44 container rerun completed in BigBoy slot
`crit007-f44-package-s4-r4`, image digest
`sha256:b138075cdea1309b4d63dce0a0ab23c0820f48ca089f2c3f3c537e30dff47dde`.
The image was invoked by mutable tag and the rustup installer was fetched
without a configured checksum, so those residual non-hermetic inputs remain
explicit release caveats even though the resolved image digest is recorded.

The resulting Fedora 44 artifact is
`magic-mesh-12.1.6-21.x86_64`, 89,749,711 bytes, SHA-256
`5735b0ea40424fbd4219665d24833dfb8b848f557e722e03ad3d937c102dca7b`.
The farm and controller digests matched; `rpm -K`, exact NVR, the real-payload
gate, hard-Requires inspection, and the 90-MiB size cap passed. Its FFmpeg/mpv
requirements use the Fedora 44 ABI family (`libavcodec.so.62`,
`libdisplay-info.so.3`, `libplacebo.so.360`, and `libudfread.so.3`). The proof
seat matched the controller digest, passed a separate `rpm -Uvh --test`, and
upgraded from release 20 to release 21. `rpm -V magic-mesh` was clean after
installation and again after reboot.

The installed recovery payload matched the source/package contract:

| Installed path | SHA-256 |
|---|---|
| `/usr/libexec/mackesd/verify-boot-recovery` | `87642912e1d3f9d00deac63e961a1a84b500c4282d4707d9bb9db5c07d2d937b` |
| `/usr/libexec/mackesd/mesh-peer-recovery` | `021a8416f4f37ca1c7e049255edcb736c7c29b286901c1a6cf4d8da3f65685ac` |
| `/usr/libexec/mackesd/mesh-xdg-bind-recovery` | `1c204099eeb1c28c094bacf2550c128900b7c20ac875ed7acd779d14e667d7b3` |
| `/usr/lib/systemd/system/mcnf-peer-recovery.service` | `9c7a85ee4bd7380683b7f2084908c10693a637b87d1f1a63649d4c0e5403cd50` |
| `/usr/lib/systemd/system/mcnf-xdg-bind-recovery.service` | `04e40463a06674dffe4237a4ef5986a188ebb65698d838b36e1aa013ae53360a` |

## Release 21 corrected-forward reboot

The package-bound preflight accepted the exact host, workstation role,
certificate-authoritative `10.42.0.5/17` overlay, and package ownership before
mutation. One warned, bounded reboot changed the boot ID from
`0bd486e3-804b-42ff-800f-9c816f1b2bdd` to
`86671e4c-5f82-4c64-b2d1-2bcbbbeadde9`.

The outer verifier initially refused after return because its generic
eight-second subprocess timeout was shorter than the aggregate package boot
gate. The same installed gate completed successfully in 11.09 seconds and then
15.27 seconds while checking the unreachable `.1` lighthouse plus the healthy
`.2`/`.3` quorum. The verifier now keeps ordinary probes at eight seconds and
gives only the explicit aggregate boot gate a bounded 30 seconds; its hostile
self-test remains 13/13. Two transient post snapshots honestly refused when
the package gate did not report `PASS`; a subsequent complete snapshot passed
without weakening any assertion.

The accepted post-reboot JSON proved:

- package `magic-mesh-12.1.6-21.x86_64` and package-owned recovery paths;
- one Nebula process and six unique matching grouped-worker processes;
- strict etcd quorum `healthy=2`, `configured=3`, `required=2`;
- ordered Nebula, substrate, and worker activation timestamps;
- active Syncthing/Bus substrate and a passing installed boot gate;
- one active `mde-shell-egui` process for `mm`; and
- exact active binds for Documents, Downloads, Music, Pictures, and Videos.

## Dell Release 21 corrected-forward reboot

Dell matched the accepted Fedora 44 artifact digest, passed `rpm -K`, exact
NVR, and a separate `rpm -Uvh --test`, then upgraded from release 12 to
`magic-mesh-12.1.6-21.x86_64`. The installed package passed `rpm -V` before
and after reboot. A transient post-transaction transport error was not treated
as success; the grouped target and strict boot gate were required to settle.

The package-bound preflight accepted exact host `DELL-LAPTOP`, workstation
role, certificate-authoritative `10.42.0.4/17`, and package ownership. One
warned, bounded reboot changed boot ID from
`7423fbc9-6060-4144-b190-fc6b923268da` to
`3b3f5937-cc61-494d-aa35-777c22e97da6`. The installed network-return unit was
then exercised explicitly.

The accepted post-reboot snapshot proved Release 21, package-owned recovery,
one Nebula process, strict `3/3` coordination health, six unique grouped
workers, healthy Syncthing/Bus substrate, one active `mm` shell, ordered
substrate/worker starts, and exact active binds for Documents, Downloads,
Music, Pictures, and Videos. The Browser VM remained shut off; its exact
overlay disk and seed ISO remained attached, and the overlay disk retained its
pre-proof size and modification timestamp.

## Remaining acceptance boundary

WL-CRIT-007 remains `Remaining` because physical suspend/resume and the
remaining Eagle, T480, Surface, and lighthouse corrected-forward matrix are
separate acceptance requirements.

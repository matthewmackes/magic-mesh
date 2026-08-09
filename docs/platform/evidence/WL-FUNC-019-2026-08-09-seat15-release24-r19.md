# WL-FUNC-019 — Seat 15 Release 24 RDP recovery (r19)

Date: 2026-08-09

Artifact source revision: `9a81474fc2ff9e30933e36560b9cfe4cb7c417de`

Live seat: `Basement-Test-Workstation`, `172.20.0.15`, Fedora 44

Windows target: `172.20.146.54:3389`

## Corrected-forward artifact

BigBoy (`172.20.0.130`) built the full Fedora 44 RPM from a clean detached
worktree at the exact source revision above. The package identified as
`magic-mesh 12.1.6-24 x86_64` and had SHA-256:

```text
5264de454a7f19afbe9403a64ef37051fcf3a4dc0ab25b9467f3b3ccae56006f
```

The real-RPM payload and hard-requirement gates passed, including the shipped
`mackesd`, `mde-shell-egui`, grouped service units, RDP client, and resource
publisher credential assets. The 85.8 MiB base RPM passed the 90 MiB channel
limit. The Fedora base image was tag-pinned rather than digest-pinned and the
live-fetched rustup installer lacked a checksum pin; those are recorded build
reproducibility gaps, not payload or runtime failures.

The two exact farm regressions for this correction passed on machine 194:

```text
probe_nmap::tests::completed_probe_owns_the_snapshot_freshness_timestamp: 1 passed
workers::probe::tests::slow_probe_does_not_add_an_extra_cadence_delay: 1 passed
```

No broad test suite was added to this live correction.

## Controlled seat transaction

Before mutation, Release 23 was the sole installed package, `rpm -V
magic-mesh` was clean, and `mackesd.target` was active. The artifact was
streamed directly from BigBoy because the controller filesystem lacked room
for a local copy. Seat 15 reported the same SHA-256 and `rpm -Uvh --test`
passed.

`/usr/libexec/mackesd/seat-update-warning` then published the mandatory visible
`AI-GENERATED-ALERT` and completed its five-second wait before `rpm -Uvh`
installed Release 24. The temporary staged RPM was removed afterward; the
verified farm artifact remains on BigBoy.

Post-transaction proof:

```text
magic-mesh-12.1.6-24.x86_64
rpm -V magic-mesh: clean
mackesd.target: active
mackesd-{control,observation,actions,data,compute,integrations}.service: active
mcnf-resource-publisher-credential.service: active (exited), status=0/SUCCESS
```

The only unrelated failed system unit was the pre-existing
`fwupd-refresh.service`; no Magic Mesh unit was failed.

## Live RDP projection and scan transition

TCP 3389 remained reachable from seat 15. A completed Release 24 scan wrote
the target at `last_seen=1786281691`; the next completed inventory transition
advanced it to `1786281752` and retained both `ssh` and `ms-wbt-server` on port
3389. The service aggregator advanced the RDP observation from
`1786281691000` to `1786281752000`, and the universal resource catalog advanced
the same card's expiry from `1786281991000` to `1786282052000`. This directly
crossed a scan transition without the Release 23 disappearance.

The latest catalog card was:

```text
class: desktop
canonical_key: probe-rdp/172.20.146.54
display_name: Remote Desktop · 172.20.146.54
transport: rdp, 172.20.146.54:3389, trusted_lan
health: available
client adapter: construct.mde-vdi-rdp
action: connect (requires local approval)
```

This proves detection, freshness renewal, typed Remote Sessions projection,
and the governed connect boundary on seat 15. An authenticated Windows login
and rendered desktop were not attempted because Windows credentials were not
provided; this evidence does not claim them.


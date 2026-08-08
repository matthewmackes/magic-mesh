# WL-ARCH-008 T480 r13 Browser VM acceptance — preflight evidence

This is bounded evidence for `docs/platform/WORKLIST.md`, not a second
tracker. It records the move of Browser VM acceptance from the Dell seat to
the T480 bench host and the resulting preflight boundary.

## Target and immutable inputs

- Host: `T480`, LAN `172.20.146.68`, wired `enp0s31f6`; the bench Ethernet
  configuration was left unchanged.
- The operator identifies this as a fixed wired bench connection. The
  acceptance work made no NetworkManager, address, route, bridge, or firewall
  changes on `enp0s31f6`.
- Host capacity observed during the follow-up read-only check: Intel
  i5-7300U, 2 physical cores / 4 threads, with the Browser VM configured for
  4 vCPU and 8 GiB. Host CPU governors were restored to `powersave` after the
  bounded experiment.
- Domain: `browser-vm`, guest address `192.168.122.225`, RDP `3389`.
- Source commit: `af3348bcfa350c6e2ed0d4f283e3e8d7da4c9ba6`.
- Base image digest:
  `sha256:6c693432dbf23ae6ce931445dbaea9704d84e319b5e31750da85101950ad232a`.
- Base image: `/var/lib/libvirt/images/browser-vm-chromium.qcow2`; the
  destination uses a separate writable overlay and NoCloud seed.
- Seed digest:
  `0c57c25ee213455ae8f7f9e4951c6c500537d88b21def6c0ad37f759c1555bd3`.

The encrypted host-bound RDP credential was passed to the full server
wrapper. Direct invocation of the observer with the encrypted envelope was
not used for acceptance.

## Destination VM definition

The T480 domain is left with the canonical accelerated virtio-gpu boundary:

```xml
<graphics type='spice' ...>
  <gl enable='yes' rendernode='/dev/dri/renderD128'/>
</graphics>
<video>
  <model type='virtio' heads='1' primary='yes'>
    <acceleration accel3d='yes'/>
  </model>
</video>
```

The earlier T480-specific `vram` and forced `1920x1080` resolution override
were removed. QEMU starts as `virtio-vga-gl`; QGA returns, the guest address
appears, and xrdp/Xorg establish a 1920x1080 session. The 2D definition did
not produce a decoded frame; enabling virgl fixed that hard transport failure.

## Bounded preflight results

The same full `serve-browser-vm-performance` path was used for each run. The
server reached the guest and created the five controlled Chromium tabs, but
the visible-cadence contract rejected the run before the collector endpoint
was published.

| Configuration | Visible quality presented | Visible RVFC | Geometry | Result |
| --- | ---: | ---: | --- | --- |
| virgl with forced 1920x1080 mode | 12.361 FPS | 3.253 FPS | pass | rejected; floor is 27 FPS |
| canonical virgl model | 10.867 FPS | 5.796 FPS | pass | rejected; floor is 27 FPS |
| diagnostic with `mackesd` frozen | 7.663 FPS | 7.416 FPS | fail | diagnostic only; no improvement |
| AC `performance` CPU governor | 11.058 FPS | 13.269 FPS | pass | rejected; floor is 27 FPS |

The CPU governor was restored to `powersave`. The daemon-freeze run was
reversible and was not used as acceptance evidence.

## Verdict and next boundary

T480 placement is complete, but Browser VM acceptance is **not passed**. The
T480 now has a working accelerated RDP frame path, yet its five-tab Chromium
preflight remains below the release floor. A 15-minute collector run must not
be treated as meaningful until that preflight passes.

The remaining investigation is the T480 guest rendering/virgl performance
boundary, with host-side QEMU/DRM telemetry captured during a passing
preflight. No threshold was relaxed.

The current evidence is consistent with a compute/rendering limit rather than
the bench Ethernet path: QEMU and the guest produce a valid accelerated frame
stream, while the five 1080p media elements remain below the visible cadence
floor. This is not sufficient to promote the T480 as a passed acceptance host.

During recovery of the reversible daemon diagnostic, T480's mesh endpoint
`10.42.0.1:2379` was unreachable and the startup fetch for `mesh-ssh-key`
remained pending. The secret-reconciliation timer was restored to active; this
coordination-plane condition is separate from the VM's static Ethernet path
and must be resolved before claiming the bench host's control plane is
healthy.

A later read-only follow-up found `enp0s31f6` still up at `172.20.146.68/16`
with the existing default, Nebula, LAN, and libvirt routes unchanged; no
NetworkManager or interface mutation was made. The same probe currently reaches
`10.42.0.1:2379`. This recovery observation does not alter the rejected
five-tab performance result or promote Browser VM acceptance.

Evidence paths on T480 include:

- `/var/lib/mcnf-browser-vm/t480-r13-virgl-smoke-server.log`
- `/var/lib/mcnf-browser-vm/t480-r13-canonical-smoke-server.log`
- `/var/lib/mcnf-browser-vm/t480-r13-daemon-paused-smoke-server.log`
- `/var/lib/mcnf-browser-vm/-server.log` (performance-governor run)

## Warned follow-up setup retest (2026-08-04)

Immediately before this live retest, the T480 centered red
`AI-GENERATED-ALERT` warning was published and its required five-second delay
completed through `/usr/libexec/mackesd/seat-update-warning`. The exact same
installed full server wrapper and immutable source/image identity above were
used against `browser-vm` at `192.168.122.225`; no host networking or VM
definition was changed.

The wrapper reached the guest and began its real five-tab setup, but the
controlled Chromium media-tab preflight rejected the run after a 4.390-second
live cadence probe, before the performance endpoint became ready and before
any collector claimed it:

```text
Chromium did not expose five ready CDP media tabs
visible quality presented: 8.883 fps (required >=27 fps)
visible RVFC: 4.328 fps
geometry: pass
background tabs: 4 accepted with non-zero progress
```

This is diagnostic evidence only and is not a 15-minute acceptance result.
The wrapper cleaned up its RDP observer, guest-controlled Chromium session,
sidecar, and media listener; a read-only post-check found no remaining
performance wrapper, observer, listener, or performance Chromium process.
The result strengthens the existing conclusion that the release boundary is
the T480 guest/rendering capacity, not its fixed Ethernet route. Browser VM
acceptance remains rejected and no threshold was relaxed.

## Warned producer retest (2026-08-05)

The installed performance helper was refreshed after the centered red
`AI-GENERATED-ALERT` warning and its required five-second delay. The prior
helper was preserved at
`/var/lib/mcnf-browser-vm/serve-browser-vm-performance.r14-before-failure-record`;
the deployed helper hash was
`sha256:168064bf54f53644acceaf7b3420b39b1015ca7091bd8416dcfdbe1cf79bab5c`.
The T480 wired interface and VM definition were not changed.

The same immutable identity and full five-tab preflight were rerun. The visible
tab reached `12.038` quality-presented FPS and `4.093` RVFC FPS, with geometry
ready; the four background tabs reported non-zero progress. The visible tab
failed the unchanged `>=27` FPS floor, so the run exited rejected before the
collector endpoint became ready. This is still diagnostic evidence, not a
15-minute acceptance result.

Unlike the earlier empty output, the runner now records a private bounded
`diagnostic-failure` line with `acceptance_eligible=false`,
`acceptance_status=not-run`, and the unchanged `acceptance_gate_seconds=905.0`.
The artifact is `/var/lib/mcnf-browser-vm/t480-r15-preflight.ndjson`; its
companion live sidecar is empty because setup failed before sampling. No
physical speaker audibility or production audio acceptance is claimed.

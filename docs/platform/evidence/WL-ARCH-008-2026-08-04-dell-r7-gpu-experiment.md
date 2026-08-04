# WL-ARCH-008 Dell r7 GPU experiment — non-promotion evidence

This is bounded evidence for `docs/platform/WORKLIST.md`, not a second tracker.
It records a failed promotion experiment so later work does not repeat a partial
success or mistake VA-API initialization for hardware decode acceptance.

## Change and rollback boundary

The accepted r5 overlay, r6 NoCloud seed, image source
`1f8bd8450a135546b96525fbd9100f052872e78d`, and image digest
`sha256:17e3820560656d765cb318fb41f81d5dd968b390cb4e58f3ea321602f46b0556`
did not change. Every restart, test, package transaction, and rollback was
preceded by Dell's centered red `AI-GENERATED-ALERT` and enforced five-second
wait.

The inactive domain was saved before mutation:

- Known-good 2D XML SHA-256:
  `4bdafc7d56ab86558077485672ce51070e737530b6687a4b606dedf8c821f137`
- Candidate XML SHA-256:
  `09b4ea77f0ea6ecf050347487fc5e25a207cb3f15833b0b15543b078c0ea8c91`
- Rollback directory:
  `/var/lib/libvirt/images/browser-vm-r7-gpu-rollback-20260804`

The candidate changed only the domain display boundary:

```xml
<graphics type='spice' listen='127.0.0.1'>
  <gl enable='yes' rendernode='/dev/dri/renderD128'/>
</graphics>
<video>
  <model type='virtio' heads='1' primary='yes'>
    <acceleration accel3d='yes'/>
  </model>
</video>
```

Libvirt validated the XML and resolved it to `virtio-vga-gl` using Dell's Intel
HD 620 render node. QEMU 10.2.2 and virglrenderer 1.3.0 were already installed.

## What passed

The GL candidate booted with QGA ready in 32 seconds. Two exact strict RDP runs
passed without changing the test binary or credential boundary:

| Run | Result | Duration | Inbound reconnect repaint | Visual identity |
| --- | --- | ---: | ---: | ---: |
| r7 GPU | 1 passed, 0 failed | 38.01 s | 79.6% | 99.9% |
| r7 GPU after host-driver probe | 1 passed, 0 failed | 37.36 s | 79.6% | 99.9% |

Both runs rendered the same 1920x1080 Chromium workload, passed the reversible
pointer/Escape menu challenge, and returned a 1,161-color reconnect frame. The
raw credential-free log hashes are
`ada056520b2bbbb06169c642a98c3534bc4708056b6c9d994a901c6a09b629a2`
and `4f4788da8523e25169dc163b71bc38e98a03edc7fade7e94fca3c62c04f5068f`.

In the actual Browser user session, `vainfo` changed from initialization failure
to exit-zero initialization:

```text
Driver version: Mesa Gallium driver 26.1.5 for virgl
  (Mesa Intel(R) HD Graphics 620 (KBL GT2))
VAProfileNone: VAEntrypointVideoProc
```

The bounded runtime record therefore reported `gpu_status=passed`, while audio
wiring and fixed-fixture Chromium decode continued to validate. Those records
remain private on Dell under
`/var/lib/libvirt/images/browser-vm-r7-gpu-session-evidence-20260804` and
`browser-vm-r7-gpu2-session-evidence-20260804`.

## Why it did not promote

The guest exposed no H.264, VP8, VP9, HEVC, or AV1 VA decode profile—only
`VAProfileNone/VideoProc`. Chromium also intermittently logged
`GpuControl.CreateCommandBuffer` transient failure. Thus `vainfo` initialization
passed, but WL-ARCH-008 acceptance criterion 7 still did not.

Dell's virglrenderer is linked with its VA backend, but its installed binary
contains and enforces `only supports mesa va drivers now`. Dell's hardware decode
driver is Intel iHD, not a Mesa Gallium VA driver. Installing Fedora's
`libva-intel-media-driver` and restarting QEMU did not add guest decode profiles;
the package and its newly installed `intel-gmmlib` dependency were then removed.
The relevant upstream implementation initializes host VA-API and explicitly
rejects non-Mesa driver vendor strings:
[virglrenderer video source](https://android.googlesource.com/platform/external/virglrenderer/+/1cde1fc0a7e1ee9ee03eeac4eb7af330bb53742b/src/virgl_video.c).

After a later GL restart, QGA remained disconnected beyond 90 seconds while
QEMU consumed sustained multi-core CPU. A display path that passes twice but
does not boot repeatedly is not releasable. The persistent domain had already
been restored to the saved 2D XML; the active stalled instance was destroyed and
restarted from that known-good definition.

## Final state

Dell now runs the original `virtio-vga` configuration with SPICE GL disabled and
`accel3d=no`. QGA returned in 31 seconds, guest IP `192.168.122.58` reappeared,
and RDP port 3389 is ready. The GL candidate and extracted diagnostics remain
available for analysis, but no speculative package or live GPU configuration
remains installed.

The viable next production choices are a GPU host whose Mesa Gallium VA driver
is supported by virgl video forwarding, a proven hardware-mediated GPU boundary,
or placement of `browser-vm` on such a mesh host while Dell remains the RDP seat.
QEMU documents virgl, host-blob, Venus, and DRM-native contexts separately and
describes accelerated virtio-gpu as evolving:
[QEMU virtio-gpu documentation](https://www.qemu.org/docs/master/system/devices/virtio/virtio-gpu.html).
Dell's QEMU 10.2 device capabilities expose `blob`, `hostmem`, and `venus`, but
not `drm_native_context`; none of those available switches converts Intel iHD
into the Mesa VA backend required by this virgl video implementation.

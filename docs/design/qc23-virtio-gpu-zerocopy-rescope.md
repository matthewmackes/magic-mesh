# QC-23 re-scope — the QEMU virtio-gpu→egui zero-copy fast path

**Status:** DESIGN — options + recommendation, 2026-07-10. Written against QC-23's
own honest "Still open" admission (`docs/WORKLIST.md`): the landed QC-23 progress
(Nova/libvirt SPICE preference, Glance virtio-video image properties, live SPICE
proof harnesses) is real, tested infrastructure — but it is entirely SPICE/console
work, and has not touched "the live dmabuf/virgl/venus importer" the acceptance
criteria still require. **Not implemented here — design/options only**, per the
task's explicit scope.
**Companions:** `docs/design/quasar-vdi-desktop.md` (lock 12, "virtio-gpu zero-copy
(dmabuf → wgpu texture)" — the original, and, per §1 below, now stale, source of
this feature), `docs/design/quasar-cloud.md` (Q34, "SPICE ... AS THE console
experience" — the lock QC-23's actual landed work advances instead), 
`docs/design/e12-9-10-libvirt-rescope.md` (the sibling re-scope this doc's format
and farm-testability method follow; its VFIO finding is the direct comparison
point for §4), `docs/design/mesh-media-player.md` +
`crates/shared/mde-egui/src/video_plane.rs` (MEDIA-2 — the closest real precedent
this codebase has for a hardware-gated dmabuf-adjacent import seam, reused
directly below).

## Summary (for the impatient)

- The current path is **not zero-copy and was never claimed to be**. It is
  `mde-vdi-spice` decoding SPICE's wire protocol into an `egui::ColorImage`
  through at least three full-framebuffer CPU copies, uploaded to the GPU on a
  hard 50 ms poll cycle regardless of guest frame rate. This is real, working,
  tested infrastructure — just not the feature this WORKLIST item's remaining
  bullets ask for. See §1.
- A true zero-copy path is **only coherent for the local, same-host Workstation
  case** (QC-23's own "As a Workstation user" framing). SPICE is a wire protocol —
  no amount of SPICE hardening can ever become "zero-copy" — and QUASAR-CLOUD's
  Q34 already locked SPICE as the answer for the *remote*/Nova-brokered console
  case. QC-23's landed "Progress" notes advanced Q34, not lock 12. See §2.
- This codebase's **production DRM renderer is `egui_glow` over EGL/GLES2, not
  wgpu** — verified against `crates/shared/mde-egui/Cargo.toml` and `src/drm.rs`.
  wgpu is used only by the windowed *dev* runner. Lock 12's "dmabuf → wgpu
  texture" wording (and its own R1 risk's "wgpu/GBM path") predates or was never
  reconciled with that implementation choice. Any real design has to target
  EGL/GLES externally-imported-texture APIs, not wgpu. See §1.3.
- The hard, expensive, currently-unsolved part is **getting a dmabuf handle out of
  QEMU's process at all** — a vhost-user-gpu backend or QEMU's `-display
  dbus,gl=on`. Both are substantial new infrastructure; the natural off-the-shelf
  pure-Rust option (`vhost-device-gpu`) does not yet support dmabuf display
  sharing upstream (verified via web search, §3.3). The shell-side import, once a
  handle exists, is comparatively cheap — this codebase already has nearly every
  primitive it needs (`prime_fd_to_buffer` / `add_planar_framebuffer` in the
  already-pinned `drm` crate; MEDIA-2's plane-scanout trait seam), verified below
  in §3.5.
- Unlike the E12-10 VFIO finding, this is **not IOMMU/dedicated-hardware-gated** —
  virgl/venus don't need device passthrough. But it needs a physical DRM seat
  (the existing, already-honest `drm` feature constraint) that can *also* host a
  local libvirt VM with a working GPU-accelerated render node — a combination
  this project has no inventory record of having tried. See §4.
- **Recommended first slice:** two small, honestly-scoped, low-risk pieces that
  make real progress without betting on the hard unsolved half. The live
  dmabuf/virgl/venus importer itself is not a "first slice" — it is gated on an
  infrastructure decision this doc surfaces but does not make. See §5.

---

## 1. Current architecture: what actually happens today, and what it actually costs

### 1.1 The SPICE frame path is real, and it is a CPU-copy pipeline

`crates/desktop/mde-vdi-spice` is a genuine, first-class SPICE client (`src/lib.rs`
doc: "the pure-Rust `spice-client` stack... is a first-class SPICE client, not the
fallback"), and it is what actually renders a QEMU/libvirt console today via
`mde-shell-egui/src/vdi.rs`'s `Session::Spice` variant. Tracing one frame from wire
to screen, verified directly against the source:

1. **`spice-client` (pinned `0.2.0`) decodes** the wire image (raw/LZ/GLZ/QUIC per
   its channel implementation) into a `DisplaySurface { width, height, format,
   data: Vec<u8> }` — already a CPU-side decode this crate doesn't control.
2. **`SpiceTransport::pump_frame`** (`crates/desktop/mde-vdi-spice/src/connect.rs:123-133`)
   hands that whole surface to `SpiceSession::apply_surface`.
3. **`Framebuffer::apply_surface`** (`src/pixel.rs:173-206`) copies it byte-for-byte
   (plus a BGRA/BGRX→RGBA swizzle when needed) into a second, persistent `Vec<u8>`.
   Its own doc comment is explicit about the delivery shape: *"the surface
   `spice-client` decodes is already the whole primary framebuffer, so there is no
   sub-rectangle blit to accumulate"* — i.e. **every SPICE update re-sends and
   re-decodes the entire desktop**, not a damage rectangle, unlike this same
   crate family's own VNC client (`pixel.rs`'s module doc names this explicitly:
   VNC accumulates "RFB-style sub-rectangles"; SPICE here does not).
4. **`Framebuffer::to_color_image`** (`src/pixel.rs:213-215`) builds an
   `egui::ColorImage` via `ColorImage::from_rgba_unmultiplied` — a third
   full-buffer pass.
5. **The shell** (`crates/desktop/mde-shell-egui/src/vdi.rs:1073-1082`) uploads that
   `ColorImage` with `ui.ctx().load_texture(...)` (first frame) or
   `handle.set(img, DESKTOP_TEX)` (every frame after) on a `TextureHandle`. Under
   the DRM production backend this stages an `egui_glow` `TexturesDelta` that
   `Painter::paint_and_update_textures` later uploads via `glTexSubImage2D` — a
   normal CPU→GPU DMA upload, but it can only start once steps 1-4 have finished
   assembling the CPU-side buffer.

So the current path is **at least three full-framebuffer CPU copies/repacks**
(decode buffer → `Framebuffer` → `ColorImage`) before the GPU ever sees the pixels,
on top of the SPICE-level image-compression CPU cost on both the QEMU server side
and the client decode side, on top of the fact that the whole surface is resent on
*any* change rather than a damage rectangle. "Zero-copy" is aimed at the first
part of that list (the CPU copies); it does not by itself fix the second
(whole-surface updates), because that's inherent to using SPICE as the transport
at all — see §2.

### 1.2 There is also a hard, unrelated 50 ms poll ceiling

`BlockingSpiceTransport::pump_frame` (`crates/desktop/mde-vdi-spice/src/connect.rs:227-232`)
sleeps 50 ms **before every single pump**, not just the first:

```rust
pub fn pump_frame(&mut self, session: &mut SpiceSession) -> Result<bool, ConnectError> {
    self.runtime.block_on(async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.transport.pump_frame(session).await
    })
}
```

The shell's `run_live_spice` worker loop (`mde-shell-egui/src/vdi.rs:708-740`) calls
this in a tight `loop`, so **the effective frame ceiling is ~20 Hz regardless of
guest activity or host GPU speed**. This is a poll-cadence/event-loop shape issue,
unrelated to zero-copy graphics — worth naming because it's a real, separately
fixable latency cost that a dmabuf importer would not touch at all (an imported
scanout still needs *something* to signal "a new frame is ready," and today that
something is a fixed 50 ms sleep, not a wakeup).

### 1.3 The production renderer is `egui_glow`/EGL/GLES2 — not wgpu

This is the load-bearing correction for everything that follows. `quasar-vdi-desktop.md`
lock 12 says **"virtio-gpu zero-copy (dmabuf → wgpu texture)"**, and its own R1 risk
entry says the DRM backend is "the smithay DRM/GBM + libinput + **wgpu**/GBM path."
Neither matches what actually shipped. Verified against
`crates/shared/mde-egui/Cargo.toml` and `src/drm.rs`:

```toml
# Cargo.toml — the DEV/windowed runner uses wgpu:
eframe = { version = "0.31", default-features = false, features = ["wgpu", "wayland", "default_fonts"] }
# the PRODUCTION DRM/KMS bare-seat backend pulls a completely different stack:
drm = ["dep:drm", "dep:gbm", "dep:khronos-egl", "dep:glow", "dep:egui_glow", "dep:input", "dep:udev"]
```

`src/drm.rs`'s own module doc says it outright: *"The render path is **GL** — EGL
on a GBM scanout surface, painted by `egui_glow` — rather than wgpu, because that
is the reliable bare-KMS path."* The seat is brought up with raw `drm`/`gbm`/
`khronos-egl` crates directly (no `smithay` dependency either — R1's "smithay
DRM/GBM" framing didn't ship as written; `Cargo.lock` has no `smithay` core crate,
only the unrelated `smithay-client-toolkit`/`smithay-clipboard` Wayland-client
helpers used elsewhere). Pinned versions (`Cargo.lock`): `drm 0.14.1`, `gbm 0.18.0`,
`khronos-egl 6.0.0`, `glow 0.16.0`, `egui_glow 0.31.1`.

**Consequence:** any real zero-copy import must target EGL/GLES2 externally-created-texture
APIs (`EGL_EXT_image_dma_buf_import` → `eglCreateImage` → `GL_OES_EGL_image_external`),
not a wgpu-shaped Vulkan/hal dmabuf-interop path. It also means zero-copy work is
inherently scoped to `feature = "drm"` builds only — the windowed `eframe`+wgpu dev
runner has no code path for this at all and would keep using the CPU-copy fallback,
mirroring exactly how MEDIA-2's video plane already degrades to a texture fallback
off a real seat (`video_plane.rs`'s `VideoPath::texture_no_drm`).

This drift is not unique to lock 12 — the same stale "cloud-hypervisor... virtio-gpu
zero-copy (dmabuf→wgpu)" framing also still appears verbatim in the now-superseded
`docs/WORKLIST.md` **E12-7** entry (still marked `[>]`, still describing
`crates/services/mde-kvm`, which QC-15 deleted outright — `find` over the current
tree returns nothing for `mde-kvm`) and in `docs/design/whitepaper-brief.md`. E12-7's
own text even says "**REMAINING (integration-gated):** live VM boot (KVM +
cloud-hypervisor + golden image) + the virtio-gpu render into the egui texture" —
work against a hypervisor and a crate that no longer exist. This doc does not edit
E12-7 (out of the scope it was asked to touch) but flags it: QC-1's own blocker note
already says *"QC-23 owns the real QEMU display implementation and live
validation,"* so E12-7 reads as a zombie entry a future WORKLIST reconciliation pass
should mark superseded-by-QC-23, the same way `docs/NEEDS-OPERATOR.md`'s E12-9 line
already got a "Corrected 2026-07-10" note for the same QC-15 deletion.

### 1.4 Neither VM-creation path configures any GPU acceleration today

Both of this codebase's domain-authoring paths were checked directly (mirroring
`e12-9-10-libvirt-rescope.md`'s "two independent VM-creation code paths" finding,
which applies here unchanged):

- `vm_lifecycle.rs::build_domain_xml` (`crates/mesh/mackesd/src/workers/vm_lifecycle.rs:596-645`)
  emits `<graphics type='spice' autoport='yes'><listen type='address' address='127.0.0.1'/></graphics>`
  and `<video><model type='virtio'/></video>` — virtio-gpu the *device model*, but
  **no `<acceleration accel3d='yes'/>`** and no blob/hostmem/venus configuration
  anywhere.
- `compute_provision.rs::build_virt_install_args` (`crates/mesh/mackesd/src/workers/compute_provision.rs:334-380`)
  is even less explicit: `--graphics spice` with **no `--video` flag at all**,
  leaving `virt-install`/libvirt to pick its own default video model.

The QC-23 "Progress" notes' Glance `hw_video_model=virtio` property
(`crates/mesh/mackesd/src/workers/openstack/image_pipeline.rs:270-277`, test-verified)
only steers Nova away from a cirrus/qxl default toward the virtio-gpu *2D* device —
QXL is SPICE's own legacy 2D device model, an unrelated lineage with no 3D or
dmabuf-export capability. None of the landed work turns on 3D acceleration or
changes the delivery mechanism away from SPICE. The image_pipeline.rs test's own
comment already says this honestly: *"The live dmabuf importer is still
separate."*

---

## 2. What "zero-copy" would actually mean here, and why the landed work is orthogonal to it

**SPICE cannot become zero-copy.** It is a remote-desktop wire protocol —
encode→transmit→decode is the whole point of it, including for `localhost`
connections. A "zero-copy SPICE path" is not a smaller version of this feature; it
is a category error. True dmabuf zero-copy means the shell process imports a
*local kernel buffer handle* that QEMU's process (or a helper it delegates to)
exported — this **only works when the shell and the QEMU process are on the same
host**, because dmabuf fds are host-local kernel objects, not something you route
over a network socket (even a loopback one — the `spice-client`/QEMU SPICE server
pairing in this codebase talks TCP, not fd-passing).

That maps directly onto two decisions this project already locked, in two
different documents, that point in different directions:

| | `quasar-vdi-desktop.md` lock 12 (2026-06-30) | `quasar-cloud.md` Q34 (2026-07-03) |
|---|---|---|
| Scope | **Local** KVM, "Workstation-local VMs" (Round 2's own heading) | **Remote**/Nova-brokered cloud instances |
| Decision | virtio-gpu zero-copy fast path | SPICE + `mde-vdi-spice` **as THE console experience** |
| Status | Not started (this doc's subject) | Landed (QC-13/QC-23's real "Progress" notes) |

QC-23's own header line is unambiguous about which of these two it is: *"**As** a
Workstation user, **I want** the local QEMU/libvirt display fast path to deliver
the guest virtio-gpu framebuffer directly into the egui Desktop surface."* That's
lock 12's population, not Q34's. But every "Progress" entry QC-23 has actually
accumulated — Nova preferring a native SPICE descriptor over the HTML5 proxy, the
Glance `hw_video_model` property, the live SPICE checksum/input-echo proof
harnesses — is Q34 work: it makes the *remote* console path (which was always
going to be SPICE, wire-protocol, CPU-copy, per Q34) more honest and better-tested.
That's real, valuable, already-shipped work. It just doesn't advance the "Still
open" bullet at all, because it was never going to — no amount of SPICE hardening
produces a dmabuf import. This isn't a criticism of the landed work; it's the
reason the "Still open" line hasn't moved despite five separate "Progress" slices
landing. A future QC-23 pass should not mistake "more SPICE work" for progress on
this bullet again.

One more scope note in the same vein: `quasar-cloud.md` Q37 ("Nova PCI + vGPU
flavors model GPU/device passthrough") is a **different mechanism** from anything
in this doc — Nova/Placement-scheduled SR-IOV/mediated-device GPU *slices* for
dedicated guest access, orthogonal to virtio-gpu's shared paravirtualized display
(lock 13 already frames these as alternatives: *"shared virtio-gpu (virgl/venus)
**or** dGPU passthrough (VFIO)"*). Don't conflate Q37 progress with QC-23 progress
if it ever lands; they solve different problems for different VM populations.

---

## 3. What a real dmabuf/virgl/venus path requires, end to end

Split into the axes that are commonly conflated: **3D acceleration** (does the
*guest* get to use the *host* GPU for rendering) and **zero-copy display delivery**
(does the *composited output*, however it was rendered, reach the client without a
CPU copy). These are separable — a domain can have one without the other, which
matters for §5's recommendation.

### 3.1 Guest side

Linux guests: mainline since Linux 4.16-ish, actively maintained — the guest's
Mesa driver (`virgl`/`venus`) redirects GL/Vulkan calls over virtio-gpu to the
host. This project's own images are Linux (the QC-1 bootc image is Fedora-based;
`STANDARD_IMAGES[0]` in `image_pipeline.rs` builds `mcnf-base.qcow2`), so guest
driver maturity is not a real risk for the images this project actually ships.

Windows guests are a different story, and this project has a concrete, existing
gap here worth naming: `quasar-vdi-desktop.md` lock 15 calls for a "Golden Windows
image pre-tooled: virtio drivers (net/disk/**gpu**)... " but per
`docs/WORKLIST.md` **TESTVM-3**, *"no Windows ISO on either dom0"* — the actual
test beds fell back to Alpine+xrdp. So Windows-side virtio-gpu/venus guest-driver
validation (already a thinner ecosystem than Linux's) is unreachable in this
environment for a second, independent reason: there is no Windows image to test it
on at all, regardless of GPU questions.

### 3.2 QEMU device / host driver requirements

Per QEMU's own docs for the exact pinned version (QC-1 evidence: QEMU 9.2.4) —
[qemu.readthedocs.io/en/v9.2.4/system/devices/virtio-gpu.html](https://qemu.readthedocs.io/en/v9.2.4/system/devices/virtio-gpu.html):

- **virgl** (OpenGL passthrough): `-device virtio-gpu-gl` — QEMU built with
  `--enable-opengl --enable-virglrenderer`, host-side `virglrenderer` library.
  Works against basically any Mesa-supported GPU (Intel/AMD/software `llvmpipe`).
- **venus** (Vulkan passthrough): needs host **blob** resource support
  (`hostmem=<size>`, `blob=true` fields) plus the `venus` field enabled; supported
  since `virglrenderer` v1.0.0 (per
  [docs.mesa3d.org/drivers/venus.html](https://docs.mesa3d.org/drivers/venus.html)).
  Needs a host Vulkan driver with specific features (ANV/RADV are reasonable;
  NVIDIA's proprietary driver support for this is historically much weaker) — this
  project's own verified hardware fact is Eagle's **Intel iGPU** (`drm.rs`'s own
  comment: *"seen live on Eagle Intel iGPU"*, `src/drm.rs:655`), which is a
  reasonable venus/virgl target in principle, not a confirmed one.
- **Fedora version match:** Fedora 42 (this project's target, per QC-1) ships
  `virglrenderer-1.1.0`
  ([packages.fedoraproject.org](https://packages.fedoraproject.org/pkgs/virglrenderer/virglrenderer/)),
  which is ≥ 1.0.0 — venus protocol support is plausible but **not confirmed**;
  the package page doesn't state whether Fedora's build enables the `venus` meson
  flag, and this doc has no way to check the actual spec file or run `qemu-system-x86_64
  -device virtio-gpu-gl,help` against the real farm image. **Unverified — flag,
  don't assume**, exactly the posture the QC-1 evidence text itself models for
  other claims.
- 3D acceleration (`accel3d='yes'`) is **independent of the delivery mechanism**:
  a domain can have `<graphics type='spice'>` (today's setup) *and*
  `<acceleration accel3d='yes'>` on its virtio-gpu-gl video device at the same
  time — SPICE's server-side display channel can itself consume a virgl-rendered
  framebuffer and encode+transmit it exactly as it does today. This is the
  ordinary "3D acceleration" checkbox behavior in tools like virt-manager/GNOME
  Boxes. Turning this on does **not** make the path zero-copy (delivery still goes
  through the same SPICE CPU-copy pipeline in §1.1) but is a real, independent,
  cheap guest-3D-performance improvement — see §5's Tier 0.

### 3.3 The hard part: getting a dmabuf handle out of QEMU

This is the part neither this project's old cloud-hypervisor code nor its new
QEMU/libvirt code has ever actually done, and it's the actual bottleneck for the
whole feature — everything in §3.4/§3.5 is waiting on a handle this step has to
produce. Two real mechanisms:

**(a) `vhost-user-gpu`** — QEMU delegates virtio-gpu device emulation to an
external backend process over a Unix domain socket; the backend is what actually
owns dmabuf export, sharing fds to *its own* display client via `SCM_RIGHTS`
ancillary data (verified against
[qemu.org's protocol doc](https://www.qemu.org/docs/master/interop/vhost-user-gpu.html):
*"aiming at sharing the rendering result of a virtio-gpu... over a UNIX domain
stream socket, since it uses socket ancillary data to share opened file
descriptors (DMABUF fds or shared memory)"*). This is architecturally what the
now-deleted `mde-kvm` did for cloud-hypervisor (QC-1's blocker note:
`VmSpec::virtio_gpu` → `build_ch_config` `gpu`, a vhost-user-gpu socket) — but
that code was written against cloud-hypervisor's specific process/API shape and
does not transfer to QEMU. The natural rust-vmm-ecosystem building block for a
QEMU-facing equivalent, `vhost-device-gpu`, **explicitly does not yet support
zero-copy dmabuf display sharing upstream** — per its own doc, cited verbatim from
web search results: *"directly sharing display output resources using dmabuf is
**not yet supported** in the vhost-device-gpu implementation."* Adopting or writing
a vhost-user-gpu backend is therefore substantial, currently-unfinished-in-the-ecosystem
new infrastructure — comparable in kind to the `e12-9-10-libvirt-rescope.md`
finding that `spice-client` lacks an audio channel (a real, thin-dependency wall),
just one level further from done.

**(b) QEMU's built-in `-display dbus,gl=on`** — no separate backend process; QEMU
itself exposes a D-Bus interface (`org.qemu.Display1.Listener`) that hands out
dmabuf fds to any D-Bus peer implementing it. This is the more modern, more
idiomatic-to-libvirt mechanism (`<graphics type='dbus'>` is a real libvirt graphics
type), and avoids standing up a whole extra process. It needs: QEMU built with
D-Bus display support (an additional, **unverified** compile-time flag on top of
the `--enable-virglrenderer` question in §3.2), and — the real cost — this
project's shell/mackesd side would need to become a **new D-Bus client**
implementing that specific listener interface (a new IPC dependency, e.g. `zbus`,
plus real protocol client work), which nothing in this codebase does today.

Either path is a genuine new-infrastructure decision, not a config flag. This doc
does not recommend one over the other — that's exactly the kind of call
`e12-9-10-libvirt-rescope.md` flagged rather than silently resolved for its own
open questions, and the right analogy here: **don't sink real effort into either
until this specific choice is made**, ideally after confirming the QEMU build's
actual compiled-in feature set on a real box (§3.2's open item).

### 3.4 libvirt XML (illustrative — not verified against this project's exact schema/version)

Mirroring `e12-9-10-libvirt-rescope.md`'s own hedge pattern for its `<audio>`
addition, this is the general shape per libvirt's domain-XML conventions (nested
capability sub-elements, matching how this same libvirt version already nests
`<hostdev><source><address>` and would nest `<sound><audio>` per the companion
doc) — **verify against `virsh domcapabilities` on a real target before
implementing**, not copy-pasted from a fetched, possibly-imprecise summary:

```xml
<!-- illustrative addition to vm_lifecycle.rs::build_domain_xml's <devices> block -->
<video>
  <model type='virtio' heads='1'>
    <acceleration accel3d='yes'/>
  </model>
</video>
<!-- ONLY if pursuing the zero-copy delivery mechanism (§3.3), which REPLACES
     the existing <graphics type='spice'> stanza rather than adding to it: -->
<graphics type='egl-headless'>
  <gl rendernode='/dev/dri/renderD128'/>
</graphics>
<!-- or, for the dbus-display mechanism: -->
<graphics type='dbus'>
  <gl enable='yes' rendernode='/dev/dri/renderD128'/>
</graphics>
```

The `<acceleration accel3d='yes'/>` addition is compatible with the *existing*
`<graphics type='spice'>` stanza (§3.2) — only the zero-copy-delivery graphics
types are mutually exclusive with SPICE on the same head.

### 3.5 Shell-side import: two options, concretely compared against this codebase

Once a dmabuf fd (+ width/height/fourcc format/stride/offset/modifier metadata)
is available in the shell process by whichever §3.3 mechanism, two genuinely
different ways exist to get it on screen. Both were checked against this
project's actual pinned dependencies, not assumed generically:

| | **Option A — GL texture import** | **Option B — KMS plane import** |
|---|---|---|
| Mechanism | `eglCreateImage(..., EGL_LINUX_DMA_BUF_EXT, ...)` → `glEGLImageTargetTexture2DOES` → bind as a `GL_TEXTURE_EXTERNAL_OES` texture | `prime_fd_to_buffer` (fd→GEM handle) → `add_planar_framebuffer` (GEM handle→KMS FB) → `set_plane` |
| Paints via | `egui_glow::Painter::register_native_texture(glow::Texture) -> egui::TextureId`, then the *exact* `SizedTexture`/paint call `vdi.rs:1092` already uses | A hardware overlay/primary plane, composited by the display controller — bypasses the GL/egui render pass for the VM content entirely |
| New crate deps | None beyond what's pinned, but needs **new `unsafe` FFI**: `glow 0.16.0` has zero bindings for `egl_image`/`EGLImage`/`image_target_texture` (grep-verified against the crate source) — the function pointer must be hand-loaded via `egl.get_proc_address` (the same mechanism `drm.rs` already uses to build its `glow::Context`, `src/drm.rs:753-758`) and called through a manually-written `extern "C"` signature | **None at all** — `drm 0.14.1`'s `control::Device` trait already has `prime_fd_to_buffer` (`src/control/mod.rs:785`) and `add_planar_framebuffer` (`src/control/mod.rs:348`, taking a `B: buffer::PlanarBuffer` with `size/format/modifier/pitches/handles/offsets` — `src/buffer/mod.rs:107-120`); `add_framebuffer` is already called live in `drm.rs`'s own render loop (`src/drm.rs:1124-1125`) |
| EGL version | `khronos-egl 6.0.0`'s safe `create_image`/`destroy_image` wrapper only exists on `Instance<T: api::EGL1_5>` (`src/lib.rs`, `mod egl1_5`) — current code loads `DynamicInstance::<egl::EGL1_4>` (`src/drm.rs:659`), so this needs a version bump. (Mesa has shipped EGL 1.5 for a decade — low risk, but a real, verified delta from current code.) `EGL_NO_CONTEXT` is constructible via the crate's `unsafe fn Context::from_ptr` + its public `NO_CONTEXT` constant, so the API shape is usable once bumped. | N/A — no EGL involvement at all for this path |
| Shader risk | `egui_glow`'s built-in shader samples plain `sampler2D`, not `samplerExternalOES` — whether this matters depends on the imported pixel format. A simple composited XRGB/ARGB desktop framebuffer can often import straight as ordinary `GL_TEXTURE_2D` (dodging the problem); YUV-ish formats typically cannot and would need a shader fork (comparable in kind to the SPICE-audio-channel "thin dependency wall" the companion doc found) | N/A |
| Existing precedent in this codebase | None | **MEDIA-2** (`crates/shared/mde-egui/src/video_plane.rs` + `src/drm.rs:1167-1392`) already does exactly this shape of thing for mpv video: `PlaneSet`/`VideoPath`/`VideoScanout` (pure, unit-tested against a `FakeCatalog`) + `DrmVideoScanout` (live, hardware-gated) whose `Frame` type is literally `drm::control::framebuffer::Handle`, fed via `set_plane`. Its own doc comment already anticipates a dmabuf producer: *"Its `Frame` is a real `drm` framebuffer handle — the mpv render API imports the decoded frame as a dmabuf/KMS framebuffer (MEDIA-3/4/8) and hands the handle here."* |
| Matches literal QC-23 wording | Yes — the acceptance bullet says "imports that framebuffer into the **existing Desktop texture path**" | No — would need that bullet's wording corrected to describe a plane-scanout path (in scope for this doc's WORKLIST update, §6) |
| Fits lock 1 ("fullscreen, one desktop at a time") | Adequate — one more textured quad in the normal render pass | Arguably a better fit — a dedicated hardware plane for a genuinely fullscreen desktop is closer to true zero-GPU-compositing-overhead than a texture draw |

**Reading of the comparison:** Option B is mechanically cheaper (zero new
dependencies, reuses an already-shipped, already-tested trait seam almost
verbatim) and lower-risk (no new unsafe FFI, no EGL version bump, no shader
uncertainty) than Option A, even though Option A is what the current acceptance
bullet's prose literally describes. This is exactly the kind of gap this doc was
asked to make precise rather than leave as a one-line aspiration.

---

## 4. Testability in this project's actual environment

The direct comparison point is `e12-9-10-libvirt-rescope.md`'s VFIO finding
("Farm build-VM slots run atop XCP-ng/Xen dom0s... nested VFIO... is not
realistically testable there... I could not find an inventory record confirming a
second GPU or a checked IOMMU/VT-d BIOS state on any live physical or farm
machine"). The honest answer here is **similar in conclusion but different in
kind** — worth being precise about the difference rather than pattern-matching to
"also hardware-gated, stop":

- **virgl/venus do not need IOMMU or device passthrough.** They work by having the
  *host's* GPU driver do host-side rendering on the *host's* behalf, indirectly —
  no `vfio-pci` binding, no dedicated/exclusive device hand-off to the guest, and
  virgl in particular can even fall back to software (`llvmpipe`) as its host-side
  GL implementation. This is a materially easier hardware bar than VFIO's.
- **But the farm's build/test VMs are excluded for an entirely separate, existing
  reason, independent of virtio-gpu:** `src/drm.rs`'s own doc comment already
  states *"The farm can only **compile** this path (no DRM master headless); the
  live render + input on a real seat is the hardware-gated `/preview`"* — this is
  the whole `feature = "drm"` backend's standing constraint (matches
  `AI_GOVERNANCE.md` §7: *"`/preview` stays optional/best-effort"*), not something
  new this feature introduces. No farm build VM can ever live-run *any* part of
  the DRM shell, virtio-gpu or otherwise.
- **So the only place this could ever be live-verified is a physical DRM seat**
  (Eagle, `.138`, `.2` per the physical-workstation inventory). That's necessary
  but not sufficient: proving the *full* QC-23 acceptance loop needs one of those
  boxes to simultaneously (a) own the DRM master for the egui shell, (b) run
  libvirt/QEMU **locally** with a working GPU-accelerated render node available to
  the QEMU process for virgl, and (c) have whichever §3.3 delivery mechanism
  wired up. This project's memory/docs confirm (a) is real for Eagle (Intel
  iGPU, DRM-capable) but there's no inventory record of (b) or (c) having been
  attempted anywhere. That's the honest gap — not "no GPU exists" (one does, on
  at least one real seat), but "the combination this feature needs has never been
  assembled or confirmed," and assembling it is gated on the §3.3 infrastructure
  decision first.
- A useful, narrower, **actually reachable** sub-question: the shell-side import
  primitives in Option B (`prime_fd_to_buffer`/`add_planar_framebuffer`/`set_plane`)
  can be liveness-checked on a real seat using a **self-produced** dmabuf (the
  code already allocates and exports its own GBM buffers for its own EGL scanout
  surface — round-tripping one through `buffer_to_prime_fd` → `prime_fd_to_buffer`
  is a self-contained test of the import mechanism, entirely independent of
  solving §3.3). This is exactly the shape of liveness check `probe_primary_video_plane`
  already does for MEDIA-2 (`src/drm.rs:1368-1392`): it drives a real `clear()`
  against a chosen plane with no actual mpv frame, proving the plane path
  end-to-end "short of a decoded frame." See §5.

---

## 5. Recommendation

No part of "the live dmabuf/virgl/venus importer" is a safe, honestly-scoped
**first slice** in the sense the rest of this project's small, one-PR
recommendations usually mean — the bottleneck (§3.3) is a real infrastructure
decision with two substantial candidate designs, neither of which this doc can
respons­ibly pick blind, and the live-proof half is gated on an unconfirmed
hardware/software combination (§4). Recommending "just wire up vhost-user-gpu" or
"just add the D-Bus client" as a first PR would repeat the overclaim pattern
QC-1's blocker note already had to correct once (the "CORES LANDED" claim on the
deleted cloud-hypervisor code).

That said, two genuinely small, real, low-risk pieces of progress exist and don't
require betting on the §3.3 decision:

**Tier 0 — turn on `accel3d='yes'` (cheap, real, independent of everything else).**
Add `<acceleration accel3d='yes'/>` to the `<video><model type='virtio'>` stanza in
`vm_lifecycle.rs::build_domain_xml`, and the `virt-install` equivalent in
`compute_provision.rs::build_virt_install_args` (which today has no `--video` flag
at all — needs one added regardless, per §1.4). This is pure XML/argv string work,
the same shape as every other builder addition in these files, works with the
*existing* `<graphics type='spice'>` stanza unchanged (§3.2), and is a real
guest-side 3D performance improvement on its own — a legitimate down payment on
lock 13's "shared virtio-gpu (virgl/venus)" half. **Open item before calling it
done:** confirm Fedora's actual QEMU package is built with
`--enable-virglrenderer` (§3.2's unverified flag) — cheap to check on a real box,
not assumed here. This does **not** touch the "Still open" bullet (it changes
nothing about *delivery*, still SPICE, still the CPU-copy path in §1.1) — it's
useful, honest, adjacent progress, not the feature.

**Tier 1 — a self-contained PRIME-import liveness check (small, real, decoupled
from §3.3).** Build the Option B (§3.5) import primitives — a small
`PlanarBuffer`-implementing wrapper plus `prime_fd_to_buffer`/`add_planar_framebuffer`
glue — as a pure, unit-testable module reusing `crate::video_plane`'s existing
trait shape, wired into a hardware-gated live check the same way
`probe_primary_video_plane` proves MEDIA-2's plane path "short of a decoded
frame" (§4's last bullet): round-trip a **locally GBM-allocated** buffer through
`buffer_to_prime_fd` → `prime_fd_to_buffer` → `add_planar_framebuffer` → `set_plane`/`clear`,
with no QEMU involvement at all. This proves the shell-side import mechanism
actually works on real hardware (a genuine unknown today — none of this has been
exercised) while staying entirely decoupled from the unresolved §3.3 question.
Per `AI_GOVERNANCE.md` §7 ("no `pub mod` with zero external refs"), this needs a
real caller chain to a compiled, hardware-gated surface (an example binary in the
`hello_video_plane` mold) to count as done, not an orphaned module — same
discipline MEDIA-2 already modeled.

**Not recommended as a first slice:** anything touching §3.3 (a vhost-user-gpu
backend or a new D-Bus display client) until that choice is made deliberately —
it's an infrastructure decision, not an engineering default, and the natural
off-the-shelf pure-Rust option isn't ready yet regardless (§3.3(a)). Also not
recommended: any further SPICE-path work under the QC-23 banner (§2) — it's real,
but it's Q34's territory and doesn't move this bullet.

If GPU-passthrough-adjacent proof matters enough to prioritize, the actual next
step is the same shape as the VFIO finding's: a decision (which §3.3 mechanism,
made deliberately, likely after a spike against a real QEMU build to answer
§3.2's open flag question) plus a confirmed hardware combination (§4's "assembled
and confirmed" gap on a physical seat) — not more speculative code against an
unconfirmed target.

---

## Rollup: what changes about how big/risky this really is

- **The current SPICE path's cost is now concrete, not hand-wavy:** ≥3 full-frame
  CPU copies plus a hard 50 ms/~20 Hz poll ceiling plus whole-surface (not
  damage-rect) updates, all independently verified against the actual source. None
  of that is fixed by "add zero-copy" alone — the poll ceiling and whole-surface
  behavior are SPICE/event-loop properties that persist even if a *separate*
  local dmabuf path is added alongside it.
- **The "wgpu" framing throughout the design docs is stale against the shipped
  renderer.** This matters beyond pedantry: it would have sent an implementer
  down a dead-end API surface (wgpu dmabuf/hal interop) that isn't what runs on a
  real seat in this codebase at all.
- **The real bottleneck is a single, shared, substantial infrastructure gap** —
  sourcing a dmabuf fd out of QEMU — that gates *every* downstream design choice
  equally. It is not yet solved by this project's natural ecosystem building
  block (`vhost-device-gpu`), verified via current upstream state, not assumed.
- **Once that gap is closed, the shell-side half is cheaper than the literal
  acceptance wording implies.** Option B (KMS plane import) needs zero new
  dependencies and reuses a real, already-shipped, already-tested trait seam
  (MEDIA-2) almost directly — genuinely less work than Option A's GL/EGL-extension
  FFI, despite Option A being what the current acceptance bullet's prose
  describes.
- **This is hardware-adjacent-gated, but not for the same reason VFIO was.** No
  dedicated/passthrough GPU is required (unlike VFIO); what's missing is a
  confirmed combination of "a physical DRM seat that can also locally host
  libvirt/QEMU with a working accelerated render node," which nobody has
  assembled or recorded trying.

## Out of scope (this doc)

- Implementing any of the above in workspace crates (design/options only, per the
  task).
- Choosing between the two §3.3 delivery mechanisms (vhost-user-gpu vs.
  `-display dbus,gl=on`) — flagged as a real, deliberate infrastructure decision,
  not made here.
- Editing `docs/WORKLIST.md` **E12-7** (the zombie cloud-hypervisor-era entry, §1.3)
  or `docs/design/whitepaper-brief.md`'s matching stale framing — flagged for a
  future WORKLIST reconciliation pass, not this doc's assigned edit target.
  `docs/NEEDS-OPERATOR.md`'s parallel OW-8 "needs a live cloud-hypervisor host"
  line has the same staleness and is likewise not touched here.
- Correcting `quasar-vdi-desktop.md` lock 12's literal text or R1's "wgpu/GBM"
  wording — flagged as stale against the shipped `egui_glow`/EGL backend (§1.3),
  left for whoever owns that design doc's maintenance.
- Nova PCI/vGPU-flavor passthrough (`quasar-cloud.md` Q37) — a different mechanism
  for a different VM population than anything in this doc (§2).
- A guest-side Windows virtio-gpu/venus driver validation plan — blocked on the
  pre-existing, unrelated "no Windows image in this environment" gap (§3.1,
  TESTVM-3), not something this doc can scope around.

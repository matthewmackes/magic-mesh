# F44 builder + physical-seat deploy (ffmpeg soname epoch skew)

> **Why this doc exists (operator directive 2026-07-12):** *"Add all learned
> information so that no other AI need to discover it."* Everything below was
> discovered live while cutting the 12.0.0 RPM and deploying it to the physical
> Fedora seats. Read this **before** attempting another seat deploy — it will
> save you the multi-hour rediscovery of the F42↔F44 media-soname blocker.

## TL;DR

The build farm is **Fedora 42**; the physical seats are **Fedora 44**. An RPM
built on F42 **with `media-mpv`** links **ffmpeg-7 sonames** that do not exist on
F44, so it **cannot install on a seat**. The fix (operator's call: *"Stand up an
F44 builder, keep media-mpv"*) is to build the RPM **natively on F44**. Since
*"Build on BigBoy ONLY. Roll new VMs as required"* + *"The machines run
terraform"*, the F44 builder is a **new VM on the BigBoy dom0**, represented in
the tofu farm config.

## 1. The blocker: ffmpeg soname epoch skew (F42 vs F44)

`--features media-mpv` (mde-shell-egui) / `--features mpv` (mde-media-core) link
the system **libmpv** (`libmpv2-sys`/bindgen), and libmpv pulls the ffmpeg
stack. cargo-generate-rpm's `find-requires` reads the built binary's `DT_NEEDED`
and bakes those sonames into the RPM `Requires`. **The wrong sonames are in the
BINARY, not just the RPM metadata** — you cannot fix this by rewriting `Requires`;
the ELF references them at runtime. A **native F44 rebuild is mandatory.**

| lib | F42 farm (ffmpeg 7) | F44 seat (ffmpeg 8) |
|---|---|---|
| libavcodec    | `.so.61` | `.so.62` |
| libavformat   | `.so.61` | `.so.62` |
| libavutil     | `.so.59` | `.so.60` |
| libswresample | `.so.5`  | `.so.6`  |
| libswscale    | `.so.8`  | `.so.9`  |
| libpostproc   | `.so.58` | `.so.59` |
| libplacebo    | `.so.349`| (differs) |
| **libmpv**    | `.so.2`  | `.so.2` (**same** — present on the seat) |

**Evidence commands:**
```sh
# what the F42-built RPM demands (the .so.5/.so.8/.61/.59 lines are the killers):
rpm -qpR ~/mcnf-release-artifacts/magic-mesh-12.0.0-1.x86_64.rpm \
  | grep -iE 'swresample|swscale|avcodec|avformat|avutil|mpv|placebo|postproc'
# what an F44 seat actually has:
ssh <seat> 'ls /usr/lib64/libsw*.so.* /usr/lib64/libmpv.so.* /usr/lib64/libavcodec.so.*'
```
Symptom when you skip this: `dnf`/`rpm` install fails with `nothing provides
libswresample.so.5()(64bit)` (and .so.8, libplacebo, libpostproc, …). RPM Fusion
does **not** help — F44's mpv-libs is built against ffmpeg-8, so it provides
`.so.6`/`.so.9`, never the `.so.5`/`.so.8` the F42 binary asks for.

## 2. Seat inventory + access (the deploy targets)

All seats are **Fedora 44**, physical (not the VDI VMs), and already carry
`magic-mesh-12.0.0-1` **built on F42 without media-mpv** (so the running shell
does not link libmpv). Password auth (`$LAB_PW`, operator-provided for this
deploy) — key auth is not set up on the seats:

```sh
SSHPW='-o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new'
sshpass -p $LAB_PW ssh $SSHPW <user>@<ip>
```

| seat | IP | user | notes |
|---|---|---|---|
| **.138** | 172.20.146.138 | `root` | physical F44, DRM-capable Quasar seat, `libmpv.so.2` present |
| **Eagle** | 172.20.146.13 | `mm`   | F44 workstation (`UNIT-EAGLE`); root is rejected → use `mm` |
| **.2** | 172.20.146.2 | `mm`   | F44; also on the current mesh |
| ~~.216~~ | 172.20.146.216 | — | **OFFLINE** ("No route to host"); power on before deploying |

**NOT seats — skip:** `.144` / `.54` are Alpine **VDI test endpoints** (RDP:3389 /
Spice:5930 / VNC:5900), reached as `root` with the farm key. They are RDP/Spice/VNC
targets, not desktop seats — the operator said *"Skip VDIs for now."*

**Version-collision gotcha:** the seats already have `12.0.0-1`. `dnf install`
of the same VR says *"Nothing to do."* Force-replace instead:
```sh
rpm -Uvh --replacepkgs --force --nosignature /tmp/magic-mesh-12.0.0-1.x86_64.rpm
#            ^ rpm uses --nosignature, NOT --nogpgcheck (that is a dnf-only flag)
```
`rpm` does **not** resolve deps — pre-install any missing runtime deps with dnf
first, or bump the release (`-2`) and `dnf install` cleanly. On F44 the media
deps (mpv-libs → ffmpeg-libs) come from **RPM Fusion free**:
`dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm`.

## 3. The F44 builder VM on BigBoy

**BigBoy dom0 = `XEN-BIGBOY` @ `172.20.145.165`** (12 cores / 34 GB), root pw
`$LAB_PW` (also `/root/.mcnf-xapi-cred`). Local SR
`faa1a7c1-9663-1877-130d-488b1c94015d`, **291 G free**; dom0 `/` only 15 G;
management network UUID `8dee4afc-4fc7-60e5-0a3f-7b9b94954631`.

- **The dom0 has NO `qemu-img`** (only `vhd-util`). Convert the cloud image on the
  dev host (which has `qemu-img`), then the roll script `scp`s the raw over.
- **Dev host is disk-tight** (`/` ~91 %, ~7 G free). The raw is 5 G — it fits, but
  clear `/var/tmp/golden-build` first for headroom. Do **not** touch
  `/var/lib/mcnf-minio` (29 G object store — real data).

**RAM contention (important):** BigBoy runs the F42 farm VM `mcnf-build-52`
(~21.5 G). With it up, only ~9 G is free — not enough for the Servo
(`mde-web-preview`) link. The classifier **gates** stopping a shared farm VM;
the operator authorized it per-op. `xe vm-shutdown` frees the RAM but the
`memory-free` metric **lags ~10 s** (it read 12 G then settled at 31 G). Restart
`mcnf-build-52` after the cut (`xe vm-start`; it has `auto_poweron=true`).

**Roll command** (F44 Cloud image = `Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2`,
5 GiB virtual, from `download.fedoraproject.org/.../releases/44/Cloud/x86_64/images/`):
```sh
./install-helpers/setup-xcp-build-vm.sh \
  --xcp-host 172.20.145.165 --xcp-pass $LAB_PW \
  --name mcnf-build-f44 \
  --ip 172.20.0.131/16 --gw 172.20.0.1 \
  --vcpus 10 --mem 24GiB --disk 80GiB \
  --qcow2 /root/f44-build/fedora44.qcow2 \
  --pubkey /root/.ssh/mackes_mesh_ed25519.pub
```
The VM comes up as `mm@172.20.0.131` (mesh key). The script writes an NM keyfile
directly (cloud-init's netplan→NM render is broken on Fedora+Xen — the historic
"dark VM" root cause) and sets `auto_poweron=true`.

## 4. Toolchain + build + cut

```sh
# 1. bake the toolchain (rust 1.94 + mpv-libs-devel + the -devel set):
./install-helpers/setup-build-vm-toolchain.sh --host 172.20.0.131 --user mm
# 2. cut the RPM natively on F44 (xcp-build drives sync + release + Servo + CEF +
#    DRM-shell relink + generate-rpm, then pulls the RPM to ~/mcnf-release-artifacts):
MCNF_BUILD_HOST=172.20.0.131 ./install-helpers/xcp-build.sh rpm
```
- Canonical features: `MDE_RPM_SHELL_FEATURES="drm,live-helper,live-vdi,media-mpv"`,
  `MDE_RPM_LOCKED="--locked"` (`install-helpers/rpm-features.sh`).
- `mde-web-preview` (Servo) is **workspace-excluded** and MUST build separately
  **before** `generate-rpm` (the `rpm` subcommand already does this) — else
  *"Asset file not found: target/release/mde-web-preview"*.
- DoD for a media build: `rpm -qpR <rpm> | grep swresample` must show `.so.6`
  (ffmpeg-8), and `rpm -qlp <rpm>` must list all four binaries
  (`mackesd`, `mde-shell-egui`, `mde-web-cef`, `mde-web-preview`).

## 5. Terraform representation (the machines run terraform)

The farm is IaC at **`infra/tofu/xen-xapi/`** (XAPI-native, 4 aliased providers,
one per dom0 — the `../` XO root is **deprecated**). VMs are a shape-model
`for_each` over `local.build_vm_specs`, all cloning **one global golden template**
`var.golden_template_name` (`MDE-VM-golden`, **F42**). BigBoy = dom0 key
`xen-bigboy`, provider alias `big`, `ip_base 172.20.0.130`.

**State backend is DOWN:** `http://10.42.0.99:8390/state/xen-xapi` (the overlay
control-VM etcd backend) is unreachable, so `tofu apply` cannot lock state right
now. Therefore the F44 builder is handled exactly like the **`.170` VM already
is** in `build-vms.tf`: present in config, **adopt-pending via `tofu import`**
when the backend returns:
```
tofu import 'xenserver_vm.build_big["xen-bigboy-f44"]' <vm-uuid>
```
To make it a first-class tofu resource you also need a **per-VM template
override** (the golden is global today) — add an F44 golden
(`setup-xcp-golden-template.sh --name MDE-VM-golden-f44 --qcow2 <F44>`) and a
`template_name` field in the spec, or keep the builder as an imported one-off.

## 6. Credentials quick-reference

> **`$LAB_PW`** throughout this doc = the operator's single lab password for this
> airgapped fleet. It is **NOT committed** — read it from `/root/.mcnf-xapi-cred`
> (0600, off-repo) on the dev host, or ask the operator. Do not inline it into any
> tracked file.

| what | how to authenticate |
|---|---|
| BigBoy dom0 root | `root@172.20.145.165`, pw in `/root/.mcnf-xapi-cred` |
| farm build VMs | `mm@` + `/root/.ssh/mackes_mesh_ed25519` (passwordless sudo) |
| seats (.138/Eagle/.2) | password auth as user per §2, pw = `$LAB_PW` |
| XO (deprecated) | `ws://172.20.145.192:8080`, `admin@mcnf.local`, see `/root/.mcnf-xo-admin` |
| dev-host mesh pubkey | `ssh-ed25519 AAAAC3…jY1 mcnf-build-farm@rocky9-kvm2` |

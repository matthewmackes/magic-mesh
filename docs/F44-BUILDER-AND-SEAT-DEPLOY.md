# F44 builder + physical-seat deploy (ffmpeg soname epoch skew)

> **CURRENT DEPLOYMENT AUTHORITY — 2026-08-03:** the Fedora 44 Workstation
> target is exactly five physical seats: T480 `172.20.146.138`, Eagle
> `172.20.146.145`, Basement seat 15 `172.20.0.15`, Dell `172.20.146.225`, and
> Microsoft Surface `172.20.146.79`. Their current overlay assignments are in
> §2 and §7. The earlier `.13`, `.2`, and `.216` seat addresses and the July
> mesh map are historical and **must not be used for deployment**. Reconcile
> future inventory changes first in [BUILD-ENVIRONMENT.md](BUILD-ENVIRONMENT.md)
> and preserve live rollout evidence in
> [the 2026-08-03 seat record](ops/f44-seat-rollout-surface-2026-08-03.md).

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

All five targets are physical Fedora 44 Workstations, not build-farm or VDI
VMs. This is the canonical target set as of 2026-08-03:

| seat | LAN address | current overlay | deployment notes |
|---|---:|---:|---|
| **T480** | `172.20.146.138` | `10.42.0.8` | Direct-DRM seat; current-mesh enrollment corrected on 2026-08-03 |
| **Eagle** | `172.20.146.145` | `10.42.0.6` | T470S Workstation; exclude from bench testing, but include in fleet deployment |
| **Basement seat 15** | `172.20.0.15` | `10.42.0.5` | Low free-space condition; no `/dev/kvm`, so client/non-KVM placement only |
| **Dell** | `172.20.146.225` | `10.42.0.4` | Preserve its existing Browser VM domain during package deployment |
| **Microsoft Surface** | `172.20.146.79` | `10.42.0.7` | Distinct from seat 15; current Browser VM baseline exceeds its local capacity |

These overlay addresses are observed identity assignments, **not inputs to a
manual allocator**. Never hand-issue a duplicate address or copy another seat's
Nebula identity. Surface is not seat 15, and no physical seat belongs in the
build-farm inventory.

The same verified `magic-mesh-12.1.6-1` review RPM was installed on all five
targets on 2026-08-03. Its recorded SHA-256 is
`edb32a228e823a16c12383792b1da63c65326cb1d3f61e3832e8adaf288c9f54`;
consult the rollout record for embedded binary hashes and limitations. This is
rollout evidence, not permission to reuse that artifact as a future release.

The historical addresses `172.20.146.13`, `172.20.146.2`, and
`172.20.146.216` are not current deployment targets. Likewise, `.144` and `.54`
were Alpine VDI test endpoints, not desktop seats. Do not substitute any of
them when a current seat is unreachable; stop and reconcile live inventory.

No credential is stored in this repository. Use the operator-authorized account
for each seat and keep the password in the existing root-only credential file.
When password authentication is required, avoid putting the secret on the
command line:

```sh
SEAT_USER='<verified seat account>'
SEAT_IP='<address from the canonical table>'
sshpass -f /root/.mcnf-xapi-cred ssh \
  -o PreferredAuthentications=password \
  -o PubkeyAuthentication=no \
  -o StrictHostKeyChecking=accept-new \
  "${SEAT_USER}@${SEAT_IP}"
```

**Version-collision gotcha:** query every seat with `rpm -q magic-mesh` rather
than assuming its installed NVR. `dnf install` of the same version-release says
*"Nothing to do."* After the artifact hash and dependency transaction have been
proved independently on every target, force-replace a same-NVR review build:
```sh
RPM=/tmp/magic-mesh-review.rpm
rpm -Uvh --test --replacepkgs --force --nosignature "$RPM"
rpm -Uvh        --replacepkgs --force --nosignature "$RPM"
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
(~21.5 G). With it up, only ~9 G is free — not enough for the full native F44
release link. The classifier **gates** stopping a shared farm VM;
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

**Operational note from the 2026-07-15 browser deploy:** the F44 builder may be
halted while the regular BigBoy farm VM (`mcnf-build-52` / `.130`) is running.
The safe handoff is: confirm no active BigBoy farm slots, shut down
`mcnf-build-52`, wait for BigBoy `memory-free` to rise to roughly 30 GiB, start
`mcnf-build-f44`, and only then run `MCNF_BUILD_HOST=172.20.0.131
./install-helpers/xcp-build.sh rpm`. On boot, `.131` can report `No route to
host` for several polls and then `Connection refused` before SSH is ready; XAPI
may also show no guest metrics during this window even though the VM is healthy.
Give it at least a minute and verify with SSH before treating it as a dark VM.
After the cut, shut down `mcnf-build-f44` and restart `mcnf-build-52` so the
normal BigBoy farm capacity returns.

**Current handoff state recorded 2026-08-03:** after the five-seat artifact was
cut, `mcnf-build-f44` (`.131`) was halted and the canonical BigBoy farm VM
`mcnf-build-52` (`.130`) was restored. This is a dated observation, not a reason
to skip the live VM-state and active-slot checks before the next handoff.

## 4. Toolchain + build + cut

**Toolchain gap found live (2026-07-12):** the DRM shell links `-linput`/`-lgbm`/
`-ludev` directly. On a fresh F44 VM `libinput-devel` is **not** pulled by anything
else, so the final relink dies `mold: fatal: library not found: input`. Fixed in
`setup-build-vm-toolchain.sh` (now installs `libinput-devel mesa-libgbm-devel
systemd-devel`); if you hit it on an older builder, `dnf install -y libinput-devel`.

```sh
# 1. bake the toolchain (rust 1.94 + mpv-libs-devel + the -devel set):
./install-helpers/setup-build-vm-toolchain.sh --host 172.20.0.131 --user mm
# 2. cut the RPM natively on F44 (xcp-build drives sync + the workspace release,
#    DRM/live-VDI/media shell relink, and generate-rpm, then pulls the RPM):
MCNF_BUILD_HOST=172.20.0.131 ./install-helpers/xcp-build.sh rpm
```
- Canonical features: `MDE_RPM_SHELL_FEATURES="drm,live-vdi,media-mpv"`,
  `MDE_RPM_LOCKED="--locked"` (`install-helpers/rpm-features.sh`).
- The host CEF/Servo Browser helpers are retired and are not RPM assets. Guest
  Chromium is delivered only through `browser-vm`; do not restore those host
  binaries to satisfy an obsolete payload expectation.
- DoD for a media build: `rpm -qpR <rpm> | grep swresample` must show `.so.6`
  (ffmpeg-8), `verify-rpm-payload.sh payload <rpm>` must pass, and `rpm -qlp
  <rpm>` must include `mackesd` plus `mde-shell-egui` without either retired
  host Browser helper.
- Artifact-name gotcha found during the 2026-07-15 browser deploy: a native F44
  `xcp-build.sh rpm` may pull a fresh `magic-mesh-12.0.0-1.x86_64.rpm` while an
  older `magic-mesh-12.0.0-1.f44.x86_64.rpm` is still present in
  `/root/mcnf-release-artifacts`. Do not trust the filename suffix. Verify
  `rpm -qip <rpm>` build time and `rpm -qpR <rpm> | grep -E
  'libavcodec|libswresample|libswscale'` before copying to a physical seat; F44
  should show `.so.62`/`.so.6`/`.so.9`.

## 5. Terraform representation (the machines run terraform)

The farm is IaC at **`infra/tofu/xen-xapi/`** (XAPI-native, 4 aliased providers,
one per dom0 — the `../` XO root is **deprecated**). VMs are a shape-model
`for_each` over `local.build_vm_specs`, all cloning **one global golden template**
`var.golden_template_name` (`MDE-VM-golden`, **F42**). BigBoy = dom0 key
`xen-bigboy`, provider alias `big`, `ip_base 172.20.0.130`.

**Historical backend state (2026-07-12):**
`http://10.42.0.99:8390/state/xen-xapi` (the overlay control-VM etcd backend)
was unreachable, so `tofu apply` could not lock state. Re-probe it before every
plan, import, or apply; do not treat this dated outage as current fact. While
the backend is unavailable, the F44 builder remains adopt-pending like the
`.170` VM in `build-vms.tf`. When the backend returns, the recorded import form
is:
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
| five canonical seats | operator-authorized account per seat; use the root-only credential source from §2 when password auth is required |
| XO (deprecated) | `ws://172.20.145.192:8080`, `admin@mcnf.local`, see `/root/.mcnf-xo-admin` |
| dev-host mesh pubkey | `ssh-ed25519 AAAAC3…jY1 mcnf-build-farm@rocky9-kvm2` |

## 7. Adding a seat to the mesh (Nebula overlay)

The final 2026-08-03 post-reboot snapshot has eight online nodes: three
lighthouses at `10.42.0.1`–`.3` and the five Workstations below. It reports
seven healthy, seat 15 degraded by its known disk-headroom alarm, zero
unreachable, and `ha_ok=true`. Seat 15 still reaches every overlay node and all
five Workstations report four of four Syncthing folder peers connected.

| overlay | current Workstation identity | LAN address |
|---:|---|---:|
| `10.42.0.4` | Dell | `172.20.146.225` |
| `10.42.0.5` | Basement seat 15 | `172.20.0.15` |
| `10.42.0.6` | Eagle | `172.20.146.145` |
| `10.42.0.7` | Surface (`peer:SURFACE`) | `172.20.146.79` |
| `10.42.0.8` | T480 (`peer:T480`) | `172.20.146.138` |

The current CA SHA-256 recorded on T480, Surface, and the lighthouses is
`0b359a2378a0407ec824631a153aaaec62485e20f4220061bd3ea383e829bc6c`.
The July two-lighthouse map, the `magic-mesh` name, and its public endpoint
mapping are historical. Do not copy them into a token or configuration. Obtain
a fresh, fingerprint-pinned, single-use token from a currently healthy
lighthouse; the token itself is the authority for the current mesh name and
enrollment endpoint.

**Designed path — `mackesd join` (preferred, boot-durable, registers the node):**
```sh
# on a lighthouse (mackesd must be `serve`-ing — the /enroll listener is on :4243):
mackesd add-peer --role workstation          # emits the current single-use token
# on the seat (after installing the RPM — install alone does NOT join):
mackesd join '<fresh-token-from-current-lighthouse>' --role workstation
```
`mackesd join` does a fingerprint-pinned TLS `POST /enroll`, materializes
`/etc/nebula/{ca.crt,host.crt,host.key,config.yaml}`, **removes the stock
`config.yml`**, pins the role, and enables+starts `nebula.service` +
`mackesd.service` + `mesh-health.timer`. Do not trust a helper's embedded
default lighthouse address: use the endpoint carried by the fresh token. Code:
`crates/mesh/mackesd/src/cli/join.rs`,
`nebula_enroll_client.rs`, `workers/nebula_supervisor.rs:394` (config write + stock
removal), `nebula_enroll_endpoint.rs` (:4243 signer).

**Historical manual-path lesson — do not execute it on the current mesh:** the
2026-07-12 bridge manually signed and copied Nebula material to two seats. That
created overlay-only nodes without the role and registration state maintained
by `mackesd join`. Do not copy another seat's config, hand-select an address, or
manually sign a current Workstation merely because enrollment is inconvenient.

Verify the joined seat's unique `nebula1` address, current CA hash, services,
and control-plane health from all three lighthouses. A first ICMP can drop while
Nebula establishes a path, so use more than one packet; do not treat ping alone
as enrollment proof.

**Enrollment lessons retained from live operations:**

- **Set a distinct hostname before `mackesd join`.** Two July seats shipped as
  `localhost.localdomain`; the endpoint deduplicated them onto one identity and
  overlay address. Normalize and verify the hostname before consuming a token.
- A July seat's public `:4243` route hit a path-MTU timeout. Diagnose the live
  route and use a current, fingerprint-pinned endpoint under operator control;
  do not rewrite a new token to a dated lighthouse address by rote.
- T480's former `10.42.0.7` belonged to a different obsolete CA. On 2026-08-03
  that state was backed up, left through the supported path, and T480 joined the
  current authority as `10.42.0.8`; Surface legitimately owns current `.7`.
  Compare CA hashes before diagnosing an apparent duplicate address.
- A stale relay trust-authority pin correctly refused T480's first cross-mesh
  enrollment. Preserve old identity material in a root-only backup and perform
  explicit supported identity teardown; never overwrite a trust pin in place.
- **After join, `systemctl restart nebula mackesd`** to converge the running
  interface IP to the newly-issued cert and clear the "circuit breaker tripped"
  transient. A full reboot can also converge it, but the physical seats can
  require encrypted-disk intervention; do not reboot them without explicit
  operator coordination.
- **Same-LAN peers that enrolled at different times may fail to hole-punch each
  other directly** (e.g. new seats ↔ the long-lived Eagle) even though both reach
  the lighthouse — a NAT-hairpin quirk. A coordinated reboot historically
  re-established the path, but service-level recovery and live diagnosis come
  first; do not turn that lesson into a default fleet-reboot procedure.

## 8. Historical fresh-seat activation lesson (learned live 2026-07-12, seat 15)

> **Historical, not a current seat-15 procedure:** Basement seat 15 is now an
> enrolled Workstation at `172.20.0.15` / `10.42.0.5` and belongs in the five-seat
> deployment set. Do not rerun fresh-seat activation during a routine RPM
> replacement. The steps remain here for a genuinely new or reimaged seat.

Installing the RPM does **not** make a seat run — the node is **role-gated fail-closed**.
The original observation was made on seat 15 while it was still brand new:

1. **`mackesd` refuses to start unpinned** — `serve` exits with *"no deployment role
   pinned (/var/lib/mde/role.toml absent) — refuses to start its worker pool (ENT-2
   fail-closed)"*, and systemd hits the restart cap ("start request repeated too quickly").
2. **The shell unit SKIPS (not fails)** — `mde-shell-egui.service` has
   `ConditionPathExists=/var/lib/mde/role.toml`; with no role it logs *"skipped, unmet
   condition check"* and never launches. Easy to misread as "installed but broken".

**The one-liner that unblocks both** (needs root — writes `/var/lib/mde/`):
```sh
sudo mackesd role-pin workstation     # ranks: lighthouse 0 < workstation 1; upgrade-only
sudo systemctl reset-failed mackesd && sudo systemctl start mackesd   # now active
sudo systemctl start mde-shell-egui                                    # now grabs DRM tty1
```
The shell unit is `ExecStart=/usr/bin/mde-shell-egui` on `TTYPath=/dev/tty1`,
`Conflicts=getty@tty1`, `After=mackesd.service`, `WantedBy=multi-user.target` — so once the
role is pinned it also **auto-starts on the next boot**. Verify live: `systemctl show
mde-shell-egui -p NRestarts` = 0 (stable, not crash-looping) and the boot journal line
`"mde-shell-egui starting" … "drm":true`.

**Fresh-seat install path (cleaner than force-rpm):** a seat with NO prior `magic-mesh`
takes `sudo dnf install -y /tmp/<rpm>` directly — dnf resolves **all** deps from Fedora +
RPM Fusion (enable it first: `dnf install -y https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-44.noarch.rpm`,
then `dnf install -y mpv-libs` to pre-stage the ffmpeg-8 sonames). The `rpm -Uvh
--replacepkgs --force` dance in §2 is only for seats that already carry the same VR.
DoD after install: `ldd /usr/bin/{mackesd,mde-shell-egui} | grep 'not found'`
returns **nothing** (all ffmpeg-8/mpv sonames resolve), and neither retired
host Browser binary is present.

**Still standalone after this** — role-pin activates the local seat but does NOT join the
mesh. For overlay membership (chat/peers), follow §7 (`mackesd join`) after the shell is up.

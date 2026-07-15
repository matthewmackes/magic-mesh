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

## 4. Toolchain + build + cut

**Toolchain gap found live (2026-07-12):** the DRM shell links `-linput`/`-lgbm`/
`-ludev` directly. On a fresh F44 VM `libinput-devel` is **not** pulled by anything
else, so the final relink dies `mold: fatal: library not found: input`. Fixed in
`setup-build-vm-toolchain.sh` (now installs `libinput-devel mesa-libgbm-devel
systemd-devel`); if you hit it on an older builder, `dnf install -y libinput-devel`.

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

## 7. Adding a seat to the mesh (Nebula overlay)

The live overlay is a **Nebula V2 mesh named `magic-mesh`**, subnet **10.42.0.0/17**.
Lighthouses: **`10.42.0.1` → 165.227.188.238:4242** (also the **CA** + `/enroll`
listener) and **`10.42.0.2` → 104.131.64.207:4242**. Assigned overlay IPs:
`.1` LH1, `.2` LH2, `.3` Eagle, `.4` mcnf-ow-testnode, **`.5` seat-138, `.6` seat-2**.

**Designed path — `mackesd join` (preferred, boot-durable, registers the node):**
```sh
# on a lighthouse (mackesd must be `serve`-ing — the /enroll listener is on :4243):
mackesd add-peer --role workstation          # prints  mesh:magic-mesh@<lh-pub-ip>:4243#<bearer>?fp=<sha256>
# on the seat (after installing the RPM — install alone does NOT join):
mackesd join 'mesh:magic-mesh@<lh-pub-ip>:4243#<bearer>?fp=<sha256>' --role workstation
```
`mackesd join` does a fingerprint-pinned TLS `POST /enroll`, materializes
`/etc/nebula/{ca.crt,host.crt,host.key,config.yaml}`, **removes the stock
`config.yml`**, pins the role, and enables+starts `nebula.service` +
`mackesd.service` + `mesh-health.timer`. Canonical one-shot:
`install-helpers/rejoin-v11-mesh.sh <lh-PUBLIC-ip> workstation '<token>'` (override
its stale default LH IP). Code: `crates/mesh/mackesd/src/cli/join.rs`,
`nebula_enroll_client.rs`, `workers/nebula_supervisor.rs:394` (config write + stock
removal), `nebula_enroll_endpoint.rs` (:4243 signer).

**Manual path (fallback — overlay-only, used 2026-07-12 to bridge .138/.2):** sign a
V2 peer cert on LH1 and place the bundle by hand. Gets the seat on the overlay but
does NOT pin the role / register with mackesd — prefer `mackesd join`.
```sh
# on LH1 (the CA):
nebula-cert sign -ca-crt /etc/nebula/ca.crt -ca-key /var/lib/mackesd/nebula-ca/ca.key \
  -name "peer:<name>" -networks "10.42.0.<n>/17" -groups "role:peer" \
  -out-crt /tmp/<name>.crt -out-key /tmp/<name>.key
# on the seat: put ca.crt + host.crt + host.key + a member config.yaml in /etc/nebula/,
# move the stock config.yml aside (nebula -config loads the WHOLE dir), then
# systemctl restart nebula.  Member config = Eagle's minus the router unsafe_routes.
```
Verify: `ip -4 -o addr show nebula1` (a 10.42.0.x addr), `ping -c2 10.42.0.1`, and
seat↔seat (`ping 10.42.0.6` from .138). Note: a brand-new peer's FIRST ICMP can drop
during hole-punch — use `-c2+`.

**Enrollment gotchas (learned live 2026-07-12 joining seat-138 + seat-2):**
- **Set a distinct hostname BEFORE `mackesd join`.** Both seats shipped as
  `localhost.localdomain`; the enroll endpoint keys identity on the cert name, so
  the second join **deduped onto the first's identity + overlay IP** (a collision).
  `hostnamectl set-hostname seat-138` / `seat-2` first. `--name` is ignored once the
  role is already pinned, so the hostname is what actually names the cert.
- **If a seat's path to the public `:4243` endpoint hangs** (TCP opens but the TLS
  `POST /enroll` times out — a path-MTU issue on that seat's internet route),
  **retarget the token at the lighthouse's OVERLAY IP**: `sed 's/@<lh-pub-ip>:/@10.42.0.1:/'`
  — the fingerprint pin ignores hostname, and a seat already on the overlay reaches
  `10.42.0.1:4243` fine. This unblocked seat-2.
- **`mackesd peers` will NOT list a freshly-joined node** — the presence plane is
  *integration-gated* in this build (the daemon logs "session store integration-gated
  — needs the live etcd session-directory reader over Nebula"). Overlay membership
  (reachable from the lighthouse + peers) is the real, verifiable state; the peers
  view populates only for already-established members.
- **After join, `systemctl restart nebula mackesd`** to converge the running
  interface IP to the newly-issued cert and clear the "circuit breaker tripped"
  transient. A full reboot (what the operator does) achieves the same.
- **Same-LAN peers that enrolled at different times may fail to hole-punch each
  other directly** (e.g. new seats ↔ the long-lived Eagle) even though both reach
  the lighthouse — a NAT-hairpin quirk; a coordinated reboot re-establishes it.

## 8. Activating a FRESH seat: role-pin before anything runs (learned live 2026-07-12, seat .15)

Installing the RPM does **not** make a seat run — the node is **role-gated fail-closed**.
On a brand-new seat (`172.20.0.15` "Basement-Test-Workstation", F44, `mm`/`$LAB_PW`):

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
DoD after install: `ldd /usr/bin/{mackesd,mde-shell-egui,mde-web-cef,mde-web-preview} |
grep 'not found'` returns **nothing** (all ffmpeg-8/mpv sonames resolve).

**Still standalone after this** — role-pin activates the local seat but does NOT join the
mesh. For overlay membership (chat/peers), follow §7 (`mackesd join`) after the shell is up.

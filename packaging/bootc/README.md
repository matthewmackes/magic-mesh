# packaging/bootc/ — the ONE immutable MCNF image (E12-13)

The §5/QC-1 delivery lock: **one immutable bootc/ostree image for every role** —
egui-DRM shell + `mackesd` + Podman + Nebula + the CONSTRUCT-CLOUD
libvirt/QEMU-KVM/OVN host bits baked in; VM disks + mesh state live on the
writable partition. **Role is a config flag, not a build**: a Lighthouse runs
the byte-identical image with the desktop seat skipped/masked. The dnf/RPM
channel lane (PKG-8, gh-pages) is unchanged and carries on in parallel — this
directory adds the image lane on top of the same monolithic `magic-mesh` RPM.

Contents:

- `Containerfile` — FROM `quay.io/fedora/fedora-bootc:42`, installs the
  `magic-mesh` RPM (two lanes, below), adds the cloud substrate: `podman`,
  libvirt/QEMU-KVM, Open vSwitch, and OVN host bits. `cloud-hypervisor` is
  deliberately absent per CONSTRUCT-CLOUD/QC-1. Wires the DRM seat, boots to
  `graphical.target`. The writable-partition doctrine is inline.
- `units/mde-shell-egui.service` — the Construct **DRM-seat unit** (greetd-style,
  no display manager, no compositor — `quasar-vdi-desktop.md` lock 34). Image
  lane only; the dnf/RPM lane keeps launching the shell from a session via
  `org.magicmesh.Shell.desktop`.
- `build-image.sh` — typed-gated build driver (podman build + optional
  bootc-image-builder disk image). Exit contract: `0` built; `2` REFUSED
  (missing/invalid inputs, itemized — author error); `3` **GATED** — the
  registry is unreachable from this host (`GATED[E12-13/base-image]`, the
  expected outcome on the airgap-ish farm; never a raw podman splat mid-build).
  An image already in local storage skips the pull probe entirely, so a
  side-loaded base (`podman load`) builds fully offline. Shellcheck-clean.
- `system-preset/45-mcnf-quasar.preset` — the image-lane systemd preset (the
  declared seat policy; the Containerfile's `systemctl enable` materializes
  it, the preset keeps factory-reset/`preset-all` flows honest).
- `verify-image.sh` — **static** acceptance checks against the built image
  (payload binaries including `virsh`, `ovs-vsctl`, and QEMU; explicit
  cloud-hypervisor absence; seat unit + preset; enablement symlinks;
  graphical default; doctrine artifacts). Explicitly not a boot test.
- `rpms/` — staging dir for the local-RPM lane (populated by
  `build-image.sh --rpm`; only `.gitkeep` is committed).
- `context.containerignore` — build-context allowlist (only `packaging/`
  reaches the builder), passed via `--ignorefile` so no other container
  build inherits it.

## Building

Context is always the **repo root** (the Containerfile COPYies
`packaging/repo/magic-mesh.repo` + `packaging/bootc/…`).

```sh
# Lane A — channel: install magic-mesh from the gh-pages dnf repo (needs network)
packaging/bootc/build-image.sh

# Lane B — local RPM: bake a farm-built RPM (no channel dependency)
packaging/bootc/build-image.sh --rpm ~/mcnf-release-artifacts/magic-mesh-11.4.5-1.x86_64.rpm

# Either lane + a bootable disk image (root podman required)
sudo packaging/bootc/build-image.sh --rpm <rpm> --disk qcow2
```

`--base` overrides the Fedora bootc base (e.g. an F43 rebase);
`--tag` names the output (default `localhost/magic-mesh-bootc:latest`);
`MCNF_PULL_TIMEOUT=<secs>` bounds the base-image pull probe (default 120 —
raise it on a slow uplink, the fleet base is GB-scale).

Fully offline (no registry egress at all): side-load the base once —

```sh
podman load -i fedora-bootc-42.tar      # exported elsewhere via podman save
packaging/bootc/build-image.sh --rpm <farm-built.rpm>
```

the gate sees the base in local storage and skips the pull; podman build's
default `missing` pull policy never touches the network; the local-RPM lane
avoids the dnf channel. Everything else fails **typed** (rc 3, above).

## Boot-to-seat & the systemd unit set

The image enables **`mde-shell-egui.service`** (new, this directory) +
**`podman.socket`**, sets `graphical.target`, and inherits the RPM
post-install's all-roles set: `mackesd.service`, `nebula.service` (+ the
`nebula.service.d/10-mesh-recovery.conf` drop-in), `mesh-health.timer`,
`mesh-status.timer`, `magic-setup.service` (first-run wizard),
the first-boot fetch oneshots
(`mesh-shell-setup` / `mesh-broker-setup` / `mesh-netdata-setup`) and the
`mde-musicd.service` user unit. `etcd.service` + `syncthing.service` ship
condition-gated (they only start where `setup-etcd.sh` /
`setup-syncthing.sh` wrote their config — that config lives in `/etc`, which
persists).

Boot flow on a fresh Workstation: `magic-setup.service` owns tty1 until a role
is pinned to `/var/lib/mde/role.toml`; `mde-shell-egui.service` (ordered
`After=magic-setup.service`, `ConditionPathExists=/var/lib/mde/role.toml`,
`ExecCondition` = role `"workstation"`) then takes the VT and the egui shell
takes DRM master directly (the RPM links `mde-shell-egui` with
`--features drm`). If the shell hits its start limit, `ExecStopPost` restores
`getty@tty1` so the console is never lost.

## Roles (the §5 config flag)

Nothing is masked by default — **every role boots this same image**:

- **Workstation** — `role = "workstation"` in `/var/lib/mde/role.toml` →
  seat unit passes its `ExecCondition`, shell on the DRM seat, VDI stack live.
- **Lighthouse / headless** — any other (or no) role → the seat unit skips
  cleanly at every boot. Hard-off (§5 "Option 1", belt-and-braces on a box
  that must never light a seat):

  ```sh
  systemctl mask mde-shell-egui.service
  systemctl --global mask mde-musicd.service
  systemctl set-default multi-user.target
  ```

  Re-roling is the reverse (`unmask` + `set-default graphical.target` + pin
  the role) — no reinstall, per §5.

Relation to the **dnf-lane mesh-only set**: on the package channel the
headless story is the `magic-mesh-server` variant RPM (same daemon + units,
no GUI payload; conflicts with the full `magic-mesh`, `dnf swap` moves a node
between them). The image lane deliberately has **no such variant** — one
image, and the seat unit's role gate does what the package split does on the
dnf lane. Both lanes read the same `/var/lib/mde/role.toml`.

## Writable-partition doctrine

Full annotated table in the `Containerfile` header. The short form:

| Lane | Semantics across `bootc upgrade` | MCNF state there |
|---|---|---|
| `/usr`, `/boot` | swapped atomically | all binaries + units + assets |
| `/etc` | 3-way merge — local edits persist | `/etc/mackesd`, `/etc/nebula`, `/etc/etcd/etcd.env`, setup-written drop-ins, `/etc/hosts` mesh merges |
| `/var` | machine-local; image `/var` seeds **first boot only** | `/var/lib/mackesd`, `/var/lib/mde/role.toml` (the role flag), `/var/lib/etcd`, `/var/lib/mcnf-syncthing`, `~/Local` VM disks (`/home`→`var/home`), `/mnt/mesh-storage` (`/mnt`→`var/mnt`), `/opt/mcnf` (`/opt`→`var/opt`) |
| `/run/mde-bus` | tmpfs, tmpfiles re-creates per boot | the shared bus spool |

⚠ Known caveat: the RPM's `/opt/mcnf` automation/backoffice plane rides the
`/var` seed, so **image upgrades do not refresh it** — acceptable today (that
plane converges control-VM-side, DAR-46), flagged by `bootc container lint`
during the build (deliberately non-fatal).

## Update / rollback story

```sh
bootc status                      # what's deployed / staged / rolled back
bootc upgrade                     # pull + stage the new image; reboot to apply
bootc rollback                    # flip back to the previous deployment; reboot
bootc switch <registry>/<image>   # rebase to a different image/tag
```

Updates are atomic: the new image stages beside the running one and a reboot
swaps `/usr` wholesale; `/etc` merges; `/var` (VM disks, mesh state, role flag)
is untouched — a failed update is one `bootc rollback` away. Publishing the
image to a registry (so `bootc upgrade` has a source) is **operator-gated**
alongside the RPM channel publish (/release).

## Verification status (2026-07-01, re-run on .170 — supersedes the killed
## worker's .130 capture)

**Authoring-lane checks (worktree):**

- `bash -n` + shellcheck clean on both scripts; `systemd-analyze verify` on
  the seat unit clean (only the expected missing-binary note off-fleet).
- Refusal gates exercised live: multi-fault itemized refusal (bad `--rpm` +
  bad `--disk`; `--out` without `--disk`) → rc 2. Typed base gate exercised
  live with an RFC-2606 `.invalid` registry → `GATED[E12-13/base-image]`,
  rc 3.
- Every doctrine claim grep-verified at source: role regex ≡
  `mde-shell-egui.service`; bus/workgroup env pins ≡ `mackesd.service`;
  the enabled-unit set ≡ the RPM `post_install_script`; tmpfiles + `/etc`
  unit + `.repo` destinations ≡ the `generate-rpm` assets.

**Real farm builds (mm@172.20.0.170, podman 5.4.1, rootless):**

- The earlier ".130 cannot reach quay.io" story is **not** the current farm
  truth: **.170 pulls `quay.io/fedora/fedora-bootc:42` fine**, and the
  resolve gate's local-storage path was also proven ("offline OK", zero
  network, on rebuilds).
- **Channel lane: fails typed at dnf, not at the registry** — the gh-pages
  channel 404s for fedora-42 repodata (`No match for argument: magic-mesh`).
  The channel lane is therefore gated on the operator `/release` channel
  publish, not on reachability.
- **Local lane: green again for QC-1 (2026-07-10)** with the farm-built
  `magic-mesh-12.0.0-1.x86_64.rpm` →
  `localhost/magic-mesh-bootc:qc1-20260710` on BigBoy. QC-1 supersedes the older
  cloud-hypervisor bake: the image now installs libvirt/QEMU-KVM, Open vSwitch,
  and OVN directly from Fedora and rejects `/usr/bin/cloud-hypervisor` in the
  static verifier. The build also fixed the linux-surface layer for Fedora 42
  `dnf5` by using `dnf -y install --allowerasing`.
- **Disk lane: green for qcow2 on BigBoy (2026-07-10)** using root podman and
  `bootc-image-builder` against `localhost/magic-mesh-bootc:qc1-ovs-20260710`.
  Artifact:
  `~/magic-mesh-farm-qc1-image-build/packaging/bootc/out-qc1-ovs/qcow2/disk.qcow2`
  (2,080,833,536 bytes). The run fixed two disk-lane blockers: relative `--out`
  paths now mount as absolute host paths, and the image now embeds
  `/usr/lib/bootc/install/50-magic-mesh.toml` with XFS as the default root
  filesystem. The linux-surface layer also removes the Fedora base kernel so
  bootc sees a single `/usr/lib/modules` tree.
- **Generated-qcow2 boot proof: live on BigBoy XCP (2026-07-10).** The qcow2
  was converted/imported as raw into throwaway XCP VMs, then destroyed after
  evidence capture. The final `qc1-ovs-20260710` VM (`testvm-qc1-cloudinit`)
  reported `running`, BIOS boot, and Xen PV drivers detected. A stock-preserving
  NoCloud seed created only the temporary `mm` SSH user and static test IP.
  Inside the booted image, `cloud-init-*`, `NetworkManager`, `openvswitch`,
  `ovsdb-server`, and `ovs-vswitchd` were active; `virsh version` reported
  libvirt 11.0.0 / QEMU 9.2.4; `sudo ovs-vsctl show` returned the OVS UUID and
  `ovs_version: "3.4.5-3.fc42"`; required RPMs were installed; and
  `/usr/bin/cloud-hypervisor` was absent. Plain `ovs-vsctl show` as the seeded
  non-root user is denied by Fedora's `/run/openvswitch/db.sock` permissions
  (`openvswitch:hugetlbfs`, `750`); root/sudo is the expected host-admin proof.
- **`verify-image.sh`: static checks cover QC-1 and pass on the built image** — payload binaries
  (`virsh`, `ovs-vsctl`, QEMU, `cloud-init`, `qemu-ga`), explicit
  cloud-hypervisor absence, seat unit + preset, role gate, the bootc install
  rootfs config, single-kernel tree, the cloud-init/OVS enablement symlinks,
  `graphical.target` default, `.repo` + tmpfiles doctrine.
  Static container inspection only — **not** a boot and not live
  virtio-gpu/QEMU validation.

**Still gated outside the image lane:**

- **QEMU virtio-gpu→egui validation** — the stock qcow2 now has live in-guest
  access and host-virt proof. The missing QEMU/libvirt fast-path implementation
  is filed as `docs/WORKLIST.md` **QC-23**; RDP/VNC/SPICE remain the honest
  fallback paths until that lands.
- **Channel-lane image build** — operator-gated `/release` channel publish
  (the 404 above).
- **Registry publish** of the bootc image (what gives `bootc upgrade` a
  source) — operator-gated alongside the RPM channel publish.

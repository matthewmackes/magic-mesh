# MCNF Build Farm — architecture, automation, and recovery

The build farm gives MCNF parallel Rust/GUI build capacity + a real multi-node
mesh test bed, off the orchestration loop. This is the single reference for what
it is, how it's automated, and how to recover it.

> **Parent doc:** the canonical build environment (local dev host + this farm +
> the toolchain + reproduce-from-scratch) is
> [`docs/BUILD-ENVIRONMENT.md`](BUILD-ENVIRONMENT.md). This file is the farm
> deep-dive it points to.

> **TL;DR direction (DEVOPS-SUBSTRATE):** the durable Farm Automation Manager is
> **Xen Orchestra + OpenTofu + Ansible + Packer + a CI runner**, *not* hand-rolled
> bash. The `install-helpers/*xcp*`/`farm.sh` scripts are a working **stopgap**;
> the IaC under `infra/` is the target. See "Automation stack" below.

## Fleet

| Host | IP | Role | Notes |
|---|---|---|---|
| `rocky9-kvm2` (dev) | `172.20.145.192` | Orchestration + local builds + podman | Rocky 9.8; full GUI toolchain installed (below); runs XO + tofu/ansible/packer |
| `XEN-HOME-SERVICES` | `172.20.0.9` | XCP-ng 8.3 dom0 — build host (4c/24G) | local SR is **`ext`** (not "Local storage"); build VM → `172.20.0.50` |
| `KVM-XCP1` | `172.20.145.193` | XCP-ng 8.3 dom0 — test bed (4c/23G) | build VM → `172.20.0.90` (the VM is *named* `mcnf-build-51` but is NOT at .51); spin throwaway test nodes here |
| `XEN-BIGBOY` | `172.20.145.165` | XCP-ng 8.3 dom0 — high-capacity (**12c/32G**, 398G SR) | build VM → `172.20.0.130` (8 vCPU/24G; named `mcnf-build-52`, NOT at .52); room for several more build VMs |
| `XEN-194` | `172.20.145.194` | XCP-ng 8.3 dom0 — 4th dom0 (4c) | build VM → `172.20.0.170` (named `mcnf-build-53`); declared in `infra/tofu/xen-xapi` |

> The real build-VM IPs are **.50 / .90 / .130 / .170** (the VM *names* `mcnf-build-50/51/52/53`
> are legacy and do NOT equal the IP octet). Derive the live inventory with
> `install-helpers/farm-inventory.sh topology` — don't memorize this table.

**Access:** management is SSH-key (`~/.ssh/mackes_mesh_ed25519`, authorized on both
dom0s + the build VMs' `mm` user). First-time dom0 provisioning needs the XCP root
password in `$XCP_PASS`. Full operator authorization for both dom0s + all VMs.

## Dev-host build toolchain (the "build environment online" baseline)

The dev host builds the **entire workspace incl. the cosmic/iced GUI** locally.
Required packages (installed): `gcc-c++`, `cmake`, `mold`, system `libopus`
(`dnf --enablerepo=crb install opus-devel` on EL9), `alsa-lib-devel`,
`libxkbcommon-devel`, `gtk3-devel`. **Caveat:** this host's gcc 11.5 rejects
`-fuse-ld=mold` (the committed `.cargo/config.toml`), so build with
`RUSTFLAGS="-C link-arg=-fuse-ld=gold"`. `mde-workbench` builds + links in ~30 s.

## Automation stack (target — the Farm Automation Manager)

| Concern | OSS tool | Replaces (stopgap) |
|---|---|---|
| Hypervisor mgmt + **console** + REST API | **Xen Orchestra** (`http://172.20.145.192:8080`, in podman) | raw `xe` over ssh |
| Declarative VM lifecycle (create/recover/destroy) | **OpenTofu** + `vatesfr/xenorchestra` provider (`infra/tofu/`) | `setup-xcp-build-vm.sh` |
| Toolchain / config (idempotent) | **Ansible** (`infra/ansible/`) | `setup-build-vm-toolchain.sh`, `xcp-authorize-farm-key.sh` |
| Golden image build | **Packer** | `build-mde-vm-golden.sh` |
| Parallel build CI | **Forgejo Actions** / Woodpecker (planned) | `xcp-build.sh` + ad-hoc parallelism |
| Local parallel slots | **podman** | — |

**Why IaC over bash:** the bash path hit (and this doc records) a string of XCP
foot-guns — `xe`-over-ssh re-splitting spaced values, SR/template name divergence
between hosts, `cloud-localds` absent on EL9, flow-style cloud-init keys, blind
consoles. OpenTofu drives those through XO's API (no ssh-quoting), declares the
SR/template by UUID, and XO gives a real console — so the IaC path sidesteps the
whole class of bugs.

## Stopgap scripts (work today; being superseded by `infra/`)

- `farm.sh` — the orchestrator: `status · up · key · provision · toolchain · doctor · build · ssh`.
- `setup-xcp-build-vm.sh` — qcow2 → VDI + cloud-init NoCloud seed → boot. Hardened:
  genisoimage seed, local-SR fallback (ext/lvm), `printf %q` xe wrapper (the
  spaced-arg fix), block-style quoted SSH key, `auto_poweron`.
- `setup-build-vm-toolchain.sh` — SSH-driven toolchain install on a build VM.
- `xcp-authorize-farm-key.sh` — install the farm key on a dom0 (passwordless `xe`).
- `xcp-build.sh` — rsync the tree to a build VM + run cargo there, pull artifacts.

## The "dark VM" root cause (PROVEN — read this first)

**Symptom:** a freshly-provisioned build VM is `running` at the hypervisor but
**never reachable on its static IP** — dom0 can't ping it, its MAC is absent from
the dom0 ARP table, serial console looks empty. This dark-VMed *every* farm VM
across four provision attempts and burned a long, wrong-headed boot-debugging arc.

**It is NOT a boot/firmware/import problem.** Proven by mounting the VM's own disk
from dom0 and reading its logs:
- `-machine pc-0.10` is Xen's *normal* HVM machine string — not a legacy bug.
- The disk import is fine (raw `dd` of the VDI reads at full speed, zero I/O errors).
- The OS **boots fully**: `cloud-init … finished … Ran 10 modules with 0 failures`,
  growpart/resizefs ran, the `mm` user + SSH key installed, host keys generated.

**The real cause:** cloud-init *parses* the netplan-v2 `network-config` from the
NoCloud seed (the log shows `applying net config … 'addresses': ['172.20.0.50/16']`)
but **renders no NetworkManager keyfile** — `/etc/NetworkManager/system-connections/`
stays empty. So the NIC (`enX0` on Xen, not `eth0`) falls back to DHCP, and the
static-only `172.20.0.0/16` LAN has no DHCP server → no address → unreachable.

**The fix (now baked into `setup-xcp-build-vm.sh`):** write the NM keyfile
*directly* via cloud-init `write_files` (generic `type=ethernet`, no interface-name,
so it binds whatever the NIC is called), disable cloud-init's net management
(`network: {config: disabled}`), and `nmcli connection up` it on first boot. The
keyfile is auto-loaded on every later boot.

**To fix an already-built dark VM in place** (no re-provision needed): mount its
root disk from dom0 and drop the keyfile — see the disk-surgery steps below.

## Disk surgery from dom0 (mount a VM's root offline)

Used both to diagnose the dark VM and to fix it in place. **The btrfs layout has a
trap:** the partition's *default* mount is the btrfs **top-level** (subvolid 5),
which only contains the `root`/`home`/`var` subvolumes as directories — the real
OS `/etc`, `/usr` live under **`root/`**. Writing to `<mnt>/etc/...` silently
creates a *new* top-level dir the booted system never reads; write to
**`<mnt>/root/etc/...`** (or mount `-o subvol=root`).

1. `xe vm-shutdown uuid=<vm> force=true`
2. Attach **RW** to dom0 (RO can't replay a dirty journal → "can't read superblock"):
   `NV=$(xe vbd-create vm-uuid=$(xe vm-list is-control-domain=true --minimal) vdi-uuid=<root-vdi> device=autodetect type=Disk mode=RW); xe vbd-plug uuid=$NV`
3. `kpartx -av /dev/sm/backend/<sr>/<vdi>` → maps `…p1..p4`. F-cloud layout:
   p1 BIOS-boot · p2 EFI vfat · p3 ext4 `BOOT` · p4 btrfs `fedora` root.
4. `mount /dev/mapper/<vdi>p4 /mnt/diag` → the **real** root is `/mnt/diag/root/`.
   cloud-init logs are under `/mnt/diag/var/log/` (top-level `var` subvol).
5. Edit, `sync`, then **detach cleanly** or the VM can't reclaim the disk
   (`SR_BACKEND_FAILURE_1200`, stuck tapdisk): `umount` → `kpartx -d` →
   `xe vbd-unplug uuid=$NV` → `xe vbd-destroy uuid=$NV`.

> Cloud images set journald `Storage=volatile` (logs in `/run`), so the systemd
> journal is **not** readable offline — rely on `/var/log/cloud-init*.log`.
> A new VM gets a new SSH host key → clear the stale one: `ssh-keygen -R <ip>`.

**XCP foot-guns (encode these in any automation):**
- `xe` over ssh re-splits on spaces → quote every spaced value (`printf %q`, or use
  XO's API / UUIDs).
- The local SR isn't always "Local storage" (it's `ext` on `.9`); resolve by type.
- Built-in templates ("Other install media") exist but the name has spaces — same
  ssh-splitting trap.
- `cloud-localds` isn't packaged on EL9 — build the `cidata` seed with `genisoimage`.
- Flow-style `ssh_authorized_keys: [ <key> ]` silently drops user-data (the key's
  spaces break the YAML list) — use block-style + quoted (XPA-13).
- New VMs don't auto-start after a host reboot unless `other-config:auto_poweron=true`
  is set on the VM **and** the pool.

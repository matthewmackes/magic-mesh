# MCNF Build & Development Environment — canonical reference

> **This is the development toolchain and build environment for the MCNF platform.**
> It is canonical: read it before building, and **do not rediscover it**. If you
> find something here is wrong or has drifted, fix *this file* (and the pointer in
> `AI_GOVERNANCE.md §10`) rather than relearning it from scratch. Every item below
> was learned the hard way; the "Gotchas index" exists so no one repeats that.

There are **two build surfaces**, both real and both supported:

1. **The local dev host** (`172.20.145.192`, Rocky 9.8) — builds the *entire*
   workspace incl. the cosmic/iced GUI in ~seconds-to-a-minute. Best for tight
   edit→build→verify loops. **Caveat: gcc 11.5 rejects `mold`** → use the gold
   override (below).
2. **The build farm** (four Fedora VMs across four dom0s — real IPs `172.20.0.50` / `.90` / `.130` / `.170`; descriptive hostnames except BigBoy's `mcnf-build-52`, see §3) — fully
   OpenTofu/Ansible-managed (see "Build farm" §). Best for offloaded/parallel
   builds, the release gates, and RPM cuts. gcc 15 there, so `mold` works as-is.

**AI directive:** all AI agents must use the build farm for build/test/gate work
unless the command is only a tiny local syntax/probe check. Parallelize
independent verification across `.50` / `.90` / `.130` / `.170` using explicit
`MCNF_BUILD_HOST` + `MCNF_BUILD_SLOT`; put the long pole on BigBoy (`.130`).
Avoid containers when a direct farm-host fixture is enough. Farm/test hosts are
safe for destructive reboot/recovery operations unless the task explicitly says
otherwise.

**Bench-test directive (operator 2026-07-07):** exclude **Eagle** from bench
testing. Use the other two available bench seats for bench verification. Those
seats have encrypted disks and require a key at boot, so do not reboot them
unless a reboot is genuinely required for the test or recovery path.

---

## 1. Quick start — build right now

**On the local dev host** (`/root/magic-mesh`), the committed `.cargo/config.toml`
selects `mold`, which this host's **gcc 11.5 rejects** (`-fuse-ld=mold` needs
gcc ≥ 12 / clang). Override to the gold linker:

```sh
cd /root/magic-mesh
source ~/.cargo/env                              # rustup isn't on the default PATH
RUSTFLAGS="-C link-arg=-fuse-ld=gold" cargo build --workspace
RUSTFLAGS="-C link-arg=-fuse-ld=gold" cargo test -p mde-theme   # token changes
```

`mde-workbench` (the heaviest cosmic/iced crate) builds + links in ~30 s this way.

**Offload to the farm** (no local linker caveat; gcc 15 + mold):

```sh
install-helpers/xcp-build.sh cargo build -p <crate>   # rsync tree → .50 → build
install-helpers/xcp-build.sh gates                    # fmt + clippy + test
install-helpers/farm.sh status                        # both nodes ready?
```

---

## 2. The toolchain

| Component | Value | Notes |
|---|---|---|
| **Rust** | pinned **1.94.0** (`rust-toolchain.toml`); MSRV floor **1.85** | 1.94 is the ceiling — softbuffer 0.4.8 breaks on 1.95. `rustup` needed (not on stock hosts). |
| **Linker** | `mold` (set in `.cargo/config.toml`) | **gcc ≥ 12 only.** EL9/gcc 11.5 dev host → `RUSTFLAGS="-C link-arg=-fuse-ld=gold"`. |
| **C/C++** | gcc + gcc-c++ + cmake | the audio chain (`audiopus_sys`) vendors Opus → needs a C++ compiler + cmake. |
| **CMake policy** | `CMAKE_POLICY_VERSION_MINIMUM=3.5` (`.cargo/config.toml`) | vendored Opus' CMakeLists predates CMake 4. |
| **Packaging** | `cargo-generate-rpm` 0.21.0 | one signed `magic-mesh` RPM; cut via `/release`. |

### System dev libraries (what the workspace links)

The set differs by distro because the **dev host is EL9 (Rocky), not Fedora**:

**Fedora** (the target platform + the farm VMs):
```sh
sudo dnf install -y gcc gcc-c++ cmake mold git rsync pkgconf-pkg-config \
  genisoimage cloud-utils protobuf-compiler openssl-devel \
  alsa-lib-devel opus-devel gtk3-devel libxkbcommon-devel
```

**EL9 / Rocky 9 (the dev host)** — same, **except `opus-devel` is in CRB**, not the
default repos (this is the single most-rediscovered prereq):
```sh
sudo dnf install -y gcc gcc-c++ cmake gtk3-devel alsa-lib-devel libxkbcommon-devel
sudo dnf --enablerepo=crb install -y opus-devel        # <-- CRB, EL9-specific
# mold is present but UNUSED here (gcc 11.5) — build with the gold override.
```

Why these: gtk3 + libxkbcommon = the cosmic/iced GUI; alsa-lib + opus = the audio
chain; protobuf = etcd-client; openssl-devel only for the build (the product is
rustls-only, openssl is cargo-deny-banned at link).

---

## 3. The hardware / fleet

Full operator authorization on all three (root, incl. destructive ops). Private
LAN `172.20.0.0/16`. Management is the **mesh SSH key**
`~/.ssh/mackes_mesh_ed25519` (authorized on both dom0s + the build VMs' `mm`
user). Secrets are **off-repo** — see "Credentials" below.

| Host | IP | OS | Cores / RAM | Role |
|---|---|---|---|---|
| `rocky9-kvm2` (dev) | `172.20.145.192` | Rocky 9.8 | — | Orchestration + **local builds** + podman; runs XO + tofu/ansible/packer; this is where Claude Code + `/root/magic-mesh` live |
| `XEN-HOME-SERVICES` | `172.20.0.9` | XCP-ng 8.3 dom0 | 4c / 24 GiB | hypervisor — build VM `mcnf-build-home-services` (172.20.0.50, 4 vCPU/12 GiB); local SR is `ext` ("Local storage") |
| `KVM-XCP1` | `172.20.145.193` | XCP-ng 8.3 dom0 | 4c / 23 GiB | hypervisor — build VM `mcnf-build-kvm-xcp1` (172.20.0.90, 4 vCPU/12 GiB) |
| `XEN-BIGBOY` | `172.20.145.165` | XCP-ng 8.3 dom0 | **12c / 32 GiB** | hypervisor — build VM `mcnf-build-52` (172.20.0.130, **12 vCPU/20 GiB**); 398 GiB SR; the high-capacity node (room for several more build VMs) |
| `XEN-194` | `172.20.145.194` | XCP-ng 8.3 dom0 | 4c / — | hypervisor — build VM `mcnf-build-xen-194` (172.20.0.170, 4 vCPU/11 GiB); the **4th dom0** (added after the 3-dom0 table was first written; confirmed live 2026-07-01) |

> ⚠️ **Build-VM IPs follow a per-dom0 lane** (`infra/tofu/xen-xapi/build-vms.tf`): XEN-HOME-SERVICES → `.50–.80`, KVM-XCP1 → `.90–.120`, XEN-BIGBOY → `.130–.160`, **XEN-194 → `.170+`**. The real farm is **4 build VMs: .50 / .90 / .130 / .170** (there are no live `.51`/`.52` IPs — probing them gives "No route to host"). The non-BigBoy build hostnames are descriptive (`mcnf-build-home-services`, `mcnf-build-kvm-xcp1`, `mcnf-build-xen-194`); BigBoy intentionally keeps `mcnf-build-52`. **Full heavy-slot capacity is 2+2+3+2 = 9** (not 7).

> **Standing rule (operator 2026-06-30): BigBoy takes the longest / most-complex build.** The single heaviest job always routes to **XEN-BIGBOY** (`172.20.0.130`, 12 vCPU / ~20 GiB — the high-capacity node): a full `cargo --workspace` build/test/clippy, the biggest egui crates (`mde-shell-egui`/`mde-workbench`), a cold cosmic/iced/wgpu compile, or the RPM release build (`MCNF_BUILD_SHAPE=big` / an explicit `MCNF_BUILD_HOST=172.20.0.130`). The 4-vCPU nodes (`.50`/`.90`/`.170`) take the shorter/simpler jobs. This composes with the per-node concurrency cap: spread the *count* to honor caps, but the *long pole* goes to BigBoy first — never leave the workspace/heavy-GUI build on a small node while BigBoy runs a trivial one.

### Credentials (locations only — never in-repo)
- **Mesh SSH key:** `~/.ssh/mackes_mesh_ed25519` (+ `.pub`) — dom0s + build-VM `mm`.
- **dom0 root password:** operator-held / in the agent's memory; needed only for a
  *first* dom0 provision (`$XCP_PASS` / `--xcp-pass`) before the key is installed.
- **XO admin creds:** `/root/.mcnf-xo-admin` (0600).
- **XO API token (OpenTofu):** `/root/.mcnf-xo-token` (0600), minted by
  `install-helpers/xo-mint-token.sh`.

---

## 4. The build farm (IaC-managed)

The three build VMs are **declared as code** and built by OpenTofu through Xen
Orchestra (XO drives XAPI, so the `xe`-over-ssh foot-guns are gone). This is the
**DEVOPS-SUBSTRATE** Farm Automation Manager; the `install-helpers/*xcp*` /
`farm.sh` bash scripts are the working stopgap underneath.

```
golden template (XCP-2) ──tofu (clone via XO)──▶ cloud-init NM-fix ──ansible──▶ toolchain
```

| Layer | Where | Day-2 |
|---|---|---|
| VM lifecycle | `infra/tofu/` (`vatesfr/xenorchestra` provider) | `source infra/tofu/env.sh; tofu plan` / `tofu apply` |
| Toolchain/config | `infra/ansible/` | `ansible-playbook infra/ansible/build-vm-toolchain.yml` |
| Golden template | `install-helpers/setup-xcp-golden-template.sh` → `MDE-VM-golden` (UEFI, both pools) | one-time per pool |
| Mgmt + console + API | **Xen Orchestra** (`http://172.20.145.192:8080`, podman) | UI / REST |
| Drive builds | `install-helpers/xcp-build.sh` (rsync + cargo on `.50`) | `xcp-build.sh cargo …` |

`tofu` state is **local + gitignored**; the XO token is off-repo (`env.sh` sources
it from `/root/.mcnf-xo-token`). Farm internals + the recovery playbook live in
[`docs/farm.md`](farm.md).

### Requesting a build — the `@farm` convention (the canonical lane)

The default way to get something built is **declarative, no AI, no manual dispatch**:
tag a worklist task with `@farm:{<command>}` and the **reconciler timer**
(`mcnf-farm-reconcile.timer`, FARM-AUTO-4) runs it on the fleet within one interval,
idempotently (it skips a job whose result already matches a clean HEAD).

```text
- [>] **SOME-TASK: …**  @farm:{cargo test -p mde-bus}  @farm:{cargo clippy -p mackesd}
```

- Jobs are parsed by `automation/lib/farm-jobs.sh` (only open/in-progress tasks are
  active); dispatched by `automation/lib/farm-dispatch.sh` to a free node
  (per-node flock, big-iron-first), built **with shared sccache** (BUILD-PLATFORM-1),
  result recorded as JSON + (via the `farm_orchestrator` worker) published to the
  Bus → the Workbench **Build Farm** panel.
- The reconciler is the *canonical* lane; the other FARM-AUTO capabilities (Forgejo
  on push, etcd pull-agents, the mackesd worker) are alternates over the same substrate.
- Design + rationale: [`docs/design/build-platform.md`](design/build-platform.md).

---

## 5. Reproduce the build environment from scratch

Another AI/operator can rebuild the whole thing from this repo:

**A. A local dev host that builds the workspace** (any EL9/Fedora box with root):
1. Install the dev libs for the distro (§2; EL9 → `opus-devel` from CRB).
2. `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.94.0` → `rustup` picks up `rust-toolchain.toml`.
3. Build: EL9 → `RUSTFLAGS="-C link-arg=-fuse-ld=gold" cargo build --workspace`; gcc ≥ 12 → plain `cargo build --workspace`.

**B. The build farm** (given XCP-ng dom0s + XO):
1. Bring up XO (`install-helpers/xo-up.sh`) + add the pools; mint the tofu token
   (`install-helpers/xo-mint-token.sh` → `/root/.mcnf-xo-token`).
2. Build the golden template on each pool:
   `install-helpers/setup-xcp-golden-template.sh --xcp-host <dom0> --name MDE-VM-golden`.
3. `cd infra/tofu && source env.sh && tofu init && tofu apply` → the build VMs.
4. `cd infra/ansible && ansible-playbook build-vm-toolchain.yml` → the toolchain.
5. `install-helpers/farm.sh status` → both nodes `ready`; smoke with
   `install-helpers/xcp-build.sh cargo build -p mde-bus`.

**C. First-time dom0 onboarding** (key + overlay): `install-helpers/onboard-xcp-host.sh`
(installs the mesh key + static-nebula; needs `$XCP_PASS` once).

---

## 6. Gotchas index (each already fixed — here so it's never re-hit)

| Symptom | Cause → fix | Lives in |
|---|---|---|
| `cannot find -fuse-ld=mold` / link fails on the dev host | gcc 11.5 (EL9) rejects mold | `RUSTFLAGS="…gold"` (§1) |
| `-lopus` not found on EL9 | `opus-devel` is in **CRB**, not default repos | `dnf --enablerepo=crb install opus-devel` |
| vendored Opus cmake configure fails | CMake 4 dropped policy < 3.5 | `CMAKE_POLICY_VERSION_MINIMUM=3.5` (`.cargo/config.toml`) |
| Rust 1.95 won't compile softbuffer | softbuffer 0.4.8 `make_dispatch!` | pin `1.94.0` (`rust-toolchain.toml`) |
| fresh build VM "running" but **never on its IP** | cloud-init parses netplan-v2 but renders **no NM keyfile** on Fedora+Xen → NIC DHCPs on a DHCP-less LAN | write the NM keyfile directly via `write_files` (`infra/tofu/cloud-init/…`, `setup-xcp-build-vm.sh`) |
| tofu clone boots **BIOS** from a UEFI template | provider defaults firmware to BIOS | `hvm_boot_firmware = "uefi"` |
| tofu "expected a single VM … found 2" | provider matches VMs by name across **all** pools | **unique** `name_label` per node |
| tofu clone hostname = `localhost` | provider meta-data leaves the cloud default | `hostnamectl` in `runcmd` |
| `xe` over ssh mangles spaced values | ssh re-splits the remote command | quote each arg with `printf %q` (the `xe()` wrapper) |
| local SR not found as "Local storage" | it's type `ext` on some hosts | resolve by name **then** type |
| `cloud-localds` missing on EL9 | not packaged | build the seed with `genisoimage` |
| can't mount a VM's `/etc` from dom0 | btrfs **top-level** ≠ root; real `/etc` is under the `root/` subvol; RO mount can't replay a dirty journal | mount `root/` subvol **RW** (`docs/farm.md` disk-surgery) |
| new VM halts after a host reboot | XCP gates auto-start on VM **and** pool flags | `other-config:auto_poweron=true` on both |

---

## See also
- [`AI_GOVERNANCE.md §10`](../AI_GOVERNANCE.md) — the directive pointing here.
- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — build prereqs, test rules, commit gates.
- [`docs/farm.md`](farm.md) — farm architecture + dom0 recovery playbook.
- [`infra/tofu/README.md`](../infra/tofu/README.md) · [`infra/ansible/README.md`](../infra/ansible/README.md) — the IaC.

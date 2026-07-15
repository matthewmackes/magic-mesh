# MCNF Build & Development Environment — canonical reference

> **This is the development toolchain and build environment for the MCNF platform.**
> It is canonical: read it before building, and **do not rediscover it**. If you
> find something here is wrong or has drifted, fix *this file* (and the pointer in
> `AI_GOVERNANCE.md §10`) rather than relearning it from scratch. Every item below
> was learned the hard way; the "Gotchas index" exists so no one repeats that.

There are **two surfaces**, but only one does heavy builds:

1. **The build farm** (four **Fedora 42** VMs across four dom0s — real IPs `172.20.0.50` / `.90` / `.130` / `.170`; descriptive hostnames except BigBoy's `mcnf-build-52`, see §3) — the **only** real path for heavy `cargo` (`build`/`test`/`check`/`clippy`), the release gates, and native farm RPM cuts. Fully OpenTofu/Ansible-managed (see "Build farm" §). Drive it with `install-helpers/xcp-build.sh`; route a job with `MCNF_BUILD_HOST`. gcc 15 there, so `mold` works as-is.
2. **The local dev host** (`172.20.145.192`, Rocky 9.8) — **fmt / metadata / probe only.** Heavy local `cargo` is **hard-disabled** here by `cargo-farm-guard.sh` (installed ahead of the real `cargo` via `install-helpers/install-drain-guardrails.sh`): local `target/` dirs fill the disk and wedge the drain, so `build`/`test`/`check`/`clippy`/`run` exit 97 and redirect you to the farm. `fmt` still runs locally (`rustup run 1.94.0 rustfmt`). This host's gcc 11.5 rejects `mold` anyway — see §5 for the fresh-box gold-linker override.

**AI directive:** all AI agents must use the build farm for build/test/gate work
unless the command is only a tiny local syntax/probe check. Parallelize
independent verification across `.50` / `.90` / `.130` / `.170` using explicit
`MCNF_BUILD_HOST` + `MCNF_BUILD_SLOT`; put the long pole on BigBoy (`.130`).
Avoid containers when a direct farm-host fixture is enough. Farm/test hosts are
safe for destructive reboot/recovery operations unless the task explicitly says
otherwise.

**Browser/runtime note (learned 2026-07-14):** the live build-VM addresses are
the `172.20.0.x` farm lanes above; inherited `10.0.0.x` pins are stale and time
out. For direct live probes outside `xcp-build.sh`, ssh as the build user with
the mesh key (`ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.x`). CEF runtime
probes currently have a warm bundle on `.50` at `$HOME/mde-cef-active`; do not
assume `/opt/mde/cef` exists on generic farm VMs unless a packaging/install step
has staged it.

**RPM target-Fedora note (learned 2026-07-15):** all four farm build VMs
currently report **Fedora 42**. Therefore `xcp-build.sh rpm` produces a native
F42-linked RPM, even when the target workstation is Fedora 44. Do not install a
native farm RPM on an F44 Workstation seat unless `rpm -Uvh --test` passes; media
and ICU sonames can differ (`mpv-libs`, FFmpeg, ICU, Python). For an F44
workstation deploy, use the container lane with an explicit Fedora argument:
`install-helpers/build-rpm-fedora43.sh 44` from a farm checkout, then copy the
RPM from `target-f43/generate-rpm/`. The directory name is historical; the first
positional argument controls the Fedora container tag. For the split Browser
package, copy and install the base and Browser RPMs together and always run the
transaction test first:
`rpm -Uvh --test --replacepkgs --force --nosignature magic-mesh-*.rpm magic-mesh-browser-*.rpm`.
The live `.15` proof on 2026-07-15 used exactly this lane and produced F44 RPMs
at 70.0 MiB (`magic-mesh`) and 39.1 MiB (`magic-mesh-browser`), both under the
90 MiB channel guard.

**DRM shell live-restart note (learned 2026-07-15):** `mde-shell-egui.service`
conflicts with `getty@tty1.service` and has an `ExecStopPost` that starts getty
again for console recovery. A remote `systemctl restart mde-shell-egui.service`
can cancel the start because the role-gate `ExecCondition` receives `SIGHUP`
during the tty1 handoff. For live `.15` deploys over SSH, use the two-step
sequence instead: `systemctl stop getty@tty1.service; sleep 1; systemctl start
mde-shell-egui.service`, then confirm `systemctl is-active
mde-shell-egui.service`, `NRestarts=0`, and the journal version line. This is a
service orchestration gotcha, not a Browser helper/runtime failure.

**Servo/browser test note (learned 2026-07-15):** cold Servo test builds can
exhaust a 4-vCPU farm VM's disk through Rust incremental/query-cache output even
when the source is fine. Put Servo tests/checks on BigBoy (`172.20.0.130`) when
they are the long pole, and set `CARGO_INCREMENTAL=0` if a previous run hit
ENOSPC. Treat `.50`/`.90`/`.170` ENOSPC during rsync or `target/` writes as a
slot-capacity problem: remove only disposable `~/magic-mesh-farm-*` slots you
created, then rerun the heavy job on BigBoy. Do not keep duplicate cold
small-node filters running after an equivalent warmed BigBoy slot has already
covered the assertion; cancel or clean the duplicate slot so the farm stays
usable for the next gate.

**BigBoy slot hygiene note (learned 2026-07-15):** BigBoy's build VM currently
has a 79G `/home`; it is the right long-pole target, but several cold heavy
slots can still fill it. Reuse a warm BigBoy slot for follow-up tests when it
already contains the needed dependency graph, and remove only your finished or
failed disposable `~/magic-mesh-farm-*` slots promptly before starting another
cold heavy crate there. If direct reuse is needed after an `xcp-build.sh` sync,
SSH to the same slot and run the focused cargo command there with
`CARGO_INCREMENTAL=0`.

**Env-gated live smoke note (learned 2026-07-15):** `xcp-build.sh cargo ...`
accepts only Cargo arguments after the `cargo` subcommand and does not forward
arbitrary local `MDE_*` smoke variables into the remote command. For Browser
live UI probes such as `MDE_CEF_LIVE_UI_SMOKE=1` or
`MDE_SERVO_LIVE_UI_SMOKE=1`, first sync/build the slot with `xcp-build.sh cargo
test ...`, then SSH into the synced `~/magic-mesh-farm-<slot>` directory and run
the env-prefixed `cargo test` directly on that farm host. Do the same direct-SSH
step for non-Cargo shell probes/checks; `xcp-build.sh` subcommands are the
documented `sync`, `cargo`, `gates`, `rpm`, `pull`, `shell`, and route helpers,
not arbitrary remote command names.

**Farm command quoting note (learned 2026-07-15):** `xcp-build.sh cargo ...`
runs the requested cargo command through the remote shell. Avoid unescaped shell
metacharacters in test filters (`|`, `&&`, `;`, redirects) unless you intend the
remote shell to interpret them. Prefer one simple Cargo test filter per farm job
or a direct SSH command from inside the synced slot when a complex expression is
really needed.

**Parallel slot reuse note (learned 2026-07-15):** do not start two
`xcp-build.sh` sync/cargo jobs against the same `MCNF_BUILD_SLOT` concurrently;
their rsync phases can collide on transient files and fail with code 23. Use
distinct slots for parallel fanout, or wait for the first sync/build to finish
and then SSH into that warmed slot for follow-up env-gated probes.

**Nested CEF workspace note (learned 2026-07-15):** `crates/desktop/mde-web-cef`
is a standalone nested Cargo workspace, not a package in the repo-root workspace.
Farm cargo gates for it must use `--manifest-path crates/desktop/mde-web-cef/Cargo.toml`
instead of `-p mde-web-cef`; `-p mde-web-cef` from the repo root will fail before
compiling.

**Bench-test directive (operator 2026-07-07):** exclude **Eagle** from bench
testing. Use the other two available bench seats for bench verification. Those
seats have encrypted disks and require a key at boot, so do not reboot them
unless a reboot is genuinely required for the test or recovery path.

---

## 1. Quick start — build right now

**Everything heavy goes to the farm** (the only real build path; gcc 15 + mold,
no linker caveat):

```sh
install-helpers/xcp-build.sh cargo build -p <crate>   # rsync tree → farm → build
install-helpers/xcp-build.sh gates                    # fmt + clippy + test
MCNF_BUILD_HOST=172.20.0.130 \
  install-helpers/xcp-build.sh cargo build --workspace   # long pole → BigBoy (.130)
install-helpers/farm-topology.sh table                # all 4 nodes: verified util table
```

Heavy local `cargo` (`build`/`test`/`check`/`clippy`/`run`) is **guard-disabled**
on this dev host (`cargo-farm-guard.sh`, see the intro): those commands exit 97
and point you at `xcp-build.sh`. Only `fmt` + metadata run locally —
`rustup run 1.94.0 rustfmt` (or `cargo +1.94.0 fmt`) for token/format changes.

> A **fresh, unguarded** EL9 box can still build the workspace locally; its
> gcc 11.5 rejects `-fuse-ld=mold` (needs gcc ≥ 12 / clang; the committed
> `.cargo/config.toml` selects mold), so override to the gold linker
> (`RUSTFLAGS="-C link-arg=-fuse-ld=gold" cargo build --workspace`) — see §5.
> `mde-shell-egui` (the heaviest egui crate) links in ~30 s that way.

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

Why these: libxkbcommon (+ the wgpu/DRM stack) = the egui GUI; alsa-lib + opus =
the audio chain; protobuf = etcd-client; openssl-devel only for the build (the
product is rustls-only, openssl is cargo-deny-banned at link). *(`gtk3-devel` is a
legacy cosmic/iced leftover in the install line — no longer needed; the tree has
zero gtk deps in `Cargo.lock`.)*

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

> Stale `10.0.0.x` build-host pins are invalid on this farm and should be
> treated as doc/agent drift. Run `install-helpers/farm.sh status` or
> `install-helpers/farm-topology.sh table`, then use the verified `172.20.0.x`
> address explicitly.

> **Standing rule (operator 2026-06-30): BigBoy takes the longest / most-complex build.** The single heaviest job always routes to **XEN-BIGBOY** (`172.20.0.130`, 12 vCPU / ~20 GiB — the high-capacity node): a full `cargo --workspace` build/test/clippy, the biggest egui crate (`mde-shell-egui`), a cold egui/eframe/wgpu compile, or the RPM release build (`MCNF_BUILD_SHAPE=big` / an explicit `MCNF_BUILD_HOST=172.20.0.130`). The 4-vCPU nodes (`.50`/`.90`/`.170`) take the shorter/simpler jobs. This composes with the per-node concurrency cap: spread the *count* to honor caps, but the *long pole* goes to BigBoy first — never leave the workspace/heavy-GUI build on a small node while BigBoy runs a trivial one.

### Credentials (locations only — never in-repo)
- **Mesh SSH key:** `~/.ssh/mackes_mesh_ed25519` (+ `.pub`) — dom0s + build-VM `mm`.
- **dom0 root password:** operator-held / in the agent's memory; needed only for a
  *first* dom0 provision (`$XCP_PASS` / `--xcp-pass`) before the key is installed.
- **XO admin creds:** `/root/.mcnf-xo-admin` (0600).
- **XO API token (OpenTofu):** `/root/.mcnf-xo-token` (0600), minted by
  `install-helpers/xo-mint-token.sh`.

---

## 4. The build farm (IaC-managed)

The four build VMs are **declared as code** and built by OpenTofu through Xen
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
| Drive builds | `install-helpers/xcp-build.sh` (rsync + cargo on a farm node; route with `MCNF_BUILD_HOST`) | `xcp-build.sh cargo …` |

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
5. `install-helpers/farm.sh status` → all four nodes `ready`; smoke with
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
| farm ssh with the default key/user fails | direct probes need the mesh key and the build user | `ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.x` |
| `xcp-build.sh bash ...` prints the usage banner | `xcp-build.sh` is not an arbitrary remote-shell wrapper; use `xcp-build.sh cargo ...` or `sync`, then SSH into `~/magic-mesh-farm-<slot>` with the farm key for live/runtime probes | intro "Env-gated live smoke note" |
| farm job hangs on `10.0.0.x` | inherited build-host pin from stale docs/agent memory | verify with `install-helpers/farm.sh status`; use `.50` / `.90` / `.130` / `.170` |
| CEF live probe says `/opt/mde/cef` is missing on a farm VM | farm build VMs do not all have the packaged runtime staged under `/opt`; `.50` currently has the warm live bundle in `$HOME/mde-cef-active` | set `MDE_CEF_ROOT=$HOME/mde-cef-active` for `.50` probes, or run the packaging installer before using `/opt/mde/cef` |
| `cargo test -p mde-web-cef` says the package does not match any packages | `mde-web-cef` is a nested standalone workspace outside the repo-root workspace package set | run through the farm with `cargo test --manifest-path crates/desktop/mde-web-cef/Cargo.toml ...` |
| rsync code 23 while renaming a transient provider file in a farm slot | two `xcp-build.sh` jobs reused the same `MCNF_BUILD_SLOT` concurrently | use distinct slots for parallel jobs, or serialize the `xcp-build.sh` sync/build and direct-SSH into the warmed slot afterward |
| native farm RPM dependency check fails on an F44 Workstation | the farm VMs are Fedora 42, so `xcp-build.sh rpm` emits an F42-linked/native-dependency RPM; F44 Workstations can have newer FFmpeg/ICU/Python sonames instead | cut the workstation RPM in the Fedora container lane, e.g. `install-helpers/build-rpm-fedora43.sh 44`, then prove it with `rpm -Uvh --test` before install |
| `.170` returns ENOSPC despite a warm default checkout | stale per-slot `~/magic-mesh-farm-*` / `cef-*` directories can consume the VM disk | remove only stale disposable slot dirs; keep the shared warm `~/magic-mesh-farm` unless intentionally resetting cache |
| focused `mde-shell-egui` tests ENOSPC on a fresh `.90` slot | the shell crate can still compile a broad desktop dependency fanout before reaching a narrow test filter | put the long shell/browser compile on BigBoy `.130` or reuse a warmed slot; clean the failed disposable slot before retrying |

---

## 7. Fedora target matrix & the glibc compatibility contract

> **Why this section exists (build-deploy-4).** Different parts of the build/
> deploy chain pin **different Fedora releases** (42 / 43 / 44), and RPMs are
> **glibc-forward-compatible only** — so "which build feeds which target" is a
> real correctness constraint, not a cosmetic detail. It used to live only in
> operator memory + scattered one-line comments; this table is the single source
> of truth. Every version below is quoted from its file at the line cited — do
> not restate a Fedora number here without updating the cited file too.

### 7.1 The contract (read this first)

A magic-mesh RPM carries an **auto-generated `Requires: libc.so.6(GLIBC_x.y)`**
whose version is the **highest glibc symbol version actually referenced by any
ELF it ships** (not simply the build host's glibc). `dnf install` then **refuses
on any Fedora whose glibc is older than that ceiling**. There is no backward
shim: an RPM built where a shipped binary pulls in a newer symbol will not
install on an older release.

- Proven in-repo: an RPM built on the **Fedora 44** toolchain auto-required
  **`GLIBC_2.43`** and would not `dnf install` on Fedora 43; rebuilding inside a
  `fedora:43` container dropped the requirement to unversioned `libc.so.6`
  (`install-helpers/build-rpm-fedora43.sh:4-9`; `docs/WORKLIST.md:476`, ONBOARD-7).
- Corollary (**the invariant**): **every `fedora-N` channel directory must be
  fed an RPM built on a glibc no newer than Fedora N's.** The channel baseurl is
  `fedora-$releasever-$basearch` (`packaging/repo/magic-mesh.repo`), so each
  fleet node pulls the RPM under its own `$releasever` — and that RPM must have
  been built on ≤ that node's Fedora.
- The risk is **latent, not deterministic**: whether a too-new RPM actually
  fails depends on which glibc symbols the shipped binaries happen to reference,
  so a native cut can install today and brick after a dependency/OS bump pulls in
  a newer symbol. Do not rely on "it installed last time."

**glibc numbers:** only **F44 → provides `GLIBC_2.43`** is pinned in-repo (the
ONBOARD-7 evidence above). The exact glibc minor for **F43** and **F42** is *not*
recorded in this repo and was not pinned offline — treat them as "the glibc that
ships with Fedora 43 / 42" (both older than 2.43), and confirm the exact number
against the distro before relying on it.

### 7.2 The matrix (component → Fedora → why → constraint)

| Component | Fedora | Source (file:line) | Why that version | glibc / install constraint |
|---|---|---|---|---|
| **bootc immutable image base** | **42** | `packaging/bootc/Containerfile:53` (`ARG BOOTC_BASE=quay.io/fedora/fedora-bootc:42`) | "matches the fleet's RPM channel … mesh-service container is FROM fedora:42 too" (`Containerfile:50-52`); `--build-arg BOOTC_BASE=…` for an F43+ rebase (`:52`) | The **oldest** live target → the effective glibc **floor**. Anything installed *into* this image (RPM + layered dnf pkgs) must not require a glibc newer than F42's. |
| **Canonical container RPM cut** | **43** (default) | `install-helpers/build-rpm-fedora43.sh:43` (`FEDORA="${1:-43}"`) | Builds the RPM inside a `fedora:43` container so its glibc `Requires` match F43 and it installs on F43 lighthouses / older cloud images (`:4-9`) | Produces an RPM installable on **F43 and newer** (forward-compat). Positional arg overrides the version. |
| **Farm native RPM cut** (`xcp-build.sh rpm`) | farm VM's Fedora (**42** current) | `docs/BUILD-ENVIRONMENT.md:11` ("four Fedora 42 VMs"); verify live with `install-helpers/farm.sh status` + `/etc/fedora-release` before relying on it | Native release build/gates run on the farm VMs (§4) | Inherits the **farm VM's glibc and native library sonames**. Today this is an F42 artifact; it is not a safe F44 Workstation artifact when FFmpeg/ICU/Python sonames differ. For F44 seats, use `install-helpers/build-rpm-fedora43.sh 44` and `rpm -Uvh --test`. |
| **CI fedora-native job** | **44** | `.github/workflows/ci.yml:312` (`container: fedora:44`) | Advisory build+test on the real target platform | `continue-on-error: true` — **not** a release artifact; never fed to a channel dir. |
| **Sovereign mesh dnf channel dirs** | **43 + 44** | `automation/forgejo/dnf-channel-up.sh:30` (`FEDORAS="${MCNF_FEDORA_VERSIONS:-43 44}"`) | Serves `fedora-43` + `fedora-44` dirs mirroring gh-pages | Each dir needs an RPM built on ≤ its Fedora. **No `fedora-42` dir is produced** by default (see 7.3). |
| **gh-pages channel (client repo)** | `$releasever` (43, 44 live) | `packaging/repo/magic-mesh.repo` (`baseurl=…/fedora-$releasever-$basearch/`) | Client dnf resolves its own `$releasever` dir | Node pulls the RPM under its own Fedora; published for `fedora-43`/`fedora-44` (`docs/WORKLIST.md:1132`). |
| **DO lighthouse droplet** | **43** | `infra/tofu/zone1-do/variables.tf:16` (`default = "fedora-43-x64"`) | Lighthouse cloud image; "must have a live dnf channel for its releasever — fedora-42 has none" (`install-helpers/do-lighthouse-up.sh:19-20`) | Needs the F43-container RPM (`build-rpm-fedora43.sh`) or a channel `fedora-43` dir. |
| **Local dev host** | **EL9 / Rocky 9.8** (not Fedora) | `docs/BUILD-ENVIRONMENT.md:12,67` | Orchestration + tight local build loops; gcc 11.5 (gold linker) | Builds workspace binaries, **not** release RPMs (its glibc is EL9's, unrelated to the Fedora channel). |

### 7.3 Known inconsistencies — flagged, NOT changed (needs-owner-confirmation)

Per the change discipline (a bootc base bump is load-bearing), **no version
default was altered** while documenting this. The following divergences are real;
each is left in place and flagged for the owner rather than "aligned" blindly:

1. **F42 image/farm vs F43 canonical RPM vs F44 CI/workstations — no enforced baseline.**
   The glibc-forward rule (7.1) says the canonical RPM should be built on the
   **oldest** live target (today **F42**, the bootc base), yet the canonical
   script defaults to **F43**, native farm cuts currently run on **F42**, and CI
   plus the live Workstation seat can be **F44**. Glibc compatibility is only
   one part of this: media/ICU/Python sonames also vary by Fedora release, as
   the 2026-07-15 `.15` deploy proved when a native F42 farm RPM failed the F44
   dependency check and the Fedora 44 container-cut RPM passed. Whether this
   spread is deliberate (F43 chosen as the lighthouse floor, F42 image/farm
   retained as the oldest baseline, F44 served as a Workstation channel) or drift
   is **not determinable from the code** — it is maintained by operator memory.
   **Owner call needed:** pick one canonical build baseline per target channel
   and enforce it. This is exactly the P1 finding
   `build-deploy-4` in `docs/review/PLATFORM-REVIEW-2026-07-10.md` (§719-725),
   whose recommendation — a committed `FLEET-BASELINE` file driving the RPM
   version + a `verify-rpm.sh` gate asserting `rpm -qpR | grep GLIBC ≤ baseline`
   — is **not yet implemented**. Do not bump the bootc base or the script default
   to "fix" this without that owner decision.

2. **No `fedora-42` channel directory, but the bootc `repo` lane targets it.**
   The F42 bootc image's default-adjacent `repo` lane (`dnf -y install magic-mesh`,
   `Containerfile:86`) resolves `$releasever=42` against a channel that only
   builds `fedora-43`/`fedora-44` dirs, so it **404s** (`No match for argument:
   magic-mesh`). This is *known and currently handled*, not silently broken:
   production bootc builds use the **`local` lane** (stage a farm-built RPM into
   `packaging/bootc/rpms/`), and the channel lane is gated on the operator
   `/release` publish (`packaging/bootc/README.md:175-181`;
   `install-helpers/do-lighthouse-up.sh:19-20`). **Caveat that keeps this on the
   list:** the local lane stages whichever RPM the operator/farm produced into
   an **F42** image. Today the native farm cut is also F42, but container-cut
   RPMs can target F43/F44 explicitly, and nothing enforces that the staged RPM
   matches the image/channel before bootc build time.

3. **`build-rpm-fedora43.sh:4` calls the dev host "The F44 dev host".** The
   canonical dev host is **Rocky 9.8 / EL9** (`docs/BUILD-ENVIRONMENT.md:12`), not
   F44. The script's mechanism is still useful: it lets the farm cut an RPM
   inside a Fedora container whose tag is chosen explicitly, instead of inheriting
   the host's Fedora release and dependency sonames. The "F44 dev host" wording is
   stale. Minor; flagged so a reader does not trust it as the current dev-host
   fact.

4. **The farm VM Fedora version is load-bearing.** This doc now records the
   live 2026-07-15 farm state as Fedora 42, but the native RPM cut still inherits
   whatever release the farm image actually runs. Keep this doc, the farm image
   template, and channel policy synchronized whenever the farm OS changes; do not
   treat a native RPM as channel-neutral without an explicit dependency test.

> **Reviewer line-number note:** `docs/review/PLATFORM-REVIEW-2026-07-10.md:720`
> cites `dnf-channel-up.sh:31` for the `FEDORAS='43 44'` default; in the current
> tree that assignment is on **line 30**. The value is unchanged.

---

## See also
- [`AI_GOVERNANCE.md §10`](../AI_GOVERNANCE.md) — the directive pointing here.
- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — build prereqs, test rules, commit gates.
- [`docs/farm.md`](farm.md) — farm architecture + dom0 recovery playbook.
- [`infra/tofu/README.md`](../infra/tofu/README.md) · [`infra/ansible/README.md`](../infra/ansible/README.md) — the IaC.

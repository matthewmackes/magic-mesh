# Environment Rebuild Runbook — MCNF DevOps Substrate from bare metal

> **Purpose.** Rebuild the entire MCNF **dev/test/CI environment** from scratch on
> **three bare-metal machines** with **stock XCP-ng installations** + one control host.
> This is the `DEVOPS-SUBSTRATE` epic (§10, `docs/design/process-governance.md`) as an
> executable runbook. The environment is **airgapped dev — no production** (AI_GOVERNANCE
> §10); the agent has full control of all three machines.
>
> **Status legend per step:** ✅ verified live · 🔧 partially in place · 🔲 to build.
> As each DS-task lands, its steps flip to ✅ and the commands are confirmed against the
> real hosts (§10 B9 — docs move with the build).

---

## 1. Topology — three machines + roles

```
                          ┌──────────────────────────────────────────┐
                          │  CONTROL / AGENT HOST   (Rocky Linux 9)   │
                          │  rocky9-kvm2 · 172.20.145.192 · root      │
                          │  • Claude Code + the git worktrees        │
                          │  • OpenTofu + Ansible (IaC control)       │
                          │  • Xen Orchestra CE (podman) — XO API     │
                          │  • Forgejo + Actions runner (podman)      │
                          │  • podman ephemeral container mesh        │
                          └───────────────┬──────────────────────────┘
                                          │ XO/XAPI (wss) + SSH + Nebula overlay
              ┌───────────────────────────┴───────────────────────────┐
              ▼                                                         ▼
  ┌─────────────────────────────┐                       ┌─────────────────────────────┐
  │ XEN HOST A  (XCP-ng 8.3.0)  │                       │ XEN HOST B  (XCP-ng 8.3.0)  │
  │ XEN-HOME-SERVICES           │                       │ KVM-XCP1                    │
  │ 172.20.0.9 · root           │                       │ 172.20.145.193 · root       │
  │ 4 cores · 24 GB · 207 GB SR │                       │ 4 cores · 23 GB · 207 GB SR │
  │ build-farm VMs              │                       │ test-fleet VMs (LH+peers)   │
  └─────────────────────────────┘                       └─────────────────────────────┘
```

- **Why three machines.** One control plane (drives everything, holds no test workload),
  two hypervisors so the mesh test gate (3 LH + 3 peers, §10 V4) spans **real** hosts —
  multi-host relay/quorum can't be faked on a single box. Each XCP host = 4 physical
  cores, so VMs **share** cores (staggered/IO-bound pipelines, not 4× linear).
- **Networks.** Control + Host B are on `172.20.145.0/24`; Host A is on `172.20.0.0/24`.
  The **Nebula overlay** abstracts the two subnets — every mesh node reaches every other
  over the overlay regardless of underlay subnet.

## 2. Inventory of record (2026-06-21)

| Role | Hostname | Underlay IP | OS / product | CPU / RAM / SR | Access |
|---|---|---|---|---|---|
| Control/agent | `rocky9-kvm2` | 172.20.145.192 | Rocky Linux 9.8 | (KVM guest) | local `root` |
| Xen Host A (build) | `XEN-HOME-SERVICES` | 172.20.0.9 | XCP-ng 8.3.0 | 4c / 24 GB / 207 GB | `root` |
| Xen Host B (test) | `KVM-XCP1` | 172.20.145.193 | XCP-ng 8.3.0 | 4c / 23 GB / 207 GB | `root` |

**Shared dom0 credential:** both XCP hosts use the same `root` password, held in the
secrets store (DS-8), **never committed**. Export it once per shell from the store:

```bash
export DOM0_PW="$(get-secret xcp/dom0-root)"     # bootstrap: operator pastes it
sshpass -p "$DOM0_PW" ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null root@<host>
```

> ⚠️ The dom0 password is a **bootstrap-only** credential and must never appear in the repo,
> argv history, or this doc. The end-state moves all secrets to **etcd + age over Nebula**
> (§10 / DS-8); rotate the dom0 password and install the control host's SSH key onto the
> hosts as part of hardening (see §11), then disable password SSH.

## 3. Phase 0 — bare metal → stock XCP-ng  ✅ (hosts already at this state)

For each of the **two hypervisor machines**:

1. Download **XCP-ng 8.3** ISO (`https://xcp-ng.org/#easy-to-install`), write to USB.
2. Boot the installer; accept defaults. Set: hostname (`XEN-HOME-SERVICES` / `KVM-XCP1`),
   a **static management IP** (per the inventory table), DNS, NTP, and the `root` password.
3. Pick the local disk as the **default SR** (the installer creates `Local storage`,
   ~207 GB here). Finish + reboot.
4. From the control host, verify XAPI:
   ```bash
   sshpass -p "$DOM0_PW" ssh root@172.20.0.9 'xe host-list params=name-label --minimal'
   ```
   Expect the host name-label. (Stock XCP-ng exposes **XAPI** on `:443` via stunnel +
   `:80`; **Xen Orchestra is NOT included** — it is added in Phase 2.)

> A stock install gives you `xe` (the XAPI CLI) on the dom0 and the XAPI endpoint. No
> management GUI yet — that's Xen Orchestra, which we self-host on the control host so a
> single XO drives **both** pools.

## 4. Phase 1 — control host bootstrap  ✅ (tofu 1.12.3 + ansible-core 2.14.18 installed 2026-06-21)

On `172.20.145.192` (`root`):

```bash
# Base tooling
dnf install -y git jq curl podman sshpass yum-utils make    # podman/sshpass/jq/git already present ✅

# OpenTofu (official RPM repo) — DS-1
cat >/etc/yum.repos.d/opentofu.repo <<'REPO'
[opentofu]
name=OpenTofu
baseurl=https://packages.opentofu.org/opentofu/tofu/rpm_any/rpm_any/$basearch
repo_gpgcheck=1
gpgcheck=1
enabled=1
gpgkey=https://packages.opentofu.org/opentofu/tofu/gpgkey
REPO
dnf install -y tofu                                          # 🔲

# Ansible — DS-2
dnf install -y ansible-core
ansible-galaxy collection install community.general          # 🔲

# Clone the repo (GitHub canonical)
git clone https://github.com/matthewmackes/magic-mesh.git /root/magic-mesh   # ✅ present as worktree
```

**SSH key onto the hosts** (replace the bootstrap password auth):
```bash
ssh-keygen -t ed25519 -f /root/.ssh/id_ed25519 -N ''        # if absent
for h in 172.20.0.9 172.20.145.193; do
  sshpass -p "$DOM0_PW" ssh-copy-id -o StrictHostKeyChecking=no root@$h
done
```

## 5. Phase 2 — Xen Orchestra CE (the XAPI/IaC bridge)  ✅ verified 2026-06-21

The OpenTofu provider talks to **Xen Orchestra**, not bare XAPI — so XO must run first.
Self-host **XO Community Edition** in podman on the control host (≈3 GB RAM; needs Redis).
Use the reproducible helper **`install-helpers/xo-up.sh`** (idempotent):

```bash
./install-helpers/xo-up.sh        # redis (aliased 'redis') + xen-orchestra-ce
# XO UI/API → http://172.20.145.192:8080
```

**Verified gotchas (baked into the helper):**
- The `ezka77/xen-orchestra-ce` image hardcodes the Redis host as `redis` → the redis
  container gets `--network-alias redis` (a custom `REDIS_URI` is ignored).
- XO listens on **container port 8000**, not 80 → map `-p 8080:8000`.
- The image ships **no default admin**. Create one inside the container:
  ```bash
  podman exec xo-ce sh -c "cd /home/node/xen-orchestra && \
    node_modules/.bin/xo-server-recover-account admin@mcnf.local '<PW>'"
  ```

**Mint a token + add both pools** (via the bundled `xo-cli`, inside the container):
```bash
podman exec xo-ce sh -c "cd /home/node/xen-orchestra && \
  node packages/xo-cli/index.mjs register --au http://localhost:8000 admin@mcnf.local '<PW>'"
# token → ~/.config/xo-cli/config.json (.token, 43 chars) → store as XOA_TOKEN (DS-8)
for h in 172.20.0.9 172.20.145.193; do
  podman exec xo-ce sh -c "cd /home/node/xen-orchestra && node packages/xo-cli/index.mjs \
    server.add host=$h username=root password='<DOM0_PW>' allowUnauthorized=true"
done
# verify: both servers report status=connected; pools XEN-HOME-SERVICES + KVM-XCP1 visible
```

> Provider choice (DS-10): the **official `vatesfr/xenorchestra`** provider takes a
> **token** + `ws/wss` URL (`XOA_TOKEN`/`XOA_URL`) — preferred (token, not password; aligns
> with mesh-native secrets). Verified live: `tofu plan` authenticates and resolves the real
> `KVM-XCP1` pool/network/SR; a full `apply` additionally needs the golden template (DS-5).

## 6. Phase 3 — IaC: OpenTofu VM lifecycle  🔧 DS-1 (provider+XO+pool proven; apply needs DS-5 golden)

The real config lives in `infra/tofu/` (committed; `tofu validate` clean, `tofu plan`
authenticates to live XO and resolves the `KVM-XCP1` pool/network/SR). Layout:

Layout under `infra/tofu/` in the repo:

```hcl
# infra/tofu/providers.tf
terraform {
  required_providers {
    xenorchestra = { source = "vatesfr/xenorchestra", version = "~> 0.30" }
  }
}
provider "xenorchestra" {
  # url   = "ws://172.20.145.192:8080"   # from $XOA_URL
  # token = ...                          # from $XOA_TOKEN  (never in-repo; see §11)
  insecure = true                         # self-signed XAPI
}
```

```bash
cd infra/tofu
export XOA_URL="ws://172.20.145.192:8080"
export XOA_TOKEN="$(get-secret xo/api-token)"     # DS-8 secret helper; bootstrap: paste
tofu init
tofu plan -out=tfplan
tofu apply tfplan                                  # provisions a VM on the chosen pool
tofu destroy                                       # tears it down — repeatable
```

- **State backend (DS-10):** bootstrap with a **local** backend committed-encrypted or kept
  on the control host; migrate to a mesh-native backend once etcd is up (DS-8). Never
  commit plaintext state (it holds resource IDs, not secrets, but treat as sensitive).
- VM definitions (LH count, peer count, template, SR placement) are **variables** so the
  3 LH + 3 peer (per-feature) and 3 LH + 9 peer (release) topologies are one `-var`.

## 7. Phase 4 — Ansible node configuration + golden image  🔲 DS-2 / DS-5

1. **Golden image:** build once with `install-helpers/build-mde-vm-golden.sh` against a pool,
   capture as an XCP **template**; Tofu clones it. The golden carries the base OS + deps so
   per-VM provision is fast (keeps the V4 gate from bottlenecking, §10).
2. **Ansible** (`infra/ansible/`) brings a cloned VM to an **enrolled mesh node**:
   - `site.yml` → installs the MCNF RPM, sets role (Lighthouse/Server/Workstation),
     runs `magic-fleet` enroll against a lighthouse, joins the Nebula overlay.
   - Idempotent: a second run is a no-op (gate: `ansible-lint`, DS-9).
3. **Snapshot-reset pool (DS-5, §10 V4):** keep a standing 3 LH + 3 peer set; before each
   gate run, `tofu`/`xe` reverts each VM to its clean snapshot. Periodic **golden rebuild**
   bounds drift; the **release** gate does a full golden rebuild + the 3 LH + 9 peer envelope.

## 8. Phase 5 — Forgejo + self-hosted CI  🔲 DS-3 / DS-4

```bash
# Forgejo (git forge + Actions) in podman on the control host
podman volume create forgejo-data
podman run -d --name forgejo -p 3000:3000 -p 2222:22 \
  -v forgejo-data:/data --restart unless-stopped \
  codeberg.org/forgejo/forgejo:9
```

1. First-run setup → create org, **pull-mirror** of `github.com/matthewmackes/magic-mesh`
   (GitHub stays canonical, §10 / DS-3; Forgejo mirrors it for CI).
2. Register a **Forgejo Actions runner** (podman or binary) on the control host (and
   optionally a runner VM per XCP host for the real-VM gate).
3. **Port `ci.yml`** to `.forgejo/workflows/` (Actions-compatible YAML; minimal change,
   DS-4). The gate body is the single `install-helpers/verify-gates.sh` (PROCESS-1), so CI
   and the local pre-commit hook run the identical gate.
4. Multi-node jobs (V4) run on a runner that can reach XO → spins the snapshot-reset mesh.

## 9. Phase 6 — ephemeral container mesh (local fast loop)  🔲 DS-6

`install-helpers/nebula-test-node.Containerfile` already exists. A helper brings up an
N-node Nebula mesh in **podman** on the control host for quick multi-node *logic* checks
**before** the real-VM gate (the gate of record stays real VMs, §10 V4):

```bash
install-helpers/container-mesh.sh up 3      # 3-node throwaway mesh   (DS-6, to write)
install-helpers/container-mesh.sh down
```

## 10. Phase 7 — mesh-native secrets  🔲 DS-8

End-state: **etcd + age over Nebula** serves CI/IaC secrets (XO token, dom0 creds, the
release GPG key) — D-W1 mesh-tooling-first, no external Vault. A `get-secret <path>` helper
on runners decrypts via an age key delivered over the overlay; **plaintext never lands in
the repo, argv, or env files**. Bootstrap (before etcd exists): a SOPS/age-encrypted file on
the control host, swapped for the etcd backend once the mesh is up.

## 11. Security / hardening (bootstrap → end-state)

- Rotate the shared dom0 `root` password; prefer **SSH key auth** (Phase 1) + disable
  password SSH on the dom0s once keys are in.
- Move every secret to DS-8 (etcd+age). No plaintext credential ever lives in this repo;
  the dom0 password + XO token are supplied from the secrets store at rebuild time and
  rotated immediately after a fresh build.
- XO admin token is sensitive (full pool control) — store via DS-8, rotate on staff change.
- The environment is airgapped dev; GitHub remains the only outbound dependency (canonical
  remote + the release pipeline). A fleet outage never loses the repo.

## 12. DS-task → phase map + current status

| DS task | Phase | What | Status |
|---|---|---|---|
| DS-10 | §1–§12 (this doc) | Research + runbook (XO needed, provider choice, layout) | ✅ this doc |
| DS-1 | 5–6 | OpenTofu + XO provider VM lifecycle | 🔧 XO up, both pools connected, `tofu plan` proven; full `apply` blocked on DS-5 golden |
| DS-2 | 7 | Ansible node config | 🔲 |
| DS-3 | 8 | Forgejo + runners, pull-mirror | 🔲 |
| DS-4 | 8 | Port `ci.yml` → Forgejo Actions | 🔲 |
| DS-5 | 7 | Snapshot-reset pool + golden, V4 gate | 🔲 |
| DS-6 | 9 | Ephemeral container mesh | 🔲 |
| DS-7 | (GUI) | Pixel-diff visual harness (B8) | 🔲 |
| DS-8 | 10 | Mesh-native secrets (etcd+age) | 🔲 |
| DS-9 | 4/8 | IaC/non-Rust linters in the gate | 🔲 |

## 13. Rebuild verification checklist (done = ✅ on a fresh build)

- [ ] Both XCP hosts answer `xe host-list` from the control host.
- [ ] XO UI reachable; both pools connected green; an API token issued.
- [ ] `tofu apply` provisions and `tofu destroy` removes a VM on each pool.
- [ ] An Ansible run brings a fresh clone to an enrolled mesh node; re-run is a no-op.
- [ ] Forgejo pull-mirrors GitHub; a runner takes `verify-gates.sh` to green.
- [ ] A 3 LH + 3 peer real-VM mesh spins from snapshots and passes a mesh acceptance test.
- [ ] `get-secret xo/api-token` returns the token with no plaintext on disk/argv.

---

*Sources for the install paths: OpenTofu RPM repo (opentofu.org/docs/intro/install/rpm),
the `vatesfr`/`terra-farm` xenorchestra Terraform providers (registry.terraform.io), and
XO CE container images (hub.docker.com/r/ezka77/xen-orchestra-ce). Verified current
2026-06-21.*

# Control-VM Tofu root — DEVOPS-AUTOMATION-REBUILD (DAR-12/13/14)

The deploy unit of the per-mesh DevOps backoffice (design:
`docs/design/devops-automation-rebuild.md` §2.1). This root creates **ONE**
dedicated XCP-NG control VM on the **founding dom0**, enrolled as a full mesh
peer (`--role server`), that carries the IaC/CI/build/DR control plane. It
supersedes the hand-built LAN control node — the backoffice now comes along when
founding a new Nebula.

## What it builds

- A single `xenserver_vm` cloned from the canonical golden template
  **`MDE-VM-golden`** (§2.8 GAP 5 — the name the template-builder produces and the
  live build VMs clone; the `-tc` variant is retired). The same baked-toolchain
  template the farm uses.
- Sized by `backoffice_tier`: **minimal** = 4 vCPU / 8 GiB / 60 GiB; **full** =
  8 / 16 / 120.
- Seeded with `cloud-init/control-vm.yaml.tftpl`: the NM static-IP keyfile fix
  (reused verbatim from `build-vm.yaml.tftpl`), a tier-aware `/etc/mackesd/site.yml`,
  and a runcmd that enrolls + self-keys + converges the tier's units.

## Pattern reuse

- **Provider:** ONE aliased `xenserver` provider (`xenserver.founder`) aimed at the
  founding dom0 — the XAPI-native, no-XO pattern proven in `../xen-xapi/`.
- **Cloud-init seam:** the 0.2.x `xenserver_vm` resource has no first-class
  `cloud_config` (that is the `xenorchestra` provider). Its only injection point is
  `other_config`, so the rendered user-data is delivered via
  `other_config["vm-data/user-data"]` (XCP-NG NoCloud-via-other-config). The 0.2.x
  create-with-seed path is the **CONTROLVM-9** risk — verify the seed actually
  attaches on a throwaway clone (live, operator-gated) before relying on it.

## Secret handling (lock 8 — verified, not asserted)

- The XAPI password and the join token are the only credentials. Both are
  **sensitive** vars sourced from the mesh secret store via `env.sh`
  (`TF_VAR_xapi_password` / `TF_VAR_join_token`) — **NEVER a literal** in any
  tracked `.tf`/`.tftpl`/tfvars.
- **NO** `write_files` entry for `/root/.mcnf-age-key` and **NO** templated unseal
  passphrase. The VM **mints its own** age identity at first boot
  (`mcnf-secret.sh init-self`); the private key never leaves the VM and never
  enters tofu state. An operator/leader `mcnf-secret.sh reseal-to <recipient>`
  grants it read access.
- Acceptance (§5): a `grep` of the produced `terraform.tfstate` shows NO age
  private key, NO unseal passphrase, NO plaintext provider token. The `other_config`
  map is marked sensitive so the token-bearing rendered user-data is redacted from
  plan/CLI output.

## State backend (no fixed center, no literal address)

State lives at `/state/control-vm` in the etcd-backed http state service. The
backend block carries ONLY the lock methods — the address is supplied per-mesh at
init (OpenTofu backends can't interpolate vars, so a literal `.192` would not come
along and can't be shadowed):

```bash
gen-backend-config.sh --control-ip <overlay-ip>   # writes control-vm.backend.hcl (DAR-8)
tofu init -backend-config=control-vm.backend.hcl
```

## Use

```bash
cp env.sh.example env.sh           # sources both secrets from the store
source ./env.sh
# tfvars are GENERATED from mesh identity (gen-tfvars.sh, DAR-33), never hand-edited.
tofu init -backend-config=control-vm.backend.hcl
tofu validate
tofu plan                          # 1 xenserver_vm to add, 0 to destroy
```

`tofu apply` (creating the real VM) is **LIVE-GATED** (operator-run via
`control-vm-provision.sh`, DAR-19) — this root is the HCL only.

`env.sh`, `*.backend.hcl`, `*.auto.tfvars`, state, and `.terraform/` are gitignored.

# infra/tofu — the build farm as code (DEVOPS-SUBSTRATE)

> **⚠️ DEPRECATED (2026-06-22, DATACENTER-1) — do NOT `tofu apply` here.** The farm
> was cut over to the **XAPI-native, no-XO** config in **`xen-xapi/`** (3 aliased
> providers, all 3 build VMs imported, `0-destroy` plan). This `xenorchestra`
> config still has state for the same VMs; applying it would conflict with the new
> one. Kept for reference/rollback only until its state is removed. Use `xen-xapi/`.

Declares the MCNF build farm against live **Xen Orchestra** via the
`vatesfr/xenorchestra` OpenTofu provider. This is the durable replacement for
the `install-helpers/*xcp*` bash provisioners: XO drives XAPI, so the whole
`xe`-over-ssh quoting / SR-naming / template class of bugs (see `docs/farm.md`)
disappears. Proven against live XO (`tofu plan` resolves both pools, networks,
and SRs).

## What it manages

- One Fedora build VM per XCP pool, on a deterministic LAN IP:
  - `XEN-HOME-SERVICES` (172.20.0.9) → build VM `172.20.0.50`
  - `KVM-XCP1` (172.20.145.193) → build VM `172.20.0.51`
- The cloud-init seed carries the **proven NM-keyfile network fix** (the dark-VM
  root cause from `docs/farm.md`): a static-IP NetworkManager keyfile written
  directly, since cloud-init's netplan-v2 → NM render is broken on Fedora+Xen.

The VM resources are **inert until `golden_template_name` is set** (XCP-2 / DS-5:
the `MDE-VM-golden` template the VMs clone from). With it empty, `tofu plan`
proves XO connectivity and validates config but creates nothing. Set it once the
golden template exists to enable `tofu apply`.

## Secrets

The XO API **token is never in the repo**. It lives `0600` at
`/root/.mcnf-xo-token`, minted with `install-helpers/xo-mint-token.sh` (a
dedicated, named, revocable `opentofu-fam` token). `url` + `insecure` are
non-secret tofu vars. State is gitignored (it can hold sensitive values).

## Run

```sh
# one-time: mint the token (idempotent — re-run to rotate)
../../install-helpers/xo-mint-token.sh

# every session: source the env, then drive tofu
cp env.sh.example env.sh        # env.sh is gitignored
source ./env.sh
tofu init
tofu plan                       # reads live XO; 0 changes until the golden template is set
```

To rotate the token: revoke the old one in the XO UI (Settings → Tokens), re-run
`xo-mint-token.sh`, re-`source ./env.sh`.

## Layout

| file | role |
|---|---|
| `versions.tf`   | provider source + version pin |
| `providers.tf`  | XO connection (token from `$XOA_TOKEN`) |
| `variables.tf`  | url/insecure, golden template, VM sizing, network |
| `main.tf`       | the fleet map + live-XO data reads (pool/network/SR) |
| `build-vms.tf`  | the build-VM resources + golden-template data source (gated) |
| `cloud-init/build-vm.yaml.tftpl` | cloud-init seed w/ the NM-keyfile fix |
| `outputs.tf`    | resolved XO IDs (connectivity proof) |

## Next (to enable `apply`)

1. Build the XCP-2 golden template (`MDE-VM-golden`); set `golden_template_name`.
2. `tofu apply` → the two build VMs, then `install-helpers/farm.sh toolchain <ip>`
   (or the Ansible play in `infra/ansible/`) for the Rust toolchain.

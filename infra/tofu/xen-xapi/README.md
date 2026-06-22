# Xen XAPI-native Tofu (DATACENTER-1 — migration prototype)

The no-fixed-center replacement for the XO-backed Xen IaC: the `xenserver`
provider talks **XAPI directly** to a pool master, so there is no central Xen
Orchestra to lose. Isolated from `../` (the live `xenorchestra`-managed farm) —
its own directory + state — so nothing here can disturb the working farm.

## Status

**Connectivity PROVEN (2026-06-22).** A read-only `tofu plan` against the
`172.20.145.193` (KVM-XCP1) dom0 authenticated over XAPI and read the live pool:
1 host (`KVM-XCP1`, 2 resident VMs), 4 SRs — with **no XO process**. The
XAPI-direct path works on XCP-ng 8.3.

## Risk findings (for the cutover decision)

- The only XAPI-native provider, `xenserver/xenserver`, is **early-stage (0.2.2)**
  — far less mature than `vatesfr/xenorchestra`. It exposes the resources the farm
  needs (`xenserver_vm`, `xenserver_sr{,_nfs,_smb}`, `xenserver_vdi`,
  `xenserver_network_vlan`, `xenserver_snapshot`, `xenserver_pool`) but its long-term
  stability is unproven here.
- **Import parity** of the live `.50/.51/.52` build VMs is the real gate before any
  cutover of `infra/tofu/` — not yet attempted (next increment).

## Use

```bash
cp env.sh.example env.sh          # reads /root/.mcnf-xapi-cred (0600, off-repo)
source ./env.sh
tofu plan                         # read-only: lists hosts + SRs
```

`env.sh`/state/`.terraform/` are gitignored. The XAPI password is **never** in the
repo — only `TF_VAR_xapi_password` from the off-repo `0600` file.

## Next steps (DATACENTER-1)

1. Prove the resource path: create + destroy a throwaway test VM via `xenserver_vm`
   on the test bed (not the live farm).
2. Import an existing VM → confirm a clean plan (import parity).
3. Only then plan the `infra/tofu/` cutover off `xenorchestra`.

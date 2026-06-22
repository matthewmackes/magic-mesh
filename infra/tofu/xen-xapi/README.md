# Xen XAPI-native Tofu (DATACENTER-1 — migration prototype)

The no-fixed-center replacement for the XO-backed Xen IaC: the `xenserver`
provider talks **XAPI directly** to a pool master, so there is no central Xen
Orchestra to lose. Isolated from `../` (the live `xenorchestra`-managed farm) —
its own directory + state — so nothing here can disturb the working farm.

## Status

**Read + write PROVEN end-to-end (2026-06-22), no XO.** Against the
`172.20.145.193` (KVM-XCP1) dom0, over pure XAPI:
- **Read:** authenticated + listed the live pool — 1 host (2 resident VMs), 4 SRs.
- **Write:** `tofu apply` cloned `MDE-VM-golden` into a throwaway VM
  (uuid `57c7d644-…`); `tofu destroy` tore it down cleanly; the host returned to its
  original 2 VMs.

The XAPI-native provider can drive the full VM lifecycle on XCP-ng 8.3 with no Xen
Orchestra in the path — the no-fixed-center hypothesis for the Xen IaC holds.

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

1. ~~Prove the resource path: create + destroy a throwaway test VM.~~ **DONE.**
2. Import an existing VM (e.g. a `.50/.51/.52` build VM) → confirm a clean plan
   (import parity). This is the real gate before cutting over the live farm.
3. Only then plan the `infra/tofu/` cutover off `xenorchestra`.

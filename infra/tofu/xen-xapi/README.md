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

## Import-parity (PROVEN 2026-06-22 — the cutover gate)

Imported the live `mcnf-build-51` (uuid `7a6753c7…`) into a throwaway
`xenserver_vm` and planned: **`0 to add, 0 to destroy`** — no recreate, no disk/
CPU/memory/boot/network change. The only residual is two metadata fields the 0.2.x
provider can't round-trip (`check_ip_timeout`, computed `default_ip`); an apply is a
**no-op on the actual VM**. (Probe was `state rm`'d after; the live VM stayed
`running`, untouched.) **→ the cutover is safe.**

The recipe for an adopted (already-provisioned) build VM to plan clean:

```hcl
resource "xenserver_vm" "build" {
  name_label        = "mcnf-build-NN"
  template_name     = "MDE-VM-golden"
  static_mem_max    = <bytes>
  vcpus             = <n>
  network_interface = [{ device = "0", network_uuid = "<pool-network-uuid>" }]
  lifecycle {
    ignore_changes = [
      hard_drive, template_name, boot_mode, boot_order, cores_per_socket,
      dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom,
    ]
  }
}
# then: tofu import xenserver_vm.build <vm-uuid>
```

## Next steps (DATACENTER-1)

1. ~~Resource path (create/destroy).~~ **DONE.**  2. ~~Import-parity clean plan.~~ **DONE.**
3. **Full cutover** (remaining): the live farm spans **3 standalone pools** (.9/.193/.165),
   and the `xenserver` provider is **single-pool** — so the new config needs **3 aliased
   providers** (one per dom0 XAPI endpoint), a `xenserver_vm` per pool (recipe above),
   import all 3, confirm clean, then promote over `../` (retire `xenorchestra`).

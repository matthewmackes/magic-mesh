# The build farm: an ELASTIC fleet of Fedora build VMs, one dom0 per XCP pool.
# FARM-AUTOSCALE (docs/design/farm-autoscale.md): each dom0 runs EITHER one
# hardware-maxing `big` VM OR `small_count` standard `small` VMs OR nothing
# (`off` = scale-to-zero) — a mutually-exclusive shape (L4) the autoscaler picks
# from live build demand. The shape per dom0 comes from var.shape; tofu converges
# the VM set to match. Build VMs clone MDE-VM-golden (XCP-2) on a deterministic
# LAN IP so install-helpers/xcp-build.sh always reaches them.
locals {
  # Per-dom0 substrate (cold facts from the design doc's dom0 table). `ip_base` is
  # the FIRST build-VM IP on that dom0 (the big VM and small-0 both land here, but
  # never at the same time — shapes are mutually exclusive). `small` VMs step the
  # last octet +10 each (small-0=base, small-1=base+10, …). small_count is capped
  # at 4, so each dom0 needs a 40-wide IP lane; the ip_bases are spaced 40 apart
  # (.50→.80, .90→.120, .130→.160) so NO dom0's small lane overlaps another's.
  # `big_vcpus`/`big_mem_gib` size the whole-host `big` VM (≈ the dom0's capacity,
  # leaving the hypervisor headroom); `small` VMs use the global build_* defaults.
  dom0 = {
    "xen-home-services" = {
      pool_name   = "XEN-HOME-SERVICES" # 172.20.0.9 — 4c / 24 GiB
      ip_base     = "172.20.0.50"       # lane .50–.80 (small-0 keeps the legacy mcnf-build-50 IP)
      big_name    = "mcnf-build-big-50"
      small_name  = "mcnf-build-50"
      big_vcpus   = 3 # ~whole 4-core host, 1 core for dom0
      big_mem_gib = 18
    }
    "kvm-xcp1" = {
      pool_name   = "KVM-XCP1"    # 172.20.145.193 — 4c / 23 GiB
      ip_base     = "172.20.0.90" # lane .90–.120
      big_name    = "mcnf-build-big-51"
      small_name  = "mcnf-build-51"
      big_vcpus   = 3
      big_mem_gib = 18
    }
    "xen-bigboy" = {
      pool_name   = "XEN-BIGBOY"   # 172.20.145.165 — 12c / 32 GiB / 398 GiB SR
      ip_base     = "172.20.0.130" # lane .130–.160
      big_name    = "mcnf-build-big-52"
      small_name  = "mcnf-build-52"
      big_vcpus   = 10 # ~whole 12-core BigBoy, 2 cores for dom0
      big_mem_gib = 26
    }
  }

  # Split each dom0's ip_base into the first-3-octets prefix + the last octet, so
  # `small` VMs can step the last octet (+10 each) for distinct LAN IPs.
  ip_prefix3    = { for dk, d in local.dom0 : dk => join(".", slice(split(".", d.ip_base), 0, 3)) }
  ip_last_octet = { for dk, d in local.dom0 : dk => tonumber(element(split(".", d.ip_base), 3)) }

  # Pure shape→VM-set expansion (L1/L4). For each dom0, the chosen shape yields a
  # list of build-VM specs:
  #   big   → ONE VM at the dom0's whole-host size (big_vcpus / big_mem_gib)
  #   small → small_count VMs at the standard build_* size, IPs ip_base, +10, +20…
  #   off   → none (scale-to-zero)
  # The result is a flat map keyed `<dom0>` (big) or `<dom0>-<n>` (small) so the
  # for_each resources stay stable as the count grows/shrinks.
  build_vm_specs = merge([
    for dk, d in local.dom0 : (
      lookup(var.shape, dk, "off") == "big" ? {
        (dk) = {
          dom0_key = dk
          name     = d.big_name
          ip_cidr  = "${d.ip_base}/16"
          vcpus    = d.big_vcpus
          mem_gib  = d.big_mem_gib
        }
        } : lookup(var.shape, dk, "off") == "small" ? {
        for i in range(lookup(var.small_count, dk, 1)) :
        "${dk}-${i}" => {
          dom0_key = dk
          # Suffix the 2nd+ small VM's name/IP so N smalls coexist on one dom0
          # without colliding (XO looks VMs up by name_label, IP must be unique).
          # IP = ip_base with +10 per extra small on its last octet (e.g. .50→.60);
          # ip_bases are spaced 40 apart so a dom0's 4-wide lane can't reach the
          # next dom0's lane (see the local.dom0 comment above).
          name    = i == 0 ? d.small_name : "${d.small_name}-${i}"
          ip_cidr = "${local.ip_prefix3[dk]}.${local.ip_last_octet[dk] + i * 10}/16"
          vcpus   = var.build_vcpus
          mem_gib = var.build_memory_gib
        }
      } : {}
    )
  ]...)

  # Build VMs are only declared once the golden template exists (XCP-2). Until
  # then this map is empty, so plan reads live XO (proving the token works) and
  # reports 0 changes rather than failing on a missing template. Per-VM `vcpus`/
  # `mem_gib` are the shape-derived sizes; per-VM overrides in var.vm_overrides
  # win (a one-off bigger node) — "optimum use of available hardware". dom0_key is
  # re-applied AFTER the merge so an override can't clobber it (it keys the live
  # pool/network/SR data sources — a bad value would crash the whole plan).
  active_build_vms = var.golden_template_name == "" ? {} : {
    for k, v in local.build_vm_specs :
    k => merge(v, lookup(var.vm_overrides, k, {}), { dom0_key = v.dom0_key })
  }
}

# --- Live XO reads (these resolve on plan → proof the token + provider work) ---

data "xenorchestra_pool" "p" {
  for_each   = local.dom0
  name_label = each.value.pool_name
}

data "xenorchestra_network" "lan" {
  for_each   = local.dom0
  name_label = "Pool-wide network associated with eth0"
  pool_id    = data.xenorchestra_pool.p[each.key].id
}

data "xenorchestra_sr" "local" {
  for_each   = local.dom0
  name_label = "Local storage"
  pool_id    = data.xenorchestra_pool.p[each.key].id
}

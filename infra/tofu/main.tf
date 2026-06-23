# The build farm: one Fedora build VM per XCP pool, on a deterministic LAN IP so
# install-helpers/xcp-build.sh always reaches it. Mirrors the fleet that the
# stopgap farm.sh drives — now declared as code.
locals {
  # Per-VM `vcpus`/`mem_gib` are OPTIONAL overrides of the global var defaults —
  # bigger hosts get bigger build nodes ("optimum use of available hardware").
  build_vms = {
    "xen-home-services" = {
      pool_name = "XEN-HOME-SERVICES" # 172.20.0.9 — 4c / 24 GiB
      ip_cidr   = "172.20.0.50/16"
      name      = "mcnf-build-50" # MUST be unique across pools: XO sees all,
    }                             # and the provider looks VMs up by name_label.
    "kvm-xcp1" = {
      pool_name = "KVM-XCP1" # 172.20.145.193 — 4c / 23 GiB
      ip_cidr   = "172.20.0.51/16"
      name      = "mcnf-build-51"
    }
    "xen-bigboy" = {
      pool_name = "XEN-BIGBOY" # 172.20.145.165 — 12c / 32 GiB / 398 GiB SR
      ip_cidr   = "172.20.0.52/16"
      name      = "mcnf-build-52"
      vcpus     = 8  # leverage the 12 cores (leaves headroom for more nodes)
      mem_gib   = 24
    }
  }

  # Build VMs are only declared once the golden template exists (XCP-2). Until
  # then this map is empty, so plan reads live XO (proving the token works) and
  # reports 0 changes rather than failing on a missing template.
  active_build_vms = var.golden_template_name == "" ? {} : local.build_vms
}

# --- Live XO reads (these resolve on plan → proof the token + provider work) ---

data "xenorchestra_pool" "p" {
  for_each   = local.build_vms
  name_label = each.value.pool_name
}

data "xenorchestra_network" "lan" {
  for_each   = local.build_vms
  name_label = "Pool-wide network associated with eth0"
  pool_id    = data.xenorchestra_pool.p[each.key].id
}

data "xenorchestra_sr" "local" {
  for_each   = local.build_vms
  name_label = "Local storage"
  pool_id    = data.xenorchestra_pool.p[each.key].id
}

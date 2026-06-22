# The build farm: one Fedora build VM per XCP pool, on a deterministic LAN IP so
# install-helpers/xcp-build.sh always reaches it. Mirrors the fleet that the
# stopgap farm.sh drives — now declared as code.
locals {
  build_vms = {
    "xen-home-services" = {
      pool_name = "XEN-HOME-SERVICES" # 172.20.0.9
      ip_cidr   = "172.20.0.50/16"
    }
    "kvm-xcp1" = {
      pool_name = "KVM-XCP1" # 172.20.145.193
      ip_cidr   = "172.20.0.51/16"
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

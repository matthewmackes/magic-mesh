# Proof outputs — these print the live XO object IDs the data sources resolved,
# confirming the token + provider reached both pools.
output "pools" {
  description = "Resolved pool IDs per host."
  value       = { for k, p in data.xenorchestra_pool.p : k => p.id }
}

output "lan_networks" {
  description = "Resolved LAN network IDs the build VMs attach to."
  value       = { for k, n in data.xenorchestra_network.lan : k => n.id }
}

output "local_srs" {
  description = "Resolved local SR IDs the build disks land on."
  value       = { for k, s in data.xenorchestra_sr.local : k => s.id }
}

output "build_vms_managed" {
  description = "Build VMs tofu manages (empty until golden_template_name is set — XCP-2)."
  value       = keys(local.active_build_vms)
}

# FARM-AUTOSCALE — the decided topology: each managed VM with its dom0, name, IP,
# and shape-derived size. Lets the autoscaler / operator read back what the shape
# vars produced (and proves the big-XOR-small-XOR-off expansion).
output "build_topology" {
  description = "Per-VM shape topology (dom0 · name · ip · vcpus · mem_gib)."
  value = {
    for k, v in local.active_build_vms : k => {
      dom0    = v.dom0_key
      name    = v.name
      ip_cidr = v.ip_cidr
      vcpus   = try(tonumber(v.vcpus), var.build_vcpus)
      mem_gib = try(tonumber(v.mem_gib), var.build_memory_gib)
    }
  }
}

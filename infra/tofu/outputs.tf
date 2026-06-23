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

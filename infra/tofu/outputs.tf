# DS-1 — outputs feed the Ansible inventory (DS-2) and the mesh gate (DS-5).
output "node_ips" {
  description = "name → first management IP of each test node (once tools report it)."
  value       = { for name, vm in xenorchestra_vm.node : name => try(vm.ipv4_addresses[0], "pending") }
}

output "lighthouse_names" {
  value = [for n in local.nodes : n.name if n.role == "lighthouse"]
}

output "peer_names" {
  value = [for n in local.nodes : n.name if n.role == "peer"]
}

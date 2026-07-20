output "network_id" {
  description = "The libvirt network id (consumed by each VM's interface)."
  value       = libvirt_network.this.id
}

output "network_name" {
  description = "The libvirt network name."
  value       = libvirt_network.this.name
}

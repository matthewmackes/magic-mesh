output "domain_id" {
  description = "The libvirt domain (VM) id."
  value       = libvirt_domain.this.id
}

output "domain_name" {
  description = "The libvirt domain (VM) name."
  value       = libvirt_domain.this.name
}

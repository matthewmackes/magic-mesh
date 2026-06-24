output "dhcp_leases" {
  description = "Live DHCP leases polled from EdgeOS: ip => \"mac|expiry|hostname\"."
  value       = data.external.dhcp_leases.result
}

output "lease_count" {
  description = "Number of active DHCP leases at poll time."
  value       = length(data.external.dhcp_leases.result)
}

output "managed_reservations" {
  description = "The static-mappings tofu manages (the converged desired state)."
  value       = var.static_mappings
}

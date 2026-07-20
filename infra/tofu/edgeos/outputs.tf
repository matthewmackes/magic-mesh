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

# WL-RUN-006 — which appliance this instance manages + its per-appliance state key.
# `gateway` is grandfathered at `state/edgeos`; any other id keys `state/router/<mac>`.
output "appliance" {
  description = "The router appliance this instance manages: id, host, and state key."
  value = {
    id        = var.appliance_id
    host      = var.edgeos_host
    state_key = var.appliance_id == "gateway" ? "state/edgeos" : "state/router/${lower(replace(var.appliance_id, ":", "-"))}"
  }
}

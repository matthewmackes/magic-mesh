# DAR-12 — the control VM's identity for the come-along orchestrator.
output "control_vm_uuid" {
  description = "UUID of the created control VM (for snapshot/destroy/adopt)."
  value       = xenserver_vm.control.uuid
}

# The LAN IP the provider reports. The DURABLE backoffice endpoint is the OVERLAY
# IP minted at `mackesd join` (discovered via `mackesd peers --json` after boot,
# NOT known at tofu plan time); default_ip is the LAN seed address only.
output "control_vm_lan_ip" {
  description = "LAN IP the dom0 reports for the control VM (seed; the overlay IP is discovered post-join)."
  value       = xenserver_vm.control.default_ip
}

output "backoffice_tier" {
  description = "Tier this control VM was provisioned for (drives the enabled unit set)."
  value       = var.backoffice_tier
}

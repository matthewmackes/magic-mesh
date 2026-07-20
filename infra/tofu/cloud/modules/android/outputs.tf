output "domain_id" {
  description = "The Cuttlefish L1 VM (libvirt domain) id."
  value       = libvirt_domain.this.id
}

output "domain_name" {
  description = "The Cuttlefish L1 VM (libvirt domain) name."
  value       = libvirt_domain.this.name
}

output "console_proto" {
  description = <<-EOT
    The console protocol the Android screen is reached by. Cuttlefish serves the
    guest screen over an in-guest VNC server (`cvd --start_vnc_server`), so the
    handle is `vnc` (mesh-tunneled over the overlay), not a libvirt SPICE display.
  EOT
  value       = "vnc"
}

output "console_note" {
  description = "How the Android console is reached (for the surface's attach hint)."
  value = format(
    "cvd --start_vnc_server inside L1 VM %s (%d vcpu / %d MiB / %d GiB, nested-virt host-passthrough)%s",
    libvirt_domain.this.name,
    local.vcpu,
    local.memory_mb,
    local.disk_gb,
    var.network_isolation ? "; isolated-network requested (reserved)" : ""
  )
}

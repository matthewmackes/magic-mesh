output "network_id" {
  description = "The libvirt network id the workloads attach to."
  value       = module.network.network_id
}

output "instances" {
  description = <<-EOT
    The provisioned VM-family roster (desktop/service/app): name => { id, name,
    delivery_type, memory_mb, vcpu }. The mackesd cloud worker + the Workloads
    surface read this to render the instance table (mirrors the neutral
    CloudInstance shape).
  EOT
  value = {
    for name, vm in module.vm : name => {
      id            = vm.domain_id
      name          = name
      delivery_type = var.vms[name].delivery_type
      memory_mb     = var.vms[name].memory_mb
      vcpu          = var.vms[name].vcpu
    }
  }
}

output "android_consoles" {
  description = <<-EOT
    The provisioned Android (Cuttlefish) L1 VMs: name => { id, name, console_proto,
    console_note }. The Android screen itself is served by `cvd --start_vnc_server`
    inside the guest; this exposes the L1 domain + how its console is reached
    (WL-ARCH-006 U12).
  EOT
  value = {
    for name, a in module.android : name => {
      id            = a.domain_id
      name          = name
      console_proto = a.console_proto
      console_note  = a.console_note
    }
  }
}

output "containers" {
  description = <<-EOT
    The declared service-container workloads: name => { name, image, rootless,
    quadlet_unit }. The Ansible container-host role reads this to render + install a
    rootless Quadlet `.container` systemd unit (WL-ARCH-006 U12).
  EOT
  value = {
    for name, c in module.container : name => {
      name         = c.name
      image        = c.image
      rootless     = c.rootless
      quadlet_unit = c.quadlet_unit
    }
  }
}

output "network_id" {
  description = "The libvirt network id the VMs attach to."
  value       = module.network.network_id
}

output "instances" {
  description = <<-EOT
    The provisioned VM roster: name => { id, name, memory_mb, vcpu }. The mackesd
    cloud worker + the recreated IaC surface read this to render the instance
    table (mirrors the neutral CloudInstance shape).
  EOT
  value = {
    for name, vm in module.vm : name => {
      id        = vm.domain_id
      name      = name
      memory_mb = var.vms[name].memory_mb
      vcpu      = var.vms[name].vcpu
    }
  }
}
